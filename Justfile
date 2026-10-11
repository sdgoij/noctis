# Noctis build and test tasks. Run `just --list` (or bare `just`) to see them.
#
# `build` forwards `target` to cargo as `--target <triple>` (any Rust target);
# the default `host` builds for the native toolchain (no `--target`), and a
# non-`default` `features` argument is forwarded as `--features`.
#
# `run` takes a project directory or a `.zip` bundle and forwards any extra
# arguments to `noctis run`.
#
# `publish-wasm` builds the browser client, repackages `examples/showcase`, and
# stages the demo in `docs/` for GitHub Pages; the `wasm-bindgen` CLI must match
# the crate version in `Cargo.lock`.

showcase := "examples/showcase"
publish := "docs"

# List the available recipes.
default:
    @just --list

# Build for a given target (default: host) with features.
build target="host" features="default":
    cargo build --workspace{{ if target == "" { "" } else if target == "host" { "" } else { " --target " + target } }}{{ if features != "default" { " --features " + features } else { "" } }}

# Clippy and fmt check for the complete workspace.
check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings

# Run a project or bundle.
run project *args:
    cargo run -- run {{ if project =~ '\.zip$' { "--bundle" } else { "--project" } }} {{ project }} {{ args }}

# Run all checks and tests.
test: check
    cargo test --workspace

# Build the browser client for the showcase and stage the demo in `docs/`.
# `noctis build --platform wasm` builds the wasm client (the stock client here; a
# Rust project links its plugin), runs `wasm-bindgen`, and packs the bundle.
publish-wasm:
    cargo run -- build --project {{ showcase }} --platform wasm --out {{ publish }}
    touch {{ publish }}/.nojekyll
