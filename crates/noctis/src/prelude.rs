//! The engine, in one import.
//!
//! ```no_run
//! use noctis::prelude::*;
//! ```
//!
//! This is a curated surface, not a wildcard: it brings in the scene and ECS
//! types, the math (vectors, quaternions, transforms), the renderer, physics,
//! audio, scripting, the runtime and the editor, without dragging in every
//! internal name. It also re-exports serde's `Serialize`/`Deserialize`, which a
//! custom [`Component`](ecs::reflect::Component) derives.

pub use audio::{AudioEngine, PlaybackConfig, SoundHandle, load_clip_bytes};
pub use ecs::prelude::*;
#[cfg(feature = "editor")]
pub use editor::{ComponentName, Editor, EditorCommand, EntityRef};
pub use physics::{PhysicsConfig, PhysicsWorld};
pub use render::{
    Frame, IblConfig, LoadedGltf, RegisteredGltf, RenderError, Renderer, RendererConfig, Tonemap,
    load_gltf, load_gltf_bytes, load_hdr_bytes,
};
pub use runtime::{RunSummary, Runtime, RuntimeConfig};
pub use script::{NoopHost, RecordingHost, ScriptHost, ScriptRuntime};
pub use serde::{Deserialize, Serialize};
