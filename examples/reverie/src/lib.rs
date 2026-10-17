//! Reverie Amidst Ruins — a Noctis project whose game *is* the original Rust.
//!
//! The project carries a Rust plugin (this crate; a `Cargo.toml` at the project
//! root is what makes `noctis` treat it as Rust). The plugin owns the whole game
//! — the vendored [`game`] module is the original's `level`/`player`/`camera`/
//! `render`/`audio`/… almost verbatim — runs one frame of it per tick, and
//! publishes the finished RGBA frame through `EngineContext::frame`, which the
//! window host blits across the window. The engine's scene is not used: the
//! original is its own software renderer.

#![forbid(unsafe_code)]

mod game;
#[cfg(not(target_arch = "wasm32"))]
mod validator;

use std::time::Duration;

use noctis::prelude::*;

use game::{Game, Input};

/// The level the game opens on (0-based).
const START_LEVEL: usize = 0;
/// The starting master volume, percent.
const VOLUME: u8 = 100;

/// The functions this project's editor extensions may call (`noctis.call`).
///
/// `reverie.validate` is the level editor's reachability search, run natively so
/// it can use the game's own parser and integrator — see [`validator`]. `start` and
/// `poll` are the same search on a worker thread, which is the one the editor uses:
/// a couple of seconds of work off the frame rather than in it. The editor does not
/// exist on the web, so the table is empty there (and the validator is not built).
#[cfg(not(target_arch = "wasm32"))]
const EXTENSION_FUNCTIONS: &[(&str, ExtensionFn)] = &[
    ("reverie.validate", validator::validate),
    ("reverie.validate.start", validator::start),
    ("reverie.poll", validator::poll),
];
#[cfg(target_arch = "wasm32")]
const EXTENSION_FUNCTIONS: &[(&str, ExtensionFn)] = &[];

/// The project's plugin.
#[noctis::plugin]
#[derive(Default)]
pub struct Reverie;

impl Plugin for Reverie {
    /// The level editor's native validator (see [`EXTENSION_FUNCTIONS`]).
    fn extension_functions(&self) -> &'static [(&'static str, ExtensionFn)] {
        EXTENSION_FUNCTIONS
    }

    /// Install the one system that runs the game: it initialises the game on the
    /// first tick, folds the engine's input into the game's, advances it, and
    /// publishes the frame it renders.
    fn build(&self, app: &mut App) {
        let mut game: Option<Game> = None;
        let mut started = false;

        app.on_update("reverie.loop", move |ctx| {
            if !started {
                started = true;
                match Game::new(START_LEVEL, true, VOLUME) {
                    Ok(ready) => game = Some(ready),
                    Err(error) => log::error!("reverie could not start: {error}"),
                }
            }

            let Some(game) = game.as_mut() else {
                return;
            };

            let input = Input::from_state(&ctx.input);
            if input.quit {
                ctx.exit_requested = true;
                return;
            }

            let dt = Duration::from_secs_f32(ctx.time.delta().max(0.0));
            if let Err(error) = game.update(&input, dt) {
                log::error!("reverie update failed: {error}");
            }

            match game.render() {
                Ok(frame) => {
                    ctx.frame = Some(FrameImage {
                        width: frame.width(),
                        height: frame.height(),
                        rgba: frame.into_raw(),
                    });
                }
                Err(error) => log::error!("reverie render failed: {error}"),
            }

            ctx.screen_text = game.hud_lines();
        });
    }
}
