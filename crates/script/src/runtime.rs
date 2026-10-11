//! The runtime that discovers, compiles, and drives per-entity scripts.
//!
//! [`ScriptRuntime`] scans a [`World`] for [`Script`] components, compiles each
//! **unique** source once through its [`ScriptHost`], and keeps a per-entity
//! instance. It then drives the lifecycle:
//!
//! * `on_start` — once, the first frame an entity is seen with an active script;
//! * `on_update` — every update frame;
//! * `on_destroy` — when the entity is despawned, its script component goes
//!   away, or it is disabled.
//!
//! Scripts may spawn entities; those entities are picked up on the *next*
//! frame, so a script can never observe a half-initialised world.

use std::collections::{HashMap, HashSet};

use ecs::assets::AssetRegistry;
use ecs::components::Script;
use ecs::reflect::ComponentRegistry;
use ecs::resources::InputState;
use ecs::world::{Entity, World};

use crate::api::{EntityId, PhysicsView, ScriptCommand, ScriptContext, entity_id};
use crate::host::{ModuleId, NoopHost, PendingApi, ScriptHost};

/// The start lifecycle function name.
pub const ON_START: &str = "on_start";
/// The per-frame lifecycle function name.
pub const ON_UPDATE: &str = "on_update";
/// The teardown lifecycle function name.
pub const ON_DESTROY: &str = "on_destroy";

/// Per-entity bookkeeping for one running script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptInstance {
    /// The script-facing handle of the owning entity.
    pub entity: EntityId,
    /// The compiled module backing this instance.
    pub module: ModuleId,
    /// The source the module was compiled from.
    pub source: String,
    /// Whether `on_start` has run.
    pub started: bool,
}

/// Counts describing what the last frame did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunReport {
    /// Instances whose `on_start` ran.
    pub started: usize,
    /// Instances whose `on_update` ran.
    pub updated: usize,
    /// Instances whose `on_destroy` ran.
    pub destroyed: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Start,
    Update,
}

/// Drives scripts for a [`World`].
///
/// Generic over the host so a project can pick the JavaScript backend (or a
/// stub) without touching this type. `ScriptRuntime::new` defaults to any host
/// with a `Default` implementation; use [`ScriptRuntime::with_host`] otherwise.
pub struct ScriptRuntime<H: ScriptHost = NoopHost> {
    host: H,
    instances: HashMap<Entity, ScriptInstance>,
    modules: HashMap<String, ModuleId>,
    input: InputState,
    /// The registered components scripts read and write by name (reflection).
    /// Empty until the caller sets them (`set_components`).
    components: ComponentRegistry,
    elapsed: f32,
    last_delta: f32,
    report: RunReport,
    command_log: Vec<ScriptCommand>,
    /// The physics snapshot scripts read this frame (`is_grounded`,
    /// `get_velocity`). The caller fills it before [`Self::run_update`].
    physics_view: PhysicsView,
}

impl<H: ScriptHost + Default> Default for ScriptRuntime<H> {
    fn default() -> Self {
        Self::with_host(H::default())
    }
}

impl<H: ScriptHost> ScriptRuntime<H> {
    /// A runtime driven by `host`.
    pub fn new(host: H) -> Self {
        Self::with_host(host)
    }

    /// A runtime driven by `host` (alias of [`Self::new`], for readability at
    /// call sites that pick a host explicitly).
    pub fn with_host(host: H) -> Self {
        Self {
            host,
            instances: HashMap::new(),
            modules: HashMap::new(),
            input: InputState::new(),
            components: ComponentRegistry::new(),
            elapsed: 0.0,
            last_delta: 0.0,
            report: RunReport::default(),
            command_log: Vec::new(),
            physics_view: PhysicsView::new(),
        }
    }

    /// Set the registered components scripts read and write by name.
    ///
    /// The runtime reloads a scene's registry on load (it travels on the scene).
    pub fn set_components(&mut self, components: ComponentRegistry) {
        self.components = components;
    }

    /// The registered components scripts read and write by name.
    pub fn components(&self) -> &ComponentRegistry {
        &self.components
    }

    /// The host.
    pub fn host(&self) -> &H {
        &self.host
    }

    /// The host, mutably.
    pub fn host_mut(&mut self) -> &mut H {
        &mut self.host
    }

    /// Consume the runtime, returning the host.
    pub fn into_host(self) -> H {
        self.host
    }

    /// The input state scripts see. Populate this before [`Self::run_update`].
    pub fn input_mut(&mut self) -> &mut InputState {
        &mut self.input
    }

    /// Replace the physics snapshot scripts read this frame.
    ///
    /// The caller builds it from the [`PhysicsWorld`](physics) before
    /// [`Self::run_update`]; without it `is_grounded` is `false` and
    /// `get_velocity` is zero.
    pub fn set_physics_view(&mut self, view: PhysicsView) {
        self.physics_view = view;
    }

    /// Total elapsed scaled time, seconds.
    pub fn elapsed(&self) -> f32 {
        self.elapsed
    }

    /// The number of live script instances.
    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }

    /// The number of unique sources compiled.
    pub fn module_count(&self) -> usize {
        self.modules.len()
    }

    /// The instance for an entity, if it has one.
    pub fn instance(&self, entity: Entity) -> Option<&ScriptInstance> {
        self.instances.get(&entity)
    }

    /// The most recent frame's [`RunReport`].
    pub fn last_report(&self) -> RunReport {
        self.report
    }

    /// Every command produced by scripts so far.
    pub fn command_log(&self) -> &[ScriptCommand] {
        &self.command_log
    }

    /// Remove and return every command produced so far.
    ///
    /// The engine loops call this after [`Self::run_update`] to apply the
    /// commands only physics can carry out (velocity, impulses), which the
    /// world-only replay leaves in the log. [`Self::command_log`] keeps
    /// accumulating for callers that never drain it.
    pub fn take_commands(&mut self) -> Vec<ScriptCommand> {
        std::mem::take(&mut self.command_log)
    }

    /// Drop all instances and compiled modules, leaving the host and input
    /// intact.
    pub fn reset(&mut self) {
        self.instances.clear();
        self.modules.clear();
        self.command_log.clear();
        self.report = RunReport::default();
        self.elapsed = 0.0;
        self.last_delta = 0.0;
        self.physics_view.clear();
    }

    /// Run the start lifecycle for every newly seen script and adopt the world.
    ///
    /// Equivalent to an update with zero delta, minus `on_update`.
    pub fn run_start(&mut self, world: &mut World, assets: &AssetRegistry) {
        self.last_delta = 0.0;
        self.run_frame(world, assets, 0.0, Phase::Start);
    }

    /// Advance scripts by `dt` seconds: start any new scripts and run
    /// `on_update` on every live one.
    pub fn run_update(&mut self, world: &mut World, assets: &AssetRegistry, dt: f32) {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        self.elapsed += dt;
        self.last_delta = dt;
        self.run_frame(world, assets, dt, Phase::Update);
    }

    /// Run `on_destroy` for `entity` (if it has a script) and despawn it.
    ///
    /// Returns `true` if the entity had a script or was alive. Safe to call on
    /// an entity that has already been despawned; the script's `on_destroy`
    /// still runs.
    pub fn run_destroy(
        &mut self,
        world: &mut World,
        assets: &AssetRegistry,
        entity: Entity,
    ) -> bool {
        let had_instance = self.instances.contains_key(&entity);
        if had_instance {
            self.invoke(world, assets, entity, ON_DESTROY, 0.0);
        }
        self.instances.remove(&entity);
        let was_alive = world.is_alive(entity);
        if was_alive {
            world.despawn(entity);
        }
        self.report.destroyed += usize::from(had_instance);
        had_instance || was_alive
    }

    // -- internals ----------------------------------------------------------

    fn ensure_module(&mut self, source: &str) -> ModuleId {
        if let Some(&module) = self.modules.get(source) {
            return module;
        }
        let module = self.host.eval_script(source);
        self.modules.insert(source.to_string(), module);
        module
    }

    fn run_frame(&mut self, world: &mut World, assets: &AssetRegistry, dt: f32, phase: Phase) {
        // Resolve the active scripts and reconcile compiled modules with the
        // world borrowed only immutably, so a script whose source is unchanged
        // is never copied (see `resolve_source`). Entities spawned or destroyed
        // during the frame are handled next frame.
        let mut active: Vec<Entity> = Vec::new();
        let mut recreate: Vec<(Entity, String, ModuleId)> = Vec::new();
        for (entity, script) in world.query::<Script>() {
            let Some(source) = resolve_source(script, assets) else {
                continue;
            };
            active.push(entity);
            let module = self.ensure_module(source);
            let replace = self
                .instances
                .get(&entity)
                .is_none_or(|instance| instance.source != source);
            if replace {
                // The only copy: a (re)subscribing instance stores its source so
                // a later change to the `Script` component is detected.
                recreate.push((entity, source.to_string(), module));
            }
        }

        let mut report = RunReport::default();

        // Adopt / re-subscribe instances.
        for (entity, source, module) in recreate {
            if self.instances.contains_key(&entity) {
                self.invoke(world, assets, entity, ON_DESTROY, 0.0);
                report.destroyed += 1;
            }
            self.instances.insert(
                entity,
                ScriptInstance {
                    entity: entity_id(entity),
                    module,
                    source,
                    started: false,
                },
            );
        }

        // Everything that stopped being an active script gets torn down.
        let live: HashSet<Entity> = active.iter().copied().collect();
        let stale: Vec<Entity> = self
            .instances
            .keys()
            .filter(|entity| !live.contains(entity))
            .copied()
            .collect();
        for entity in stale {
            self.invoke(world, assets, entity, ON_DESTROY, 0.0);
            self.instances.remove(&entity);
            report.destroyed += 1;
        }

        // Start first-timers.
        let to_start: Vec<Entity> = self
            .instances
            .iter()
            .filter(|(_, instance)| !instance.started)
            .map(|(entity, _)| *entity)
            .collect();
        for entity in to_start {
            if !world.is_alive(entity) {
                continue;
            }
            self.invoke(world, assets, entity, ON_START, dt);
            if let Some(instance) = self.instances.get_mut(&entity) {
                instance.started = true;
            }
            report.started += 1;
        }

        // Update.
        if phase == Phase::Update {
            for entity in &active {
                if !world.is_alive(*entity) {
                    continue;
                }
                if !self.instances.get(entity).is_some_and(|i| i.started) {
                    continue;
                }
                self.invoke(world, assets, *entity, ON_UPDATE, dt);
                report.updated += 1;
            }
        }

        self.report = report;
    }

    fn invoke(
        &mut self,
        world: &mut World,
        assets: &AssetRegistry,
        entity: Entity,
        func: &str,
        dt: f32,
    ) {
        let Self {
            host,
            instances,
            components,
            input,
            elapsed,
            command_log,
            physics_view,
            ..
        } = self;
        let Some(instance) = instances.get(&entity) else {
            return;
        };
        let module = instance.module;
        let id = instance.entity;

        host.set_pending_api(PendingApi {
            entity: Some(id),
            delta: dt,
            elapsed: *elapsed,
        });
        let mut ctx = ScriptContext {
            world,
            assets,
            components: &*components,
            input: &*input,
            physics: Some(&*physics_view),
            elapsed: *elapsed,
            delta: dt,
            entity: Some(id),
            commands: Vec::new(),
        };
        host.call_lifecycle(module, func, &mut ctx);
        command_log.append(&mut ctx.commands);
    }

    /// The delta of the most recent update.
    pub fn last_delta(&self) -> f32 {
        self.last_delta
    }
}

/// Resolve a script component's source as a borrow, or `None` if it is disabled
/// or its asset path cannot be resolved.
///
/// Borrowing (rather than cloning, as this once did) keeps the per-frame scan
/// allocation-free for a script whose source has not changed; the runtime copies
/// the source only when it first adopts or re-subscribes an instance.
fn resolve_source<'a>(script: &'a Script, assets: &'a AssetRegistry) -> Option<&'a str> {
    if !script.enabled {
        return None;
    }
    if let Some(source) = &script.source {
        return Some(source.as_str());
    }
    let path = script.path.as_deref()?;
    assets.scripts.get_by_path(path).map(|a| a.source.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{PhysicsView, ScriptApi, entity_from_id, entity_id};
    use crate::host::RecordingHost;
    use ecs::assets::ScriptAsset;
    use ecs::components::{LocalTransform, Name};
    use ecs::resources::Key;
    use math::vec3;

    fn world_with_script(source: &str) -> (World, Entity) {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Script::from_source(source));
        (world, entity)
    }

    #[test]
    fn compiles_each_unique_source_once() {
        let mut host = RecordingHost::new();
        host.on("on_update", |_api: &mut ScriptApi<'_, '_>| {});
        let mut runtime = ScriptRuntime::with_host(host);
        let assets = AssetRegistry::new();

        let mut world = World::new();
        for _ in 0..3 {
            let e = world.spawn();
            world.insert(e, Script::from_source("shared();"));
        }
        let solo = world.spawn();
        world.insert(solo, Script::from_source("unique();"));

        runtime.run_update(&mut world, &assets, 0.016);
        assert_eq!(runtime.module_count(), 2);
        assert_eq!(runtime.host().compiled_count(), 2);
        assert_eq!(runtime.instance_count(), 4);
    }

    #[test]
    fn on_start_runs_once_and_on_update_every_frame() {
        let mut host = RecordingHost::new();
        host.on("on_start", |api: &mut ScriptApi<'_, '_>| {
            if let Some(e) = api.current_entity() {
                api.set_position(e, vec3(1.0, 0.0, 0.0));
            }
        });
        host.on("on_update", |_api: &mut ScriptApi<'_, '_>| {});
        let mut runtime = ScriptRuntime::with_host(host);
        let assets = AssetRegistry::new();
        let (mut world, entity) = world_with_script("behave();");

        runtime.run_update(&mut world, &assets, 0.016);
        runtime.run_update(&mut world, &assets, 0.016);
        runtime.run_update(&mut world, &assets, 0.016);

        assert_eq!(runtime.host().count_calls("on_start"), 1);
        assert_eq!(runtime.host().count_calls("on_update"), 3);
        assert_eq!(
            world.get::<LocalTransform>(entity).unwrap().translation,
            vec3(1.0, 0.0, 0.0)
        );
    }

    #[test]
    fn run_start_starts_without_updating() {
        let mut host = RecordingHost::new();
        host.on("on_start", |_api: &mut ScriptApi<'_, '_>| {});
        host.on("on_update", |_api: &mut ScriptApi<'_, '_>| {});
        let mut runtime = ScriptRuntime::with_host(host);
        let assets = AssetRegistry::new();
        let (mut world, _) = world_with_script("s();");

        runtime.run_start(&mut world, &assets);
        assert_eq!(runtime.host().count_calls("on_start"), 1);
        assert_eq!(runtime.host().count_calls("on_update"), 0);
    }

    #[test]
    fn spawned_entity_starts_on_the_next_frame() {
        // The root script spawns a child that carries its own script.
        let mut host = RecordingHost::new();
        host.on("on_start", |api: &mut ScriptApi<'_, '_>| {
            let child = api.spawn(Some("Child".into()));
            api.add_script(child, "child();");
        });
        host.on("on_update", |_api: &mut ScriptApi<'_, '_>| {});
        let mut runtime = ScriptRuntime::with_host(host);
        let assets = AssetRegistry::new();
        let (mut world, _) = world_with_script("root();");

        // Frame 1: root starts and spawns the child.
        runtime.run_update(&mut world, &assets, 0.016);
        assert_eq!(runtime.instance_count(), 1);
        assert_eq!(runtime.module_count(), 1);

        // Frame 2: the child is discovered and started.
        runtime.run_update(&mut world, &assets, 0.016);
        assert_eq!(runtime.instance_count(), 2);
        assert_eq!(runtime.module_count(), 2);
        assert_eq!(runtime.host().count_calls("on_start"), 2);
    }

    #[test]
    fn script_spawn_adds_entity_to_world() {
        let mut host = RecordingHost::new();
        host.on("on_update", |api: &mut ScriptApi<'_, '_>| {
            api.spawn(Some("Spawned".into()));
        });
        let mut runtime = ScriptRuntime::with_host(host);
        let assets = AssetRegistry::new();
        let (mut world, _) = world_with_script("s();");

        let before = world.entities().len();
        runtime.run_update(&mut world, &assets, 0.016);
        assert_eq!(world.entities().len(), before + 1);
        assert!(
            world
                .entities_with::<Name>()
                .iter()
                .any(|e| { world.get::<Name>(*e).is_some_and(|n| n.0 == "Spawned") })
        );
    }

    #[test]
    fn disabled_script_does_not_run() {
        let mut host = RecordingHost::new();
        host.on("on_start", |_api: &mut ScriptApi<'_, '_>| {});
        host.on("on_update", |_api: &mut ScriptApi<'_, '_>| {});
        let mut runtime = ScriptRuntime::with_host(host);
        let assets = AssetRegistry::new();

        let mut world = World::new();
        let entity = world.spawn();
        let mut script = Script::from_source("s();");
        script.enabled = false;
        world.insert(entity, script);

        runtime.run_update(&mut world, &assets, 0.016);
        assert_eq!(runtime.instance_count(), 0);
        assert_eq!(runtime.host().count_calls("on_start"), 0);
        assert_eq!(runtime.host().count_calls("on_update"), 0);
    }

    #[test]
    fn disabling_a_running_script_tears_it_down() {
        let mut host = RecordingHost::new();
        host.on("on_start", |_api: &mut ScriptApi<'_, '_>| {});
        host.on("on_destroy", |_api: &mut ScriptApi<'_, '_>| {});
        let mut runtime = ScriptRuntime::with_host(host);
        let assets = AssetRegistry::new();
        let (mut world, entity) = world_with_script("s();");

        runtime.run_update(&mut world, &assets, 0.016);
        assert_eq!(runtime.host().count_calls("on_start"), 1);

        world.get_mut::<Script>(entity).unwrap().enabled = false;
        runtime.run_update(&mut world, &assets, 0.016);
        assert_eq!(runtime.host().count_calls("on_destroy"), 1);
        assert_eq!(runtime.instance_count(), 0);
    }

    #[test]
    fn destroying_an_entity_runs_on_destroy_and_removes_components() {
        let mut host = RecordingHost::new();
        host.on("on_destroy", |_api: &mut ScriptApi<'_, '_>| {});
        let mut runtime = ScriptRuntime::with_host(host);
        let assets = AssetRegistry::new();
        let (mut world, entity) = world_with_script("s();");

        runtime.run_update(&mut world, &assets, 0.016);
        assert!(runtime.run_destroy(&mut world, &assets, entity));

        assert_eq!(runtime.host().count_calls("on_destroy"), 1);
        assert!(!world.is_alive(entity));
        assert!(!world.has::<Script>(entity));
        assert!(!world.has::<LocalTransform>(entity));
        assert_eq!(runtime.instance_count(), 0);
    }

    #[test]
    fn external_despawn_is_detected_next_frame() {
        let mut host = RecordingHost::new();
        host.on("on_destroy", |_api: &mut ScriptApi<'_, '_>| {});
        let mut runtime = ScriptRuntime::with_host(host);
        let assets = AssetRegistry::new();
        let (mut world, entity) = world_with_script("s();");

        runtime.run_update(&mut world, &assets, 0.016);
        world.despawn(entity);
        runtime.run_update(&mut world, &assets, 0.016);

        assert_eq!(runtime.host().count_calls("on_destroy"), 1);
        assert_eq!(runtime.instance_count(), 0);
    }

    #[test]
    fn set_position_through_a_script_round_trips() {
        let mut host = RecordingHost::new();
        host.on("on_update", |api: &mut ScriptApi<'_, '_>| {
            if let Some(entity) = api.current_entity() {
                api.set_position(entity, vec3(3.0, 4.0, 5.0));
                assert_eq!(api.get_position(entity), vec3(3.0, 4.0, 5.0));
            }
        });
        let mut runtime = ScriptRuntime::with_host(host);
        let assets = AssetRegistry::new();
        let (mut world, entity) = world_with_script("s();");

        runtime.run_update(&mut world, &assets, 0.016);
        assert_eq!(
            world.get::<LocalTransform>(entity).unwrap().translation,
            vec3(3.0, 4.0, 5.0)
        );
    }

    #[test]
    fn input_reaches_scripts() {
        let mut host = RecordingHost::new();
        host.on("on_update", |api: &mut ScriptApi<'_, '_>| {
            if api.key_down("Space") {
                api.log("jump");
            }
        });
        let mut runtime = ScriptRuntime::with_host(host);
        let assets = AssetRegistry::new();
        let (mut world, _) = world_with_script("s();");
        runtime.input_mut().press_key(Key::Space);

        runtime.run_update(&mut world, &assets, 0.016);
        assert!(matches!(
            runtime.command_log().last(),
            Some(ScriptCommand::Log { .. })
        ));
    }

    #[test]
    fn the_physics_snapshot_reaches_scripts() {
        let mut host = RecordingHost::new();
        host.on("on_update", |api: &mut ScriptApi<'_, '_>| {
            let Some(entity) = api.current_entity() else {
                return;
            };
            if api.is_grounded(entity) {
                api.log("grounded");
            }
            api.log(format!("vy={}", api.get_velocity(entity).y));
        });
        let mut runtime = ScriptRuntime::with_host(host);
        let assets = AssetRegistry::new();
        let (mut world, entity) = world_with_script("s();");

        let id = entity_id(entity);
        let mut view = PhysicsView::new();
        view.set_grounded(id, true);
        view.set_velocity(id, vec3(0.0, 2.5, 0.0));
        runtime.set_physics_view(view);

        runtime.run_update(&mut world, &assets, 0.016);

        let log = runtime.command_log();
        assert!(
            log.iter()
                .any(|c| matches!(c, ScriptCommand::Log { message, .. } if message == "grounded")),
            "grounded flag did not reach the script: {log:?}"
        );
        assert!(
            log.iter()
                .any(|c| matches!(c, ScriptCommand::Log { message, .. } if message == "vy=2.5")),
            "velocity did not reach the script: {log:?}"
        );
    }

    #[test]
    fn the_physics_snapshot_reports_overlaps() {
        let mut host = RecordingHost::new();
        host.on("on_update", |api: &mut ScriptApi<'_, '_>| {
            if let (Some(a), Some(b)) = (api.find_entity("A"), api.find_entity("B"))
                && api.is_overlapping(a, b)
            {
                api.log("overlap");
            }
        });
        let mut runtime = ScriptRuntime::with_host(host);
        let assets = AssetRegistry::new();
        let mut world = World::new();
        let a = world.spawn();
        world.insert(a, Name::new("A"));
        world.insert(a, Script::from_source("s();"));
        let b = world.spawn();
        world.insert(b, Name::new("B"));

        let mut view = PhysicsView::new();
        view.add_overlap(entity_id(a), entity_id(b));
        runtime.set_physics_view(view);

        runtime.run_update(&mut world, &assets, 0.016);
        assert!(
            runtime
                .command_log()
                .iter()
                .any(|c| matches!(c, ScriptCommand::Log { message, .. } if message == "overlap")),
            "the overlap did not reach the script: {:?}",
            runtime.command_log()
        );
    }

    #[test]
    fn prefab_instantiation_via_api_creates_entities() {
        let json = r#"{
            "name": "Pack",
            "scene": {
                "name": "Pack",
                "entities": [
                    { "id": 0, "name": "P0",
                      "transform": { "translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] } },
                    { "id": 1, "name": "P1", "parent": 0,
                      "transform": { "translation": [0,1,0], "rotation": [0,0,0,1], "scale": [1,1,1] } },
                    { "id": 2, "name": "P2", "parent": 0,
                      "transform": { "translation": [0,2,0], "rotation": [0,0,0,1], "scale": [1,1,1] } }
                ]
            }
        }"#;

        let mut host = RecordingHost::new();
        host.on("on_start", move |api: &mut ScriptApi<'_, '_>| {
            let ids = api.instantiate_prefab(json, None);
            assert_eq!(ids.len(), 3);
        });
        let mut runtime = ScriptRuntime::with_host(host);
        let assets = AssetRegistry::new();
        let (mut world, _) = world_with_script("s();");

        let before = world.entities().len();
        runtime.run_update(&mut world, &assets, 0.016);
        assert_eq!(world.entities().len(), before + 3);
    }

    #[test]
    fn script_path_is_resolved_from_assets() {
        let mut assets = AssetRegistry::new();
        assets.scripts.insert_named(
            "scripts/hero.js",
            ScriptAsset {
                name: "scripts/hero.js".into(),
                source: "hero();".into(),
            },
        );

        let mut host = RecordingHost::new();
        host.on("on_start", |_api: &mut ScriptApi<'_, '_>| {});
        let mut runtime = ScriptRuntime::with_host(host);
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Script::from_path("scripts/hero.js"));

        runtime.run_update(&mut world, &assets, 0.016);
        assert_eq!(runtime.module_count(), 1);
        assert_eq!(runtime.host().count_calls("on_start"), 1);
    }

    #[test]
    fn script_with_unresolvable_path_is_skipped() {
        let assets = AssetRegistry::new();
        let mut host = RecordingHost::new();
        host.on("on_start", |_api: &mut ScriptApi<'_, '_>| {});
        let mut runtime = ScriptRuntime::with_host(host);
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Script::from_path("missing.js"));

        runtime.run_update(&mut world, &assets, 0.016);
        assert_eq!(runtime.instance_count(), 0);
    }

    #[test]
    fn add_script_at_runtime_starts_next_frame() {
        let mut host = RecordingHost::new();
        host.on("on_update", |api: &mut ScriptApi<'_, '_>| {
            if let Some(entity) = api.current_entity() {
                // Replacing the running script swaps the module.
                api.add_script(entity, "replacement();");
            }
        });
        host.on("on_start", |_api: &mut ScriptApi<'_, '_>| {});
        host.on("on_destroy", |_api: &mut ScriptApi<'_, '_>| {});
        let mut runtime = ScriptRuntime::with_host(host);
        let assets = AssetRegistry::new();
        let (mut world, _) = world_with_script("original();");

        runtime.run_update(&mut world, &assets, 0.016);
        assert_eq!(runtime.module_count(), 1);

        // Next frame: source changed, so a new module compiles and the old
        // instance is torn down before the new one starts.
        runtime.run_update(&mut world, &assets, 0.016);
        assert_eq!(runtime.module_count(), 2);
        assert_eq!(runtime.host().count_calls("on_destroy"), 1);
        assert_eq!(runtime.host().count_calls("on_start"), 2);
    }

    #[test]
    fn elapsed_accumulates() {
        let mut host = RecordingHost::new();
        host.on("on_update", |_api: &mut ScriptApi<'_, '_>| {});
        let mut runtime = ScriptRuntime::with_host(host);
        let assets = AssetRegistry::new();
        let (mut world, _) = world_with_script("s();");

        runtime.run_update(&mut world, &assets, 0.25);
        runtime.run_update(&mut world, &assets, 0.25);
        assert!((runtime.elapsed() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn commands_are_collected_across_calls() {
        let mut host = RecordingHost::new();
        host.on("on_update", |api: &mut ScriptApi<'_, '_>| {
            api.spawn(Some("N".into()));
        });
        let mut runtime = ScriptRuntime::with_host(host);
        let assets = AssetRegistry::new();
        let (mut world, _) = world_with_script("s();");

        runtime.run_update(&mut world, &assets, 0.016);
        assert_eq!(runtime.command_log().len(), 1);
        assert!(matches!(
            runtime.command_log()[0],
            ScriptCommand::Spawn { .. }
        ));
    }

    #[test]
    fn reset_clears_state_but_keeps_host() {
        let mut host = RecordingHost::new();
        host.on("on_update", |_api: &mut ScriptApi<'_, '_>| {});
        let mut runtime = ScriptRuntime::with_host(host);
        let assets = AssetRegistry::new();
        let (mut world, _) = world_with_script("s();");
        runtime.run_update(&mut world, &assets, 0.016);
        assert_eq!(runtime.instance_count(), 1);

        runtime.reset();
        assert_eq!(runtime.instance_count(), 0);
        assert_eq!(runtime.module_count(), 0);
        assert_eq!(runtime.command_log().len(), 0);
    }

    // Keeps `entity_from_id` in use in this module's tests without a live
    // lookup, documenting the id encoding used throughout.
    #[test]
    fn instance_ids_match_world_entities() {
        let mut host = RecordingHost::new();
        host.on("on_update", |_api: &mut ScriptApi<'_, '_>| {});
        let mut runtime = ScriptRuntime::with_host(host);
        let assets = AssetRegistry::new();
        let (mut world, entity) = world_with_script("s();");
        runtime.run_update(&mut world, &assets, 0.016);

        let instance = runtime.instance(entity).unwrap();
        assert_eq!(entity_from_id(&world, instance.entity), Some(entity));
    }
}
