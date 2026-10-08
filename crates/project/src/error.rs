//! Errors from loading or resolving a project.

use std::fmt;

/// Anything that can go wrong while opening or resolving a [`Project`](crate::Project).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectError {
    /// A filesystem operation failed.
    Io(String),
    /// The `noctis.toml` manifest could not be parsed or is semantically invalid.
    Manifest(String),
    /// An asset path was not a canonical, project-relative path.
    Path(String),
    /// A scene or prefab referenced an asset that resolves to no project file.
    MissingAsset(String),
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProjectError::Io(message) => write!(f, "io error: {message}"),
            ProjectError::Manifest(message) => write!(f, "manifest error: {message}"),
            ProjectError::Path(message) => write!(f, "asset path error: {message}"),
            ProjectError::MissingAsset(reference) => {
                write!(
                    f,
                    "no project file resolves the asset reference {reference:?}"
                )
            }
        }
    }
}

impl std::error::Error for ProjectError {}
