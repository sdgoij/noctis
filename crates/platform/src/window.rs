//! The winit window host.

use std::sync::Arc;
use std::time::Instant;

use render::{RenderError, Renderer, RendererConfig};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::driver::AppDriver;
use crate::error::PlatformError;

/// How the window is created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowConfig {
    /// The window title.
    pub title: String,
    /// The initial width in logical pixels.
    pub width: u32,
    /// The initial height in logical pixels.
    pub height: u32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "Noctis".to_string(),
            width: 1280,
            height: 720,
        }
    }
}

/// Map a winit [`KeyCode`] to an engine [`ecs::resources::Key`].
///
/// `KeyCode`'s `Debug` representation (`"KeyW"`, `"Digit3"`, `"ArrowUp"`, ...)
/// is exactly the naming the engine uses, so this is a single lookup. Keys the
/// engine does not model (function keys, etc.) map to `None`.
pub fn map_key(code: KeyCode) -> Option<ecs::resources::Key> {
    ecs::resources::Key::from_name(&format!("{code:?}"))
}

/// A window running an [`AppDriver`].
pub struct WindowHost<D: AppDriver> {
    config: WindowConfig,
    driver: D,
    renderer: Option<Renderer>,
    window: Option<Arc<Window>>,
    last: Instant,
}

impl<D: AppDriver> WindowHost<D> {
    /// Build a host that will run `driver` in a window.
    pub fn new(driver: D, config: WindowConfig) -> Self {
        Self {
            config,
            driver,
            renderer: None,
            window: None,
            last: Instant::now(),
        }
    }

    fn create_window(&mut self, event_loop: &ActiveEventLoop) {
        let attributes = Window::default_attributes()
            .with_title(self.config.title.clone())
            .with_inner_size(LogicalSize::new(self.config.width, self.config.height));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                log::error!("failed to create window: {err}");
                event_loop.exit();
                return;
            }
        };
        let size = window.inner_size();

        let instance = Renderer::create_instance();
        let surface = match instance.create_surface(window.clone()) {
            Ok(surface) => surface,
            Err(err) => {
                log::error!("failed to create surface: {err}");
                event_loop.exit();
                return;
            }
        };
        let renderer_config = RendererConfig {
            width: size.width.max(1),
            height: size.height.max(1),
            hdr: true,
            sample_count: 1,
        };
        match Renderer::for_surface(renderer_config, instance, surface) {
            Ok(renderer) => self.renderer = Some(renderer),
            Err(RenderError::NoAdapter) => {
                log::warn!("no graphics adapter available; the window will not render");
            }
            Err(err) => {
                log::error!("renderer init failed: {err}");
            }
        }

        self.driver.prepare();
        self.last = Instant::now();
        self.window = Some(window);
    }

    fn redraw(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().clamp(0.0, 0.1);
        self.last = now;

        self.driver.update(dt);

        let camera = self.driver.camera();
        if let (Some(renderer), Some(camera)) = (self.renderer.as_mut(), camera) {
            if let Err(err) = renderer.present(self.driver.world(), self.driver.assets(), camera) {
                log::warn!("present failed: {err}");
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

impl<D: AppDriver> ApplicationHandler for WindowHost<D> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            self.create_window(event_loop);
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    if let Err(err) = renderer.resize(size.width, size.height) {
                        log::warn!("resize failed: {err}");
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if let Some(key) = map_key(code) {
                        if let Some(input) = self.driver.input_mut() {
                            match event.state {
                                ElementState::Pressed => input.press_key(key),
                                ElementState::Released => input.release_key(key),
                            }
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
                if self.driver.should_exit() {
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }
}

/// Run `driver` in a window until it is closed. Blocks.
pub fn run<D: AppDriver>(driver: D, config: WindowConfig) -> Result<(), PlatformError> {
    let event_loop = EventLoop::new().map_err(|err| PlatformError::EventLoop(err.to_string()))?;
    let mut host = WindowHost::new(driver, config);
    event_loop
        .run_app(&mut host)
        .map_err(|err| PlatformError::EventLoop(err.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ecs::resources::Key;

    #[test]
    fn window_config_defaults() {
        let config = WindowConfig::default();
        assert_eq!(config.width, 1280);
        assert_eq!(config.height, 720);
        assert!(!config.title.is_empty());
    }

    #[test]
    fn maps_letters_digits_and_named_keys() {
        assert_eq!(map_key(KeyCode::KeyW), Key::letter('W'));
        assert_eq!(map_key(KeyCode::KeyA), Key::letter('A'));
        assert_eq!(map_key(KeyCode::Digit3), Key::digit('3'));
        assert_eq!(map_key(KeyCode::Space), Some(Key::Space));
        assert_eq!(map_key(KeyCode::ArrowUp), Some(Key::ArrowUp));
        assert_eq!(map_key(KeyCode::ShiftLeft), Some(Key::ShiftLeft));
        assert_eq!(map_key(KeyCode::ControlLeft), Some(Key::ControlLeft));
    }

    #[test]
    fn unmapped_keys_are_none() {
        assert_eq!(map_key(KeyCode::F12), None);
        assert_eq!(map_key(KeyCode::Home), None);
    }
}
