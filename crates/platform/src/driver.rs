//! The trait a windowed application implements.

use ecs::assets::AssetRegistry;
use ecs::resources::InputState;
use ecs::world::{Entity, World};

/// Something the window host can drive: a simulation plus the data needed to
/// render it.
///
/// The host calls [`AppDriver::update`] once per frame, then renders
/// [`AppDriver::world`] with [`AppDriver::assets`] from [`AppDriver::camera`].
pub trait AppDriver {
    /// Called once after the window and renderer exist, before the first frame.
    /// Use it to set up anything that needs to be ready before rendering (for
    /// example, ensuring a camera exists).
    fn prepare(&mut self) {}

    /// Advance the simulation by `dt` seconds.
    fn update(&mut self, dt: f32);

    /// The world to render.
    fn world(&self) -> &World;

    /// The assets to render with.
    fn assets(&self) -> &AssetRegistry;

    /// The camera entity to render from, if the driver has one.
    fn camera(&self) -> Option<Entity>;

    /// Input state to feed key events into, if the driver accepts them.
    ///
    /// Note that the engine's frame loop clears per-frame edge state
    /// (`key_pressed`/`key_released`) at the start of each frame; a driver that
    /// wants edge semantics must sample them itself. `key_down` survives.
    fn input_mut(&mut self) -> Option<&mut InputState> {
        None
    }

    /// Whether the driver has requested the application exit.
    fn should_exit(&self) -> bool {
        false
    }
}
