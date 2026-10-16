# Reverie Amidst Ruins — Noctis plugin port

A port of Dave Eddy's terminal platformer
[**reverie-amidst-ruins**](https://github.com/bahamas10/reverie-amidst-ruins)
(MIT) to a Noctis window.

This is a **project Rust** port: the project carries a `Cargo.toml` + `src/lib.rs`
(a Noctis *plugin*), and the plugin runs the original game — vendored almost
verbatim under `src/game/` — as its own software renderer. Every tick it draws the
game to an RGBA image (the original's `render.rs`) and publishes it through
`EngineContext::frame`; the window host blits that frame across the window and
does **not** render the engine's scene.

## Run it

From the workspace root:

```sh
# Play (builds the plugin optimized — see Performance below):
cargo run -- run --release --project examples/reverie

# Iterate (unoptimized; the game loop is ~10x slower):
cargo run -- run --project examples/reverie
```

Controls (as the original): `A`/`D` or `←`/`→` move, `Space`/`W`/`↑` jump,
`Enter` confirm, `R` restart, `1`/`3`/`4` effects/palette/debug, `2` screenshot,
`m` mute, `8`/`9` volume, `Esc`/`Q` quit.

## Level editor

The project ships an **editor extension** (`extensions/level-editor/`, wired up by
the `[extensions]` table in `noctis.toml`) that edits `levels/*.txt` visually in
the editor's viewport and validates the result as you paint.

```sh
cargo run -- editor --project examples/reverie
```

The extension draws the level grid over the viewport (`noctis.ui.viewport`), reads
and writes the project's level files by slot (`noctis.files`), and adds a
**Reverie Level** panel and a `level` verb.

| Input | Effect |
| --- | --- |
| Left click / drag | Paint the selected brush |
| Right click / drag | Erase to air |
| Wheel | Zoom about the cursor |
| Shift + left drag | Pan |

Brushed with the panel's buttons (or `level brush <char>`): `G` ground, `P`
platform, `C` coin, `S` spawn, `F` goal, `.` erase. Painting the spawn or the goal
*moves* it — the game allows exactly one of each.

### Music, background and size

The panel also sets the level's **music** and **background**, each chosen from a menu
of exactly the names the game's own tables know (`SONGS` in `src/game/audio.rs`, the
backgrounds `src/game/render.rs` draws), because the game *refuses to load* a level
naming anything else. An unknown name is a structural error, so the editor will not
let you save one. The settings sit at the top of the panel, above the level list:
a dock's height is capped (a long panel must not starve the viewport), so only the
growing lists are ever scrolled out of sight.

**New** makes a blank level at the panel's size, and **Resize** applies that size to
the current level: growing pads the new space with air, and shrinking is refused
when it would cut off the spawn, the goal or a coin — the editor never silently
drops something you placed. (`level new [<width> <height>]`,
`level resize <width> <height>`.)

### What it validates

- **Structural** (instant, after every edit): the metadata the game's parser
  indexes by name (`name`, `music`, `background`, `palette`), the four-colour
  `#rrggbb` palette, exactly one spawn and one goal, only tile characters the
  parser knows, and a spawn or goal that is not buried in a tile. Flagged cells are
  outlined in red and the field's border turns red.
- **Reachability** (a background sweep): it re-runs the game's own movement
  integrator (`src/game/player.rs` — the same constants, collision resolution and
  `1/60` step) as a search over jump arcs from the spawn, and reports whether the
  goal and each coin can be reached. The cells it reached are tinted, the route it
  found is drawn, and a coin it could not reach is crossed out.
- **Incremental**: the sweep records what every arc it simulated *did* — where it
  landed, the coins and goal it touched, and the box of tiles its body could have
  read. An edit can only change the arcs whose box it touched (the grid is the only
  input an arc reads), so painting re-simulates those few arcs and re-derives
  reachability, the coins and the route from the records. A tile painted out of every
  trajectory's way costs nothing at all; sealing the level off from the goal (which
  *is* found, not missed) cost 122 arcs against the full sweep's 3502. The panel's
  **Recheck reachability** and `level check` still restart the sweep from scratch —
  the escape hatch if a re-validation ever looks wrong.

### Limits, honestly

- The sweep is a **conservative approximation**: it takes off from three points in
  a standing cell and over a small set of jump holds and running speeds, and one arc
  holds a single horizontal input from takeoff to landing. A route that needs
  frame-perfect timing — or a mid-air reversal — can be missed. A verdict of
  *playable*, and every *unreachable coin* marker, is trustworthy in that direction;
  *no route found* is a hint, not a proof, so it reports how much of the level it
  actually searched ("sweep reached 64/3750 cells"). The big hand-made maze of
  `level3.txt` reads as *no route found* this way, and it is a **false negative**:
  sampling harder does not rescue it. Measured, a maximally-sampled sweep — every
  pixel of the body's footprint as a takeoff, every jump hold 0–26 frames, jumps
  that start from rest, 27 x 191 = 5157 arcs per standing cell — exhausts 428,000
  arcs (minutes in the editor's engine) and still reaches only 82 of the level's
  3750 cells. Widening the default policy set instead is *worse*: doubling it to
  ~2x the cost truncates `level2` against the arc budget and still does not help
  `level3`. So the set stays small and the verdict stays honestly hedged.
- It is **not free, but it is paid once**: the editor's JavaScript engine runs the
  integrator far slower than the game does, so the *first* sweep of a 72x17 level is
  a few thousand arcs spread across frames (a couple per frame) — up to half a minute
  of background work. Edits after that are re-validated around the change instead
  (see *Incremental* above), so a session's cost is the first check plus a handful of
  arcs per edit. The verdict fills in live, the editor stays responsive, the sweep
  stops early once the goal and every coin have been reached, and it says so rather
  than guessing if it hits its arc budget. `noctis editor --headless … --run "level check"`
  runs a full sweep synchronously, for a script or a smoke test.
- **Use `--release`** for a session in the level editor, as for the game itself
  (`cargo run --release -- editor --project examples/reverie`). The sweep's
  JavaScript is the whole cost, and a release tool runs it an order of magnitude
  faster, which is what keeps a frame from hitching. `SEARCH.slice` in
  `extension.js` is the knob if a session still hitches: it is the number of
  trajectories simulated per frame.
- The game embeds `levels/*.txt` with `include_str!`, so a **save is picked up by
  the next build** of the game (written into `src/game/mod.rs`'s `LEVELS`).
  `noctis run` recompiles automatically when a level file changes.

`level` in the console: `level <slot>`, `level new [<width> <height>]`,
`level resize <width> <height>`, `level save`, `level reload`, `level check`,
`level music <name>`, `level background <name>`, `level brush <char>`,
`level paint <x> <y> <char>` (the pointer's own path, for a script), `level status`.
Its report ends with *over N arcs*, the simulation the last re-validation actually
spent. A headless run is a usable smoke test:

```sh
cargo run -- editor --headless --project examples/reverie --run "level check"
# ok level1 | 72x17 | 13 coins | 210 tiles | playable | playable — reached 99 of 1224 cells ... over 3502 arcs
```

## Browser demo

`just publish-wasm` builds this project into the browser client staged in
`docs/` for GitHub Pages, alongside the showcase and Rust Ball; pick
"Reverie Amidst Ruins" in the page's project menu. The plugin builds for
`wasm32-unknown-unknown` and plays sound there too, through the engine's
`AudioEngine` (rodio natively, the browser's WebAudio on the web, with the
`AudioContext` resumed on the first user gesture). The other web concessions:
palette randomization uses a built-in PRNG rather than `rand`/`getrandom`, and
screenshots are native-only (the game times itself from the frame delta, not a
wall clock, so `Instant`/`SystemTime` are never needed).

## Performance

The original was ~25 fps unoptimized and 144 fps in release; the same holds here,
because the game is a CPU software renderer run once per frame.

- **Optimize the build.** `noctis run --release` builds the project's plugin tool
  with cargo's release profile; a plain `noctis run` builds it in debug.
- Profiling the game loop (`update` + `render`) gave **~20 / 26 / 46 ms/frame**
  for levels 1 / 2 / 3 in **debug**, and **~1.3 / 1.5 / 2.1 ms/frame** in
  **release** — so the frame cost is the software renderer's per-pixel loops, and
  a debug build is the whole story.
- The tileset and background PNGs were being decoded *every frame* (as in the
  original); `src/game/render.rs` now decodes each once and caches it.

## How the port maps

| Original (Rust)                    | Here |
| ---------------------------------- | ---- |
| `level.rs`, `player.rs`, `camera.rs`, `color.rs`, `render.rs`, `stats.rs`, `screen.rs` | `src/game/*.rs` — **vendored nearly verbatim** (only module paths changed) |
| `audio.rs`                          | `src/game/audio.rs` — reused; the plugin links `rodio` directly |
| `main.rs` (the game loop)           | `src/game/mod.rs` — `Game::update` (logic) + `Game::render` (the frame) |
| `input/*` (`crossterm` + `gilrs`)   | `src/game/input.rs` — folds the engine's keyboard state into the game's `Input` |
| `terminal/*` (Kitty graphics)       | **deleted** — `ctx.frame` presents the pixels |
| `args.rs`                           | replaced by `noctis.toml` |

So the collision math, the 4-colour palette swap, the parallax backgrounds, the
sprite drop shadows, and the vignette/scanline post-process are all the
original's own code — the port's job was to get its frames on screen.

The real assets are bundled: `assets/tiles/tileset.png`, `assets/backgrounds/*`,
`assets/songs/*`, `assets/sfx/*`.

## Not carried over

- **Gamepad** — the engine's input model is keyboard/mouse only (winit), so the
  original's `gilrs` path is dropped. (Would need `gilrs` in `platform`.)
- **The HUD line** — the original printed `fps | coins | deaths` below the image;
  a window has no such line, and the frame is only the game image. A HUD would be
  a follow-up (drawn into the frame, or an overlay).
- **Image scaling (`-`/`=`)** — the window scales the frame itself.
- **`--level`, `--fps`, `--volume`, `--no-audio` CLI flags** — the game starts on
  level 1 at 100 % volume; these would become manifest settings.
