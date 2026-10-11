//! `noctis` — the Noctis engine tool (binary).
//!
//! Routing lives in the library ([`noctis::main_entry`]), so the same code serves
//! the stock binary and a project's generated plugin-aware binary. This file only
//! starts logging and hands the command line over.

#![forbid(unsafe_code)]

#[cfg(not(target_arch = "wasm32"))]
use std::process::ExitCode;

/// The `noctis` tool is native-only. On `wasm32` this crate is used as a library
/// (a project's generated web entry links it), so the binary has nothing to do.
#[cfg(not(target_arch = "wasm32"))]
fn main() -> ExitCode {
    runtime::cli::init_logging();
    let args: Vec<String> = std::env::args().skip(1).collect();
    noctis::main_entry(&args)
}

#[cfg(target_arch = "wasm32")]
fn main() {}
