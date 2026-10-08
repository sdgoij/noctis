//! # project — the Noctis project model
//!
//! A *project* is a directory with a `noctis.toml` manifest. It owns one shared
//! asset registry, zero or more scenes, prefabs, and scripts, and it defines how
//! the game is built. The full design is in `docs/project.md`; this crate
//! implements it.
//!
//! This first slice owns:
//!
//! * [`ProjectManifest`] — the parsed `noctis.toml`, parsed strictly so a typo is
//!   an error rather than silent forward compatibility.
//! * [`AssetPath`] — canonical, project-relative asset paths (with sub-asset
//!   fragments), the form asset references take in scene and prefab documents.
//! * [`detect`] / [`AssetKind`] — kind detection by extension, for drag & drop.
//! * [`Project`] — an open manifest, its directories (convention plus `[paths]`
//!   overrides), and scene/prefab discovery.
//!
//! Asset import (with the `.noctis/` cache), the resolver (asset path → registry
//! handle), and build are the following slices.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

mod assets;
mod error;
mod manifest;
mod path;
mod project;

pub use assets::{AssetImportError, AssetMap, AssetMapEntry, ImportedAssets, fragment_path};
pub use error::ProjectError;
pub use manifest::{
    BuildTarget, InputSettings, Mode, Paths, Platform, Profile, ProjectInfo, ProjectManifest,
    RenderSettings, Settings, WindowSettings,
};
pub use path::{AssetKind, AssetPath, detect};
pub use project::{MANIFEST_FILE, Project};
