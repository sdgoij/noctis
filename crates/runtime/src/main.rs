//! `noctis-game` — the stripped game runtime.
//!
//! This is the binary `noctis build` copies into an exported game folder: it
//! carries the game loop and nothing else — no editor, no authoring tools — so a
//! shipped game contains only what it needs to run. Launched with no arguments it
//! loads the `scene.json` bundled beside it.
//!
//! Development runs go through `noctis run` instead, which uses the same command
//! line ([`runtime::cli`]) but is part of the full `noctis` tool.

#![forbid(unsafe_code)]

fn main() -> std::process::ExitCode {
    runtime::cli::run_cli(std::env::args().skip(1).collect())
}
