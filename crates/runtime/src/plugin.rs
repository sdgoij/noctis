//! Build and run a project's plugin-aware tool binary.
//!
//! A project with a `[rust]` table carries a [`Plugin`](ecs::app::Plugin) —
//! custom components and systems the stock `noctis` binary does not link. For such
//! a project the tool generates a small crate under the project's gitignored
//! `.noctis/tool/`, builds it (a cargo no-op when it is already fresh), and runs it
//! as a subprocess. The generated `main` calls
//! `noctis::dispatch_with_plugin(<plugin>())`, so `run`/`editor`/`build` behave
//! exactly as the stock tool but with the plugin registered before the scene loads
//! and its systems scheduled after the engine's. See `docs/embedding.md`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use project::{Profile, Project};

use crate::export::{absolute, executable_name, profile_dir, workspace_root};

/// The subdirectory of a project's `.noctis` where the generated tool lives.
pub const TOOL_DIR: &str = ".noctis/tool";

/// The project to delegate `args` to, when the tool must use its plugin-aware
/// binary — a `run`/`editor` invocation whose `--project` names a project with a
/// `[rust]` table.
///
/// A missing or malformed project is `None` here; the stock tool reports it.
pub fn plugin_project(args: &[String]) -> Option<Project> {
    let command = args.first()?;
    if !matches!(command.as_str(), "run" | "editor") {
        return None;
    }
    let dir = project_arg(args)?;
    let project = Project::open(dir).ok()?;
    project.manifest().rust.is_some().then_some(project)
}

/// Build the project's plugin-aware tool and run it with `args`, returning the
/// child's exit code. `args` is the tool's whole command line (including the
/// subcommand), since the generated binary dispatches it the same way.
pub fn delegate(project: &Project, args: &[String]) -> Result<i32, String> {
    let binary = build_tool(project, Profile::Dev)?;
    let status = Command::new(&binary)
        .args(args)
        .status()
        .map_err(|error| format!("could not run the project's plugin tool: {error}"))?;
    Ok(status.code().unwrap_or(1))
}

/// Generate and build the project's plugin-aware tool, returning the binary path.
pub fn build_tool(project: &Project, profile: Profile) -> Result<PathBuf, String> {
    let plugin_path = project
        .plugin_path()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("the project {:?} has no [rust] table", project.name()))?;
    let plugin_dir = project
        .rust_crate_dir()
        .ok_or_else(|| format!("the project {:?} has no [rust] table", project.name()))?;
    let package = project
        .rust_package_name()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("the project {:?} has no [rust] table", project.name()))?;
    let root = workspace_root().ok_or_else(|| {
        "could not locate the engine workspace (building a project's Rust needs it)".to_string()
    })?;
    let noctis_dir = absolute(&root.join("crates").join("noctis"));
    let plugin_dir = absolute(&plugin_dir);

    let tool_name = tool_crate_name(project.name());
    let dir = absolute(&project.dir(TOOL_DIR));
    fs::create_dir_all(&dir)
        .map_err(|error| format!("could not create {}: {error}", dir.display()))?;

    let manifest = tool_manifest(&tool_name, &noctis_dir, &plugin_path, &package, &plugin_dir);
    let main = tool_main(&plugin_path);
    write(&dir.join("Cargo.toml"), &manifest)?;
    write(&dir.join("main.rs"), &main)?;

    log::info!("building the project's plugin tool (cargo build --bin {tool_name})");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .arg("build")
        .arg("--manifest-path")
        .arg(dir.join("Cargo.toml"))
        .arg("--bin")
        .arg(&tool_name);
    if profile == Profile::Release {
        command.arg("--release");
    }
    match command.status() {
        Ok(status) if status.success() => {}
        Ok(status) => {
            return Err(format!(
                "building the project's plugin tool failed ({status})"
            ));
        }
        Err(error) => {
            return Err(format!(
                "could not run cargo to build the plugin tool: {error}"
            ));
        }
    }

    let binary = dir
        .join("target")
        .join(profile_dir(profile))
        .join(executable_name(&tool_name));
    if !binary.is_file() {
        return Err(format!(
            "the built plugin tool was not found at {}",
            binary.display()
        ));
    }
    Ok(binary)
}

fn write(path: &Path, contents: &str) -> Result<(), String> {
    fs::write(path, contents)
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

/// A cargo-safe crate name for the project's tool, derived from the project name.
pub fn tool_crate_name(project_name: &str) -> String {
    let mut sanitized: String = project_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while sanitized.contains("--") {
        sanitized = sanitized.replace("--", "-");
    }
    let sanitized = sanitized.trim_matches('-');
    if sanitized.is_empty() {
        "noctis-tool".to_string()
    } else {
        format!("noctis-tool-{sanitized}")
    }
}

/// The `--project <dir>` (or `--project=<dir>`, `-p`) value, if present.
fn project_arg(args: &[String]) -> Option<PathBuf> {
    let mut index = 1;
    while index < args.len() {
        let arg = &args[index];
        if let Some(value) = arg.strip_prefix("--project=") {
            return Some(PathBuf::from(value));
        }
        if arg == "--project" || arg == "-p" {
            return args.get(index + 1).map(PathBuf::from);
        }
        index += 1;
    }
    None
}

/// The generated tool crate's `Cargo.toml`.
///
/// The dependency key is the plugin path's first segment (its extern name); when
/// that differs from the crate's package name the entry renames with `package`.
/// A `[workspace]` table keeps the generated crate out of the engine workspace.
pub fn tool_manifest(
    tool_name: &str,
    noctis_dir: &Path,
    plugin_path: &str,
    package: &str,
    plugin_dir: &Path,
) -> String {
    let extern_name = plugin_path.split("::").next().unwrap_or(package);
    let dependency = if extern_name == package {
        format!("{extern_name} = {{ path = {plugin_dir:?} }}")
    } else {
        format!("{extern_name} = {{ package = {package:?}, path = {plugin_dir:?} }}")
    };
    format!(
        "[package]\n\
         name = {tool_name:?}\n\
         version = \"0.0.0\"\n\
         edition = \"2024\"\n\
         publish = false\n\n\
         [[bin]]\n\
         name = {tool_name:?}\n\
         path = \"main.rs\"\n\n\
         [dependencies]\n\
         noctis = {{ path = {noctis_dir:?} }}\n\
         {dependency}\n\n\
         [workspace]\n"
    )
}

/// The generated tool crate's `main.rs`.
pub fn tool_main(plugin_path: &str) -> String {
    format!(
        "//! Generated by `noctis`: the project's plugin-aware tool binary.\n\
         //!\n\
         //! Do not edit; it is rewritten from `noctis.toml` on every build.\n\n\
         fn main() -> std::process::ExitCode {{\n    \
         noctis::dispatch_with_plugin({plugin_path}())\n}}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_crate_names_are_sanitized() {
        assert_eq!(tool_crate_name("Showcase"), "noctis-tool-showcase");
        assert_eq!(tool_crate_name("My Game!"), "noctis-tool-my-game");
        assert_eq!(tool_crate_name("--weird--"), "noctis-tool-weird");
        assert_eq!(tool_crate_name("!!!"), "noctis-tool");
    }

    #[test]
    fn the_manifest_names_the_noctis_and_plugin_paths() {
        let manifest = tool_manifest(
            "noctis-tool-game",
            Path::new("/engine/crates/noctis"),
            "my_game::plugin",
            "my-game",
            Path::new("/project/game/rust"),
        );
        assert!(
            manifest.contains("name = \"noctis-tool-game\""),
            "{manifest}"
        );
        assert!(manifest.contains("path = \"main.rs\""), "{manifest}");
        assert!(
            manifest.contains("noctis = { path = \"/engine/crates/noctis\" }"),
            "{manifest}"
        );
        assert!(
            manifest.contains("my_game = { package = \"my-game\", path = \"/project/game/rust\" }"),
            "{manifest}"
        );
        assert!(manifest.contains("[workspace]"), "{manifest}");
    }

    #[test]
    fn a_matching_package_name_needs_no_rename() {
        let manifest = tool_manifest(
            "noctis-tool-game",
            Path::new("/engine/crates/noctis"),
            "mygame::plugin",
            "mygame",
            Path::new("/project/game/rust"),
        );
        assert!(
            manifest.contains("mygame = { path = \"/project/game/rust\" }"),
            "{manifest}"
        );
    }

    #[test]
    fn the_main_dispatches_with_the_plugin() {
        let main = tool_main("my_game::plugin");
        assert!(
            main.contains("noctis::dispatch_with_plugin(my_game::plugin())"),
            "{main}"
        );
    }

    #[test]
    fn project_arg_reads_every_spelling() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            project_arg(&args(&["run", "--project", "p"])),
            Some(PathBuf::from("p"))
        );
        assert_eq!(
            project_arg(&args(&["editor", "--project=p"])),
            Some(PathBuf::from("p"))
        );
        assert_eq!(
            project_arg(&args(&["run", "-p", "p"])),
            Some(PathBuf::from("p"))
        );
        assert_eq!(project_arg(&args(&["run"])), None);
        assert_eq!(project_arg(&args(&["run", "--project"])), None);
    }

    #[test]
    fn plugin_project_requires_a_rust_project() {
        let dir = std::env::temp_dir().join(format!("noctis-plugin-proj-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        // No [rust]: nothing to delegate.
        std::fs::write(dir.join("noctis.toml"), "[project]\nname = \"T\"\n").expect("manifest");
        let project = dir.to_string_lossy().into_owned();
        assert!(plugin_project(&args(&["run", "--project", &project])).is_none());

        // With [rust]: delegate run/editor/build, but not unrelated commands.
        std::fs::write(
            dir.join("noctis.toml"),
            "[project]\nname = \"T\"\n\n[rust]\n",
        )
        .expect("manifest");
        assert!(plugin_project(&args(&["run", "--project", &project])).is_some());
        assert!(plugin_project(&args(&["editor", "--project", &project])).is_some());
        assert!(plugin_project(&args(&["build", "--project", &project])).is_none());
        assert!(plugin_project(&args(&["rename", "--project", &project])).is_none());
        assert!(plugin_project(&args(&["run"])).is_none());

        std::fs::remove_dir_all(&dir).ok();
    }
}
