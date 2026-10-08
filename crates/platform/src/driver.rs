//! The trait a windowed application implements.

use ecs::assets::AssetRegistry;
use ecs::resources::InputState;
use ecs::world::{Entity, World};
use render::GizmoGeometry;

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

    /// Draw the driver's UI into the given egui UI. Called once per frame,
    /// before the scene is presented, when the window host has an egui layer.
    ///
    /// `Ui` derefs to `egui::Context`, so `ui.ctx()` is available if needed.
    fn ui(&mut self, _ui: &mut egui::Ui) {}

    /// An editor gizmo to draw over the scene this frame: its geometry and the
    /// world transform that places it. The host uploads it right before
    /// presenting, so a driver can drive it from state it computes in
    /// [`AppDriver::ui`]. Defaults to no gizmo.
    fn gizmo(&self) -> Option<(GizmoGeometry, math::Mat4)> {
        None
    }

    /// Whether the driver has requested the application exit.
    fn should_exit(&self) -> bool {
        false
    }

    /// A file was dropped onto the window. The default does nothing; a driver
    /// that acts on drops (for example, importing the file into a project)
    /// implements it.
    fn drop_file(&mut self, path: &std::path::Path) {
        let _ = path;
    }
}
