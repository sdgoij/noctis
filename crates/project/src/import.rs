//! Importing a loose file into a project (the drag & drop operation).
//!
//! [`Project::import_file`] detects a file's kind by extension, copies it into the
//! kind's conventional directory, and returns the new asset path. It never
//! clobbers an existing file: a name collision gets a `-1`, `-2`, … suffix. An
//! unrecognised file is an error (the caller should ask the user what it is),
//! mirroring the drag & drop table in `docs/project.md`.

use std::path::{Path, PathBuf};

use crate::error::ProjectError;
use crate::path::{AssetPath, detect};
use crate::project::{Project, relative_asset_path};

impl Project {
    /// Copy `source` into the project, under the conventional directory for its
    /// kind, and return the new asset path.
    ///
    /// The file name is preserved; a collision gets a `-N` suffix rather than
    /// overwriting the existing file.
    pub fn import_file(&self, source: &Path) -> Result<AssetPath, ProjectError> {
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| ProjectError::Path(format!("{} has no file name", source.display())))?;
        let kind = detect(name);
        let Some(dir) = kind.default_dir() else {
            return Err(ProjectError::Path(format!(
                "cannot import {name:?}: unrecognised file type (expected a model, texture, \
                 audio, font, script, scene, or prefab)"
            )));
        };

        let target_dir = self.dir(dir);
        std::fs::create_dir_all(&target_dir).map_err(|e| ProjectError::Io(e.to_string()))?;
        let destination = unique_destination(&target_dir, name);
        std::fs::copy(source, &destination).map_err(|e| ProjectError::Io(e.to_string()))?;

        relative_asset_path(self.root(), &destination).ok_or_else(|| {
            ProjectError::Path(format!(
                "imported file {} is not under the project root",
                destination.display()
            ))
        })
    }
}

/// The destination inside `dir` for `name`, with a `-N` suffix when the name is
/// already taken (`wood.png` → `wood-1.png`, `wood-2.png`, …).
fn unique_destination(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, extension) = split_name(name);
    for counter in 1.. {
        let candidate = match extension {
            Some(extension) => dir.join(format!("{stem}-{counter}.{extension}")),
            None => dir.join(format!("{stem}-{counter}")),
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("an unbounded counter always finds a free name")
}

/// Split a file name into its stem and extension.
fn split_name(name: &str) -> (&str, Option<&str>) {
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, Some(extension)),
        _ => (name, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_name_handles_dots() {
        assert_eq!(split_name("wood.png"), ("wood", Some("png")));
        assert_eq!(split_name("model.glb"), ("model", Some("glb")));
        assert_eq!(split_name("noext"), ("noext", None));
        assert_eq!(split_name(".hidden"), (".hidden", None));
    }

    #[test]
    fn unique_destination_suffixes_on_collision() {
        let dir = std::env::temp_dir().join(format!(
            "noctis-unique-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();

        assert_eq!(unique_destination(&dir, "a.png"), dir.join("a.png"));
        std::fs::write(dir.join("a.png"), b"1").unwrap();
        assert_eq!(unique_destination(&dir, "a.png"), dir.join("a-1.png"));
        std::fs::write(dir.join("a-1.png"), b"2").unwrap();
        assert_eq!(unique_destination(&dir, "a.png"), dir.join("a-2.png"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
