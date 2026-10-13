//! The plugin screen-UI seam.
//!
//! A plugin installs a callback on `EngineContext::ui`; the runtime's
//! [`AppDriver::ui`] draws it into the host's egui layer, through
//! [`platform::EguiUi`]. This checks the seam end-to-end: the callback runs and
//! its widgets paint.

use ecs::scene::Scene;
use ecs::ui::Ui;
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

fn headless() -> Runtime<NoopHost> {
    let config = RuntimeConfig {
        audio: false,
        ..RuntimeConfig::default()
    };
    Runtime::with_host(Scene::new("UI"), config, NoopHost)
}

#[test]
fn the_runtime_draws_a_plugins_ui() {
    let mut runtime = headless();
    let bare = draw(&mut runtime);

    runtime.app_mut().context.ui = Some(Box::new(|ui: &mut dyn Ui| {
        ui.panel("main", "Goats", &mut |ui: &mut dyn Ui| {
            ui.label("Reverie Amidst Ruins");
            if ui.button("Play") {
                // A click would start the game; the test only needs it drawn.
            }
            ui.separator();
            let _ = ui.button("Quit");
            let mut volume = 0.5_f32;
            let _ = ui.slider("Volume", &mut volume, 0.0, 1.0);
            let mut muted = false;
            let _ = ui.toggle("Mute", &mut muted);
            let mut take = 0_usize;
            let _ = ui.combo("Weather", &mut take, &["clear", "rain"]);
            let mut line = String::new();
            let _ = ui.text_field("console", &mut line);
        });
    }));

    let with_ui = draw(&mut runtime);
    assert!(
        with_ui > bare,
        "the plugin's UI was not painted ({with_ui} shapes with the UI vs {bare} without)"
    );
}

#[test]
fn a_plugin_without_a_ui_paints_nothing_extra() {
    let mut runtime = headless();
    let bare = draw(&mut runtime);
    assert_eq!(bare, draw(&mut runtime), "no callback, no plugin shapes");
}
