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
        self.apply_environment(renderer);
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

    fn input_mut(&mut self) -> Option<&mut InputState> {
        Some(&mut self.app_mut().context.input)
    }

    fn cursor_captured(&self) -> bool {
        self.captured
    }

    fn on_escape(&mut self) {
        // While playing, `Esc` releases the cursor (and, once the menu exists,
        // pauses); again to resume. The cursor follows the toggle.
        self.captured = !self.captured;
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
