//! The winit window host, with an egui overlay.

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

/// Map a winit mouse button to an engine [`ecs::resources::MouseButton`], or
/// `None` for the extra buttons the engine does not model.
fn map_mouse_button(button: winit::event::MouseButton) -> Option<ecs::resources::MouseButton> {
    use ecs::resources::MouseButton as M;
    use winit::event::MouseButton as W;
    Some(match button {
        W::Left => M::Left,
        W::Right => M::Right,
        W::Middle => M::Middle,
        _ => return None,
    })
}

/// Convert a winit scroll delta to engine scroll points.
///
/// A line-wheel notch maps to [`LINE_SCROLL_POINTS`] points so that wheel and
/// trackpad scrolls share one scale; a pixel/trackpad delta is already in points.
fn scroll_points(delta: winit::event::MouseScrollDelta) -> f32 {
    match delta {
        winit::event::MouseScrollDelta::LineDelta(_, y) => y * LINE_SCROLL_POINTS,
        winit::event::MouseScrollDelta::PixelDelta(position) => position.y as f32,
    }
}

/// Scroll points per line-wheel notch, matching egui's native line size.
const LINE_SCROLL_POINTS: f32 = 40.0;

/// The egui integration state (context, winit bridge, wgpu renderer).
struct EguiLayer {
    ctx: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
}

/// A window running an [`AppDriver`] with an egui overlay.
pub struct WindowHost<D: AppDriver> {
    config: WindowConfig,
    driver: D,
    renderer: Option<Renderer>,
    egui: Option<EguiLayer>,
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
            egui: None,
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
            Ok(renderer) => {
                let format = renderer
                    .present_format()
                    .unwrap_or(wgpu::TextureFormat::Bgra8Unorm);
                let egui_ctx = egui::Context::default();
                let egui_state = egui_winit::State::new(
                    egui_ctx.clone(),
                    egui::ViewportId::ROOT,
                    window.as_ref(),
                    Some(window.scale_factor() as f32),
                    None,
                    Some(renderer.device().limits().max_texture_dimension_2d as usize),
                );
                let egui_renderer = egui_wgpu::Renderer::new(
                    renderer.device(),
                    format,
                    egui_wgpu::RendererOptions::default(),
                );
                self.egui = Some(EguiLayer {
                    ctx: egui_ctx,
                    state: egui_state,
                    renderer: egui_renderer,
                });
                self.renderer = Some(renderer);
            }
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

    fn forward_to_egui(&mut self, event: &WindowEvent) {
        if let (Some(egui), Some(window)) = (self.egui.as_mut(), self.window.clone()) {
            let _ = egui.state.on_window_event(&window, event);
        }
    }

    /// Upload the driver's editor gizmo (if any) to the renderer. Called after
    /// the driver's UI has run, so interaction state (a hovered handle) is
    /// current.
    fn apply_gizmo(&self, renderer: &mut Renderer) {
        match self.driver.gizmo() {
            Some((geometry, transform)) => renderer.set_gizmo(&geometry, &transform),
            None => renderer.clear_gizmo(),
        }
    }

    fn redraw(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().clamp(0.0, 0.1);
        self.last = now;

        self.driver.update(dt);

        let Some(window) = self.window.clone() else {
            return;
        };
        // Take the renderer and egui layer out so `self.driver` can be borrowed
        // freely while we build the egui frame and present.
        let Some(mut renderer) = self.renderer.take() else {
            window.request_redraw();
            return;
        };
        let camera = self.driver.camera();
        let egui = self.egui.take();

        if let Some(mut egui) = egui {
            let raw_input = egui.state.take_egui_input(&window);
            let mut output = egui.ctx.run_ui(raw_input, |ui| self.driver.ui(ui));
            egui.state
                .handle_platform_output(&window, output.platform_output);
            let paint_jobs = egui.ctx.tessellate(output.shapes, output.pixels_per_point);

            let size = window.inner_size();
            let screen_descriptor = egui_wgpu::ScreenDescriptor {
                size_in_pixels: [size.width.max(1), size.height.max(1)],
                pixels_per_point: output.pixels_per_point,
            };

            for (id, deltas) in &output.textures_delta.set {
                for delta in deltas {
                    egui.renderer
                        .update_texture(renderer.device(), renderer.queue(), *id, delta);
                }
            }
            {
                let device = renderer.device();
                let queue = renderer.queue();
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("egui.upload"),
                });
                let extra = egui.renderer.update_buffers(
                    device,
                    queue,
                    &mut encoder,
                    &paint_jobs,
                    &screen_descriptor,
                );
                queue.submit(std::iter::once(encoder.finish()).chain(extra));
            }

            if let Some(camera) = camera {
                self.apply_gizmo(&mut renderer);
                let result = renderer.present_with_overlay(
                    self.driver.world(),
                    self.driver.assets(),
                    camera,
                    &mut |encoder, view| {
                        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("egui.pass"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Load,
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: None,
                            timestamp_writes: None,
                            occlusion_query_set: None,
                            multiview_mask: None,
                        });
                        let mut pass = pass.forget_lifetime();
                        egui.renderer
                            .render(&mut pass, &paint_jobs, &screen_descriptor);
                    },
                );
                if let Err(err) = result {
                    log::warn!("present failed: {err}");
                }
            }

            for id in &output.textures_delta.free {
                egui.renderer.free_texture(id);
            }
            // Tell epaint the deltas have been applied; otherwise its `Drop`
            // check panics about unapplied deltas.
            output.textures_delta.clear();
            self.egui = Some(egui);
        } else if let Some(camera) = camera {
            self.apply_gizmo(&mut renderer);
            if let Err(err) = renderer.present(self.driver.world(), self.driver.assets(), camera) {
                log::warn!("present failed: {err}");
            }
        }

        self.renderer = Some(renderer);
        window.request_redraw();
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
        match &event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut()
                    && let Err(err) = renderer.resize(size.width, size.height)
                {
                    log::warn!("resize failed: {err}");
                }
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
                if self.driver.should_exit() {
                    event_loop.exit();
                }
            }
            WindowEvent::KeyboardInput {
                event: key_event, ..
            } => {
                if let PhysicalKey::Code(code) = key_event.physical_key
                    && let Some(key) = map_key(code)
                    && let Some(input) = self.driver.input_mut()
                {
                    match key_event.state {
                        ElementState::Pressed => input.press_key(key),
                        ElementState::Released => input.release_key(key),
                    }
                }
                self.forward_to_egui(&event);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(button) = map_mouse_button(*button)
                    && let Some(input) = self.driver.input_mut()
                {
                    match state {
                        ElementState::Pressed => input.press_mouse(button),
                        ElementState::Released => input.release_mouse(button),
                    }
                }
                self.forward_to_egui(&event);
            }
            WindowEvent::CursorMoved { position, .. } => {
                // `InputState` stores the cursor in logical pixels; winit reports
                // physical pixels.
                let scale = self.window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
                if let Some(input) = self.driver.input_mut() {
                    input.set_mouse_position(position.x as f32 / scale, position.y as f32 / scale);
                }
                self.forward_to_egui(&event);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if let Some(input) = self.driver.input_mut() {
                    input.add_scroll(scroll_points(*delta));
                }
                self.forward_to_egui(&event);
            }
            WindowEvent::DroppedFile(path) => {
                self.driver.drop_file(path);
            }
            other => self.forward_to_egui(other),
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

    #[test]
    fn maps_the_modelled_mouse_buttons() {
        use ecs::resources::MouseButton as M;
        use winit::event::MouseButton as W;
        assert_eq!(map_mouse_button(W::Left), Some(M::Left));
        assert_eq!(map_mouse_button(W::Right), Some(M::Right));
        assert_eq!(map_mouse_button(W::Middle), Some(M::Middle));
        assert_eq!(map_mouse_button(W::Back), None);
        assert_eq!(map_mouse_button(W::Forward), None);
    }

    #[test]
    fn scroll_units_are_normalised_to_points() {
        use winit::event::MouseScrollDelta::{LineDelta, PixelDelta};
        // One notch up (positive) is one line of points, in the zoom-in direction.
        assert_eq!(scroll_points(LineDelta(0.0, 1.0)), LINE_SCROLL_POINTS);
        assert_eq!(scroll_points(LineDelta(0.0, -1.0)), -LINE_SCROLL_POINTS);
        // A trackpad/pixel delta is already in points and passes through.
        assert_eq!(scroll_points(PixelDelta((0.0, 12.0).into())), 12.0);
    }
}
