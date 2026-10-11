//! Packaging a project into a `.zip` **bundle** (see `.notes/web.md`).
//!
//! A bundle is the project at an archive's root — the same `noctis.toml`, scenes,
//! prefabs, assets and scripts a directory holds — so a browser client (or any
//! [`vfs::ZipStore`] reader) can open it with [`Project::open_in`]. Generated
//! directories are dropped: they are build artifacts, not project content.

use crate::error::ProjectError;
use crate::project::Project;

/// Directories never bundled: build artifacts and VCS metadata.
pub const PACK_EXCLUDE: &[&str] = &[".noctis", "dist", "target", ".git"];

impl Project {
    /// Package the project into a `.zip` bundle, returning its bytes.
    ///
    /// The archive holds the project at its root, with [`PACK_EXCLUDE`] directories
    /// dropped, so `Project::open_in(Box::new(ZipStore::new(&bytes)?), name)` reads
    /// it back.
    pub fn bundle(&self) -> Result<Vec<u8>, ProjectError> {
        vfs::zip_store(self.store(), PACK_EXCLUDE)
            .map_err(|error| ProjectError::Io(error.to_string()))
    }
}
