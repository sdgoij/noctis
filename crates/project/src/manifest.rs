//! The `noctis.toml` manifest.
//!
//! The manifest is parsed strictly: an unknown key is an error, because a typo
//! in a project file is a bug, not forward compatibility. Cross-field rules
//! (a non-empty name, unique build targets, valid asset paths) are checked by
//! [`ProjectManifest::validate`].

use serde::Deserialize;

use ecs::scene::SceneMode;

use crate::error::ProjectError;
use crate::path::AssetPath;

/// A parsed `noctis.toml`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectManifest {
    /// The `[project]` table (name, version, description, settings).
    pub project: ProjectInfo,
    /// Optional `[paths]` overrides for the conventional directories.
    #[serde(default)]
    pub paths: Paths,
    /// Optional `[window]` settings.
    #[serde(default)]
    pub window: WindowSettings,
    /// Optional `[render]` settings.
    #[serde(default)]
    pub render: RenderSettings,
    /// Optional `[input]` settings.
    #[serde(default)]
    pub input: InputSettings,
    /// `[[build]]` targets.
    #[serde(default, rename = "build")]
    pub build_targets: Vec<BuildTarget>,
    /// `[[scene]]` entries: explicit scenes, for names and order.
    #[serde(default, rename = "scene")]
    pub scene_list: Vec<SceneListEntry>,
}

impl ProjectManifest {
    /// Parse a manifest from TOML text.
    pub fn from_toml(text: &str) -> Result<Self, ProjectError> {
        toml::from_str(text).map_err(|error| ProjectError::Manifest(error.to_string()))
    }

    /// Check cross-field rules that serde cannot express.
    pub fn validate(&self) -> Result<(), ProjectError> {
        if self.project.name.trim().is_empty() {
            return Err(ProjectError::Manifest(
                "[project].name must not be empty".to_string(),
            ));
        }

        for (key, dir) in [
            ("assets", &self.paths.assets),
            ("scenes", &self.paths.scenes),
            ("prefabs", &self.paths.prefabs),
            ("scripts", &self.paths.scripts),
        ] {
            AssetPath::parse(dir).map_err(|error| {
                ProjectError::Manifest(format!("[paths].{key} is invalid: {error}"))
            })?;
        }

        if let Some(scene) = &self.project.settings.default_scene {
            AssetPath::parse(scene).map_err(|error| {
                ProjectError::Manifest(format!("default_scene is invalid: {error}"))
            })?;
        }

        if let Some(environment) = &self.render.environment {
            AssetPath::parse(environment).map_err(|error| {
                ProjectError::Manifest(format!("[render] environment is invalid: {error}"))
            })?;
        }

        let mut seen = std::collections::HashSet::new();
        for target in &self.build_targets {
            if target.name.trim().is_empty() {
                return Err(ProjectError::Manifest(
                    "a [[build]] target has an empty name".to_string(),
                ));
            }
            if !seen.insert(target.name.as_str()) {
                return Err(ProjectError::Manifest(format!(
                    "duplicate build target name {:?}",
                    target.name
                )));
            }
            if let Some(scene) = &target.scene {
                AssetPath::parse(scene).map_err(|error| {
                    ProjectError::Manifest(format!(
                        "build target {:?} scene is invalid: {error}",
                        target.name
                    ))
                })?;
            }
        }

        let mut listed = std::collections::HashSet::new();
        for entry in &self.scene_list {
            AssetPath::parse(&entry.path).map_err(|error| {
                ProjectError::Manifest(format!("[[scene]] path is invalid: {error}"))
            })?;
            if !listed.insert(entry.path.as_str()) {
                return Err(ProjectError::Manifest(format!(
                    "duplicate [[scene]] path {:?}",
                    entry.path
                )));
            }
        }

        Ok(())
    }
}

/// A `[[scene]]` entry: an explicit scene, for its display name and order.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneListEntry {
    /// The scene document, project-relative.
    pub path: String,
    /// A display name; defaults to the file stem.
    #[serde(default)]
    pub name: Option<String>,
}

/// The `[project]` table.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectInfo {
    /// The project name.
    pub name: String,
    /// The project version.
    #[serde(default)]
    pub version: Option<String>,
    /// A one-line description.
    #[serde(default)]
    pub description: Option<String>,
    /// Project-wide settings (`[project.settings]`).
    #[serde(default)]
    pub settings: Settings,
}

/// The `[project.settings]` table.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// The scene the runtime runs when none is given.
    #[serde(default)]
    pub default_scene: Option<String>,
    /// The default presentation mode for new scenes.
    #[serde(default)]
    pub mode: SceneMode,
    /// Gravity applied to dynamic bodies, metres per second squared.
    #[serde(default = "default_gravity")]
    pub gravity: [f32; 3],
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_scene: None,
            mode: SceneMode::default(),
            gravity: default_gravity(),
        }
    }
}

fn default_gravity() -> [f32; 3] {
    [0.0, -9.81, 0.0]
}

/// The `[paths]` table: overrides for the conventional directories.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Paths {
    /// Imported source files.
    #[serde(default = "default_assets")]
    pub assets: String,
    /// Scene documents.
    #[serde(default = "default_scenes")]
    pub scenes: String,
    /// Prefab documents.
    #[serde(default = "default_prefabs")]
    pub prefabs: String,
    /// Scripts.
    #[serde(default = "default_scripts")]
    pub scripts: String,
}

impl Default for Paths {
    fn default() -> Self {
        Self {
            assets: default_assets(),
            scenes: default_scenes(),
            prefabs: default_prefabs(),
            scripts: default_scripts(),
        }
    }
}

fn default_assets() -> String {
    "assets".to_string()
}
fn default_scenes() -> String {
    "scenes".to_string()
}
fn default_prefabs() -> String {
    "prefabs".to_string()
}
fn default_scripts() -> String {
    "scripts".to_string()
}

/// The `[window]` table.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowSettings {
    /// The window title; defaults to the project name.
    #[serde(default)]
    pub title: Option<String>,
    /// The initial width in logical pixels.
    #[serde(default = "default_width")]
    pub width: u32,
    /// The initial height in logical pixels.
    #[serde(default = "default_height")]
    pub height: u32,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            title: None,
            width: default_width(),
            height: default_height(),
        }
    }
}

fn default_width() -> u32 {
    1280
}
fn default_height() -> u32 {
    720
}

/// The `[render]` table.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenderSettings {
    /// Whether the HDR pipeline and tonemapping are used.
    #[serde(default = "default_true")]
    pub hdr: bool,
    /// MSAA sample count (`1` disables it).
    #[serde(default = "default_one")]
    pub sample_count: u32,
    /// The equirectangular `.hdr` environment the renderer precomputes image-based
    /// lighting from, project-relative. When absent the renderer falls back to a
    /// neutral, fully-lit environment.
    #[serde(default)]
    pub environment: Option<String>,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            hdr: true,
            sample_count: 1,
            environment: None,
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_one() -> u32 {
    1
}

/// The `[input]` table.
#[derive(Clone, Debug, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputSettings {
    /// The camera-navigation bindings file (see `noctis-editor --controls`).
    #[serde(default)]
    pub controls: Option<String>,
}

/// A `[[build]]` target.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildTarget {
    /// The artifact name.
    pub name: String,
    /// The scene to build; defaults to `settings.default_scene`.
    #[serde(default)]
    pub scene: Option<String>,
    /// The target platform.
    #[serde(default)]
    pub platform: Platform,
    /// The build profile.
    #[serde(default)]
    pub profile: Profile,
    /// The output directory; defaults to `dist/<name>`.
    #[serde(default)]
    pub output: Option<String>,
}

/// A build target platform.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    /// Whatever platform the build runs on.
    #[default]
    Host,
    /// Windows.
    Windows,
    /// macOS.
    Macos,
    /// Linux.
    Linux,
}

/// A build profile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    /// A debug/development build.
    Dev,
    /// A release build (the default).
    #[default]
    Release,
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"
[project]
name = "MyGame"
version = "0.1.0"
description = "A small platformer."

[project.settings]
default_scene = "scenes/main.scene.json"
mode = "2d"
gravity = [0.0, -3.0, 0.0]

[paths]
assets = "content"
scenes = "levels"
prefabs = "blueprints"
scripts = "code"

[window]
title = "MyGame"
width = 1920
height = 1080

[render]
hdr = false
sample_count = 4

[input]
controls = "input/controls.json"

[[build]]
name = "mygame"
scene = "levels/main.scene.json"
platform = "windows"
profile = "dev"
output = "dist/mygame"

[[build]]
name = "mygame-linux"
platform = "linux"
"#;

    #[test]
    fn parses_a_full_manifest() {
        let manifest = ProjectManifest::from_toml(FULL).unwrap();
        assert_eq!(manifest.project.name, "MyGame");
        assert_eq!(manifest.project.version.as_deref(), Some("0.1.0"));
        assert_eq!(manifest.project.settings.mode, SceneMode::TwoD);
        assert_eq!(manifest.project.settings.gravity, [0.0, -3.0, 0.0]);
        assert_eq!(
            manifest.project.settings.default_scene.as_deref(),
            Some("scenes/main.scene.json")
        );
        assert_eq!(manifest.paths.assets, "content");
        assert_eq!(manifest.window.width, 1920);
        assert!(!manifest.render.hdr);
        assert_eq!(manifest.render.sample_count, 4);
        assert_eq!(
            manifest.input.controls.as_deref(),
            Some("input/controls.json")
        );
        assert_eq!(manifest.build_targets.len(), 2);
        assert_eq!(manifest.build_targets[0].platform, Platform::Windows);
        assert_eq!(manifest.build_targets[0].profile, Profile::Dev);
        assert_eq!(manifest.build_targets[1].platform, Platform::Linux);
        assert_eq!(manifest.build_targets[1].profile, Profile::Release);
        manifest.validate().unwrap();
    }

    #[test]
    fn defaults_are_sensible() {
        let manifest = ProjectManifest::from_toml("[project]\nname = \"T\"\n").unwrap();
        assert_eq!(manifest.project.settings.mode, SceneMode::ThreeD);
        assert_eq!(manifest.project.settings.gravity, [0.0, -9.81, 0.0]);
        assert_eq!(manifest.paths.assets, "assets");
        assert_eq!(manifest.paths.scenes, "scenes");
        assert_eq!(manifest.window.width, 1280);
        assert_eq!(manifest.window.height, 720);
        assert!(manifest.render.hdr);
        assert!(manifest.build_targets.is_empty());
        manifest.validate().unwrap();
    }

    #[test]
    fn an_unknown_key_is_an_error() {
        // A typo in a manifest must be caught, not ignored.
        let error =
            ProjectManifest::from_toml("[project]\nname = \"T\"\nnaam = \"x\"\n").unwrap_err();
        assert!(matches!(error, ProjectError::Manifest(_)), "{error:?}");
        let error = ProjectManifest::from_toml("[project]\nname = \"T\"\n[render]\nfxaa = true\n")
            .unwrap_err();
        assert!(matches!(error, ProjectError::Manifest(_)), "{error:?}");
    }

    #[test]
    fn a_missing_project_table_is_an_error() {
        let error = ProjectManifest::from_toml("[render]\nhdr = true\n").unwrap_err();
        assert!(matches!(error, ProjectError::Manifest(_)), "{error:?}");
    }

    #[test]
    fn validate_rejects_empty_names_and_duplicate_targets() {
        let empty = ProjectManifest::from_toml("[project]\nname = \"  \"\n").unwrap();
        assert!(matches!(empty.validate(), Err(ProjectError::Manifest(_))));

        let dup = ProjectManifest::from_toml(
            "[project]\nname = \"T\"\n\n[[build]]\nname = \"a\"\n\n[[build]]\nname = \"a\"\n",
        )
        .unwrap();
        assert!(matches!(dup.validate(), Err(ProjectError::Manifest(_))));
    }

    #[test]
    fn validate_rejects_an_escaping_default_scene() {
        let manifest = ProjectManifest::from_toml(
            "[project]\nname = \"T\"\n[project.settings]\ndefault_scene = \"../x.scene.json\"\n",
        )
        .unwrap();
        assert!(matches!(
            manifest.validate(),
            Err(ProjectError::Manifest(_))
        ));
    }

    #[test]
    fn validate_rejects_an_escaping_environment() {
        let manifest = ProjectManifest::from_toml(
            "[project]\nname = \"T\"\n[render]\nenvironment = \"../sky.hdr\"\n",
        )
        .unwrap();
        assert!(matches!(
            manifest.validate(),
            Err(ProjectError::Manifest(_))
        ));
    }

    #[test]
    fn parses_a_render_environment() {
        let manifest = ProjectManifest::from_toml(
            "[project]\nname = \"T\"\n[render]\nenvironment = \"assets/hdris/sky.hdr\"\n",
        )
        .unwrap();
        assert_eq!(
            manifest.render.environment.as_deref(),
            Some("assets/hdris/sky.hdr")
        );
        assert!(
            manifest.render.hdr,
            "the other render keys keep their defaults"
        );
        manifest.validate().unwrap();

        let none = ProjectManifest::from_toml("[project]\nname = \"T\"\n").unwrap();
        assert_eq!(none.render.environment, None);
    }

    #[test]
    fn parses_scene_entries_and_rejects_duplicates() {
        let manifest = ProjectManifest::from_toml(
            "[project]\nname = \"T\"\n\n[[scene]]\npath = \"scenes/a.scene.json\"\nname = \"A\"\n",
        )
        .unwrap();
        assert_eq!(manifest.scene_list.len(), 1);
        assert_eq!(manifest.scene_list[0].path, "scenes/a.scene.json");
        assert_eq!(manifest.scene_list[0].name.as_deref(), Some("A"));
        manifest.validate().unwrap();

        let duplicate = ProjectManifest::from_toml(
            "[project]\nname = \"T\"\n\n[[scene]]\npath = \"scenes/a.scene.json\"\n\n[[scene]]\npath = \"scenes/a.scene.json\"\n",
        )
        .unwrap();
        assert!(matches!(
            duplicate.validate(),
            Err(ProjectError::Manifest(_))
        ));
    }
}
