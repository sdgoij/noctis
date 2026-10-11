# Noctis Showcase

An example **project** for Noctis: a real directory with a `noctis.toml`, several
scenes, prefabs, scripts, and assets, chosen to exercise every part of the
engine — all thirteen components, glTF models with PBR textures, sounds, fonts,
physics, 3D and 2D scenes, and the whole JavaScript scripting API.

It is both a demo to open and a working reference for how a Noctis project is
put together (see [`.notes/project.md`](../../.notes/project.md) for the design).

## Layout

```
showcase/
  noctis.toml                    # the manifest: settings, scene list, build targets
  scenes/
    showcase.scene.json          # the broad scene — every component kind
    physics.scene.json           # a physics playground (all collider shapes)
    menu.scene.json              # a 2D (orthographic) menu
  prefabs/
    Crate.prefab.json            # a spinning, physical crate
    Lamp.prefab.json             # a post with a light on a child entity
  scripts/
    spinner.js  bobber.js  spin2d.js  third_person.js  spawner.js  menu.js
  assets/
    models/drone.glb             # a textured cube: mesh + PBR material + 2 textures
    models/palette.glb           # the named PBR material palette the scenes shade with
    textures/panel_normal.png    # a loose normal map (+ a `srgb = false` sidecar)
    hdris/sky.hdr                # the equirectangular HDR environment (IBL)
    audio/chime.wav              # a one-shot bell arpeggio (non-spatial)
    audio/engine_hum.wav         # a looping drone (spatial)
    fonts/NoctisSans.ttf         # a font, generated from scratch (see below)
  tools/                         # the generators that produced the assets
```

## Run it

The project attaches JavaScript, which the engine always runs. Watch the demo:

```sh
# The default scene (the menu) in a window; Play (Space) loads the showcase.
cargo run -- run --project examples/showcase

# Headless: import the assets, run 120 frames, print a summary.
cargo run -- run --headless --project examples/showcase --frames 120

# Headless with the GPU on a dummy surface (needs a graphics adapter).
cargo run -- run --headless --project examples/showcase --render --frames 4
```

Run a different scene with `--scene`, project-relative:

```sh
cargo run -- run --project examples/showcase --scene scenes/physics.scene.json
cargo run -- run --project examples/showcase --scene scenes/menu.scene.json
```

## Edit it

The editor opens the project, imports its assets and prefabs, and shows a live
viewport (WASD/QE to fly, middle-mouse to orbit/pan, wheel to dolly):

```sh
cargo run -- editor --project examples/showcase
```

The editor viewport is authoring-only: it does not run scripts, so a scene with
behaviours looks static there. Use `noctis run` to watch them run.

The editor is fully drivable by an agent through its JSON command API — spawn
entities, set transforms, add components, import a glTF, save a prefab, and write
the scene back:

```sh
cargo run -- editor --print-schema
cargo run -- editor --project examples/showcase --headless \
    --command '[{ "op": "spawn", "name": "Box" },
                { "op": "add_rigid_body", "entity": { "name": "Box" } }]' \
    --out scenes/new.scene.json
```

Entities are referenced **by name** (or by `{ "index": .., "generation": .. }`),
so a batch reads like a script: `spawn`, then act on the name.

### Create a 2D scene from the command line

A scene's `mode` is a presentation hint, not a schema fork. `--mode 2d` records
`"mode": "2d"` on a scene created from scratch, and a 2D scene wants an
*orthographic* camera. This batch builds a small 2D board from nothing — the
commands apply in order and address earlier entities by name:

```sh
cargo run -- editor --project examples/showcase --headless \
    --scene scenes/board.scene.json --mode 2d \
    --command '[
      { "op": "spawn", "name": "Camera" },
      { "op": "set_translation", "entity": { "name": "Camera" }, "translation": [0, 0, 10] },
      { "op": "add_camera", "entity": { "name": "Camera" }, "params": {
          "projection": { "type": "orthographic", "height": 10.0, "near": 0.1, "far": 100.0 },
          "clear_color": [0.03, 0.04, 0.07, 1.0] } },

      { "op": "spawn", "name": "Backdrop" },
      { "op": "set_translation", "entity": { "name": "Backdrop" }, "translation": [0, 0, -2] },
      { "op": "set_scale", "entity": { "name": "Backdrop" }, "scale": [20, 12, 0.2] },
      { "op": "set_mesh_renderer", "entity": { "name": "Backdrop" }, "params": {
          "mesh": { "path": "primitives/cube" }, "material": { "path": "Ink" },
          "cast_shadows": false, "receive_shadows": false } },

      { "op": "spawn", "name": "Spinner" },
      { "op": "set_translation", "entity": { "name": "Spinner" }, "translation": [-4, -3, 0] },
      { "op": "set_mesh_renderer", "entity": { "name": "Spinner" }, "params": {
          "mesh": { "path": "primitives/cube" }, "material": { "path": "Gold" } } },
      { "op": "attach_script_path", "entity": { "name": "Spinner" }, "path": "scripts/spin2d.js" },

      { "op": "spawn", "name": "Fill Light" },
      { "op": "set_translation", "entity": { "name": "Fill Light" }, "translation": [0, 0, 5] },
      { "op": "add_light", "entity": { "name": "Fill Light" }, "params": {
          "kind": { "type": "directional", "direction": [0, 0, -1] },
          "color": [1, 1, 1], "intensity": 1.0, "cast_shadows": false } }
    ]' \
    --out scenes/board.scene.json
```

That writes `scenes/board.scene.json` with `"mode": "2d"`, an orthographic
camera, an `Ink` background quad, a spinning `Gold` square, and a light. Open it
in the editor (`noctis editor`, then pick it in the outliner) or run it directly:

```sh
cargo run -- run --project examples/showcase --scene scenes/board.scene.json
```

`Text` is a first-class command now. Appending
`{ "op": "add_text", "entity": { "name": "Title" }, "params": { "text":
"Tic-Tac-Toe", "size": 0.6, "font": { "path": "assets/fonts/NoctisSans.ttf" } } }`
(after a `spawn`) schedules a label, and `Text` is in the `component_name` list
`remove_component` accepts — so the whole walkthrough is CLI-only.

## Build it

```sh
# Bundle the default scene to dist/release/.
cargo run -- build --project examples/showcase

# A named [[build]] target gets its own folder, e.g. dist/release/showcase-physics/.
cargo run -- build --project examples/showcase --target showcase-physics
```

A build copies the `noctis-game` client (built on demand when missing), the
scene, every asset the scene references, and a generated `noctis.toml`; run the
executable in its folder with no arguments to play the bundled scene.

The default `release` profile bakes the default scene's scripts into the client,
so the game folder carries no `scripts/` directory. `--profile dev` copies nothing
and runs the project in place, so the client hot-reloads a script (`.js`/`.wasm`)
the moment you save it (see [`.notes/scripts.md`](../../.notes/scripts.md)). Builds are
organised by profile (`dist/release/`, `dist/dev/`), so a dev build does not
overwrite the release build.

```sh
cargo run -- build --project examples/showcase --profile dev
```

`noctis rename` moves an asset and rewrites every reference to it:

```sh
cargo run -- rename --project examples/showcase \
    assets/textures/panel_normal.png assets/textures/panel_data.png
```

## What each scene shows

**`scenes/showcase.scene.json`** (the default) — the broad one. A parented
hierarchy, two cameras (one active, one inactive with a higher render priority),
the three light kinds (one shadow-casting directional plus a point and a spot),
every built-in primitive mesh shaded with the named palette materials, a glTF
mesh with its imported material and textures, rigid bodies and all four collider
shapes (including a sensor), spatial and non-spatial audio, and a billboarded
`Text` label drawn from the generated font. It also carries entities an editor
would toggle: one `active: false`, one `visible: false` — the only way `Active`
and `Visible` appear in a scene document.

**`scenes/physics.scene.json`** — a playground: a ground plane, a tilted ramp, a
stack of dropping cubes, a bouncy and a heavy ball, a capsule, a fixed pillar, a
**kinematic** platform driven by `bobber.js`, and a sensor pad (a flattened
trigger volume you walk over).

**`scenes/menu.scene.json`** — a 2D scene (`"mode": "2d"`) with an orthographic
camera, a backdrop, a title and subtitle, a `PLAY` button, and a spinning
prompt driven by `spin2d.js` (rotation about Z). It is the project's default
scene; `menu.js` on the Play button calls `load_scene` to start the showcase.

## Scripting

Behaviours are plain globals — **no `api` object, no `export`**:

```js
function on_start(entity, dt)  {}   // once, when the script first runs
function on_update(entity, dt) {}   // every frame
function on_destroy(entity, dt) {}  // when the entity is removed
```

`entity` is a numeric handle and `dt` is the frame time in seconds. Vector
getters (`get_position`, `get_rotation_euler`, `get_scale`) return a
`"[x,y,z]"` **string**; the scripts here parse it with a small `vec3` helper.
The rest of the surface: `spawn`, `destroy`, `destroy_by_name`, `find_entity`,
`set_position`/`translate`, `set_rotation_euler`, `set_scale`, `time_delta`,
`time_elapsed`, `key_down` (winit key names such as `KeyW`, `Space`,
`ArrowUp`), `mouse_down` (`left`/`right`/`middle`), pointer motion
(`mouse_move` → `"[dx,dy]"`, `mouse_position` → `"[x,y]"`, `mouse_scroll`),
driving a dynamic body (`set_velocity`/`add_velocity`/`apply_impulse`, applied to
physics by the runtime; `get_velocity` and `is_grounded` read back this frame's
snapshot), `add_script`, `log`, `instantiate_prefab`, and the component reflection
calls `has_component`, `get_component`, `set_component`, `remove_component` (any
component — a Rust project's custom components or a built-in, whose JSON is its
scene-descriptor form; see `.notes/embedding.md`), and `load_scene`, which replaces
the running scene with another the project ships (see `.notes/controls.md`).
`viewport_size()` gives the window size (to map the cursor into a 2D scene) and
`set_mouse_captured(bool)` captures or releases the OS cursor.

- `menu.js` — the title screen: it releases the cursor (`set_mouse_captured(false)`)
  and drives a custom `Cursor` entity to the pointer, and pressing
  **Space**/**Enter** (or clicking) calls `load_scene("scenes/showcase.scene.json")`,
  so Play hands off to the showcase. The game starts on this scene (the project's
  default).
- `spinner.js` — spins an entity about Y (attach it to see it turn).
- `bobber.js` — floats an entity up and down around its start height.
- `spin2d.js` — spins about Z, for a 2D scene.
- `third_person.js` — the Player's controller: **WASD** moves in the yaw frame,
  **Space** jumps when grounded, the **mouse** turns the player (X) and tilts a
  follow camera (Y). The body is kinematic; the script integrates its own gravity
  (see `is_grounded`, `translate`, `mouse_move`, `get_position`).
- `spawner.js` — **E** spawns a physical crate (a cube mesh, a dynamic body, and a
  collider set through `set_component`, plus a lifetime via `add_script`), **R**
  instantiates a prefab from inline JSON, **X** destroys everything it created
  (`destroy` and `destroy_by_name`). A crate that falls off the floor, or outlives
  its lifetime, is removed.

`bobber.js` and the other scripts are also covered by an integration test that
runs them in the real engine:

```sh
cargo test -p script --test showcase_scripts
```

## Prefabs

`prefabs/*.prefab.json` are reusable subtrees. The editor loads the library on
open (so `instantiate_prefab` resolves a name) and writes it back with
`--save-prefabs`. At runtime a script instantiates a prefab from inline JSON (see
`spawner.js`), which is why the verbatim prefab document travels inside the
script.

## Assets are generated, not committed as art

Everything under `assets/` is produced by the two scripts in `tools/`, so the
project is reproducible from a clean checkout with only Python's standard library
and no third-party art:

```sh
python tools/make_font.py        # assets/fonts/NoctisSans.ttf
python tools/generate_assets.py  # the models, textures, environment, and audio
```

`make_font.py` builds a small TrueType font from scratch (a 5×7 pixel bitmap,
emitted as a well-formed sfnt with `head`/`glyf`/`cmap`/`hmtx`/… and a real
`head.checkSumAdjustment`) — shipping a system font would mean committing a font
we do not have the rights to redistribute. It is a caps-style font, so text is
readable in uppercase.

The model's PBR textures ride **inside** `drone.glb`, and the named PBR
**materials** the scenes shade with live in `palette.glb`: the engine imports a
loose `.png` as a texture and a loose `.gltf`/`.glb` as a model, but a scene
references *materials*, and the glTF loader is the only importer that produces
one today — so both the textures and the material palette travel inside a model.
`Slate`/`Steel`/`Copper`/`Gold`/`Mint`/`Coral`/`Glow`/`Ink` are registered under
those names, and a scene says `"material": "Gold"`.

`assets/textures/panel_normal.png` is a loose data map instead, with a
`panel_normal.png.import.toml` sidecar (`srgb = false`); it is the kind of file
you assign in the editor's material editor, and it demonstrates the texture
importer, per-asset import settings, and the `.noctis/` decode cache.

`assets/hdris/sky.hdr` is the scene's **image-based lighting**: a real
Radiance `.hdr` panorama (written in the new-style per-component RLE the decoder
requires), a hazy sky with a bright sun so the environment has a *direction*. The
manifest points at it —

```toml
[render]
environment = "assets/hdris/sky.hdr"
```

— and the renderer precomputes the irradiance, prefiltered specular, and BRDF LUT
from it when the game or the editor viewport opens. It is what makes the metal
reflect the sky, gives the surfaces a shadowed side, and keeps the floor from
washing out; without it the renderer falls back to a neutral (fully white)
environment, which is flat but usable.

## Notes and limitations

- **IBL is CPU-precomputed** when the game (or the editor viewport) starts, from
the `[render] environment` the manifest names; there is no GPU precompute and no
run-time re-bake, so changing the environment means restarting. An `.exr`
environment is not decodable yet (only `.hdr`).
- **Anti-aliasing** is MSAA: `[render] sample_count = 4` here. It smooths
  geometry silhouettes — the sphere, capsule, cylinder and torus edges — but not
  shader detail: a hard texel edge (the drone's checker) or a glyph's coverage
  still steps. That needs alpha-to-coverage or a post-process AA pass, neither of
  which exists yet. A device that cannot do 4x degrades to 2x or 1x rather than
  failing.
- **Prefabs are not bundled by `noctis build`** — only assets a scene references
  are. The runtime path (`instantiate_prefab` with inline JSON) still works in an
  exported game.
- Only **one shadow-casting** directional/spot light is used.
- `.noctis/` (the import cache) and `dist/` (build output) are generated and
  gitignored.
- The characters the generated font covers are `A–Z`, `a–z` (folding to caps),
  the digits, and `. : - !`; other characters render blank.

## Tests

The example is covered by the workspace suite, so it cannot silently rot:

```sh
cargo test --workspace
```

- `crates/project/tests/showcase.rs` — imports every asset, loads and bundles
  every scene, checks the showcase scene's component coverage, loads the
  prefabs, and bakes the generated font.
- `crates/project/tests/render_previews.rs` — renders each scene on a headless
  GPU and requires a non-blank frame (skips when there is no adapter). Set
  `NOCTIS_PREVIEWS=target/previews` to also write the frames out as PNGs.
- `crates/script/tests/showcase_scripts.rs` — runs all five scripts in the real
  engine (`cargo test -p script --test showcase_scripts`).
- `crates/runtime/tests/showcase_scenes.rs` — runs each scene through the runtime
  with the real Slag engine and requires its scripts to move their entities (the
  spinners turn, the bobbers leave their authored height) without errors
  (`cargo test --manifest-path crates/runtime/Cargo.toml --test showcase_scenes`).
