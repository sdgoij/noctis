//! Windowed play mode: implement [`platform::AppDriver`] for [`Runtime`].
//!
//! Enabling this is how `noctis run` and `noctis editor` open a real viewport:
//! the window host ticks the runtime each frame and
//! presents the scene through a surface-backed renderer.

use ecs::assets::AssetRegistry;
use ecs::components::{Camera, LocalTransform};
use ecs::resources::InputState;
use ecs::world::{Entity, World};
use math::vec3;
use platform::{AppDriver, PlatformError, WindowConfig};
use script::ScriptHost;

use crate::Runtime;

impl<H: ScriptHost + 'static> AppDriver for Runtime<H> {
    fn prepare(&mut self) {
        let world = &mut self.app_mut().context.scene.world;
        if render::first_active_camera(world).is_none() {
            let camera = world.spawn();
            world.insert(
                camera,
                LocalTransform::from_translation(vec3(0.0, 1.5, 6.0)),
            );
            world.insert(camera, Camera::default());
            log::info!("scene had no camera; spawned one at (0, 1.5, 6)");
        }
    }

    /// Apply the project's `[render] environment` to the window host's renderer,
    /// so a windowed run gets the same image-based lighting as a headless one.
    fn configure_renderer(&self, renderer: &mut render::Renderer) {
        self.apply_render_settings(renderer);
    }

    fn update(&mut self, dt: f32) {
        self.tick(dt);
    }

    fn world(&self) -> &World {
        &self.app().context.scene.world
    }

    fn assets(&self) -> &AssetRegistry {
        &self.app().context.assets
    }

    fn camera(&self) -> Option<Entity> {
        render::first_active_camera(&self.app().context.scene.world)
    }

    /// Draw the screen-space HUD lines a driver published, over the presented
    /// frame: each line in white with a soft dark shadow, down the top-left. Then
    /// the plugin's own UI, if it installed one — its menus, settings and console,
    /// drawn through the [`ecs::ui::Ui`] seam.
    fn ui(&mut self, ui: &mut platform::egui::Ui) {
        use platform::egui::{Align2, Color32, FontId, pos2};

        let lines = self.app().context.screen_text.clone();
        if !lines.is_empty() {
            let painter = ui.painter();
            let mut y = 8.0_f32;
            for line in &lines {
                let at = pos2(8.0, y);
                let shadow = pos2(at.x + 1.0, at.y + 1.0);
                let font = FontId::monospace(14.0);
                painter.text(
                    shadow,
                    Align2::LEFT_TOP,
                    line.as_str(),
                    font.clone(),
                    Color32::from_black_alpha(180),
                );
                painter.text(at, Align2::LEFT_TOP, line.as_str(), font, Color32::WHITE);
                y += 16.0;
            }
        }

        // The plugin's UI shares the egui layer with the HUD.
        let context = &mut self.app_mut().context;
        if let Some(callback) = context.ui.as_mut() {
            let mut seam = platform::EguiUi::new(ui);
            callback(&mut seam);
        }
    }

    /// A full-window frame the app published this tick (a plugin's software
    /// renderer), presented in place of the scene.
    fn frame(&self) -> Option<&ecs::resources::FrameImage> {
        self.app().context.frame.as_ref()
    }

    fn input_mut(&mut self) -> Option<&mut InputState> {
        Some(&mut self.app_mut().context.input)
    }

    fn cursor_captured(&self) -> bool {
        self.captured
    }

    fn on_escape(&mut self) {
        // `Esc` pauses to the menu (and releases the cursor), or resumes.
        self.toggle_pause();
    }

    fn should_exit(&self) -> bool {
        self.app().context.exit_requested
    }
}

/// Run `runtime` in a window until it is closed. Blocks.
pub fn run_in_window<H: ScriptHost + 'static>(
    runtime: Runtime<H>,
    config: WindowConfig,
) -> Result<(), PlatformError> {
    platform::run(runtime, config)
}
