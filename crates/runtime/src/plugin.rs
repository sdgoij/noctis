//! Build a project's plugin-aware binaries.
//!
//! A project with a `Cargo.toml` at its root carries a
//! [`Plugin`](ecs::app::Plugin) — custom components and systems the stock `noctis`
//! binary does not link, exported as the crate's `noctis_plugin` symbol by
//! `#[noctis::plugin]`. This module generates and builds the two binaries such a
//! project needs, each a small crate under the project's gitignored `.noctis/`:
//!
//! * the **tool** (`.noctis/tool/`) — links `noctis` and the plugin and calls
//!   `noctis::dispatch_with_plugin(<crate>::noctis_plugin())`, so
//!   `noctis run`/`editor`/`build` behave exactly as the stock tool but with the
//!   plugin registered (see [`build_tool`] and [`delegate`]);
//! * the **client** (`.noctis/client/`) — links the `runtime` and the plugin and
//!   calls `runtime::cli::run_cli_with_plugin`, the stripped game an export ships
//!   (see [`build_client`]). It depends on `noctis` *without* the editor feature
//!   (transitively, through the plugin), so a shipped plugin game carries no
//!   authoring tools.
//!
//! See `.notes/embedding.md`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use project::{Profile, Project};

use crate::export::{ExportFile, absolute, executable_name, profile_dir, workspace_root};

/// The subdirectory of a project's `.noctis` the generated tool lives in.
pub const TOOL_DIR: &str = ".noctis/tool";

/// The subdirectory of a project's `.noctis` the generated client lives in.
pub const CLIENT_DIR: &str = ".noctis/client";

/// The project to delegate `args` to, when the tool must use its plugin-aware
/// binary — a `run`/`editor`/`build` invocation whose `--project` names a project
/// with a `Cargo.toml` at its root (a Rust plugin).
///
/// A missing or malformed project is `None` here; the stock tool reports it.
pub fn plugin_project(args: &[String]) -> Option<Project> {
    let command = args.first()?;
    if !matches!(command.as_str(), "run" | "editor" | "build") {
        return None;
    }
    let dir = project_arg(args)?;
    let project = Project::open(dir).ok()?;
    project.rust_crate_dir().is_some().then_some(project)
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
    let info = PluginInfo::resolve(project)?;
    let name = tool_crate_name(project.name());
    let dir = project.dir(TOOL_DIR);
    let manifest = tool_manifest(&name, &info.noctis_dir, &info);
    let main = tool_main(&info.plugin_path);
    generate(&dir, &name, &manifest, &main, &[("build.rs", BUILD_RS)])?;
    cargo_build(&dir, &name, profile)
}

/// Generate and build the project's plugin-linked game client, returning the
/// binary path. `scripts` are baked in (empty for a development client, which
/// loads them from disk).
pub fn build_client(
    project: &Project,
    profile: Profile,
    scripts: &[ExportFile],
) -> Result<PathBuf, String> {
    let info = PluginInfo::resolve(project)?;
    let name = client_crate_name(project.name());
    let dir = project.dir(CLIENT_DIR);
    let manifest = client_manifest(&name, &info.runtime_dir, &info);
    let main = client_main(&info.plugin_path);
    let embedded = embedded_rs(scripts);
    generate(
        &dir,
        &name,
        &manifest,
        &main,
        &[("embedded.rs", embedded.as_str()), ("build.rs", BUILD_RS)],
    )?;
    cargo_build(&dir, &name, profile)
}

/// The resolved locations and names a generated crate needs.
struct PluginInfo {
    noctis_dir: PathBuf,
    runtime_dir: PathBuf,
    plugin_path: String,
    package: String,
    plugin_dir: PathBuf,
}

impl PluginInfo {
    fn resolve(project: &Project) -> Result<Self, String> {
        let plugin_path = project
            .plugin_path()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| no_rust_plugin(project))?;
        let plugin_dir = project
            .rust_crate_dir()
            .ok_or_else(|| no_rust_plugin(project))?;
        let package = project
            .rust_package_name()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| no_rust_plugin(project))?;
        let root = workspace_root().ok_or_else(|| {
            "could not locate the engine workspace (building a project's Rust needs it)".to_string()
        })?;
        Ok(Self {
            noctis_dir: absolute(&root.join("crates").join("noctis")),
            runtime_dir: absolute(&root.join("crates").join("runtime")),
            plugin_path,
            package,
            plugin_dir: absolute(&plugin_dir),
        })
    }

    /// The `plugin = { ... }` dependency line for a generated crate.
    fn dependency(&self) -> String {
        let extern_name = self.plugin_path.split("::").next().unwrap_or(&self.package);
        if extern_name == self.package {
            format!("{extern_name} = {{ path = {:?} }}", self.plugin_dir)
        } else {
            format!(
                "{extern_name} = {{ package = {:?}, path = {:?} }}",
                self.package, self.plugin_dir
            )
        }
    }
}

/// Write a generated crate (`Cargo.toml`, `main.rs`, and any extra files).
fn generate(
    dir: &Path,
    name: &str,
    manifest: &str,
    main: &str,
    extra: &[(&str, &str)],
) -> Result<(), String> {
    let dir = absolute(dir);
    fs::create_dir_all(&dir)
        .map_err(|error| format!("could not create {}: {error}", dir.display()))?;
    let write = |file: &str, contents: &str| -> Result<(), String> {
        let path = dir.join(file);
        fs::write(&path, contents)
            .map_err(|error| format!("could not write {}: {error}", path.display()))
    };
    write("Cargo.toml", manifest)?;
    write("main.rs", main)?;
    for (file, contents) in extra {
        write(file, contents)?;
    }
    log::debug!("generated the {name} crate in {}", dir.display());
    Ok(())
}

/// Build a generated crate's binary and return its path.
fn cargo_build(dir: &Path, name: &str, profile: Profile) -> Result<PathBuf, String> {
    let dir = absolute(dir);
    log::info!("building the project's {name} (cargo build --bin {name})");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .arg("build")
        .arg("--manifest-path")
        .arg(dir.join("Cargo.toml"))
        .arg("--bin")
        .arg(name);
    if profile == Profile::Release {
        command.arg("--release");
    }
    match command.status() {
        Ok(status) if status.success() => {}
        Ok(status) => return Err(format!("building the project's {name} failed ({status})")),
        Err(error) => return Err(format!("could not run cargo to build {name}: {error}")),
    }
    let binary = dir
        .join("target")
        .join(profile_dir(profile))
        .join(executable_name(name));
    if !binary.is_file() {
        return Err(format!(
            "the built {name} was not found at {}",
            binary.display()
        ));
    }
    Ok(binary)
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

/// A cargo-safe crate name for the project's tool, derived from the project name.
fn tool_crate_name(project_name: &str) -> String {
    format!("noctis-tool-{}", sanitize(project_name))
}

/// A cargo-safe crate name for the project's game client.
fn client_crate_name(project_name: &str) -> String {
    format!("noctis-client-{}", sanitize(project_name))
}

fn sanitize(name: &str) -> String {
    let mut sanitized: String = name
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
    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        "game".to_string()
    } else {
        trimmed.to_string()
    }
}

/// The generated tool crate's `Cargo.toml`.
///
/// The dependency key is the plugin path's first segment (its extern name); when
/// that differs from the crate's package name the entry renames with `package`.
/// A `[workspace]` table keeps the generated crate out of the engine workspace.
fn tool_manifest(name: &str, noctis_dir: &Path, info: &PluginInfo) -> String {
    let dependency = info.dependency();
    format!(
        "[package]\n\
         name = {name:?}\n\
         version = \"0.0.0\"\n\
         edition = \"2024\"\n\
         publish = false\n\n\
         [[bin]]\n\
         name = {name:?}\n\
         path = \"main.rs\"\n\n\
         [dependencies]\n\
         noctis = {{ path = {noctis_dir:?} }}\n\
         {dependency}\n\n\
         [workspace]\n"
    )
}

/// The generated client crate's `Cargo.toml`.
///
/// It depends on the `runtime` and the plugin, never on `noctis`, so the linked
/// plugin's `noctis` (with `default-features = false`) keeps the editor out of a
/// shipped game.
fn client_manifest(name: &str, runtime_dir: &Path, info: &PluginInfo) -> String {
    let dependency = info.dependency();
    format!(
        "[package]\n\
         name = {name:?}\n\
         version = \"0.0.0\"\n\
         edition = \"2024\"\n\
         publish = false\n\n\
         [[bin]]\n\
         name = {name:?}\n\
         path = \"main.rs\"\n\n\
         [dependencies]\n\
         runtime = {{ path = {runtime_dir:?} }}\n\
         {dependency}\n\n\
         [workspace]\n"
    )
}

/// The error when a project the tool must link has no Rust plugin.
fn no_rust_plugin(project: &Project) -> String {
    format!(
        "the project {:?} has no Cargo.toml (no Rust plugin)",
        project.name()
    )
}

/// The generated tool crate's `main.rs`.
fn tool_main(plugin_path: &str) -> String {
    format!(
        "//! Generated by `noctis`: the project's plugin-aware tool binary.\n\
         //!\n\
         //! Do not edit; it is rewritten from the project's `Cargo.toml` on every build.\n\n\
         fn main() -> std::process::ExitCode {{\n    \
         noctis::dispatch_with_plugin({plugin_path}())\n}}\n"
    )
}

/// The generated client crate's `main.rs`.
fn client_main(plugin_path: &str) -> String {
    format!(
        "//! Generated by `noctis`: the project's plugin-linked game client.\n\
         //!\n\
         //! Do not edit; it is rewritten from the project's `Cargo.toml` on every build.\n\n\
         mod embedded {{\n    \
         use runtime::EmbeddedScript;\n    \
         include!(\"embedded.rs\");\n}}\n\n\
         fn main() -> std::process::ExitCode {{\n    \
         runtime::cli::run_cli_with_plugin(\n        \
         std::env::args().skip(1).collect(),\n        \
         embedded::EMBEDDED,\n        \
         &{plugin_path}(),\n    )\n}}\n"
    )
}

/// A generated crate's `build.rs`: on Windows, reserve a large main-thread stack,
/// because the editor's extension host and the game's scripting run a
/// stack-hungry JavaScript engine (the Windows default main-thread stack is
/// only 1 MiB).
const BUILD_RS: &str = r#"fn main() {
    if std::env::var("CARGO_CFG_WINDOWS").is_ok() {
        println!("cargo:rustc-link-arg-bins=/STACK:16777216");
    }
}
"#;

/// The generated client's `embedded.rs`: the scripts baked into the binary by
/// absolute path, so a release game carries no loose `scripts/`.
fn embedded_rs(scripts: &[ExportFile]) -> String {
    let mut out = String::from("pub static EMBEDDED: &[EmbeddedScript] = &[\n");
    for file in scripts {
        let source = absolute(&file.source);
        out.push_str(&format!(
            "    EmbeddedScript {{ path: {:?}, bytes: include_bytes!({:?}) }},\n",
            file.relative, source
        ));
    }
    out.push_str("];\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> PluginInfo {
        PluginInfo {
            noctis_dir: PathBuf::from("/engine/crates/noctis"),
            runtime_dir: PathBuf::from("/engine/crates/runtime"),
            plugin_path: "my_game::noctis_plugin".to_string(),
            package: "my-game".to_string(),
            plugin_dir: PathBuf::from("/project/game"),
        }
    }

    #[test]
    fn tool_crate_names_are_sanitized() {
        assert_eq!(tool_crate_name("Showcase"), "noctis-tool-showcase");
        assert_eq!(tool_crate_name("My Game!"), "noctis-tool-my-game");
        assert_eq!(tool_crate_name("--weird--"), "noctis-tool-weird");
        assert_eq!(tool_crate_name("!!!"), "noctis-tool-game");
        assert_eq!(client_crate_name("Showcase"), "noctis-client-showcase");
    }

    #[test]
    fn the_tool_manifest_names_noctis_and_the_plugin() {
        let manifest = tool_manifest(
            "noctis-tool-game",
            Path::new("/engine/crates/noctis"),
            &info(),
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
            manifest.contains("my_game = { package = \"my-game\", path = \"/project/game\" }"),
            "{manifest}"
        );
        assert!(manifest.contains("[workspace]"), "{manifest}");
    }

    #[test]
    fn the_client_manifest_links_the_runtime_not_noctis() {
        let manifest = client_manifest(
            "noctis-client-game",
            Path::new("/engine/crates/runtime"),
            &info(),
        );
        assert!(
            manifest.contains("runtime = { path = \"/engine/crates/runtime\" }"),
            "{manifest}"
        );
        assert!(
            !manifest.contains("noctis = "),
            "no direct noctis: {manifest}"
        );
        assert!(
            manifest.contains("my_game = { package = \"my-game\", path = \"/project/game\" }"),
            "{manifest}"
        );
    }

    #[test]
    fn a_matching_package_name_needs_no_rename() {
        let mut info = info();
        info.plugin_path = "mygame::noctis_plugin".to_string();
        info.package = "mygame".to_string();
        let manifest = tool_manifest(
            "noctis-tool-game",
            Path::new("/engine/crates/noctis"),
            &info,
        );
        assert!(
            manifest.contains("mygame = { path = \"/project/game\" }"),
            "{manifest}"
        );
    }

    #[test]
    fn the_mains_dispatch_with_the_plugin() {
        assert!(
            tool_main("my_game::noctis_plugin")
                .contains("noctis::dispatch_with_plugin(my_game::noctis_plugin())"),
            "tool main"
        );
        let client = client_main("my_game::noctis_plugin");
        assert!(
            client.contains("runtime::cli::run_cli_with_plugin("),
            "{client}"
        );
        assert!(client.contains("&my_game::noctis_plugin()"), "{client}");
        assert!(client.contains("include!(\"embedded.rs\")"), "{client}");
    }

    #[test]
    fn embedded_lists_each_script_by_absolute_path() {
        let scripts = vec![ExportFile {
            relative: "scripts/a.js".to_string(),
            source: PathBuf::from("scripts/a.js"),
        }];
        let generated = embedded_rs(&scripts);
        assert!(generated.contains("path: \"scripts/a.js\""), "{generated}");
        assert!(generated.contains("include_bytes!("), "{generated}");
        assert_eq!(
            embedded_rs(&[]),
            "pub static EMBEDDED: &[EmbeddedScript] = &[\n];\n"
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

        // No Cargo.toml: nothing to delegate.
        std::fs::write(dir.join("noctis.toml"), "[project]\nname = \"T\"\n").expect("manifest");
        let project = dir.to_string_lossy().into_owned();
        assert!(plugin_project(&args(&["run", "--project", &project])).is_none());

        // A root Cargo.toml: delegate run/editor/build, but not unrelated commands.
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("crate manifest");
        assert!(plugin_project(&args(&["run", "--project", &project])).is_some());
        assert!(plugin_project(&args(&["editor", "--project", &project])).is_some());
        assert!(plugin_project(&args(&["build", "--project", &project])).is_some());
        assert!(plugin_project(&args(&["rename", "--project", &project])).is_none());
        assert!(plugin_project(&args(&["run"])).is_none());

        std::fs::remove_dir_all(&dir).ok();
    }
}
