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

use std::process::ExitCode;

use editor::{Editor, command_schema, parse_commands};
use platform::WindowConfig;

const USAGE: &str = "\
noctis-editor — build Noctis scenes from JSON commands

USAGE:
    noctis-editor [OPTIONS]

OPTIONS:
    -s, --scene <PATH>      Open a scene document before applying commands
    -c, --command <SRC>     Apply a JSON command batch. <SRC> is a file path,
                            the literal `-` for stdin, or inline JSON.
        --out <PATH>        Save the resulting scene to <PATH>
        --window            Open the scene in a window with a live viewport
        --print-schema      Print the JSON command schema and exit
    -h, --help              Print this help and exit

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
    command: Option<String>,
    out: Option<String>,
    window: bool,
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

    let mut editor = Editor::new();

    if let Some(scene) = &options.scene {
        editor
            .open(scene)
            .map_err(|e| CliError::Message(format!("failed to open scene {scene:?}: {e}")))?;
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

    let entities = editor.list_entities();
    serde_json::to_string_pretty(&entities)
        .map_err(|e| CliError::Message(format!("failed to serialize entities: {e}")))
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
            "--scene" | "-s" => {
                let (value, consumed) = value_for(&inline, args, index, &flag)?;
                options.scene = Some(value);
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
            other if other.starts_with('-') => {
                return Err(CliError::Usage(format!("unknown option {other:?}")));
            }
            other => {
                return Err(CliError::Usage(format!("unexpected argument {other:?}")));
            }
        }

        index += 1;
    }

    if !options.help
        && !options.print_schema
        && options.scene.is_none()
        && options.command.is_none()
    {
        return Err(CliError::Usage(
            "nothing to do: pass --scene and/or --command".into(),
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

/// Whether the invocation asked for a window.
fn wants_window(args: &[String]) -> bool {
    args.iter()
        .any(|arg| arg == "--window" || arg.starts_with("--window="))
}

/// Build the scene from the options, then run it in a window until closed.
fn run_windowed(options: &Options) -> Result<(), CliError> {
    let mut editor = Editor::new();
    if let Some(scene) = &options.scene {
        editor
            .open(scene)
            .map_err(|e| CliError::Message(format!("failed to open scene {scene:?}: {e}")))?;
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

    let config = WindowConfig {
        title: "Noctis Editor".to_string(),
        width: 1280,
        height: 720,
    };
    editor::window::run(editor, config).map_err(|e| CliError::Message(e.to_string()))
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
    fn empty_invocation_is_a_usage_error() {
        let err = parse_args(&args(&[])).unwrap_err();
        assert!(matches!(err, CliError::Usage(_)));
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
