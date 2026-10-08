---
name: noctis-engine
description: Development guide for the Noctis 3D game engine (Rust, wgpu, rapier3d, rodio, Slag). Use when building, testing, modifying, or extending the engine crates (math, ecs, render, physics, audio, script, testscene, project, editor, runtime, platform, noctis), adding components, writing systems, or fixing engine bugs.
---

# Working on the Noctis engine

## Orientation

- The workspace root is this repository. Crates live in `crates/<name>`.
- `math` and `ecs` are the **frozen foundation**. Their public API is documented
  in `docs/core-api.md`. Never change them to fit a single consumer; if a change
  is genuinely needed, change the contract and every affected crate together.
- Read `AGENTS.md` for the non-negotiable standards.
- `crates/noctis` is the `noctis` multicall tool (`editor`/`run`/`build`/`rename`)
  and the embedding facade (`noctis::prelude`). It is the workspace's default
  member, so `cargo run -- editor` (and friends) runs it without a `-p`.

## The loop

1. Read `docs/core-api.md` (and the relevant `crates/*/src/*.rs`).
2. Make the smallest correct change.
3. Add/extend `#[cfg(test)]` tests **in the same module**.
4. Run **`cargo test --workspace`** and make it green. Also run
   `cargo build --workspace` and `cargo clippy --workspace --all-targets`.
5. If you added a component, see the checklist in `AGENTS.md` (component index,
   scene serialization, test scene coverage assertion).

## Rules that are easy to get wrong

- **No `unsafe`** — every crate is `#![forbid(unsafe_code)]`.
- **No panics on runtime paths.** Return `Result`/`Option`.
- **GPU code must be adapter-guarded.** If there is no adapter, skip the test
  (return early on `RenderError::NoAdapter`); never fail a machine without a GPU.
- **Audio code must be device-free in tests.** Use `AudioEngine::silent()`.
- **`render` uses wgpu 30** and `physics` uses **rapier3d 0.36** — both are recent
  and their APIs move; check `docs.rs` and let the compiler guide you.
- The Slag JS backend is behind the `script` crate's `slag` feature (off by
  default because it fetches a large submodule). Enable it with
  `cargo build -p script --features slag`.

## Component workflow

`crates/ecs/src/components.rs` defines the component; `crates/ecs/src/scene.rs`
serializes it (`ComponentDesc` + `component_descs` + `apply_component`);
`crates/testscene` must exercise it; the subsystem crate that consumes it lives
in `crates/{render,physics,audio,script}`.

## Commands

```sh
cargo test --workspace
cargo test -p <crate>
cargo clippy --workspace --all-targets
cargo fmt --all
cargo run -- editor                      # the noctis tool is the default binary
cargo build -p script --features slag
cargo build --manifest-path crates/runtime/Cargo.toml --features slag --bin noctis-game
```
