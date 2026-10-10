//! A `.zip` archive backend, read into memory (read-only).

use std::collections::BTreeMap;
use std::io::Read;

use crate::{
    Entry, MAX_ARCHIVE_BYTES, MAX_ARCHIVE_FILES, MAX_FILE_BYTES, Store, StoreError, children,
    has_dir, validate_relative,
};

/// A read-only [`Store`] over a `.zip` archive read fully into memory.
///
/// This is the reader an editor-extension archive and a project bundle share. It
/// is bounded (per-file and total size, and a file count) so a decompression bomb
/// cannot exhaust memory, and it drops entries that are not plain relative paths.
#[derive(Debug, Clone)]
pub struct ZipStore {
    files: BTreeMap<String, Vec<u8>>,
}

impl ZipStore {
    /// Read a whole archive.
    pub fn new(bytes: &[u8]) -> Result<Self, StoreError> {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
            .map_err(|error| StoreError::Archive(error.to_string()))?;
        if archive.len() > MAX_ARCHIVE_FILES {
            return Err(StoreError::Archive(format!(
                "holds {} entries, over the {MAX_ARCHIVE_FILES} file limit",
                archive.len()
            )));
        }

        let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut total: u64 = 0;
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|error| StoreError::Archive(error.to_string()))?;
            if entry.is_dir() {
                continue;
            }
            let name = entry
                .name()
                .map_err(|error| StoreError::Archive(error.to_string()))?
                .to_string();
            // An entry that is not a plain relative path is ignored, not fatal.
            let Ok(key) = validate_relative(&name) else {
                continue;
            };
            if key.is_empty() {
                continue;
            }
            let mut buffer = Vec::new();
            (&mut entry)
                .take(MAX_FILE_BYTES + 1)
                .read_to_end(&mut buffer)
                .map_err(|error| StoreError::Archive(format!("{name}: {error}")))?;
            if buffer.len() as u64 > MAX_FILE_BYTES {
                return Err(StoreError::Archive(format!(
                    "{name} is over the {MAX_FILE_BYTES} byte per-file cap"
                )));
            }
            total += buffer.len() as u64;
            if total > MAX_ARCHIVE_BYTES {
                return Err(StoreError::Archive(format!(
                    "expands to over {MAX_ARCHIVE_BYTES} bytes"
                )));
            }
            files.insert(key, buffer);
        }
        Ok(Self { files })
    }
}

impl Store for ZipStore {
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

    fn write(&mut self, path: &str, _bytes: &[u8]) -> Result<(), StoreError> {
        Err(StoreError::ReadOnly(path.to_string()))
    }

    fn remove(&mut self, path: &str) -> Result<(), StoreError> {
        Err(StoreError::ReadOnly(path.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, bytes) in files {
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn reads_entries_and_lists_directories() {
        let bytes = archive(&[("extension.json", b"{}"), ("assets/grid.bin", b"payload")]);
        let store = ZipStore::new(&bytes).expect("a valid archive");

        assert_eq!(store.read("extension.json").unwrap(), b"{}");
        assert_eq!(store.read("assets/grid.bin").unwrap(), b"payload");
        assert!(store.exists("extension.json"));
        assert!(!store.exists("missing"));
        assert!(store.is_dir("assets"));
        assert!(store.is_dir(""));
        assert_eq!(
            store.list("").unwrap(),
            [
                Entry {
                    name: "assets".into(),
                    is_dir: true
                },
                Entry {
                    name: "extension.json".into(),
                    is_dir: false
                },
            ]
        );
    }

    #[test]
    fn escaping_entries_are_dropped() {
        let bytes = archive(&[("../escape.txt", b"nope"), ("ok.txt", b"yes")]);
        let store = ZipStore::new(&bytes).expect("a valid archive");
        assert!(store.exists("ok.txt"));
        assert!(!store.exists("../escape.txt"));
    }

    #[test]
    fn a_non_archive_is_refused() {
        assert!(matches!(
            ZipStore::new(b"not a zip"),
            Err(StoreError::Archive(_))
        ));
    }

    #[test]
    fn writing_is_read_only() {
        let bytes = archive(&[("a.txt", b"a")]);
        let mut store = ZipStore::new(&bytes).unwrap();
        assert!(matches!(
            store.write("b.txt", b"b"),
            Err(StoreError::ReadOnly(_))
        ));
        assert!(matches!(
            store.remove("a.txt"),
            Err(StoreError::ReadOnly(_))
        ));
    }
}
