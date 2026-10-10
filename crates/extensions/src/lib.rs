//! # extensions — the Noctis editor extension loader
//!
//! This crate is deliberately pure: it walks the filesystem and parses JSON and
//! TOML, but it never touches the editor or a script engine. The host (in
//! `crates/editor`) reads bytes here, then registers and evaluates them itself.
//! That keeps discovery, ordering, validation and the enablement config
//! unit-testable with no window and no Slag.
//!
//! An **extension** is a directory holding an `extension.json` (see
//! [`Manifest`]), optionally an `extension.js` and/or `extension.wasm`, and
//! declared assets. JavaScript never sees a path: a declared asset is read into
//! memory here and re-exposed under an opaque name (`ext:<id>:<slot>`).
//!
//! The whole design is `docs/extensions.md`.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod config;
mod manifest;

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::PathBuf;

use vfs::{DirStore, Store, StoreError, ZipStore, is_zip};

pub use config::{ExtensionConfig, config_dir, default_config_path};
pub use manifest::{Asset, Manifest, WasmModule};

/// The `noctis` API major this build understands. A manifest targeting another
/// major is refused rather than loaded halfway.
pub const EXTENSION_API: u32 = 1;

/// How many extensions are considered at all.
pub const MAX_EXTENSIONS: usize = 256;
/// A manifest is metadata; it has no business being large.
pub const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
/// An extension's entry source or compiled module.
pub const MAX_ENTRY_BYTES: u64 = 4 * 1024 * 1024;
/// A single declared asset.
pub const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;

/// Why an extension directory was refused or skipped. Collected rather than
/// fatal: one bad extension must never stop the others.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadError {
    /// The directory (or manifest) that failed.
    pub path: PathBuf,
    /// A human message.
    pub message: String,
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.path.display(), self.message)
    }
}

/// The discovered, validated, ordered extensions plus the errors from the ones
/// that were skipped.
#[derive(Debug, Default)]
pub struct Loader {
    extensions: Vec<Manifest>,
    errors: Vec<LoadError>,
}

impl Loader {
    /// An empty loader.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Discover every extension in `dirs` (in order), then order them.
    ///
    /// A directory without an `extension.json` is ignored; a directory with one
    /// that fails to load is recorded in [`Loader::errors`] and skipped. A
    /// duplicate `id` is a warning and the first discovered wins.
    pub fn discover(dirs: &[PathBuf]) -> Self {
        let mut extensions: Vec<Manifest> = Vec::new();
        let mut errors: Vec<LoadError> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut first_dir: HashMap<String, PathBuf> = HashMap::new();

        'dirs: for dir in dirs {
            let parent = DirStore::new(dir.clone());
            let entries = match parent.list("") {
                Ok(entries) => entries,
                Err(StoreError::NotFound(_)) => continue,
                Err(error) => {
                    errors.push(LoadError {
                        path: dir.clone(),
                        message: error.to_string(),
                    });
                    continue;
                }
            };

            for entry in entries {
                let origin = dir.join(&entry.name);
                let zip;
                let directory;
                let store: &dyn Store = if entry.is_dir {
                    if !parent.exists(&format!("{}/{}", entry.name, manifest::MANIFEST)) {
                        continue;
                    }
                    directory = DirStore::new(origin.clone());
                    &directory
                } else if is_zip(&entry.name) {
                    let bytes = match parent.read(&entry.name) {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            errors.push(LoadError {
                                path: origin,
                                message: error.to_string(),
                            });
                            continue;
                        }
                    };
                    match ZipStore::new(&bytes) {
                        Ok(parsed) if parsed.exists(manifest::MANIFEST) => {
                            zip = parsed;
                            &zip
                        }
                        Ok(_) => continue,
                        Err(error) => {
                            errors.push(LoadError {
                                path: origin,
                                message: error.to_string(),
                            });
                            continue;
                        }
                    }
                } else {
                    continue;
                };

                if extensions.len() >= MAX_EXTENSIONS {
                    errors.push(LoadError {
                        path: origin,
                        message: format!("over the {MAX_EXTENSIONS} extension limit"),
                    });
                    break 'dirs;
                }
                match manifest::load(store, origin.clone()) {
                    Ok(extension) => {
                        if seen.contains(&extension.id) {
                            let first = first_dir
                                .get(&extension.id)
                                .map(|p| p.display().to_string())
                                .unwrap_or_default();
                            errors.push(LoadError {
                                path: origin,
                                message: format!(
                                    "duplicate id {:?}; keeping the one in {first}",
                                    extension.id
                                ),
                            });
                        } else {
                            seen.insert(extension.id.clone());
                            first_dir.insert(extension.id.clone(), origin);
                            extensions.push(extension);
                        }
                    }
                    Err(message) => errors.push(LoadError {
                        path: origin,
                        message,
                    }),
                }
            }
        }

        let extensions = order(extensions, &mut errors);
        Self { extensions, errors }
    }

    /// The extensions, in load order.
    pub fn extensions(&self) -> &[Manifest] {
        &self.extensions
    }

    /// The extensions that were refused or skipped.
    pub fn errors(&self) -> &[LoadError] {
        &self.errors
    }

    /// The extension with `id`, if it loaded.
    pub fn get(&self, id: &str) -> Option<&Manifest> {
        self.extensions.iter().find(|extension| extension.id == id)
    }

    /// The loaded ids, in load order.
    pub fn ids(&self) -> Vec<String> {
        self.extensions
            .iter()
            .map(|extension| extension.id.clone())
            .collect()
    }

    /// The default search directories: the environment, beside the executable,
    /// the working directory, then the project's own directories.
    ///
    /// `--extensions` on the command line replaces this set.
    pub fn default_dirs(project_dirs: &[PathBuf]) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Some(from_env) = std::env::var_os("NOCTIS_EXTENSIONS")
            && !from_env.is_empty()
        {
            dirs.push(PathBuf::from(from_env));
        }
        if let Ok(exe) = std::env::current_exe()
            && let Some(parent) = exe.parent()
        {
            dirs.push(parent.join("extensions"));
        }
        if let Ok(cwd) = std::env::current_dir() {
            dirs.push(cwd.join("extensions"));
        }
        dirs.extend(project_dirs.iter().cloned());

        let mut seen = HashSet::new();
        dirs.retain(|dir| seen.insert(dir.clone()));
        dirs
    }
}

/// Order extensions: by `id`, then topologically by `loadAfter` (id as the
/// tiebreak). A cycle is recorded and the involved extensions keep `id` order.
fn order(mut extensions: Vec<Manifest>, errors: &mut Vec<LoadError>) -> Vec<Manifest> {
    extensions.sort_by(|a, b| a.id.cmp(&b.id));
    let count = extensions.len();
    let position: HashMap<&str, usize> = extensions
        .iter()
        .enumerate()
        .map(|(index, extension)| (extension.id.as_str(), index))
        .collect();

    let mut indegree = vec![0usize; count];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (index, extension) in extensions.iter().enumerate() {
        for after in &extension.load_after {
            if let Some(&dependency) = position.get(after.as_str())
                && dependency != index
            {
                indegree[index] += 1;
                dependents[dependency].push(index);
            }
        }
    }

    let mut ready: BinaryHeap<Reverse<usize>> = (0..count)
        .filter(|&index| indegree[index] == 0)
        .map(Reverse)
        .collect();
    let mut ordered: Vec<usize> = Vec::with_capacity(count);
    while let Some(Reverse(index)) = ready.pop() {
        ordered.push(index);
        for &dependent in &dependents[index] {
            indegree[dependent] -= 1;
            if indegree[dependent] == 0 {
                ready.push(Reverse(dependent));
            }
        }
    }

    if ordered.len() < count {
        let remaining: Vec<usize> = (0..count)
            .filter(|index| !ordered.contains(index))
            .collect();
        let names: Vec<&str> = remaining
            .iter()
            .map(|&index| extensions[index].id.as_str())
            .collect();
        errors.push(LoadError {
            path: extensions[remaining[0]].dir.clone(),
            message: format!("load-order cycle among: {}", names.join(", ")),
        });
        ordered.extend(remaining);
    }

    let mut slots: Vec<Option<Manifest>> = extensions.into_iter().map(Some).collect();
    ordered
        .into_iter()
        .filter_map(|index| slots[index].take())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("noctis-loader-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn write_extension(root: &Path, id: &str, extra: &str) {
        let dir = root.join(id.replace('.', "_"));
        std::fs::create_dir_all(&dir).expect("extension dir");
        std::fs::write(
            dir.join(manifest::MANIFEST),
            format!(r#"{{ "id": "{id}", "name": "{id}", "version": "1.0.0", "api": 1{extra} }}"#),
        )
        .expect("manifest");
    }

    #[test]
    fn discovery_finds_directories_with_a_manifest() {
        let root = temp_dir("discover");
        write_extension(&root, "com.example.b", "");
        write_extension(&root, "com.example.a", "");
        std::fs::create_dir_all(root.join("not-an-extension")).unwrap();
        std::fs::write(root.join("loose.txt"), "ignored").unwrap();

        let loader = Loader::discover(std::slice::from_ref(&root));
        assert!(loader.errors().is_empty(), "{:?}", loader.errors());
        assert_eq!(loader.ids(), ["com.example.a", "com.example.b"]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_broken_extension_is_named_and_skipped() {
        let root = temp_dir("broken");
        write_extension(&root, "com.example.good", "");
        let bad = root.join("bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join(manifest::MANIFEST), "{ not json").unwrap();

        let loader = Loader::discover(std::slice::from_ref(&root));
        assert_eq!(loader.ids(), ["com.example.good"]);
        assert_eq!(loader.errors().len(), 1);
        assert_eq!(loader.errors()[0].path, bad);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn load_after_orders_before_the_id_sort() {
        let root = temp_dir("loadafter");
        // "a" must load after "z", which the plain id sort would not do.
        write_extension(
            &root,
            "com.example.a",
            r#", "loadAfter": ["com.example.z"]"#,
        );
        write_extension(&root, "com.example.z", "");
        let loader = Loader::discover(std::slice::from_ref(&root));
        assert_eq!(loader.ids(), ["com.example.z", "com.example.a"]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_cycle_is_reported_and_stays_in_id_order() {
        let root = temp_dir("cycle");
        write_extension(
            &root,
            "com.example.a",
            r#", "loadAfter": ["com.example.b"]"#,
        );
        write_extension(
            &root,
            "com.example.b",
            r#", "loadAfter": ["com.example.a"]"#,
        );
        let loader = Loader::discover(std::slice::from_ref(&root));
        assert_eq!(loader.ids(), ["com.example.a", "com.example.b"]);
        assert!(
            loader
                .errors()
                .iter()
                .any(|error| error.message.contains("cycle")),
            "{:?}",
            loader.errors()
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_duplicate_id_keeps_the_first() {
        let root = temp_dir("duplicate");
        write_extension(&root, "com.example.a", "");
        let again = root.join("zzz");
        std::fs::create_dir_all(&again).unwrap();
        std::fs::write(
            again.join(manifest::MANIFEST),
            r#"{ "id": "com.example.a", "name": "again", "version": "1", "api": 1 }"#,
        )
        .unwrap();

        let loader = Loader::discover(std::slice::from_ref(&root));
        assert_eq!(loader.ids(), ["com.example.a"]);
        assert!(loader.errors()[0].message.contains("duplicate"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_directory_is_not_an_error() {
        let loader = Loader::discover(&[PathBuf::from("this/does/not/exist")]);
        assert!(loader.extensions().is_empty());
        assert!(loader.errors().is_empty());
    }

    #[test]
    fn a_zip_archive_is_discovered_and_read() {
        use std::io::Write;

        let root = temp_dir("zip");
        let path = root.join("packed.zip");
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("extension.json", options).unwrap();
        writer
            .write_all(
                br#"{ "id": "com.example.zip", "name": "Zip", "version": "1.0.0", "api": 1, "entry": "extension.js" }"#,
            )
            .unwrap();
        writer.start_file("extension.js", options).unwrap();
        writer
            .write_all(b"noctis.command('zip', function () { return 'ok'; });")
            .unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        std::fs::write(&path, bytes).unwrap();

        let loader = Loader::discover(std::slice::from_ref(&root));
        assert!(loader.errors().is_empty(), "{:?}", loader.errors());
        assert_eq!(loader.ids(), ["com.example.zip"]);
        let manifest = loader.get("com.example.zip").expect("loaded");
        assert_eq!(manifest.dir, path);
        assert!(
            manifest
                .entry_source
                .as_deref()
                .is_some_and(|source| source.contains("noctis.command"))
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_corrupt_zip_is_reported_and_skipped() {
        let root = temp_dir("badzip");
        std::fs::write(root.join("broken.zip"), b"not a zip at all").unwrap();
        let loader = Loader::discover(std::slice::from_ref(&root));
        assert!(loader.extensions().is_empty());
        assert_eq!(loader.errors().len(), 1);
        std::fs::remove_dir_all(&root).ok();
    }
}
