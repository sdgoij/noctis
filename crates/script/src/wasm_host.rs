//! The native WebAssembly script host (feature `slag`).
//!
//! A `.wasm` module can be a first-class script alongside a `.js` one. The engine
//! feeds every script through one channel (a source string), so a WebAssembly
//! module travels as a text descriptor: [`PREFIX`] (`"wasm:"`) followed by the
//! hex bytes, produced by [`encode`] and recognised by [`is_wasm_source`]. A
//! project registers the module bytes once (`Script::from_source(encode(&bytes))`
//! or a `ScriptAsset`), and [`WasmHost`] compiles and instantiates them.
//!
//! ## The module ABI
//!
//! The module exports any of `on_start`, `on_update`, `on_destroy`, each typed
//! `(i32, f32) -> ()` — an opaque entity handle and the delta time. It may
//! import the engine API from the module `"noctis"`:
//!
//! | import | signature | effect |
//! |---|---|---|
//! | `set_position` | `(i32, f32, f32, f32) -> ()` | set local translation |
//! | `translate` | `(i32, f32, f32, f32) -> ()` | offset local translation |
//! | `set_rotation_euler` | `(i32, f32, f32, f32) -> ()` | set rotation (radians) |
//! | `set_scale` | `(i32, f32, f32, f32) -> ()` | set local scale |
//! | `destroy` | `(i32) -> ()` | despawn an entity |
//! | `log` | `(i32, i32) -> ()` | log `memory[ptr..ptr+len]` |
//! | `spawn` | `(i32, i32) -> i32` | spawn an entity named `memory[ptr..ptr+len]` |
//! | `find_entity` | `(i32, i32) -> i32` | handle of the entity with that name |
//! | `time_delta` | `() -> f32` | seconds since the last frame |
//! | `time_elapsed` | `() -> f32` | total elapsed seconds |
//!
//! Handles are `i32` indices into a per-call table: handle `0` is the script's
//! own entity, and handles returned by `spawn`/`find_entity` are valid for the
//! rest of the call. String arguments read the module's exported `memory`; a
//! module that never calls a string import need not export one.
//!
//! ## How a call runs
//!
//! Slag's engine runs a module through a *resumable* driver: an import call parks
//! the run as a [`RunProgress::Host`] request, the host services it with the
//! store unborrowed, and [`Store::resume`] continues. That mirrors the
//! JavaScript backend exactly, so this host uses the shared [`Bridge`]: it
//! snapshots the world, queues [`ScriptCommand`]s while the module runs, then
//! replays them through [`ScriptApi`](crate::api::ScriptApi).

use std::collections::HashMap;

use math::vec3;
use slag::wasm::exec::{HostRequest, RunProgress};
use slag::wasm::module::ImportDesc;
use slag::wasm::types::{FuncType, ValType};
use slag::wasm::{ExternVal, Store, Value, decode, validate};

use crate::api::{EntityId, ScriptCommand, ScriptContext};
use crate::bridge::{Bridge, replay};
use crate::host::{ModuleId, PendingApi, ScriptHost};

/// The module name a WebAssembly script imports the engine API from.
pub const IMPORT_MODULE: &str = "noctis";
/// The prefix that marks a script source as a WebAssembly module descriptor.
pub const PREFIX: &str = "wasm:";

/// Encode a WebAssembly module as script source: `PREFIX` plus the hex bytes.
///
/// The engine's script channel is text, so a binary module travels hex-encoded.
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(PREFIX.len() + bytes.len() * 2);
    out.push_str(PREFIX);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize]);
        out.push(HEX[(byte & 0x0f) as usize]);
    }
    out
}

/// `true` if `source` is a WebAssembly module descriptor.
pub fn is_wasm_source(source: &str) -> bool {
    source.starts_with(PREFIX)
}

const HEX: [char; 16] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
];

/// Decode a WebAssembly module descriptor produced by [`encode`].
pub fn decode_source(source: &str) -> Option<Vec<u8>> {
    let hex = source.strip_prefix(PREFIX)?;
    if hex.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    let digits = hex.as_bytes();
    for pair in digits.as_chunks::<2>().0 {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        bytes.push(((high << 4) | low) as u8);
    }
    Some(bytes)
}

/// One engine function a WebAssembly script may import.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Abi {
    SetPosition,
    Translate,
    SetRotationEuler,
    SetScale,
    Destroy,
    Log,
    Spawn,
    FindEntity,
    TimeDelta,
    TimeElapsed,
}

impl Abi {
    /// The engine function `name`, if the ABI defines it.
    fn for_name(name: &str) -> Option<Abi> {
        Some(match name {
            "set_position" => Abi::SetPosition,
            "translate" => Abi::Translate,
            "set_rotation_euler" => Abi::SetRotationEuler,
            "set_scale" => Abi::SetScale,
            "destroy" => Abi::Destroy,
            "log" => Abi::Log,
            "spawn" => Abi::Spawn,
            "find_entity" => Abi::FindEntity,
            "time_delta" => Abi::TimeDelta,
            "time_elapsed" => Abi::TimeElapsed,
            _ => return None,
        })
    }

    /// The signature the ABI fixes for this function.
    fn func_type(self) -> FuncType {
        use Abi::*;
        let (params, results): (Vec<ValType>, Vec<ValType>) = match self {
            SetPosition | Translate | SetRotationEuler | SetScale => (
                vec![ValType::I32, ValType::F32, ValType::F32, ValType::F32],
                vec![],
            ),
            Destroy => (vec![ValType::I32], vec![]),
            Log => (vec![ValType::I32, ValType::I32], vec![]),
            Spawn | FindEntity => (vec![ValType::I32, ValType::I32], vec![ValType::I32]),
            TimeDelta | TimeElapsed => (vec![], vec![ValType::F32]),
        };
        FuncType { params, results }
    }

    /// Service a call: mutate the bridge and return the results.
    #[allow(clippy::too_many_arguments)]
    fn apply(
        self,
        args: &[Value],
        bridge: &mut Bridge,
        handles: &mut Vec<EntityId>,
        memory: Option<&[u8]>,
    ) -> Vec<Value> {
        let handle = |index: usize| -> Option<EntityId> {
            let raw = i32_of(args.get(index).copied().unwrap_or(Value::I32(-1)));
            usize::try_from(raw)
                .ok()
                .and_then(|i| handles.get(i))
                .copied()
        };
        let vector = |base: usize| {
            vec3(
                f32_of(args, base),
                f32_of(args, base + 1),
                f32_of(args, base + 2),
            )
        };
        match self {
            Abi::SetPosition => {
                if let Some(id) = handle(0) {
                    bridge.set_position(id, vector(1));
                }
                Vec::new()
            }
            Abi::Translate => {
                if let Some(id) = handle(0) {
                    bridge.translate(id, vector(1));
                }
                Vec::new()
            }
            Abi::SetRotationEuler => {
                if let Some(id) = handle(0) {
                    bridge.set_rotation(id, vector(1));
                }
                Vec::new()
            }
            Abi::SetScale => {
                if let Some(id) = handle(0) {
                    bridge.set_scale(id, vector(1));
                }
                Vec::new()
            }
            Abi::Destroy => {
                if let Some(id) = handle(0) {
                    bridge.commands.push(ScriptCommand::Destroy { entity: id });
                }
                Vec::new()
            }
            Abi::Log => {
                if let Some(text) = memory_string(args, memory) {
                    bridge.commands.push(ScriptCommand::Log {
                        message: text,
                        entity: bridge.entity,
                    });
                }
                Vec::new()
            }
            Abi::Spawn => {
                let name = memory_string(args, memory);
                let id = bridge.spawn(name);
                handles.push(id);
                vec![Value::I32(handle_of(handles, id))]
            }
            Abi::FindEntity => {
                let found = memory_string(args, memory)
                    .and_then(|name| bridge.find(&name))
                    .map(|id| {
                        if let Some(existing) = handles.iter().position(|h| *h == id) {
                            existing
                        } else {
                            handles.push(id);
                            handles.len() - 1
                        }
                    });
                vec![Value::I32(found.map_or(-1, |index| index as i32))]
            }
            Abi::TimeDelta => vec![Value::F32(bridge.delta.to_bits())],
            Abi::TimeElapsed => vec![Value::F32(bridge.elapsed.to_bits())],
        }
    }
}

/// The handle of `id`, assigning a new one if it has none yet.
fn handle_of(handles: &mut Vec<EntityId>, id: EntityId) -> i32 {
    if let Some(index) = handles.iter().position(|h| *h == id) {
        return index as i32;
    }
    handles.push(id);
    (handles.len() - 1) as i32
}

fn f32_of(args: &[Value], index: usize) -> f32 {
    match args.get(index).copied() {
        Some(Value::F32(bits)) => f32::from_bits(bits),
        Some(Value::F64(bits)) => f64::from_bits(bits) as f32,
        _ => 0.0,
    }
}

fn i32_of(value: Value) -> i32 {
    match value {
        Value::I32(value) => value,
        Value::I64(value) => value as i32,
        _ => -1,
    }
}

/// Read the `(ptr, len)` string arguments against the module's memory.
fn memory_string(args: &[Value], memory: Option<&[u8]>) -> Option<String> {
    let memory = memory?;
    let raw = i32_of(args.first().copied()?);
    let ptr = usize::try_from(raw).ok()?;
    let len = usize::try_from(i32_of(args.get(1).copied()?)).ok()?;
    let bytes = memory.get(ptr..ptr.checked_add(len)?)?;
    Some(String::from_utf8_lossy(bytes).into_owned())
}

/// The bytes of a module instance's exported `memory`, if it exports one.
fn memory_bytes(store: &Store, instance: usize) -> Option<&[u8]> {
    match store.export(instance, "memory")? {
        ExternVal::Memory(cell) => store.memory_bytes(cell),
        _ => None,
    }
}

/// A native WebAssembly [`ScriptHost`].
///
/// Sources that are not WebAssembly descriptors are ignored (see
/// [`is_wasm_source`]); pair it with a JavaScript host through
/// [`EngineHost`](crate::engine_host::EngineHost) to support both languages.
pub struct WasmHost {
    store: Store,
    instances: HashMap<ModuleId, usize>,
    abi: HashMap<u64, Abi>,
    bridge: Bridge,
    next_module: ModuleId,
    next_token: u64,
    errors: Vec<String>,
}

impl Default for WasmHost {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for WasmHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmHost")
            .field("instances", &self.instances.len())
            .field("abi", &self.abi.len())
            .field("errors", &self.errors.len())
            .finish_non_exhaustive()
    }
}

impl WasmHost {
    /// Create an empty host.
    pub fn new() -> Self {
        Self {
            store: Store::new(),
            instances: HashMap::new(),
            abi: HashMap::new(),
            bridge: Bridge::default(),
            next_module: 1,
            next_token: 1,
            errors: Vec::new(),
        }
    }

    /// Errors from modules that failed to decode, validate, or run.
    pub fn errors(&self) -> &[String] {
        &self.errors
    }

    /// Compile and instantiate `bytes`, returning the instance id.
    fn load(&mut self, bytes: &[u8]) -> Result<usize, String> {
        let module = decode(bytes).map_err(|err| format!("decode: {err}"))?;
        validate(&module).map_err(|err| format!("validate: {err}"))?;

        // Register a host function for every engine import, checking its
        // signature against the ABI so a mismatched module is rejected rather
        // than called with misread arguments.
        let mut host_ids: HashMap<(String, String), usize> = HashMap::new();
        for import in &module.imports {
            let ImportDesc::Func(type_index) = import.desc else {
                return Err(format!(
                    "unsupported import {}.{}: only functions are imported",
                    import.module, import.name
                ));
            };
            if import.module != IMPORT_MODULE {
                return Err(format!("unknown import module {:?}", import.module));
            }
            let Some(abi) = Abi::for_name(&import.name) else {
                return Err(format!("unknown import {}.{}", import.module, import.name));
            };
            let declared = module
                .func_at_cloned(type_index)
                .ok_or_else(|| format!("import {} has an unknown type", import.name))?;
            let expected = abi.func_type();
            if declared.params != expected.params || declared.results != expected.results {
                return Err(format!("import {} has the wrong signature", import.name));
            }
            let token = self.next_token;
            self.next_token += 1;
            let host_id = self.store.external_host(declared, token);
            self.abi.insert(token, abi);
            host_ids.insert((import.module.clone(), import.name.clone()), host_id);
        }

        let mut resolve = |module_name: &str, field: &str| -> Option<ExternVal> {
            host_ids
                .get(&(module_name.to_string(), field.to_string()))
                .copied()
                .map(ExternVal::HostFunc)
        };
        self.store
            .instantiate(&module, &mut resolve)
            .map_err(|err| format!("instantiate: {err}"))
    }

    /// Run one lifecycle export, servicing its engine imports.
    fn run_export(
        &mut self,
        instance: usize,
        func: &str,
        entity: Option<EntityId>,
        delta: f32,
    ) -> Vec<ScriptCommand> {
        self.bridge.commands.clear();
        self.bridge.entity = entity;
        self.bridge.delta = delta;

        let Some(index) = self.store.exported_func(instance, func) else {
            return Vec::new();
        };
        // Handle 0 is the script's own entity; the rest are handed out lazily.
        let mut handles: Vec<EntityId> = vec![entity.unwrap_or(0)];
        let args = [Value::I32(0), Value::F32(delta.to_bits())];

        let mut progress = self.store.start(instance, index, &args);
        loop {
            match progress {
                Ok(RunProgress::Finished(_)) => break,
                Ok(RunProgress::Host(request)) => {
                    let reply = self.service(instance, request, &mut handles);
                    progress = self.store.resume(reply);
                }
                Err(fail) => {
                    self.errors.push(format!("{func}: {fail:?}"));
                    break;
                }
            }
        }
        std::mem::take(&mut self.bridge.commands)
    }

    /// Answer one [`HostRequest`] with the store unborrowed.
    fn service(
        &mut self,
        instance: usize,
        request: HostRequest,
        handles: &mut Vec<EntityId>,
    ) -> Result<Vec<Value>, slag::wasm::ExecFail> {
        let Some(abi) = self.abi.get(&request.token).copied() else {
            return Ok(Vec::new());
        };
        // `memory` borrows only `self.store`, so it coexists with the mutable
        // borrow of `self.bridge` below.
        let memory = memory_bytes(&self.store, instance);
        Ok(abi.apply(&request.args, &mut self.bridge, handles, memory))
    }
}

impl ScriptHost for WasmHost {
    fn eval_script(&mut self, source: &str) -> ModuleId {
        let id = self.next_module;
        self.next_module += 1;
        if !is_wasm_source(source) {
            self.errors
                .push(format!("module {id}: not a WebAssembly source"));
            return id;
        }
        match decode_source(source).ok_or_else(|| "invalid wasm descriptor".to_string()) {
            Ok(bytes) => match self.load(&bytes) {
                Ok(instance) => {
                    self.instances.insert(id, instance);
                }
                Err(err) => self.errors.push(format!("module {id}: {err}")),
            },
            Err(err) => self.errors.push(format!("module {id}: {err}")),
        }
        id
    }

    fn call_lifecycle(&mut self, module: ModuleId, func: &str, ctx: &mut ScriptContext<'_>) {
        self.bridge.begin(ctx);
        let Some(instance) = self.instances.get(&module).copied() else {
            return;
        };
        let commands = self.run_export(instance, func, ctx.entity, ctx.delta);
        replay(ctx, commands);
    }

    fn set_pending_api(&mut self, _api: PendingApi) {
        // The lifecycle call carries entity/delta/elapsed on the context, so
        // unlike a sandboxed engine this host needs no separate snapshot.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_round_trips_through_decode() {
        let module = [0x00u8, 0x61, 0x73, 0x6d, 0xff, 0x00, 0x7f];
        let source = encode(&module);
        assert!(is_wasm_source(&source));
        assert_eq!(decode_source(&source).as_deref(), Some(module.as_slice()));
    }

    #[test]
    fn a_plain_script_is_not_a_wasm_source() {
        assert!(!is_wasm_source("function on_update() {}"));
        assert_eq!(decode_source("on_update();"), None);
    }

    #[test]
    fn a_malformed_descriptor_is_rejected() {
        assert_eq!(decode_source("wasm:abc"), None); // odd digit count
        assert_eq!(decode_source("wasm:zz"), None); // not hex
    }

    #[test]
    fn abi_signatures_are_fixed_per_name() {
        assert_eq!(Abi::for_name("set_position"), Some(Abi::SetPosition));
        assert_eq!(Abi::for_name("nope"), None);
        let set_position = Abi::SetPosition.func_type();
        assert_eq!(
            set_position.params,
            vec![ValType::I32, ValType::F32, ValType::F32, ValType::F32]
        );
        assert!(set_position.results.is_empty());
        assert_eq!(Abi::Spawn.func_type().results, vec![ValType::I32]);
    }

    #[test]
    fn a_non_wasm_source_is_reported_not_run() {
        let mut host = WasmHost::new();
        let module = host.eval_script("function on_update() {}");
        assert!(!host.errors().is_empty());
        assert!(!host.instances.contains_key(&module));
    }

    #[test]
    fn a_module_of_garbage_fails_to_load_with_an_error() {
        let mut host = WasmHost::new();
        host.eval_script(&encode(&[0, 1, 2, 3, 4, 5, 6, 7]));
        assert!(
            host.errors().iter().any(|e| e.contains("decode")),
            "expected a decode error, got {:?}",
            host.errors()
        );
    }
}
