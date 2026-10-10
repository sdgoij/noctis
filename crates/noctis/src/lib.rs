//! # noctis — the Noctis engine
//!
//! `noctis` is both the engine's multicall command-line tool and its embedding
//! facade.
//!
//! * The **binary** dispatches subcommands — `noctis editor`, `noctis run`,
//!   `noctis build`, `noctis rename` — each implemented by the crate that owns
//!   it. A project that carries Rust (a `[rust]` table) is run through a generated
//!   plugin-aware binary (see [`runtime::plugin`]).
//! * The **library** exposes the whole engine through [`prelude`], so an
//!   application can `use noctis::prelude::*` and drive the very crates the tool
//!   does. Everything is re-exported from the engine crates, never reimplemented
//!   here.
//!
//! A project's own Rust links the plugin with [`dispatch_with_plugin`], the
//! function the generated per-project binary calls:
//!
//! ```no_run
//! use noctis::prelude::*;
//!
//! fn main() -> std::process::ExitCode {
//!     noctis::dispatch_with_plugin(NullPlugin)
//! }
//! ```
//!
//! ```no_run
//! use noctis::prelude::*;
//!
//! let mut scene = Scene::new("Game");
//! let camera = scene.world.spawn();
//! scene.world.insert(camera, LocalTransform::from_translation(Vec3::new(0.0, 1.5, 6.0)));
//! scene.world.insert(camera, Camera::default());
//!
//! let mut runtime = Runtime::new(scene, RuntimeConfig::default());
//! let summary = runtime.run_headless(60, 1.0 / 60.0);
//! assert_eq!(summary.frames, 60);
//! ```

#![forbid(unsafe_code)]

mod commands;

use std::process::ExitCode;

use commands::CliError;
use ecs::app::{NullPlugin, Plugin};

pub mod prelude;

/// The `ecs` crate, re-exported so a crate that depends only on `noctis` can use
/// `#[derive(Component)]` — the derive resolves the reflect types through here.
pub use ecs;

const USAGE: &str = "\
noctis — the Noctis engine tool

USAGE:
    noctis <COMMAND> [OPTIONS]

COMMANDS:
    editor             Open the editor, or build scenes from JSON commands.
    run                Run a scene or game (a window by default; --headless exits).
    build              Build a project into a self-contained game folder.
    rename             Move an asset and rewrite every reference to it.
    help               Show this help.

Run `noctis <COMMAND> --help` for a command's options.";

/// Where a command line routes, so the routing is testable without a process.
#[derive(Debug)]
enum Route {
    /// Print `text` and exit successfully.
    Print(String),
    /// Hand the remaining arguments to the editor CLI.
    Editor(Vec<String>),
    /// Hand the remaining arguments to the game CLI.
    Run(Vec<String>),
    /// Fail.
    Fail(CliError),
}

fn route(args: &[String]) -> Route {
    let Some(command) = args.first() else {
        return Route::Print(USAGE.to_string());
    };
    let rest = args[1..].to_vec();
    match command.as_str() {
        "-h" | "--help" | "help" => Route::Print(USAGE.to_string()),
        "editor" => Route::Editor(rest),
        "run" => Route::Run(rest),
        "build" => finish(commands::build(&rest)),
        "rename" => finish(commands::rename(&rest)),
        other => Route::Fail(CliError::Usage(format!("unknown command {other:?}"))),
    }
}

fn finish(result: Result<String, CliError>) -> Route {
    match result {
        Ok(text) => Route::Print(text),
        Err(error) => Route::Fail(error),
    }
}

/// Dispatch `args`, routing to the crate that implements each subcommand.
///
/// The editor and the game runner receive `plugin`, so a project's Rust (custom
/// components and systems) is registered before the scene loads. `build`/`rename`
/// do not consult the plugin.
pub fn dispatch(args: &[String], plugin: &dyn Plugin) -> ExitCode {
    match route(args) {
        Route::Print(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Route::Editor(rest) => editor::cli::run_cli_with_plugin(rest, plugin),
        Route::Run(rest) => runtime::cli::run_cli_with_plugin(rest, &[], plugin),
        Route::Fail(CliError::Usage(message)) => {
            eprintln!("noctis: {message}\n");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
        Route::Fail(CliError::Message(message)) => {
            eprintln!("noctis: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Dispatch the process's own command line with `plugin`.
///
/// This is the entry point a generated per-project binary calls.
pub fn dispatch_with_plugin<P: Plugin>(plugin: P) -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    dispatch(&args, &plugin)
}

/// The stock `noctis` binary's entry point.
///
/// When `args` names a project with a `[rust]` table, the project is run through
/// its generated plugin-aware tool (built on demand); otherwise the stock tool
/// dispatches with no plugin.
pub fn main_entry(args: &[String]) -> ExitCode {
    if let Some(project) = runtime::plugin::plugin_project(args) {
        return match runtime::plugin::delegate(&project, args) {
            Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
            Err(error) => {
                eprintln!("noctis: {error}");
                ExitCode::FAILURE
            }
        };
    }
    dispatch(args, &NullPlugin)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_command_and_help_print_the_usage() {
        assert!(matches!(route(&[]), Route::Print(text) if text.contains("COMMANDS")));
        assert!(matches!(route(&args(&["--help"])), Route::Print(_)));
        assert!(matches!(route(&args(&["help"])), Route::Print(_)));
    }

    #[test]
    fn editor_and_run_hand_off_their_arguments() {
        match route(&args(&["editor", "--headless", "--print-schema"])) {
            Route::Editor(rest) => assert_eq!(rest, args(&["--headless", "--print-schema"])),
            other => panic!("expected editor, got {other:?}"),
        }
        match route(&args(&["run", "--headless", "--frames", "3"])) {
            Route::Run(rest) => assert_eq!(rest, args(&["--headless", "--frames", "3"])),
            other => panic!("expected run, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_command_is_a_usage_error() {
        assert!(matches!(
            route(&args(&["dance"])),
            Route::Fail(CliError::Usage(_))
        ));
    }
}
