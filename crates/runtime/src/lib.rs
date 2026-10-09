//! # runtime — the Noctis game/export loop
//!
//! This crate wires the engine subsystems into [`ecs::app::App`] so a scene
//! actually runs. It provides the [`cli`] command line (the game runner shared by
//! `noctis run` and the `noctis-game` binary an export ships) and the build/export
//! machinery behind `noctis build`.
//!
//! The entry point is [`Runtime`], which owns an [`App`](ecs::app::App) plus a
//! [`PhysicsWorld`](physics::PhysicsWorld), an
//! [`AudioEngine`](audio::AudioEngine), and a
//! [`ScriptRuntime`](script::ScriptRuntime), and schedules them across the app's
//! `startup` / `fixed` / `update` / `late` stages (see the `runtime` module
//! source for the exact ordering).
//!
//! ## Running JavaScript
//!
//! The runtime is generic over its script host; the engine always runs the real
//! Slag engine ([`EngineHost`](script::engine_host::EngineHost)), which supports
//! JavaScript with its Cranelift JIT and WebAssembly JS API, and native `.wasm`
//! scripts, through one runtime. The inert [`NoopHost`](script::NoopHost) and
//! [`RecordingHost`](script::RecordingHost) remain for tests. The loop runs on a
//! single thread, so a host need not be `Send`; see [`Runtime`].
//!
//! ```
//! use ecs::prelude::*;
//! use math::vec3;
//! use runtime::{Runtime, RuntimeConfig};
//!
//! // A scene with a fixed floor and a ball dropped onto it.
//! let mut scene = Scene::new("Example");
//! let floor = scene.world.spawn();
//! scene.world.insert(floor, GlobalTransform::identity());
//! scene.world.insert(floor, RigidBody { body_type: BodyType::Fixed, ..Default::default() });
//! scene.world.insert(floor, Collider::default());
//!
//! let ball = scene.world.spawn();
//! let local = LocalTransform::from_translation(vec3(0.0, 4.0, 0.0));
//! scene.world.insert(ball, local);
//! scene.world.insert(ball, GlobalTransform::from_local(&local));
//! scene.world.insert(ball, RigidBody::default());
//! scene.world.insert(ball, Collider { shape: ColliderShape::sphere(), ..Default::default() });
//!
//! let config = RuntimeConfig { audio: false, ..RuntimeConfig::default() };
//! let mut runtime = Runtime::new(scene, config);
//! let summary = runtime.run_headless(120, 1.0 / 60.0);
//!
//! assert_eq!(summary.frames, 120);
//! assert!(summary.entities >= 2);
//! ```

#![forbid(unsafe_code)]

mod config;
pub mod embed;
pub mod export;
mod runtime;
mod watch;

pub mod build;
pub mod cli;
pub mod demo;
pub mod window;

pub use build::{BuildOptions, build_project};
pub use config::RuntimeConfig;
pub use embed::EmbeddedScript;
pub use export::{ExportError, ExportFile, ExportReport, bundled_scene, export_game};
pub use runtime::{RunSummary, Runtime};
pub use window::run_in_window;

/// The most commonly used imports.
pub mod prelude {
    pub use crate::build::{BuildOptions, build_project};
    pub use crate::config::RuntimeConfig;
    pub use crate::demo::demo_scene;
    pub use crate::embed::EmbeddedScript;
    pub use crate::runtime::{RunSummary, Runtime};
    pub use audio::AudioEngine;
    pub use ecs::prelude::*;
    pub use math;
    pub use physics::{PhysicsConfig, PhysicsWorld};
    pub use render::{Frame, RenderError, Renderer, RendererConfig};
    pub use script::{NoopHost, RecordingHost, ScriptHost, ScriptRuntime};
}
