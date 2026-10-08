//! Errors produced by editor operations.
//!
//! Every fallible entry point returns [`EditorError`]. Nothing in this crate
//! panics on a runtime path: missing entities, unknown names, unreadable files,
//! and malformed commands all surface as a value the caller (or the CLI) can
//! report cleanly.

use crate::command::EntityRef;
use std::fmt;

/// Anything that can go wrong while editing a scene or applying a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorError {
    /// A command referenced an entity that is not alive.
    NoSuchEntity(EntityRef),
    /// A command referenced an entity by a name that no entity has.
    UnknownName(String),
    /// A command referenced an asset by a path (or id) that is not registered.
    MissingAsset(String),
    /// The entity exists but does not have the component an operation required.
    MissingComponent {
        /// The entity that was inspected.
        entity: EntityRef,
        /// The canonical component name that was expected.
        component: &'static str,
    },
    /// A command payload was structurally invalid (bad indices, empty mesh, ...).
    Invalid(String),
    /// A scene document could not be parsed.
    Scene(String),
    /// A glTF import failed.
    Gltf(String),
    /// A filesystem operation failed.
    Io(String),
    /// A JSON payload could not be (de)serialized.
    Json(String),
    /// There is nothing to undo (or redo).
    EmptyHistory,
}

impl fmt::Display for EditorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditorError::NoSuchEntity(r) => write!(f, "no such entity: {r:?}"),
            EditorError::UnknownName(name) => write!(f, "no entity named {name:?}"),
            EditorError::MissingAsset(path) => write!(f, "no asset registered at {path:?}"),
            EditorError::MissingComponent { entity, component } => {
                write!(f, "entity {entity:?} has no {component} component")
            }
            EditorError::Invalid(msg) => write!(f, "invalid command: {msg}"),
            EditorError::Scene(msg) => write!(f, "scene error: {msg}"),
            EditorError::Gltf(msg) => write!(f, "glTF import error: {msg}"),
            EditorError::Io(msg) => write!(f, "io error: {msg}"),
            EditorError::Json(msg) => write!(f, "json error: {msg}"),
            EditorError::EmptyHistory => write!(f, "history is empty"),
        }
    }
}

impl std::error::Error for EditorError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_messages_are_non_empty() {
        let cases = [
            EditorError::NoSuchEntity(EntityRef::named("x")),
            EditorError::UnknownName("x".into()),
            EditorError::MissingAsset("m".into()),
            EditorError::MissingComponent {
                entity: EntityRef::named("x"),
                component: "Camera",
            },
            EditorError::Invalid("boom".into()),
            EditorError::Scene("boom".into()),
            EditorError::Gltf("boom".into()),
            EditorError::Io("boom".into()),
            EditorError::Json("boom".into()),
            EditorError::EmptyHistory,
        ];
        for case in cases {
            assert!(!case.to_string().is_empty());
        }
    }

    #[test]
    fn implements_std_error() {
        fn assert_error<E: std::error::Error>(_: &E) {}
        assert_error(&EditorError::EmptyHistory);
    }
}
