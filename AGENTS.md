# AGENTS.md — Noctis engine development guide

Noctis is a 3D game engine for Windows and Linux (macOS is untested and
unsupported): a Rust core (edition 2024, Rust 1.98) with a wgpu renderer,
rapier3d physics, rodio audio, and JavaScript scripting via the Slag engine.
**No hacks, no shortcuts.**

## Workspace layout

| Crate | Path | Responsibility |
|---|---|---|
| `math` | `crates/math` | glm-based vectors/matrices + our `Quat`/`Transform`/`Aabb`. |
| `ecs` | `crates/ecs` | ECS, components, scene + hierarchy, assets, resources, app loop. |
| `ecs-macros` | `crates/ecs-macros` | `#[derive(Component)]` for custom (registered) components. |
| `render` | `crates/render` | wgpu 30 PBR renderer: glTF import, IBL, soft shadows, SSAO, HDR tonemapping, gizmos. |
| `physics` | `crates/physics` | rapier3d 0.36 integration (`PhysicsWorld`). |
| `audio` | `crates/audio` | rodio 0.22 engine with 3D spatialization; a WebAudio backend on `wasm32`. |
| `script` | `crates/script` | Script runtime + engine API; JavaScript/WebAssembly via the Slag engine. |
| `testscene` | `crates/testscene` | The all-features test scene (every component + the whole scripting API). |
| `project` | `crates/project` | The project model: `noctis.toml`, canonical asset paths, type detection, discovery. |
| `extensions` | `crates/extensions` | Loader for editor extensions: discovery, `extension.json`, ordering, config. |
| `vfs` | `crates/vfs` | Filesystem-store abstraction: `Store` over a native dir, an in-memory zip, or memory. |
| `editor` | `crates/editor` | Scene editor library + AI-controllable command API; its CLI is `noctis editor`. |
| `runtime` | `crates/runtime` | Game runtime/export loop (library); the tool and the client drive it in-process. |
| `platform` | `crates/platform` | Window + winit event loop + egui overlay; drives any `AppDriver` into a surface-backed viewport. |
| `noctis` | `crates/noctis` | The `noctis` multicall tool (`editor`/`run`/`build`/`rename`) and the embedding facade (`noctis::prelude`). |
| `client` | `crates/client` | The distributable `noctis-game` client: a stripped runtime binary, no editor. |

The frozen interface between `math`/`ecs` and everything else is documented in
[`.notes/core-api.md`](.notes/core-api.md). **Treat it as a contract**: if a consumer
needs a change, change the contract deliberately and update this doc and the
affected crates together — never bend `math`/`ecs` to fit one caller.

What a *project* is — the `noctis.toml` manifest, the directory conventions, asset
paths and references, import, and build targets — is designed in
[`.notes/project.md`](.notes/project.md). That document is the contract for the
`project` crate; update it deliberately when the design changes.

Embedding the engine in a Rust application, and carrying Rust in a project (the
`Plugin` seam, scene-authorable custom components, and script reflection), is
designed in [`.notes/embedding.md`](.notes/embedding.md). The `Plugin` seam and project
Rust are shipped (it extends the `ecs` core contract — a component registry — which
`.notes/core-api.md` records); script reflection is not yet built.

Running the engine in a browser (WebAssembly) — the generic game client first, a
`vfs` `Store` for files, a `platform-web` host, WebGPU-only rendering and
JS-only scripting — is designed in [`.notes/web.md`](.notes/web.md). **Nothing is
built yet.**

The editor's windowed UI — a custom title bar, three docks (left, right, bottom)
around a central viewport, and a status bar that opens and closes dockable views
(the outliner, inspector, console, Extensions screen, and every extension panel)
— is designed in [`.notes/editor-ui.md`](.notes/editor-ui.md). It is shipped; tab
drag between docks is the one follow-up.

Skeletal animation — playing a skinned glTF model's clips through an `Animator`
component and a GPU-skinned draw path — is designed in
[`.notes/animation.md`](.notes/animation.md). **Phases 1-3 (glTF import, the
skinned draw and clip playback) are shipped.**

Porting **Slag Goat** (`goats/`), a JavaScript-and-raylib goat sandbox, onto the
engine — a Rust plugin for the simulation, scenes for the world and menus, egui
for the screens — is planned in [`.notes/goats.md`](.notes/goats.md). **M0 (the
render-path spike) and M1 (locomotion) are shipped** as `examples/goats`: the goat
model loads as a skinned mesh, the ground is built from the original's noise, and
the goat's idle/walk/trot/run/jump/sleep state machine drives it under an orbit
camera. M2 has the health/energy system (exhaustion, death, restart), the meadow
(the procedural grass field, and grazing it), and the weather state machine (rain
and wind slow the goat and make it drain energy faster). The water table, the
weather's clouds and rain, the rest of the food model and the world simulation are
not built yet.

## Commands

```sh
cargo build --workspace                 # build everything, incl. the noctis-game client
cargo test  --workspace                 # run the whole suite (required before claiming done)
cargo test  -p ecs                      # one crate
cargo clippy --workspace --all-targets  # lints
cargo fmt --all                         # format (max_width = 100)
cargo run   -- editor                   # `noctis` is the default binary (default-members)
cargo run   -- run --project <dir>      # run a project (windowed; --headless to exit)
cargo run   -- run --release --project <dir>  # a Rust project's plugin, optimized
```

`noctis build` and `noctis run --export` build the `noctis-game` client
themselves (a cargo no-op when it is already fresh), so no separate build step is
needed.

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
5. The **`examples/showcase` project is the end-to-end spine**: its scenes are
   not only loaded but *run* through the runtime with the real Slag engine
   (`crates/runtime/tests/showcase_scenes.rs`), so a component or script that
   stops moving an entity fails the suite. Update it when a scene changes.

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

- `render`: MSAA is supported up to 4x (`[render] sample_count`), degrading to
  the largest count the device supports for the HDR colour and depth formats; it
  smooths geometry silhouettes, so shader-level aliasing (a hard texture edge, a
  glyph's coverage) is still visible. MSAA adds no pass, but a screen-space pass
  cannot sample a multisampled depth texture, so the single-sampled camera depth
  prepass SSAO reads cannot be the multisampled depth the forward passes write;
  because SSAO is then that prepass's only reader, under MSAA it renders at half
  resolution. IBL is precomputed on the CPU at load time
  from the `[render] environment` a project names (a `.hdr` equirectangular map
  decoded into the `hdr` store) — there is no GPU precompute, no `.exr` decode,
  and no live re-bake, and an unset environment falls back to a neutral (fully
  white) one; one shadow-casting directional/spot light at a time. In-world text
  (`ecs::components::Text`) is drawn billboarded by a dedicated pass, rasterized
  from a font atlas baked on demand (`ab_glyph`); the atlas is cached per
  process, not yet persisted under `.noctis/`. A material may be **unlit** — its
  base color, times its base-color texture, is output directly with no direct
  light, IBL, or SSAO, the flat-shaded path for 2D sprites and UI — and may
  sample its textures with **nearest** filtering for crisp pixel art; both are
  imported from glTF (`KHR_materials_unlit`, a `NEAREST` mag-filter) and
  authorable in the inspector. A glTF material's `KHR_texture_transform` is
  applied to the primitive's UVs (the base-color map's transform — the engine has
  one UV set, so a map whose transform differs is not representable). There is
  still no dedicated 2D/sprite renderer or
  screen-space texture blit: a sprite is a textured quad on an orthographic
  camera. A driver may instead publish a whole frame of its own pixels —
  `ecs::resources::FrameImage` on `EngineContext::frame` — which the window host
  blits across the window in place of the scene (`Renderer::present_image`,
  headless `Renderer::render_image`); this is the seam a *software* renderer (a
  plugin's own RGBA framebuffer) uses to get on screen. Such a driver may also
  publish screen-space HUD lines — `EngineContext::screen_text` — which the
  runtime's `AppDriver::ui` draws over the frame.
- `script`: scripting is the `github.com/sdgoij/slag` engine, pinned to a
  revision in the workspace `Cargo.toml` (`[workspace.dependencies] slag`) — a
  plain dependency, with no feature to enable. A Slag bump is a one-line `rev`
  change there, and cargo re-resolves it everywhere, including the generated
  `.noctis/*` client crates. It provides **JavaScript** (with Slag's WebAssembly JS
  API and Cranelift JIT hook — hence the Cranelift build cost, ~20-30s on a cold
  build), **native WebAssembly scripts** (`wasm_host::WasmHost`, a `.wasm` module
  driven through a documented import ABI covering transforms (read and write),
  spawning, `destroy`/`destroy_by_name`, `log`, `find_entity`, `add_script`,
  `instantiate_prefab` (which returns a handle for the prefab's root entity, so
  a script can position or inspect what it just instantiated),
  `key_down`/`mouse_down`, pointer motion (`mouse_move`/`mouse_position`/
  `mouse_scroll`), driving a dynamic body (`set_velocity`/`add_velocity`/
  `apply_impulse`; the runtime applies these to physics) and reading it back
  (`get_velocity`, `is_grounded`, `is_overlapping`), scene control
  (`load_scene`, and `teleport_scene`, which carries a named entity and its
  subtree into the target scene, landing it at that scene's `Entry` marker),
  and timing), and `engine_host::EngineHost`, which runs
  both languages in one runtime. Both backends address entities through small,
  stable per-instance handle indices rather than raw ids, because a JavaScript
  `f64` cannot represent a 64-bit `EntityId` exactly; the table persists for the
  instance's life, so a handle a script stores keeps naming the same entity
  across frames. A WebAssembly module travels as a
  `wasm:`-prefixed hex descriptor (the script channel is text); the descriptor
  encode/decode lives in `ecs::assets`, a `.wasm` file imports as a script asset
  (so a scene references it by path like a `.js`), `wasm_host::load`/`register`
  read one into that form, and `noctis run --wasm <name>=<path>` registers one
  for a game. `NoopHost` and `RecordingHost` remain as inert/recording test
  hosts.
  To make the single-threaded loop possible, `App` systems are deliberately
  **not** `Send + Sync`:
  the loop owns them on one thread, which lets a system own a non-thread-safe
  engine. Component types remain `Send + Sync + 'static`.
  **Component reflection**: JavaScript reads and writes components by name through
  `has_component`/`get_component` (JSON text)/`set_component`/`remove_component`,
  backed by the scene reflection layer (`ecs::scene`, `ScriptCommand::SetComponent`/
  `RemoveComponent`). A registered custom component uses its own JSON; a built-in
  uses its scene-descriptor JSON (`{"type": "rigid_body", ...}`). Native WebAssembly
  reaches the same layer through its import ABI with JSON over linear memory
  (`has_component`/`get_component` into the module's memory/`set_component`/
  `remove_component`). The entity-level `LocalTransform`/`Name`/`Active`/`Visible`
  are not in the reflected list; they keep their dedicated bindings.
- **Scene switching and portals** (see [`.notes/portals.md`](.notes/portals.md)).
  A script switches scenes with `load_scene`, which replaces the world (physics
  rebuilt, scripting restarted). `teleport_scene` additionally *carries* an entity
  and its subtree across the load, placing its root at the target scene's `Entry`
  marker (or keeping its transform when the target has none) — the showcase's
  Trigger Pad uses it to move the Player into the physics playground, and a
  Sensor Zone carries it back. Only one named entity travels per transition, and
  transient state restarts on the far side (script locals such as velocity, and
  the carried entity's scripts re-run `on_start`).
- `noctis build` ships scripts by **profile** (see [`.notes/scripts.md`](.notes/scripts.md)):
  a `release` build bakes the scene's `.js`/`.wasm` into the `noctis-game` client
  (via `NOCTIS_EMBED_PACK` → `crates/client/build.rs` → `include_bytes!`) and
  writes no `scripts/` into the game folder; a `dev` build copies them in, and the
  client watches the project root (`notify`) and hot-reloads a changed script,
  which works because the script runtime's module cache is keyed by source. Only
  scripts embed for now — scenes, models, textures, audio, fonts, and HDR
  environments stay loose files, so packaging larger assets is still to come.
- **Project Rust (a plugin)** is shipped (see
  [`.notes/embedding.md`](.notes/embedding.md)). A project carries Rust when its
  root holds a `Cargo.toml` — a library crate whose type is marked
  `#[noctis::plugin]` (exporting the `noctis_plugin` symbol); there is no manifest
  table. `noctis run`/`editor` generate a per-project **tool** under the project's
  gitignored `.noctis/tool/` that links it and dispatches the same CLI, and
  `noctis build` generates a plugin-linked **client** under `.noctis/client/`
  (runtime + plugin, no editor) to ship, baking scripts in for a release build. A
  registered custom component's asset references are bundled. The `Component`
  derive and the `plugin` attribute both resolve their crate path for `ecs`
  consumers and `noctis`-only plugin crates, and `noctis`'s `editor` is a default
  feature so a shipped plugin game can drop it. A worked example (a custom `Spin`
  component, a fly-camera system, and a simulated ball) is `examples/rustball`.
  `noctis build --platform wasm` (or a `[[build]]` `platform = "wasm"` target)
  additionally builds a **browser** client: each project's plugin is linked into a
  generated `.noctis/web/` crate, compiled for `wasm32-unknown-unknown`, run
  through `wasm-bindgen`, and staged with a `<key>.zip` bundle and a generated
  `index.html` — `--project` may repeat, so one client dispatches across several
  projects by name. `crates/web` exposes a plugin-aware `run_bundle_with` (and a
  no-plugin `run_bundle`), and the `noctis` facade builds for wasm (its CLI is
  native-gated). See [`.notes/web.md`](.notes/web.md).
  **Not yet:** script reflection over custom components (phase 3) and
  native-WebAssembly reflection.
- **Editor extensions** are shipped for the editor only (see
  [`.notes/extensions.md`](.notes/extensions.md)): a project or user directory of
  `extension.js`/`extension.wasm` extensions is discovered by `noctis editor`, and
  each is evaluated against a versioned `noctis` global (`on`/`command`/`edit`/
  `editor`/`ui`) in the Slag engine. The pure loader is `crates/extensions`; the
  host is `editor::extension` (JS) and `editor::extension::wasm` (native WASM),
  and the windowed UI is `editor::window`.
  `--extensions`/`--no-extensions` scope discovery, `--run` dispatches a verb, and
  `--watch` rebuilds the host when a watched extension directory changes;
  the windowed editor draws extension panels, a console with a scrollback, and an
  Extensions screen whose toggles persist to a config file. A worked example is
  `examples/extensions`. A `.wasm` module links the `noctis` imports
  (`log`/`editor_json`/`edit`/`run`) — default-deny by the linker, no WASI — and
  exports `noctis_abi`/`noctis_init`/`noctis_update`, driven once per frame, and
  draws panels through `noctis_alloc`/`noctis_ui` with `ui_panel` and
  `ui_activated`/`ui_value`. Each JavaScript extension runs in **its own** Slag
  context (a realm each): the host owns the contexts, keeps the load order, and
  provides the cross-cutting built-ins (`help` aggregates every context's verbs;
  a verb line is offered to each context in turn) with panel ids offset per
  context. A worker-thread host is still open — `Context`/`Agent` are `!Send`,
  so a worker would have to own the whole host and exchange only the bridge. See
  `.notes/extensions.md` ("Contexts and threads").
- `noctis editor` and `noctis run` open a live windowed viewport (a window is the
  default; `--headless` opts out). Both accept
  `--project <dir>` (a directory with a `noctis.toml`, see [`.notes/project.md`](.notes/project.md)):
  it makes `--scene` project-relative, supplies the default scene when `--scene`
  is omitted, and drives the window title/size, gravity (for the runtime), and the
  `[render]` environment both CLIs precompute image-based lighting from, from the
  manifest; `--scene` alone still opens a loose scene file. The editor draws
  egui panels (outliner, inspector, transform editing), supports click-to-select
  viewport picking (a ray vs. mesh AABBs), and a full transform gizmo (Move /
  Rotate / Scale handles, drawn world- or entity-oriented, one undo step per
  drag) driven through `AppDriver::gizmo`; the toolbar also has Undo/Redo, **Save**
  (Ctrl+S — writes the scene and the prefab library back into the project, or to
  the loose `--scene` file), and a **scene picker** that switches between the
  project's scenes; the outliner's **Delete** button (or Delete/Backspace) removes
  the selection. The
  inspector edits every component of the selection — name, Active/Visible, the
  transform, and Camera/Light/MeshRenderer/RigidBody/Collider/AudioSource/Script
  — with add/remove buttons, asset pickers for meshes/materials/audio clips, and
  an in-place material editor that assigns textures to each map slot. An
  **Import asset** section imports a glTF model (spawning its hierarchy, then
  selecting the root) or a texture file (`Editor::import_texture`, decoded by
  `render::load_texture`). Edits go through
  `Editor::edit_component`/`insert_component`, so they preserve fields the
  command path would reset, and a drag or typing session folds into a single
  undo step. The
  viewport is navigated with WASD (fly), Q/E (down/up), a middle-mouse drag
  (orbit about the selection, or the origin when nothing is selected), Shift +
  middle-mouse (pan) and the scroll wheel (dolly); these bindings live in
  `editor::CameraControls` (loadable from JSON with `--controls <file>`) and are
  remappable. It spawns primitive objects (cube/sphere/plane/light/camera)
  from the Outliner's **Add** buttons and `--testscene` loads the all-features test
  scene. With `--project` the editor imports the project's assets and loads its
  prefabs (`prefabs/*.prefab.json`), and `--save-prefabs` writes the edited prefab
  library back.
  `noctis run --export <dir> --scene <file>` writes a self-contained game folder
  (a copy of `noctis-game`, `scene.json`, and a manifest) that loads its bundled
  scene when run with no arguments; `noctis build` does the same for a project
  (its referenced assets included). The windowed loop is not unit-tested (it
  blocks and needs a display).

## Commit messages

Subject line ≤ 50 chars, imperative, no trailing period, `type:` prefix
(`feat:`, `fix:`, `test:`, `docs:`). Add a body only when it carries real
information (what changed and why, grouped by crate/module with concrete file
names and test counts). See the repo history for examples.
