//! `noctis build`, `noctis pack`, and `noctis rename`.
//!
//! Each is a filesystem operation on a project (build a game folder; package a
//! `.zip` bundle; move an asset and rewrite the documents that reference it).
//! Parsing is hand-rolled; usage problems are distinguished from failures so the
//! caller can exit `2` or `1`.

use std::path::{Path, PathBuf};

use ecs::app::Plugin;
use project::{AssetPath, Profile, Project};
use runtime::build::{BuildOptions, build_project};

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
    --profile <PROFILE>  'release' (default) embeds scripts in the client; 'dev'
                         ships them as loose files, watched at run time.
    -h, --help           Show this help.";

const PACK_USAGE: &str = "\
noctis pack — package a project into a distributable .zip bundle

USAGE:
    noctis pack --project <DIR> [OPTIONS]

OPTIONS:
    --project <DIR>      The project directory (a noctis.toml). Required.
    --out <PATH>         The output .zip (default: <project>/dist/<name>.zip).
    -h, --help           Show this help.

A bundle is the project at the archive root, ready for a `ZipStore` reader — a
browser client, or `Project::open_in`. Generated directories (.noctis, dist,
target, .git) are excluded.";

const RENAME_USAGE: &str = "\
noctis rename — move an asset and rewrite every reference to it

USAGE:
    noctis rename --project <DIR> <FROM> <TO>

OPTIONS:
    --project <DIR>      The project directory (a noctis.toml). Required.
    -h, --help           Show this help.";

/// Parse `build`'s options and run the build. `plugin` is the project's Rust: a
/// `[rust]` project links it into the exported game and bundles its components'
/// asset references.
pub(crate) fn build(args: &[String], plugin: &dyn Plugin) -> Result<String, CliError> {
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
            "--profile" | "-P" => {
                let (value, consumed) = value_for("--profile")?;
                options.profile = Some(match value.as_str() {
                    "dev" => Profile::Dev,
                    "release" => Profile::Release,
                    other => {
                        return Err(CliError::Usage(format!(
                            "--profile must be 'dev' or 'release', got {other:?}"
                        )));
                    }
                });
                index = consumed;
            }
            other => return Err(CliError::Usage(format!("unknown option {other:?}"))),
        }
        index += 1;
    }

    let project =
        project.ok_or_else(|| CliError::Usage("build requires --project <DIR>".into()))?;
    // Fail fast on a bad project before spending time building the client.
    Project::open(PathBuf::from(&project)).map_err(|error| CliError::Message(error.to_string()))?;

    // `build_project` acquires the game client itself (embedding the project's
    // scripts for a release build, and linking the plugin for a `[rust]` project),
    // so `noctis build` is one command.
    let report = build_project(
        &PathBuf::from(&project),
        target.as_deref(),
        &options,
        plugin,
    )
    .map_err(CliError::Message)?;
    Ok(report.to_string())
}

/// Parse `pack`'s options and write the bundle.
pub(crate) fn pack(args: &[String]) -> Result<String, CliError> {
    let mut project: Option<String> = None;
    let mut out: Option<String> = None;
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
            "-h" | "--help" => return Ok(PACK_USAGE.to_string()),
            "--project" | "-p" => {
                let (value, consumed) = value_for("--project")?;
                project = Some(value);
                index = consumed;
            }
            "--out" | "-o" => {
                let (value, consumed) = value_for("--out")?;
                out = Some(value);
                index = consumed;
            }
            other => return Err(CliError::Usage(format!("unknown option {other:?}"))),
        }
        index += 1;
    }

    let dir = project.ok_or_else(|| CliError::Usage("pack requires --project <DIR>".into()))?;
    let project = Project::open(&dir).map_err(|error| CliError::Message(error.to_string()))?;
    let bytes = project
        .bundle()
        .map_err(|error| CliError::Message(format!("failed to pack {dir:?}: {error}")))?;

    let destination = out.map_or_else(
        || {
            Path::new(&dir)
                .join("dist")
                .join(format!("{}.zip", project.name()))
        },
        PathBuf::from,
    );
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|error| CliError::Message(error.to_string()))?;
    }
    std::fs::write(&destination, &bytes).map_err(|error| CliError::Message(error.to_string()))?;

    Ok(format!(
        "packed {} ({} bytes) to {}",
        project.name(),
        bytes.len(),
        destination.display()
    ))
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
    let mut project = Project::open(PathBuf::from(&project))
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
    use ecs::app::NullPlugin;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn build_requires_a_project() {
        assert!(matches!(
            build(&args(&["--target", "game"]), &NullPlugin),
            Err(CliError::Usage(_))
        ));
    }

    #[test]
    fn build_reports_a_missing_project() {
        assert!(matches!(
            build(&args(&["--project", "does-not-exist-xyz"]), &NullPlugin),
            Err(CliError::Message(_))
        ));
    }

    #[test]
    fn unknown_build_option_is_a_usage_error() {
        assert!(matches!(
            build(&args(&["--project", "p", "--nope"]), &NullPlugin),
            Err(CliError::Usage(_))
        ));
    }

    #[test]
    fn pack_parses_and_reports_errors() {
        assert!(pack(&args(&["--help"])).unwrap().contains("noctis pack"));
        assert!(matches!(pack(&args(&[])), Err(CliError::Usage(_))));
        assert!(matches!(
            pack(&args(&["--project", "p", "--nope"])),
            Err(CliError::Usage(_))
        ));
        // A missing project is a message error, not a usage one.
        assert!(matches!(
            pack(&args(&["--project", "does-not-exist-xyz"])),
            Err(CliError::Message(_))
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
