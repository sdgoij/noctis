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

## Browser demo

`just publish-wasm` builds this project into the browser client staged in
`docs/` for GitHub Pages, alongside the showcase and Rust Ball; pick
"Reverie Amidst Ruins" in the page's project menu. The plugin builds for
`wasm32-unknown-unknown`: `rodio` is a native-only dependency (the web runs
silent, see `src/game/audio.rs`), palette randomization uses a built-in PRNG
rather than `rand`/`getrandom`, and screenshots are native-only (the game reads
the frame delta, not a wall clock, so `Instant`/`SystemTime` are never needed).

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
