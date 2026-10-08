# AGENTS.md — Noctis engine development guide

Noctis is a production-grade 3D game engine for Windows/macOS/Linux: a Rust core
(edition 2024, Rust 1.98) with a wgpu renderer, rapier3d physics, rodio audio,
and JavaScript scripting via the Slag engine. **No hacks, no shortcuts.**

## Workspace layout

| Crate | Path | Responsibility |
|---|---|---|
| `math` | `crates/math` | glm-based vectors/matrices + our `Quat`/`Transform`/`Aabb`. |
| `ecs` | `crates/ecs` | ECS, components, scene + hierarchy, assets, resources, app loop. |
| `render` | `crates/render` | wgpu 30 PBR renderer: glTF import, IBL, soft shadows, SSAO, HDR tonemapping, gizmos. |
| `physics` | `crates/physics` | rapier3d 0.36 integration (`PhysicsWorld`). |
| `audio` | `crates/audio` | rodio 0.22 engine with 3D spatialization. |
| `script` | `crates/script` | Script runtime + engine API; Slag JS backend behind the `slag` feature. |
| `testscene` | `crates/testscene` | The all-features test scene (every component + the whole scripting API). |
| `editor` | `crates/editor` | Scene editor library + AI-controllable command API + `noctis-editor` CLI. |
| `runtime` | `crates/runtime` | Game runtime/export loop + `noctis-runtime` CLI. |
| `platform` | `crates/platform` | Window + winit event loop + egui overlay; drives any `AppDriver` into a surface-backed viewport. |

The frozen interface between `math`/`ecs` and everything else is documented in
[`docs/core-api.md`](docs/core-api.md). **Treat it as a contract**: if a consumer
needs a change, change the contract deliberately and update this doc and the
affected crates together — never bend `math`/`ecs` to fit one caller.

## Commands

```sh
cargo build --workspace                 # build everything
cargo test  --workspace                 # run the whole suite (required before claiming done)
cargo test  -p ecs                      # one crate
cargo clippy --workspace --all-targets  # lints
cargo fmt --all                         # format (max_width = 100)
cargo test  -p script  --features slag  # real JavaScript backend + its smoke tests
cargo test  --manifest-path crates/runtime/Cargo.toml --features slag  # JS in a game
```

## Non-negotiable standards

- **Edition 2024, Rust 1.98.** Toolchain pinned in `rust-toolchain.toml`.
- **`#![forbid(unsafe_code)]`** in every crate. Do not reach for `unsafe`.
- **Tests are the product.** Every module with logic has `#[cfg(test)]` tests.
  Prefer many small, deterministic tests. Nothing is "done" until
  `cargo test --workspace` is green.
- **No panics on runtime paths.** Return `Result`/`Option`. `unwrap`/`expect`
  only for genuinely infallible cases (e.g. serializing an owned value).
- **Docs:** module-level `//!` docs and `///` docs on public items.
- **Formatting:** `cargo fmt` (see `rustfmt.toml`).
- **Keep it simple.** Prefer a clear, explicit implementation over a clever one.

## Testing doctrine

1. Unit-test each module's logic in-crate.
2. The **`testscene` crate is the integration spine**: it builds a scene that
   uses *every* component and then drives it through physics, scripting, audio,
   and the renderer's CPU-side extraction. When you add a component, add it to
   the test scene *and* to its coverage assertion.
3. GPU tests must **skip gracefully** when no adapter is present
   (`RenderError::NoAdapter`); never fail CI because a machine lacks a GPU.
4. Audio tests must run **without a device** (`AudioEngine::silent()`).

## Adding a component (checklist)

1. Define the struct in `crates/ecs/src/components.rs` with `Debug`, sensible
   `Default`, and docs.
2. Add it to `component_index::ComponentKind` (`name`/`from_name`/`ALL`).
3. Serialize it: add a `ComponentDesc` variant and wire `component_descs` /
   `apply_component` in `crates/ecs/src/scene.rs`.
4. Consume it in the relevant subsystem crate.
5. Add it to `crates/testscene` and to the "every component present" assertion.
6. `cargo test --workspace`.

## Known limitations (tracked, not hidden)

- `render`: `sample_count > 1` (MSAA) is clamped to 1; IBL is precomputed on the
  CPU at load time; one shadow-casting directional/spot light at a time.
- `script`: the real backends are the `slag` feature (off by default). It builds
  from the pinned `github.com/sdgoij/slag` and its smoke tests pass
  (`cargo test -p script --features slag`); the default build uses the testable
  `NoopHost`/`RecordingHost`. The feature enables **JavaScript** (Slag's engine,
  with its WebAssembly JS API and Cranelift JIT hook — hence the Cranelift build
  cost), **native WebAssembly scripts** (`wasm_host::WasmHost`, a `.wasm` module
  driven through a documented import ABI covering transforms (read and write),
  spawning, `destroy`/`destroy_by_name`, `log`, `find_entity`, `add_script`,
  `instantiate_prefab` (which returns a handle for the prefab's root entity, so
  a script can position or inspect what it just instantiated),
  `key_down`/`mouse_down`, and timing), and
  `engine_host::EngineHost`, which runs both languages in one runtime. Both
  backends address entities through small per-call handle indices rather than raw
  ids, because a JavaScript `f64` cannot represent a 64-bit `EntityId` exactly. A
  WebAssembly module travels as a `wasm:`-prefixed hex descriptor (the script
  channel is text); `wasm_host::load`/`register` read a `.wasm` file into that
  form, and `noctis-runtime --wasm <name>=<path>` registers one for a game. Note
  Slag's Cranelift
  build adds ~20-30s to a cold build of the feature. `runtime` forwards the
  feature, so a game built and run with it executes real JavaScript and
  WebAssembly (see the `slag` entry under Commands; use `--manifest-path`, as
  `-p runtime` is ambiguous with a transitive Slag crate).
  To make that possible, `App` systems are deliberately **not** `Send + Sync`:
  the loop owns them on one thread, which lets a system own a non-thread-safe
  engine. Component types remain `Send + Sync + 'static`.
- `editor`/`runtime` open a live windowed viewport (`--window`). The editor draws
  egui panels (outliner, inspector, transform editing), supports click-to-select
  viewport picking (a ray vs. mesh AABBs), and a full transform gizmo (Move /
  Rotate / Scale handles, drawn world- or entity-oriented, one undo step per
  drag) driven through `AppDriver::gizmo`; the toolbar also has Undo/Redo. It
  spawns primitive objects (cube/sphere/plane/light/camera) from the Outliner's
  **Add** buttons and `--testscene` loads the all-features test scene.
  `noctis-runtime --export <dir> --scene <file>` writes a self-contained game
  folder (a copy of the executable, `scene.json`, and a manifest) that loads its
  bundled scene when run with no arguments. The windowed loop is not unit-tested
  (it blocks and needs a display).

## Commit messages

Subject line ≤ 50 chars, imperative, no trailing period, `type:` prefix
(`feat:`, `fix:`, `test:`, `docs:`). Add a body only when it carries real
information (what changed and why, grouped by crate/module with concrete file
names and test counts). See the repo history for examples.
