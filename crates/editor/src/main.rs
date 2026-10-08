//! `noctis-editor` — the headless, agent-drivable editor CLI.
//!
//! The CLI is deliberately tiny and dependency-free beyond the editor crate:
//! it parses a handful of flags by hand, applies a JSON batch of
//! [`editor::EditorCommand`]s, and prints the resulting entity list as JSON.
//! It never panics — every failure becomes a non-zero exit and a clear message
//! on stderr.
//!
//! ```text
//! noctis-editor --scene level.json --command edits.json --out level.json
//! noctis-editor --command '[{ "op": "spawn", "name": "Sun" }]'
//! noctis-editor --print-schema
//! ```

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

use ecs::scene::SceneMode;
use editor::{Editor, command_schema, parse_commands};
use platform::WindowConfig;
use project::Project;

const USAGE: &str = "\
noctis-editor — build Noctis scenes from JSON commands

USAGE:
    noctis-editor [OPTIONS]

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
        --window            Open the scene in a window with a live viewport
        --testscene         Load the engine's all-features test scene
        --controls <PATH>   Load camera-navigation bindings (JSON) for --window
        --print-schema      Print the JSON command schema and exit
    -h, --help              Print this help and exit

With no options, a new empty scene is opened and its (empty) entity list is
printed. `--scene` and `--command` are both optional.

The batch is a JSON array of commands; a single command object is also
accepted. On success the resulting entity list is printed as JSON.";

/// A top-level CLI failure, distinguished so usage problems can exit `2`.
#[derive(Debug)]
enum CliError {
    /// The command line was malformed.
    Usage(String),
    /// A runtime failure (bad scene, command, or I/O).
    Message(String),
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if wants_window(&args) {
        let options = match parse_args(&args) {
            Ok(options) => options,
            Err(CliError::Usage(message)) => {
                eprintln!("noctis-editor: {message}\n");
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
            Err(CliError::Message(message)) => {
                eprintln!("noctis-editor: {message}");
                return ExitCode::FAILURE;
            }
        };
        return match run_windowed(&options) {
            Ok(()) => ExitCode::SUCCESS,
            Err(CliError::Usage(message)) => {
                eprintln!("noctis-editor: {message}\n");
                eprintln!("{USAGE}");
                ExitCode::from(2)
            }
            Err(CliError::Message(message)) => {
                eprintln!("noctis-editor: {message}");
                ExitCode::FAILURE
            }
        };
    }

    match run(&args) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(CliError::Usage(message)) => {
            eprintln!("noctis-editor: {message}\n");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
        Err(CliError::Message(message)) => {
            eprintln!("noctis-editor: {message}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Default, Debug, PartialEq)]
struct Options {
    scene: Option<String>,
    project: Option<String>,
    mode: Option<SceneMode>,
    command: Option<String>,
    out: Option<String>,
    controls: Option<String>,
    window: bool,
    testscene: bool,
    print_schema: bool,
    help: bool,
}

fn run(args: &[String]) -> Result<String, CliError> {
    let options = parse_args(args)?;

    if options.help {
        return Ok(USAGE.to_string());
    }
    if options.print_schema {
        let schema = command_schema();
        return serde_json::to_string_pretty(&schema)
            .map_err(|e| CliError::Message(format!("failed to render schema: {e}")));
    }

    let editor = build_editor(&options, open_project(&options)?.as_ref())?;
    let entities = editor.list_entities();
    serde_json::to_string_pretty(&entities)
        .map_err(|e| CliError::Message(format!("failed to serialize entities: {e}")))
}

/// Build the editor for the options: a `--testscene`, a scene (from `--scene`,
/// resolved against `--project`, or the project's default), then any command
/// batch, saving when `--out` is given.
fn build_editor(options: &Options, project: Option<&Project>) -> Result<Editor, CliError> {
    let mut editor = Editor::new();

    // A project owns one shared asset registry: import every asset file up front
    // so the scene's references resolve. Without a project the editor keeps the
    // registry's placeholder defaults.
    if let Some(project) = project {
        let imported = project
            .import_assets()
            .map_err(|e| CliError::Message(format!("failed to import project assets: {e}")))?;
        for error in &imported.errors {
            eprintln!(
                "noctis-editor: warning: asset '{}' failed to import: {}",
                error.path.as_str(),
                error.message
            );
        }
        eprintln!(
            "noctis-editor: imported {} asset file(s) ({} failed)",
            imported.files,
            imported.errors.len()
        );
        *editor.assets_mut() = imported.registry;
    }

    if options.testscene {
        let (scene, assets) = testscene::build();
        *editor.scene_mut() = scene;
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

    if let Some(out) = &options.out {
        editor
            .save(out)
            .map_err(|e| CliError::Message(format!("failed to save scene {out:?}: {e}")))?;
    }

    Ok(editor)
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
            "--window" => {
                reject_inline(&inline, &flag)?;
                options.window = true;
            }
            "--testscene" => {
                reject_inline(&inline, &flag)?;
                options.testscene = true;
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
            other if other.starts_with('-') => {
                return Err(CliError::Usage(format!("unknown option {other:?}")));
            }
            other => {
                return Err(CliError::Usage(format!("unexpected argument {other:?}")));
            }
        }

        index += 1;
    }

    if !options.help && !options.print_schema {
        // Every option is optional: with no arguments we work on a new, empty
        // scene. No argument combination is a usage error here.
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

/// Whether the invocation asked for a window.
fn wants_window(args: &[String]) -> bool {
    args.iter()
        .any(|arg| arg == "--window" || arg.starts_with("--window="))
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
    };
    if let Some(project) = project {
        let window = &project.manifest().window;
        config.title = window
            .title
            .clone()
            .unwrap_or_else(|| project.name().to_string());
        config.width = window.width;
        config.height = window.height;
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
fn run_windowed(options: &Options) -> Result<(), CliError> {
    let project = open_project(options)?;
    let editor = build_editor(options, project.as_ref())?;
    let config = window_config(project.as_ref());

    // `--controls` wins over the project's `[input].controls`.
    let controls = options
        .controls
        .clone()
        .or_else(|| project_controls_path(project.as_ref()));

    match controls {
        Some(path) => {
            let controls = editor::CameraControls::load(&path)
                .map_err(|e| CliError::Message(format!("failed to load controls {path:?}: {e}")))?;
            editor::window::run_with_controls(editor, config, controls)
                .map_err(|e| CliError::Message(e.to_string()))
        }
        None => editor::window::run(editor, config).map_err(|e| CliError::Message(e.to_string())),
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
        let options = parse_args(&args(&["--window", "--controls", "controls.json"])).unwrap();
        assert!(options.window);
        assert_eq!(options.controls.as_deref(), Some("controls.json"));
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

        run(&args(&["--mode", "2d", "--out", out.to_str().unwrap()])).unwrap();
        let json = std::fs::read_to_string(&out).expect("scene written");
        assert!(json.contains("\"mode\": \"2d\""), "{json}");

        std::fs::remove_dir_all(&dir).ok();
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
        let output = run(&args(&[])).unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value.as_array().unwrap().len(), 0);
    }

    #[test]
    fn run_applies_inline_commands_and_returns_entities() {
        let output = run(&args(&[
            "--command",
            r#"[{ "op": "spawn", "name": "Sun" }]"#,
        ]))
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value.as_array().unwrap().len(), 1);
        assert_eq!(value[0]["name"], "Sun");
    }

    #[test]
    fn run_reports_bad_command_json() {
        let err = run(&args(&["--command", "not json"])).unwrap_err();
        assert!(matches!(err, CliError::Message(_)));
    }

    #[test]
    fn run_reports_unknown_entity() {
        let err = run(&args(&[
            "--command",
            r#"[{ "op": "rename", "entity": { "name": "ghost" }, "name": "x" }]"#,
        ]))
        .unwrap_err();
        assert!(matches!(err, CliError::Message(_)));
    }

    #[test]
    fn print_schema_returns_valid_json() {
        let output = run(&args(&["--print-schema"])).unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(value["commands"].is_array());
    }
}
