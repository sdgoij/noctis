//! # editor — the Noctis authoring surface
//!
//! The Noctis editor is fully headless and fully programmable. Its entire
//! authoring surface is a set of Rust methods and a serializable command enum,
//! so an AI agent (or a build script, or a test) can construct a complete game
//! scene without a single mouse click.
//!
//! Two entry points, one implementation:
//!
//! * [`Editor`] — imperative methods (`spawn`, `set_translation`, `add_light`,
//!   `import_gltf`, `undo`, ...).
//! * [`EditorCommand`] — a serde enum covering the same operations, applied
//!   through [`Editor::apply`] or a JSON array through [`Editor::apply_json`].
//!
//! Both share one [`Scene`](ecs::scene::Scene), one asset registry, and one
//! undo/redo history.
//!
//! ## The `noctis-editor` CLI
//!
//! The bundled `noctis-editor` binary turns the command API into a shell tool:
//!
//! ```text
//! noctis-editor --scene level.json --command edits.json --out level.json
//! noctis-editor --print-schema
//! ```
//!
//! It prints the resulting `list_entities()` JSON to stdout, writes scene changes
//! with `--out`, and exits non-zero with a clear message on bad input.
//!
//! ```
//! use editor::{Editor, EntityRef, EditorCommand, LightParams};
//! use math::vec3;
//!
//! let mut editor = Editor::new();
//! let sun = editor.spawn("Sun", None);
//! editor.add_light(sun, LightParams::default()).unwrap();
//! editor.set_translation(sun, vec3(0.0, 10.0, 0.0)).unwrap();
//!
//! // The same thing, as a JSON command batch:
//! editor.apply_json(r#"[{ "op": "spawn", "name": "Moon" }]"#).unwrap();
//! assert_eq!(editor.list_entities().len(), 2);
//! ```

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod command;
pub mod editor;
pub mod error;
pub mod info;
pub mod picking;
pub mod window;

pub use command::{
    AlphaModeSpec, AssetRef, AudioSourceParams, BodyTypeSpec, CameraParams, ColliderParams,
    ColliderShapeSpec, CommandOutcome, ComponentName, EditorCommand, EntityRef, LightParams,
    LightSpec, MaterialParams, MeshRendererParams, ProjectionSpec, RigidBodyParams, schema,
};
pub use editor::{Editor, parse_commands};
pub use error::EditorError;
pub use info::{EntityDto, EntityInfo};

/// The JSON schema/help document for the command API.
///
/// This is what `noctis-editor --print-schema` emits. It lists every command,
/// its fields, the two entity-reference forms, and the asset-reference forms.
pub fn command_schema() -> serde_json::Value {
    schema()
}
