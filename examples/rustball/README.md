# Rust Ball

A minimal Noctis **project** whose gameplay is **Rust** rather than a script. It is
the smallest complete demonstration of the `[rust]` plugin seam (see
[`.notes/embedding.md`](../../.notes/embedding.md)): a custom, scene-authorable
component and the systems that act on it.

The scene holds a ball that keeps bouncing around a coloured floor and respawns
when it drifts off the edge, an upright beacon that spins, and a camera the player
flies.

## What the plugin does

The `rust/` crate is the project's plugin; it installs four systems:

- **paint** (once, on startup) — registers a small PBR palette and colours every
  named object. A project with no imported assets can only name the engine's
  built-in `$default` material, so the plugin supplies the colours (a slate floor,
  a red ball, a blue beacon) and points each `MeshRenderer` at them.
- **spin** — a system that turns every entity carrying the custom component
  **`Spin`** (the **Beacon**) about its local `+Y` axis. `Spin` is the project's
  own component, so the scene carries it as
  `{ "type": "custom", "name": "Spin", "data": { "speed": 1.6 } }`, the editor
  shows its `speed` field, and a script can read or write it through the
  reflection calls (`has_component`/`get_component`/`set_component`).
- **ball** — simulates the ball: gravity, a bounce off the floor (within the
  floor's footprint), a rolling rotation, and a **respawn above the centre** once
  the ball drifts past the floor's edge, falls, and misses it. The launch
  direction cycles on each respawn so the motion varies.
- **camera** — flies the **active camera** from input:
  - hold the **middle mouse button** to look around,
  - **`W`/`A`/`S`/`D`** to move, **`E`/`Q`** for up and down,
  - the **scroll wheel** to dolly along the view.

The ball is **not** a rigid body: a plugin cannot drive the physics engine (it has
no access to the physics world, so no impulses), which is exactly why the ball's
motion lives in this crate. The engine's physics is demonstrated by the showcase
project, not here.

## Layout

```
rustball/
  noctis.toml                  # the manifest: settings, the default scene, [rust]
  scenes/ball.scene.json       # the camera, sun, floor, ball, and the Beacon
  rust/
    Cargo.toml                 # the plugin library crate (depends on `noctis`)
    src/lib.rs                 # the `Spin` component, the plugin, and its systems
```

## Run it

```sh
# In a window (Esc closes it). The stock `noctis` builds the project's
# plugin-aware tool on demand, then runs it.
cargo run -- run --project examples/rustball

# Headless: run 300 frames without a window and print a summary.
cargo run -- run --headless --project examples/rustball --frames 300

# Headless with the GPU on a dummy surface (needs a graphics adapter).
cargo run -- run --headless --project examples/rustball --render --frames 4
```

Because the project carries Rust, `noctis` dispatches through a generated tool
under the project's gitignored `.noctis/tool/`, which links the plugin so the
`Spin` component resolves when the scene loads.

## Edit it

```sh
cargo run -- editor --project examples/rustball
```

The editor opens the project with the plugin's component registered, so the Beacon
shows a **Spin** component in the inspector and **Spin** appears in the
Add-component menu. The editor viewport does not run the game systems, so the
objects keep the scene's `$default` material and the ball and beacon are static
there — use `noctis run` to see the palette, the bouncing ball, and the spin.

## Build it

```sh
# Bundle to dist/release/ (a plugin-linked client, so the game carries `Spin`).
cargo run -- build --project examples/rustball
```

The build produces a per-project client under `.noctis/client/` that links the
plugin's component types and depends on the runtime, never on the authoring tool
chain, so a shipped game has no editor.

## Notes

- There is no `[render] environment` in the manifest, so image-based lighting is the
  neutral (fully white) fallback. Point `[render] environment` at an `.hdr` for a
  richer look.
- The plugin crate is a detached cargo workspace (`[workspace]` in its `Cargo.toml`),
  so it builds into `rust/target/`, not the engine's root `target/`. Its unit tests
  run with `cargo test --manifest-path examples/rustball/rust/Cargo.toml`.
