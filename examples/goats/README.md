# Goats

A Noctis port of the [Slag Goat](../../goats) sandbox: a JavaScript-and-raylib 3D
goat sandbox, rebuilt as a Noctis project whose gameplay is a Rust plugin. The
full plan is [`.notes/goats.md`](../../.notes/goats.md), and the test harness is
[`.notes/harness.md`](../../.notes/harness.md).

**M0-M3 are shipped.** The goat model loads as a skinned mesh and plays its baked
clips — idle, the three gaits, a jump, sleep, eat, death, and the flung arc —
through the engine's skeletal animation. The ground is built from the original's
own noise. An orbit camera follows the goat, with a HUD. The world sim adds health
and energy (exhaustion, death, restart), the meadow (procedural grass, and grazing
it), the weather state machine, and the water table (rain-filled pools that slow a
wading goat and hide the tufts they cover). The hazards add mines and trapped
tufts, blasts, craters dished into the ground, and the ballistic fling.

Run it:

```sh
cargo run -r -- run --project examples/goats
```

(`cargo run -r` releases the engine, which also builds the plugin's tool
optimized — a `run` follows the engine's profile, so this is the fast path.)

## Layout

The project is a cargo workspace. `noctis` finds a project's Rust through the
root `Cargo.toml`, so a virtual workspace names its plugin crate as the first
`default-members` entry — here `crates/plugin` — and the simulation is the `goats`
crate beside it.

| Path | What it is |
| --- | --- |
| `Cargo.toml` | The workspace root; its `default-members` name the plugin and the simulation |
| `crates/plugin/` | The plugin crate: the `#[noctis::plugin]` type — it owns the `World` and drives it a frame at a time |
| `crates/goats/src/lib.rs` | The simulation: the goat, its state machine, the world, `apply_frame` |
| `crates/goats/src/terrain.rs` | The heightfield and the craters a blast leaves |
| `crates/goats/src/grass.rs` | The meadow: procedural tufts, grazing, and the culls |
| `crates/goats/src/water.rs` | The water table |
| `crates/goats/src/weather.rs` | The weather state machine |
| `crates/goats/src/hazards.rs` | Mines, traps, blasts and the fling |
| `crates/goats/content/goat_animated.glb` | The rigged goat, embedded with `include_bytes!` |
| `scenes/meadow.scene.json` | The scene: a camera and a sun; the plugin adds the world |
| `assets/hdris/meadow.hdr` | The `[render] environment` the lighting is precomputed from |

## What is not here yet

The weather's clouds and rain and the water's surface (M5, the presentation
layer), the rest of the food model and the world, the herd (M4), the menus (M6),
mods (M7) and multiplayer (M8) — all in [`.notes/goats.md`](../../.notes/goats.md).
