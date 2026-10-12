//! The web host (`wasm32`): a `<canvas>`, a wgpu surface, and a
//! `requestAnimationFrame` loop driving any [`AppDriver`].
//!
//! The browser **client** is the first target (see `.notes/web.md`); the host
//! renders the scene, feeds input, and paints an **egui overlay** over it
//! (`web_egui.rs`), so the editor's UI works in a browser too. GPU startup is
//! asynchronous, so it is awaited on the browser's event loop
//! ([`Renderer::for_surface_async`]) rather than blocked on.
//!
//! Input follows the same [`AppDriver`] contract as the native host: keys, mouse
//! buttons and motion reach [`AppDriver::input_mut`], `Esc` reaches
//! [`AppDriver::on_escape`], and [`AppDriver::cursor_captured`] drives the
//! browser's **pointer lock** (with `movementX`/`Y` supplying the delta while
//! locked), so mouse-look works the same as it does in a window. A dropped file
//! reaches [`AppDriver::drop_bytes`] (a browser `File` has bytes and a name, not
//! a path): the DOM handler reads each file's bytes asynchronously and queues the
//! result for the next frame.

use std::cell::RefCell;
use std::rc::Rc;

use render::{Renderer, RendererConfig};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::config::WindowConfig;
use crate::driver::AppDriver;
use crate::error::PlatformError;
use crate::web_egui::WebEgui;

/// A DOM event captured between frames, applied to the driver's input at the next
/// frame.
enum RawEvent {
    Key {
        name: String,
        down: bool,
    },
    /// `Esc` was pressed: a driver action, not input state.
    Escape,
    Button {
        button: u16,
        down: bool,
    },
    Cursor {
        x: f32,
        y: f32,
    },
    /// Raw pointer motion (`movementX`/`Y`) while the cursor is locked.
    Motion {
        dx: f32,
        dy: f32,
    },
    Wheel {
        points: f32,
    },
    /// A file dropped onto the window: its name and bytes, read asynchronously by
    /// the DOM handler and applied at the next frame.
    Drop {
        name: String,
        bytes: Vec<u8>,
    },
}

/// The per-frame loop state.
struct Loop<D: AppDriver> {
    driver: D,
    renderer: Renderer,
    canvas: web_sys::HtmlCanvasElement,
    /// The egui overlay: its context, its wgpu renderer, and the queued DOM events.
    egui: WebEgui,
    events: Rc<RefCell<Vec<RawEvent>>>,
    last: f64,
    size: (u32, u32),
    /// The pointer-lock state last applied, so a driver's wish is only pushed to
    /// the browser when it changes.
    captured: bool,
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

    // The egui overlay is created against the surface's present format, so its
    // pipeline matches the swapchain colour space.
    let format = renderer
        .present_format()
        .unwrap_or(wgpu::TextureFormat::Bgra8Unorm);
    let egui = WebEgui::new(renderer.device(), format, &window, &canvas);

    let events = Rc::new(RefCell::new(Vec::new()));
    install_listeners(&window, &canvas, &events);

    let state = Rc::new(RefCell::new(Loop {
        driver,
        renderer,
        canvas,
        egui,
        events,
        last: now(&window),
        size,
        captured: false,
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
    // Focusable, so clicking it captures keyboard input (see `install_listeners`).
    canvas.set_tab_index(0);
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
                let name = event.code();
                // A key the engine knows must not also scroll/zoom the page.
                if ecs::resources::Key::from_name(&name).is_some() {
                    event.prevent_default();
                }
                let mut pending = events.borrow_mut();
                // `Esc` pauses the game: a driver action, not input state.
                if down && name == "Escape" {
                    pending.push(RawEvent::Escape);
                }
                pending.push(RawEvent::Key { name, down });
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
                // While the cursor is locked, the browser reports the absolute
                // position as frozen and offers raw motion instead; feed that as
                // the delta (the captured-cursor path).
                let locked = web_sys::window()
                    .and_then(|window| window.document())
                    .and_then(|document| document.pointer_lock_element())
                    .is_some();
                let mut pending = events.borrow_mut();
                if locked {
                    pending.push(RawEvent::Motion {
                        dx: event.movement_x() as f32,
                        dy: event.movement_y() as f32,
                    });
                } else {
                    pending.push(RawEvent::Cursor {
                        x: event.offset_x() as f32,
                        y: event.offset_y() as f32,
                    });
                }
            });
        let _ =
            canvas.add_event_listener_with_callback("mousemove", closure.as_ref().unchecked_ref());
        closure.forget();
    }

    for down in [true, false] {
        let events = Rc::clone(events);
        let focusable = canvas.clone();
        let closure =
            Closure::<dyn FnMut(web_sys::MouseEvent)>::new(move |event: web_sys::MouseEvent| {
                if down {
                    // Keyboard events follow focus, so clicking the canvas focuses
                    // it and the game receives keys even without a window listener.
                    let _ = focusable.focus();
                }
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

    // A drop has two halves: `dragover` must cancel the default (the browser only
    // fires `drop` for a target that did); `drop` reads each `File`'s bytes —
    // asynchronously, since a `File` is a lazy blob — and queues a [`RawEvent::Drop`]
    // for the next frame. The listeners go on the whole window, so a drop anywhere
    // on the page counts, not just exactly on the canvas.
    {
        let closure =
            Closure::<dyn FnMut(web_sys::DragEvent)>::new(move |event: web_sys::DragEvent| {
                event.prevent_default()
            });
        let _ =
            window.add_event_listener_with_callback("dragover", closure.as_ref().unchecked_ref());
        closure.forget();
    }
    {
        let events = Rc::clone(events);
        let closure =
            Closure::<dyn FnMut(web_sys::DragEvent)>::new(move |event: web_sys::DragEvent| {
                event.prevent_default();
                let Some(files) = event.data_transfer().and_then(|transfer| transfer.files())
                else {
                    return;
                };
                for index in 0..files.length() {
                    let Some(file) = files.get(index) else {
                        continue;
                    };
                    let name = file.name();
                    let events = Rc::clone(&events);
                    wasm_bindgen_futures::spawn_local(async move {
                        match read_dropped_file(&file).await {
                            Ok(bytes) => events.borrow_mut().push(RawEvent::Drop { name, bytes }),
                            Err(error) => log::warn!("failed to read dropped file: {error}"),
                        }
                    });
                }
            });
        let _ = window.add_event_listener_with_callback("drop", closure.as_ref().unchecked_ref());
        closure.forget();
    }
}

/// Read a dropped `File`'s bytes. A `File` is a lazy blob, so this awaits its
/// `arrayBuffer()`.
async fn read_dropped_file(file: &web_sys::File) -> Result<Vec<u8>, String> {
    let buffer = wasm_bindgen_futures::JsFuture::from(file.array_buffer())
        .await
        .map_err(|error| format!("{error:?}"))?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
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

        // Mouse-look: request or release the pointer lock as the driver wishes.
        sync_pointer_lock(&mut state);

        let seconds = now / 1000.0;
        present(&mut state, seconds);
    }

    request_frame(&window, handle);
}

/// Draw the driver's UI over the scene and present the frame.
///
/// The scene is rendered and the egui overlay painted into it in one pass
/// (`present_with_overlay`), mirroring the native host — egui's overlay pass
/// loads the rendered colour and draws on top.
fn present<D: AppDriver>(state: &mut Loop<D>, seconds: f64) {
    let Loop {
        driver,
        egui,
        renderer,
        canvas,
        size,
        ..
    } = state;

    let raw_input = egui.begin_frame(canvas, seconds);
    let mut output = egui.ctx.run_ui(raw_input, |ui| driver.ui(ui));
    let paint_jobs = egui.ctx.tessellate(output.shapes, output.pixels_per_point);
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [size.0.max(1), size.1.max(1)],
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
        let extra = egui
            .renderer
            .update_buffers(device, queue, &mut encoder, &paint_jobs, &screen);
        queue.submit(std::iter::once(encoder.finish()).chain(extra));
    }

    if let Some(camera) = driver.camera() {
        let result = renderer.present_with_overlay(
            driver.world(),
            driver.assets(),
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
                egui.renderer.render(&mut pass, &paint_jobs, &screen);
            },
        );
        if let Err(error) = result {
            log::warn!("present failed: {error}");
        }
    }

    for id in &output.textures_delta.free {
        egui.renderer.free_texture(id);
    }
    // Tell epaint the deltas have been applied, or its `Drop` check panics.
    output.textures_delta.clear();
}

/// Apply the pending DOM events to the driver's input, then clear the queue.
fn apply_events<D: AppDriver>(state: &mut Loop<D>) {
    let pending: Vec<RawEvent> = std::mem::take(&mut *state.events.borrow_mut());

    // `Esc` and a dropped file are driver actions, not input state, so they reach
    // the driver even when it takes no input.
    for event in &pending {
        match event {
            RawEvent::Escape => state.driver.on_escape(),
            RawEvent::Drop { name, bytes } => state.driver.drop_bytes(name, bytes),
            _ => {}
        }
    }

    let Some(input) = state.driver.input_mut() else {
        return;
    };
    for event in pending {
        match event {
            RawEvent::Escape | RawEvent::Drop { .. } => {}
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
            RawEvent::Motion { dx, dy } => input.add_mouse_motion(dx, dy),
            RawEvent::Wheel { points } => input.add_scroll(points),
        }
    }
}

/// Request or release the browser's pointer lock to match
/// [`AppDriver::cursor_captured`], only when the driver's wish changes.
///
/// A browser requires a user gesture for a lock request; the game asks for the
/// cursor on the frame after the click or key that started it, still inside the
/// gesture's transient-activation window. If a request is refused, mouse-look
/// falls back to the unlocked absolute-`mousemove` path.
fn sync_pointer_lock<D: AppDriver>(state: &mut Loop<D>) {
    let want = state.driver.cursor_captured();
    if want == state.captured {
        return;
    }
    state.captured = want;
    if want {
        state.canvas.request_pointer_lock();
    } else if let Some(document) = web_sys::window().and_then(|window| window.document()) {
        document.exit_pointer_lock();
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

/// A `log` sink that forwards to the browser console, so the engine's `log`
/// output — wgpu's startup notes, a driver's failures — is visible in a browser
/// (the native host's `StderrLogger` is its counterpart).
struct ConsoleLogger;

impl log::Log for ConsoleLogger {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        let message: wasm_bindgen::JsValue =
            format!("[{}] {}", record.level(), record.args()).into();
        match record.level() {
            log::Level::Error => web_sys::console::error_1(&message),
            log::Level::Warn => web_sys::console::warn_1(&message),
            log::Level::Info => web_sys::console::info_1(&message),
            _ => web_sys::console::log_1(&message),
        }
    }

    fn flush(&self) {}
}

/// Install the browser-console `log` sink.
///
/// Idempotent (a second call is a no-op, like the native `init_logging`), so an
/// entry point can call it first — before the work whose failures it wants to
/// see. Without it the `log` macros are silent on the web.
pub fn init_logging() {
    static LOGGER: ConsoleLogger = ConsoleLogger;
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Info);
}

/// Trigger a browser download of `bytes`, as a file named `name`.
///
/// Wraps the bytes in a `Blob`, points a temporary object URL at it, and clicks a
/// synthetic `<a download>` — how a page saves bytes it generated rather than
/// fetched. The link is attached before the click (some browsers ignore a
/// detached one) and removed after; the object URL is revoked once the click has
/// started the download.
pub fn browser_download(name: &str, bytes: &[u8]) -> Result<(), PlatformError> {
    let parts = js_sys::Array::new();
    parts.push(&js_sys::Uint8Array::from(bytes));
    let blob = web_sys::Blob::new_with_u8_array_sequence(parts.as_ref())
        .map_err(|_| host_err("failed to build a download blob"))?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)
        .map_err(|_| host_err("failed to create a download URL"))?;

    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| host_err("no document"))?;
    let anchor = document
        .create_element("a")
        .map_err(|_| host_err("failed to create a download link"))?
        .dyn_into::<web_sys::HtmlAnchorElement>()
        .map_err(|_| host_err("the download link is not an <a>"))?;
    anchor.set_href(&url);
    anchor.set_download(name);

    let body = document.body();
    if let Some(body) = &body {
        let _ = body.append_child(&anchor);
    }
    anchor.click();
    if let Some(body) = &body {
        let _ = body.remove_child(&anchor);
    }
    let _ = web_sys::Url::revoke_object_url(&url);
    Ok(())
}
