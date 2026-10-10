//! Build a distributable game folder.
//!
//! `noctis run --export <dir> --scene <level.json>` writes a self-contained
//! folder containing:
//!
//! * a copy of the running `noctis-game` executable, renamed to the game,
//! * the scene as [`BUNDLED_SCENE`],
//! * a [`MANIFEST`] naming both.
//!
//! The exported executable loads [`BUNDLED_SCENE`] from its own directory when it
//! is run with no arguments (see [`bundled_scene`] and the binary's
//! `build_runtime`), so the folder is self-contained and double-clickable.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use project::Profile;

/// The scene file name inside an exported folder.
pub const BUNDLED_SCENE: &str = "scene.json";

/// The manifest file name inside an exported folder.
pub const MANIFEST: &str = "noctis-game.json";

/// The project manifest an exported folder may carry so the runtime can import
/// its bundled assets (see [`BUNDLED_SCENE`]).
pub const BUNDLED_PROJECT: &str = "noctis.toml";

/// The runtime executable's base name (the game an export ships).
pub const RUNTIME_BIN: &str = "noctis-game";

/// A project file to place inside an exported folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportFile {
    /// The destination path relative to the game folder, with `/` separators
    /// (a project-relative asset path).
    pub relative: String,
    /// The source file on disk.
    pub source: PathBuf,
}

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
    /// The bundled project manifest, when assets were copied.
    pub project_manifest: Option<PathBuf>,
    /// The copied asset files, relative to the game folder.
    pub files: Vec<String>,
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
    /// An asset file's relative path is not a safe, portable path.
    InvalidPath(String),
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
            ExportError::InvalidPath(path) => {
                write!(
                    f,
                    "invalid asset path {path:?}: use a portable relative path"
                )
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

/// `true` if `relative` is a safe, portable relative path (POSIX separators,
/// no empty/`.`/`..` segments, no drive prefix).
pub fn valid_relative_path(relative: &str) -> bool {
    !relative.is_empty()
        && !relative.contains(['\\', ':'])
        && !relative.chars().any(char::is_control)
        && relative
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

/// The scene bundled next to an executable, if one is present.
pub fn bundled_scene(directory: &Path) -> Option<PathBuf> {
    let candidate = directory.join(BUNDLED_SCENE);
    candidate.is_file().then_some(candidate)
}

/// The executable an exported game should ship.
///
/// This is the `noctis-game` binary when one sits beside the running process (so
/// `noctis build` ships the stripped game runtime, not the `noctis` tool itself),
/// else the running executable (which is the game when it exports itself).
pub fn default_runtime_executable() -> std::io::Result<PathBuf> {
    let current = std::env::current_exe()?;
    let Some(dir) = current.parent() else {
        return Ok(current);
    };
    let sibling = dir.join(executable_name(RUNTIME_BIN));
    Ok(if sibling.is_file() { sibling } else { current })
}

/// The project manifest bundled next to an executable, if one is present.
pub fn bundled_project(directory: &Path) -> Option<PathBuf> {
    let candidate = directory.join(BUNDLED_PROJECT);
    candidate.is_file().then_some(candidate)
}

/// The environment variable that names the embed pack `crates/client/build.rs`
/// bakes into the client.
pub const EMBED_PACK_ENV: &str = "NOCTIS_EMBED_PACK";

/// The game client an export should ship, building it first if needed.
///
/// `profile` selects the cargo profile ([`Profile::Release`] builds with
/// `--release`, which is where scripts are embedded); `pack` names the embed pack
/// to bake in (see [`EMBED_PACK_ENV`]), or `None` for a development client that
/// loads scripts from disk. An export ships [`RUNTIME_BIN`] (`noctis-game`),
/// which lives in `crates/client`; this builds it from the workspace — a no-op
/// when it is already up to date — and returns its path. When the workspace is
/// unavailable it falls back to a client beside the running executable, and
/// fails with the manual command otherwise.
pub fn game_client(profile: Profile, pack: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(root) = workspace_root() {
        let manifest = root.join("crates/client/Cargo.toml");
        if manifest.is_file() {
            log::info!("building the game client (cargo build --bin {RUNTIME_BIN})");
            let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
            let mut command = Command::new(cargo);
            command
                .arg("build")
                .arg("--manifest-path")
                .arg(&manifest)
                .arg("--bin")
                .arg(RUNTIME_BIN);
            if profile == Profile::Release {
                command.arg("--release");
            }
            if let Some(pack) = pack {
                command.env(EMBED_PACK_ENV, pack);
            }
            match command.status() {
                Ok(status) if status.success() => {}
                Ok(status) => return Err(format!("building the game client failed ({status})")),
                Err(error) => {
                    log::warn!("could not run cargo to build the game client: {error}");
                }
            }
            let client = target_dir(&root)
                .join(profile_dir(profile))
                .join(executable_name(RUNTIME_BIN));
            if client.is_file() {
                return Ok(client);
            }
            log::warn!("the built client was not found at {}", client.display());
        }
    }

    let client = default_runtime_executable().map_err(|error| error.to_string())?;
    if client.file_stem().and_then(|stem| stem.to_str()) != Some(RUNTIME_BIN) {
        return Err(format!(
            "could not find or build the game client ({RUNTIME_BIN}); \
             build it with `cargo build --workspace`"
        ));
    }
    Ok(client)
}

/// The workspace `target` directory, honouring `CARGO_TARGET_DIR`.
fn target_dir(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"))
}

/// The cargo profile subdirectory a build lands in.
pub(crate) fn profile_dir(profile: Profile) -> &'static str {
    match profile {
        Profile::Dev => "debug",
        Profile::Release => "release",
    }
}

/// The workspace root, derived from this crate's manifest directory.
pub(crate) fn workspace_root() -> Option<PathBuf> {
    workspace_root_from(Path::new(env!("CARGO_MANIFEST_DIR")))
}

/// The workspace root for a crate at `manifest_dir` (its parent's parent), when
/// that directory carries a `Cargo.toml`.
fn workspace_root_from(manifest_dir: &Path) -> Option<PathBuf> {
    let root = manifest_dir.parent()?.parent()?;
    root.join("Cargo.toml")
        .is_file()
        .then(|| root.to_path_buf())
}

/// Export a self-contained game folder.
///
/// Copies `runtime` (or, when `None`, [`default_runtime_executable`]) as the game
/// executable, writes `scene_json` as [`BUNDLED_SCENE`], copies each of `files`
/// into the folder (preserving its relative path), and writes a [`MANIFEST`].
/// When any file is bundled, a [`BUNDLED_PROJECT`] manifest is written too, so the
/// exported runtime imports the bundled assets and keeps its settings — including
/// `environment`, the project-relative `.hdr` the game's IBL is precomputed from.
/// Re-exporting over an existing folder overwrites the executable, scene,
/// manifest, and files.
pub fn export_game(
    destination: &Path,
    name: &str,
    scene_json: &str,
    files: &[ExportFile],
    environment: Option<&str>,
    runtime: Option<&Path>,
) -> Result<ExportReport, ExportError> {
    if !valid_name(name) {
        return Err(ExportError::InvalidName(name.to_string()));
    }
    for file in files {
        if !valid_relative_path(&file.relative) {
            return Err(ExportError::InvalidPath(file.relative.clone()));
        }
    }
    fs::create_dir_all(destination).map_err(io)?;

    let executable = destination.join(executable_name(name));
    let source = match runtime {
        Some(path) => path.to_path_buf(),
        None => default_runtime_executable().map_err(io)?,
    };
    // Copying a running executable onto itself would fail; that only happens when
    // exporting into the executable's own directory, which needs no copy.
    if source != executable {
        fs::copy(&source, &executable).map_err(io)?;
    }

    let scene = destination.join(BUNDLED_SCENE);
    fs::write(&scene, scene_json).map_err(io)?;

    // Bundle the referenced project files, preserving their relative paths.
    let mut copied = Vec::with_capacity(files.len());
    for file in files {
        let target = file
            .relative
            .split('/')
            .fold(destination.to_path_buf(), |path, segment| {
                path.join(segment)
            });
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(io)?;
        }
        fs::copy(&file.source, &target).map_err(io)?;
        copied.push(file.relative.clone());
    }

    // A project manifest lets the exported runtime import the bundled assets and
    // keep the settings a scene cannot carry (the IBL environment).
    let project_manifest = if copied.is_empty() {
        None
    } else {
        let path = destination.join(BUNDLED_PROJECT);
        fs::write(&path, bundled_project_toml(name, environment)).map_err(io)?;
        Some(path)
    };

    let manifest = destination.join(MANIFEST);
    let document = serde_json::json!({
        "name": name,
        "engine": concat!("noctis ", env!("CARGO_PKG_VERSION")),
        "executable": executable_name(name),
        "scene": BUNDLED_SCENE,
        "environment": environment,
        "assets": copied,
    });
    let text =
        serde_json::to_string_pretty(&document).map_err(|e| ExportError::Json(e.to_string()))?;
    fs::write(&manifest, text).map_err(io)?;

    Ok(ExportReport {
        directory: destination.to_path_buf(),
        executable,
        scene,
        manifest,
        project_manifest,
        files: copied,
    })
}

/// Export a development game folder: the client plus a manifest that runs the
/// project it was built from *in place*, so the live scenes, assets, and scripts
/// are used and script edits are picked up (the runtime watches them). Nothing is
/// copied.
///
/// `project_root` is the project directory the build was made from, `scene` the
/// project-relative default scene, and `environment` the `[render] environment`.
pub fn export_dev_game(
    destination: &Path,
    name: &str,
    project_root: &Path,
    scene: &str,
    environment: Option<&str>,
    runtime: Option<&Path>,
) -> Result<ExportReport, ExportError> {
    if !valid_name(name) {
        return Err(ExportError::InvalidName(name.to_string()));
    }
    // The manifest lives in the game folder but must point at the project, so the
    // root has to be absolute (a relative one would resolve against the folder).
    let project_root = absolute(project_root);
    let project_root = project_root.as_path();
    fs::create_dir_all(destination).map_err(io)?;

    let executable = destination.join(executable_name(name));
    let source = match runtime {
        Some(path) => path.to_path_buf(),
        None => default_runtime_executable().map_err(io)?,
    };
    if source != executable {
        fs::copy(&source, &executable).map_err(io)?;
    }

    let project_manifest = destination.join(BUNDLED_PROJECT);
    fs::write(
        &project_manifest,
        dev_project_toml(name, project_root, scene, environment),
    )
    .map_err(io)?;

    let manifest = destination.join(MANIFEST);
    let document = serde_json::json!({
        "name": name,
        "engine": concat!("noctis ", env!("CARGO_PKG_VERSION")),
        "executable": executable_name(name),
        "scene": scene,
        "environment": environment,
        "project": project_root.to_string_lossy(),
        "assets": [],
    });
    let text =
        serde_json::to_string_pretty(&document).map_err(|e| ExportError::Json(e.to_string()))?;
    fs::write(&manifest, text).map_err(io)?;

    Ok(ExportReport {
        directory: destination.to_path_buf(),
        executable,
        scene: project_root.join(scene),
        manifest,
        project_manifest: Some(project_manifest),
        files: Vec::new(),
    })
}

/// Make `path` absolute against the current directory.
///
/// This deliberately avoids `fs::canonicalize`, whose Windows `\\?\` verbatim
/// prefix `include_bytes!` does not accept.
pub(crate) fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path),
        Err(_) => path.to_path_buf(),
    }
}

/// The `noctis.toml` a development game folder carries: a project that runs the
/// project it was built from, in place, via `[project] root`.
fn dev_project_toml(name: &str, root: &Path, scene: &str, environment: Option<&str>) -> String {
    // Forward slashes keep the manifest readable and are valid TOML and Windows
    // paths alike.
    let root = root.to_string_lossy().replace('\\', "/");
    let mut toml = format!(
        "[project]\nname = {name:?}\nroot = {root:?}\n\n[project.settings]\ndefault_scene = {scene:?}\n"
    );
    if let Some(environment) = environment {
        toml.push_str(&format!("\n[render]\nenvironment = {environment:?}\n"));
    }
    toml
}

/// The `noctis.toml` an exported folder carries: a project that runs its bundled
/// scene and imports its bundled assets.
fn bundled_project_toml(name: &str, environment: Option<&str>) -> String {
    // `name` is already validated as a single-component file name, but TOML
    // string escaping is not needed for the `[project].name` we write.
    let mut toml = format!(
        "[project]\nname = {name:?}\n\n[project.settings]\ndefault_scene = {scene:?}\n",
        scene = BUNDLED_SCENE
    );
    if let Some(environment) = environment {
        toml.push_str(&format!("\n[render]\nenvironment = {environment:?}\n"));
    }
    toml
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
        let report = export_game(&dir, "my-game", scene_json, &[], None, None).expect("export");

        assert!(report.executable.is_file());
        assert_eq!(
            fs::read_to_string(&report.scene).expect("read scene"),
            scene_json
        );
        assert!(report.project_manifest.is_none(), "no assets, no manifest");

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
        let error = export_game(&dir, "../evil", "{}", &[], None, None).expect_err("must reject");
        assert!(matches!(error, ExportError::InvalidName(_)));
        assert!(!dir.exists());
    }

    #[test]
    fn export_bundles_files_and_writes_a_project_manifest() {
        let dir = temp_dir("bundle");
        let source = temp_dir("bundle-src");
        fs::create_dir_all(source.join("assets/textures")).expect("mkdir");
        fs::write(source.join("assets/textures/wood.png"), b"PNG").expect("write asset");
        let file = ExportFile {
            relative: "assets/textures/wood.png".to_string(),
            source: source.join("assets/textures/wood.png"),
        };

        let report = export_game(
            &dir,
            "my-game",
            "{}",
            std::slice::from_ref(&file),
            None,
            None,
        )
        .expect("export");
        assert_eq!(report.files, vec!["assets/textures/wood.png".to_string()]);
        assert_eq!(
            fs::read(dir.join("assets/textures/wood.png")).expect("bundled asset"),
            b"PNG"
        );

        // The bundle carries a project manifest naming its bundled scene.
        let manifest = report.project_manifest.expect("project manifest");
        let text = fs::read_to_string(&manifest).expect("read manifest");
        assert!(text.contains("default_scene = \"scene.json\""), "{text}");
        assert_eq!(bundled_project(&dir), Some(manifest));

        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&source).ok();
    }

    #[test]
    fn an_export_keeps_the_environment_it_was_built_with() {
        let dir = temp_dir("env");
        let source = temp_dir("env-src");
        fs::create_dir_all(source.join("assets/hdris")).expect("mkdir");
        fs::write(source.join("assets/hdris/sky.hdr"), b"HDR").expect("write env");
        let file = ExportFile {
            relative: "assets/hdris/sky.hdr".to_string(),
            source: source.join("assets/hdris/sky.hdr"),
        };

        let report = export_game(
            &dir,
            "my-game",
            "{}",
            std::slice::from_ref(&file),
            Some("assets/hdris/sky.hdr"),
            None,
        )
        .expect("export");

        // The bundled manifest names the environment, so the exported game
        // precomputes the same image-based lighting.
        let text = fs::read_to_string(report.project_manifest.expect("manifest")).expect("read");
        assert!(text.contains("[render]"), "{text}");
        assert!(
            text.contains("environment = \"assets/hdris/sky.hdr\""),
            "{text}"
        );

        let game: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&report.manifest).expect("read"))
                .expect("parse");
        assert_eq!(game["environment"], "assets/hdris/sky.hdr");

        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&source).ok();
    }

    #[test]
    fn export_rejects_an_escaping_relative_path() {
        let dir = temp_dir("escape");
        let file = ExportFile {
            relative: "../evil".to_string(),
            source: PathBuf::from("whatever"),
        };
        let error =
            export_game(&dir, "my-game", "{}", &[file], None, None).expect_err("must reject");
        assert!(matches!(error, ExportError::InvalidPath(_)));
        assert!(!valid_relative_path("../evil"));
        assert!(valid_relative_path("assets/models/a.glb"));
        assert!(!valid_relative_path("/abs"));
        assert!(!valid_relative_path("a//b"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bundled_scene_is_none_without_a_scene_file() {
        let dir = temp_dir("empty");
        fs::create_dir_all(&dir).expect("mkdir");
        assert_eq!(bundled_scene(&dir), None);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn workspace_root_walks_up_two_directories_to_a_manifest() {
        let root = temp_dir("workspace");
        fs::create_dir_all(root.join("crates/runtime")).expect("mkdir");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write");

        assert_eq!(
            workspace_root_from(&root.join("crates/runtime")),
            Some(root.clone())
        );
        // Two levels up from here is `crates`, with no Cargo.toml of its own.
        assert_eq!(workspace_root_from(&root.join("crates/runtime/deep")), None);

        fs::remove_dir_all(&root).ok();
    }
}
