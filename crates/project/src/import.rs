//! Importing a loose file into a project (the drag & drop operation).
//!
//! [`Project::import_file`] detects a file's kind by extension, copies it into the
//! kind's conventional directory, and returns the new asset path. It never
//! clobbers an existing file: a name collision gets a `-1`, `-2`, … suffix. An
//! unrecognised file is an error (the caller should ask the user what it is),
//! mirroring the drag & drop table in `docs/project.md`.

use std::path::Path;

use vfs::Store;

use crate::error::ProjectError;
use crate::path::{AssetPath, detect};
use crate::project::Project;

impl Project {
    /// Copy `source` into the project, under the conventional directory for its
    /// kind, and return the new asset path.
    ///
    /// The file name is preserved; a collision gets a `-N` suffix rather than
    /// overwriting the existing file.
    pub fn import_file(&mut self, source: &Path) -> Result<AssetPath, ProjectError> {
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

        // The source is an arbitrary path outside the project, so it is read
        // directly; the copy into the project goes through the store.
        let bytes = std::fs::read(source).map_err(|e| ProjectError::Io(e.to_string()))?;
        let store = self.store_mut();
        let file_name = unique_name(store, dir, name);
        let relative = format!("{dir}/{file_name}");
        store
            .write(&relative, &bytes)
            .map_err(|e| ProjectError::Io(e.to_string()))?;
        AssetPath::parse(relative)
    }
}

/// The name inside `dir` for `name`, with a `-N` suffix when the name is already
/// taken (`wood.png` → `wood-1.png`, `wood-2.png`, …).
fn unique_name(store: &dyn Store, dir: &str, name: &str) -> String {
    if !store.exists(&format!("{dir}/{name}")) {
        return name.to_string();
    }
    let (stem, extension) = split_name(name);
    for counter in 1.. {
        let candidate = match extension {
            Some(extension) => format!("{stem}-{counter}.{extension}"),
            None => format!("{stem}-{counter}"),
        };
        if !store.exists(&format!("{dir}/{candidate}")) {
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
    use vfs::DirStore;

    #[test]
    fn split_name_handles_dots() {
        assert_eq!(split_name("wood.png"), ("wood", Some("png")));
        assert_eq!(split_name("model.glb"), ("model", Some("glb")));
        assert_eq!(split_name("noext"), ("noext", None));
        assert_eq!(split_name(".hidden"), (".hidden", None));
    }

    #[test]
    fn unique_name_suffixes_on_collision() {
        let dir = std::env::temp_dir().join(format!(
            "noctis-unique-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = DirStore::new(&dir);

        assert_eq!(unique_name(&store, "assets", "a.png"), "a.png");
        store.write("assets/a.png", b"1").unwrap();
        assert_eq!(unique_name(&store, "assets", "a.png"), "a-1.png");
        store.write("assets/a-1.png", b"2").unwrap();
        assert_eq!(unique_name(&store, "assets", "a.png"), "a-2.png");

        std::fs::remove_dir_all(&dir).ok();
    }
}
