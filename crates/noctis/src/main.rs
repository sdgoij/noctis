//! `noctis` — the Noctis engine tool.
//!
//! One binary, one command per capability. Each subcommand is implemented by the
//! crate that owns it; this file only routes.
//!
//! ```text
//! noctis editor [OPTIONS]     open the editor, or build scenes from JSON
//! noctis run [OPTIONS]        run a scene or game (a window by default)
//! noctis build [OPTIONS]      build a project into a game folder
//! noctis rename [OPTIONS]     move an asset and rewrite its references
//! ```

#![forbid(unsafe_code)]

mod commands;

use std::process::ExitCode;

use commands::CliError;

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

fn main() -> ExitCode {
    runtime::cli::init_logging();
    let args: Vec<String> = std::env::args().skip(1).collect();
    dispatch(&args)
}

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

fn dispatch(args: &[String]) -> ExitCode {
    match route(args) {
        Route::Print(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Route::Editor(rest) => editor::cli::run_cli(rest),
        Route::Run(rest) => runtime::cli::run_cli(rest),
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
