//! Build a distributable game folder.
//!
//! `noctis-runtime --export <dir> --scene <level.json>` writes a self-contained
//! folder containing:
//!
//! * a copy of the running runtime executable, renamed to the game,
//! * the scene as [`BUNDLED_SCENE`],
//! * a [`MANIFEST`] naming both.
//!
//! The exported executable loads [`BUNDLED_SCENE`] from its own directory when it
//! is run with no arguments (see [`bundled_scene`] and the binary's
//! `build_runtime`), so the folder is self-contained and double-clickable.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// The scene file name inside an exported folder.
pub const BUNDLED_SCENE: &str = "scene.json";

/// The manifest file name inside an exported folder.
pub const MANIFEST: &str = "noctis-game.json";

/// What [`export_game`] wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportReport {
    /// The game folder.
    pub directory: PathBuf,
    /// The copied executable.
    pub executable: PathBuf,
    /// The bundled scene file.
    pub scene: PathBuf,
    /// The manifest file.
    pub manifest: PathBuf,
}

impl fmt::Display for ExportReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = self
            .executable
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        write!(f, "exported {name} to {}", self.directory.display())
    }
}

/// Anything that can go wrong while exporting.
#[derive(Debug)]
pub enum ExportError {
    /// The game name is not a usable single-component file name.
    InvalidName(String),
    /// A filesystem operation failed.
    Io(String),
    /// The export manifest could not be serialized.
    Json(String),
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExportError::InvalidName(name) => {
                write!(f, "invalid game name {name:?}: use a plain file name")
            }
            ExportError::Io(message) => write!(f, "io error: {message}"),
            ExportError::Json(message) => write!(f, "json error: {message}"),
        }
    }
}

impl std::error::Error for ExportError {}

fn io(error: std::io::Error) -> ExportError {
    ExportError::Io(error.to_string())
}

/// `true` if `name` is a safe, single-component game name.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\'])
        && !name.contains(char::is_control)
}

/// The exported executable's file name for `name`.
pub fn executable_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// The scene bundled next to an executable, if one is present.
pub fn bundled_scene(directory: &Path) -> Option<PathBuf> {
    let candidate = directory.join(BUNDLED_SCENE);
    candidate.is_file().then_some(candidate)
}

/// Export a self-contained game folder.
///
/// Copies the current executable, writes `scene_json` as [`BUNDLED_SCENE`], and
/// writes a [`MANIFEST`]. Re-exporting over an existing folder overwrites the
/// executable, scene, and manifest.
pub fn export_game(
    destination: &Path,
    name: &str,
    scene_json: &str,
) -> Result<ExportReport, ExportError> {
    if !valid_name(name) {
        return Err(ExportError::InvalidName(name.to_string()));
    }
    fs::create_dir_all(destination).map_err(io)?;

    let executable = destination.join(executable_name(name));
    let source = std::env::current_exe().map_err(io)?;
    // Copying a running executable onto itself would fail; that only happens when
    // exporting into the executable's own directory, which needs no copy.
    if source != executable {
        fs::copy(&source, &executable).map_err(io)?;
    }

    let scene = destination.join(BUNDLED_SCENE);
    fs::write(&scene, scene_json).map_err(io)?;

    let manifest = destination.join(MANIFEST);
    let document = serde_json::json!({
        "name": name,
        "engine": concat!("noctis ", env!("CARGO_PKG_VERSION")),
        "executable": executable_name(name),
        "scene": BUNDLED_SCENE,
    });
    let text =
        serde_json::to_string_pretty(&document).map_err(|e| ExportError::Json(e.to_string()))?;
    fs::write(&manifest, text).map_err(io)?;

    Ok(ExportReport {
        directory: destination.to_path_buf(),
        executable,
        scene,
        manifest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "noctis-export-{tag}-{}-{nanos}",
            std::process::id()
        ))
    }

    #[test]
    fn valid_names_reject_paths_and_empties() {
        assert!(valid_name("my-game"));
        assert!(valid_name("Game_2024"));
        assert!(!valid_name(""));
        assert!(!valid_name("."));
        assert!(!valid_name(".."));
        assert!(!valid_name("a/b"));
        assert!(!valid_name("a\\b"));
        assert!(!valid_name("bad\nname"));
    }

    #[test]
    fn executable_name_matches_the_platform() {
        assert_eq!(executable_name("game").ends_with(".exe"), cfg!(windows));
    }

    #[test]
    fn export_writes_a_self_contained_folder() {
        let dir = temp_dir("folder");
        let scene_json = r#"{"name":"Level"}"#;
        let report = export_game(&dir, "my-game", scene_json).expect("export");

        assert!(report.executable.is_file());
        assert_eq!(
            fs::read_to_string(&report.scene).expect("read scene"),
            scene_json
        );

        let manifest: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&report.manifest).expect("read manifest"))
                .expect("parse manifest");
        assert_eq!(manifest["name"], "my-game");
        assert_eq!(manifest["scene"], BUNDLED_SCENE);
        assert_eq!(manifest["executable"], executable_name("my-game"));
        assert!(
            manifest["engine"]
                .as_str()
                .is_some_and(|s| s.starts_with("noctis "))
        );

        // The copied executable is byte-for-byte the one we are running.
        let source = fs::read(std::env::current_exe().expect("current exe")).expect("read exe");
        assert_eq!(fs::read(&report.executable).expect("read copy"), source);

        // The exported folder exposes the scene through `bundled_scene`.
        assert_eq!(bundled_scene(&dir), Some(report.scene.clone()));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn export_rejects_an_unsafe_name_without_touching_disk() {
        let dir = temp_dir("bad-name");
        let error = export_game(&dir, "../evil", "{}").expect_err("must reject");
        assert!(matches!(error, ExportError::InvalidName(_)));
        assert!(!dir.exists());
    }

    #[test]
    fn bundled_scene_is_none_without_a_scene_file() {
        let dir = temp_dir("empty");
        fs::create_dir_all(&dir).expect("mkdir");
        assert_eq!(bundled_scene(&dir), None);
        fs::remove_dir_all(&dir).ok();
    }
}
