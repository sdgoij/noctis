//! Per-asset import settings.
//!
//! Import settings control how a source file is decoded. They are **persisted
//! next to the asset** as a sidecar (`wood.png.import.toml`) so they are
//! versioned and portable, unlike the generated `.noctis/` cache. A file without
//! a sidecar uses the defaults for its kind.
//!
//! ```toml
//! # assets/textures/wood.png.import.toml
//! srgb = true
//! ```

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The sidecar file-name suffix (`<asset>.import.toml`).
pub const SIDECAR_SUFFIX: &str = ".import.toml";

/// How a texture file is decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextureImportSettings {
    /// Whether the texture holds color (sRGB) rather than data (linear).
    #[serde(default = "default_true")]
    pub srgb: bool,
}

impl Default for TextureImportSettings {
    fn default() -> Self {
        Self { srgb: true }
    }
}

fn default_true() -> bool {
    true
}

impl TextureImportSettings {
    /// The sidecar path for `file`: `<file>.import.toml`.
    pub fn sidecar_for(file: &Path) -> PathBuf {
        let mut name = file.as_os_str().to_os_string();
        name.push(SIDECAR_SUFFIX);
        PathBuf::from(name)
    }

    /// Load the settings for `file`, or the defaults when there is no sidecar.
    pub fn load(file: &Path) -> Result<Self, String> {
        let sidecar = Self::sidecar_for(file);
        let text = match std::fs::read_to_string(&sidecar) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(format!("failed to read {}: {error}", sidecar.display())),
        };
        toml::from_str(&text).map_err(|error| format!("invalid {}: {error}", sidecar.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sidecar_path_appends_the_suffix() {
        let path = Path::new("assets/textures/wood.png");
        assert_eq!(
            TextureImportSettings::sidecar_for(path),
            PathBuf::from("assets/textures/wood.png.import.toml")
        );
    }

    #[test]
    fn defaults_are_srgb() {
        assert!(TextureImportSettings::default().srgb);
        let loaded = TextureImportSettings::load(Path::new("does-not-exist.png")).unwrap();
        assert_eq!(loaded, TextureImportSettings::default());
    }

    #[test]
    fn a_sidecar_overrides_the_default() {
        let dir = std::env::temp_dir().join(format!(
            "noctis-import-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let asset = dir.join("data.png");
        std::fs::write(&asset, b"x").unwrap();
        std::fs::write(TextureImportSettings::sidecar_for(&asset), "srgb = false\n").unwrap();

        let loaded = TextureImportSettings::load(&asset).unwrap();
        assert!(!loaded.srgb);

        // An unknown key is an error (a typo in an import setting is a bug).
        std::fs::write(
            TextureImportSettings::sidecar_for(&asset),
            "brightness = 2.0\n",
        )
        .unwrap();
        assert!(TextureImportSettings::load(&asset).is_err());

        std::fs::remove_dir_all(&dir).ok();
    }
}
