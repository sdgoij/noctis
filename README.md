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
| `editor` / `runtime` | Editor (AI-controllable) and game runtime/export binaries. |

## Quick start

```sh
cargo test --workspace        # run everything
cargo run -p runtime          # run the sample scene headlessly
cargo run -p editor -- --help # drive the editor from the CLI / scripts
```

Enable the real JavaScript backend:

```sh
cargo test -p script --features slag
```

## Status

The `math`, `ecs`, `render`, `physics`, `audio`, and `script` crates are
implemented and tested. Windowing/presentation (live viewport) is the next
milestone; the editor and runtime are otherwise fully functional headlessly and
through their command-line interfaces, which is what makes them drivable by AI
agents.
