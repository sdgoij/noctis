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
//! The runtime is generic over its script host. The default is the inert
//! [`NoopHost`](script::NoopHost) — scripts are discovered and tracked but not
//! executed. With the `slag` feature the game runs the real Slag engine instead,
//! with its Cranelift JIT, its WebAssembly JS API, and the native WebAssembly
//! script host enabled, so a game runs `.js` and `.wasm` scripts through one
//! runtime:
//!
//! ```text
//! cargo build --manifest-path crates/runtime/Cargo.toml --features slag
//! ```
//!
//! (The manifest path is used because `-p runtime` is ambiguous: the Slag engine
//! ships a transitive crate that is also named `runtime`.) [`scripts`] is the gate
//! that stops `run` and `build` from silently producing a game whose scripts never
//! execute. The loop runs on a single thread, so a host need not be `Send`; see
//! [`Runtime`].
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
pub mod export;
mod runtime;

pub mod build;
pub mod cli;
pub mod demo;
pub mod scripts;
pub mod window;

pub use build::{BuildOptions, build_project};
pub use config::RuntimeConfig;
pub use export::{ExportError, ExportFile, ExportReport, bundled_scene, export_game};
pub use runtime::{RunSummary, Runtime};
pub use window::run_in_window;

/// The most commonly used imports.
pub mod prelude {
    pub use crate::build::{BuildOptions, build_project};
    pub use crate::config::RuntimeConfig;
    pub use crate::demo::demo_scene;
    pub use crate::runtime::{RunSummary, Runtime};
    pub use audio::AudioEngine;
    pub use ecs::prelude::*;
    pub use math;
    pub use physics::{PhysicsConfig, PhysicsWorld};
    pub use render::{Frame, RenderError, Renderer, RendererConfig};
    pub use script::{NoopHost, RecordingHost, ScriptHost, ScriptRuntime};
}
