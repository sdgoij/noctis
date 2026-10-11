//! The native WebAssembly script host.
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
//! | `get_position` | `(i32) -> (f32, f32, f32)` | read local translation |
//! | `get_rotation_euler` | `(i32) -> (f32, f32, f32)` | read rotation (radians) |
//! | `get_scale` | `(i32) -> (f32, f32, f32)` | read local scale |
//! | `destroy` | `(i32) -> ()` | despawn an entity |
//! | `destroy_by_name` | `(i32, i32) -> ()` | despawn the entity named `memory[ptr..ptr+len]` |
//! | `log` | `(i32, i32) -> ()` | log `memory[ptr..ptr+len]` |
//! | `spawn` | `(i32, i32) -> i32` | spawn an entity named `memory[ptr..ptr+len]` |
//! | `find_entity` | `(i32, i32) -> i32` | handle of the entity with that name |
//! | `add_script` | `(i32, i32, i32) -> ()` | attach `memory[ptr..ptr+len]` as a script |
//! | `instantiate_prefab` | `(i32, i32, i32) -> ()` | instantiate a prefab JSON under a parent |
//! | `has_component` | `(i32, i32, i32) -> i32` | is the named component present |
//! | `get_component` | `(i32, i32, i32, i32, i32) -> i32` | write the component's JSON to `memory[out_ptr..]` (`out_cap` bytes); returns the byte length, `-1` when absent, `-2` when the buffer is too small |
//! | `set_component` | `(i32, i32, i32, i32, i32) -> ()` | set the component from `memory[json_ptr..json_ptr+len]` |
//! | `remove_component` | `(i32, i32, i32) -> i32` | remove the component; `1` if it was present |
//! | `key_down` | `(i32, i32) -> i32` | is the named key held |
//! | `mouse_down` | `(i32, i32) -> i32` | is the named button held |
//! | `time_delta` | `() -> f32` | seconds since the last frame |
//! | `time_elapsed` | `() -> f32` | total elapsed seconds |
//!
//! The vector getters return three floats as WebAssembly **multi-value**
//! results. Two helpers read a `.wasm` file from disk into that text form:
//! [`load`] returns a ready [`Script`](ecs::components::Script) and [`register`]
//! installs it as a script asset under a name.
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
use std::path::Path;

use ecs::assets::{AssetRegistry, ScriptAsset};
use ecs::components::Script;
use ecs::resources::{Key, MouseButton};
use math::vec3;
use slag::wasm::exec::{HostRequest, RunProgress};
use slag::wasm::module::ImportDesc;
use slag::wasm::types::{FuncType, ValType};
use slag::wasm::{ExternVal, Store, Value, decode, validate};

use crate::api::{EntityId, ScriptCommand, ScriptContext};
use crate::bridge::{Bridge, entity_of, handle_of, replay};
use crate::host::{ModuleId, PendingApi, ScriptHost};

/// The module name a WebAssembly script imports the engine API from.
pub const IMPORT_MODULE: &str = "noctis";

// The `wasm:` module descriptor is an asset-encoding concern shared with the
// `project` importer (a `.wasm` file is a script asset), so it lives in
// `ecs::assets` and is re-exported here for the host code that decodes it.
pub use ecs::assets::{
    WASM_PREFIX as PREFIX, decode_wasm as decode_source, encode_wasm as encode, is_wasm_source,
};

/// One engine function a WebAssembly script may import.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Abi {
    SetPosition,
    Translate,
    SetRotationEuler,
    SetScale,
    GetPosition,
    GetRotationEuler,
    GetScale,
    Destroy,
    DestroyByName,
    Log,
    Spawn,
    FindEntity,
    AddScript,
    InstantiatePrefab,
    HasComponent,
    GetComponent,
    SetComponent,
    RemoveComponent,
    KeyDown,
    MouseDown,
    MouseMove,
    MousePosition,
    MouseScroll,
    SetVelocity,
    AddVelocity,
    ApplyImpulse,
    GetVelocity,
    IsGrounded,
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
            "get_position" => Abi::GetPosition,
            "get_rotation_euler" => Abi::GetRotationEuler,
            "get_scale" => Abi::GetScale,
            "destroy" => Abi::Destroy,
            "destroy_by_name" => Abi::DestroyByName,
            "log" => Abi::Log,
            "spawn" => Abi::Spawn,
            "find_entity" => Abi::FindEntity,
            "add_script" => Abi::AddScript,
            "instantiate_prefab" => Abi::InstantiatePrefab,
            "has_component" => Abi::HasComponent,
            "get_component" => Abi::GetComponent,
            "set_component" => Abi::SetComponent,
            "remove_component" => Abi::RemoveComponent,
            "key_down" => Abi::KeyDown,
            "mouse_down" => Abi::MouseDown,
            "mouse_move" => Abi::MouseMove,
            "mouse_position" => Abi::MousePosition,
            "mouse_scroll" => Abi::MouseScroll,
            "set_velocity" => Abi::SetVelocity,
            "add_velocity" => Abi::AddVelocity,
            "apply_impulse" => Abi::ApplyImpulse,
            "get_velocity" => Abi::GetVelocity,
            "is_grounded" => Abi::IsGrounded,
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
            // Vector getters return three floats (WebAssembly multi-value).
            GetPosition | GetRotationEuler | GetScale => (
                vec![ValType::I32],
                vec![ValType::F32, ValType::F32, ValType::F32],
            ),
            Destroy => (vec![ValType::I32], vec![]),
            DestroyByName => (vec![ValType::I32, ValType::I32], vec![]),
            Log => (vec![ValType::I32, ValType::I32], vec![]),
            Spawn | FindEntity => (vec![ValType::I32, ValType::I32], vec![ValType::I32]),
            KeyDown | MouseDown => (vec![ValType::I32, ValType::I32], vec![ValType::I32]),
            // Mouse reads take no arguments; motion/position return two floats.
            MouseMove | MousePosition => (vec![], vec![ValType::F32, ValType::F32]),
            MouseScroll => (vec![], vec![ValType::F32]),
            SetVelocity | AddVelocity | ApplyImpulse => (
                vec![ValType::I32, ValType::F32, ValType::F32, ValType::F32],
                vec![],
            ),
            GetVelocity => (
                vec![ValType::I32],
                vec![ValType::F32, ValType::F32, ValType::F32],
            ),
            IsGrounded => (vec![ValType::I32], vec![ValType::I32]),
            AddScript => (vec![ValType::I32, ValType::I32, ValType::I32], vec![]),
            InstantiatePrefab => (
                vec![ValType::I32, ValType::I32, ValType::I32],
                vec![ValType::I32],
            ),
            // Reflection: `has`/`remove` take (entity, name_ptr, name_len),
            // `get`/`set` add (json/out, len/cap) and `get` returns a length.
            HasComponent | RemoveComponent => (
                vec![ValType::I32, ValType::I32, ValType::I32],
                vec![ValType::I32],
            ),
            GetComponent => (
                vec![
                    ValType::I32,
                    ValType::I32,
                    ValType::I32,
                    ValType::I32,
                    ValType::I32,
                ],
                vec![ValType::I32],
            ),
            SetComponent => (
                vec![
                    ValType::I32,
                    ValType::I32,
                    ValType::I32,
                    ValType::I32,
                    ValType::I32,
                ],
                vec![],
            ),
            TimeDelta | TimeElapsed => (vec![], vec![ValType::F32]),
        };
        FuncType { params, results }
    }

    /// Service a call: mutate the bridge and return the results.
    fn apply(
        self,
        args: &[Value],
        bridge: &mut Bridge,
        handles: &mut Vec<EntityId>,
        memory: Option<&StoreMemory<'_>>,
    ) -> Vec<Value> {
        use Abi::*;
        let handle = |index: usize| -> Option<EntityId> {
            let raw = i32_of(args.get(index).copied().unwrap_or(Value::I32(-1)));
            entity_of(handles, raw)
        };
        let vector = |base: usize| {
            vec3(
                f32_of(args, base),
                f32_of(args, base + 1),
                f32_of(args, base + 2),
            )
        };
        let vector_result = |value: math::Vec3| {
            vec![
                Value::F32(value.x.to_bits()),
                Value::F32(value.y.to_bits()),
                Value::F32(value.z.to_bits()),
            ]
        };
        match self {
            SetPosition => {
                if let Some(id) = handle(0) {
                    bridge.set_position(id, vector(1));
                }
                Vec::new()
            }
            Translate => {
                if let Some(id) = handle(0) {
                    bridge.translate(id, vector(1));
                }
                Vec::new()
            }
            SetRotationEuler => {
                if let Some(id) = handle(0) {
                    bridge.set_rotation(id, vector(1));
                }
                Vec::new()
            }
            SetScale => {
                if let Some(id) = handle(0) {
                    bridge.set_scale(id, vector(1));
                }
                Vec::new()
            }
            GetPosition => handle(0).map_or_else(Vec::new, |id| vector_result(bridge.position(id))),
            GetRotationEuler => {
                handle(0).map_or_else(Vec::new, |id| vector_result(bridge.rotation(id)))
            }
            GetScale => handle(0).map_or_else(Vec::new, |id| vector_result(bridge.scale(id))),
            Destroy => {
                if let Some(id) = handle(0) {
                    bridge.commands.push(ScriptCommand::Destroy { entity: id });
                }
                Vec::new()
            }
            DestroyByName => {
                if let Some(name) = memory_string_at(args, memory, 0) {
                    bridge.commands.push(ScriptCommand::DestroyByName { name });
                }
                Vec::new()
            }
            Log => {
                if let Some(text) = memory_string_at(args, memory, 0) {
                    bridge.commands.push(ScriptCommand::Log {
                        message: text,
                        entity: bridge.entity,
                    });
                }
                Vec::new()
            }
            Spawn => {
                let name = memory_string_at(args, memory, 0);
                let id = bridge.spawn(name);
                vec![Value::I32(handle_of(handles, id))]
            }
            FindEntity => {
                let found = memory_string_at(args, memory, 0)
                    .and_then(|name| bridge.find(&name))
                    .map(|id| handle_of(handles, id));
                vec![Value::I32(found.unwrap_or(-1))]
            }
            AddScript => {
                if let (Some(id), Some(source)) = (handle(0), memory_string_at(args, memory, 1)) {
                    bridge
                        .commands
                        .push(ScriptCommand::AddScript { entity: id, source });
                }
                Vec::new()
            }
            InstantiatePrefab => {
                let Some(json) = memory_string_at(args, memory, 0) else {
                    return Vec::new();
                };
                // Return a handle for the prefab's root so the module can address
                // it immediately (resolved to the real entity at replay).
                let root = bridge.instantiate_prefab(json, handle(2));
                vec![Value::I32(handle_of(handles, root))]
            }
            HasComponent => {
                let has = match (handle(0), memory_string_at(args, memory, 1)) {
                    (Some(id), Some(name)) => bridge.has_component(id, &name),
                    _ => false,
                };
                vec![Value::I32(i32::from(has))]
            }
            GetComponent => {
                let json = match (handle(0), memory_string_at(args, memory, 1)) {
                    (Some(id), Some(name)) => bridge.component_json(id, &name),
                    _ => None,
                };
                let Some(json) = json else {
                    return vec![Value::I32(-1)];
                };
                let out_ptr = out_index(args, 3);
                let out_cap = out_index(args, 4);
                if json.len() > out_cap {
                    return vec![Value::I32(-2)];
                }
                let wrote = memory.is_some_and(|memory| memory.write(out_ptr, json.as_bytes()));
                if !wrote {
                    return vec![Value::I32(-1)];
                }
                vec![Value::I32(json.len() as i32)]
            }
            SetComponent => {
                if let (Some(id), Some(name), Some(json)) = (
                    handle(0),
                    memory_string_at(args, memory, 1),
                    memory_string_at(args, memory, 3),
                ) {
                    bridge.set_component(id, &name, json);
                }
                Vec::new()
            }
            RemoveComponent => {
                let removed = match (handle(0), memory_string_at(args, memory, 1)) {
                    (Some(id), Some(name)) => {
                        let present = bridge.has_component(id, &name);
                        bridge.remove_component(id, &name);
                        present
                    }
                    _ => false,
                };
                vec![Value::I32(i32::from(removed))]
            }
            KeyDown => {
                let down = memory_string_at(args, memory, 0)
                    .and_then(|name| Key::from_name(&name))
                    .is_some_and(|key| bridge.input.key_down(key));
                vec![Value::I32(i32::from(down))]
            }
            MouseDown => {
                let down = memory_string_at(args, memory, 0)
                    .and_then(|name| mouse_button(&name))
                    .is_some_and(|button| bridge.input.mouse.is_down(button));
                vec![Value::I32(i32::from(down))]
            }
            MouseMove => {
                let d = bridge.input.mouse.delta;
                vec![Value::F32(d[0].to_bits()), Value::F32(d[1].to_bits())]
            }
            MousePosition => {
                let p = bridge.input.mouse.position;
                vec![Value::F32(p[0].to_bits()), Value::F32(p[1].to_bits())]
            }
            MouseScroll => vec![Value::F32(bridge.input.mouse.scroll.to_bits())],
            SetVelocity => {
                if let Some(id) = handle(0) {
                    bridge.set_velocity(id, vector(1));
                }
                Vec::new()
            }
            AddVelocity => {
                if let Some(id) = handle(0) {
                    bridge.add_velocity(id, vector(1));
                }
                Vec::new()
            }
            ApplyImpulse => {
                if let Some(id) = handle(0) {
                    bridge.apply_impulse(id, vector(1));
                }
                Vec::new()
            }
            GetVelocity => handle(0).map_or_else(Vec::new, |id| vector_result(bridge.velocity(id))),
            IsGrounded => {
                let grounded = handle(0).is_some_and(|id| bridge.is_grounded(id));
                vec![Value::I32(i32::from(grounded))]
            }
            TimeDelta => vec![Value::F32(bridge.delta.to_bits())],
            TimeElapsed => vec![Value::F32(bridge.elapsed.to_bits())],
        }
    }
}

/// The mouse button named `name` (as the JavaScript API spells them).
fn mouse_button(name: &str) -> Option<MouseButton> {
    match name.to_ascii_lowercase().as_str() {
        "left" => Some(MouseButton::Left),
        "right" => Some(MouseButton::Right),
        "middle" => Some(MouseButton::Middle),
        _ => None,
    }
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

/// Read a `(ptr, len)` string from the module's memory, starting at `base`.
fn memory_string_at(
    args: &[Value],
    memory: Option<&StoreMemory<'_>>,
    base: usize,
) -> Option<String> {
    let memory = memory?;
    let raw = i32_of(args.get(base).copied()?);
    let ptr = usize::try_from(raw).ok()?;
    let len = usize::try_from(i32_of(args.get(base + 1).copied()?)).ok()?;
    let bytes = memory.read(ptr, len)?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// An argument read as a non-negative byte offset (a `usize`).
fn out_index(args: &[Value], index: usize) -> usize {
    usize::try_from(i32_of(args.get(index).copied().unwrap_or(Value::I32(0)))).unwrap_or(0)
}

/// The memory cell of a module instance's exported `memory`, if it exports one.
fn memory_cell(store: &Store, instance: usize) -> Option<usize> {
    match store.export(instance, "memory")? {
        ExternVal::Memory(cell) => Some(cell),
        _ => None,
    }
}

/// A read/write view of a module instance's exported `memory`, for one call.
struct StoreMemory<'a> {
    store: &'a Store,
    cell: usize,
}

impl StoreMemory<'_> {
    /// The bytes `[ptr, ptr+len)` of the memory, if in bounds.
    fn read(&self, ptr: usize, len: usize) -> Option<Vec<u8>> {
        let block = self.store.memory_block(self.cell)?;
        block.read(ptr, len).ok()
    }

    /// Write `bytes` at `ptr`, returning whether they fit.
    fn write(&self, ptr: usize, bytes: &[u8]) -> bool {
        match self.store.memory_block(self.cell) {
            Some(block) => block.write(ptr, bytes).is_ok(),
            None => false,
        }
    }
}

/// Read a `.wasm` file and produce a [`Script`](ecs::components::Script) that
/// runs it.
///
/// ```no_run
/// let script = script::wasm_host::load("turret.wasm").expect("read module");
/// ```
pub fn load(path: impl AsRef<Path>) -> std::io::Result<Script> {
    let bytes = std::fs::read(path)?;
    Ok(Script::from_source(encode(&bytes)))
}

/// Read a `.wasm` file and register it as a script asset under `name`, so a
/// `Script::from_path(name)` (or a scene reference) runs it.
///
/// ```no_run
/// # use ecs::assets::AssetRegistry;
/// let mut assets = AssetRegistry::with_defaults();
/// script::wasm_host::register(&mut assets, "scripts/turret.wasm", "turret.wasm")
///     .expect("read module");
/// ```
pub fn register(
    assets: &mut AssetRegistry,
    name: impl Into<String>,
    path: impl AsRef<Path>,
) -> std::io::Result<()> {
    let name = name.into();
    let bytes = std::fs::read(path)?;
    assets.scripts.insert_named(
        name.clone(),
        ScriptAsset {
            name,
            source: encode(&bytes),
        },
    );
    Ok(())
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
        let memory = memory_cell(&self.store, instance).map(|cell| StoreMemory {
            store: &self.store,
            cell,
        });
        Ok(abi.apply(&request.args, &mut self.bridge, handles, memory.as_ref()))
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
        // The vector getters return three floats (multi-value).
        assert_eq!(
            Abi::GetPosition.func_type().results,
            vec![ValType::F32, ValType::F32, ValType::F32]
        );
        assert_eq!(Abi::GetScale.func_type().params, vec![ValType::I32]);
        // Input probes, string commands, and prefabs.
        assert_eq!(Abi::KeyDown.func_type().results, vec![ValType::I32]);
        assert_eq!(Abi::MouseDown.func_type().results, vec![ValType::I32]);
        assert!(Abi::DestroyByName.func_type().results.is_empty());
        assert_eq!(
            Abi::AddScript.func_type().params,
            vec![ValType::I32, ValType::I32, ValType::I32]
        );
        assert_eq!(
            Abi::InstantiatePrefab.func_type().params,
            vec![ValType::I32, ValType::I32, ValType::I32]
        );
        // Instantiation hands back the prefab root.
        assert_eq!(
            Abi::InstantiatePrefab.func_type().results,
            vec![ValType::I32]
        );
        // Reflection imports.
        assert_eq!(Abi::for_name("has_component"), Some(Abi::HasComponent));
        assert_eq!(Abi::for_name("get_component"), Some(Abi::GetComponent));
        assert_eq!(Abi::for_name("set_component"), Some(Abi::SetComponent));
        assert_eq!(
            Abi::for_name("remove_component"),
            Some(Abi::RemoveComponent)
        );
        assert_eq!(Abi::HasComponent.func_type().results, vec![ValType::I32]);
        assert_eq!(Abi::RemoveComponent.func_type().results, vec![ValType::I32]);
        assert_eq!(
            Abi::GetComponent.func_type().params,
            vec![
                ValType::I32,
                ValType::I32,
                ValType::I32,
                ValType::I32,
                ValType::I32
            ]
        );
        assert_eq!(Abi::GetComponent.func_type().results, vec![ValType::I32]);
        assert!(Abi::SetComponent.func_type().results.is_empty());
    }

    #[test]
    fn mouse_button_names_match_the_javascript_api() {
        assert_eq!(mouse_button("left"), Some(MouseButton::Left));
        assert_eq!(mouse_button("Right"), Some(MouseButton::Right));
        assert_eq!(mouse_button("middle"), Some(MouseButton::Middle));
        assert_eq!(mouse_button("side"), None);
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
