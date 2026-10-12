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
cargo run -- run --project examples/reverie
```

Controls (as the original): `A`/`D` or `←`/`→` move, `Space`/`W`/`↑` jump,
`Enter` confirm, `R` restart, `1`/`3`/`4` effects/palette/debug, `2` screenshot,
`m` mute, `8`/`9` volume, `Esc`/`Q` quit.

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
