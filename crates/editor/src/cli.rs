//! The `noctis editor` command line.
//!
//! The editor is deliberately tiny and dependency-free beyond the editor crate:
//! it parses a handful of flags by hand, applies a JSON batch of
//! [`editor::EditorCommand`]s, and prints the resulting entity list as JSON. It
//! never panics — every failure becomes a non-zero exit and a clear message on
//! stderr.
//!
//! The window is the default (that is usually why you open the editor). Pass
//! `--headless`, or ask for a batch operation (`--command`, `--out`,
//! `--save-prefabs`, `--print-schema`), to run without one.
//!
//! ```text
//! noctis editor --project game
//! noctis editor --headless --scene level.json --command edits.json --out level.json
//! noctis editor --print-schema
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use ecs::app::{NullPlugin, Plugin};
use ecs::reflect::ComponentRegistry;
use ecs::scene::SceneMode;
use platform::WindowConfig;
use project::Project;

use crate::extension::{ExtensionReload, ExtensionWatch};
use crate::{Editor, ExtensionHost, command_schema, parse_commands};

/// The prefix on every diagnostic message.
const PROGRAM: &str = "noctis";

const USAGE: &str = "\
noctis editor — open the editor, or build scenes from JSON commands

USAGE:
    noctis editor [OPTIONS]

OPTIONS:
    -s, --scene <PATH>      Open a scene; if the file does not exist, start a new
                            empty scene instead (written when you pass --out)
        --project <DIR>     Open a project (its noctis.toml); --scene is then a
                            project-relative path, and the project's default
                            scene is used when --scene is omitted
        --mode <2d|3d>      Presentation mode for a scene created from scratch
                            (overrides the project's default; an existing scene
                            keeps the mode stored in its file)
    -c, --command <SRC>     Apply a JSON command batch. <SRC> is a file path,
                            the literal `-` for stdin, or inline JSON.
        --out <PATH>        Save the resulting scene to <PATH>
        --headless          Do not open a window (the window is the default)
        --testscene         Load the engine's all-features test scene
        --save-prefabs      Write the prefab library to the project's prefabs/
                            directory (requires --project)
        --controls <PATH>   Load camera-navigation bindings (JSON)
        --extensions <DIR>  Scan only this directory for editor extensions
                            (repeatable; replaces the default search set)
        --no-extensions     Scan no extension directories
        --watch             Reload extensions when their files change (windowed)
        --run <LINE>        Dispatch an extension verb line, e.g.
                            --run \"scatter 5\" (repeatable)
        --print-schema      Print the JSON command schema and exit
    -h, --help              Print this help and exit

With no options, the editor opens the (empty) scene in a window. `--scene` and
`--command` are both optional.

The batch is a JSON array of commands; a single command object is also
accepted. In a headless run the resulting entity list is printed as JSON.";

/// A top-level CLI failure, distinguished so usage problems can exit `2`.
#[derive(Debug)]
enum CliError {
    /// The command line was malformed.
    Usage(String),
    /// A runtime failure (bad scene, command, or I/O).
    Message(String),
}

/// Run the editor command line and return the process exit code.
pub fn run_cli(args: Vec<String>) -> ExitCode {
    run_cli_with_plugin(args, &NullPlugin)
}

/// Run the editor command line with a project's [`Plugin`].
///
/// The per-project binary a Rust build produces calls this: the plugin's
/// custom components are registered so the editor loads, edits, and saves scenes
/// that use them, and lists them in the Add-component menu.
pub fn run_cli_with_plugin(args: Vec<String>, plugin: &dyn Plugin) -> ExitCode {
    let options = match parse_args(&args) {
        Ok(options) => options,
        Err(error) => return report(error),
    };

    if opens_a_window(&options) {
        return match run_windowed(&options, plugin) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => report(error),
        };
    }

    match run(&args, plugin) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => report(error),
    }
}

fn report(error: CliError) -> ExitCode {
    match error {
        CliError::Usage(message) => {
            eprintln!("{PROGRAM}: {message}\n");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
        CliError::Message(message) => {
            eprintln!("{PROGRAM}: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Whether the invocation should open the editor window.
///
/// The window is the default; `--headless` opts out. `--help` and
/// `--print-schema` print and exit, so they never open one.
fn opens_a_window(options: &Options) -> bool {
    !options.headless && !options.help && !options.print_schema
}

#[derive(Default, Debug, PartialEq)]
struct Options {
    scene: Option<String>,
    project: Option<String>,
    mode: Option<SceneMode>,
    command: Option<String>,
    out: Option<String>,
    controls: Option<String>,
    headless: bool,
    testscene: bool,
    save_prefabs: bool,
    extensions: Vec<String>,
    no_extensions: bool,
    watch: bool,
    run: Vec<String>,
    print_schema: bool,
    help: bool,
}

fn run(args: &[String], plugin: &dyn Plugin) -> Result<String, CliError> {
    let options = parse_args(args)?;

    if options.help {
        return Ok(USAGE.to_string());
    }
    if options.print_schema {
        let schema = command_schema();
        return serde_json::to_string_pretty(&schema)
            .map_err(|e| CliError::Message(format!("failed to render schema: {e}")));
    }

    let mut project = open_project(&options)?;
    let mut editor = build_editor(&options, project.as_ref(), plugin)?;

    // Extensions run after the scene and any command batch, and before `--out`,
    // so an extension's edits (`--run`, a `"ready"` hook) are saved too.
    let _ = attach_extensions(&mut editor, &options, project.as_ref())?;
    finalize(&mut editor, &options, project.as_mut())?;

    let entities = editor.list_entities();
    serde_json::to_string_pretty(&entities)
        .map_err(|e| CliError::Message(format!("failed to serialize entities: {e}")))
}

/// Build the editor for the options: a `--testscene`, a scene (from `--scene`,
/// resolved against `--project`, or the project's default), then any command
/// batch, saving when `--out` is given.
fn build_editor(
    options: &Options,
    project: Option<&Project>,
    plugin: &dyn Plugin,
) -> Result<Editor, CliError> {
    // The plugin's custom components must be registered before any scene that
    // uses them loads, and are what the Add-component menu lists.
    let mut components = ComponentRegistry::new();
    plugin.register(&mut components);
    let mut editor = Editor::with_component_registry(components.clone());

    // A project owns one shared asset registry: import every asset file up front
    // so the scene's references resolve. Without a project the editor keeps the
    // registry's placeholder defaults.
    if let Some(project) = project {
        let imported = project
            .import_assets()
            .map_err(|e| CliError::Message(format!("failed to import project assets: {e}")))?;
        for error in &imported.errors {
            eprintln!(
                "{PROGRAM}: warning: asset '{}' failed to import: {}",
                error.path.as_str(),
                error.message
            );
        }
        eprintln!(
            "{PROGRAM}: imported {} asset file(s) ({} failed)",
            imported.files,
            imported.errors.len()
        );
        *editor.assets_mut() = imported.registry;

        // Load the project's prefabs so a command batch can instantiate them.
        let prefabs = project
            .load_prefabs()
            .map_err(|e| CliError::Message(format!("failed to load project prefabs: {e}")))?;
        for prefab in prefabs {
            editor.add_prefab(prefab);
        }
    }

    if options.testscene {
        let (scene, assets) = testscene::build();
        *editor.scene_mut() = scene.with_component_registry(components.clone());
        *editor.assets_mut() = assets;
        editor.scene_mut().update_transforms();
    } else if let Some(scene) = resolve_scene(options, project) {
        let display = scene.display().to_string();
        // A scene that does not exist yet is created in the effective mode.
        let mode = effective_mode(options, project);
        editor
            .open_or_new_with_mode(&scene, mode)
            .map_err(|e| CliError::Message(format!("failed to open scene {display:?}: {e}")))?;
    } else if let Some(project) = project {
        // A project with no scene and no default scene: start one named after it.
        editor.new_scene_with_mode(project.name(), effective_mode(options, Some(project)));
    } else if let Some(mode) = options.mode {
        // A bare invocation starts a fresh scene; `--mode` applies to it. There is
        // no history yet, so assigning the field directly records no undo step.
        editor.scene_mut().mode = mode;
    }

    if let Some(source) = &options.command {
        let json = read_commands(source)?;
        let commands = parse_commands(&json)
            .map_err(|e| CliError::Message(format!("invalid command batch: {e}")))?;
        for (index, command) in commands.into_iter().enumerate() {
            editor
                .apply(command)
                .map_err(|e| CliError::Message(format!("command #{} failed: {e}", index + 1)))?;
        }
    }

    Ok(editor)
}

/// Apply `--out` and `--save-prefabs`, after extensions have run.
fn finalize(
    editor: &mut Editor,
    options: &Options,
    project: Option<&mut Project>,
) -> Result<(), CliError> {
    if let Some(out) = &options.out {
        editor
            .save(out)
            .map_err(|e| CliError::Message(format!("failed to save scene {out:?}: {e}")))?;
    }

    if options.save_prefabs {
        let project = project.ok_or_else(|| {
            CliError::Usage(
                "--save-prefabs requires --project (prefabs live in the project)".into(),
            )
        })?;
        for prefab in editor.prefabs() {
            project
                .save_prefab(prefab)
                .map_err(|e| CliError::Message(format!("failed to save prefab: {e}")))?;
        }
    }

    Ok(())
}

/// Discover and load the enabled extensions, fire their `"ready"` hook and any
/// `--run` lines, and hand back the live host (for a windowed run) with the
/// config the Extensions screen writes.
///
/// `None` when there is nothing to load or run. Extension output (replies, logs,
/// load warnings) goes to stderr, so stdout stays the scene JSON.
fn attach_extensions(
    editor: &mut Editor,
    options: &Options,
    project: Option<&Project>,
) -> Result<Option<LoadedExtensions>, CliError> {
    if options.no_extensions {
        return Ok(None);
    }

    // `--extensions` replaces the default search set; otherwise the environment,
    // the executable and working directories, and the project's own directories.
    let dirs: Vec<PathBuf> = if options.extensions.is_empty() {
        let project_dirs = project.map(Project::extension_dirs).unwrap_or_default();
        extensions::Loader::default_dirs(&project_dirs)
    } else {
        options.extensions.iter().map(PathBuf::from).collect()
    };
    let loader = extensions::Loader::discover(&dirs);
    for error in loader.errors() {
        eprintln!(
            "{PROGRAM}: warning: extension {}: {}",
            error.path.display(),
            error.message
        );
    }

    let config_path = extensions::default_config_path();
    let config = match extensions::ExtensionConfig::load(&config_path) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{PROGRAM}: warning: extension config: {error}");
            extensions::ExtensionConfig::default()
        }
    };
    let project_disabled: Vec<String> = project
        .map(|project| project.disabled_extensions().to_vec())
        .unwrap_or_default();
    let enabled = |id: &str| config.is_enabled(id) && !project_disabled.iter().any(|d| d == id);

    let any_enabled = loader
        .extensions()
        .iter()
        .any(|manifest| enabled(&manifest.id));
    // Under `--watch`, keep the (possibly empty) host so a newly created
    // extension is picked up by the watcher without a restart.
    if !any_enabled && options.run.is_empty() && !options.watch {
        return Ok(None);
    }

    let mut host = ExtensionHost::new().map_err(|error| {
        CliError::Message(format!("failed to start the extension host: {error}"))
    })?;
    host.load(&loader, enabled);
    for line in host.ready(editor) {
        eprintln!("{PROGRAM}: {line}");
    }
    for log in host.take_logs() {
        eprintln!("{PROGRAM}: log: {log}");
    }

    for line in &options.run {
        let reply = host.dispatch(editor, line).map_err(CliError::Message)?;
        if !reply.is_empty() {
            eprintln!("{PROGRAM}: {reply}");
        }
        for log in host.take_logs() {
            eprintln!("{PROGRAM}: log: {log}");
        }
    }

    for error in host.errors() {
        eprintln!("{PROGRAM}: warning: extension {error}");
    }

    Ok(Some(LoadedExtensions {
        host,
        config: config.clone(),
        config_path: config_path.clone(),
        dirs,
        disabled: project_disabled.clone(),
    }))
}

/// The loaded extension host plus the inputs a windowed `--watch` needs.
struct LoadedExtensions {
    host: ExtensionHost,
    config: extensions::ExtensionConfig,
    config_path: PathBuf,
    /// The directories the host was discovered from.
    dirs: Vec<PathBuf>,
    /// The ids the project disables.
    disabled: Vec<String>,
}

fn parse_args(args: &[String]) -> Result<Options, CliError> {
    let mut options = Options::default();
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) => (flag.to_string(), Some(value.to_string())),
            None => (arg.clone(), None),
        };

        match flag.as_str() {
            "--help" | "-h" => {
                reject_inline(&inline, &flag)?;
                options.help = true;
            }
            "--print-schema" | "--schema" => {
                reject_inline(&inline, &flag)?;
                options.print_schema = true;
            }
            "--headless" => {
                reject_inline(&inline, &flag)?;
                options.headless = true;
            }
            "--window" => {
                return Err(CliError::Usage(
                    "--window was removed: the editor opens a window by default, so pass \
                     --headless to run without one"
                        .into(),
                ));
            }
            "--testscene" => {
                reject_inline(&inline, &flag)?;
                options.testscene = true;
            }
            "--save-prefabs" => {
                reject_inline(&inline, &flag)?;
                options.save_prefabs = true;
            }
            "--scene" | "-s" => {
                let (value, consumed) = value_for(&inline, args, index, &flag)?;
                options.scene = Some(value);
                index = consumed;
            }
            "--project" => {
                let (value, consumed) = value_for(&inline, args, index, &flag)?;
                options.project = Some(value);
                index = consumed;
            }
            "--mode" => {
                let (value, consumed) = value_for(&inline, args, index, &flag)?;
                options.mode = Some(value.parse().map_err(CliError::Message)?);
                index = consumed;
            }
            "--command" | "--commands" | "-c" => {
                let (value, consumed) = value_for(&inline, args, index, &flag)?;
                options.command = Some(value);
                index = consumed;
            }
            "--out" | "--output" | "-o" => {
                let (value, consumed) = value_for(&inline, args, index, &flag)?;
                options.out = Some(value);
                index = consumed;
            }
            "--controls" | "--bindings" => {
                let (value, consumed) = value_for(&inline, args, index, &flag)?;
                options.controls = Some(value);
                index = consumed;
            }
            "--extensions" | "-e" => {
                let (value, consumed) = value_for(&inline, args, index, &flag)?;
                options.extensions.push(value);
                index = consumed;
            }
            "--no-extensions" => {
                reject_inline(&inline, &flag)?;
                options.no_extensions = true;
            }
            "--watch" => {
                reject_inline(&inline, &flag)?;
                options.watch = true;
            }
            "--run" => {
                let (value, consumed) = value_for(&inline, args, index, &flag)?;
                options.run.push(value);
                index = consumed;
            }
            other if other.starts_with('-') => {
                return Err(CliError::Usage(format!("unknown option {other:?}")));
            }
            other => {
                return Err(CliError::Usage(format!("unexpected argument {other:?}")));
            }
        }

        index += 1;
    }

    if options.watch && options.headless {
        return Err(CliError::Usage(
            "--watch needs a window; drop --headless".into(),
        ));
    }

    Ok(options)
}

/// The value for a value-taking option, from `--flag=value` or the next arg.
///
/// Returns the value and the index of the argument that supplied it.
fn value_for(
    inline: &Option<String>,
    args: &[String],
    index: usize,
    flag: &str,
) -> Result<(String, usize), CliError> {
    if let Some(value) = inline {
        return Ok((value.clone(), index));
    }
    match args.get(index + 1) {
        Some(value) => Ok((value.clone(), index + 1)),
        None => Err(CliError::Usage(format!("option {flag} requires a value"))),
    }
}

fn reject_inline(inline: &Option<String>, flag: &str) -> Result<(), CliError> {
    if inline.is_some() {
        Err(CliError::Usage(format!("option {flag} takes no value")))
    } else {
        Ok(())
    }
}

/// The mode for a scene created from scratch: `--mode`, else the project's
/// default, else 3D.
fn effective_mode(options: &Options, project: Option<&Project>) -> SceneMode {
    options
        .mode
        .or_else(|| project.map(Project::mode))
        .unwrap_or_default()
}

/// Open the project named by `--project`, if any.
fn open_project(options: &Options) -> Result<Option<Project>, CliError> {
    match &options.project {
        Some(dir) => Project::open(dir)
            .map(Some)
            .map_err(|e| CliError::Message(format!("failed to open project {dir:?}: {e}"))),
        None => Ok(None),
    }
}

/// The scene to open: `--scene` (resolved against the project when given), else
/// the project's default scene.
fn resolve_scene(options: &Options, project: Option<&Project>) -> Option<PathBuf> {
    if let Some(scene) = &options.scene {
        return Some(match project {
            Some(project) => project.resolve_path(scene),
            None => PathBuf::from(scene),
        });
    }
    project.and_then(Project::default_scene_file)
}

/// The window config: the project's `[window]`, else the editor default.
fn window_config(project: Option<&Project>) -> WindowConfig {
    let mut config = WindowConfig {
        title: "Noctis Editor".to_string(),
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
        // The viewport anti-aliases at the project's `[render] sample_count`.
        config.sample_count = project.manifest().render.sample_count;
    }
    config
}

/// Build the project's camera controls path, if it sets one.
fn project_controls_path(project: Option<&Project>) -> Option<String> {
    let project = project?;
    let controls = project.manifest().input.controls.as_ref()?;
    Some(
        project
            .resolve_path(controls)
            .to_string_lossy()
            .into_owned(),
    )
}

/// Build the scene from the options, then run it in a window until closed.
fn run_windowed(options: &Options, plugin: &dyn Plugin) -> Result<(), CliError> {
    let mut project = open_project(options)?;
    let mut editor = build_editor(options, project.as_ref(), plugin)?;
    // The scene the editor opened, so Save can write it back; the test scene and
    // a bare invocation have no file.
    let scene_path = if options.testscene {
        None
    } else {
        resolve_scene(options, project.as_ref())
    };
    let extensions = attach_extensions(&mut editor, options, project.as_ref())?;
    finalize(&mut editor, options, project.as_mut())?;
    let config = window_config(project.as_ref());

    let attached = extensions.map(|loaded| {
        let reload = watch_extensions(options, &loaded);
        crate::window::AttachedExtensions {
            host: loaded.host,
            config: loaded.config,
            config_path: loaded.config_path,
            reload,
        }
    });

    // `--controls` wins over the project's `[input].controls`.
    let controls = options
        .controls
        .clone()
        .or_else(|| project_controls_path(project.as_ref()));

    match controls {
        Some(path) => {
            let controls = crate::CameraControls::load(&path)
                .map_err(|e| CliError::Message(format!("failed to load controls {path:?}: {e}")))?;
            crate::window::run_with_controls(
                editor, config, controls, scene_path, project, attached,
            )
            .map_err(|e| CliError::Message(e.to_string()))
        }
        None => crate::window::run(editor, config, scene_path, project, attached)
            .map_err(|e| CliError::Message(e.to_string())),
    }
}

/// Start the `--watch` watcher over the loaded extension directories, if asked.
fn watch_extensions(options: &Options, loaded: &LoadedExtensions) -> Option<ExtensionReload> {
    if !options.watch {
        return None;
    }
    match ExtensionWatch::new(&loaded.dirs) {
        Ok(watch) => Some(ExtensionReload {
            watch,
            dirs: loaded.dirs.clone(),
            disabled: loaded.disabled.clone(),
        }),
        Err(error) => {
            eprintln!("{PROGRAM}: warning: extension watch: {error}");
            None
        }
    }
}

/// Read the command batch from a file, stdin (`-`), or the literal argument.
fn read_commands(source: &str) -> Result<String, CliError> {
    if source == "-" {
        use std::io::Read;
        let mut buffer = String::new();
        std::io::stdin()
            .read_to_string(&mut buffer)
            .map_err(|e| CliError::Message(format!("failed to read stdin: {e}")))?;
        return Ok(buffer);
    }
    let path = std::path::Path::new(source);
    if path.is_file() {
        std::fs::read_to_string(path)
            .map_err(|e| CliError::Message(format!("failed to read command file {source:?}: {e}")))
    } else {
        Ok(source.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// `run` with the null plugin, for the tests that do not exercise one.
    fn run_default(list: &[&str]) -> Result<String, CliError> {
        run(&args(list), &NullPlugin)
    }

    #[test]
    fn parses_scene_command_and_out() {
        let options = parse_args(&args(&[
            "--scene",
            "level.json",
            "--command",
            "edits.json",
            "--out",
            "out.json",
        ]))
        .unwrap();
        assert_eq!(options.scene.as_deref(), Some("level.json"));
        assert_eq!(options.command.as_deref(), Some("edits.json"));
        assert_eq!(options.out.as_deref(), Some("out.json"));
    }

    #[test]
    fn parses_equals_and_short_forms() {
        let options = parse_args(&args(&["-s=level.json", "-c=edits.json"])).unwrap();
        assert_eq!(options.scene.as_deref(), Some("level.json"));
        assert_eq!(options.command.as_deref(), Some("edits.json"));
    }

    #[test]
    fn parses_the_controls_path() {
        let options = parse_args(&args(&["--controls", "controls.json"])).unwrap();
        assert_eq!(options.controls.as_deref(), Some("controls.json"));
    }

    #[test]
    fn the_window_is_the_default_and_headless_opts_out() {
        assert!(opens_a_window(&parse_args(&args(&[])).unwrap()));
        assert!(opens_a_window(
            &parse_args(&args(&["--project", "game"])).unwrap()
        ));
        assert!(!opens_a_window(
            &parse_args(&args(&["--headless"])).unwrap()
        ));
    }

    #[test]
    fn commands_and_out_keep_the_window() {
        // `--command` and `--out` do not force headless: the window is the default
        // and `--headless` is how you opt out.
        for flags in [
            args(&["--command", "edits.json"]),
            args(&["--out", "out.json"]),
            args(&["--save-prefabs"]),
        ] {
            let options = parse_args(&flags).unwrap();
            assert!(opens_a_window(&options), "{flags:?} should open a window");
        }
        // Printing and exiting never opens one.
        for flags in [args(&["--print-schema"]), args(&["--help"])] {
            let options = parse_args(&flags).unwrap();
            assert!(
                !opens_a_window(&options),
                "{flags:?} should not open a window"
            );
        }
    }

    #[test]
    fn the_removed_window_flag_is_rejected_with_a_hint() {
        let err = parse_args(&args(&["--window"])).unwrap_err();
        match err {
            CliError::Usage(message) => assert!(message.contains("--headless"), "{message}"),
            other => panic!("expected a usage error, got {other:?}"),
        }
    }

    #[test]
    fn parses_a_project_and_a_scene() {
        let options = parse_args(&args(&[
            "--project",
            "game",
            "--scene",
            "scenes/main.scene.json",
        ]))
        .unwrap();
        assert_eq!(options.project.as_deref(), Some("game"));
        assert_eq!(options.scene.as_deref(), Some("scenes/main.scene.json"));
    }

    #[test]
    fn parses_the_scene_mode() {
        assert_eq!(
            parse_args(&args(&["--mode", "2d"])).unwrap().mode,
            Some(SceneMode::TwoD)
        );
        assert_eq!(
            parse_args(&args(&["--mode=3d"])).unwrap().mode,
            Some(SceneMode::ThreeD)
        );
        let err = parse_args(&args(&["--mode", "sideways"])).unwrap_err();
        assert!(matches!(err, CliError::Message(_)));
    }

    #[test]
    fn run_creates_a_scene_in_the_requested_mode() {
        let dir = std::env::temp_dir().join(format!(
            "noctis-mode-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let out = dir.join("menu.scene.json");

        run_default(&["--mode", "2d", "--out", out.to_str().unwrap()]).unwrap();
        let json = std::fs::read_to_string(&out).expect("scene written");
        assert!(json.contains("\"mode\": \"2d\""), "{json}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parses_save_prefabs() {
        assert!(parse_args(&args(&["--save-prefabs"])).unwrap().save_prefabs);
    }

    #[test]
    fn save_prefabs_without_a_project_is_a_usage_error() {
        let err = run_default(&["--save-prefabs"]).unwrap_err();
        assert!(matches!(err, CliError::Usage(_)));
    }

    #[test]
    fn print_schema_and_help_need_no_scene() {
        assert!(parse_args(&args(&["--print-schema"])).unwrap().print_schema);
        assert!(parse_args(&args(&["-h"])).unwrap().help);
    }

    #[test]
    fn unknown_option_is_a_usage_error() {
        let err = parse_args(&args(&["--nope"])).unwrap_err();
        assert!(matches!(err, CliError::Usage(_)));
    }

    #[test]
    fn missing_value_is_a_usage_error() {
        let err = parse_args(&args(&["--scene"])).unwrap_err();
        assert!(matches!(err, CliError::Usage(_)));
    }

    #[test]
    fn empty_invocation_opens_a_new_scene() {
        // No options is not an error: it opens an empty scene and prints `[]`.
        let options = parse_args(&args(&[])).unwrap();
        assert!(options.scene.is_none());
        let output = run_default(&[]).unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value.as_array().unwrap().len(), 0);
    }

    #[test]
    fn run_applies_inline_commands_and_returns_entities() {
        let output = run_default(&["--command", r#"[{ "op": "spawn", "name": "Sun" }]"#]).unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value.as_array().unwrap().len(), 1);
        assert_eq!(value[0]["name"], "Sun");
    }

    #[test]
    fn run_reports_bad_command_json() {
        let err = run_default(&["--command", "not json"]).unwrap_err();
        assert!(matches!(err, CliError::Message(_)));
    }

    #[test]
    fn run_reports_unknown_entity() {
        let err = run_default(&[
            "--command",
            r#"[{ "op": "rename", "entity": { "name": "ghost" }, "name": "x" }]"#,
        ])
        .unwrap_err();
        assert!(matches!(err, CliError::Message(_)));
    }

    #[test]
    fn print_schema_returns_valid_json() {
        let output = run_default(&["--print-schema"]).unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(value["commands"].is_array());
    }

    #[test]
    fn parses_extension_flags() {
        let options = parse_args(&args(&[
            "--extensions",
            "a",
            "--extensions=b",
            "--no-extensions",
            "--run",
            "scatter 3",
            "--run=help",
        ]))
        .unwrap();
        assert_eq!(options.extensions, ["a", "b"]);
        assert!(options.no_extensions);
        assert_eq!(options.run, ["scatter 3", "help"]);
    }

    #[test]
    fn watch_is_parsed_and_needs_a_window() {
        assert!(parse_args(&args(&["--watch"])).unwrap().watch);
        assert!(parse_args(&args(&["--watch", "--headless"])).is_err());
    }

    #[test]
    fn a_headless_run_dispatches_an_extension_verb() {
        let dir = std::env::temp_dir().join(format!("noctis-cli-ext-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let extension = dir.join("boxer");
        std::fs::create_dir_all(&extension).unwrap();
        std::fs::write(
            extension.join("extension.json"),
            r#"{ "id": "com.example.boxer", "name": "Boxer", "version": "1.0.0", "api": 1, "entry": "extension.js" }"#,
        )
        .unwrap();
        std::fs::write(
            extension.join("extension.js"),
            r#"noctis.command("box", function (parts) { return "ok " + noctis.edit({ op: "spawn", name: parts[1] || "Box" }); });"#,
        )
        .unwrap();

        let options = Options {
            headless: true,
            extensions: vec![dir.to_string_lossy().into_owned()],
            run: vec!["box Made".to_string()],
            ..Options::default()
        };
        let mut editor = build_editor(&options, None, &NullPlugin).expect("build");
        attach_extensions(&mut editor, &options, None).expect("attach");
        assert!(editor.find_by_name("Made").is_some());
        std::fs::remove_dir_all(&dir).ok();
    }
}
