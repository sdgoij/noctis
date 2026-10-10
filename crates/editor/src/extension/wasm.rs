//! Native WebAssembly editor extensions.
//!
//! A module is instantiated with the `noctis` import set and driven by
//! `noctis_update`. The governing idea is the one `goats/ABIv1.md` states: the
//! import list *is* the API. A module can reach nothing it did not declare, and a
//! module importing something the host does not grant fails to *link* — so
//! default-deny is a property of the linker, not a runtime check. No WASI, so
//! file I/O does not link.
//!
//! The boundary is coarse: the host calls `noctis_update(ptr, count, dt)` once
//! per frame and the module queues work through the imports. Writes (`edit`,
//! `run`) are queued and replayed by the [`ExtensionHost`](super::ExtensionHost)
//! after the call, the same way the JS host batches them; reads
//! (`editor_json`) are served from the frame's snapshot. See `docs/extensions.md`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use slag::wasm::exec::{HostRequest, RunProgress};
use slag::wasm::module::ImportDesc;
use slag::wasm::types::{FuncType, ValType};
use slag::wasm::{ExternVal, Store, Value, decode, validate};

use super::{Bridge, Queued};

/// The module name a WebAssembly extension imports the host API from.
const IMPORT_MODULE: &str = "noctis";

/// The `noctis` WASM ABI version this build implements.
pub(crate) const WASM_ABI: i32 = 1;

/// One host function a module may import.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Abi {
    /// `log(ptr, len)` — log UTF-8 bytes from the module's memory.
    Log,
    /// `editor_json(out_ptr, out_cap) -> i32` — write the entity list JSON.
    EditorJson,
    /// `edit(ptr, len) -> i32` — apply an `EditorCommand` batch from memory.
    Edit,
    /// `run(ptr, len)` — dispatch a verb line.
    Run,
}

impl Abi {
    fn for_name(name: &str) -> Option<Abi> {
        Some(match name {
            "log" => Abi::Log,
            "editor_json" => Abi::EditorJson,
            "edit" => Abi::Edit,
            "run" => Abi::Run,
            _ => return None,
        })
    }

    /// The exact signature the ABI fixes for this function.
    fn func_type(self) -> FuncType {
        let (params, results): (Vec<ValType>, Vec<ValType>) = match self {
            Abi::Log | Abi::Run => (vec![ValType::I32, ValType::I32], vec![]),
            Abi::Edit | Abi::EditorJson => (vec![ValType::I32, ValType::I32], vec![ValType::I32]),
        };
        FuncType { params, results }
    }
}

/// One instantiated module.
struct Instance {
    id: usize,
    update: Option<usize>,
}

/// The WebAssembly half of the extension host.
pub(crate) struct WasmHost {
    store: Store,
    instances: Vec<Instance>,
    tokens: HashMap<u64, Abi>,
    bridge: Rc<RefCell<Bridge>>,
    next_token: u64,
    errors: Vec<String>,
}

impl std::fmt::Debug for WasmHost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WasmHost")
            .field("instances", &self.instances.len())
            .field("errors", &self.errors.len())
            .finish_non_exhaustive()
    }
}

impl WasmHost {
    /// An empty host sharing `bridge` with the JavaScript host, so a module's
    /// queued edits, verbs and logs flow through the same apply/dispatch path.
    pub(crate) fn new(bridge: Rc<RefCell<Bridge>>) -> Self {
        Self {
            store: Store::new(),
            instances: Vec::new(),
            tokens: HashMap::new(),
            bridge,
            next_token: 1,
            errors: Vec::new(),
        }
    }

    /// Compile, link, instantiate and initialise a module.
    pub(crate) fn load(&mut self, bytes: &[u8]) -> Result<(), String> {
        let module = decode(bytes).map_err(|error| format!("decode: {error}"))?;
        validate(&module).map_err(|error| format!("validate: {error}"))?;

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
            self.tokens.insert(token, abi);
            host_ids.insert((import.module.clone(), import.name.clone()), host_id);
        }

        let mut resolve = |module_name: &str, field: &str| -> Option<ExternVal> {
            host_ids
                .get(&(module_name.to_string(), field.to_string()))
                .copied()
                .map(ExternVal::HostFunc)
        };
        let id = self
            .store
            .instantiate(&module, &mut resolve)
            .map_err(|error| format!("instantiate: {error}"))?;

        let abi = self.call(id, "noctis_abi", &[])?;
        match abi.as_slice() {
            [Value::I32(version)] if *version == WASM_ABI => {}
            other => {
                return Err(format!(
                    "noctis_abi returned {other:?}; this editor implements ABI {WASM_ABI}"
                ));
            }
        }
        self.call(id, "noctis_init", &[])?;

        let update = self.store.exported_func(id, "noctis_update");
        self.instances.push(Instance { id, update });
        Ok(())
    }

    /// Advance every module one frame.
    pub(crate) fn update(&mut self, dt: f32) {
        let calls: Vec<(usize, usize)> = self
            .instances
            .iter()
            .filter_map(|instance| instance.update.map(|update| (instance.id, update)))
            .collect();
        for (id, update) in calls {
            let args = [Value::I32(0), Value::I32(0), Value::F32(dt.to_bits())];
            if let Err(error) = self.call_index(id, update, &args) {
                self.errors.push(format!("noctis_update: {error}"));
            }
        }
    }

    /// Take the errors from loading or running modules.
    pub(crate) fn take_errors(&mut self) -> Vec<String> {
        std::mem::take(&mut self.errors)
    }

    fn call(&mut self, instance: usize, func: &str, args: &[Value]) -> Result<Vec<Value>, String> {
        let Some(index) = self.store.exported_func(instance, func) else {
            return Err(format!("the module does not export {func}"));
        };
        self.call_index(instance, index, args)
    }

    /// Run an exported function, servicing its imports.
    fn call_index(
        &mut self,
        instance: usize,
        index: usize,
        args: &[Value],
    ) -> Result<Vec<Value>, String> {
        let mut progress = self.store.start(instance, index, args);
        loop {
            match progress {
                Ok(RunProgress::Finished(values)) => return Ok(values),
                Ok(RunProgress::Host(request)) => {
                    let reply = self.service(instance, request);
                    progress = self.store.resume(reply);
                }
                Err(fail) => return Err(format!("{fail:?}")),
            }
        }
    }

    /// Answer one [`HostRequest`] with the store unborrowed.
    fn service(
        &self,
        instance: usize,
        request: HostRequest,
    ) -> Result<Vec<Value>, slag::wasm::ExecFail> {
        let abi = self.tokens.get(&request.token).copied();
        // `memory` borrows only `self.store`, so it coexists with the internal
        // borrow of the shared bridge.
        let memory = memory_cell(&self.store, instance).map(|cell| StoreMemory {
            store: &self.store,
            cell,
        });
        let mut bridge = self.bridge.borrow_mut();
        Ok(match abi {
            Some(Abi::Log) => {
                if let Some(text) = memory_string_at(&request.args, memory.as_ref(), 0) {
                    bridge.logs.push(text);
                }
                Vec::new()
            }
            Some(Abi::Run) => {
                if let Some(line) = memory_string_at(&request.args, memory.as_ref(), 0) {
                    bridge.queued.push(Queued::Run(line));
                }
                Vec::new()
            }
            Some(Abi::Edit) => {
                if let Some(json) = memory_string_at(&request.args, memory.as_ref(), 0) {
                    bridge.queued.push(Queued::Edit(json));
                }
                vec![Value::I32(0)]
            }
            Some(Abi::EditorJson) => {
                let out_ptr = out_index(&request.args, 0);
                let out_cap = out_index(&request.args, 1);
                let json = bridge.reads.clone();
                if json.len() > out_cap {
                    vec![Value::I32(-2)]
                } else if memory
                    .as_ref()
                    .is_some_and(|memory| memory.write(out_ptr, json.as_bytes()))
                {
                    vec![Value::I32(json.len() as i32)]
                } else {
                    vec![Value::I32(-1)]
                }
            }
            None => Vec::new(),
        })
    }
}

/// The memory cell of a module instance's exported `memory`, if it exports one.
fn memory_cell(store: &Store, instance: usize) -> Option<usize> {
    match store.export(instance, "memory")? {
        ExternVal::Memory(cell) => Some(cell),
        _ => None,
    }
}

/// A read/write view of a module instance's exported `memory`.
struct StoreMemory<'a> {
    store: &'a Store,
    cell: usize,
}

impl StoreMemory<'_> {
    fn read(&self, ptr: usize, len: usize) -> Option<Vec<u8>> {
        self.store.memory_block(self.cell)?.read(ptr, len).ok()
    }

    fn write(&self, ptr: usize, bytes: &[u8]) -> bool {
        self.store
            .memory_block(self.cell)
            .is_some_and(|block| block.write(ptr, bytes).is_ok())
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
    Some(String::from_utf8_lossy(&memory.read(ptr, len)?).into_owned())
}

/// An argument read as a non-negative byte offset.
fn out_index(args: &[Value], index: usize) -> usize {
    usize::try_from(i32_of(args.get(index).copied().unwrap_or(Value::I32(0)))).unwrap_or(0)
}
