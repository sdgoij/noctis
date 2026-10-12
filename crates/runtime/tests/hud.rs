//! The screen-space HUD the runtime draws over a presented frame.
//!
//! A driver publishes `EngineContext::screen_text` (and, when it draws its own
//! pixels, an `EngineContext::frame` for the host to present underneath). The
//! runtime's [`AppDriver::ui`] draws those lines. This checks the drawing runs and
//! paints the published text — the seam the Reverie plugin's HUD uses.

use ecs::scene::Scene;
use platform::{AppDriver, egui};
use runtime::{Runtime, RuntimeConfig};
use script::NoopHost;

/// Run one egui frame with the runtime's UI drawn, returning the number of paint
/// shapes it produced.
fn draw(runtime: &mut Runtime<NoopHost>) -> usize {
    let ctx = egui::Context::default();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| AppDriver::ui(runtime, ui));
    let shapes = output.shapes.len();
    // egui panics if a `TexturesDelta` is dropped unapplied; a host uploads it.
    output.textures_delta.clear();
    shapes
}

#[test]
fn the_runtime_paints_published_screen_text() {
    let config = RuntimeConfig {
        audio: false,
        ..RuntimeConfig::default()
    };
    let mut runtime = Runtime::with_host(Scene::new("HUD"), config, NoopHost);

    let bare = draw(&mut runtime);

    runtime.app_mut().context.screen_text = vec![
        "fps 60 | coins 3 | deaths 1".to_string(),
        "total playtime: 0:01:02.003".to_string(),
    ];
    let with_hud = draw(&mut runtime);

    assert!(
        with_hud > bare,
        "the HUD text was not painted ({with_hud} shapes with the HUD vs {bare} without)"
    );
}
