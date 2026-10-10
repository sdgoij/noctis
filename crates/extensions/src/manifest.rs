//! The extension manifest (`extension.json`) and how it is read.
//!
//! Parsing is strict (`deny_unknown_fields`): a typo is an error, not forward
//! compatibility — the same rule `noctis.toml` follows. A manifest that fails is
//! reported for that extension only; the others still load.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::source::Source;
use crate::{EXTENSION_API, MAX_ASSET_BYTES, MAX_ENTRY_BYTES, MAX_MANIFEST_BYTES};

/// The manifest file name every extension directory holds.
pub const MANIFEST: &str = "extension.json";

/// A declared asset: a slot an extension points at a file, read into memory.
///
/// `name` is the opaque, host-registered name the scene references — never the
/// path. See `docs/extensions.md`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    /// The logical slot name.
    pub slot: String,
    /// The opaque registered name (`ext:<id>:<slot>`).
    pub name: String,
    /// The file's bytes.
    pub bytes: Vec<u8>,
}

/// A compiled extension module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WasmModule {
    /// The module path, relative to the extension directory.
    pub module: String,
    /// The module's bytes.
    pub bytes: Vec<u8>,
}

/// A discovered, validated extension.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    /// The stable id.
    pub id: String,
    /// The human name.
    pub name: String,
    /// The extension's own version.
    pub version: String,
    /// The `noctis` API major it targets.
    pub api: u32,
    /// A one-line description.
    pub description: Option<String>,
    /// Free-text author.
    pub author: Option<String>,
    /// Ids that must load first.
    pub load_after: Vec<String>,
    /// The JS entry path, relative to the directory.
    pub entry: Option<String>,
    /// The JS entry source, read into memory.
    pub entry_source: Option<String>,
    /// The compiled module, if any.
    pub wasm: Option<WasmModule>,
    /// The declared assets.
    pub assets: Vec<Asset>,
    /// The extension's directory, or the archive it was read from.
    pub dir: PathBuf,
}

impl Manifest {
    /// The directory holding the extension.
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// The raw JSON shape. Separate from [`Manifest`] so the parsed, in-memory form
/// (with sources and bytes read) never crosses `serde`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    id: String,
    name: String,
    version: String,
    api: u32,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default, rename = "loadAfter")]
    load_after: Vec<String>,
    #[serde(default)]
    entry: Option<String>,
    #[serde(default)]
    wasm: Option<RawWasm>,
    #[serde(default)]
    assets: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWasm {
    module: String,
}

/// Read and validate the extension in `source`, loading its entry, module and
/// assets.
pub(crate) fn load(source: &Source) -> Result<Manifest, String> {
    let text = source.read(MANIFEST, MAX_MANIFEST_BYTES, MANIFEST)?;
    let text = String::from_utf8(text).map_err(|e| format!("{MANIFEST} is not UTF-8: {e}"))?;
    let raw: RawManifest = serde_json::from_str(&text).map_err(|e| format!("{MANIFEST}: {e}"))?;

    if !valid_id(&raw.id) {
        return Err(format!(
            "id {:?} is not a valid identifier (letters, digits, '.', '-', '_')",
            raw.id
        ));
    }
    if raw.name.trim().is_empty() {
        return Err("name must not be empty".into());
    }
    if raw.version.trim().is_empty() {
        return Err("version must not be empty".into());
    }
    if raw.api != EXTENSION_API {
        return Err(format!(
            "api {} is not supported (this editor is api {EXTENSION_API})",
            raw.api
        ));
    }

    let entry_source = match &raw.entry {
        Some(relative) => {
            let bytes = source.read(relative, MAX_ENTRY_BYTES, "entry")?;
            Some(
                String::from_utf8(bytes)
                    .map_err(|e| format!("entry {relative:?} is not UTF-8: {e}"))?,
            )
        }
        None => None,
    };

    let wasm = match &raw.wasm {
        Some(wasm) => {
            let bytes = source.read(&wasm.module, MAX_ENTRY_BYTES, "wasm module")?;
            if !bytes.starts_with(b"\0asm") {
                return Err(format!("{:?} is not a WebAssembly module", wasm.module));
            }
            Some(WasmModule {
                module: wasm.module.clone(),
                bytes,
            })
        }
        None => None,
    };

    let mut assets = Vec::new();
    for (slot, file) in &raw.assets {
        if slot.trim().is_empty() {
            return Err("an asset slot name is empty".into());
        }
        let bytes = source.read(file, MAX_ASSET_BYTES, "asset")?;
        assets.push(Asset {
            slot: slot.clone(),
            name: format!("ext:{}:{}", raw.id, slot),
            bytes,
        });
    }

    Ok(Manifest {
        id: raw.id,
        name: raw.name,
        version: raw.version,
        api: raw.api,
        description: raw.description,
        author: raw.author,
        load_after: raw.load_after,
        entry: raw.entry,
        entry_source,
        wasm,
        assets,
        dir: source.origin().to_path_buf(),
    })
}

/// A valid id: non-empty, at most 128 bytes, letters/digits/`.`/`-`/`_`, and not
/// starting or ending with a dot.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('.')
        && !id.ends_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("noctis-ext-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(path, text).expect("write");
    }

    /// Load a directory as a [`Source`] and parse its manifest. Shadows the
    /// imported `load` so the tests below read a directory by path.
    fn load(dir: &Path) -> Result<Manifest, String> {
        super::load(&Source::dir(dir.to_path_buf()))
    }

    #[test]
    fn a_minimal_manifest_loads() {
        let dir = temp_dir("minimal");
        write(
            &dir.join(MANIFEST),
            r#"{ "id": "com.example.a", "name": "A", "version": "1.0.0", "api": 1 }"#,
        );
        let manifest = load(&dir).expect("loads");
        assert_eq!(manifest.id, "com.example.a");
        assert_eq!(manifest.name, "A");
        assert!(manifest.entry.is_none());
        assert!(manifest.wasm.is_none());
        assert!(manifest.assets.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn entry_wasm_and_assets_are_read() {
        let dir = temp_dir("full");
        write(
            &dir.join(MANIFEST),
            r#"{
              "id": "com.example.full", "name": "Full", "version": "2.0.0", "api": 1,
              "entry": "extension.js",
              "wasm": { "module": "extension.wasm" },
              "assets": { "grid": "assets/grid.bin" }
            }"#,
        );
        write(&dir.join("extension.js"), "noctis.log('hi');");
        std::fs::write(dir.join("extension.wasm"), b"\0asm\x01\0\0\0").unwrap();
        write(&dir.join("assets/grid.bin"), "payload");

        let manifest = load(&dir).expect("loads");
        assert_eq!(manifest.entry_source.as_deref(), Some("noctis.log('hi');"));
        assert_eq!(manifest.wasm.as_ref().unwrap().bytes, b"\0asm\x01\0\0\0");
        assert_eq!(manifest.assets.len(), 1);
        assert_eq!(manifest.assets[0].name, "ext:com.example.full:grid");
        assert_eq!(manifest.assets[0].bytes, b"payload");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_api_mismatch_is_refused() {
        let dir = temp_dir("api");
        write(
            &dir.join(MANIFEST),
            r#"{ "id": "a", "name": "A", "version": "1", "api": 2 }"#,
        );
        let error = load(&dir).unwrap_err();
        assert!(error.contains("api 2"), "{error}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unknown_field_is_refused() {
        let dir = temp_dir("unknown");
        write(
            &dir.join(MANIFEST),
            r#"{ "id": "a", "name": "A", "version": "1", "api": 1, "shiny": true }"#,
        );
        let error = load(&dir).unwrap_err();
        assert!(error.contains("unknown field"), "{error}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_path_that_escapes_is_refused() {
        let dir = temp_dir("escape");
        write(
            &dir.join(MANIFEST),
            r#"{ "id": "a", "name": "A", "version": "1", "api": 1, "entry": "../evil.js" }"#,
        );
        let error = load(&dir).unwrap_err();
        assert!(error.contains("escapes"), "{error}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_non_module_wasm_is_refused() {
        let dir = temp_dir("badwasm");
        write(
            &dir.join(MANIFEST),
            r#"{ "id": "a", "name": "A", "version": "1", "api": 1, "wasm": { "module": "x.wasm" } }"#,
        );
        write(&dir.join("x.wasm"), "not a module");
        let error = load(&dir).unwrap_err();
        assert!(error.contains("WebAssembly"), "{error}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_bad_id_is_refused() {
        let dir = temp_dir("badid");
        write(
            &dir.join(MANIFEST),
            r#"{ "id": "bad/id", "name": "A", "version": "1", "api": 1 }"#,
        );
        assert!(load(&dir).unwrap_err().contains("valid identifier"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
