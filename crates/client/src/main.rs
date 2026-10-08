//! `noctis-game` — the distributable game client.
//!
//! This is the stripped runtime `noctis build` copies into an exported game
//! folder: the game loop and nothing else — no editor, no authoring tools — so a
//! shipped game contains only what it needs to run. Launched with no arguments it
//! loads the `scene.json` bundled beside it.
//!
//! Development runs go through `noctis run`, which uses the same command line
//! ([`runtime::cli`]) but is part of the full `noctis` tool.

#![forbid(unsafe_code)]

fn main() -> std::process::ExitCode {
    runtime::cli::run_cli(std::env::args().skip(1).collect())
}
