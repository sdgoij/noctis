//! The game loop: wires physics, audio, and scripting into
//! [`ecs::app::App`].
//!
//! A [`Runtime`] owns an [`App`] plus a [`PhysicsWorld`], an [`AudioEngine`],
//! and a [`ScriptRuntime`], and schedules them across the app's stages:
//!
//! | stage | work |
//! |---|---|
//! | `startup` | sync physics, run each script's `on_start` |
//! | `fixed` | sync from the world, `physics.step`, write back poses |
//! | `update` | feed input, run each script's `on_update(dt)` |
//! | `late` | reconcile the world's `AudioSource`s with the audio engine |
//!
//! The subsystems are shared with the stage systems through
//! `Arc<Mutex<_>>`, which is what lets the closures registered on the `App`
//! reach state the `Runtime` also owns. The runtime never holds a lock across a
//! stage boundary, so the sharing cannot deadlock.
//!
//! Rendering is optional and off by default ([`Runtime::enable_renderer`]); when
//! enabled, a frame is drawn each tick and the last [`Frame`] is kept. A missing
//! GPU adapter is logged and ignored, never fatal.

#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use audio::AudioEngine;
use ecs::app::App;
use ecs::assets::AssetRegistry;
#[cfg(not(target_arch = "wasm32"))]
use ecs::assets::ScriptAsset;
use ecs::components::RigidBody;
use ecs::scene::{Scene, propagate_transforms};
use physics::PhysicsWorld;
use render::{Frame, IblConfig, Renderer};
#[cfg(not(target_arch = "wasm32"))]
use render::{RenderError, RendererConfig};
use script::{NoopHost, PhysicsView, ScriptCommand, ScriptHost, ScriptRuntime};
#[cfg(not(target_arch = "wasm32"))]
use vfs::{DirStore, Store};

use crate::config::RuntimeConfig;

/// How far below a body's collider the ground probe starts and looks (twice
/// that): a human-scale clearance that clears small steps but not a jump.
const GROUND_PROBE: f32 = 0.15;

/// A summary of a headless run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunSummary {
    /// How many frames were run.
    pub frames: u32,
    /// Total scaled simulation time elapsed, seconds.
    pub elapsed_seconds: f32,
    /// Live entity count at the end of the run.
    pub entities: usize,
    /// How many fixed (physics) steps were taken.
    pub fixed_steps: u64,
}

impl RunSummary {
    /// `true` if no frames were run.
    pub fn is_empty(&self) -> bool {
        self.frames == 0
    }
}

impl std::fmt::Display for RunSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "frames={} elapsed={:.3}s entities={} fixed_steps={}",
            self.frames, self.elapsed_seconds, self.entities, self.fixed_steps
        )
    }
}

/// The game/export loop.
///
/// Generic over the script host so callers can pick a backend; the default is
/// [`NoopHost`], which discovers scripts but runs no code. A host need not be
/// `Send`: the loop (and the [`App`](ecs::app::App) systems it is built from) run
/// on a single thread, which is what lets a game drive a non-thread-safe engine
/// such as Slag.
pub struct Runtime<H: ScriptHost + 'static = NoopHost> {
    app: App,
    config: RuntimeConfig,
    physics: Arc<Mutex<PhysicsWorld>>,
    audio: Arc<Mutex<AudioEngine>>,
    script: Arc<Mutex<ScriptRuntime<H>>>,
    renderer: Option<Renderer>,
    last_frame: Option<Frame>,
    /// Whether the windowed game currently captures the cursor (mouse-look).
    /// `Esc` toggles it; the menu/pause build ties it to the paused state.
    pub(crate) captured: bool,
    /// The development script watcher, when scripts load from disk.
    #[cfg(not(target_arch = "wasm32"))]
    watch: Option<crate::watch::ScriptWatch>,
}

impl Runtime<NoopHost> {
    /// Build a runtime for `scene` using the default (no-op) script host.
    pub fn new(scene: Scene, config: RuntimeConfig) -> Self {
        Self::with_host(scene, config, NoopHost)
    }
}

impl<H: ScriptHost + 'static> Runtime<H> {
    /// Build a runtime for `scene` driven by `host`, using a default asset
    /// registry.
    pub fn with_host(scene: Scene, config: RuntimeConfig, host: H) -> Self {
        Self::from_parts(scene, AssetRegistry::with_defaults(), config, host)
    }

    /// Build a runtime for `scene` with an explicit asset registry and host.
    pub fn from_parts(scene: Scene, assets: AssetRegistry, config: RuntimeConfig, host: H) -> Self {
        let audio_engine = if config.audio {
            match AudioEngine::new() {
                Ok(engine) => engine,
                Err(err) => {
                    log::warn!("audio device unavailable ({err}); using silent audio");
                    AudioEngine::silent()
                }
            }
        } else {
            AudioEngine::silent()
        };

        let physics = Arc::new(Mutex::new(PhysicsWorld::new(config.physics_config())));
        let audio = Arc::new(Mutex::new(audio_engine));
        // The scene carries the registry of registered (plugin) components, which
        // scripts read and write by name through reflection.
        let mut script_runtime = ScriptRuntime::with_host(host);
        script_runtime.set_components(scene.components.clone());
        let script = Arc::new(Mutex::new(script_runtime));

        let mut app = App::new(scene.name.clone());
        app.context.scene = scene;
        app.context.assets = assets;
        // Give every entity a `GlobalTransform` before the first frame so the
        // startup physics sync sees it (the app only propagates after `update`).
        propagate_transforms(&mut app.context.scene.world);

        {
            let physics = Arc::clone(&physics);
            app.on_startup("runtime.physics_sync", move |ctx| {
                lock(&physics).sync_from_world(&ctx.scene.world);
            });
        }
        {
            let script = Arc::clone(&script);
            app.on_startup("runtime.script_start", move |ctx| {
                let mut script = lock(&script);
                *script.input_mut() = ctx.input.clone();
                script.run_start(&mut ctx.scene.world, &ctx.assets);
            });
        }
        {
            let physics = Arc::clone(&physics);
            app.on_fixed("runtime.physics_step", move |ctx| {
                let dt = ctx.time.fixed_delta();
                let mut physics = lock(&physics);
                physics.sync_from_world(&ctx.scene.world);
                physics.step(&mut ctx.scene.world, dt);
            });
        }
        {
            let scripts = Arc::clone(&script);
            let physics_handle = Arc::clone(&physics);
            app.on_update("runtime.script_update", move |ctx| {
                let dt = ctx.time.delta();
                // Snapshot what scripts may read (`is_grounded`, `get_velocity`)
                // before running them, then run, then apply the physics writes the
                // world-only replay left queued (velocity, impulses).
                let view = {
                    let physics = lock(&physics_handle);
                    let mut view = PhysicsView::new();
                    for (entity, _) in ctx.scene.world.query::<RigidBody>() {
                        let id = script::entity_id(entity);
                        if let Some(velocity) = physics.linear_velocity_of(entity) {
                            view.set_velocity(id, velocity);
                        }
                        view.set_grounded(
                            id,
                            physics.is_grounded(&ctx.scene.world, entity, GROUND_PROBE),
                        );
                    }
                    view
                };
                let commands = {
                    let mut scripts = lock(&scripts);
                    *scripts.input_mut() = ctx.input.clone();
                    scripts.set_physics_view(view);
                    scripts.run_update(&mut ctx.scene.world, &ctx.assets, dt);
                    scripts.take_commands()
                };
                let mut physics = lock(&physics_handle);
                for command in commands {
                    match command {
                        ScriptCommand::SetVelocity { entity, velocity } => {
                            if let Some(entity) = script::entity_from_id(&ctx.scene.world, entity) {
                                physics.set_linear_velocity(&ctx.scene.world, entity, velocity);
                            }
                        }
                        ScriptCommand::AddVelocity { entity, velocity } => {
                            if let Some(entity) = script::entity_from_id(&ctx.scene.world, entity)
                                && let Some(current) = physics.linear_velocity_of(entity)
                            {
                                physics.set_linear_velocity(
                                    &ctx.scene.world,
                                    entity,
                                    current + velocity,
                                );
                            }
                        }
                        ScriptCommand::ApplyImpulse { entity, velocity } => {
                            if let Some(entity) = script::entity_from_id(&ctx.scene.world, entity) {
                                physics.apply_impulse(&ctx.scene.world, entity, velocity);
                            }
                        }
                        _ => {}
                    }
                }
            });
        }
        {
            let audio = Arc::clone(&audio);
            app.on_late("runtime.audio_update", move |ctx| {
                lock(&audio).update(&ctx.scene.world, &ctx.assets);
            });
        }

        Self {
            app,
            config,
            physics,
            audio,
            script,
            renderer: None,
            last_frame: None,
            captured: true,
            #[cfg(not(target_arch = "wasm32"))]
            watch: None,
        }
    }

    /// Advance the simulation by one frame with raw delta `dt` seconds.
    pub fn tick(&mut self, dt: f32) {
        #[cfg(not(target_arch = "wasm32"))]
        self.pump_script_watch();
        self.app.tick(dt);
        self.render();
    }

    /// Advance one frame using the configured [`RuntimeConfig::timestep`].
    pub fn tick_once(&mut self) {
        let dt = self.config.timestep;
        self.tick(dt);
    }

    /// Run up to `frames` frames (or until an exit is requested), each with a
    /// raw delta of `dt` seconds, and summarise the result.
    pub fn run_headless(&mut self, frames: u32, dt: f32) -> RunSummary {
        let mut count = 0;
        while count < frames && !self.app.context.exit_requested {
            self.tick(dt);
            count += 1;
        }
        self.summary(count)
    }

    /// A summary of the current state, reporting `frames` as the frame count.
    pub fn summary(&self, frames: u32) -> RunSummary {
        RunSummary {
            frames,
            elapsed_seconds: self.app.context.time.elapsed(),
            entities: self.app.context.scene.world.entities().len(),
            fixed_steps: self.app.fixed_steps_taken(),
        }
    }

    /// The active scene.
    pub fn scene(&self) -> &Scene {
        &self.app.context.scene
    }

    /// The underlying app.
    pub fn app(&self) -> &App {
        &self.app
    }

    /// The underlying app, mutably (for registering extra systems, reading
    /// input, or requesting an exit).
    pub fn app_mut(&mut self) -> &mut App {
        &mut self.app
    }

    /// The configuration this runtime was built with.
    pub fn config(&self) -> RuntimeConfig {
        self.config
    }

    /// The physics world. The guard derefs to [`PhysicsWorld`].
    pub fn physics(&self) -> MutexGuard<'_, PhysicsWorld> {
        lock(&self.physics)
    }

    /// The audio engine.
    pub fn audio(&self) -> MutexGuard<'_, AudioEngine> {
        lock(&self.audio)
    }

    /// The script runtime.
    pub fn script(&self) -> MutexGuard<'_, ScriptRuntime<H>> {
        lock(&self.script)
    }

    /// Watch `root` for `.js`/`.mjs`/`.wasm` changes and reload scripts as they
    /// are edited. A development run ([`cli`](crate::cli)) calls this when scripts
    /// are not embedded in the binary; see `.notes/scripts.md`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn watch_scripts(&mut self, root: impl AsRef<Path>) -> std::io::Result<()> {
        self.watch = Some(
            crate::watch::ScriptWatch::new(root.as_ref())
                .map_err(|error| std::io::Error::other(error.to_string()))?,
        );
        log::info!("watching {} for script changes", root.as_ref().display());
        Ok(())
    }

    /// Re-read the `relative` (project-relative) script from disk into the asset
    /// registry, returning `true` when it changed. The script runtime recompiles
    /// the new source on the next frame. Requires a watched root.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn reload_script(&mut self, relative: &str) -> bool {
        let Some(root) = self.watch.as_ref().map(|watch| watch.root().to_path_buf()) else {
            return false;
        };
        let loaded = DirStore::new(&root)
            .read(relative)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                ScriptAsset::from_file_bytes(relative, &bytes).map_err(|error| error.to_string())
            });
        match loaded {
            Ok(asset) => {
                self.app
                    .context
                    .assets
                    .scripts
                    .insert_named(relative, asset);
                log::info!("reloaded script {relative}");
                true
            }
            Err(error) => {
                log::warn!("could not reload script {relative}: {error}");
                false
            }
        }
    }

    /// Re-read every script the watcher reported changed since the last frame.
    #[cfg(not(target_arch = "wasm32"))]
    fn pump_script_watch(&mut self) {
        let Some(watch) = self.watch.as_ref() else {
            return;
        };
        let changed = watch.drain();
        if changed.is_empty() {
            return;
        }
        let root = watch.root().to_path_buf();
        for path in changed {
            let Ok(relative) = path.strip_prefix(&root) else {
                continue;
            };
            let relative = relative.to_string_lossy().replace('\\', "/");
            self.reload_script(&relative);
        }
    }

    /// `true` if the audio engine is connected to a real output device.
    pub fn is_audio_live(&self) -> bool {
        lock(&self.audio).is_live()
    }

    /// The most recently rendered frame, if rendering is enabled and a frame
    /// has been drawn.
    pub fn last_frame(&self) -> Option<&Frame> {
        self.last_frame.as_ref()
    }

    /// The renderer, if [`enable_renderer`](Self::enable_renderer) succeeded.
    pub fn renderer(&self) -> Option<&Renderer> {
        self.renderer.as_ref()
    }

    /// Try to enable headless rendering.
    ///
    /// Returns [`RenderError::NoAdapter`] (after logging) when no GPU adapter is
    /// available; the runtime keeps running without rendering. This never
    /// panics.
    ///
    /// Native only: the web host supplies a surface-backed renderer through
    /// `AppDriver::configure_renderer` instead.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn enable_renderer(&mut self, config: RendererConfig) -> Result<(), RenderError> {
        match Renderer::headless(config) {
            Ok(mut renderer) => {
                self.apply_environment(&mut renderer);
                self.renderer = Some(renderer);
                Ok(())
            }
            Err(RenderError::NoAdapter) => {
                log::warn!("no graphics adapter available; rendering stays disabled");
                Err(RenderError::NoAdapter)
            }
            Err(err) => {
                log::warn!("renderer initialization failed: {err}");
                Err(err)
            }
        }
    }

    /// Precompute and upload the configured IBL environment into `renderer`.
    ///
    /// Called from both renderer paths — the headless one here, and the
    /// windowed host through [`AppDriver::configure_renderer`] — so a project's
    /// `[render] environment` applies however the game is run. A configured but
    /// unloaded environment is a warning, not a failure: the renderer keeps its
    /// neutral fallback so a game still runs.
    ///
    /// [`AppDriver::configure_renderer`]: platform::AppDriver::configure_renderer
    pub(crate) fn apply_environment(&self, renderer: &mut Renderer) {
        let Some(handle) = self.config.environment else {
            return;
        };
        match self.app.context.assets.hdr.get(handle) {
            Some(hdr) => {
                log::info!(
                    "precomputing image-based lighting from environment '{}' ({}x{})",
                    hdr.name,
                    hdr.width,
                    hdr.height
                );
                renderer.set_environment(hdr, IblConfig::default());
            }
            None => {
                log::warn!("the configured environment is not loaded; keeping the neutral fallback")
            }
        }
    }

    /// Turn rendering off and drop any cached frame.
    pub fn disable_renderer(&mut self) {
        self.renderer = None;
        self.last_frame = None;
    }

    /// Render one frame if rendering is enabled and an active camera exists.
    fn render(&mut self) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let Some(camera) = render::first_active_camera(&self.app.context.scene.world) else {
            log::debug!("no active camera; skipping render");
            return;
        };
        let world = &self.app.context.scene.world;
        let assets = &self.app.context.assets;
        match renderer.render_frame(world, assets, camera) {
            Ok(frame) => self.last_frame = Some(frame),
            Err(err) => log::warn!("frame render failed: {err}"),
        }
    }
}

impl<H: ScriptHost + Default + 'static> Runtime<H> {
    /// Build a runtime for `scene` with an explicit asset registry, using the
    /// host's `Default`.
    pub fn from_assets(scene: Scene, assets: AssetRegistry, config: RuntimeConfig) -> Self {
        Self::from_parts(scene, assets, config, H::default())
    }
}

impl<H: ScriptHost + 'static> std::fmt::Debug for Runtime<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime")
            .field("config", &self.config)
            .field("entities", &self.app.context.scene.world.entities().len())
            .field("renderer", &self.renderer.is_some())
            .field("has_frame", &self.last_frame.is_some())
            .finish_non_exhaustive()
    }
}

/// Lock `mutex`, recovering the data even if the lock was poisoned. The runtime
/// treats a poisoned subsystem lock as recoverable rather than panicking.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ecs::components::{GlobalTransform, LocalTransform, Name, Script};
    use ecs::reflect::ComponentRegistry;
    use math::vec3_approx_eq;
    use script::{RecordingHost, ScriptApi};

    /// A tiny inline scene: a fixed floor, a dynamic ball above it, and an
    /// entity carrying an (inert) script.
    fn scene_json() -> String {
        r#"{
            "name": "RuntimeTest",
            "entities": [
                {
                    "id": 0,
                    "name": "Floor",
                    "transform": { "translation": [0, -0.5, 0], "rotation": [0,0,0,1], "scale": [1,1,1] },
                    "components": [
                        { "type": "rigid_body", "body_type": "fixed", "mass": 1.0, "restitution": 0.0, "friction": 0.5, "gravity_scale": 1.0, "linear_velocity": [0,0,0], "angular_velocity": [0,0,0] },
                        { "type": "collider", "shape": { "kind": "cuboid", "half_extents": [10, 0.5, 10] }, "offset": [0,0,0], "is_sensor": false, "friction": 0.5, "restitution": 0.0 }
                    ]
                },
                {
                    "id": 1,
                    "name": "Ball",
                    "transform": { "translation": [0, 5, 0], "rotation": [0,0,0,1], "scale": [1,1,1] },
                    "components": [
                        { "type": "rigid_body", "body_type": "dynamic", "mass": 1.0, "restitution": 0.0, "friction": 0.5, "gravity_scale": 1.0, "linear_velocity": [0,0,0], "angular_velocity": [0,0,0] },
                        { "type": "collider", "shape": { "kind": "ball", "radius": 0.5 }, "offset": [0,0,0], "is_sensor": false, "friction": 0.5, "restitution": 0.0 }
                    ]
                },
                {
                    "id": 2,
                    "name": "Runner",
                    "transform": { "translation": [0, 0, 0], "rotation": [0,0,0,1], "scale": [1,1,1] },
                    "components": [
                        { "type": "script", "source": "function on_update(dt) {}", "path": null, "enabled": true }
                    ]
                }
            ]
        }"#
        .to_string()
    }

    fn load_scene() -> Scene {
        let assets = AssetRegistry::with_defaults();
        Scene::from_json(&scene_json(), &assets).expect("test scene parses")
    }

    fn entity_named(runtime: &Runtime<RecordingHost>, name: &str) -> Option<ecs::world::Entity> {
        let world = &runtime.scene().world;
        world
            .entities_with::<Name>()
            .into_iter()
            .find(|e| world.get::<Name>(*e).is_some_and(|n| n.as_str() == name))
    }

    fn y_of(runtime: &Runtime<RecordingHost>, name: &str) -> f32 {
        let entity = entity_named(runtime, name).expect("named entity exists");
        runtime
            .scene()
            .world
            .get::<GlobalTransform>(entity)
            .expect("entity has a global transform")
            .position()
            .y
    }

    #[test]
    fn a_script_velocity_reaches_the_physics_body() {
        // A script names the ball and overwrites its velocity; the runtime applies
        // the queued command to the physics world after the call.
        let mut host = RecordingHost::new();
        host.on("on_update", |api: &mut ScriptApi<'_, '_>| {
            if let Some(ball) = api.find_entity("Ball") {
                api.set_velocity(ball, math::vec3(2.0, 0.0, 0.0));
            }
        });
        let config = RuntimeConfig {
            audio: false,
            ..RuntimeConfig::default()
        };
        let mut runtime = Runtime::with_host(load_scene(), config, host);
        runtime.tick_once();

        let ball = entity_named(&runtime, "Ball").expect("the ball exists");
        let velocity = runtime
            .physics()
            .linear_velocity_of(ball)
            .expect("the ball has a body");
        assert!(velocity.x > 1.0, "ball velocity = {velocity:?}");
    }

    #[test]
    fn a_script_reflects_a_custom_component_through_the_runtime() {
        // A scene that carries the registry, with an inline script whose behaviour
        // reads and writes the custom component the plugin would register.
        #[derive(
            Debug, Default, PartialEq, serde::Serialize, serde::Deserialize, ecs::Component,
        )]
        struct Health {
            current: f32,
            max: f32,
        }

        let mut registry = ComponentRegistry::new();
        registry.register::<Health>();
        let assets = AssetRegistry::with_defaults();
        let json = r#"{
            "name": "Reflect",
            "entities": [{
                "id": 0, "name": "Hero",
                "transform": { "translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] },
                "components": [
                    { "type": "script", "source": "function on_update(entity, dt) {}", "path": null, "enabled": true },
                    { "type": "custom", "name": "Health", "data": { "current": 1.0, "max": 10.0 } }
                ]
            }]
        }"#;
        let scene = Scene::from_json_with(json, &assets, &registry).expect("scene loads");

        let mut host = RecordingHost::new();
        host.on("on_update", |api: &mut ScriptApi<'_, '_>| {
            let entity = api.current_entity().unwrap();
            let json = api
                .get_component(entity, "Health")
                .expect("Health is present");
            assert!(json.contains("\"max\":10.0"), "{json}");
            api.set_component(entity, "Health", r#"{"current":5.0,"max":10.0}"#)
                .expect("set");
        });

        let config = RuntimeConfig {
            audio: false,
            ..RuntimeConfig::default()
        };
        let mut runtime = Runtime::with_host(scene, config, host);
        runtime.run_headless(2, 1.0 / 60.0);

        let hero = entity_named(&runtime, "Hero").expect("Hero exists");
        assert_eq!(
            runtime.scene().world.get::<Health>(hero),
            Some(&Health {
                current: 5.0,
                max: 10.0
            })
        );
    }

    #[test]
    fn dynamic_body_falls_and_comes_to_rest() {
        let mut host = RecordingHost::new();
        host.on("on_update", |_api: &mut ScriptApi<'_, '_>| {});
        let config = RuntimeConfig {
            audio: false,
            ..RuntimeConfig::default()
        };
        let mut runtime = Runtime::with_host(load_scene(), config, host);

        let entities_before = runtime.scene().world.entities().len();
        assert_eq!(entities_before, 3);
        let start_y = y_of(&runtime, "Ball");
        assert!(y_of(&runtime, "Ball") > 4.0);

        let summary = runtime.run_headless(120, 1.0 / 60.0);

        assert_eq!(summary.frames, 120);
        assert_eq!(summary.fixed_steps, 120);
        assert!((summary.elapsed_seconds - 2.0).abs() < 0.05);
        assert_eq!(summary.entities, entities_before);
        assert_eq!(runtime.scene().world.entities().len(), entities_before);

        let end_y = y_of(&runtime, "Ball");
        assert!(end_y < start_y - 1.0, "the ball fell: {start_y} -> {end_y}");
        assert!(
            (end_y - 0.5).abs() < 0.2,
            "the ball rests on the floor (y = 0.5), got {end_y}"
        );

        // The script's `on_update` ran once per frame.
        assert_eq!(runtime.script().host().count_calls("on_update"), 120);
        assert_eq!(runtime.script().instance_count(), 1);
    }

    #[test]
    fn scripts_can_spawn_entities_with_silent_audio() {
        let mut host = RecordingHost::new();
        host.on("on_update", |api: &mut ScriptApi<'_, '_>| {
            api.spawn(Some("Spawned".into()));
        });
        let config = RuntimeConfig {
            audio: false,
            ..RuntimeConfig::default()
        };
        let mut runtime = Runtime::with_host(load_scene(), config, host);

        // No device was requested, so the engine is silent.
        assert!(!runtime.is_audio_live());

        let before = runtime.scene().world.entities().len();
        let summary = runtime.run_headless(10, 1.0 / 60.0);
        let after = runtime.scene().world.entities().len();

        assert_eq!(summary.frames, 10);
        assert_eq!(
            after,
            before + 10,
            "one spawn per frame: {before} -> {after}"
        );

        let world = &runtime.scene().world;
        assert!(world.entities_with::<Name>().into_iter().any(|e| {
            world
                .get::<Name>(e)
                .is_some_and(|n| n.as_str() == "Spawned")
        }));
    }

    #[test]
    fn tick_once_uses_the_configured_timestep() {
        let mut runtime = Runtime::new(
            load_scene(),
            RuntimeConfig {
                audio: false,
                ..RuntimeConfig::default()
            },
        );
        runtime.tick_once();
        let summary = runtime.summary(1);
        assert_eq!(summary.frames, 1);
        assert!(summary.elapsed_seconds > 0.0);
        assert_eq!(summary.fixed_steps, 1);
    }

    #[test]
    fn editing_a_script_reloads_it() {
        // A project on disk with one script the scene references by path.
        let root = std::env::temp_dir().join(format!("noctis-reload-{}", std::process::id()));
        let scripts = root.join("scripts");
        std::fs::create_dir_all(&scripts).expect("scripts dir");
        let file = scripts.join("spin.js");
        std::fs::write(&file, "first();").expect("write script");

        let mut host = RecordingHost::new();
        host.on("on_start", |_api: &mut ScriptApi<'_, '_>| {});

        let mut scene = Scene::new("Reload");
        let entity = scene.world.spawn();
        scene.world.insert(entity, LocalTransform::default());
        scene
            .world
            .insert(entity, Script::from_path("scripts/spin.js"));

        let config = RuntimeConfig {
            audio: false,
            ..RuntimeConfig::default()
        };
        let mut runtime = Runtime::from_parts(scene, AssetRegistry::new(), config, host);
        runtime.watch_scripts(&root).expect("watch the project");
        assert!(runtime.reload_script("scripts/spin.js"), "the file loads");

        runtime.run_headless(1, 1.0 / 60.0);
        assert_eq!(runtime.script().host().count_calls("on_start"), 1);
        assert_eq!(runtime.script().host().compiled_count(), 1);

        // Editing the file and reloading recompiles it and re-runs `on_start`.
        std::fs::write(&file, "second();").expect("rewrite script");
        assert!(runtime.reload_script("scripts/spin.js"));
        runtime.run_headless(1, 1.0 / 60.0);
        assert_eq!(runtime.script().host().count_calls("on_start"), 2);
        assert_eq!(runtime.script().host().compiled_count(), 2);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_watcher_reloads_a_saved_script() {
        use std::time::{Duration, Instant};

        let root = std::env::temp_dir().join(format!("noctis-watch-{}", std::process::id()));
        let scripts = root.join("scripts");
        std::fs::create_dir_all(&scripts).expect("scripts dir");
        let file = scripts.join("spin.js");
        std::fs::write(&file, "first();").expect("write script");

        let mut host = RecordingHost::new();
        host.on("on_start", |_api: &mut ScriptApi<'_, '_>| {});

        let mut scene = Scene::new("Watch");
        let entity = scene.world.spawn();
        scene.world.insert(entity, LocalTransform::default());
        scene
            .world
            .insert(entity, Script::from_path("scripts/spin.js"));

        let config = RuntimeConfig {
            audio: false,
            ..RuntimeConfig::default()
        };
        let mut runtime = Runtime::from_parts(scene, AssetRegistry::new(), config, host);
        runtime.watch_scripts(&root).expect("watch the project");
        assert!(runtime.reload_script("scripts/spin.js"));
        runtime.run_headless(1, 1.0 / 60.0);
        assert_eq!(runtime.script().host().count_calls("on_start"), 1);

        // Saving the file reaches the watcher, which reloads it without any
        // manual call; the next frame re-runs `on_start`.
        std::fs::write(&file, "second();").expect("rewrite script");
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut reloaded = false;
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            runtime.run_headless(1, 1.0 / 60.0);
            let started = runtime.script().host().count_calls("on_start");
            if started >= 2 {
                reloaded = true;
                break;
            }
        }
        assert!(reloaded, "the watcher did not reload the saved script");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn renderer_enable_degrades_without_an_adapter() {
        // The demo scene has an active camera and renderable geometry, so a
        // successful renderer should produce a frame.
        let mut assets = AssetRegistry::with_defaults();
        let scene = crate::demo::demo_scene(&mut assets);
        let mut runtime = Runtime::from_parts(
            scene,
            assets,
            RuntimeConfig {
                audio: false,
                ..RuntimeConfig::default()
            },
            NoopHost,
        );
        match runtime.enable_renderer(RendererConfig::default()) {
            Ok(()) => {
                runtime.run_headless(2, 1.0 / 60.0);
                assert!(runtime.last_frame().is_some(), "a frame was rendered");
            }
            Err(RenderError::NoAdapter) => {
                // Expected on headless machines: rendering is simply skipped.
                assert!(runtime.last_frame().is_none());
            }
            Err(other) => panic!("unexpected renderer error: {other}"),
        }
    }

    #[test]
    fn summary_display_is_one_line() {
        let summary = RunSummary {
            frames: 60,
            elapsed_seconds: 1.0,
            entities: 4,
            fixed_steps: 60,
        };
        let line = summary.to_string();
        assert!(!line.contains('\n'));
        assert!(line.contains("frames=60"));
        assert!(line.contains("entities=4"));
        assert!(!summary.is_empty());
        assert!(
            RunSummary {
                frames: 0,
                ..summary
            }
            .is_empty()
        );
    }

    #[test]
    fn gravity_override_is_applied() {
        let config = RuntimeConfig {
            audio: false,
            gravity: math::vec3(0.0, -1.62, 0.0),
            ..RuntimeConfig::default()
        };
        let mut runtime = Runtime::with_host(load_scene(), config, RecordingHost::new());
        assert!(vec3_approx_eq(
            runtime.physics().gravity(),
            math::vec3(0.0, -1.62, 0.0),
            1e-6
        ));
        assert_eq!(runtime.run_headless(1, 1.0 / 60.0).frames, 1);
    }

    #[test]
    fn physics_body_count_tracks_the_scene() {
        let mut runtime = Runtime::with_host(
            load_scene(),
            RuntimeConfig {
                audio: false,
                ..RuntimeConfig::default()
            },
            RecordingHost::new(),
        );
        runtime.run_headless(1, 1.0 / 60.0);
        assert_eq!(runtime.physics().body_count(), 2);
        assert_eq!(runtime.physics().collider_count(), 2);
        assert!(vec3_approx_eq(
            runtime.physics().gravity(),
            math::vec3(0.0, -9.81, 0.0),
            1e-6
        ));
    }

    #[test]
    fn runs_with_a_non_send_host() {
        // The runtime must accept a host that is not `Send` (a JS engine is not),
        // because `App` systems now run on the loop thread. `Rc`/`Cell` make this
        // host deliberately `!Send` at compile time.
        use std::cell::Cell;
        use std::rc::Rc;

        use script::{ModuleId, PendingApi, ScriptContext, ScriptHost};

        struct NonSendHost {
            starts: Rc<Cell<usize>>,
        }

        impl ScriptHost for NonSendHost {
            fn eval_script(&mut self, _source: &str) -> ModuleId {
                1
            }

            fn call_lifecycle(
                &mut self,
                _module: ModuleId,
                func: &str,
                _ctx: &mut ScriptContext<'_>,
            ) {
                if func == script::ON_START {
                    self.starts.set(self.starts.get() + 1);
                }
            }

            fn set_pending_api(&mut self, _api: PendingApi) {}
        }

        let starts = Rc::new(Cell::new(0));
        let mut scene = Scene::new("NonSend");
        let entity = scene.world.spawn();
        scene
            .world
            .insert(entity, ecs::components::Script::from_source("ignored();"));

        let mut runtime = Runtime::with_host(
            scene,
            RuntimeConfig {
                audio: false,
                ..RuntimeConfig::default()
            },
            NonSendHost {
                starts: Rc::clone(&starts),
            },
        );
        runtime.run_headless(2, 1.0 / 60.0);

        // The host was driven: its `on_start` ran exactly once.
        assert_eq!(starts.get(), 1);
    }
}
