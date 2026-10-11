//! A script host that routes each script to the right backend.
//!
//! The engine has one script channel per [`Script`](ecs::components::Script)
//! component, so a project that mixes JavaScript and native WebAssembly needs one
//! host that speaks both. [`EngineHost`] wraps a [`SlagHost`] (JavaScript) and a
//! [`WasmHost`] (WebAssembly) and picks by the source: a
//! [`wasm_host`](crate::wasm_host) descriptor goes to the WebAssembly backend and
//! anything else to JavaScript. Module ids are namespaced by the router so the
//! two backends' internal counters cannot collide.

use std::collections::HashMap;

use crate::api::{EntityId, ScriptContext};
use crate::host::{ModuleId, PendingApi, ScriptHost};
use crate::slag::{SlagError, SlagHost};
use crate::wasm_host::{WasmHost, is_wasm_source};

/// Which backend a module belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    Javascript,
    Wasm,
}

/// A [`ScriptHost`] that runs JavaScript and WebAssembly scripts side by side.
#[derive(Debug)]
pub struct EngineHost {
    javascript: SlagHost,
    wasm: WasmHost,
    routes: HashMap<ModuleId, (Backend, ModuleId)>,
    next_module: ModuleId,
}

impl EngineHost {
    /// Create both backends (starting the Slag engine and, with the `jit`
    /// feature, installing its Cranelift hook).
    pub fn new() -> Result<Self, SlagError> {
        Ok(Self {
            javascript: SlagHost::new()?,
            wasm: WasmHost::new(),
            routes: HashMap::new(),
            next_module: 1,
        })
    }

    /// The JavaScript backend.
    pub fn javascript(&self) -> &SlagHost {
        &self.javascript
    }

    /// The WebAssembly backend.
    pub fn wasm(&self) -> &WasmHost {
        &self.wasm
    }

    /// Errors from either backend.
    pub fn errors(&self) -> Vec<String> {
        let mut errors = self.javascript.errors().to_vec();
        errors.extend(self.wasm.errors().iter().cloned());
        errors
    }

    /// `true` if the JavaScript backend installed its Cranelift JIT hook.
    pub fn jit_enabled(&self) -> bool {
        self.javascript.jit_enabled()
    }
}

impl ScriptHost for EngineHost {
    fn eval_script(&mut self, source: &str) -> ModuleId {
        let (backend, inner) = if is_wasm_source(source) {
            (Backend::Wasm, self.wasm.eval_script(source))
        } else {
            (Backend::Javascript, self.javascript.eval_script(source))
        };
        let id = self.next_module;
        self.next_module += 1;
        self.routes.insert(id, (backend, inner));
        id
    }

    fn call_lifecycle(&mut self, module: ModuleId, func: &str, ctx: &mut ScriptContext<'_>) {
        match self.routes.get(&module).copied() {
            Some((Backend::Javascript, inner)) => self.javascript.call_lifecycle(inner, func, ctx),
            Some((Backend::Wasm, inner)) => self.wasm.call_lifecycle(inner, func, ctx),
            None => {}
        }
    }

    fn set_pending_api(&mut self, api: PendingApi) {
        self.javascript.set_pending_api(api);
        self.wasm.set_pending_api(api);
    }

    fn forget_instance(&mut self, entity: EntityId) {
        self.javascript.forget_instance(entity);
        self.wasm.forget_instance(entity);
    }
}
