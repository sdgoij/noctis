# Noctis

A production-grade, simple and straight-forward 3D game engine for Windows,
macOS and Linux. Rust core, [wgpu](https://wgpu.rs) renderer, rapier physics,
rodio audio, and JavaScript scripting via [Slag](https://github.com/sdgoij/slag).

See [`spec.md`](spec.md) for the design goals and [`AGENTS.md`](AGENTS.md) for how
to work in this repository.

## Crates

| Crate | Role |
|---|---|
| `math` | Vectors/matrices (glm), quaternions, transforms, bounds. |
| `ecs` | ECS, components, scenes, assets, app loop. |
| `render` | PBR renderer: glTF, IBL, shadows, SSAO, HDR tonemapping, gizmos. |
| `physics` | rapier3d integration. |
| `audio` | rodio playback + 3D spatialization. |
| `script` | Script runtime + engine API (Slag backend optional). |
| `testscene` | The all-features scene used for testing. |
| `project` | The project model: `noctis.toml`, asset paths, asset import. |
| `editor` / `runtime` | Editor (AI-controllable) and game runtime/export binaries. |
| `platform` | Window + winit event loop + egui overlay; presents a live viewport. |

## Quick start

```sh
cargo test --workspace        # run everything
cargo run -p runtime          # run the sample scene headlessly
cargo run -p runtime -- --window --demo   # open the demo scene in a window
cargo run -p editor -- --testscene --window   # open the all-features test scene
cargo run -p editor -- --help # drive the editor from the CLI / scripts
```

Enable the real JavaScript backend:

```sh
cargo test -p script --features slag
```

### Pre-commit hook

The `hooks/pre-commit` script blocks commits whose code fails
`cargo fmt --check` or `cargo clippy --workspace --all-targets -- -D warnings`.
Point Git at it once per clone:

```sh
git config core.hooksPath hooks
```

## Status

All crates — `math`, `ecs`, `render`, `physics`, `audio`, `script`, `testscene`,
`project`, `editor`, `runtime`, and `platform` — are implemented and tested. The
renderer presents to a real window (`noctis-runtime --window`,
`noctis-editor --window`), and the editor draws an
[egui](https://github.com/emilk/egui) interface (outliner, inspector, live
transform editing) over the viewport. The whole engine also runs headlessly for
CI. Click-to-select viewport picking is implemented (a ray vs. mesh bounds); the
editor can spawn primitive objects (cube/sphere/plane/light/camera) with the
Outliner's **Add** buttons, and `--testscene` loads the all-features test scene;
the transform gizmo (Move/Rotate/Scale) is implemented.

Both CLIs accept `--project <dir>`: it opens a project (a directory with a
`noctis.toml`, see [`docs/project.md`](docs/project.md)), imports its assets into
one shared registry, and makes `--scene` project-relative. A scene carries a
presentation `mode` (`2d` | `3d`); the editor creates new scenes in the project's
mode (or `--mode`), and a 2D scene's default camera is orthographic.

`noctis build --project <dir> [--target <name>]` exports a project into a
self-contained game folder: the runtime executable, the scene, every asset the
scene references (resolved transitively through the import index), and a
generated `noctis.toml` so the exported game imports its bundled assets and runs
its bundled scene when launched. `noctis rename --project <dir> <from> <to>` moves
an asset and rewrites every reference to it in the project's scenes and prefabs.

In-world text renders: a `Text` component draws billboarded glyphs from a font
asset (`render::text` bakes a glyph atlas on demand, using
[`ab_glyph`](https://docs.rs/ab_glyph)), in a depth-tested, alpha-blended pass.
