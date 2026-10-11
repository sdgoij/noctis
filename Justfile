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
wasm_out := "crates/web/web"
wasm_artifact := "target/wasm32-unknown-unknown/release/web.wasm"
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

# Build a new wasm, package the showcase, and stage the demo in docs/.
publish-wasm:
    cargo build -p web --target wasm32-unknown-unknown --release
    wasm-bindgen --target web --out-dir {{ wasm_out }} {{ wasm_artifact }}
    cargo run -- pack --project {{ showcase }} --out {{ wasm_out }}/game.zip
    mkdir -p {{ publish }}
    cp {{ wasm_out }}/index.html {{ wasm_out }}/env.js {{ wasm_out }}/web.js {{ wasm_out }}/web_bg.wasm {{ wasm_out }}/game.zip {{ publish }}/
    touch {{ publish }}/.nojekyll
