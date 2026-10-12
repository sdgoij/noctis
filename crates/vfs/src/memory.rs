//! An in-memory backend, for tests and composition.

use std::collections::BTreeMap;

use crate::{Entry, Store, StoreError, children, has_dir, validate_relative};

/// A [`Store`] backed by a map of `/`-joined paths to bytes.
#[derive(Debug, Default, Clone)]
pub struct MemoryStore {
    files: BTreeMap<String, Vec<u8>>,
}

impl MemoryStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// A store holding one file (a convenience for tests).
    pub fn with_file(path: &str, bytes: impl Into<Vec<u8>>) -> Self {
        let mut store = Self::new();
        store
            .write(path, &bytes.into())
            .expect("a valid relative path");
        store
    }

    /// Every file, for a backend that persists the whole store — the browser
    /// backend writes this working copy back to OPFS.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn files(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.files
    }
}

impl Store for MemoryStore {
    fn read(&self, path: &str) -> Result<Vec<u8>, StoreError> {
        let key = validate_relative(path)?;
        self.files
            .get(&key)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(path.to_string()))
    }

    fn exists(&self, path: &str) -> bool {
        validate_relative(path).is_ok_and(|key| self.files.contains_key(&key))
    }

    fn is_dir(&self, path: &str) -> bool {
        match validate_relative(path) {
            Ok(key) if key.is_empty() => true,
            Ok(key) => has_dir(&self.files, &key),
            Err(_) => false,
        }
    }

    fn list(&self, dir: &str) -> Result<Vec<Entry>, StoreError> {
        children(&self.files, dir)
    }

    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), StoreError> {
        let key = validate_relative(path)?;
        if key.is_empty() {
            return Err(StoreError::Invalid("a file path must not be empty".into()));
        }
        self.files.insert(key, bytes.to_vec());
        Ok(())
    }

    fn remove(&mut self, path: &str) -> Result<(), StoreError> {
        let key = validate_relative(path)?;
        if self.files.remove(&key).is_some() {
            return Ok(());
        }
        // A directory: drop everything beneath it.
        if has_dir(&self.files, &key) {
            let prefix = format!("{key}/");
            self.files.retain(|path, _| !path.starts_with(&prefix));
            return Ok(());
        }
        Err(StoreError::NotFound(path.to_string()))
    }

    fn duplicate(&self) -> Box<dyn Store> {
        Box::new(MemoryStore::clone(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_write_list_and_remove() {
        let mut store = MemoryStore::new();
        store.write("a/b.txt", b"hi").unwrap();
        store.write("a/c.txt", b"yo").unwrap();
        store.write("top.txt", b"t").unwrap();

        assert_eq!(store.read("a/b.txt").unwrap(), b"hi");
        assert!(store.exists("top.txt"));
        assert!(!store.exists("nope"));
        assert!(store.is_dir("a"));
        assert!(store.is_dir(""));
        assert!(!store.is_dir("top.txt"));

        assert_eq!(
            store.list("").unwrap(),
            [
                Entry {
                    name: "a".into(),
                    is_dir: true
                },
                Entry {
                    name: "top.txt".into(),
                    is_dir: false
                },
            ]
        );
        assert_eq!(
            store.list("a").unwrap(),
            [
                Entry {
                    name: "b.txt".into(),
                    is_dir: false
                },
                Entry {
                    name: "c.txt".into(),
                    is_dir: false
                },
            ]
        );
        assert_eq!(
            store.list("missing").unwrap_err(),
            StoreError::NotFound("missing".into())
        );

        store.remove("a").unwrap();
        assert!(!store.exists("a/b.txt"));
        assert!(!store.is_dir("a"));
    }

    #[test]
    fn paths_that_escape_are_refused() {
        let mut store = MemoryStore::new();
        assert!(matches!(
            store.write("../x", b"x"),
            Err(StoreError::Invalid(_))
        ));
        assert!(matches!(
            store.read("/etc/passwd"),
            Err(StoreError::Invalid(_))
        ));
        assert!(!store.exists("../x"));
    }
}
