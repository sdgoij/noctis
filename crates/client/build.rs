//! Bake the project's scripts into the client when `noctis build` asks.
//!
//! `noctis build --profile release` writes an embed pack and sets
//! `NOCTIS_EMBED_PACK` when it builds this crate. This script turns that pack into
//! `$OUT_DIR/embedded.rs`, a `&'static [EmbeddedScript]` the binary passes to
//! `runtime::cli::run_cli_with`. With no pack set the table is empty and the
//! client loads scripts from disk (the development shape). See `docs/scripts.md`.

use std::env;
use std::fmt::Write as _;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-env-changed=NOCTIS_EMBED_PACK");

    // The game host runs a JavaScript engine (Slag), which recurses while parsing
    // and evaluating a script. Reserve a large main-thread stack on Windows, whose
    // default is only 1 MiB (Linux and macOS already give the main thread 8 MiB).
    if env::var("CARGO_CFG_WINDOWS").is_ok() {
        println!("cargo:rustc-link-arg-bins=/STACK:16777216");
    }

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR is set by cargo");
    let dest = Path::new(&out_dir).join("embedded.rs");

    let code = match env::var_os("NOCTIS_EMBED_PACK") {
        Some(pack) => generate(Path::new(&pack)),
        None => String::from("pub static EMBEDDED: &[EmbeddedScript] = &[];\n"),
    };
    std::fs::write(&dest, code).expect("write embedded.rs");
}

/// The generated `EMBEDDED` table for the pack at `pack`.
fn generate(pack: &Path) -> String {
    println!("cargo:rerun-if-changed={}", pack.display());
    let text = std::fs::read_to_string(pack)
        .unwrap_or_else(|error| panic!("reading the embed pack {}: {error}", pack.display()));
    let document: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("parsing the embed pack {}: {error}", pack.display()));

    let mut out = String::from("pub static EMBEDDED: &[EmbeddedScript] = &[\n");
    if let Some(scripts) = document
        .get("scripts")
        .and_then(serde_json::Value::as_array)
    {
        for script in scripts {
            let path = script
                .get("path")
                .and_then(serde_json::Value::as_str)
                .expect("every pack entry has a path");
            let source = script
                .get("source")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| panic!("pack entry {path:?} has no source"));
            // The source is embedded by absolute path, so editing it rebuilds.
            println!("cargo:rerun-if-changed={source}");
            let _ = writeln!(
                out,
                "    EmbeddedScript {{ path: {path:?}, bytes: include_bytes!({source:?}) }},"
            );
        }
    }
    out.push_str("];\n");
    out
}
