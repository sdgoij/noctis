//! The `noctis run` / `noctis-game` command line.
//!
//! This is the game runner behind both `noctis run` (development) and the
//! `noctis-game` binary that `noctis build` ships inside an exported folder, so
//! the two behave identically. Argument parsing is hand-rolled (no CLI
//! dependency); every failure becomes a non-zero exit and a message on stderr.
//!
//! A window is the default — the common case is wanting to see the game. Pass
//! `--headless` to run the frame loop and exit instead, and `--export` to write a
//! distributable folder rather than run.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ecs::app::{NullPlugin, Plugin};
use ecs::assets::AssetRegistry;
use ecs::reflect::ComponentRegistry;
use ecs::scene::Scene;
use platform::WindowConfig;
use project::{AssetPath, Profile, Project};
use render::RendererConfig;
use vfs::{DirStore, Store, ZipStore};

use crate::window::run_in_window;
use crate::{
    BuildOptions, EmbeddedScript, ExportReport, RunSummary, Runtime, RuntimeConfig, build_project,
    bundled_scene, demo, export_game,
};

/// The prefix on every diagnostic message.
const PROGRAM: &str = "noctis";

/// Parsed command-line arguments.
#[derive(Clone, Debug)]
struct Args {
    scene: Option<String>,
    project: Option<String>,
    bundle: Option<String>,
    target: Option<String>,
    demo: bool,
    frames: u32,
    /// Whether `--frames` was given, so a windowed run can say it ignores it.
    frames_given: bool,
    dt: f32,
    /// Whether `--dt` was given, so a windowed run can say it ignores it.
    dt_given: bool,
    render: bool,
    headless: bool,
    audio: bool,
    export: Option<String>,
    name: Option<String>,
    wasm: Vec<(String, String)>,
    print_summary: bool,
    quiet: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            scene: None,
            project: None,
            bundle: None,
            target: None,
            demo: false,
            frames: 300,
            frames_given: false,
            dt: 1.0 / 60.0,
            dt_given: false,
            render: false,
            headless: false,
            audio: true,
            export: None,
            name: None,
            wasm: Vec::new(),
            print_summary: false,
            quiet: false,
        }
    }
}

impl Args {
    /// Parse arguments. Returns `Ok(None)` when help was requested.
    fn parse<I: Iterator<Item = String>>(mut it: I) -> Result<Option<Args>, String> {
        let mut args = Args::default();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--help" | "-h" => return Ok(None),
                "--demo" => args.demo = true,
                "--render" => args.render = true,
                "--headless" => args.headless = true,
                // Consumed by the outer `noctis` for a Rust plugin; accepted here
                // so the generated tool's CLI does not reject them.
                "--release" => {}
                "--debug" => {}
                "--window" => {
                    return Err(
                        "--window was removed: a window is now the default, so pass --headless \
                         to run without one"
                            .into(),
                    );
                }
                "--no-audio" => args.audio = false,
                "--print-summary" => args.print_summary = true,
                "--quiet" | "-q" => args.quiet = true,
                "--scene" => {
                    let value = it.next().ok_or("--scene requires a file path")?;
                    args.scene = Some(value);
                }
                "--project" => {
                    let value = it.next().ok_or("--project requires a directory")?;
                    args.project = Some(value);
                }
                "--bundle" => {
                    let value = it.next().ok_or("--bundle requires a .zip path")?;
                    args.bundle = Some(value);
                }
                "--target" => {
                    let value = it.next().ok_or("--target requires a name")?;
                    args.target = Some(value);
                }
                "--export" => {
                    let value = it.next().ok_or("--export requires a directory")?;
                    args.export = Some(value);
                }
                "--name" => {
                    let value = it.next().ok_or("--name requires a value")?;
                    args.name = Some(value);
                }
                "--wasm" => {
                    let value = it.next().ok_or("--wasm requires <name>=<path>")?;
                    let (name, path) = value
                        .split_once('=')
                        .ok_or("--wasm expects <name>=<path>")?;
                    args.wasm.push((name.to_string(), path.to_string()));
                }
                "--frames" => {
                    let value = it.next().ok_or("--frames requires a number")?;
                    args.frames = value
                        .parse()
                        .map_err(|_| format!("invalid --frames value '{value}'"))?;
                    args.frames_given = true;
                }
                "--dt" => {
                    let value = it.next().ok_or("--dt requires a number")?;
                    let dt: f32 = value
                        .parse()
                        .map_err(|_| format!("invalid --dt value '{value}'"))?;
                    if !(dt.is_finite() && dt > 0.0) {
                        return Err(format!("--dt must be a positive number, got '{value}'"));
                    }
                    args.dt = dt;
                    args.dt_given = true;
                }
                other => return Err(format!("unknown argument '{other}'")),
            }
        }
        if args.demo && args.scene.is_some() {
            return Err("--demo and --scene are mutually exclusive".into());
        }
        if args.bundle.is_some() && args.project.is_some() {
            return Err("--bundle and --project are mutually exclusive".into());
        }
        if args.bundle.is_some() && args.export.is_some() {
            return Err("--bundle cannot be exported; pass --project to export".into());
        }
        // No scene is not an error: `load_scene` falls back to a scene bundled
        // next to the executable, then to the built-in demo.
        Ok(Some(args))
    }
}

/// Run the game command line (`noctis run`) with no embedded scripts or plugin.
pub fn run_cli(args: Vec<String>) -> ExitCode {
    run_cli_with_plugin(args, &[], &NullPlugin)
}

/// Run the game command line, seeding `embedded` into the asset registry.
///
/// The `noctis-game` client passes the scripts baked into it; `noctis run` passes
/// an empty slice and loads scripts from disk (and watches them). See
/// `.notes/scripts.md`.
pub fn run_cli_with(args: Vec<String>, embedded: &'static [EmbeddedScript]) -> ExitCode {
    run_cli_with_plugin(args, embedded, &NullPlugin)
}

/// Run the game command line with a project's [`Plugin`].
///
/// The per-project binary a Rust build produces calls this: the plugin's
/// components are registered before the scene loads, and its systems are added to
/// the app.
pub fn run_cli_with_plugin(
    args: Vec<String>,
    embedded: &'static [EmbeddedScript],
    plugin: &dyn Plugin,
) -> ExitCode {
    init_logging();

    let args = match Args::parse(args.into_iter()) {
        Ok(Some(args)) => args,
        Ok(None) => {
            println!("{}", usage());
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("{PROGRAM}: {message}\n");
            eprintln!("{}", usage());
            return ExitCode::from(2);
        }
    };

    // A window is the default; `--export` never opens one.
    if !args.headless && args.export.is_none() {
        return match run_windowed(&args, embedded, plugin) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => fail(&message),
        };
    }

    if !args.headless {
        log::warn!("--frames/--render/--print-summary are ignored without --headless");
    }

    if let Some(destination) = args.export.clone() {
        return match do_export(&args, &destination, plugin) {
            Ok(report) => {
                println!("{report}");
                ExitCode::SUCCESS
            }
            Err(message) => fail(&message),
        };
    }

    match run(&args, embedded, plugin) {
        Ok(summary) => {
            if args.print_summary || !args.quiet {
                println!("{summary}");
            }
            ExitCode::SUCCESS
        }
        Err(message) => fail(&message),
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("{PROGRAM}: {message}");
    ExitCode::from(1)
}

fn usage() -> &'static str {
    "\
noctis run — run a Noctis scene or game

USAGE:
    noctis run [OPTIONS]
    noctis run --headless --frames <N> [OPTIONS]

OPTIONS:
    --scene <path>     Load and run a scene JSON file.
    --project <dir>    Open a project (its noctis.toml); --scene is then a
                       project-relative path and the default scene is used when no
                       --scene is given.
    --bundle <file.zip>
                       Open a packaged project bundle (noctis pack); the default
                       scene inside the archive is run.
    --target <name>    Export the named [[build]] target from the project (its
                       scene, name, and output directory are used).
    --demo             Run a small built-in scene.
    --headless         Run without a window: advance --frames frames and exit.
                       Without this, a window opens and runs until it is closed.
    --release          Build the project's Rust plugin optimized (a project with
                       a Cargo.toml); the default when `noctis` itself is a release
                       build, and a no-op otherwise.
    --debug            Build the project's Rust plugin unoptimized even when
                       `noctis` is a release build; a no-op otherwise.
    --frames <N>       Number of frames to run headlessly (default 300).
    --dt <seconds>     Frame delta time (default 1/60).
    --render           Render frames with a headless GPU (skipped if unavailable).
    --no-audio         Do not open an audio device (use silent audio).
    --export <dir>     Export a self-contained game folder (requires --scene).
    --name <name>      Exported executable name (default: the scene file stem).
    --wasm <name>=<path>
                       Register a .wasm module as a script asset (repeatable).
    --print-summary    Print the run summary (printed by default).
    --quiet            Suppress the run summary.
    -h, --help         Show this help.

ENVIRONMENT:
    NOCTIS_LOG         error | warn | info (default) | debug | trace | off"
}

/// The script host a game runs with: the Slag engine — JavaScript with its
/// Cranelift JIT and WebAssembly JS API, plus native `.wasm` scripts.
type Host = script::engine_host::EngineHost;

/// Build the script host for a run.
fn make_host() -> Result<Host, String> {
    log::info!("JavaScript and WebAssembly scripting enabled (Slag engine)");
    script::engine_host::EngineHost::new()
        .map_err(|err| format!("failed to start the JavaScript engine: {err}"))
}

/// Build the runtime for the requested scene.
fn build_runtime(
    args: &Args,
    project: Option<&Project>,
    embedded: &'static [EmbeddedScript],
    plugin: &dyn Plugin,
) -> Result<Runtime<Host>, String> {
    // The plugin's components must exist before the scene that uses them loads.
    let mut components = ComponentRegistry::new();
    plugin.register(&mut components);
    let mut config = RuntimeConfig {
        audio: args.audio,
        timestep: args.dt,
        ..RuntimeConfig::default()
    };

    // A project owns one shared asset registry: import every asset file so the
    // scene's references resolve. Without a project, start from the engine's
    // placeholder defaults.
    let mut assets = match project {
        Some(project) => {
            let imported = project
                .import_assets()
                .map_err(|error| format!("failed to import project assets: {error}"))?;
            for error in &imported.errors {
                log::warn!(
                    "asset '{}' failed to import: {}",
                    error.path.as_str(),
                    error.message
                );
            }
            log::info!(
                "imported {} asset file(s) from the project ({} failed)",
                imported.files,
                imported.errors.len()
            );
            imported.registry
        }
        None => AssetRegistry::with_builtins(),
    };

    // Gravity and the `[render] environment` (IBL) the manifest names. A named
    // environment that did not import is an error: the game would silently lose
    // its lighting and fall back to the renderer's washed-out white IBL.
    if let Some(project) = project {
        config.apply_project(project, &assets)?;
    }

    register_wasm_modules(args, &mut assets)?;

    // Scripts baked into the client (a release build) take precedence over any
    // loose file, so seed them before the scene resolves its script paths.
    if !embedded.is_empty() {
        let seeded = crate::embed::seed(&mut assets, embedded);
        log::info!("seeded {seeded} embedded script(s)");
    }
    let scene = load_scene(args, project, &mut assets, &components)?;

    let host = make_host()?;

    let mut runtime: Runtime<Host> = Runtime::from_parts(scene, assets, config, host);

    // The plugin's systems are scheduled after the engine's own, so within each
    // stage the engine runs first and the plugin can observe the result.
    plugin.build(runtime.app_mut());

    // A project can switch scenes at runtime: a `load_scene` path is read through
    // the project's store, so a bundled/exported project resolves its scenes too.
    if let Some(project) = project {
        let home = project.default_scene().map(str::to_string);
        let current = args
            .scene
            .clone()
            .or_else(|| project.default_scene().map(str::to_string));
        let project = project.clone();
        runtime.set_scene_loader(Box::new(move |path: &str| {
            let asset = AssetPath::parse(path).map_err(|error| error.to_string())?;
            match project.read_document(asset.file_path()) {
                Ok(Some(json)) => Ok(json),
                Ok(None) => Err(format!("scene {path:?} was not found in the project")),
                Err(error) => Err(error.to_string()),
            }
        }));
        // `Esc` pauses to the project's default scene (the menu) and back.
        if let Some(home) = home {
            runtime.set_home_scene(home);
        }
        if let Some(current) = current {
            runtime.set_current_scene(current);
        }
    }

    if args.render {
        // Anti-alias the headless render at the manifest's sample count.
        let renderer_config = RendererConfig {
            sample_count: project
                .map(|project| project.manifest().render.sample_count)
                .unwrap_or(1),
            ..RendererConfig::default()
        };
        match runtime.enable_renderer(renderer_config) {
            Ok(()) => log::info!("headless rendering enabled"),
            Err(err) => log::warn!("rendering disabled: {err}"),
        }
    }

    Ok(runtime)
}

/// Register every `--wasm name=path` module as a script asset, so a scene can
/// reference it with `Script::from_path(name)`.
fn register_wasm_modules(args: &Args, assets: &mut AssetRegistry) -> Result<(), String> {
    for (name, path) in &args.wasm {
        script::wasm_host::register(assets, name.clone(), path)
            .map_err(|err| format!("failed to register wasm module {path:?}: {err}"))?;
    }
    Ok(())
}

/// Load the scene to run: `--scene`, or the project's default, read through the
/// project's store (a directory, or a bundle's zip); else a loose file resolved
/// against `--project`; else a `scene.json` bundled next to the executable (an
/// exported game); else the built-in demo.
fn load_scene(
    args: &Args,
    project: Option<&Project>,
    assets: &mut AssetRegistry,
    components: &ComponentRegistry,
) -> Result<Scene, String> {
    if args.demo {
        return Ok(demo::demo_scene(assets).with_component_registry(components.clone()));
    }

    // A project owns its scenes: a project-relative path read through the
    // project's store. Anything that is not a canonical asset path (an absolute
    // path, say) falls through to the loose-file branch below.
    if let Some(project) = project {
        let requested = args.scene.as_deref().or_else(|| project.default_scene());
        if let Some(scene) = requested
            && let Ok(asset) = AssetPath::parse(scene)
        {
            return match project.read_document(asset.file_path()) {
                Ok(Some(json)) => Scene::from_json_with(&json, assets, components)
                    .map_err(|err| format!("failed to load scene {scene:?}: {err}")),
                Ok(None) => {
                    log::warn!("scene {scene:?} not found; using the built-in demo scene");
                    Ok(demo::demo_scene(assets).with_component_registry(components.clone()))
                }
                Err(error) => Err(format!("failed to read scene {scene:?}: {error}")),
            };
        }
    }

    // A loose scene: an explicit file (resolved against `--project` when given),
    // the project's default file, or a `scene.json` next to the executable.
    let path = args
        .scene
        .as_deref()
        .map(|scene| match project {
            Some(project) => project.resolve_path(scene),
            None => PathBuf::from(scene),
        })
        .or_else(|| project.and_then(Project::default_scene_file))
        .or_else(bundled_next_to_exe);
    let Some(path) = path else {
        log::info!("no scene specified; using the built-in demo scene");
        return Ok(demo::demo_scene(assets).with_component_registry(components.clone()));
    };
    if !path.is_file() {
        log::warn!(
            "scene '{}' not found; using the built-in demo scene",
            path.display()
        );
        return Ok(demo::demo_scene(assets).with_component_registry(components.clone()));
    }
    let (store, key) = DirStore::for_file(&path);
    let json = store
        .read_string(&key)
        .map_err(|err| format!("failed to read scene '{}': {err}", path.display()))?;
    Scene::from_json_with(&json, assets, components)
        .map_err(|err| format!("failed to load scene '{}': {err}", path.display()))
}

/// A `scene.json` sitting next to the running executable, if any.
fn bundled_next_to_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    bundled_scene(exe.parent()?)
}

/// Export a self-contained game folder for `destination`.
fn do_export(args: &Args, destination: &str, plugin: &dyn Plugin) -> Result<ExportReport, String> {
    // A project build bundles the referenced assets and acquires the client
    // itself (embedding the project's scripts for a release build).
    if let Some(dir) = export_project_dir(args) {
        let options = BuildOptions {
            destination: Some(PathBuf::from(destination)),
            name: args.name.clone(),
            scene: args.scene.clone(),
            ..BuildOptions::default()
        };
        return build_project(Path::new(&dir), args.target.as_deref(), &options, plugin);
    }

    // A loose scene has no project, so no scripts to embed: ship the stripped
    // client built for this tool's own profile.
    let client = crate::export::game_client(tool_profile(), None)?;

    let scene = args
        .scene
        .clone()
        .ok_or("--export requires --scene <path>, or a project with a default scene")?;
    let scene_json = fs::read_to_string(&scene)
        .map_err(|err| format!("failed to read scene '{scene}': {err}"))?;
    let name = args
        .name
        .clone()
        .unwrap_or_else(|| default_game_name(&scene));
    export_game(
        Path::new(destination),
        &name,
        &scene_json,
        &[],
        None,
        Some(&client),
        None,
    )
    .map_err(|err| err.to_string())
}

/// The cargo profile this tool was built with, used for a build that has no
/// project profile to honour (a loose-scene export).
fn tool_profile() -> Profile {
    if cfg!(debug_assertions) {
        Profile::Dev
    } else {
        Profile::Release
    }
}

/// The directory of the project to export from: `--project`, else a bundled one
/// (an exported game re-exporting itself).
fn export_project_dir(args: &Args) -> Option<String> {
    args.project
        .clone()
        .or_else(|| bundled_project_dir().map(|dir| dir.to_string_lossy().into_owned()))
}

/// The default exported game name: the scene file's stem, when it is usable.
fn default_game_name(scene_path: &str) -> String {
    Path::new(scene_path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| crate::export::valid_name(stem))
        .unwrap_or("game")
        .to_string()
}

/// Open the project named by `--project`, if any, else an exported game's
/// bundled project (a `noctis.toml` next to the executable, used when no
/// `--scene` is given).
fn open_project(args: &Args) -> Result<Option<Project>, String> {
    if let Some(bundle) = &args.bundle {
        let bytes = std::fs::read(bundle)
            .map_err(|error| format!("failed to read bundle {bundle:?}: {error}"))?;
        let store = ZipStore::new(&bytes)
            .map_err(|error| format!("{bundle:?} is not a readable bundle: {error}"))?;
        return Project::open_in(Box::new(store), bundle_root(bundle))
            .map(Some)
            .map_err(|error| format!("failed to open bundle {bundle:?}: {error}"));
    }
    if let Some(dir) = &args.project {
        return Project::open(dir)
            .map(Some)
            .map_err(|error| format!("failed to open project {dir:?}: {error}"));
    }
    if args.scene.is_none()
        && let Some(dir) = bundled_project_dir()
    {
        return Project::open(&dir)
            .map(Some)
            .map_err(|error| format!("failed to open the bundled project: {error}"));
    }
    Ok(None)
}

/// A logical project root for a bundle. Only the generated cache is written under
/// it, so keep it out of the user's tree.
fn bundle_root(bundle: &str) -> PathBuf {
    let stem = Path::new(bundle)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("bundle");
    std::env::temp_dir().join(format!("noctis-bundle-{stem}"))
}

/// The directory of a project manifest bundled next to the running executable.
fn bundled_project_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.to_path_buf();
    crate::export::bundled_project(&dir).map(|_| dir)
}

/// The window config: the project's `[window]` when a project is open, else a
/// default titled after the scene. The `[render]` MSAA setting applies either way.
fn window_config(args: &Args, project: Option<&Project>) -> WindowConfig {
    let mut config = WindowConfig {
        title: "Noctis".to_string(),
        width: 1280,
        height: 720,
        ..WindowConfig::default()
    };
    if let Some(project) = project {
        let window = &project.manifest().window;
        config.title = window
            .title
            .clone()
            .unwrap_or_else(|| project.name().to_string());
        config.width = window.width;
        config.height = window.height;
        config.sample_count = project.manifest().render.sample_count;
    } else if let Some(scene) = &args.scene {
        config.title = scene.clone();
    }
    config
}

/// Load the scene and run the loop headlessly. Returns the run summary.
fn run(
    args: &Args,
    embedded: &'static [EmbeddedScript],
    plugin: &dyn Plugin,
) -> Result<RunSummary, String> {
    let project = open_project(args)?;
    let mut runtime = build_runtime(args, project.as_ref(), embedded, plugin)?;
    Ok(runtime.run_headless(args.frames, args.dt))
}

/// Run the scene in a window until it is closed.
fn run_windowed(
    args: &Args,
    embedded: &'static [EmbeddedScript],
    plugin: &dyn Plugin,
) -> Result<(), String> {
    // `--frames`/`--dt` only shape a headless run: a window runs on wall-clock
    // time until it is closed, so say so rather than silently ignoring them.
    if args.frames_given || args.dt_given {
        log::warn!("--frames/--dt only apply to a headless run and are ignored in a window");
    }
    let project = open_project(args)?;
    let mut runtime = build_runtime(args, project.as_ref(), embedded, plugin)?;
    // A development run loads scripts from disk; watch them so an edit reloads
    // on the next frame. An embedded build needs no watching, and a bundle is
    // read-only.
    if embedded.is_empty()
        && args.bundle.is_none()
        && let Some(root) = script_root(project.as_ref())
        && let Err(error) = runtime.watch_scripts(root)
    {
        log::warn!("not watching scripts for changes: {error}");
    }
    let config = window_config(args, project.as_ref());
    run_in_window(runtime, config).map_err(|err| err.to_string())
}

/// The directory a development run watches for script changes: the project root,
/// else the directory the executable runs from (an exported game).
fn script_root(project: Option<&Project>) -> Option<PathBuf> {
    project
        .map(|project| project.root().to_path_buf())
        .or_else(|| Some(std::env::current_exe().ok()?.parent()?.to_path_buf()))
}

/// A minimal logger that writes `log` records to stderr.
struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("[{}] {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

/// Install [`StderrLogger`], honouring the `NOCTIS_LOG` environment variable.
///
/// Idempotent: a second call is a no-op, so `noctis` and `noctis run` can both
/// call it without fighting over the global logger.
pub fn init_logging() {
    static LOGGER: StderrLogger = StderrLogger;
    let level = match std::env::var("NOCTIS_LOG").ok().as_deref() {
        Some("off") => log::LevelFilter::Off,
        Some("error") => log::LevelFilter::Error,
        Some("warn") => log::LevelFilter::Warn,
        Some("debug") => log::LevelFilter::Debug,
        Some("trace") => log::LevelFilter::Trace,
        _ => log::LevelFilter::Info,
    };
    // `set_logger` fails only if a logger is already set; ignore that.
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(level);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Option<Args>, String> {
        Args::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_a_project_and_a_project_relative_scene() {
        let args = parse(&["--project", "game", "--scene", "scenes/main.scene.json"])
            .unwrap()
            .unwrap();
        assert_eq!(args.project.as_deref(), Some("game"));
        assert_eq!(args.scene.as_deref(), Some("scenes/main.scene.json"));
    }

    #[test]
    fn a_project_without_a_scene_is_allowed() {
        let args = parse(&["--project", "game"]).unwrap().unwrap();
        assert!(args.project.is_some());
        assert!(args.scene.is_none());
    }

    #[test]
    fn a_missing_project_value_is_an_error() {
        assert!(parse(&["--project"]).is_err());
    }

    #[test]
    fn a_window_is_the_default_and_headless_opts_out() {
        assert!(!parse(&["--project", "game"]).unwrap().unwrap().headless);
        assert!(parse(&["--headless"]).unwrap().unwrap().headless);
    }

    #[test]
    fn the_removed_window_flag_is_rejected_with_a_hint() {
        let error = parse(&["--window"]).unwrap_err();
        assert!(error.contains("--headless"), "{error}");
    }

    #[test]
    fn frames_and_dt_are_recorded_as_given() {
        let plain = parse(&["--project", "game"]).unwrap().unwrap();
        assert!(!plain.frames_given && !plain.dt_given, "unset by default");

        let given = parse(&["--headless", "--frames", "10", "--dt", "0.02"])
            .unwrap()
            .unwrap();
        assert!(given.frames_given && given.dt_given);
        assert_eq!(given.frames, 10);
        assert!((given.dt - 0.02).abs() < 1e-9);
    }

    #[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
    struct Health {
        current: f32,
        max: f32,
    }

    impl ecs::reflect::Component for Health {
        const NAME: &'static str = "Health";
    }

    /// A plugin that registers `Health`, standing in for a project's Rust.
    struct HealthPlugin;

    impl Plugin for HealthPlugin {
        fn register(&self, components: &mut ComponentRegistry) {
            components.register::<Health>();
        }
    }

    const HEALTH_SCENE: &str = r#"{
        "name": "Custom",
        "entities": [{
            "id": 0,
            "transform": { "translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] },
            "components": [
                { "type": "custom", "name": "Health", "data": { "current": 3.0, "max": 10.0 } }
            ]
        }]
    }"#;

    /// Write `HEALTH_SCENE` to a fresh temp file and return `--scene <path>` args.
    fn health_scene_args() -> (Vec<String>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "noctis-plugin-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let scene = dir.join("scene.json");
        std::fs::write(&scene, HEALTH_SCENE).expect("write scene");
        (
            vec!["--scene".into(), scene.to_string_lossy().into_owned()],
            dir,
        )
    }

    #[test]
    fn a_plugin_registers_its_components_before_the_scene_loads() {
        let (argv, dir) = health_scene_args();
        let args = Args::parse(argv.into_iter()).unwrap().unwrap();

        let runtime = build_runtime(&args, None, &[], &HealthPlugin).expect("plugin loads");
        assert_eq!(runtime.scene().world.entities_with::<Health>().len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_scene_needing_an_absent_plugin_fails_to_load() {
        let (argv, dir) = health_scene_args();
        let args = Args::parse(argv.into_iter()).unwrap().unwrap();

        let error = build_runtime(&args, None, &[], &NullPlugin).unwrap_err();
        assert!(error.contains("Health"), "{error}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bundle_parses_and_excludes_project_and_export() {
        let args = parse(&["--bundle", "game.zip", "--headless"])
            .unwrap()
            .unwrap();
        assert_eq!(args.bundle.as_deref(), Some("game.zip"));
        assert!(parse(&["--bundle"]).is_err(), "a missing value is an error");
        assert!(parse(&["--bundle", "g.zip", "--project", "p"]).is_err());
        assert!(parse(&["--bundle", "g.zip", "--export", "out"]).is_err());
    }

    #[test]
    fn a_bundle_runs_its_default_scene() {
        let dir = std::env::temp_dir().join(format!(
            "noctis-bundle-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let project_dir = dir.join("game");
        std::fs::create_dir_all(project_dir.join("scenes")).expect("dirs");
        std::fs::write(
            project_dir.join("noctis.toml"),
            "[project]\nname = \"Bundled\"\n[project.settings]\ndefault_scene = \"scenes/main.scene.json\"\n",
        )
        .expect("manifest");
        std::fs::write(
            project_dir.join("scenes/main.scene.json"),
            r#"{ "name": "Bundle", "entities": [{ "id": 0, "name": "Marker",
                 "transform": { "translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] },
                 "components": [] }] }"#,
        )
        .expect("scene");

        let project = Project::open(&project_dir).expect("open");
        let bytes = project.bundle().expect("bundle");
        let zip = dir.join("game.zip");
        std::fs::write(&zip, &bytes).expect("write bundle");

        let argv = vec![
            "--bundle".to_string(),
            zip.to_string_lossy().into_owned(),
            "--headless".to_string(),
        ];
        let args = Args::parse(argv.into_iter()).unwrap().unwrap();
        let opened = open_project(&args).expect("open the bundle");
        assert!(opened.is_some(), "the bundle opens as a project");

        let runtime =
            build_runtime(&args, opened.as_ref(), &[], &NullPlugin).expect("run the bundle");
        assert_eq!(
            runtime.scene().world.entities().len(),
            1,
            "the scene from inside the archive loaded"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
