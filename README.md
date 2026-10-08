# Noctis

A production-grade, simple and straight-forward 3D game engine for Windows,
macOS and Linux. Rust core, [wgpu](https://wgpu.rs) renderer, rapier physics,
rodio audio, and JavaScript scripting via [Slag](https://github.com/sdgoij/slag).

See [`spec.md`](spec.md) for the design goals, [`docs/project.md`](docs/project.md)
for the project model, [`docs/core-api.md`](docs/core-api.md) for the frozen
`math`/`ecs` interface, and [`AGENTS.md`](AGENTS.md) for how to work in this
repository.

## Crates

| Crate | Role |
|---|---|
| `math` | Vectors/matrices (glm), quaternions, transforms, bounds. |
| `ecs` | ECS, components, scenes, assets, app loop. |
| `render` | PBR renderer: glTF, IBL, shadows, SSAO, HDR tonemapping, gizmos, text. |
| `physics` | rapier3d integration. |
| `audio` | rodio playback + 3D spatialization + file decoding. |
| `script` | Script runtime + engine API (Slag JS/WASM backend optional). |
| `testscene` | The all-features scene used for testing. |
| `project` | The project model: `noctis.toml`, asset paths, import, build. |
| `editor` / `runtime` | Editor (AI-controllable), game runtime, and the `noctis` CLI. |
| `platform` | Window + winit event loop + egui overlay; presents a live viewport. |

## Quick start

```sh
cargo test --workspace        # run everything
cargo run -p runtime                      # run the sample scene headlessly
cargo run -p runtime -- --window --demo   # open the demo scene in a window
cargo run -p editor  -- --testscene --window   # open the all-features test scene
cargo run -p editor  -- --help            # drive the editor from the CLI / scripts
cargo run -p runtime --bin noctis -- --help    # the project build/rename tool

cargo run -p runtime -- --project examples/showcase --window   # the example project
```

[`examples/showcase`](examples/showcase) is a complete example project — scenes,
prefabs, scripts, and generated assets exercising every engine feature. See its
[README](examples/showcase/README.md).

Enable the real JavaScript backend:

```sh
cargo test -p script --features slag
```

## Usage

### What a project is

A **project** is a directory with a `noctis.toml` manifest. It owns one shared
asset registry, zero or more scenes, prefabs, and scripts, and it defines the
game's build targets. The full design is [`docs/project.md`](docs/project.md).

```
mygame/
  noctis.toml           # the manifest (required)
  assets/
    models/             #   .glb/.gltf
    textures/           #   .png/.jpg/...
    hdris/              #   .hdr equirectangular environments (image-based lighting)
    audio/              #   .wav/.ogg/.mp3
    fonts/              #   .ttf/.otf
  scenes/               #   *.scene.json
  prefabs/              #   *.prefab.json
  scripts/              #   *.js / *.wasm
  .noctis/              #   GENERATED cache — add to .gitignore
```

These directories are conventions, overridable with `[paths]`. Assets are
referenced by their **project-relative path** (the form written into scenes), and
a glTF sub-asset is addressable as `assets/models/robot.glb#mesh/Torso`.

A minimal manifest:

```toml
[project]
name = "MyGame"

[project.settings]
default_scene = "scenes/main.scene.json"
mode = "3d"                       # default for new scenes; a scene may override
gravity = [0.0, -9.81, 0.0]

[window]
title = "MyGame"
width = 1280
height = 720
```

`noctis.toml` is parsed strictly — an unknown key is an error, not silent forward
compatibility. `[[build]]` targets and an optional `[[scene]]` list (names and
order) are also supported; see `docs/project.md`.

### The workflow

```sh
# 1. Author: open the project, import its assets, edit a scene, save it back.
cargo run -p editor -- --project mygame --window             # interactive
cargo run -p editor -- --project mygame \
    --command '[{ "op": "spawn", "name": "Box" }]' --out mygame/scenes/main.scene.json

# 2. Run: import the project's assets and play its default scene.
cargo run -p runtime -- --project mygame --window

# 3. Build: export a self-contained, distributable game folder.
cargo run -p runtime --bin noctis -- build --project mygame
```

With `--project <dir>` both CLIs import every asset file into one registry,
make `--scene` project-relative, and use the project's `default_scene` when
`--scene` is omitted. `--scene` on its own still works for a loose, self-contained
scene file.

**Assets** are imported automatically on open: models (glTF), textures, HDR
environments (`.hdr`), audio (WAV/Ogg/MP3), fonts, and scripts. A file that fails
to import is reported as a warning and does not stop the project opening; a
*scene* that references a missing asset is a load error naming the path. Texture
decoding is cached under `.noctis/` (add that directory to your `.gitignore`).
Per-asset settings live in a sidecar next to the file, for example
`assets/textures/wood.png.import.toml`:

```toml
srgb = false     # a data map (normal/roughness); omit for colour maps
```

**Image-based lighting** comes from an HDR environment the manifest names:

```toml
[render]
environment = "assets/hdris/sky.hdr"   # equirectangular; omit for a neutral env
sample_count = 4                       # MSAA samples (1 disables); clamped to the device
```

The `.hdr` imports into the `hdr` store as linear floats, and the renderer
precomputes the diffuse irradiance, the prefiltered specular chain, and the BRDF
LUT on the CPU when the game (or the editor viewport) starts. Drop the setting and
surfaces fall back to a neutral, fully-lit white environment — flat, and only
useful as a placeholder.

**Prefabs** live in the project as `prefabs/<name>.prefab.json`. The editor loads
them on open (so `instantiate_prefab` resolves them) and `--save-prefabs` writes
the edited library back.

### Scenes and 2D / 3D (`--mode`)

There is one scene format; a scene carries a `mode` of `"2d"` or `"3d"` (a
presentation hint, not a schema fork). A project's `mode` is the default for new
scenes, and a scene may override it.

```sh
# Create / open a 2D scene; in a 2D scene the editor adds an orthographic camera.
cargo run -p editor -- --scene menu.scene.json --mode 2d --out menu.scene.json

# A project with mode = "2d" makes new scenes 2D automatically.
cargo run -p editor -- --project mygame
```

An existing scene keeps the mode stored in its file; `--mode` only applies when a
scene is created from scratch.

### In-world text (`Text`)

A `Text` component draws billboarded glyphs at an entity's position, from a font
asset. Fonts import automatically from `assets/fonts/*.ttf|*.otf`; the glyph
atlas is baked on demand and cached in the renderer.

In the editor: select an entity, **Add component → Text**, then edit the string,
font, size (line height in world units), colour, anchor, and depth-test flag in
the inspector.

In a scene document it is:

```json
{
  "type": "text",
  "text": "Hello, Noctis",
  "font": "assets/fonts/body.ttf",
  "size": 1.0,
  "color": [1.0, 0.9, 0.4, 1.0],
  "anchor": "center",
  "depth_test": true
}
```

`anchor` is one of `center` (default), `top_left`/`top_center`/`top_right`,
`middle_left`/`middle_right`, `bottom_left`/`bottom_center`/`bottom_right`.

### Building and exporting (`noctis build`)

```sh
# Export the project's default scene (or --scene) to <project>/dist/<name>.
cargo run -p runtime --bin noctis -- build --project mygame

# Build a named [[build]] target (its scene, name, and output are used).
cargo run -p runtime --bin noctis -- build --project mygame --target release

# Override the output or name.
cargo run -p runtime --bin noctis -- build --project mygame --out dist/demo --name demo
```

A build writes a self-contained folder: the runtime executable, `scene.json`,
every asset the scene references (resolved transitively through the import index),
a generated `noctis.toml` so the exported game imports its own assets, and
`noctis-game.json`. Running the exported executable with no arguments loads its
bundled scene. A `[[build]]` target looks like:

```toml
[[build]]
name = "mygame"
scene = "scenes/main.scene.json"  # defaults to project.settings.default_scene
output = "dist/mygame"            # project-relative; defaults to dist/<name>
platform = "host"                 # "host" | "windows" | "macos" | "linux"
profile = "release"               # "dev" | "release"
```

### Renaming assets (`noctis rename`)

Renaming or moving an asset is a first-class operation: it moves the file and
rewrites every reference to it (including `#fragment` sub-asset references) in the
project's scenes and prefabs, so no scene is left with a broken path.

```sh
cargo run -p runtime --bin noctis -- rename --project mygame \
    assets/models/robot.glb assets/models/mech.glb
```

The rewrite is textual, so documents keep their exact formatting: a rename that
touches no reference changes no bytes.

### Command reference

`noctis` (project tool):

```
noctis build  --project <DIR> [--target <NAME>] [--scene <PATH>] [--name <NAME>] [--out <DIR>]
noctis rename --project <DIR> <FROM> <TO>
```

`noctis-editor`:

```
--scene <PATH>     open or create a scene (project-relative with --project)
--project <DIR>    open a project; imports its assets and prefabs
--mode <2d|3d>     presentation mode for a scene created from scratch
--command <SRC>    apply a JSON command batch (file, `-` for stdin, or inline)
--out <PATH>       save the resulting scene
--window           open a live viewport (WASD/QE fly, MMB orbit/pan, wheel zoom)
--testscene        load the engine's all-features scene
--save-prefabs     write the prefab library to the project's prefabs/
--controls <PATH>  camera-navigation bindings (JSON)
--print-schema     print the JSON command schema
```

`noctis-runtime`:

```
--scene <PATH>     run a scene JSON (project-relative with --project)
--project <DIR>    open a project; the default scene runs when --scene is omitted
--target <NAME>    export a named [[build]] target
--demo             run a small built-in scene
--frames <N>       frames to run headlessly (default 300)
--render           render each frame with a headless GPU (skipped if unavailable)
--window           run in a window
--export <DIR>     export a self-contained game folder
--name <NAME>      exported executable name
--wasm <name>=<path>   register a .wasm script (requires the `slag` feature)
--no-audio         do not open an audio device
--print-summary / --quiet
```

## Pre-commit hook

The `hooks/pre-commit` script blocks commits whose code fails
`cargo fmt --check` or `cargo clippy --workspace --all-targets -- -D warnings`.
Point Git at it once per clone:

```sh
git config core.hooksPath hooks
```

## Status

All crates are implemented and tested. The renderer presents to a real window
(`--window`), and the editor draws an [egui](https://github.com/emilk/egui)
interface (outliner, inspector, viewport picking, transform gizmo) over a live
viewport. The whole engine also runs headlessly for CI, so `cargo test
--workspace` passes without a GPU or an audio device (GPU and font tests skip
gracefully when unavailable). Known limitations are tracked in
[`AGENTS.md`](AGENTS.md).
