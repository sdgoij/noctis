//! The application loop and system scheduling.
//!
//! [`App`] owns an [`EngineContext`] (scene, assets, time, input) and a set of
//! systems grouped into stages. Each frame runs, in order:
//!
//! 1. `startup` (once, on the first frame),
//! 2. `fixed` (zero or more times, for physics/stable logic),
//! 3. `update` (once),
//! 4. `late` (once, after transforms propagate),
//! 5. transform propagation and event buffer rotation.
//!
//! Systems are plain closures, which keeps the engine testable headlessly.

use crate::assets::AssetRegistry;
use crate::resources::{ActionMap, Events, FrameImage, InputState, Time};
use crate::scene::{Scene, propagate_transforms};
use crate::ui::UiCallback;
use crate::world::Entity;

pub use crate::resources::{Key, MouseButton, MouseState};

/// Events produced by the engine itself.
#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    /// An entity was spawned.
    Spawned(Entity),
    /// An entity was despawned.
    Destroyed(Entity),
    /// A scene (or prefab) finished loading.
    Loaded(String),
    /// Free-form event for scripts and gameplay code.
    Custom {
        /// The event name.
        name: String,
        /// A JSON payload.
        payload: String,
    },
}

/// Everything a system can touch.
pub struct EngineContext {
    /// The active scene.
    pub scene: Scene,
    /// The asset registry.
    pub assets: AssetRegistry,
    /// Frame timing.
    pub time: Time,
    /// Input for this frame.
    pub input: InputState,
    /// Named input actions.
    pub actions: ActionMap,
    /// Engine and custom events.
    pub events: Events<EngineEvent>,
    /// Set to request that the loop stop after this frame.
    pub exit_requested: bool,
    /// A full-window frame to present instead of the scene, when a driver draws
    /// its own pixels (see [`FrameImage`]). `None` renders the scene as usual.
    pub frame: Option<FrameImage>,
    /// Screen-space HUD lines a driver publishes for the host to draw over the
    /// presented frame: one string per line, top-left. Empty draws nothing.
    pub screen_text: Vec<String>,
    /// A driver's wish for the cursor: `Some(true)` captures it (hidden and locked
    /// to the window, for mouse-look), `Some(false)` releases it, and `None`
    /// leaves the decision to scripts and the pause state. The window host applies
    /// it, the way it draws `screen_text` — the seam a plugin that owns the loop
    /// asks for mouse-look through, without a script.
    pub mouse_captured: Option<bool>,
    /// A driver's frame lighting: the ambient (indirect) light the scene is lit with
    /// this frame — `rgb` the colour, `a` the intensity, the frame uniform's own
    /// `ambient`. `None` leaves the renderer's setting alone, the way
    /// [`mouse_captured`](Self::mouse_captured) leaves the cursor to scripts; the
    /// host applies it. It is the seam a plugin that owns the loop drives a day/night
    /// ambient through, without a script and without reaching for the renderer — and
    /// the sun a scene already sets as a `Light` component needs no seam at all.
    pub frame_ambient: Option<[f32; 4]>,
    /// The plugin's screen-UI callback: the host calls it once a frame, when it
    /// has a UI layer, with a [`Ui`](crate::ui::Ui) it implements over its own
    /// toolkit (see [`crate::ui`]). `None` draws no plugin UI. A plugin sets it
    /// each frame from a system, the way it publishes `screen_text`.
    pub ui: Option<UiCallback>,
}

impl EngineContext {
    /// A context with an empty scene named `name`.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            scene: Scene::new(name),
            assets: AssetRegistry::with_defaults(),
            time: Time::new(),
            input: InputState::new(),
            actions: ActionMap::new(),
            events: Events::new(),
            exit_requested: false,
            frame: None,
            screen_text: Vec::new(),
            mouse_captured: None,
            frame_ambient: None,
            ui: None,
        }
    }

    /// Request that the loop stop.
    pub fn request_exit(&mut self) {
        self.exit_requested = true;
    }
}

impl Default for EngineContext {
    fn default() -> Self {
        Self::new("Main")
    }
}

/// A system is a closure over the engine context.
///
/// Systems run strictly on the thread that owns the [`App`], in registration
/// order, so they are **not** required to be `Send` or `Sync`. That is what lets
/// a system own single-threaded state — most importantly a JavaScript engine,
/// which is not thread-safe. (Component *types* are still `Send + Sync + 'static`;
/// only the closures are unbound.)
pub type System = Box<dyn FnMut(&mut EngineContext)>;

/// A named group of systems.
#[derive(Default)]
pub struct Stage {
    systems: Vec<(String, System)>,
}

impl Stage {
    /// Register a system.
    pub fn add(
        &mut self,
        name: impl Into<String>,
        system: impl FnMut(&mut EngineContext) + 'static,
    ) {
        self.systems.push((name.into(), Box::new(system)));
    }

    /// The number of registered systems.
    pub fn len(&self) -> usize {
        self.systems.len()
    }

    /// `true` if no systems are registered.
    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }

    /// The system names, in registration order.
    pub fn names(&self) -> Vec<&str> {
        self.systems.iter().map(|(n, _)| n.as_str()).collect()
    }
}

/// The application: a context plus staged systems.
pub struct App {
    /// The shared engine context.
    pub context: EngineContext,
    /// Systems that run once, before the first frame.
    pub startup: Stage,
    /// Systems that run at the fixed timestep.
    pub fixed: Stage,
    /// Systems that run every frame.
    pub update: Stage,
    /// Systems that run every frame after transform propagation.
    pub late: Stage,
    started: bool,
    fixed_steps_taken: u64,
}

impl App {
    /// An app with an empty scene.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            context: EngineContext::new(name),
            startup: Stage::default(),
            fixed: Stage::default(),
            update: Stage::default(),
            late: Stage::default(),
            started: false,
            fixed_steps_taken: 0,
        }
    }

    /// Register a startup system.
    pub fn on_startup(
        &mut self,
        name: impl Into<String>,
        system: impl FnMut(&mut EngineContext) + 'static,
    ) -> &mut Self {
        self.startup.add(name, system);
        self
    }

    /// Register a fixed-update system.
    pub fn on_fixed(
        &mut self,
        name: impl Into<String>,
        system: impl FnMut(&mut EngineContext) + 'static,
    ) -> &mut Self {
        self.fixed.add(name, system);
        self
    }

    /// Register a per-frame update system.
    pub fn on_update(
        &mut self,
        name: impl Into<String>,
        system: impl FnMut(&mut EngineContext) + 'static,
    ) -> &mut Self {
        self.update.add(name, system);
        self
    }

    /// Register a late-update system.
    pub fn on_late(
        &mut self,
        name: impl Into<String>,
        system: impl FnMut(&mut EngineContext) + 'static,
    ) -> &mut Self {
        self.late.add(name, system);
        self
    }

    /// `true` if the startup stage has run.
    pub fn has_started(&self) -> bool {
        self.started
    }

    /// The total number of fixed steps taken.
    pub fn fixed_steps_taken(&self) -> u64 {
        self.fixed_steps_taken
    }

    /// Advance the simulation by one frame with the given raw delta time.
    pub fn tick(&mut self, raw_delta: f32) {
        self.context.time.advance(raw_delta);

        let App {
            context,
            startup,
            fixed,
            update,
            late,
            started,
            fixed_steps_taken,
        } = self;

        if !*started {
            run_stage(startup, context);
            *started = true;
        }

        while context.time.step_fixed().is_some() {
            run_stage(fixed, context);
            *fixed_steps_taken += 1;
        }

        run_stage(update, context);
        propagate_transforms(&mut context.scene.world);
        run_stage(late, context);

        context.events.update();
        // Clear per-frame input *after* the stages consume it, so events fed
        // between ticks (a window's mouse motion and wheel, key edges) are visible
        // to this frame's systems. Clearing at the start would discard them.
        context.input.begin_frame();
    }

    /// Run the loop up to `max_frames` frames (or until an exit is requested),
    /// using a fixed `dt` per frame. Returns the number of frames run. This is
    /// the headless driver used by tests and by the `--headless` engine mode.
    pub fn run_headless(&mut self, max_frames: u32, dt: f32) -> u32 {
        let mut frames = 0;
        while frames < max_frames && !self.context.exit_requested {
            self.tick(dt);
            frames += 1;
        }
        frames
    }
}

/// A project's (or application's) Rust extension: custom components and systems.
///
/// A *plugin* is linked into a binary and registered by the runtime before the
/// scene loads. `register` runs first — components must exist before a scene that
/// uses them is loaded — and `build` runs after the runtime's own systems are
/// scheduled, so a plugin's systems run after the engine's within each stage.
///
/// ```ignore
/// use ecs::prelude::*;
///
/// #[derive(Default)]
/// struct Game;
///
/// impl Plugin for Game {
///     fn register(&self, components: &mut ComponentRegistry) {
///         components.register::<Health>();
///     }
///     fn build(&self, app: &mut App) {
///         app.on_update("regen", |ctx| { /* ... */ });
///     }
/// }
/// ```
pub trait Plugin {
    /// Register custom components, before any scene is loaded.
    fn register(&self, _components: &mut crate::reflect::ComponentRegistry) {}
    /// Add systems and resources to the app.
    fn build(&self, _app: &mut App) {}
    /// The functions this project's editor extensions may call, by name.
    ///
    /// The editor exposes them to JavaScript as `noctis.call(name, args)` and
    /// lists them with `noctis.calls()`. This is the seam for work only the
    /// project's own Rust can do well — a level validator that reuses the game's
    /// own parser and integrator, say — without marshalling it through a script.
    /// A project that has none returns the default empty table.
    fn extension_functions(&self) -> &'static [(&'static str, ExtensionFn)] {
        &[]
    }
}

/// A function a project's editor extensions may call by name.
///
/// The argument and the result are JSON text; the engine never interprets either.
/// It is a plain function pointer so the host can hold it for the life of the
/// process: a project that needs state across calls keeps it in a `static`.
pub type ExtensionFn = fn(&str) -> Result<String, String>;

/// The plugin for an application that has none.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullPlugin;

impl Plugin for NullPlugin {}

fn run_stage(stage: &mut Stage, context: &mut EngineContext) {
    for (_, system) in stage.systems.iter_mut() {
        system(context);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{GlobalTransform, LocalTransform, Name};
    use math::vec3;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn startup_runs_once() {
        let mut app = App::new("t");
        let count = Arc::new(AtomicU32::new(0));
        let c = Arc::clone(&count);
        app.on_startup("init", move |_| {
            c.fetch_add(1, Ordering::SeqCst);
        });
        app.tick(0.016);
        app.tick(0.016);
        app.tick(0.016);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(app.has_started());
    }

    #[test]
    fn update_runs_every_frame_and_fixed_accumulates() {
        let mut app = App::new("t");
        let updates = Arc::new(AtomicU32::new(0));
        let u = Arc::clone(&updates);
        app.on_update("u", move |_| {
            u.fetch_add(1, Ordering::SeqCst);
        });
        // 3 frames at exactly one fixed step each.
        let dt = app.context.time.fixed_delta();
        app.tick(dt);
        app.tick(dt);
        app.tick(dt);
        assert_eq!(updates.load(Ordering::SeqCst), 3);
        assert_eq!(app.fixed_steps_taken(), 3);
    }

    #[test]
    fn systems_can_mutate_the_scene() {
        let mut app = App::new("t");
        app.on_startup("spawn", |ctx| {
            let e = ctx.scene.world.spawn();
            ctx.scene.world.insert(e, Name::new("thing"));
            ctx.scene
                .world
                .insert(e, LocalTransform::from_translation(vec3(0.0, 0.0, 1.0)));
        });
        app.tick(0.016);
        assert_eq!(app.context.scene.world.count::<Name>(), 1);
        // Transforms propagate during the frame.
        let e = app.context.scene.world.entities_with::<Name>()[0];
        assert!(app.context.scene.world.has::<GlobalTransform>(e));
    }

    #[test]
    fn exit_request_stops_headless_run() {
        let mut app = App::new("t");
        let frames = Arc::new(AtomicU32::new(0));
        let f = Arc::clone(&frames);
        app.on_update("stop", move |ctx| {
            let n = f.fetch_add(1, Ordering::SeqCst) + 1;
            if n >= 5 {
                ctx.request_exit();
            }
        });
        let ran = app.run_headless(100, 0.016);
        assert_eq!(ran, 5);
    }

    #[test]
    fn systems_need_not_be_send_or_sync() {
        // A system that owns single-threaded state (an `Rc`/`Cell`) must compile
        // and run: this is what lets the runtime host a non-`Send` JS engine.
        use std::cell::Cell;
        use std::rc::Rc;

        let mut app = App::new("t");
        let ticks = Rc::new(Cell::new(0));
        let shared = Rc::clone(&ticks);
        app.on_update("non_send", move |_| shared.set(shared.get() + 1));
        app.tick(0.016);
        app.tick(0.016);
        assert_eq!(ticks.get(), 2);
    }

    #[test]
    fn input_fed_before_a_tick_reaches_the_update_system() {
        let mut app = App::new("t");
        let seen = Arc::new(AtomicU32::new(0));
        let s = Arc::clone(&seen);
        app.on_update("read_input", move |ctx| {
            let w = Key::letter('W').expect("W is a letter key");
            if ctx.input.key_down(w)
                && ctx.input.mouse.delta[0] != 0.0
                && ctx.input.mouse.scroll != 0.0
            {
                s.fetch_add(1, Ordering::SeqCst);
            }
        });

        let w = Key::letter('W').expect("W is a letter key");
        app.context.input.press_key(w);
        app.context.input.set_mouse_position(10.0, 0.0);
        app.context.input.add_scroll(2.0);
        app.tick(0.016);
        assert_eq!(
            seen.load(Ordering::SeqCst),
            1,
            "the frame's mouse delta and scroll reached the system"
        );

        // With no new events, the next frame sees the edges cleared.
        app.tick(0.016);
        assert_eq!(
            seen.load(Ordering::SeqCst),
            1,
            "per-frame input is cleared after the frame"
        );
        assert!(app.context.input.key_down(w), "held keys survive");
    }
}
