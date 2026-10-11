//! The web host (`wasm32`): a `<canvas>`, a wgpu surface, and a
//! `requestAnimationFrame` loop driving any [`AppDriver`].
//!
//! The browser **client** is the first target (see `docs/web.md`), so this host
//! renders the scene and feeds input but draws no egui overlay — the editor's UI
//! is a later step. GPU startup is asynchronous, so it is awaited on the browser's
//! event loop ([`Renderer::for_surface_async`]) rather than blocked on.

use std::cell::RefCell;
use std::rc::Rc;

use render::{Renderer, RendererConfig};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::config::WindowConfig;
use crate::driver::AppDriver;
use crate::error::PlatformError;

/// A DOM event captured between frames, applied to the driver's input at the next
/// frame.
enum RawEvent {
    Key { name: String, down: bool },
    Button { button: u16, down: bool },
    Cursor { x: f32, y: f32 },
    Wheel { points: f32 },
}

/// The per-frame loop state.
struct Loop<D: AppDriver> {
    driver: D,
    renderer: Renderer,
    canvas: web_sys::HtmlCanvasElement,
    events: Rc<RefCell<Vec<RawEvent>>>,
    last: f64,
    size: (u32, u32),
}

/// Run `driver` in the browser, driven by `requestAnimationFrame`.
///
/// Returns immediately: the loop runs until the page unloads. GPU startup is
/// asynchronous, so a failure is logged (there is no caller left to return to).
pub fn run<D: AppDriver + 'static>(driver: D, config: WindowConfig) -> Result<(), PlatformError> {
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(error) = start(driver, config).await {
            log::error!("web host failed: {error}");
        }
    });
    Ok(())
}

/// Acquire the canvas, open the surface renderer, and start the loop.
async fn start<D: AppDriver + 'static>(
    driver: D,
    config: WindowConfig,
) -> Result<(), PlatformError> {
    let window = web_sys::window().ok_or_else(|| host_err("no window"))?;
    let canvas = canvas(&window, &config)?;

    let instance = Renderer::create_instance();
    let surface = instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
        .map_err(|error| host_err(format!("failed to create the surface: {error}")))?;
    let size = (canvas.width().max(1), canvas.height().max(1));
    let renderer_config = RendererConfig {
        width: size.0,
        height: size.1,
        hdr: true,
        sample_count: config.sample_count,
    };
    let mut renderer = Renderer::for_surface_async(renderer_config, instance, surface)
        .await
        .map_err(|error| host_err(format!("renderer init failed: {error}")))?;

    let mut driver = driver;
    driver.configure_renderer(&mut renderer);
    driver.prepare();

    let events = Rc::new(RefCell::new(Vec::new()));
    install_listeners(&window, &canvas, &events);

    let state = Rc::new(RefCell::new(Loop {
        driver,
        renderer,
        canvas,
        events,
        last: now(&window),
        size,
    }));
    request_frame(&window, state);
    Ok(())
}

/// The canvas to render into: an existing `#noctis-canvas`, else a new one.
fn canvas(
    window: &web_sys::Window,
    config: &WindowConfig,
) -> Result<web_sys::HtmlCanvasElement, PlatformError> {
    let id = &config.canvas_id;
    let document = window.document().ok_or_else(|| host_err("no document"))?;
    let canvas = match document.get_element_by_id(id) {
        Some(element) => element
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .map_err(|_| host_err(format!("#{id} is not a <canvas> element")))?,
        None => {
            let canvas = document
                .create_element("canvas")
                .map_err(|_| host_err("failed to create a <canvas>"))?
                .dyn_into::<web_sys::HtmlCanvasElement>()
                .map_err(|_| host_err("created element is not a <canvas>"))?;
            canvas.set_id(id);
            let body = document.body().ok_or_else(|| host_err("no <body>"))?;
            body.append_child(&canvas)
                .map_err(|_| host_err("failed to attach the canvas"))?;
            canvas
        }
    };
    canvas.set_width(config.width.max(1));
    canvas.set_height(config.height.max(1));
    Ok(canvas)
}

/// Attach the DOM listeners that feed [`RawEvent`]s into the frame loop.
///
/// The closures live for the page's lifetime (they are `forget`ten): there are a
/// fixed handful, and they must outlive every frame.
fn install_listeners(
    window: &web_sys::Window,
    canvas: &web_sys::HtmlCanvasElement,
    events: &Rc<RefCell<Vec<RawEvent>>>,
) {
    for down in [true, false] {
        let events = Rc::clone(events);
        let closure = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
            move |event: web_sys::KeyboardEvent| {
                events.borrow_mut().push(RawEvent::Key {
                    name: event.code(),
                    down,
                });
            },
        );
        let kind = if down { "keydown" } else { "keyup" };
        let _ = window.add_event_listener_with_callback(kind, closure.as_ref().unchecked_ref());
        closure.forget();
    }

    {
        let events = Rc::clone(events);
        let closure =
            Closure::<dyn FnMut(web_sys::MouseEvent)>::new(move |event: web_sys::MouseEvent| {
                events.borrow_mut().push(RawEvent::Cursor {
                    x: event.offset_x() as f32,
                    y: event.offset_y() as f32,
                });
            });
        let _ =
            canvas.add_event_listener_with_callback("mousemove", closure.as_ref().unchecked_ref());
        closure.forget();
    }

    for down in [true, false] {
        let events = Rc::clone(events);
        let closure =
            Closure::<dyn FnMut(web_sys::MouseEvent)>::new(move |event: web_sys::MouseEvent| {
                let mut pending = events.borrow_mut();
                pending.push(RawEvent::Button {
                    button: event.button() as u16,
                    down,
                });
                pending.push(RawEvent::Cursor {
                    x: event.offset_x() as f32,
                    y: event.offset_y() as f32,
                });
            });
        let kind = if down { "mousedown" } else { "mouseup" };
        let _ = canvas.add_event_listener_with_callback(kind, closure.as_ref().unchecked_ref());
        closure.forget();
    }

    {
        let events = Rc::clone(events);
        let closure =
            Closure::<dyn FnMut(web_sys::WheelEvent)>::new(move |event: web_sys::WheelEvent| {
                // A browser `deltaY` grows downward; engine scroll grows upward.
                events.borrow_mut().push(RawEvent::Wheel {
                    points: -(event.delta_y() as f32),
                });
            });
        let _ = canvas.add_event_listener_with_callback("wheel", closure.as_ref().unchecked_ref());
        closure.forget();
    }
}

/// Schedule the next frame with `requestAnimationFrame`.
fn request_frame<D: AppDriver + 'static>(window: &web_sys::Window, state: Rc<RefCell<Loop<D>>>) {
    // A `once` closure is freed by the browser after it runs, so the loop does
    // not accumulate closures.
    let callback = Closure::once_into_js(move || tick(&state));
    let _ = window.request_animation_frame(callback.unchecked_ref());
}

/// One frame: apply input, resize if needed, advance the driver, present.
fn tick<D: AppDriver + 'static>(state: &Rc<RefCell<Loop<D>>>) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let handle = Rc::clone(state);

    {
        let mut state = state.borrow_mut();

        let now = now(&window);
        let dt = ((now - state.last) / 1000.0).clamp(0.0, 0.1) as f32;
        state.last = now;

        apply_events(&mut state);

        // The canvas's CSS size can change when the page resizes.
        let size = (
            state.canvas.client_width().max(1) as u32,
            state.canvas.client_height().max(1) as u32,
        );
        if size != state.size {
            state.canvas.set_width(size.0);
            state.canvas.set_height(size.1);
            if let Err(error) = state.renderer.resize(size.0, size.1) {
                log::warn!("resize failed: {error}");
            }
            state.size = size;
        }

        state.driver.update(dt);

        // Split the borrow so `renderer` and `driver` are disjoint.
        let Loop {
            driver, renderer, ..
        } = &mut *state;
        if let Some(camera) = driver.camera()
            && let Err(error) = renderer.present(driver.world(), driver.assets(), camera)
        {
            log::warn!("present failed: {error}");
        }
    }

    request_frame(&window, handle);
}

/// Apply the pending DOM events to the driver's input, then clear the queue.
fn apply_events<D: AppDriver>(state: &mut Loop<D>) {
    let pending: Vec<RawEvent> = std::mem::take(&mut *state.events.borrow_mut());
    let Some(input) = state.driver.input_mut() else {
        return;
    };
    for event in pending {
        match event {
            RawEvent::Key { name, down } => {
                if let Some(key) = ecs::resources::Key::from_name(&name) {
                    if down {
                        input.press_key(key);
                    } else {
                        input.release_key(key);
                    }
                }
            }
            RawEvent::Button { button, down } => {
                let button = match button {
                    0 => ecs::resources::MouseButton::Left,
                    1 => ecs::resources::MouseButton::Middle,
                    2 => ecs::resources::MouseButton::Right,
                    _ => continue,
                };
                if down {
                    input.press_mouse(button);
                } else {
                    input.release_mouse(button);
                }
            }
            RawEvent::Cursor { x, y } => input.set_mouse_position(x, y),
            RawEvent::Wheel { points } => input.add_scroll(points),
        }
    }
}

/// The current time from `performance.now()`, in milliseconds.
fn now(window: &web_sys::Window) -> f64 {
    window
        .performance()
        .map(|performance| performance.now())
        .unwrap_or(0.0)
}

fn host_err(message: impl Into<String>) -> PlatformError {
    PlatformError::EventLoop(message.into())
}
