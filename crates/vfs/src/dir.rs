//! A native directory backend (`std::fs`).

use std::path::{Path, PathBuf};

use crate::{Entry, Store, StoreError, validate_relative};

/// A [`Store`] over a native directory. All paths resolve inside `root`.
#[derive(Debug)]
pub struct DirStore {
    root: PathBuf,
}

impl DirStore {
    /// A store rooted at `root` (which need not exist yet).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The directory the store is rooted at.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The store view of a single, path-addressed file: a `DirStore` rooted at
    /// `path`'s parent, plus `path`'s file name as the key.
    ///
    /// Handy for reading or writing one document (a scene, a controls file)
    /// through the [`Store`] API without opening a whole project. A bare file
    /// name (no parent) is rooted at the current directory; a path with no file
    /// name yields an empty key.
    pub fn for_file(path: &Path) -> (DirStore, String) {
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
            .to_string();
        (DirStore::new(parent), name)
    }

    /// Resolve a relative path to a native path inside the root.
    fn resolve(&self, path: &str) -> Result<PathBuf, StoreError> {
        let relative = validate_relative(path)?;
        Ok(if relative.is_empty() {
            self.root.clone()
        } else {
            self.root.join(&relative)
        })
    }
}

impl Store for DirStore {
    fn read(&self, path: &str) -> Result<Vec<u8>, StoreError> {
        std::fs::read(self.resolve(path)?).map_err(|error| io_error(path, error))
    }

    fn read_capped(&self, path: &str, max: u64) -> Result<Vec<u8>, StoreError> {
        let full = self.resolve(path)?;
        let metadata = std::fs::metadata(&full).map_err(|error| io_error(path, error))?;
        let len = metadata.len();
        if len > max {
            return Err(StoreError::TooLarge {
                path: path.to_string(),
                len,
                max,
            });
        }
        std::fs::read(&full).map_err(|error| io_error(path, error))
    }

    fn exists(&self, path: &str) -> bool {
        self.resolve(path)
            .map(|full| full.is_file())
            .unwrap_or(false)
    }

    fn is_dir(&self, path: &str) -> bool {
        self.resolve(path)
            .map(|full| full.is_dir())
            .unwrap_or(false)
    }

    fn list(&self, dir: &str) -> Result<Vec<Entry>, StoreError> {
        let full = self.resolve(dir)?;
        let entries = std::fs::read_dir(&full).map_err(|error| io_error(dir, error))?;
        let mut out = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| StoreError::Io(error.to_string()))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            out.push(Entry { name, is_dir });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), StoreError> {
        let full = self.resolve(path)?;
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).map_err(|error| StoreError::Io(error.to_string()))?;
        }
        std::fs::write(&full, bytes).map_err(|error| io_error(path, error))
    }

    fn remove(&mut self, path: &str) -> Result<(), StoreError> {
        let full = self.resolve(path)?;
        let metadata = std::fs::metadata(&full).map_err(|error| io_error(path, error))?;
        let result = if metadata.is_dir() {
            std::fs::remove_dir_all(&full)
        } else {
            std::fs::remove_file(&full)
        };
        result.map_err(|error| io_error(path, error))
    }

    fn copy(&mut self, from: &str, to: &str) -> Result<(), StoreError> {
        let source = self.resolve(from)?;
        let destination = self.resolve(to)?;
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|error| StoreError::Io(error.to_string()))?;
        }
        std::fs::copy(&source, &destination)
            .map(|_| ())
            .map_err(|error| io_error(from, error))
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), StoreError> {
        let source = self.resolve(from)?;
        let destination = self.resolve(to)?;
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|error| StoreError::Io(error.to_string()))?;
        }
        std::fs::rename(&source, &destination).map_err(|error| io_error(from, error))
    }

    fn duplicate(&self) -> Box<dyn Store> {
        Box::new(DirStore::new(self.root.clone()))
    }
}

/// Map an I/O error, distinguishing a missing path.
fn io_error(path: &str, error: std::io::Error) -> StoreError {
    if error.kind() == std::io::ErrorKind::NotFound {
        StoreError::NotFound(path.to_string())
    } else {
        StoreError::Io(format!("{path}: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("noctis-vfs-dir-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn read_write_list_and_remove() {
        let dir = temp_dir("rw");
        let mut store = DirStore::new(dir.clone());

        store.write("a/b.txt", b"hi").unwrap();
        assert!(store.exists("a/b.txt"));
        assert!(!store.exists("a"));
        assert!(store.is_dir("a"));
        assert!(store.is_dir(""));
        assert_eq!(store.read("a/b.txt").unwrap(), b"hi");
        assert_eq!(store.read_string("a/b.txt").unwrap(), "hi");
        assert_eq!(store.read("a/./b.txt").unwrap(), b"hi");

        let root = store.list("").unwrap();
        assert_eq!(
            root,
            [Entry {
                name: "a".into(),
                is_dir: true
            }]
        );
        let a = store.list("a").unwrap();
        assert_eq!(
            a,
            [Entry {
                name: "b.txt".into(),
                is_dir: false
            }]
        );

        assert_eq!(
            store.list("missing").unwrap_err(),
            StoreError::NotFound("missing".into())
        );
        assert_eq!(
            store.read("missing").unwrap_err(),
            StoreError::NotFound("missing".into())
        );

        store.remove("a/b.txt").unwrap();
        assert!(!store.exists("a/b.txt"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_cap_is_checked_before_reading() {
        let dir = temp_dir("cap");
        let mut store = DirStore::new(dir.clone());
        store.write("big.bin", &[0u8; 8]).unwrap();
        assert!(matches!(
            store.read_capped("big.bin", 4),
            Err(StoreError::TooLarge { len: 8, max: 4, .. })
        ));
        assert_eq!(store.read_capped("big.bin", 8).unwrap().len(), 8);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rename_and_copy_move_bytes() {
        let dir = temp_dir("move");
        let mut store = DirStore::new(dir.clone());
        store.write("a.txt", b"one").unwrap();
        store.rename("a.txt", "sub/b.txt").unwrap();
        assert!(!store.exists("a.txt"));
        assert_eq!(store.read("sub/b.txt").unwrap(), b"one");
        store.copy("sub/b.txt", "c.txt").unwrap();
        assert_eq!(store.read("c.txt").unwrap(), b"one");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_escape_is_refused() {
        let store = DirStore::new(temp_dir("escape"));
        assert!(matches!(store.read("../x"), Err(StoreError::Invalid(_))));
        assert!(!store.exists("../x"));
    }

    #[test]
    fn for_file_views_the_parent_and_name() {
        let dir = temp_dir("for-file");
        let (store, key) = DirStore::for_file(&dir.join("scenes/level.scene.json"));
        assert_eq!(key, "level.scene.json");
        // The store's root is the file's parent, so the key resolves back.
        assert_eq!(store.root(), dir.join("scenes"));

        // A bare file name is rooted at the current directory and keeps its name.
        let (store, key) = DirStore::for_file(Path::new("noctis.toml"));
        assert_eq!(key, "noctis.toml");
        assert_eq!(store.root(), Path::new("."));
    }

    #[test]
    fn duplicate_views_the_same_directory() {
        let dir = temp_dir("duplicate");
        let mut store = DirStore::new(dir.clone());
        store.write("a.txt", b"hi").unwrap();

        let mut copy = store.duplicate();
        assert_eq!(copy.read("a.txt").unwrap(), b"hi");
        copy.write("b.txt", b"yo").unwrap();
        // A directory clone sees the same files through the same root.
        assert!(store.exists("b.txt"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
