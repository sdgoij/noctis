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
//!   overrides), scene/prefab discovery, asset import (with the `.noctis/` cache
//!   and sidecar [`TextureImportSettings`]), [`Project::import_file`] (a loose
//!   file → an asset), [`Project::rename_asset`] with reference rewriting, and
//!   [`Project::bundle`] (a `.zip` project bundle).
//! * [`referenced_files`] — a scene's asset references → the files to bundle.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

mod assets;
mod cache;
mod error;
mod import;
mod manifest;
mod pack;
mod path;
mod project;
mod rename;
mod resolve;
mod settings;

pub use assets::{AssetImportError, AssetMap, AssetMapEntry, ImportedAssets, fragment_path};
pub use cache::{CACHE_DIR, ImportCache, texture_key};
pub use ecs::scene::SceneMode;
pub use error::ProjectError;
pub use manifest::{
    BuildTarget, InputSettings, Paths, Platform, Profile, ProjectInfo, ProjectManifest,
    RenderSettings, SceneListEntry, Settings, WindowSettings,
};
pub use pack::PACK_EXCLUDE;
pub use path::{AssetKind, AssetPath, detect};
pub use project::{MANIFEST_FILE, Project, SceneInfo};
pub use rename::{RenameReport, rewrite_references};
pub use resolve::{
    is_builtin, referenced_files, referenced_files_with, scene_references, scene_references_with,
};
pub use settings::{SIDECAR_SUFFIX, TextureImportSettings};
