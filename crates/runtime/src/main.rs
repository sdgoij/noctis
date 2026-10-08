//! `noctis-runtime` — run (or "export") a Noctis scene headlessly from the
//! command line.
//!
//! ```text
//! noctis-runtime --scene level.json --frames 600 --print-summary
//! noctis-runtime --demo --frames 60 --render --print-summary
//! ```
//!
//! Argument parsing is hand-rolled (no CLI dependency). Set `NOCTIS_LOG` to
//! `error`, `warn`, `info` (default), `debug`, `trace`, or `off` to control
//! logging.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use ecs::assets::AssetRegistry;
use ecs::scene::Scene;
use platform::WindowConfig;
use render::RendererConfig;
use runtime::{RunSummary, Runtime, RuntimeConfig, demo, run_in_window};

/// Parsed command-line arguments.
#[derive(Clone, Debug)]
struct Args {
    scene: Option<String>,
    demo: bool,
    frames: u32,
    dt: f32,
    render: bool,
    window: bool,
    audio: bool,
    print_summary: bool,
    quiet: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            scene: None,
            demo: false,
            frames: 300,
            dt: 1.0 / 60.0,
            render: false,
            window: false,
            audio: true,
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
                "--window" => args.window = true,
                "--no-audio" => args.audio = false,
                "--print-summary" => args.print_summary = true,
                "--quiet" | "-q" => args.quiet = true,
                "--scene" => {
                    let value = it.next().ok_or("--scene requires a file path")?;
                    args.scene = Some(value);
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
        // With no scene, fall back to the built-in demo so the binary always
        // does something useful instead of erroring.
        if !args.demo && args.scene.is_none() {
            args.demo = true;
        }
        Ok(Some(args))
    }
}

fn usage() -> &'static str {
    "\
noctis-runtime — run a Noctis scene headlessly

USAGE:
    noctis-runtime --scene <path> [OPTIONS]
    noctis-runtime [OPTIONS]                 # runs the built-in demo scene

OPTIONS:
    --scene <path>     Load and run a scene JSON file (optional).
    --demo             Run a small built-in scene (the default when --scene is omitted).
    --frames <N>       Number of frames to run (default 300).
    --dt <seconds>     Frame delta time (default 1/60).
    --render           Render frames with a headless GPU (skipped if unavailable).
    --window           Open a window and run with a live viewport (blocks).
    --no-audio         Do not open an audio device (use silent audio).
    --print-summary    Print the run summary (printed by default).
    --quiet            Suppress the run summary.
    -h, --help         Show this help.

ENVIRONMENT:
    NOCTIS_LOG         error | warn | info (default) | debug | trace | off"
}

/// Build the runtime for the requested scene (demo or file).
fn build_runtime(args: &Args) -> Result<Runtime, String> {
    let config = RuntimeConfig {
        audio: args.audio,
        timestep: args.dt,
        ..RuntimeConfig::default()
    };

    let mut assets = AssetRegistry::with_defaults();
    let scene = if args.demo {
        demo::demo_scene(&mut assets)
    } else {
        let path = args
            .scene
            .as_deref()
            .ok_or("no scene path (internal error: pass --scene or --demo)")?;
        if std::path::Path::new(path).exists() {
            let json = std::fs::read_to_string(path)
                .map_err(|err| format!("failed to read scene '{path}': {err}"))?;
            Scene::from_json(&json, &assets)
                .map_err(|err| format!("failed to load scene '{path}': {err}"))?
        } else {
            log::warn!("scene '{path}' not found; using the built-in demo scene");
            demo::demo_scene(&mut assets)
        }
    };

    let mut runtime: Runtime = Runtime::from_assets(scene, assets, config);

    if args.render {
        match runtime.enable_renderer(RendererConfig::default()) {
            Ok(()) => log::info!("headless rendering enabled"),
            Err(err) => log::warn!("rendering disabled: {err}"),
        }
    }

    Ok(runtime)
}

/// Load the scene and run the loop headlessly. Returns the run summary.
fn run(args: &Args) -> Result<RunSummary, String> {
    let mut runtime = build_runtime(args)?;
    Ok(runtime.run_headless(args.frames, args.dt))
}

/// Run the scene in a window until it is closed.
fn run_windowed(args: &Args) -> Result<(), String> {
    let runtime = build_runtime(args)?;
    let title = args.scene.clone().unwrap_or_else(|| "Noctis".to_string());
    let config = WindowConfig {
        title,
        width: 1280,
        height: 720,
    };
    run_in_window(runtime, config).map_err(|err| err.to_string())
}

fn main() -> ExitCode {
    init_logging();

    let args = match Args::parse(std::env::args().skip(1)) {
        Ok(Some(args)) => args,
        Ok(None) => {
            println!("{}", usage());
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("noctis-runtime: {message}\n");
            eprintln!("{}", usage());
            return ExitCode::from(2);
        }
    };

    if args.window {
        return match run_windowed(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("noctis-runtime: {message}");
                ExitCode::from(1)
            }
        };
    }

    match run(&args) {
        Ok(summary) => {
            if args.print_summary || !args.quiet {
                println!("{summary}");
            }
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("noctis-runtime: {message}");
            ExitCode::from(1)
        }
    }
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
fn init_logging() {
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
