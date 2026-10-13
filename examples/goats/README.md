# Goats

A Noctis port of the [Slag Goat](../../goats) sandbox: a JavaScript-and-raylib 3D
goat sandbox, rebuilt as a Noctis project whose gameplay is a Rust plugin. The
full plan is [`.notes/goats.md`](../../.notes/goats.md).

**M0 — the render-path spike.** This milestone proves the render path and nothing
more:

- the goat model (`content/goat_animated.glb`, embedded) loads as a **static
  mesh** at bind pose — the fourteen animation clips are not played yet
  ([`.notes/animation.md`](../../.notes/animation.md));
- the ground is a `48×48` mesh built from the original's own three-octave noise
  (`src/terrain.rs`, ported from `goats/crates/goats/src/game/world.js`);
- an **orbit camera** frames the goat (drag to orbit, wheel to zoom, arrow keys as
  a keyboard fallback), and a HUD line reports the frame time.

Run it:

```sh
cargo run -r -- run --project examples/goats
```

(`cargo run -r` releases the engine, which also builds the plugin's tool
optimized — a `run` follows the engine's profile, so this is the fast path.)

## Layout

| Path | What it is |
| --- | --- |
| `Cargo.toml` | The plugin crate (a `Cargo.toml` at the project root is what makes `noctis` treat the project as Rust) |
| `src/lib.rs` | The `#[noctis::plugin]` type: spawns the terrain and the goat, drives the camera and the HUD |
| `src/terrain.rs` | The heightfield: the original's `terrainShape`/`terrainBaseHeight` and the grid-mesh builder |
| `scenes/meadow.scene.json` | The scene: a camera and a sun; the plugin adds the terrain and the goat |
| `content/goat_animated.glb` | The rigged goat, embedded with `include_bytes!` |

## What is not here yet

M0 is deliberately the smallest slice that proves the render path. The goat does
not move, the ground does not scroll, and there is no grass, sky, water, weather,
herd, or audio — those are M1 onward (`../.notes/goats.md`).
