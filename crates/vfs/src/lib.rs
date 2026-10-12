//! # vfs — a filesystem-store abstraction
//!
//! The engine reads and writes files (a project's manifest, assets, scenes,
//! prefabs; an extension's entry, module and assets) through one trait, [`Store`],
//! rather than calling `std::fs` directly. That is what lets the same code run
//! against a native directory, an in-memory `.zip`, or (on the web) browser
//! storage — see [`.notes/web.md`](../../.notes/web.md).
//!
//! Backends:
//!
//! - [`DirStore`] — a native directory (`std::fs`).
//! - [`ZipStore`] — a `.zip` archive read into memory (read-only; the same reader
//!   an extension archive and a project bundle use).
//! - [`MemoryStore`] — an in-memory map, for tests and composition.
//! - `OpfsStore` — the browser's Origin Private File System (`wasm32` only).
//!
//! Every path is **relative** and `/`-separated; [`validate_relative`] is the one
//! rule (no absolute paths, no `..`) that keeps a store inside its root. That rule
//! used to live in `crates/extensions`; it moved here so every backend shares it.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

mod dir;
mod memory;
#[cfg(target_arch = "wasm32")]
pub mod opfs;
mod zip;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path};

pub use dir::DirStore;
pub use memory::MemoryStore;
#[cfg(target_arch = "wasm32")]
pub use opfs::OpfsStore;
pub use zip::{ZipStore, zip_store};

/// The largest single file a store will read out of an archive.
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// The largest total decompressed size of an archive.
pub const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
/// The most entries an archive may hold.
pub const MAX_ARCHIVE_FILES: usize = 1024;

/// Why a store operation failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// The path does not exist.
    NotFound(String),
    /// The path is not a valid relative path within the store.
    Invalid(String),
    /// The store does not support writing (`ZipStore`).
    ReadOnly(String),
    /// A file is larger than the caller allows.
    TooLarge {
        /// The path that was too large.
        path: String,
        /// Its size in bytes.
        len: u64,
        /// The cap it exceeded.
        max: u64,
    },
    /// An archive could not be read.
    Archive(String),
    /// An underlying I/O error.
    Io(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::NotFound(path) => write!(formatter, "{path}: not found"),
            StoreError::Invalid(message) => write!(formatter, "invalid path: {message}"),
            StoreError::ReadOnly(path) => write!(formatter, "{path}: the store is read-only"),
            StoreError::TooLarge { path, len, max } => {
                write!(formatter, "{path} is {len} bytes, over the {max} byte cap")
            }
            StoreError::Archive(message) => write!(formatter, "archive: {message}"),
            StoreError::Io(message) => write!(formatter, "{message}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// One immediate child of a directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The child's name (no directory part).
    pub name: String,
    /// Whether the child is a directory.
    pub is_dir: bool,
}

/// A byte- and path-oriented filesystem view.
///
/// Paths are relative, `/`-separated keys. `""` names the root. Reads and writes
/// are whole-file (the engine's files are small: JSON, sources, textures, audio);
/// a caller that needs a byte cap uses [`Store::read_capped`].
pub trait Store: fmt::Debug {
    /// Read the whole file at `path`.
    fn read(&self, path: &str) -> Result<Vec<u8>, StoreError>;

    /// Read `path`, refusing a file over `max` bytes.
    ///
    /// The default reads first and then checks; a backend that can see the size
    /// up front (a directory) overrides this to refuse before allocating.
    fn read_capped(&self, path: &str, max: u64) -> Result<Vec<u8>, StoreError> {
        let bytes = self.read(path)?;
        let len = bytes.len() as u64;
        if len > max {
            return Err(StoreError::TooLarge {
                path: path.to_string(),
                len,
                max,
            });
        }
        Ok(bytes)
    }

    /// Read `path` as UTF-8 text.
    fn read_string(&self, path: &str) -> Result<String, StoreError> {
        let bytes = self.read(path)?;
        String::from_utf8(bytes)
            .map_err(|error| StoreError::Invalid(format!("{path} is not UTF-8: {error}")))
    }

    /// Whether `path` is a readable file.
    fn exists(&self, path: &str) -> bool;

    /// Whether `path` is a directory (the root `""` is a directory).
    fn is_dir(&self, path: &str) -> bool;

    /// The immediate children of directory `dir`, sorted by name.
    ///
    /// A `dir` that does not exist is [`StoreError::NotFound`].
    fn list(&self, dir: &str) -> Result<Vec<Entry>, StoreError>;

    /// Write `bytes` to `path`, creating parent directories as needed.
    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), StoreError>;

    /// Remove the file (or directory) at `path`.
    fn remove(&mut self, path: &str) -> Result<(), StoreError>;

    /// Copy `from` to `to`. The default reads and rewrites.
    fn copy(&mut self, from: &str, to: &str) -> Result<(), StoreError> {
        let bytes = self.read(from)?;
        self.write(to, &bytes)
    }

    /// Move `from` to `to`. The default copies and then removes.
    fn rename(&mut self, from: &str, to: &str) -> Result<(), StoreError> {
        let bytes = self.read(from)?;
        self.write(to, &bytes)?;
        self.remove(from)
    }

    /// A boxed clone of this store.
    ///
    /// A `dyn Store` cannot derive `Clone`, so a type that owns one (a
    /// [`Store`]-backed project) clones through this. A backend whose state is a
    /// handle to shared data (a directory, a browser origin) returns a second
    /// handle to the same data; a purely in-memory backend copies its files.
    fn duplicate(&self) -> Box<dyn Store>;
}

/// Validate and normalize a relative path to a `/`-joined key.
///
/// Every component must be a plain name; an absolute path or a `..` is refused.
/// `.` components and repeated separators are folded away, `\` is folded to `/`
/// (on every platform, not only Windows), and `""` (the root) is allowed.
pub fn validate_relative(path: &str) -> Result<String, StoreError> {
    // Keys are `/`-separated, so a `\` is always a separator — normalization
    // must not depend on the host platform (`Path::components` only folds `\`
    // on Windows). This also makes a `\`-rooted path absolute on Unix, so the
    // escape check below catches it there too.
    let text: std::borrow::Cow<'_, str> = if path.contains('\\') {
        std::borrow::Cow::Owned(path.replace('\\', "/"))
    } else {
        std::borrow::Cow::Borrowed(path)
    };
    let candidate = Path::new(text.as_ref());
    if candidate.is_absolute() {
        return Err(StoreError::Invalid(format!(
            "{path:?} must be a relative path"
        )));
    }
    let mut parts: Vec<&str> = Vec::new();
    for component in candidate.components() {
        match component {
            Component::Normal(part) => {
                let part = part
                    .to_str()
                    .ok_or_else(|| StoreError::Invalid(format!("{path:?} is not valid UTF-8")))?;
                parts.push(part);
            }
            // A lone `.` or a doubled separator is harmless; anything else
            // (`..`, a root, a Windows prefix) escapes and is refused.
            Component::CurDir => {}
            _ => {
                return Err(StoreError::Invalid(format!(
                    "{path:?} escapes the store root"
                )));
            }
        }
    }
    Ok(parts.join("/"))
}

/// Whether `name` looks like a `.zip` archive (case-insensitive).
pub fn is_zip(name: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
}

/// Whether any key in a flat file map lives under directory `key`.
pub(crate) fn has_dir(files: &BTreeMap<String, Vec<u8>>, key: &str) -> bool {
    let prefix = format!("{key}/");
    files.keys().any(|path| path.starts_with(&prefix))
}

/// The immediate children of `dir` in a flat file map (the archive/memory model).
pub(crate) fn children(
    files: &BTreeMap<String, Vec<u8>>,
    dir: &str,
) -> Result<Vec<Entry>, StoreError> {
    let key = validate_relative(dir)?;
    let prefix = if key.is_empty() {
        String::new()
    } else {
        format!("{key}/")
    };
    let mut out: BTreeMap<String, bool> = BTreeMap::new();
    for path in files.keys() {
        let Some(rest) = path.strip_prefix(&prefix) else {
            continue;
        };
        if rest.is_empty() {
            continue;
        }
        match rest.split_once('/') {
            Some((name, _)) => {
                out.insert(name.to_string(), true);
            }
            None => {
                out.insert(rest.to_string(), false);
            }
        }
    }
    if out.is_empty() && !key.is_empty() && !has_dir(files, &key) {
        return Err(StoreError::NotFound(dir.to_string()));
    }
    Ok(out
        .into_iter()
        .map(|(name, is_dir)| Entry { name, is_dir })
        .collect())
}

/// Every file in `store`, as a `/`-joined path to bytes — a recursive walk.
///
/// The counterpart of [`MemoryStore::from_files`]: together they move a whole
/// store between backends (a bundle into browser storage, a project into a
/// `.zip`).
pub fn snapshot(store: &dyn Store) -> Result<BTreeMap<String, Vec<u8>>, StoreError> {
    let mut files = BTreeMap::new();
    walk(store, "", &mut files)?;
    Ok(files)
}

/// Collect `dir` and its subdirectories from `store` into `files`.
fn walk(
    store: &dyn Store,
    dir: &str,
    files: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), StoreError> {
    for entry in store.list(dir)? {
        let path = if dir.is_empty() {
            entry.name.clone()
        } else {
            format!("{dir}/{}", entry.name)
        };
        if entry.is_dir {
            walk(store, &path, files)?;
        } else {
            files.insert(path.clone(), store.read(&path)?);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_relative_path_is_normalized() {
        assert_eq!(validate_relative("extension.js").unwrap(), "extension.js");
        assert_eq!(
            validate_relative("assets/grid.bin").unwrap(),
            "assets/grid.bin"
        );
        assert_eq!(
            validate_relative("assets\\grid.bin").unwrap(),
            "assets/grid.bin"
        );
        assert_eq!(validate_relative("").unwrap(), "");
        assert_eq!(validate_relative("a/./b").unwrap(), "a/b");
    }

    #[test]
    fn paths_that_escape_are_refused() {
        assert!(validate_relative("../evil.js").is_err());
        assert!(validate_relative("a/../../b").is_err());
        assert!(validate_relative("/etc/passwd").is_err());
    }

    #[test]
    fn zip_names_are_recognized() {
        assert!(is_zip("packed.zip"));
        assert!(is_zip("PACKED.ZIP"));
        assert!(!is_zip("packed.zip.bak"));
        assert!(!is_zip("extension.js"));
        assert!(!is_zip("noext"));
    }

    #[test]
    fn zip_store_round_trips_and_excludes() {
        let mut store = MemoryStore::new();
        store
            .write("noctis.toml", b"[project]\nname = \"P\"\n")
            .unwrap();
        store.write("scenes/a.scene.json", b"{}").unwrap();
        store.write("assets/models/deep/x.glb", b"bytes").unwrap();
        store.write(".noctis/textures/dead.bin", b"junk").unwrap();
        store.write("dist/game.exe", b"junk").unwrap();

        let bytes = zip_store(&store, &[".noctis", "dist"]).unwrap();
        let read = ZipStore::new(&bytes).expect("the archive reads back");
        assert!(read.exists("noctis.toml"));
        assert!(read.exists("scenes/a.scene.json"));
        assert_eq!(read.read("assets/models/deep/x.glb").unwrap(), b"bytes");
        assert!(!read.exists(".noctis/textures/dead.bin"));
        assert!(!read.exists("dist/game.exe"));
    }

    #[test]
    fn a_store_snapshots_and_rebuilds() {
        let mut store = MemoryStore::new();
        store.write("noctis.toml", b"x").unwrap();
        store.write("scenes/a.scene.json", b"y").unwrap();
        store.write("assets/models/x.glb", b"z").unwrap();

        // `snapshot` walks the whole tree; `from_files` rebuilds a store from it
        // (how a bundle becomes browser storage, and back).
        let files = snapshot(&store).unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files["assets/models/x.glb"], b"z");

        let rebuilt = MemoryStore::from_files(files);
        assert_eq!(rebuilt.read_string("scenes/a.scene.json").unwrap(), "y");
        assert!(rebuilt.is_dir("assets/models"));
    }
}
