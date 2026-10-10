//! Build script for the `noctis` tool.
//!
//! The editor's extension host runs a JavaScript engine (Slag), which recurses
//! while it parses and evaluates a script. The Windows main thread's default
//! 1 MiB stack is too small for the extension bootstrap, so reserve 16 MiB for
//! the binary. (Linux and macOS already give the main thread 8 MiB.)

fn main() {
    if std::env::var("CARGO_CFG_WINDOWS").is_ok() {
        println!("cargo:rustc-link-arg-bins=/STACK:16777216");
    }
}
