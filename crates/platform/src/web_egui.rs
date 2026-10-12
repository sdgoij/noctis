//! The egui overlay for the web host (`wasm32`).
//!
//! `egui-winit` cannot build for `wasm32-unknown-unknown`, so this host feeds
//! [`egui::RawInput`] from DOM events itself and paints with `egui-wgpu` over the
//! rendered scene — the browser counterpart of the native host's egui layer. It is
//! a first cut: pointer, wheel, keys, text and modifiers, which is what the editor
//! UI needs.

use std::cell::RefCell;
use std::rc::Rc;

use egui::{
    Context, Event, Key, Modifiers, MouseWheelUnit, Pos2, RawInput, Rect, TouchPhase, Vec2,
};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

/// egui state for the web host: a context, its wgpu renderer, and the DOM events
/// gathered between frames.
pub struct WebEgui {
    /// The egui context the driver draws into.
    pub ctx: Context,
    /// The wgpu (re)paint for egui (pipelines, buffers, font atlas).
    pub renderer: egui_wgpu::Renderer,
    /// Events gathered since the last frame.
    events: Rc<RefCell<Vec<Event>>>,
}

impl WebEgui {
    /// Build the context and its renderer, and attach the DOM listeners (they live
    /// for the page's lifetime).
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        window: &web_sys::Window,
        canvas: &web_sys::HtmlCanvasElement,
    ) -> Self {
        let events = Rc::new(RefCell::new(Vec::new()));
        let modifiers = Rc::new(RefCell::new(Modifiers::default()));
        install_listeners(window, canvas, &events, &modifiers);
        Self {
            ctx: Context::default(),
            renderer: egui_wgpu::Renderer::new(
                device,
                format,
                egui_wgpu::RendererOptions::default(),
            ),
            events,
        }
    }

    /// Build this frame's [`RawInput`] from the queued DOM events.
    pub fn begin_frame(&self, canvas: &web_sys::HtmlCanvasElement, seconds: f64) -> RawInput {
        let ppp = web_sys::window()
            .map(|window| window.device_pixel_ratio())
            .unwrap_or(1.0) as f32;
        let size = Vec2::new(
            canvas.client_width().max(1) as f32,
            canvas.client_height().max(1) as f32,
        );
        let rect = Rect::from_min_size(Pos2::ZERO, size);
        let mut input = RawInput {
            screen_rect: Some(rect),
            time: Some(seconds),
            predicted_dt: 1.0 / 60.0,
            focused: true,
            events: std::mem::take(&mut self.events.borrow_mut()),
            ..Default::default()
        };
        input.viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo {
                native_pixels_per_point: Some(ppp),
                inner_rect: Some(rect),
                ..Default::default()
            },
        );
        input
    }
}

/// Attach the DOM listeners that feed [`Event`]s into the frame loop.
fn install_listeners(
    window: &web_sys::Window,
    canvas: &web_sys::HtmlCanvasElement,
    events: &Rc<RefCell<Vec<Event>>>,
    modifiers: &Rc<RefCell<Modifiers>>,
) {
    for down in [true, false] {
        let events = Rc::clone(events);
        let modifiers = Rc::clone(modifiers);
        let closure = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
            move |event: web_sys::KeyboardEvent| {
                let mods = Modifiers {
                    alt: event.alt_key(),
                    ctrl: event.ctrl_key(),
                    shift: event.shift_key(),
                    mac_cmd: event.meta_key(),
                    command: event.ctrl_key() || event.meta_key(),
                };
                let code = event.code();
                let mut queue = events.borrow_mut();
                set_modifiers(&modifiers, &mut queue, mods);
                if let Some(key) = egui_key(&code) {
                    queue.push(Event::Key {
                        key,
                        physical_key: None,
                        pressed: down,
                        repeat: down && event.repeat(),
                        modifiers: mods,
                    });
                }
                if down {
                    if is_ui_key(&code) {
                        // The UI owns these keys: stop the page acting on them.
                        event.prevent_default();
                    } else if !mods.ctrl && !mods.command && !mods.alt {
                        let text = event.key();
                        if !text.is_empty() {
                            queue.push(Event::Text(text));
                        }
                    }
                }
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
                events.borrow_mut().push(Event::PointerMoved(Pos2::new(
                    event.offset_x() as f32,
                    event.offset_y() as f32,
                )));
            });
        let _ =
            canvas.add_event_listener_with_callback("mousemove", closure.as_ref().unchecked_ref());
        closure.forget();
    }

    for down in [true, false] {
        let events = Rc::clone(events);
        let closure =
            Closure::<dyn FnMut(web_sys::MouseEvent)>::new(move |event: web_sys::MouseEvent| {
                if down {
                    event.prevent_default();
                }
                let Some(button) = pointer_button(event.button()) else {
                    return;
                };
                events.borrow_mut().push(Event::PointerButton {
                    pos: Pos2::new(event.offset_x() as f32, event.offset_y() as f32),
                    button,
                    pressed: down,
                    modifiers: Modifiers {
                        alt: event.alt_key(),
                        ctrl: event.ctrl_key(),
                        shift: event.shift_key(),
                        mac_cmd: event.meta_key(),
                        command: event.ctrl_key() || event.meta_key(),
                    },
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
                // The engine counts scroll-up as positive (as winit does); the
                // browser's `deltaY` grows downward.
                events.borrow_mut().push(Event::MouseWheel {
                    unit: MouseWheelUnit::Point,
                    delta: Vec2::new(-(event.delta_x() as f32), -(event.delta_y() as f32)),
                    phase: TouchPhase::Move,
                    modifiers: Modifiers {
                        alt: event.alt_key(),
                        ctrl: event.ctrl_key(),
                        shift: event.shift_key(),
                        mac_cmd: event.meta_key(),
                        command: event.ctrl_key() || event.meta_key(),
                    },
                });
            });
        let _ = canvas.add_event_listener_with_callback("wheel", closure.as_ref().unchecked_ref());
        closure.forget();
    }
}

/// Push `ModifiersChanged` when the modifier state changes, so `ui.input(|i|
/// i.modifiers)` (and every shortcut) sees it.
fn set_modifiers(modifiers: &Rc<RefCell<Modifiers>>, queue: &mut Vec<Event>, mods: Modifiers) {
    let mut current = modifiers.borrow_mut();
    if *current != mods {
        *current = mods;
        queue.push(Event::ModifiersChanged(mods));
    }
}

/// Map a DOM `KeyboardEvent.code` to an egui [`Key`].
///
/// egui's `Key::from_name` already knows `"Digit1"`, `"ArrowUp"`, `"Space"`,
/// `"Escape"`, …; a letter arrives as `"KeyW"` and is reduced to `"W"`.
fn egui_key(code: &str) -> Option<Key> {
    Key::from_name(code).or_else(|| code.strip_prefix("Key").and_then(Key::from_name))
}

/// Whether the UI owns this key (so the page must not act on it, and it is not
/// text).
fn is_ui_key(code: &str) -> bool {
    matches!(
        code,
        "Enter"
            | "NumpadEnter"
            | "Tab"
            | "Backspace"
            | "Delete"
            | "Escape"
            | "ArrowUp"
            | "ArrowDown"
            | "ArrowLeft"
            | "ArrowRight"
            | "Home"
            | "End"
            | "PageUp"
            | "PageDown"
    )
}

/// A DOM mouse button index as an egui [`egui::PointerButton`].
fn pointer_button(button: i16) -> Option<egui::PointerButton> {
    match button {
        0 => Some(egui::PointerButton::Primary),
        1 => Some(egui::PointerButton::Middle),
        2 => Some(egui::PointerButton::Secondary),
        _ => None,
    }
}
