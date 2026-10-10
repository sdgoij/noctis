//! The [`Project`]: an open `noctis.toml` manifest plus the directory it lives in.

use std::path::{Path, PathBuf};

use ecs::scene::SceneMode;
use vfs::{DirStore, Store, StoreError};

use crate::error::ProjectError;
use crate::manifest::{BuildTarget, ProjectManifest};
use crate::path::AssetPath;

/// The manifest file name.
pub const MANIFEST_FILE: &str = "noctis.toml";

/// A project scene: its document path and a display name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneInfo {
    /// The scene document, project-relative.
    pub path: AssetPath,
    /// The display name (the `[[scene]]` name, or the file stem).
    pub name: String,
}

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
        let manifest_dir = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| ProjectError::Io(format!("{}: bad manifest path", path.display())))?;
        let text = DirStore::new(manifest_dir)
            .read_string(name)
            .map_err(|error| ProjectError::Io(error.to_string()))?;
        let manifest = ProjectManifest::from_toml(&text)?;
        manifest.validate()?;
        // A `[project] root` relocates the project, so a development build's
        // manifest can sit beside its client while the files live in the project
        // it was built from.
        let root = match manifest.project.root.as_deref() {
            Some(root) => {
                let root = Path::new(root);
                if root.is_absolute() {
                    root.to_path_buf()
                } else {
                    manifest_dir.join(root)
                }
            }
            None => manifest_dir.to_path_buf(),
        };
        Ok(Self { root, manifest })
    }

    /// Build a project from an already-parsed manifest rooted at `dir`.
    pub fn from_manifest(root: impl Into<PathBuf>, manifest: ProjectManifest) -> Self {
        Self {
            root: root.into(),
            manifest,
        }
    }

    /// A store rooted at the project directory.
    ///
    /// Every project file is read and written through this, so the crate does not
    /// call `std::fs` itself (see `docs/web.md`) and a browser build can swap the
    /// backend.
    pub(crate) fn store(&self) -> DirStore {
        DirStore::new(self.root.clone())
    }

    /// Read a project-relative file as UTF-8 text.
    fn read_string(&self, relative: &str) -> Result<String, ProjectError> {
        self.store()
            .read_string(relative)
            .map_err(|error| ProjectError::Io(format!("{relative}: {error}")))
    }

    /// Write a project-relative file, creating parent directories.
    fn write_file(&self, relative: &str, bytes: &[u8]) -> Result<(), ProjectError> {
        let mut store = self.store();
        store
            .write(relative, bytes)
            .map_err(|error| ProjectError::Io(format!("{relative}: {error}")))
    }

    /// The files under a project-relative directory, as `/`-joined relative paths.
    pub(crate) fn walk(&self, dir: &str, out: &mut Vec<String>) -> Result<(), ProjectError> {
        walk_files(&self.store(), dir, out)
    }

    /// The `[package].name` of the crate at `<crate_dir>/Cargo.toml`.
    fn crate_package_name(&self, crate_dir: &str) -> Result<String, ProjectError> {
        let relative = format!("{crate_dir}/Cargo.toml");
        let text = self.read_string(&relative)?;
        let value: toml::Value =
            toml::from_str(&text).map_err(|error| ProjectError::Manifest(error.to_string()))?;
        value
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(|name| name.as_str())
            .map(str::to_string)
            .ok_or_else(|| ProjectError::Manifest(format!("{relative}: missing [package].name")))
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

    /// The plugin library crate directory, when the project carries Rust.
    pub fn rust_crate_dir(&self) -> Option<PathBuf> {
        self.manifest
            .rust
            .as_ref()
            .map(|rust| self.dir(&rust.crate_dir))
    }

    /// The plugin function's Rust path (e.g. `mygame::plugin`).
    ///
    /// When the manifest gives no `[rust].plugin`, the default is
    /// `<package name>::plugin`, read from the plugin crate's `Cargo.toml` (with
    /// `-` normalized to `_`). `None` when the project has no `[rust]` table.
    pub fn plugin_path(&self) -> Result<Option<String>, ProjectError> {
        let Some(rust) = &self.manifest.rust else {
            return Ok(None);
        };
        if let Some(plugin) = &rust.plugin {
            return Ok(Some(plugin.clone()));
        }
        let package = self.crate_package_name(&rust.crate_dir)?;
        Ok(Some(format!("{}::plugin", package.replace('-', "_"))))
    }

    /// The plugin crate's `[package].name`, when the project carries Rust.
    pub fn rust_package_name(&self) -> Result<Option<String>, ProjectError> {
        let Some(rust) = &self.manifest.rust else {
            return Ok(None);
        };
        Ok(Some(self.crate_package_name(&rust.crate_dir)?))
    }

    /// The project's extension search directories, absolute (see `docs/extensions.md`).
    pub fn extension_dirs(&self) -> Vec<PathBuf> {
        self.manifest
            .extensions
            .dirs
            .iter()
            .map(|dir| self.dir(dir))
            .collect()
    }

    /// The extension ids this project disables.
    pub fn disabled_extensions(&self) -> &[String] {
        &self.manifest.extensions.disabled
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

    /// The project's scenes: the manifest's explicit `[[scene]]` entries first (in
    /// order, with their names), then any discovered scene not listed.
    pub fn scenes(&self) -> Result<Vec<SceneInfo>, ProjectError> {
        let mut seen = std::collections::HashSet::new();
        let mut scenes = Vec::new();
        for entry in &self.manifest.scene_list {
            let path = AssetPath::parse(&entry.path)?;
            let name = entry.name.clone().unwrap_or_else(|| scene_name(&path));
            seen.insert(path.clone());
            scenes.push(SceneInfo { path, name });
        }
        for path in self.discover_scenes()? {
            if seen.insert(path.clone()) {
                scenes.push(SceneInfo {
                    name: scene_name(&path),
                    path,
                });
            }
        }
        Ok(scenes)
    }

    /// Every prefab document under the prefabs directory, sorted.
    pub fn discover_prefabs(&self) -> Result<Vec<AssetPath>, ProjectError> {
        self.discover(&self.manifest.paths.prefabs, ".prefab.json")
    }

    /// The document path for a prefab named `name`: `prefabs/<name>.prefab.json`.
    pub fn prefab_path(&self, name: &str) -> Result<AssetPath, ProjectError> {
        let name = name.trim();
        if name.is_empty()
            || name.contains(['/', '\\'])
            || name.contains(char::is_control)
            || name == "."
            || name == ".."
        {
            return Err(ProjectError::Path(format!(
                "invalid prefab name {name:?}: use a plain name"
            )));
        }
        AssetPath::parse(format!(
            "{}/{name}.prefab.json",
            self.manifest.paths.prefabs
        ))
    }

    /// Write `prefab` to its document (`prefabs/<name>.prefab.json`), creating the
    /// directory. Returns the asset path written.
    pub fn save_prefab(&self, prefab: &ecs::scene::Prefab) -> Result<AssetPath, ProjectError> {
        let path = self.prefab_path(&prefab.name)?;
        self.write_file(path.file_path(), prefab.to_json().as_bytes())?;
        Ok(path)
    }

    /// Every prefab document, parsed.
    ///
    /// A malformed prefab document is an error (a prefab is authored, not
    /// generated, so a broken one is a bug to surface, not skip).
    pub fn load_prefabs(&self) -> Result<Vec<ecs::scene::Prefab>, ProjectError> {
        let mut prefabs = Vec::new();
        for path in self.discover_prefabs()? {
            let text = self.read_string(path.file_path())?;
            let prefab = ecs::scene::Prefab::from_json(&text).map_err(|e| {
                ProjectError::Manifest(format!("invalid prefab {}: {e}", path.as_str()))
            })?;
            prefabs.push(prefab);
        }
        Ok(prefabs)
    }

    /// Walk `dir` (recursively) and collect files whose name ends with `suffix`,
    /// as sorted project-relative asset paths. A missing directory yields none.
    fn discover(&self, dir: &str, suffix: &str) -> Result<Vec<AssetPath>, ProjectError> {
        let mut files = Vec::new();
        self.walk(dir, &mut files)?;
        let mut paths: Vec<AssetPath> = files
            .into_iter()
            .filter(|relative| relative.to_ascii_lowercase().ends_with(suffix))
            .filter_map(|relative| AssetPath::parse(relative).ok())
            .collect();
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

/// The display name for a scene path: its file name without `.scene.json`.
fn scene_name(path: &AssetPath) -> String {
    let file = path.file_name();
    file.strip_suffix(".scene.json").unwrap_or(file).to_string()
}

/// Recursively collect the files under `dir` (a store-relative directory) as
/// `/`-joined store-relative paths. A missing directory yields none.
pub(crate) fn walk_files(
    store: &DirStore,
    dir: &str,
    out: &mut Vec<String>,
) -> Result<(), ProjectError> {
    let entries = match store.list(dir) {
        Ok(entries) => entries,
        Err(StoreError::NotFound(_)) => return Ok(()),
        Err(error) => return Err(ProjectError::Io(error.to_string())),
    };
    for entry in entries {
        let path = if dir.is_empty() {
            entry.name.clone()
        } else {
            format!("{dir}/{}", entry.name)
        };
        if entry.is_dir {
            walk_files(store, &path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenes_lists_explicit_entries_first_with_names() {
        let manifest = ProjectManifest::from_toml(
            "[project]\nname = \"S\"\n\n[[scene]]\npath = \"scenes/menu.scene.json\"\nname = \"Main Menu\"\n\n[[scene]]\npath = \"scenes/level.scene.json\"\n",
        )
        .unwrap();
        let project = Project::from_manifest("/nonexistent-root", manifest);
        let scenes = project.scenes().unwrap();
        assert_eq!(scenes.len(), 2);
        assert_eq!(scenes[0].name, "Main Menu");
        assert_eq!(scenes[0].path.as_str(), "scenes/menu.scene.json");
        // A missing name defaults to the file stem without `.scene.json`.
        assert_eq!(scenes[1].name, "level");
    }

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

    #[test]
    fn a_root_override_relocates_the_project() {
        let dir = std::env::temp_dir().join(format!("noctis-root-{}", std::process::id()));
        let nested = dir.join("dist/dev");
        std::fs::create_dir_all(&nested).expect("dirs");
        // An absolute root, as a development build writes.
        let root = dir.to_string_lossy().replace('\\', "/");
        std::fs::write(
            nested.join(MANIFEST_FILE),
            format!(
                "[project]\nname = \"G\"\nroot = {root:?}\n\n[project.settings]\ndefault_scene = \"scenes/main.scene.json\"\n"
            ),
        )
        .expect("manifest");

        let project = Project::open(&nested).expect("open");
        // The files live at the declared root, not beside the manifest.
        assert_eq!(project.root(), Path::new(&root));
        assert_eq!(
            project.default_scene_file(),
            Some(Path::new(&root).join("scenes/main.scene.json"))
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rust_settings_resolve_the_crate_and_plugin_path() {
        let dir = std::env::temp_dir().join(format!("noctis-rust-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("game")).expect("dirs");
        std::fs::write(
            dir.join("game/Cargo.toml"),
            "[package]\nname = \"my-game\"\nversion = \"0.1.0\"\n",
        )
        .expect("crate manifest");

        // An explicit plugin path is used verbatim.
        std::fs::write(
            dir.join(MANIFEST_FILE),
            "[project]\nname = \"T\"\n\n[rust]\ncrate = \"game\"\nplugin = \"my_game::game\"\n",
        )
        .expect("manifest");
        let project = Project::open(&dir).expect("open");
        assert_eq!(project.rust_crate_dir(), Some(dir.join("game")));
        assert_eq!(
            project.rust_package_name().unwrap().as_deref(),
            Some("my-game")
        );
        assert_eq!(
            project.plugin_path().unwrap().as_deref(),
            Some("my_game::game")
        );

        // Absent, it defaults to `<package name>::plugin`, with `-` normalized.
        std::fs::write(
            dir.join("default.toml"),
            "[project]\nname = \"T\"\n\n[rust]\ncrate = \"game\"\n",
        )
        .expect("manifest");
        let project = Project::from_manifest_file(dir.join("default.toml")).expect("open");
        assert_eq!(
            project.plugin_path().unwrap().as_deref(),
            Some("my_game::plugin")
        );

        // No [rust] table at all.
        let none = Project::from_manifest(
            "/x",
            ProjectManifest::from_toml("[project]\nname = \"T\"\n").unwrap(),
        );
        assert_eq!(none.rust_crate_dir(), None);
        assert_eq!(none.rust_package_name().unwrap(), None);
        assert_eq!(none.plugin_path().unwrap(), None);

        // A crate directory with no Cargo.toml is an error, not a silent default.
        std::fs::write(
            dir.join("missing.toml"),
            "[project]\nname = \"T\"\n\n[rust]\ncrate = \"absent\"\n",
        )
        .expect("manifest");
        let missing = Project::from_manifest_file(dir.join("missing.toml")).expect("open");
        assert!(matches!(missing.plugin_path(), Err(ProjectError::Io(_))));

        std::fs::remove_dir_all(&dir).ok();
    }
}
