//! The [`Project`]: an open `noctis.toml` manifest plus the directory it lives in.

use std::path::{Path, PathBuf};

use ecs::scene::SceneMode;

use crate::error::ProjectError;
use crate::manifest::{BuildTarget, ProjectManifest};
use crate::path::AssetPath;

/// The manifest file name.
pub const MANIFEST_FILE: &str = "noctis.toml";

/// An open Noctis project: a root directory and its parsed manifest.
#[derive(Clone, Debug)]
pub struct Project {
    root: PathBuf,
    manifest: ProjectManifest,
}

impl Project {
    /// Open the project rooted at `dir`, reading `<dir>/noctis.toml`.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, ProjectError> {
        Self::from_manifest_file(dir.as_ref().join(MANIFEST_FILE))
    }

    /// Open the project whose manifest is at `path`.
    pub fn from_manifest_file(path: impl AsRef<Path>) -> Result<Self, ProjectError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|e| ProjectError::Io(e.to_string()))?;
        let manifest = ProjectManifest::from_toml(&text)?;
        manifest.validate()?;
        let root = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        Ok(Self { root, manifest })
    }

    /// Build a project from an already-parsed manifest rooted at `dir`.
    pub fn from_manifest(root: impl Into<PathBuf>, manifest: ProjectManifest) -> Self {
        Self {
            root: root.into(),
            manifest,
        }
    }

    /// The project root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The parsed manifest.
    pub fn manifest(&self) -> &ProjectManifest {
        &self.manifest
    }

    /// The project name.
    pub fn name(&self) -> &str {
        &self.manifest.project.name
    }

    /// The default presentation mode for new scenes.
    pub fn mode(&self) -> SceneMode {
        self.manifest.project.settings.mode
    }

    /// The `[[build]]` targets, in manifest order.
    pub fn build_targets(&self) -> &[BuildTarget] {
        &self.manifest.build_targets
    }

    /// A build target by name.
    pub fn build_target(&self, name: &str) -> Result<&BuildTarget, ProjectError> {
        self.manifest
            .build_targets
            .iter()
            .find(|target| target.name == name)
            .ok_or_else(|| ProjectError::Manifest(format!("no build target named {name:?}")))
    }

    /// The `default_scene` setting, as a raw project-relative path.
    pub fn default_scene(&self) -> Option<&str> {
        self.manifest.project.settings.default_scene.as_deref()
    }

    /// The default scene's file on disk, if a valid default is set.
    pub fn default_scene_file(&self) -> Option<PathBuf> {
        let scene = self.default_scene()?;
        AssetPath::parse(scene)
            .ok()
            .map(|asset| self.file_for(&asset))
    }

    /// Resolve a user-supplied path against the project: a relative path is
    /// treated as a project-relative asset path, while an absolute path (or one
    /// that is not a canonical asset path) is used as given.
    pub fn resolve_path(&self, path: &str) -> PathBuf {
        if Path::new(path).is_absolute() {
            return PathBuf::from(path);
        }
        match AssetPath::parse(path) {
            Ok(asset) => self.file_for(&asset),
            Err(_) => PathBuf::from(path),
        }
    }

    /// An absolute path for a project-relative path.
    pub fn dir(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// The absolute assets directory.
    pub fn assets_dir(&self) -> PathBuf {
        self.dir(&self.manifest.paths.assets)
    }

    /// The absolute scenes directory.
    pub fn scenes_dir(&self) -> PathBuf {
        self.dir(&self.manifest.paths.scenes)
    }

    /// The absolute prefabs directory.
    pub fn prefabs_dir(&self) -> PathBuf {
        self.dir(&self.manifest.paths.prefabs)
    }

    /// The absolute scripts directory.
    pub fn scripts_dir(&self) -> PathBuf {
        self.dir(&self.manifest.paths.scripts)
    }

    /// The on-disk file for a canonical asset path (ignoring any sub-asset
    /// fragment).
    pub fn file_for(&self, asset: &AssetPath) -> PathBuf {
        self.root.join(asset.file_path())
    }

    /// Every scene document under the scenes directory, as project-relative
    /// asset paths, sorted for determinism.
    pub fn discover_scenes(&self) -> Result<Vec<AssetPath>, ProjectError> {
        self.discover(&self.manifest.paths.scenes, ".scene.json")
    }

    /// Every prefab document under the prefabs directory, sorted.
    pub fn discover_prefabs(&self) -> Result<Vec<AssetPath>, ProjectError> {
        self.discover(&self.manifest.paths.prefabs, ".prefab.json")
    }

    /// Walk `dir` (recursively) and collect files whose name ends with `suffix`,
    /// as sorted project-relative asset paths. A missing directory yields none.
    fn discover(&self, dir: &str, suffix: &str) -> Result<Vec<AssetPath>, ProjectError> {
        let mut files = Vec::new();
        walk_files(&self.dir(dir), &mut files)?;
        let mut paths: Vec<AssetPath> = files
            .into_iter()
            .filter(|path| has_suffix(path, suffix))
            .filter_map(|path| relative_asset_path(&self.root, &path))
            .collect();
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

/// Whether `path`'s file name ends with `suffix` (case-insensitive).
fn has_suffix(path: &Path, suffix: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(suffix))
}

/// A project-relative asset path for an absolute `path` under `root`.
pub(crate) fn relative_asset_path(root: &Path, path: &Path) -> Option<AssetPath> {
    let relative = path.strip_prefix(root).ok()?;
    let text = relative.to_string_lossy().replace('\\', "/");
    AssetPath::parse(text).ok()
}

/// Recursively collect files under `dir`. A missing directory is not an error.
pub(crate) fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), ProjectError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(ProjectError::Io(error.to_string())),
    };
    for entry in entries {
        let entry = entry.map_err(|e| ProjectError::Io(e.to_string()))?;
        let file_type = entry
            .file_type()
            .map_err(|e| ProjectError::Io(e.to_string()))?;
        if file_type.is_dir() {
            walk_files(&entry.path(), out)?;
        } else {
            out.push(entry.path());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirs_follow_the_defaults() {
        let manifest = ProjectManifest::from_toml("[project]\nname = \"T\"\n").unwrap();
        let project = Project::from_manifest("/tmp/game", manifest);
        assert_eq!(project.root(), Path::new("/tmp/game"));
        assert_eq!(project.assets_dir(), Path::new("/tmp/game/assets"));
        assert_eq!(project.scenes_dir(), Path::new("/tmp/game/scenes"));
        assert_eq!(project.prefabs_dir(), Path::new("/tmp/game/prefabs"));
        assert_eq!(project.scripts_dir(), Path::new("/tmp/game/scripts"));
        assert_eq!(project.mode(), SceneMode::ThreeD);
    }

    #[test]
    fn dirs_follow_path_overrides() {
        let manifest =
            ProjectManifest::from_toml("[project]\nname = \"T\"\n[paths]\nscenes = \"levels\"\n")
                .unwrap();
        let project = Project::from_manifest("/tmp/game", manifest);
        assert_eq!(project.scenes_dir(), Path::new("/tmp/game/levels"));
        assert_eq!(project.assets_dir(), Path::new("/tmp/game/assets"));
    }

    #[test]
    fn file_for_ignores_the_fragment() {
        let manifest = ProjectManifest::from_toml("[project]\nname = \"T\"\n").unwrap();
        let project = Project::from_manifest("/tmp/game", manifest);
        let asset = AssetPath::parse("assets/models/robot.glb#mesh/Torso").unwrap();
        assert_eq!(
            project.file_for(&asset),
            Path::new("/tmp/game/assets/models/robot.glb")
        );
    }

    #[test]
    fn resolve_path_joins_relative_and_keeps_absolute() {
        let manifest = ProjectManifest::from_toml("[project]\nname = \"T\"\n").unwrap();
        let project = Project::from_manifest("/tmp/game", manifest);
        assert_eq!(
            project.resolve_path("scenes/main.scene.json"),
            Path::new("/tmp/game/scenes/main.scene.json")
        );
        assert_eq!(
            project.resolve_path("/abs/other.scene.json"),
            Path::new("/abs/other.scene.json")
        );
    }

    #[test]
    fn default_scene_file_resolves_the_setting() {
        let manifest = ProjectManifest::from_toml(
            "[project]\nname = \"T\"\n[project.settings]\ndefault_scene = \"scenes/main.scene.json\"\n",
        )
        .unwrap();
        let project = Project::from_manifest("/tmp/game", manifest);
        assert_eq!(
            project.default_scene_file(),
            Some(Path::new("/tmp/game/scenes/main.scene.json").to_path_buf())
        );
    }

    #[test]
    fn opening_a_missing_project_is_an_io_error() {
        let error = Project::open("/definitely/not/a/real/noctis/dir").unwrap_err();
        assert!(matches!(error, ProjectError::Io(_)), "{error:?}");
    }
}
