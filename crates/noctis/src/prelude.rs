//! The engine, in one import.
//!
//! ```no_run
//! use noctis::prelude::*;
//! ```
//!
//! This is a curated surface, not a wildcard: it brings in the scene and ECS
//! types, the math (vectors, quaternions, transforms), the renderer, physics,
//! audio, scripting, the runtime and the editor, without dragging in every
//! internal name.

pub use audio::AudioEngine;
pub use ecs::prelude::*;
pub use editor::{ComponentName, Editor, EditorCommand, EntityRef};
pub use physics::{PhysicsConfig, PhysicsWorld};
pub use render::{Frame, RenderError, Renderer, RendererConfig};
pub use runtime::{RunSummary, Runtime, RuntimeConfig};
pub use script::{NoopHost, RecordingHost, ScriptHost, ScriptRuntime};
