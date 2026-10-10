//! Enablement persistence: which extensions the user has turned off.
//!
//! One small TOML file at [`default_config_path`]. Everything discovered is
//! enabled unless its id is listed in `disabled`. The file is read at startup
//! and written by the (planned) extensions screen; it is never required to exist.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The user's extension config.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExtensionConfig {
    /// The ids the user has disabled.
    pub disabled: Vec<String>,
}

impl ExtensionConfig {
    /// Parse from TOML text.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|error| error.to_string())
    }

    /// Render to TOML text.
    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("the config serializes")
    }

    /// Load the config at `path`; a missing file is the default (nothing disabled).
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        match std::fs::read_to_string(path.as_ref()) {
            Ok(text) => Self::from_toml(&text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.to_string()),
        }
    }

    /// Write the config to `path`, creating parent directories.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(path, self.to_toml()).map_err(|error| error.to_string())
    }

    /// Whether `id` is enabled (not listed in `disabled`).
    pub fn is_enabled(&self, id: &str) -> bool {
        !self.disabled.iter().any(|disabled| disabled == id)
    }

    /// Enable or disable `id`, keeping `disabled` sorted and duplicate-free.
    pub fn set_enabled(&mut self, id: &str, enabled: bool) {
        self.disabled.retain(|disabled| disabled != id);
        if !enabled {
            self.disabled.push(id.to_string());
            self.disabled.sort();
            self.disabled.dedup();
        }
    }
}

/// The directory the config lives in.
///
/// `$NOCTIS_CONFIG_DIR` wins; otherwise the platform config directory
/// (`%APPDATA%\noctis` on Windows, `$XDG_CONFIG_HOME/noctis` or
/// `~/.config/noctis` elsewhere); otherwise `.noctis` in the working directory.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("NOCTIS_CONFIG_DIR")
        && !dir.is_empty()
    {
        return PathBuf::from(dir);
    }

    #[cfg(windows)]
    if let Some(appdata) = std::env::var_os("APPDATA")
        && !appdata.is_empty()
    {
        return PathBuf::from(appdata).join("noctis");
    }

    #[cfg(not(windows))]
    {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME")
            && !xdg.is_empty()
        {
            return PathBuf::from(xdg).join("noctis");
        }
        if let Some(home) = std::env::var_os("HOME")
            && !home.is_empty()
        {
            return PathBuf::from(home).join(".config").join("noctis");
        }
    }

    PathBuf::from(".noctis")
}

/// The default config file path (`config_dir()/extensions.toml`).
pub fn default_config_path() -> PathBuf {
    config_dir().join("extensions.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("noctis-extcfg-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("extensions.toml")
    }

    #[test]
    fn a_missing_file_is_the_default() {
        let path = temp_path("missing");
        let config = ExtensionConfig::load(&path).expect("default");
        assert!(config.disabled.is_empty());
        assert!(config.is_enabled("anything"));
    }

    #[test]
    fn set_enabled_keeps_the_list_sorted_and_unique() {
        let mut config = ExtensionConfig::default();
        config.set_enabled("b", false);
        config.set_enabled("a", false);
        config.set_enabled("b", false);
        assert_eq!(config.disabled, ["a", "b"]);
        assert!(!config.is_enabled("a"));
        assert!(config.is_enabled("c"));

        config.set_enabled("a", true);
        assert_eq!(config.disabled, ["b"]);
    }

    #[test]
    fn it_round_trips_through_a_file() {
        let path = temp_path("round-trip");
        let mut config = ExtensionConfig::default();
        config.set_enabled("com.example.noisy", false);
        config.save(&path).expect("save");

        let back = ExtensionConfig::load(&path).expect("load");
        assert_eq!(back, config);
        assert!(!back.is_enabled("com.example.noisy"));
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn an_unknown_field_is_refused() {
        let error = ExtensionConfig::from_toml("enabled = true").unwrap_err();
        assert!(error.contains("unknown field"), "{error}");
    }

    #[test]
    fn a_toml_typo_is_reported() {
        assert!(ExtensionConfig::from_toml("disabled = 3").is_err());
    }
}
