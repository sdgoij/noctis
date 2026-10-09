//! Embedded scripts: the scripts baked into a client binary.
//!
//! A release build embeds a project's scripts in the client, so the game folder
//! carries no loose `scripts/` directory (see `docs/scripts.md`).
//! `crates/client/build.rs` generates a `&'static [EmbeddedScript]` from the pack
//! `noctis build` writes, and the CLI seeds it into the asset registry at
//! startup, so a scene's `Script.path` resolves with no file on disk.

use ecs::assets::{AssetRegistry, ScriptAsset};

/// One script baked into the client: its project-relative path and raw bytes.
///
/// The bytes are the file exactly as it was on disk — JavaScript text, or a
/// WebAssembly module — decoded by [`ScriptAsset::from_file_bytes`].
#[derive(Clone, Copy, Debug)]
pub struct EmbeddedScript {
    /// The project-relative path a scene references (for example `scripts/x.js`).
    pub path: &'static str,
    /// The file's raw bytes.
    pub bytes: &'static [u8],
}

impl EmbeddedScript {
    /// The script asset for this entry, or an error when the bytes are neither
    /// UTF-8 JavaScript nor a WebAssembly module.
    pub fn asset(&self) -> Result<ScriptAsset, String> {
        ScriptAsset::from_file_bytes(self.path, self.bytes)
    }
}

/// Seed `assets` with every entry in `scripts`, returning how many registered.
///
/// An entry that cannot be decoded is logged and skipped rather than fatal, so a
/// single bad script cannot stop a game from starting.
pub fn seed(assets: &mut AssetRegistry, scripts: &[EmbeddedScript]) -> usize {
    let mut seeded = 0;
    for script in scripts {
        match script.asset() {
            Ok(asset) => {
                assets.scripts.insert_named(script.path, asset);
                seeded += 1;
            }
            Err(error) => {
                log::warn!(
                    "embedded script {:?} could not be decoded: {error}",
                    script.path
                );
            }
        }
    }
    seeded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_javascript_and_wasm_scripts() {
        static SCRIPTS: &[EmbeddedScript] = &[
            EmbeddedScript {
                path: "scripts/a.js",
                bytes: b"function on_update() {}",
            },
            EmbeddedScript {
                path: "scripts/b.wasm",
                bytes: b"\0asm\x01\x00",
            },
        ];
        let mut assets = AssetRegistry::new();
        assert_eq!(seed(&mut assets, SCRIPTS), 2);

        let js = assets
            .scripts
            .get_by_path("scripts/a.js")
            .expect("js seeded");
        assert_eq!(js.source, "function on_update() {}");
        let wasm = assets
            .scripts
            .get_by_path("scripts/b.wasm")
            .expect("wasm seeded");
        assert!(ecs::assets::is_wasm_source(&wasm.source));
    }

    #[test]
    fn an_undecodable_entry_is_skipped() {
        static SCRIPTS: &[EmbeddedScript] = &[EmbeddedScript {
            path: "scripts/bad",
            bytes: &[0xff, 0xfe],
        }];
        let mut assets = AssetRegistry::new();
        assert_eq!(seed(&mut assets, SCRIPTS), 0);
        assert!(assets.scripts.get_by_path("scripts/bad").is_none());
    }
}
