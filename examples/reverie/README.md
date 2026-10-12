# Reverie Amidst Ruins — Noctis port

A port of Dave Eddy's terminal platformer
[**reverie-amidst-ruins**](https://github.com/bahamas10/reverie-amidst-ruins)
(MIT) to a Noctis window. The original renders each frame to an RGBA image and
ships it to the terminal over the Kitty graphics protocol; this version renders
the same ASCII levels as flat, unlit quads on an orthographic camera.

## Run it

From the workspace root:

```sh
cargo run -- run --project examples/reverie
```

Controls: `A`/`D` or `←`/`→` move, `Space`/`W`/`↑` jump, `R` restarts the level.

## How it works

A Noctis project is `noctis.toml` + `scenes/*.scene.json` + `scripts/*.js`, and
this port keeps the original's shape: the **scene is the level's artwork**, and a
**single script is the game loop**.

| Original (Rust)              | Here |
| ---------------------------- | ---- |
| `level.rs` (ASCII → grid)     | `tools/build_levels.py` bakes the grid into the scene *and* the script |
| `render.rs` (tiles → pixels)  | ground runs as quads with the unlit `Ground`/`GroundTop` materials |
| `player.rs` (tile collision)  | `scripts/levelN.js` — the same `approach`/`resolve_collision` math, ported to JS |
| `camera.rs` (dead-zone follow)| `scripts/levelN.js` `followCamera` |
| `screen.rs`/`main.rs`         | one scene per level; the goal calls `load_scene` |
| `main.rs` HUD line            | a `Text` entity the script repositions and rewrites |

Coordinates and constants (tile 16, gravity 800, jump 300, max fall 450, …) are
the original's; the script keeps the level's y-down tile space and flips to the
engine's y-up world only when it places the player.

**Everything blue here is data or generated code** — run the tools to rebuild it:

```sh
python tools/build_assets.py   # assets/models/palette.gltf (the unlit materials)
python tools/build_levels.py   # scenes/*.scene.json and scripts/level*.js
```

The materials are `KHR_materials_unlit`, and the engine renders unlit quads with
`Material::unlit` (added in the `feat(render): unlit materials and nearest
sampling` commit) — the flat-shading path a 2D game needs.

## Not ported yet

- **Audio** — the original plays looping music per level and jump/coin/death
  SFX. Scripts have no audio verb, so music would need an `AudioSource` entity
  per scene and the SFX have no trigger today.
- **Post-processing** — the vignette/scanlines are a full-screen CPU filter with
  no engine equivalent.
- **Title and end screens** — there are no `title`/`end` scenes yet.
- **Backgrounds, palette randomise, screenshots, image scaling, gamepad** — the
  levels currently clear to the palette's darkest colour; the tile art is flat
  colours rather than the original tileset (no sprite atlas / palette lookup).
