//! Pluggable script backends.
//!
//! A [`ScriptHost`] is the thing that actually turns script source into
//! something executable and invokes its lifecycle functions. This crate ships
//! two engine-free hosts:
//!
//! * [`NoopHost`] — compiles nothing and calls nothing. A test seam for a world
//!   with no scripting.
//! * [`RecordingHost`] — records every lifecycle call and can execute
//!   registered [`ScriptBehaviour`]s, so the runtime and the API can be tested
//!   without a JavaScript engine.
//!
//! The JavaScript backend is [`crate::slag::SlagHost`].

use std::collections::HashMap;

use crate::api::{EntityId, ScriptApi, ScriptCommand, ScriptContext};

/// Identifies a compiled script module within one host.
pub type ModuleId = u32;

/// One recorded lifecycle invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifecycleCall {
    /// The module the call was made on.
    pub module: ModuleId,
    /// The lifecycle function name (`"on_start"`, ...).
    pub func: String,
    /// The entity the call was made for.
    pub entity: Option<EntityId>,
}

/// A small, owned snapshot the runtime installs on a host before it drives a
/// callback.
///
/// Hosts that work directly on the [`ScriptContext`] they are handed (like the
/// built-in ones) can ignore this. A host that calls into a sandboxed engine,
/// and therefore cannot hold a borrow of the [`World`](ecs::world::World)
/// across the call boundary, can stash it to know which entity/timing the call
/// pertains to.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PendingApi {
    /// The entity the current lifecycle call pertains to.
    pub entity: Option<EntityId>,
    /// Delta time for the current frame, seconds.
    pub delta: f32,
    /// Total elapsed time, seconds.
    pub elapsed: f32,
}

/// A unit of script behaviour a host can execute for a lifecycle call.
///
/// A blanket implementation lets a plain closure stand in for a named type,
/// which keeps tests terse.
pub trait ScriptBehaviour: Send + Sync {
    /// Run the behaviour against the script API.
    fn run(&self, api: &mut ScriptApi<'_, '_>);
}

impl<F> ScriptBehaviour for F
where
    F: for<'api, 'ctx> Fn(&mut ScriptApi<'api, 'ctx>) + Send + Sync + 'static,
{
    fn run(&self, api: &mut ScriptApi<'_, '_>) {
        self(api);
    }
}

/// The backend that executes script code.
pub trait ScriptHost {
    /// Compile `source` into a module and return its handle.
    ///
    /// The runtime calls this at most once per unique source.
    fn eval_script(&mut self, source: &str) -> ModuleId;

    /// Invoke the lifecycle function `func` on `module`, driving the engine
    /// through `ctx`.
    fn call_lifecycle(&mut self, module: ModuleId, func: &str, ctx: &mut ScriptContext<'_>);

    /// Install a snapshot for the next [`Self::call_lifecycle`].
    fn set_pending_api(&mut self, api: PendingApi);

    /// Release any per-instance state the host keeps for `entity` — the runtime
    /// calls this when a script instance ends, so a persistent handle table does
    /// not outlive it. The default does nothing.
    fn forget_instance(&mut self, entity: EntityId) {
        let _ = entity;
    }

    /// Release a compiled module. The runtime calls this when no entity
    /// references it any more. The default does nothing.
    fn drop_module(&mut self, module: ModuleId) {
        let _ = module;
    }
}

/// A host that does nothing: it compiles sources to module `0` and never calls
/// them. A test seam for a world with no scripting.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopHost;

impl ScriptHost for NoopHost {
    fn eval_script(&mut self, _source: &str) -> ModuleId {
        0
    }

    fn call_lifecycle(&mut self, _module: ModuleId, _func: &str, _ctx: &mut ScriptContext<'_>) {}

    fn set_pending_api(&mut self, _api: PendingApi) {}
}

/// A host that records lifecycle calls and runs registered behaviours.
///
/// It also deduplicates sources: evaluating the same source twice returns the
/// same [`ModuleId`].
#[derive(Default)]
pub struct RecordingHost {
    next_module: ModuleId,
    sources: Vec<String>,
    by_source: HashMap<String, ModuleId>,
    calls: Vec<LifecycleCall>,
    behaviours: HashMap<String, Vec<Box<dyn ScriptBehaviour>>>,
    pending: Vec<PendingApi>,
    command_log: Vec<ScriptCommand>,
}

impl RecordingHost {
    /// An empty host.
    pub fn new() -> Self {
        Self {
            next_module: 1,
            ..Self::default()
        }
    }

    /// Register a behaviour to run whenever `func` is invoked, for any module.
    ///
    /// Behaviours run in registration order, before the call's commands are
    /// drained into the host's command log.
    pub fn on(&mut self, func: impl Into<String>, behaviour: impl ScriptBehaviour + 'static) {
        self.behaviours
            .entry(func.into())
            .or_default()
            .push(Box::new(behaviour));
    }

    /// The number of distinct sources compiled.
    pub fn compiled_count(&self) -> usize {
        self.sources.len()
    }

    /// The source a module was compiled from.
    pub fn module_source(&self, module: ModuleId) -> Option<&str> {
        self.by_source
            .iter()
            .find_map(|(source, &m)| (m == module).then_some(source.as_str()))
    }

    /// Every lifecycle call, in order.
    pub fn calls(&self) -> &[LifecycleCall] {
        &self.calls
    }

    /// How many times `func` was invoked.
    pub fn count_calls(&self, func: &str) -> usize {
        self.calls.iter().filter(|c| c.func == func).count()
    }

    /// Every command produced by the behaviours so far.
    pub fn command_log(&self) -> &[ScriptCommand] {
        &self.command_log
    }

    /// Every [`PendingApi`] snapshot the runtime installed.
    pub fn pending_calls(&self) -> &[PendingApi] {
        &self.pending
    }

    /// Forget recorded calls, commands, and pending snapshots (modules are
    /// kept).
    pub fn clear_records(&mut self) {
        self.calls.clear();
        self.command_log.clear();
        self.pending.clear();
    }
}

impl ScriptHost for RecordingHost {
    fn eval_script(&mut self, source: &str) -> ModuleId {
        if let Some(&module) = self.by_source.get(source) {
            return module;
        }
        let module = self.next_module;
        self.next_module += 1;
        self.sources.push(source.to_string());
        self.by_source.insert(source.to_string(), module);
        module
    }

    fn call_lifecycle(&mut self, module: ModuleId, func: &str, ctx: &mut ScriptContext<'_>) {
        self.calls.push(LifecycleCall {
            module,
            func: func.to_string(),
            entity: ctx.entity,
        });
        if let Some(behaviours) = self.behaviours.get(func) {
            for behaviour in behaviours {
                let mut api = ctx.api();
                behaviour.run(&mut api);
            }
        }
        // Read (do not drain) the call's commands: the runtime collects them
        // from the same context after this returns, and we keep our own copy
        // for host-level tests.
        self.command_log.extend(ctx.commands.iter().cloned());
    }

    fn set_pending_api(&mut self, api: PendingApi) {
        self.pending.push(api);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ScriptContext;
    use ecs::assets::AssetRegistry;
    use ecs::reflect::ComponentRegistry;
    use ecs::resources::InputState;
    use ecs::world::World;
    use math::vec3;

    /// An empty registry, for the tests that do not exercise reflection.
    fn empty_components() -> &'static ComponentRegistry {
        static EMPTY: std::sync::OnceLock<ComponentRegistry> = std::sync::OnceLock::new();
        EMPTY.get_or_init(ComponentRegistry::new)
    }

    #[test]
    fn eval_script_deduplicates_sources() {
        let mut host = RecordingHost::new();
        let a = host.eval_script("let x = 1;");
        let b = host.eval_script("let x = 1;");
        let c = host.eval_script("let x = 2;");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(host.compiled_count(), 2);
        assert_eq!(host.module_source(a), Some("let x = 1;"));
    }

    #[test]
    fn noop_host_never_records() {
        let mut host = NoopHost;
        assert_eq!(host.eval_script("anything"), 0);
        let mut world = World::new();
        let assets = AssetRegistry::new();
        let input = InputState::new();
        let mut ctx = ScriptContext::new(&mut world, &assets, empty_components(), &input);
        host.set_pending_api(PendingApi::default());
        host.call_lifecycle(0, "on_update", &mut ctx);
        assert!(ctx.commands.is_empty());
    }

    #[test]
    fn recording_host_records_calls_and_pending() {
        let mut host = RecordingHost::new();
        let module = host.eval_script("src");
        let mut world = World::new();
        let assets = AssetRegistry::new();
        let input = InputState::new();
        let mut ctx = ScriptContext::new(&mut world, &assets, empty_components(), &input);
        ctx.entity = Some(42);
        host.set_pending_api(PendingApi {
            entity: Some(42),
            delta: 0.5,
            elapsed: 2.0,
        });
        host.call_lifecycle(module, "on_start", &mut ctx);

        assert_eq!(host.calls().len(), 1);
        assert_eq!(host.count_calls("on_start"), 1);
        assert_eq!(host.calls()[0].entity, Some(42));
        assert_eq!(host.pending_calls()[0].delta, 0.5);
    }

    #[test]
    fn behaviours_run_and_produce_commands() {
        let mut host = RecordingHost::new();
        let module = host.eval_script("src");
        host.on("on_update", |api: &mut ScriptApi<'_, '_>| {
            if let Some(entity) = api.current_entity() {
                api.translate(entity, vec3(1.0, 0.0, 0.0));
            }
        });

        let mut world = World::new();
        let entity = world.spawn();
        let assets = AssetRegistry::new();
        let input = InputState::new();
        let mut ctx = ScriptContext::new(&mut world, &assets, empty_components(), &input);
        ctx.entity = Some(crate::api::entity_id(entity));

        host.call_lifecycle(module, "on_update", &mut ctx);
        assert_eq!(host.command_log().len(), 1);
        assert!(matches!(
            host.command_log()[0],
            ScriptCommand::Translate { .. }
        ));
    }
}
