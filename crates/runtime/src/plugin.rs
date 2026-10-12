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

use crate::export::{
    ExportFile, ExportReport, absolute, executable_name, profile_dir, workspace_root,
};

/// The subdirectory of a project's `.noctis` the generated tool lives in.
pub const TOOL_DIR: &str = ".noctis/tool";

/// The subdirectory of a project's `.noctis` the generated client lives in.
pub const CLIENT_DIR: &str = ".noctis/client";

/// The subdirectory of a project's `.noctis` the generated **web** client lives in.
pub const WEB_DIR: &str = ".noctis/web";

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
    // `--release` asks for an optimized plugin tool. It only applies to `run`
    // (an interactive game); the tool itself accepts the flag as a no-op.
    let release = args.first().map(String::as_str) == Some("run")
        && args.iter().any(|arg| arg == "--release");
    let profile = if release {
        Profile::Release
    } else {
        Profile::Dev
    };
    let binary = build_tool(project, profile)?;
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
    generate(
        &dir,
        &name,
        &manifest,
        "main.rs",
        &main,
        &[("build.rs", BUILD_RS)],
    )?;
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
        "main.rs",
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

/// Write a generated crate (`Cargo.toml`, an entry file, and any extra files).
fn generate(
    dir: &Path,
    name: &str,
    manifest: &str,
    entry_name: &str,
    entry: &str,
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
    write(entry_name, entry)?;
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

/// One project in a web build: its bundle key, a display label, and — when it
/// carries Rust — the plugin to link.
struct WebEntry {
    /// The bundle key: the project directory, sanitized. It names the page's
    /// option, the `<key>.zip` bundle, and the `start` dispatch arm.
    key: String,
    /// The display label (the project's name).
    label: String,
    /// The plugin, when the project carries Rust (`None` runs the stock client).
    plugin: Option<PluginLink>,
}

/// A project's plugin, as the generated crate needs it.
struct PluginLink {
    /// The dependency line, e.g. `rustball = { path = "…" }`.
    dependency: String,
    /// The call path, e.g. `rustball::noctis_plugin`.
    call: String,
}

/// The key, label, and plugin link for each project.
///
/// The key is the project directory, sanitized; a project with a root
/// `Cargo.toml` links its plugin, one without runs the stock client.
fn web_entries(projects: &[Project]) -> Result<Vec<WebEntry>, String> {
    let mut entries: Vec<WebEntry> = Vec::with_capacity(projects.len());
    for project in projects {
        let key = sanitize(&project_dir_name(project));
        if entries.iter().any(|entry| entry.key == key) {
            return Err(format!("two projects share the web key {key:?}"));
        }
        let plugin = PluginInfo::resolve(project).ok().map(|info| PluginLink {
            dependency: info.dependency(),
            call: info.plugin_path.clone(),
        });
        entries.push(WebEntry {
            key,
            label: project.name().to_string(),
            plugin,
        });
    }
    Ok(entries)
}

/// A project's directory name — its web key before sanitizing.
fn project_dir_name(project: &Project) -> String {
    project
        .root()
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_else(|| project.name())
        .to_string()
}

/// Generate and build a **browser** client for one or more projects, returning
/// the built `.wasm`.
///
/// Generates a `cdylib` crate under `dir` whose `start(canvas_id, project,
/// bundle_url)` dispatches by project name; it links `web` (without its `entry`
/// feature) and each project's plugin, then compiles for
/// `wasm32-unknown-unknown`.
fn build_web(entries: &[WebEntry], dir: &Path, profile: Profile) -> Result<PathBuf, String> {
    let root = workspace_root().ok_or_else(|| {
        "could not locate the engine workspace (a web build needs it)".to_string()
    })?;
    let web_dir = absolute(&root.join("crates").join("web"));
    let wasm_bindgen = locked_version(&root, "wasm-bindgen").ok_or_else(|| {
        "the engine's Cargo.lock does not pin wasm-bindgen (a web build needs it)".to_string()
    })?;

    let name = "noctis-web";
    let manifest = web_manifest(entries, &web_dir, &wasm_bindgen);
    let lib = web_lib(entries);
    generate(dir, name, &manifest, "lib.rs", &lib, &[])?;
    cargo_build_wasm(dir, "noctis_web", profile)
}

/// Build one or more projects into a single browser client: a wasm module whose
/// `start(canvas_id, project, bundle_url)` runs the chosen project, a `<key>.zip`
/// bundle per project, and a generated `index.html` that selects between them.
///
/// A project that carries Rust links its plugin; one that does not runs the stock
/// (no-plugin) client. All are staged in `destination`.
pub fn export_web(
    projects: &[Project],
    destination: &Path,
    profile: Profile,
) -> Result<ExportReport, String> {
    let Some(first) = projects.first() else {
        return Err("a web build needs at least one project".to_string());
    };
    let root =
        workspace_root().ok_or_else(|| "could not locate the engine workspace".to_string())?;
    let entries = web_entries(projects)?;

    let wasm = build_web(&entries, &first.dir(WEB_DIR), profile)?;

    fs::create_dir_all(destination)
        .map_err(|error| format!("could not create {}: {error}", destination.display()))?;
    run_wasm_bindgen(destination, &wasm)?;

    // Each project ships as the bundle its client fetches.
    let mut files = vec!["web.js".to_string(), "web_bg.wasm".to_string()];
    for (project, entry) in projects.iter().zip(&entries) {
        let bundle = project.bundle().map_err(|error| error.to_string())?;
        let zip = destination.join(format!("{}.zip", entry.key));
        fs::write(&zip, &bundle)
            .map_err(|error| format!("could not write {}: {error}", zip.display()))?;
        files.push(format!("{}.zip", entry.key));
    }

    // The generated host page (with the project selector) and the Slag `env` shim.
    let env = root.join("crates").join("web").join("web").join("env.js");
    fs::copy(&env, destination.join("env.js"))
        .map_err(|error| format!("could not copy {}: {error}", env.display()))?;
    // A content hash of the module versions the page's URLs, so a rebuild is not
    // served from a stale cache.
    let wasm_bytes = fs::read(destination.join("web_bg.wasm"))
        .map_err(|error| format!("could not read web_bg.wasm: {error}"))?;
    let version = content_hash(&wasm_bytes);
    fs::write(
        destination.join("index.html"),
        index_html(&entries, &version),
    )
    .map_err(|error| format!("could not write index.html: {error}"))?;
    files.push("env.js".to_string());
    files.push("index.html".to_string());

    Ok(ExportReport {
        directory: destination.to_path_buf(),
        executable: destination.join("web_bg.wasm"),
        scene: destination.join(format!("{}.zip", entries[0].key)),
        manifest: destination.join("index.html"),
        project_manifest: None,
        files,
    })
}

/// Build a generated web crate for `wasm32`, returning its `.wasm`.
fn cargo_build_wasm(dir: &Path, lib_name: &str, profile: Profile) -> Result<PathBuf, String> {
    let dir = absolute(dir);
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .arg("build")
        .arg("--manifest-path")
        .arg(dir.join("Cargo.toml"))
        .arg("--lib")
        .arg("--target")
        .arg("wasm32-unknown-unknown");
    if profile == Profile::Release {
        command.arg("--release");
    }
    match command.status() {
        Ok(status) if status.success() => {}
        Ok(status) => {
            return Err(format!(
                "building the project's web client failed ({status})"
            ));
        }
        Err(error) => {
            return Err(format!(
                "could not run cargo to build the web client: {error}"
            ));
        }
    }
    let wasm = dir
        .join("target")
        .join("wasm32-unknown-unknown")
        .join(profile_dir(profile))
        .join(format!("{lib_name}.wasm"));
    if !wasm.is_file() {
        return Err(format!(
            "the built web client was not found at {}",
            wasm.display()
        ));
    }
    Ok(wasm)
}

/// Run the `wasm-bindgen` CLI over `wasm`, writing the JS glue and a trimmed
/// module named `web.js` / `web_bg.wasm` into `out_dir` — the names
/// `crates/web/web/index.html` imports.
fn run_wasm_bindgen(out_dir: &Path, wasm: &Path) -> Result<(), String> {
    let bindgen = std::env::var_os("WASM_BINDGEN").unwrap_or_else(|| "wasm-bindgen".into());
    let mut command = Command::new(bindgen);
    command
        .arg("--target")
        .arg("web")
        .arg("--out-name")
        .arg("web")
        .arg("--out-dir")
        .arg(out_dir)
        .arg(wasm);
    match command.status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!(
            "wasm-bindgen failed ({status}); is the `wasm-bindgen` CLI installed and \
             matching the engine's version?"
        )),
        Err(error) => Err(format!("could not run wasm-bindgen: {error}")),
    }
}

/// The generated web crate's `Cargo.toml`.
///
/// It links the `web` crate *without* its `entry` feature (the generated crate
/// supplies its own `start`), each project's plugin, and pins `wasm-bindgen` to
/// the engine's locked version so the glue matches the CLI.
fn web_manifest(entries: &[WebEntry], web_dir: &Path, wasm_bindgen: &str) -> String {
    let mut plugins = String::new();
    for entry in entries {
        if let Some(plugin) = &entry.plugin {
            plugins.push_str(&plugin.dependency);
            plugins.push('\n');
        }
    }
    format!(
        "[package]\n\
         name = \"noctis-web\"\n\
         version = \"0.0.0\"\n\
         edition = \"2024\"\n\
         publish = false\n\n\
         [lib]\n\
         crate-type = [\"cdylib\"]\n\
         path = \"lib.rs\"\n\n\
         [dependencies]\n\
         web = {{ path = {web_dir:?}, default-features = false }}\n\
         wasm-bindgen = {{ version = \"={wasm_bindgen}\" }}\n\
         {plugins}\n\
         [workspace]\n"
    )
}

/// The generated web crate's `lib.rs`: `start(canvas_id, project, bundle_url)`
/// dispatches to the chosen project's client.
fn web_lib(entries: &[WebEntry]) -> String {
    let mut arms = String::new();
    for entry in entries {
        let key = &entry.key;
        match &entry.plugin {
            Some(plugin) => {
                let call = &plugin.call;
                arms.push_str(&format!(
                    "        {key:?} => web::run_bundle_with(canvas_id, bundle_url, {call}()),\n"
                ));
            }
            None => arms.push_str(&format!(
                "        {key:?} => web::run_bundle(canvas_id, bundle_url),\n"
            )),
        }
    }
    format!(
        "//! Generated by `noctis`: a browser client for one or more projects.\n\
         //!\n\
         //! Do not edit; it is rewritten on every build.\n\n\
         #[wasm_bindgen::prelude::wasm_bindgen]\n\
         pub fn start(canvas_id: &str, project: &str, bundle_url: &str) {{\n    \
         match project {{\n{arms}        _ => {{}}\n    }}\n}}\n"
    )
}

/// The generated host page: a project `<select>` that reloads with `?p=<key>`
/// and boots that project's bundle.
///
/// `version` is a content hash, appended to the glue, the module, and the
/// bundles, so a rebuild is not served from a stale cache.
fn index_html(entries: &[WebEntry], version: &str) -> String {
    let mut options = String::new();
    for entry in entries {
        options.push_str(&format!(
            "        <option value=\"{}\">{}</option>\n",
            entry.key,
            escape_html(&entry.label)
        ));
    }
    let mut page = String::new();
    page.push_str(PAGE_HEAD);
    page.push_str(&options);
    page.push_str(PAGE_MID);
    page.push_str(version);
    page.push_str(PAGE_SCRIPT);
    page.push_str(&entries[0].key);
    page.push_str(PAGE_TAIL);
    page
}

/// The generated page, split around the project list, the version, and the
/// default key.
const PAGE_HEAD: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Noctis</title>
    <style>
      :root { color-scheme: dark; }
      html, body { margin: 0; height: 100%; background: #0b0d10; color: #8a93a0;
        font: 13px/1.4 ui-monospace, monospace; display: grid;
        grid-template-rows: auto 1fr; }
      header { display: flex; gap: 12px; align-items: center; padding: 10px 16px;
        border-bottom: 1px solid #1c2128; }
      label { display: inline-flex; gap: 6px; align-items: center; }
      select { background: #12161c; color: inherit; border: 1px solid #2a323c;
        border-radius: 6px; padding: 5px 8px; font: inherit; }
      main { display: grid; place-items: center; padding: 16px; }
      #noctis-canvas { display: block; width: min(100%, 1280px); aspect-ratio: 16 / 9;
        max-height: 100%; background: #000; border-radius: 8px;
        box-shadow: 0 8px 40px rgb(0 0 0 / 60%); outline: none; }
      #noctis-canvas:focus-visible { box-shadow: 0 8px 40px rgb(0 0 0 / 60%), 0 0 0 2px #4c8dff; }
    </style>
  </head>
  <body>
    <header>
      <label>Project
        <select id="project">
"#;

const PAGE_MID: &str = r#"        </select>
      </label>
    </header>
    <main><canvas id="noctis-canvas" tabindex="0"></canvas></main>

    <!-- Slag (the script engine), compiled to wasm, imports its clock from a
         module named `env`; this map points it at the shim alongside. -->
    <script type="importmap">
      { "imports": { "env": "./env.js" } }
    </script>

    <script type="module">
      const V = ""#;

const PAGE_SCRIPT: &str = r#"";
      // Everything is versioned by `V` so a rebuild is never served from cache.
      const { default: init, start } = await import("./web.js?v=" + V);
      const select = document.getElementById("project");
      const canvas = document.getElementById("noctis-canvas");
      canvas.addEventListener("pointerdown", () => canvas.focus());

      const picked =
        new URLSearchParams(location.search).get("p") ?? ""#;

const PAGE_TAIL: &str = r#"";
      if ([...select.options].some((o) => o.value === picked)) select.value = picked;
      select.addEventListener("change", () => { location.search = "?p=" + select.value; });

      await init(new URL("./web_bg.wasm?v=" + V, import.meta.url));
      // start(canvasId, project, bundleUrl) runs the bundle; it never returns.
      start(canvas.id, select.value, "./" + select.value + ".zip?v=" + V);
    </script>
  </body>
</html>
"#;

/// A short content hash (FNV-1a) of `bytes`, for cache-busting URLs.
fn content_hash(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Escape the few characters that matter inside an HTML text node.
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The version `name` is locked to in the engine's `Cargo.lock`.
fn locked_version(root: &Path, name: &str) -> Option<String> {
    let text = fs::read_to_string(root.join("Cargo.lock")).ok()?;
    let needle = format!("name = {name:?}");
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if line.trim() != needle {
            continue;
        }
        for next in lines.by_ref() {
            if let Some(value) = next.trim().strip_prefix("version = ") {
                return Some(value.trim_matches('"').to_string());
            }
        }
    }
    None
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

    /// A web entry for a project with (or without) a plugin.
    fn web_entry(key: &str, label: &str, plugin: Option<(&str, &str)>) -> WebEntry {
        WebEntry {
            key: key.to_string(),
            label: label.to_string(),
            plugin: plugin.map(|(dependency, call)| PluginLink {
                dependency: dependency.to_string(),
                call: call.to_string(),
            }),
        }
    }

    fn web_fixtures() -> Vec<WebEntry> {
        vec![
            web_entry("showcase", "Noctis Showcase", None),
            web_entry(
                "rustball",
                "Rust Ball",
                Some((
                    "my_game = { package = \"my-game\", path = \"/project/game\" }",
                    "my_game::noctis_plugin",
                )),
            ),
        ]
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
    fn the_web_manifest_links_web_without_its_entry_and_each_plugin() {
        let manifest = web_manifest(&web_fixtures(), Path::new("/engine/crates/web"), "0.2.129");
        assert!(manifest.contains("crate-type = [\"cdylib\"]"), "{manifest}");
        assert!(manifest.contains("path = \"lib.rs\""), "{manifest}");
        assert!(
            manifest.contains("web = { path = \"/engine/crates/web\", default-features = false }"),
            "{manifest}"
        );
        assert!(
            manifest.contains("wasm-bindgen = { version = \"=0.2.129\" }"),
            "{manifest}"
        );
        assert!(
            manifest.contains("my_game = { package = \"my-game\", path = \"/project/game\" }"),
            "{manifest}"
        );
        assert!(manifest.contains("[workspace]"), "{manifest}");
    }

    #[test]
    fn the_web_lib_dispatches_by_project() {
        let lib = web_lib(&web_fixtures());
        assert!(
            lib.contains("pub fn start(canvas_id: &str, project: &str, bundle_url: &str)"),
            "{lib}"
        );
        assert!(
            lib.contains("\"showcase\" => web::run_bundle(canvas_id, bundle_url)"),
            "{lib}"
        );
        assert!(
            lib.contains(
                "\"rustball\" => web::run_bundle_with(canvas_id, bundle_url, my_game::noctis_plugin())"
            ),
            "{lib}"
        );
    }

    #[test]
    fn the_page_lists_every_project_and_versions_its_assets() {
        let html = index_html(&web_fixtures(), "deadbeef");
        assert!(
            html.contains("<option value=\"showcase\">Noctis Showcase</option>"),
            "{html}"
        );
        assert!(
            html.contains("<option value=\"rustball\">Rust Ball</option>"),
            "{html}"
        );
        assert!(html.contains(r#".get("p") ?? "showcase""#), "{html}");
        // The glue, module, and bundles are versioned by the content hash.
        assert!(html.contains(r#"const V = "deadbeef""#), "{html}");
        assert!(html.contains(r#"import("./web.js?v=" + V)"#), "{html}");
        assert!(html.contains(r#".zip?v=" + V"#), "{html}");
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
