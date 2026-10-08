//! `noctis build` and `noctis rename`.
//!
//! Both are filesystem operations on a project (build a game folder; move an
//! asset and rewrite the documents that reference it). Parsing is hand-rolled;
//! usage problems are distinguished from failures so the caller can exit `2` or
//! `1`.

use std::path::PathBuf;

use project::{AssetPath, Project};
use runtime::build::{BuildOptions, build_project};
use runtime::export::{RUNTIME_BIN, default_runtime_executable};

/// A top-level CLI failure, distinguished so usage problems can exit `2`.
#[derive(Debug)]
pub(crate) enum CliError {
    /// The command line was malformed.
    Usage(String),
    /// A runtime failure (bad project, scene, or I/O).
    Message(String),
}

const BUILD_USAGE: &str = "\
noctis build — build a project into a distributable game folder

USAGE:
    noctis build --project <DIR> [OPTIONS]

OPTIONS:
    --project <DIR>      The project directory (a noctis.toml). Required.
    --target <NAME>      Build the named [[build]] target (its scene, name, and
                         output directory are used).
    --scene <PATH>       Override the scene (project-relative).
    --name <NAME>        Override the game/executable name.
    --out <DIR>          Override the output directory.
    -h, --help           Show this help.";

const RENAME_USAGE: &str = "\
noctis rename — move an asset and rewrite every reference to it

USAGE:
    noctis rename --project <DIR> <FROM> <TO>

OPTIONS:
    --project <DIR>      The project directory (a noctis.toml). Required.
    -h, --help           Show this help.";

/// Parse `build`'s options and run the build.
pub(crate) fn build(args: &[String]) -> Result<String, CliError> {
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
        let value_for = |flag: &str| -> Result<(String, usize), CliError> {
            if let Some(value) = inline.clone() {
                return Ok((value, index));
            }
            match args.get(index + 1) {
                Some(value) => Ok((value.clone(), index + 1)),
                None => Err(CliError::Usage(format!("{flag} requires a value"))),
            }
        };
        match flag {
            "-h" | "--help" => return Ok(BUILD_USAGE.to_string()),
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
            other => return Err(CliError::Usage(format!("unknown option {other:?}"))),
        }
        index += 1;
    }

    let project =
        project.ok_or_else(|| CliError::Usage("build requires --project <DIR>".into()))?;

    // Ship the stripped game runtime, not this tool. It must sit beside `noctis`.
    let runtime = default_runtime_executable()
        .map_err(|error| CliError::Message(format!("cannot locate the game runtime: {error}")))?;
    if runtime.file_stem().and_then(|stem| stem.to_str()) != Some(RUNTIME_BIN) {
        return Err(CliError::Message(
            "the game runtime was not found next to noctis; build it first \
             (`cargo build --workspace`, which builds the noctis-game client)"
                .to_string(),
        ));
    }
    options.runtime = Some(runtime);

    let report = build_project(&PathBuf::from(&project), target.as_deref(), &options)
        .map_err(CliError::Message)?;
    Ok(report.to_string())
}

/// Parse `rename`'s options and move the asset.
pub(crate) fn rename(args: &[String]) -> Result<String, CliError> {
    let mut project: Option<String> = None;
    let mut paths: Vec<String> = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--help" || arg == "-h" {
            return Ok(RENAME_USAGE.to_string());
        }
        match arg.split_once('=') {
            Some(("--project" | "-p", value)) => project = Some(value.to_string()),
            _ if arg == "--project" || arg == "-p" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| CliError::Usage("--project requires a value".into()))?;
                project = Some(value.clone());
                index += 1;
            }
            Some((flag, _)) if flag.starts_with('-') => {
                return Err(CliError::Usage(format!("unknown option {flag:?}")));
            }
            None if arg.starts_with('-') => {
                return Err(CliError::Usage(format!("unknown option {arg:?}")));
            }
            _ => paths.push(arg.clone()),
        }
        index += 1;
    }

    let project =
        project.ok_or_else(|| CliError::Usage("rename requires --project <DIR>".into()))?;
    if paths.len() != 2 {
        return Err(CliError::Usage(
            "rename requires <FROM> <TO> asset paths".into(),
        ));
    }
    let from = AssetPath::parse(&paths[0]).map_err(|error| CliError::Message(error.to_string()))?;
    let to = AssetPath::parse(&paths[1]).map_err(|error| CliError::Message(error.to_string()))?;
    let project = Project::open(PathBuf::from(&project))
        .map_err(|error| CliError::Message(error.to_string()))?;
    let report = project
        .rename_asset(&from, &to)
        .map_err(|error| CliError::Message(error.to_string()))?;
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
    fn build_requires_a_project() {
        assert!(matches!(
            build(&args(&["--target", "game"])),
            Err(CliError::Usage(_))
        ));
    }

    #[test]
    fn build_reports_a_missing_project() {
        assert!(matches!(
            build(&args(&["--project", "does-not-exist-xyz"])),
            Err(CliError::Message(_))
        ));
    }

    #[test]
    fn unknown_build_option_is_a_usage_error() {
        assert!(matches!(
            build(&args(&["--project", "p", "--nope"])),
            Err(CliError::Usage(_))
        ));
    }

    #[test]
    fn rename_requires_a_project_and_two_paths() {
        assert!(matches!(
            rename(&args(&["a", "b"])),
            Err(CliError::Usage(_))
        ));
        assert!(matches!(
            rename(&args(&["--project", "p"])),
            Err(CliError::Usage(_))
        ));
        // A bad asset path is a message error, not usage.
        assert!(matches!(
            rename(&args(&["--project", "p", "/abs", "b"])),
            Err(CliError::Message(_))
        ));
    }
}
