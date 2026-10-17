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
- **Reachability**: the search runs in the project's Rust — `src/validator.rs`, reached
  from the extension as `noctis.call("reverie.validate", …)`. It reuses the game's own
  `LevelDefinition::parse` and `Player::update`, so the editor validates the real
  rules rather than a transcription of them, and it searches a *state space*: a node
  is a place the player came to rest (in pixels, with the momentum it arrived with),
  and an arc is one input program — direction, jump hold, and an optional mid-air
  stop or reversal — simulated through the real integrator until it lands. The cells
  the body covered are tinted, the route it found is drawn, and a coin it could not
  reach is crossed out.
- **Without the plugin** (the extension copied into a project that has no Rust) it
  falls back to a JavaScript arc sweep of the same shape, which is the thing the
  native validator replaced — see *The native validator* below for what that costs
  and what it misses.
- **A second opinion**, when the sweep gives up: a *deliberately unsound*,
  tile-level estimate (`coarse_estimate`) floods the tiles the body could occupy, with
  gravity and one bounded hop — no arc, no momentum, no timing. It overstates what is
  possible, so it never overrules a *playable* verdict; it only softens a *no route
  found* into "the sweep's model is too narrow", which is what it reports on
  `level3.txt` (the sweep routes 64 of 3750 cells, the estimate 2073).
- **Incremental** (the fallback only): the sweep records what every arc it simulated
  *did* — where it landed, the coins and goal it touched, and the box of tiles its
  body could have read. An edit can only change the arcs whose box it touched (the
  grid is the only input an arc reads), so painting re-simulates those few arcs and
  re-derives reachability, the coins and the route from the records. A tile painted
  out of every trajectory's way costs nothing at all; sealing the level off from the
  goal (which *is* found, not missed) cost 122 arcs against the full sweep's 3502.
  The native validator has no incremental path — a whole search is a second or two,
  and a new edit simply cancels the previous one — so this paragraph matters only
  when the extension is used without the plugin.

### Limits, honestly

- **The JavaScript fallback** is a **conservative approximation**, and only that: it
  takes off from three points in a standing cell and over a small set of jump holds
  and running speeds, and one arc holds a single horizontal input from takeoff to
  landing. A route that needs frame-perfect timing — or a mid-air reversal — can be
  missed. A verdict of *playable*, and every *unreachable coin* marker, is trustworthy
  in that direction; *no route found* is a hint, not a proof, so it reports how much
  of the level it actually searched ("sweep reached 64/3750 cells") and a coarse
  tile-level estimate gives a second opinion. `level3.txt` is exactly the case that
  drove the native validator: its maze needs a mid-air reversal, so the sweep read it
  as *no route found* even after 428,000 arcs of maximally-sampled searching, while
  the native search finds a route in ~2.8 s. **With the plugin loaded, this bullet
  only describes the fallback.**
- **Cost**: a native search is a second or two of a worker thread, and the frame
  never waits for it — the verdict fills in when it lands. A level whose state space
  is much larger than these three is bounded by the node budget, and then says
  *partial* rather than guessing. (The fallback is slower: a few thousand arcs spread
  two per frame, i.e. up to half a minute for a 72x17 level.)
  `noctis editor --headless … --run "level check"` runs the blocking form
  synchronously, for a script or a smoke test.
- **Use `--release`** for a session in the level editor, as for the game itself
  (`cargo run --release -- editor --project examples/reverie`). The plugin tool is
  built in debug otherwise, and the native validator is ~20x slower there (the game
  loop too). `SEARCH.slice` in `extension.js` is the knob for the *fallback* — the
  number of trajectories simulated per frame.
- The game embeds `levels/*.txt` with `include_str!`, so a **save is picked up by
  the next build** of the game (written into `src/game/mod.rs`'s `LEVELS`).
  `noctis run` recompiles automatically when a level file changes.

### The native validator

The reachability search is the one thing the extension cannot do well in
JavaScript, and the measurements say why. On a release build the JavaScript sweep
runs at about **7–9k arcs/s** (a full `level1` sweep is 3502 arcs in ~0.5 s), so the
language was not the bottleneck. Two other things were: the live panel only
advances `SEARCH.slice` — two arcs per frame, about 120 arcs/s — so a few thousand
arcs take up to half a minute of wall clock; and the arc model itself (one input
held for a whole flight) cannot express a mid-air reversal.

So the search lives in `src/validator.rs` now. It is a macro-action search over the
game's own `LevelDefinition::parse` and `Player::update` — the editor validates the
real rules, not a copy. A **node** is a place the player came to rest, in pixels and
with the horizontal momentum it arrived with; an **arc** is one input program
(direction, jump hold, and an optional mid-air stop or reversal) simulated through
the real integrator until it lands. Keeping the landing position in pixels is what
the JavaScript model lost by rounding to a tile.

The extension reaches it through the engine's extension-function seam: the plugin
returns `reverie.validate`, `reverie.validate.start` and `reverie.poll` from
`Plugin::extension_functions` (`src/lib.rs`), and JavaScript calls them with
`noctis.call(name, args)` / lists them with `noctis.calls()`. The editor uses the
threaded pair — `start` hands the level to a worker and `poll` collects the report
(it is called from the `"update"` hook) — so a couple of seconds of search never
freezes the frame, and a new edit cancels the one before it. `level check` runs the
blocking form, because a CLI has no frames to poll on.

Measured in release: `level1` 4690 nodes / ~0.9 s, `level2` 10755 / ~1.8 s,
`level3` 17509 / ~2.8 s. `level3` is also **playable** now — the verdict the
JavaScript model could not reach even after 428,000 arcs — and a level sealed off by
a full-height wall still reads as *no route found*. The node budget (`NATIVE_BUDGET`
in the extension, `DEFAULT_BUDGET` in the validator) bounds a pathological level and
the verdict says *partial* if it runs out.

Without the plugin the extension keeps the JavaScript sweep as a fallback:
`noctis.calls()` decides once, and the panel's **Recheck reachability** still
restarts it.

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
