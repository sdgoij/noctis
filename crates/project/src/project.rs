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

/// An open Noctis project: a root directory, the store its files are read and
/// written through, and its parsed manifest.
///
/// The store is a native directory for [`Project::open`], or any [`Store`] for
/// [`Project::open_in`] (a `.zip` bundle, browser storage). The `root` is the
/// logical base for the path-returning helpers ([`Project::dir`],
/// [`Project::file_for`], ...); on the web those paths name nothing, and the
/// store is what is read.
#[derive(Debug)]
pub struct Project {
    root: PathBuf,
    store: Box<dyn Store>,
    manifest: ProjectManifest,
}

impl Clone for Project {
    fn clone(&self) -> Self {
        Self {
            root: self.root.clone(),
            store: self.store.duplicate(),
            manifest: self.manifest.clone(),
        }
    }
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
        let store: Box<dyn Store> = Box::new(DirStore::new(manifest_dir));
        let text = store
            .read_string(name)
            .map_err(|error| ProjectError::Io(error.to_string()))?;
        Self::from_manifest_text(store, manifest_dir, &text)
    }

    /// Open a project stored in `store`, with logical root `root`.
    ///
    /// The manifest is read from the store's root as `noctis.toml`. This is the
    /// browser counterpart of [`Project::open`]: the store can be a project
    /// bundle (`ZipStore`) or browser storage, and `root` is only the base for the
    /// path-returning helpers.
    pub fn open_in(store: Box<dyn Store>, root: impl Into<PathBuf>) -> Result<Self, ProjectError> {
        let root = root.into();
        let text = store
            .read_string(MANIFEST_FILE)
            .map_err(|error| ProjectError::Io(format!("{MANIFEST_FILE}: {error}")))?;
        Self::from_manifest_text(store, &root, &text)
    }

    /// Build a project from already-read manifest `text`, with `store` and a
    /// `base` for the root. A `[project] root` relocates the root, so a
    /// development build's manifest can sit beside its client while the files
    /// live in the project it was built from.
    fn from_manifest_text(
        store: Box<dyn Store>,
        base: &Path,
        text: &str,
    ) -> Result<Self, ProjectError> {
        let manifest = ProjectManifest::from_toml(text)?;
        manifest.validate()?;
        let root = match manifest.project.root.as_deref() {
            Some(root) => {
                let root = Path::new(root);
                if root.is_absolute() {
                    root.to_path_buf()
                } else {
                    base.join(root)
                }
            }
            None => base.to_path_buf(),
        };
        Ok(Self {
            root,
            store,
            manifest,
        })
    }

    /// Build a project from an already-parsed manifest rooted at `dir`.
    pub fn from_manifest(root: impl Into<PathBuf>, manifest: ProjectManifest) -> Self {
        let root = root.into();
        let store: Box<dyn Store> = Box::new(DirStore::new(root.clone()));
        Self {
            root,
            store,
            manifest,
        }
    }

    /// The store every project file is read and written through (see
    /// `.notes/web.md`).
    pub fn store(&self) -> &dyn Store {
        self.store.as_ref()
    }

    /// The store, mutably, for this project's writes.
    pub fn store_mut(&mut self) -> &mut dyn Store {
        self.store.as_mut()
    }

    /// Read a project-relative file as UTF-8 text.
    fn read_string(&self, relative: &str) -> Result<String, ProjectError> {
        self.store()
            .read_string(relative)
            .map_err(|error| ProjectError::Io(format!("{relative}: {error}")))
    }

    /// Read a project-relative document (a scene or prefab) as UTF-8 text, or
    /// `Ok(None)` when it does not exist.
    ///
    /// This is how a caller loads a scene through the project's store — a
    /// directory, or a bundle's zip — rather than a native path.
    pub fn read_document(&self, relative: &str) -> Result<Option<String>, ProjectError> {
        match self.store().read_string(relative) {
            Ok(text) => Ok(Some(text)),
            Err(StoreError::NotFound(_)) => Ok(None),
            Err(error) => Err(ProjectError::Io(format!("{relative}: {error}"))),
        }
    }

    /// Write a project-relative file, creating parent directories.
    fn write_file(&mut self, relative: &str, bytes: &[u8]) -> Result<(), ProjectError> {
        self.store_mut()
            .write(relative, bytes)
            .map_err(|error| ProjectError::Io(format!("{relative}: {error}")))
    }

    /// The files under a project-relative directory, as `/`-joined relative paths.
    pub(crate) fn walk(&self, dir: &str, out: &mut Vec<String>) -> Result<(), ProjectError> {
        walk_files(self.store(), dir, out)
    }

    /// The project root's `Cargo.toml` text, or `None` when the project carries no
    /// Rust.
    ///
    /// A `Cargo.toml` at the project root is what makes a project a *Rust*
    /// project; its `[package].name` names the crate the generated tool and client
    /// link.
    fn cargo_toml(&self) -> Result<Option<String>, ProjectError> {
        self.read_document("Cargo.toml")
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

    /// Whether the project carries Rust: its root holds a `Cargo.toml`.
    pub fn carries_rust(&self) -> bool {
        self.cargo_toml().ok().flatten().is_some()
    }

    /// The plugin crate, relative to the project root, when the project carries
    /// Rust.
    ///
    /// A root `Cargo.toml` is what makes a project carry Rust. If it is a
    /// *package*, the root is the plugin crate (`""`). If it is a *virtual
    /// workspace*, the plugin is its first `default-members` (or, failing that,
    /// its first `members`) — so a project can keep its simulation in sibling
    /// crates and let a member be the plugin (`.notes/embedding.md`).
    fn rust_crate_relative(&self) -> Result<Option<String>, ProjectError> {
        let Some(text) = self.cargo_toml()? else {
            return Ok(None);
        };
        let value: toml::Value =
            toml::from_str(&text).map_err(|error| ProjectError::Manifest(error.to_string()))?;
        if value.get("package").is_some() {
            return Ok(Some(String::new()));
        }
        let member = value
            .get("workspace")
            .and_then(|workspace| {
                workspace
                    .get("default-members")
                    .or_else(|| workspace.get("members"))
            })
            .and_then(toml::Value::as_array)
            .and_then(|members| members.first())
            .and_then(toml::Value::as_str);
        let Some(member) = member else {
            return Err(ProjectError::Manifest(
                "Cargo.toml: a workspace project must name its plugin crate in [workspace] \
                 default-members"
                    .to_string(),
            ));
        };
        if member.contains(['*', '?', '[', ']']) {
            return Err(ProjectError::Manifest(format!(
                "Cargo.toml: the plugin crate {member:?} must be a path, not a glob"
            )));
        }
        Ok(Some(member.trim_matches('/').replace('\\', "/")))
    }

    /// The plugin crate's directory, when the project carries Rust: the project
    /// root, or the workspace member that is the plugin.
    pub fn rust_crate_dir(&self) -> Option<PathBuf> {
        let relative = self.rust_crate_relative().ok().flatten()?;
        if relative.is_empty() {
            Some(self.root.clone())
        } else {
            Some(self.root.join(relative))
        }
    }

    /// The plugin crate's `[package].name`, when the project carries Rust.
    ///
    /// A workspace member with no `[package]` (a nested virtual workspace) is an
    /// error: the plugin must be a library crate.
    pub fn rust_package_name(&self) -> Result<Option<String>, ProjectError> {
        let Some(relative) = self.rust_crate_relative()? else {
            return Ok(None);
        };
        let manifest = if relative.is_empty() {
            "Cargo.toml".to_string()
        } else {
            format!("{relative}/Cargo.toml")
        };
        let Some(text) = self.read_document(&manifest)? else {
            return Err(ProjectError::Manifest(format!("{manifest}: not found")));
        };
        let value: toml::Value =
            toml::from_str(&text).map_err(|error| ProjectError::Manifest(error.to_string()))?;
        let name = value
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(|name| name.as_str());
        match name {
            Some(name) => Ok(Some(name.to_string())),
            None => Err(ProjectError::Manifest(format!(
                "{manifest}: missing [package].name"
            ))),
        }
    }

    /// The plugin crate's Rust path, `<package>::noctis_plugin`.
    ///
    /// `#[noctis::plugin]` exports the `noctis_plugin` symbol; the crate name is
    /// the plugin crate's `[package].name` (the project root, or its workspace
    /// default member), with `-` normalized to `_`. `None` when the project has
    /// no root `Cargo.toml`.
    pub fn plugin_path(&self) -> Result<Option<String>, ProjectError> {
        let Some(package) = self.rust_package_name()? else {
            return Ok(None);
        };
        Ok(Some(format!(
            "{}::noctis_plugin",
            package.replace('-', "_")
        )))
    }

    /// The project's extension search directories, absolute (see `.notes/extensions.md`).
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
    pub fn save_prefab(&mut self, prefab: &ecs::scene::Prefab) -> Result<AssetPath, ProjectError> {
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
    store: &dyn Store,
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
    fn a_project_opens_from_a_store() {
        use vfs::MemoryStore;

        let mut store = MemoryStore::new();
        store
            .write("noctis.toml", b"[project]\nname = \"Bundle\"\n")
            .unwrap();
        store
            .write("scenes/level.scene.json", b"{ \"entities\": [] }")
            .unwrap();
        store.write("scenes/notes.txt", b"not a scene").unwrap();

        // The store is the source of truth; `root` is only a path base.
        let project = Project::open_in(Box::new(store), "/virtual/root").expect("open from store");
        assert_eq!(project.name(), "Bundle");
        assert_eq!(project.root(), Path::new("/virtual/root"));

        let scenes: Vec<String> = project
            .scenes()
            .expect("scenes")
            .into_iter()
            .map(|scene| scene.path.as_str().to_string())
            .collect();
        assert_eq!(scenes, vec!["scenes/level.scene.json"]);
        assert!(project.discover_prefabs().expect("prefabs").is_empty());
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
    fn a_root_cargo_toml_makes_the_project_carry_rust() {
        let dir = std::env::temp_dir().join(format!("noctis-rust-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dirs");
        std::fs::write(dir.join(MANIFEST_FILE), "[project]\nname = \"T\"\n").expect("manifest");

        // No Cargo.toml: the project carries no Rust.
        let project = Project::open(&dir).expect("open");
        assert_eq!(project.rust_crate_dir(), None);
        assert_eq!(project.rust_package_name().unwrap(), None);
        assert_eq!(project.plugin_path().unwrap(), None);

        // A root Cargo.toml is the signal: the crate is the plugin, its package
        // name names it, and the plugin path is the convention (`-` normalized).
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"my-game\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("crate manifest");
        let project = Project::open(&dir).expect("open");
        assert_eq!(project.rust_crate_dir(), Some(dir.clone()));
        assert_eq!(
            project.rust_package_name().unwrap().as_deref(),
            Some("my-game")
        );
        assert_eq!(
            project.plugin_path().unwrap().as_deref(),
            Some("my_game::noctis_plugin")
        );

        // A virtual workspace root names its plugin crate as the first default
        // member, so the simulation can live in sibling crates.
        std::fs::create_dir_all(dir.join("crates/plugin")).expect("plugin dir");
        std::fs::create_dir_all(dir.join("crates/sim")).expect("sim dir");
        std::fs::write(
            dir.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/sim\", \"crates/plugin\"]\n\
             default-members = [\"crates/plugin\"]\n",
        )
        .expect("workspace manifest");
        std::fs::write(
            dir.join("crates/plugin/Cargo.toml"),
            "[package]\nname = \"my-game\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("plugin manifest");
        let project = Project::open(&dir).expect("open");
        assert!(project.carries_rust());
        assert_eq!(project.rust_crate_dir(), Some(dir.join("crates/plugin")));
        assert_eq!(
            project.rust_package_name().unwrap().as_deref(),
            Some("my-game")
        );
        assert_eq!(
            project.plugin_path().unwrap().as_deref(),
            Some("my_game::noctis_plugin")
        );

        // With no `default-members`, the first `members` entry is the plugin.
        std::fs::write(
            dir.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/plugin\", \"crates/sim\"]\n",
        )
        .expect("workspace manifest");
        let project = Project::open(&dir).expect("open");
        assert_eq!(project.rust_crate_dir(), Some(dir.join("crates/plugin")));

        // A workspace that names no member is an error, not a silent default.
        std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = []\n")
            .expect("workspace manifest");
        let project = Project::open(&dir).expect("open");
        assert!(project.carries_rust());
        assert_eq!(project.rust_crate_dir(), None);
        assert!(matches!(
            project.rust_package_name(),
            Err(ProjectError::Manifest(_))
        ));

        std::fs::remove_dir_all(&dir).ok();
    }
}
