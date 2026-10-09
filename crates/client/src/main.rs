//! `noctis-game` — the distributable game client.
//!
//! This is the stripped runtime `noctis build` copies into an exported game
//! folder: the game loop and nothing else — no editor, no authoring tools — so a
//! shipped game contains only what it needs to run. Launched with no arguments it
//! loads the `scene.json` bundled beside it.
//!
//! A release build (`noctis build`) bakes the project's scripts into this binary
//! (see `build.rs` and `docs/scripts.md`); a development build leaves the table
//! empty and loads scripts from disk, watching them for edits.
//!
//! Development runs go through `noctis run`, which uses the same command line
//! ([`runtime::cli`]) but is part of the full `noctis` tool.

#![forbid(unsafe_code)]

mod embedded {
    use runtime::EmbeddedScript;

    include!(concat!(env!("OUT_DIR"), "/embedded.rs"));
}

fn main() -> std::process::ExitCode {
    runtime::cli::run_cli_with(std::env::args().skip(1).collect(), embedded::EMBEDDED)
}
