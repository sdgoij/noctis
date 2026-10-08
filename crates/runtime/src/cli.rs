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

use ecs::assets::AssetRegistry;
use ecs::scene::Scene;
use math::vec3;
use platform::WindowConfig;
use project::Project;
use render::RendererConfig;

use crate::scripts;
use crate::window::run_in_window;
use crate::{
    BuildOptions, ExportReport, RunSummary, Runtime, RuntimeConfig, build_project, bundled_scene,
    demo, export_game,
};

/// The prefix on every diagnostic message.
const PROGRAM: &str = "noctis";

/// Parsed command-line arguments.
#[derive(Clone, Debug)]
struct Args {
    scene: Option<String>,
    project: Option<String>,
    target: Option<String>,
    demo: bool,
    frames: u32,
    dt: f32,
    render: bool,
    headless: bool,
    audio: bool,
    export: Option<String>,
    name: Option<String>,
    wasm: Vec<(String, String)>,
    print_summary: bool,
    quiet: bool,
    allow_inert: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            scene: None,
            project: None,
            target: None,
            demo: false,
            frames: 300,
            dt: 1.0 / 60.0,
            render: false,
            headless: false,
            audio: true,
            export: None,
            name: None,
            wasm: Vec::new(),
            print_summary: false,
            quiet: false,
            allow_inert: false,
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
                "--allow-inert" => args.allow_inert = true,
                "--scene" => {
                    let value = it.next().ok_or("--scene requires a file path")?;
                    args.scene = Some(value);
                }
                "--project" => {
                    let value = it.next().ok_or("--project requires a directory")?;
                    args.project = Some(value);
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
                }
                other => return Err(format!("unknown argument '{other}'")),
            }
        }
        if args.demo && args.scene.is_some() {
            return Err("--demo and --scene are mutually exclusive".into());
        }
        // No scene is not an error: `load_scene` falls back to a scene bundled
        // next to the executable, then to the built-in demo.
        Ok(Some(args))
    }
}

/// Run the game command line and return the process exit code.
///
/// Used by both `noctis run` and the `noctis-game` binary an export ships.
pub fn run_cli(args: Vec<String>) -> ExitCode {
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
        return match run_windowed(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => fail(&message),
        };
    }

    if !args.headless {
        log::warn!("--frames/--render/--print-summary are ignored without --headless");
    }

    if let Some(destination) = args.export.clone() {
        return match do_export(&args, &destination) {
            Ok(report) => {
                println!("{report}");
                ExitCode::SUCCESS
            }
            Err(message) => fail(&message),
        };
    }

    match run(&args) {
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
    --target <name>    Export the named [[build]] target from the project (its
                       scene, name, and output directory are used).
    --demo             Run a small built-in scene.
    --headless         Run without a window: advance --frames frames and exit.
                       Without this, a window opens and runs until it is closed.
    --frames <N>       Number of frames to run headlessly (default 300).
    --dt <seconds>     Frame delta time (default 1/60).
    --render           Render frames with a headless GPU (skipped if unavailable).
    --no-audio         Do not open an audio device (use silent audio).
    --export <dir>     Export a self-contained game folder (requires --scene).
    --name <name>      Exported executable name (default: the scene file stem).
    --wasm <name>=<path>
                       Register a .wasm module as a script asset (repeatable;
                       requires a build with the `slag` feature).
    --allow-inert      Run even if the project attaches scripts this build cannot
                       execute (they are skipped).
    --print-summary    Print the run summary (printed by default).
    --quiet            Suppress the run summary.
    -h, --help         Show this help.

ENVIRONMENT:
    NOCTIS_LOG         error | warn | info (default) | debug | trace | off"
}

/// The script host a game runs with: the Slag JavaScript engine plus the native
/// WebAssembly host when the `slag` feature is enabled, otherwise the inert
/// default.
#[cfg(feature = "slag")]
type Host = script::engine_host::EngineHost;
#[cfg(not(feature = "slag"))]
type Host = script::NoopHost;

/// Build the script host for a run.
#[cfg(feature = "slag")]
fn make_host() -> Result<Host, String> {
    log::info!("JavaScript and WebAssembly scripting enabled (Slag engine)");
    script::engine_host::EngineHost::new()
        .map_err(|err| format!("failed to start the JavaScript engine: {err}"))
}

/// Build the inert script host (scripts are discovered but not executed).
#[cfg(not(feature = "slag"))]
fn make_host() -> Result<Host, String> {
    Ok(script::NoopHost)
}

/// Build the runtime for the requested scene.
fn build_runtime(args: &Args, project: Option<&Project>) -> Result<Runtime<Host>, String> {
    let mut config = RuntimeConfig {
        audio: args.audio,
        timestep: args.dt,
        ..RuntimeConfig::default()
    };
    if let Some(project) = project {
        let gravity = project.manifest().project.settings.gravity;
        config.gravity = vec3(gravity[0], gravity[1], gravity[2]);
    }

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

    // The project's `[render] environment` names the `.hdr` the renderer
    // precomputes image-based lighting from. A named environment that did not
    // import is an error: the game would silently lose its lighting.
    if let Some(path) = project.and_then(|p| p.manifest().render.environment.as_deref()) {
        let handle = assets.hdr.handle(path).ok_or_else(|| {
            format!("the project's [render] environment {path:?} is not an importable .hdr asset")
        })?;
        config.environment = Some(handle);
    }

    register_wasm_modules(args, &mut assets)?;
    let scene = load_scene(args, project, &mut assets)?;

    // Refuse to run a game whose scripts this build cannot execute, unless the
    // caller opted into an inert run.
    scripts::require_scripting(scripts::attached_scripts(&scene.world), args.allow_inert)?;

    let host = make_host()?;

    let mut runtime: Runtime<Host> = Runtime::from_parts(scene, assets, config, host);

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
#[cfg(feature = "slag")]
fn register_wasm_modules(args: &Args, assets: &mut AssetRegistry) -> Result<(), String> {
    for (name, path) in &args.wasm {
        script::wasm_host::register(assets, name.clone(), path)
            .map_err(|err| format!("failed to register wasm module {path:?}: {err}"))?;
    }
    Ok(())
}

/// `--wasm` needs the `slag` feature that compiles the WebAssembly host.
#[cfg(not(feature = "slag"))]
fn register_wasm_modules(args: &Args, _assets: &mut AssetRegistry) -> Result<(), String> {
    if args.wasm.is_empty() {
        Ok(())
    } else {
        Err("--wasm requires a build with the `slag` feature".to_string())
    }
}

/// Load the scene to run: an explicit file (resolved against `--project` when
/// given), the project's default scene, a `scene.json` bundled next to the
/// executable (an exported game), or the built-in demo.
fn load_scene(
    args: &Args,
    project: Option<&Project>,
    assets: &mut AssetRegistry,
) -> Result<Scene, String> {
    if args.demo {
        return Ok(demo::demo_scene(assets));
    }

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
        return Ok(demo::demo_scene(assets));
    };
    if !path.is_file() {
        log::warn!(
            "scene '{}' not found; using the built-in demo scene",
            path.display()
        );
        return Ok(demo::demo_scene(assets));
    }
    let json = fs::read_to_string(&path)
        .map_err(|err| format!("failed to read scene '{}': {err}", path.display()))?;
    Scene::from_json(&json, assets)
        .map_err(|err| format!("failed to load scene '{}': {err}", path.display()))
}

/// A `scene.json` sitting next to the running executable, if any.
fn bundled_next_to_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    bundled_scene(exe.parent()?)
}

/// Export a self-contained game folder for `destination`.
fn do_export(args: &Args, destination: &str) -> Result<ExportReport, String> {
    // A project build bundles the referenced assets; a loose scene does not.
    if let Some(dir) = export_project_dir(args) {
        let options = BuildOptions {
            destination: Some(PathBuf::from(destination)),
            name: args.name.clone(),
            scene: args.scene.clone(),
            allow_inert: args.allow_inert,
            ..BuildOptions::default()
        };
        return build_project(Path::new(&dir), args.target.as_deref(), &options);
    }

    let scene = args
        .scene
        .clone()
        .ok_or("--export requires --scene <path>, or a project with a default scene")?;
    let scene_json = fs::read_to_string(&scene)
        .map_err(|err| format!("failed to read scene '{scene}': {err}"))?;
    scripts::require_scripting(scripts::scripts_in_document(&scene_json)?, args.allow_inert)?;
    let name = args
        .name
        .clone()
        .unwrap_or_else(|| default_game_name(&scene));
    export_game(Path::new(destination), &name, &scene_json, &[], None, None)
        .map_err(|err| err.to_string())
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
fn run(args: &Args) -> Result<RunSummary, String> {
    let project = open_project(args)?;
    let mut runtime = build_runtime(args, project.as_ref())?;
    Ok(runtime.run_headless(args.frames, args.dt))
}

/// Run the scene in a window until it is closed.
fn run_windowed(args: &Args) -> Result<(), String> {
    let project = open_project(args)?;
    let runtime = build_runtime(args, project.as_ref())?;
    let config = window_config(args, project.as_ref());
    run_in_window(runtime, config).map_err(|err| err.to_string())
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
    fn allow_inert_is_parsed() {
        assert!(parse(&["--allow-inert"]).unwrap().unwrap().allow_inert);
    }
}
