//! Where an extension's files come from.
//!
//! An extension is either a directory (the common form, editable in place) or a
//! `.zip` archive with `extension.json` at its root. Both are read through
//! [`Source`], so the manifest loader never touches `std::fs` directly and an
//! archive is held to the same rules (relative paths only, byte caps) as a
//! directory.
//!
//! A zip is read fully into memory when it is opened, with a per-entry and a
//! total cap so a decompression bomb cannot exhaust memory. The manifest loader
//! already reads an extension's entry, module and assets into memory, so the
//! archive's bytes are not retained once loading is done.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use crate::{MAX_ASSET_BYTES, MAX_EXTENSION_BYTES, MAX_FILES_PER_EXTENSION};

/// One extension's files: a directory on disk, or a zip read into memory.
#[derive(Debug)]
pub(crate) enum Source {
    /// A directory holding `extension.json` and its files.
    Dir(PathBuf),
    /// A zip archive, with its files keyed by a `/`-joined relative path.
    Zip {
        /// The archive's path, for error messages.
        path: PathBuf,
        /// The archive's files, read into memory.
        files: BTreeMap<String, Vec<u8>>,
    },
}

impl Source {
    /// A directory source (the path is not checked here).
    pub(crate) fn dir(path: PathBuf) -> Self {
        Source::Dir(path)
    }

    /// Open a `.zip` archive and read its files into memory.
    pub(crate) fn zip(path: PathBuf) -> Result<Self, String> {
        let file =
            std::fs::File::open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|error| format!("{} is not a zip archive: {error}", path.display()))?;
        if archive.len() > MAX_FILES_PER_EXTENSION {
            return Err(format!(
                "{} holds {} entries, over the {MAX_FILES_PER_EXTENSION} file limit",
                path.display(),
                archive.len()
            ));
        }

        let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut total: u64 = 0;
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            if entry.is_dir() {
                continue;
            }
            let name = entry
                .name()
                .map_err(|error| format!("{}: {error}", path.display()))?
                .to_string();
            // An entry that is not a plain relative path is ignored, not fatal:
            // the manifest simply will not find it.
            let Ok(key) = validate_relative(&name) else {
                continue;
            };
            let bytes = read_limited(&mut entry, MAX_ASSET_BYTES + 1)
                .map_err(|error| format!("{}: {name}: {error}", path.display()))?;
            if bytes.len() as u64 > MAX_ASSET_BYTES {
                return Err(format!(
                    "{}: {name} is over the {MAX_ASSET_BYTES} byte per-file cap",
                    path.display()
                ));
            }
            total += bytes.len() as u64;
            if total > MAX_EXTENSION_BYTES {
                return Err(format!(
                    "{} expands to over {MAX_EXTENSION_BYTES} bytes",
                    path.display()
                ));
            }
            files.insert(key, bytes);
        }
        Ok(Source::Zip { path, files })
    }

    /// The path shown in errors: the directory, or the archive.
    pub(crate) fn origin(&self) -> &Path {
        match self {
            Source::Dir(path) => path,
            Source::Zip { path, .. } => path,
        }
    }

    /// Whether a relative file is present.
    pub(crate) fn has(&self, relative: &str) -> bool {
        let Ok(key) = validate_relative(relative) else {
            return false;
        };
        match self {
            Source::Dir(dir) => dir.join(&key).is_file(),
            Source::Zip { files, .. } => files.contains_key(&key),
        }
    }

    /// Read `relative` (a relative path), refusing one over `max` bytes.
    pub(crate) fn read(&self, relative: &str, max: u64, what: &str) -> Result<Vec<u8>, String> {
        let key = validate_relative(relative)?;
        match self {
            Source::Dir(dir) => read_file(&dir.join(&key), max, what),
            Source::Zip { files, .. } => {
                let bytes = files
                    .get(&key)
                    .ok_or_else(|| format!("{what} {relative:?} is missing"))?;
                if bytes.len() as u64 > max {
                    return Err(format!(
                        "{what} {relative:?} is {} bytes, over the {max} byte cap",
                        bytes.len()
                    ));
                }
                Ok(bytes.clone())
            }
        }
    }
}

/// Validate a relative path and return it as a `/`-joined key.
///
/// Every component must be a plain name; an absolute path, a `..`, or a `.` for
/// the whole path is refused.
pub(crate) fn validate_relative(relative: &str) -> Result<String, String> {
    let path = Path::new(relative);
    if path.is_absolute() || relative.is_empty() {
        return Err(format!("{relative:?} must be a relative path"));
    }
    let mut parts: Vec<&str> = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part
                    .to_str()
                    .ok_or_else(|| format!("{relative:?} is not valid UTF-8"))?;
                parts.push(part);
            }
            _ => return Err(format!("{relative:?} escapes the extension directory")),
        }
    }
    if parts.is_empty() {
        return Err(format!("{relative:?} does not name a file"));
    }
    Ok(parts.join("/"))
}

/// Read a file, refusing one over `max` bytes.
fn read_file(path: &Path, max: u64, what: &str) -> Result<Vec<u8>, String> {
    let metadata =
        std::fs::metadata(path).map_err(|error| format!("{what} {}: {error}", path.display()))?;
    if metadata.len() > max {
        return Err(format!(
            "{what} {} is {} bytes, over the {max} byte cap",
            path.display(),
            metadata.len()
        ));
    }
    std::fs::read(path).map_err(|error| format!("{what} {}: {error}", path.display()))
}

/// Read an archive entry, refusing one over `limit` bytes, so a lying header
/// cannot make us allocate without bound.
fn read_limited<R: Read>(reader: &mut R, limit: u64) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit).read_to_end(&mut bytes)?;
    Ok(bytes)
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
    }

    #[test]
    fn paths_that_escape_are_refused() {
        assert!(validate_relative("../evil.js").is_err());
        assert!(validate_relative("a/../../b").is_err());
        assert!(validate_relative("/etc/passwd").is_err());
        assert!(validate_relative("").is_err());
    }

    #[test]
    fn a_directory_source_reads_and_caps() {
        let dir = std::env::temp_dir().join(format!("noctis-source-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("hello.txt"), "hi").unwrap();

        let source = Source::dir(dir.clone());
        assert!(source.has("hello.txt"));
        assert!(!source.has("missing.txt"));
        assert_eq!(source.read("hello.txt", 1024, "file").unwrap(), b"hi");
        assert!(source.read("hello.txt", 1, "file").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_zip_source_reads_its_entries() {
        use std::io::Write;

        let dir = std::env::temp_dir().join(format!("noctis-source-zip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pack.zip");

        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("a.txt", options).unwrap();
        writer.write_all(b"alpha").unwrap();
        writer.start_file("../escape.txt", options).unwrap();
        writer.write_all(b"nope").unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        std::fs::write(&path, bytes).unwrap();

        let source = Source::zip(path.clone()).expect("opens");
        assert!(source.has("a.txt"));
        assert_eq!(source.read("a.txt", 1024, "file").unwrap(), b"alpha");
        // The escaping entry was dropped when the archive was read.
        assert!(!source.has("../escape.txt"));
        assert!(source.read("missing.txt", 1024, "file").is_err());
        assert_eq!(source.origin(), path);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_non_zip_file_is_refused() {
        let dir = std::env::temp_dir().join(format!("noctis-source-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.zip");
        std::fs::write(&path, b"not a zip").unwrap();
        assert!(Source::zip(path).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
