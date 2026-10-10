//! `noctis` — the Noctis engine tool (binary).
//!
//! Routing lives in the library ([`noctis::main_entry`]), so the same code serves
//! the stock binary and a project's generated plugin-aware binary. This file only
//! starts logging and hands the command line over.

#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    runtime::cli::init_logging();
    let args: Vec<String> = std::env::args().skip(1).collect();
    noctis::main_entry(&args)
}
