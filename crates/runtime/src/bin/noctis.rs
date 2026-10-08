//! `noctis` — the project tool: build a project into a distributable game.
//!
//! ```text
//! noctis build --project <dir> [--target <name>] [--scene <path>] [--name <n>] [--out <dir>]
//! ```
//!
//! Argument parsing is hand-rolled (no CLI dependency). Every failure becomes a
//! non-zero exit and a message on stderr.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

use project::{AssetPath, Project};
use runtime::build::{BuildOptions, build_project};

const USAGE: &str = "\
noctis — build and run Noctis projects

USAGE:
    noctis build --project <DIR> [OPTIONS]
    noctis rename --project <DIR> <FROM> <TO>
    noctis --help

COMMANDS:
    build                Build the project into a self-contained game folder.
    rename               Move an asset and rewrite every reference to it.

OPTIONS:
    --project <DIR>      The project directory (a noctis.toml). Required.
    --target <NAME>      Build the named [[build]] target (its scene, name, and
                         output directory are used).
    --scene <PATH>       Override the scene (project-relative).
    --name <NAME>        Override the game/executable name.
    --out <DIR>          Override the output directory.
    -h, --help           Show this help.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(Error::Usage(message)) => {
            eprintln!("noctis: {message}\n");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
        Err(Error::Message(message)) => {
            eprintln!("noctis: {message}");
            ExitCode::FAILURE
        }
    }
}

/// A top-level failure, so usage errors can exit `2`.
enum Error {
    Usage(String),
    Message(String),
}

fn run(args: &[String]) -> Result<String, Error> {
    let Some(command) = args.first() else {
        return Err(Error::Usage("no command given".to_string()));
    };
    match command.as_str() {
        "-h" | "--help" | "help" => Ok(USAGE.to_string()),
        "build" => build(&args[1..]),
        "rename" => rename(&args[1..]),
        other => Err(Error::Usage(format!("unknown command {other:?}"))),
    }
}

/// Parse `build`'s options and run the build.
fn build(args: &[String]) -> Result<String, Error> {
    let mut project: Option<String> = None;
    let mut target: Option<String> = None;
    let mut options = BuildOptions::default();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) => (flag, Some(value.to_string())),
            None => (arg.as_str(), None),
        };
        let value_for = |flag: &str| -> Result<(String, usize), Error> {
            if let Some(value) = inline.clone() {
                return Ok((value, index));
            }
            match args.get(index + 1) {
                Some(value) => Ok((value.clone(), index + 1)),
                None => Err(Error::Usage(format!("{flag} requires a value"))),
            }
        };
        match flag {
            "-h" | "--help" => return Ok(USAGE.to_string()),
            "--project" | "-p" => {
                let (value, consumed) = value_for("--project")?;
                project = Some(value);
                index = consumed;
            }
            "--target" | "-t" => {
                let (value, consumed) = value_for("--target")?;
                target = Some(value);
                index = consumed;
            }
            "--scene" | "-s" => {
                let (value, consumed) = value_for("--scene")?;
                options.scene = Some(value);
                index = consumed;
            }
            "--name" => {
                let (value, consumed) = value_for("--name")?;
                options.name = Some(value);
                index = consumed;
            }
            "--out" | "-o" => {
                let (value, consumed) = value_for("--out")?;
                options.destination = Some(PathBuf::from(value));
                index = consumed;
            }
            other => return Err(Error::Usage(format!("unknown option {other:?}"))),
        }
        index += 1;
    }

    let project = project.ok_or_else(|| Error::Usage("build requires --project <DIR>".into()))?;

    // Ship the runtime binary, not this tool. It must sit beside `noctis`.
    let runtime = runtime::export::default_runtime_executable().map_err(|error| {
        Error::Message(format!("cannot locate the runtime executable: {error}"))
    })?;
    if runtime.file_stem().and_then(|stem| stem.to_str()) != Some(runtime::export::RUNTIME_BIN) {
        return Err(Error::Message(
            "the noctis-runtime executable was not found next to noctis; \
             build the runtime first (`cargo build -p runtime`)"
                .to_string(),
        ));
    }
    options.runtime = Some(runtime);

    let report = build_project(&PathBuf::from(&project), target.as_deref(), &options)
        .map_err(Error::Message)?;
    Ok(report.to_string())
}

/// Parse `rename`'s options and move the asset.
fn rename(args: &[String]) -> Result<String, Error> {
    let mut project: Option<String> = None;
    let mut paths: Vec<String> = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        match arg.split_once('=') {
            Some(("--project" | "-p", value)) => project = Some(value.to_string()),
            _ if arg == "--project" || arg == "-p" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| Error::Usage("--project requires a value".into()))?;
                project = Some(value.clone());
                index += 1;
            }
            Some((flag, _)) if flag.starts_with('-') => {
                return Err(Error::Usage(format!("unknown option {flag:?}")));
            }
            None if arg.starts_with('-') => {
                return Err(Error::Usage(format!("unknown option {arg:?}")));
            }
            _ => paths.push(arg.clone()),
        }
        index += 1;
    }

    let project = project.ok_or_else(|| Error::Usage("rename requires --project <DIR>".into()))?;
    if paths.len() != 2 {
        return Err(Error::Usage(
            "rename requires <FROM> <TO> asset paths".into(),
        ));
    }
    let from = AssetPath::parse(&paths[0]).map_err(|error| Error::Message(error.to_string()))?;
    let to = AssetPath::parse(&paths[1]).map_err(|error| Error::Message(error.to_string()))?;
    let project = Project::open(PathBuf::from(&project))
        .map_err(|error| Error::Message(error.to_string()))?;
    let report = project
        .rename_asset(&from, &to)
        .map_err(|error| Error::Message(error.to_string()))?;
    Ok(format!(
        "renamed {} -> {} (rewrote {} document(s))",
        report.from.display(),
        report.to.display(),
        report.documents.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn help_needs_no_project() {
        assert!(run(&args(&["--help"])).is_ok());
        assert!(matches!(run(&args(&[])), Err(Error::Usage(_))));
    }

    #[test]
    fn an_unknown_command_is_a_usage_error() {
        assert!(matches!(run(&args(&["dance"])), Err(Error::Usage(_))));
    }

    #[test]
    fn build_requires_a_project() {
        assert!(matches!(
            run(&args(&["build", "--target", "game"])),
            Err(Error::Usage(_))
        ));
    }

    #[test]
    fn build_reports_a_missing_project() {
        let result = run(&args(&["build", "--project", "does-not-exist-xyz"]));
        assert!(matches!(result, Err(Error::Message(_))));
    }

    #[test]
    fn unknown_build_option_is_a_usage_error() {
        assert!(matches!(
            run(&args(&["build", "--project", "p", "--nope"])),
            Err(Error::Usage(_))
        ));
    }

    #[test]
    fn rename_requires_a_project_and_two_paths() {
        assert!(matches!(
            run(&args(&["rename", "a", "b"])),
            Err(Error::Usage(_))
        ));
        assert!(matches!(
            run(&args(&["rename", "--project", "p"])),
            Err(Error::Usage(_))
        ));
        // A bad asset path is a message error, not usage.
        assert!(matches!(
            run(&args(&["rename", "--project", "p", "/abs", "b"])),
            Err(Error::Message(_))
        ));
    }
}
