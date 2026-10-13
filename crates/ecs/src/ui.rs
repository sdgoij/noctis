//! The screen-UI seam: an egui-free way for a plugin to draw widgets.
//!
//! A plugin owns the simulation but not the window, so it cannot draw egui
//! itself. Instead it installs a callback on
//! [`EngineContext::ui`](crate::app::EngineContext::ui) — a [`UiCallback`] —
//! and the host calls it once a frame with a [`Ui`] it implements over its own
//! toolkit. This crate defines the interface and nothing else: the egui
//! implementation lives in `platform`, so `ecs` never depends on egui, and a
//! headless test can drive a [`RecordingUi`] instead.
//!
//! This is the same *engine owns the seam, the host renders it* shape as
//! [`screen_text`](crate::app::EngineContext::screen_text) and
//! [`FrameImage`](crate::resources::FrameImage).
//!
//! ```no_run
//! use ecs::ui::Ui;
//!
//! fn menu(ui: &mut dyn Ui) {
//!     ui.panel("main", "Goats", &mut |ui| {
//!         ui.label("Reverie Amidst Ruins");
//!         if ui.button("Play") {
//!             /* start */
//!         }
//!         let mut volume = 0.5;
//!         ui.slider("Volume", &mut volume, 0.0, 1.0);
//!     });
//! }
//! ```

/// The widgets a plugin may draw. Immediate mode: each call draws one widget
/// and returns its interaction this frame, and [`Ui::panel`] nests a body.
///
/// Deliberately small — the handful of widgets a game menu needs. A consumer
/// that wants a richer toolkit uses egui directly (an `AppDriver`), which a
/// plugin cannot; this seam is the plugin's way in.
pub trait Ui {
    /// A titled, floating panel; `body` draws its contents. `id` distinguishes
    /// two panels with the same title.
    fn panel(&mut self, id: &str, title: &str, body: &mut dyn FnMut(&mut dyn Ui));

    /// A plain text label.
    fn label(&mut self, text: &str);

    /// A horizontal rule.
    fn separator(&mut self);

    /// A clickable button; `true` when it was clicked this frame.
    fn button(&mut self, label: &str) -> bool;

    /// A checkbox bound to `value`; returns whether it was toggled.
    fn toggle(&mut self, label: &str, value: &mut bool) -> bool;

    /// A slider bound to `value` over `min..=max`; returns whether it moved.
    fn slider(&mut self, label: &str, value: &mut f32, min: f32, max: f32) -> bool;

    /// A dropdown of `options` bound to `selected`; returns whether it changed.
    fn combo(&mut self, label: &str, selected: &mut usize, options: &[&str]) -> bool;

    /// A single-line text field bound to `value`; returns whether it changed.
    /// `id` distinguishes fields that share a label.
    fn text_field(&mut self, id: &str, value: &mut String) -> bool;
}

/// A plugin's UI, installed on
/// [`EngineContext::ui`](crate::app::EngineContext::ui) and called once a frame
/// by the host with its [`Ui`] implementation.
pub type UiCallback = Box<dyn FnMut(&mut dyn Ui)>;

/// A [`Ui`] that records the calls made and answers scripted interactions, so a
/// test can drive a menu with no window and no egui.
///
/// ```no_run
/// use ecs::ui::{RecordingUi, Ui};
///
/// let mut ui = RecordingUi::new();
/// ui.press("Play"); // arm the button to report a click
/// let mut clicked = false;
/// ui.panel("main", "Menu", &mut |ui| {
///     clicked = ui.button("Play");
/// });
/// assert!(clicked);
/// assert!(ui.drew("panel:main/Menu"));
/// ```
#[derive(Default)]
pub struct RecordingUi {
    calls: Vec<String>,
    presses: Vec<String>,
}

impl RecordingUi {
    /// A recorder with nothing armed.
    pub fn new() -> RecordingUi {
        RecordingUi::default()
    }

    /// Arm the widget labelled `label` to report an interaction: a button
    /// returns `true`, a toggle flips its value, a slider/field reports a
    /// change. A `&str` or a `String` both work.
    pub fn press(&mut self, label: impl Into<String>) {
        self.presses.push(label.into());
    }

    /// The call log, in order, as `"<widget>:<detail>"`.
    pub fn calls(&self) -> &[String] {
        &self.calls
    }

    /// Whether any call starts with `prefix` (for example `"button:Play"`).
    pub fn drew(&self, prefix: &str) -> bool {
        self.calls.iter().any(|call| call.starts_with(prefix))
    }

    fn armed(&self, label: &str) -> bool {
        self.presses.iter().any(|pressed| pressed == label)
    }
}

impl Ui for RecordingUi {
    fn panel(&mut self, id: &str, title: &str, body: &mut dyn FnMut(&mut dyn Ui)) {
        self.calls.push(format!("panel:{id}/{title}"));
        body(self);
    }

    fn label(&mut self, text: &str) {
        self.calls.push(format!("label:{text}"));
    }

    fn separator(&mut self) {
        self.calls.push("separator".to_string());
    }

    fn button(&mut self, label: &str) -> bool {
        self.calls.push(format!("button:{label}"));
        self.armed(label)
    }

    fn toggle(&mut self, label: &str, value: &mut bool) -> bool {
        self.calls.push(format!("toggle:{label}={value}"));
        if self.armed(label) {
            *value = !*value;
            true
        } else {
            false
        }
    }

    fn slider(&mut self, label: &str, value: &mut f32, min: f32, max: f32) -> bool {
        self.calls
            .push(format!("slider:{label}={value} ({min}..{max})"));
        self.armed(label)
    }

    fn combo(&mut self, label: &str, selected: &mut usize, options: &[&str]) -> bool {
        self.calls
            .push(format!("combo:{label}={selected} of {}", options.len()));
        if self.armed(label) && !options.is_empty() {
            *selected = (*selected + 1) % options.len();
            true
        } else {
            false
        }
    }

    fn text_field(&mut self, id: &str, value: &mut String) -> bool {
        self.calls.push(format!("field:{id}={value}"));
        self.armed(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recording_ui_logs_nested_calls_and_answers_presses() {
        let mut ui = RecordingUi::new();
        ui.press("Play");
        ui.press("Solid");

        let mut clicked = false;
        let mut volume = 0.5;
        let mut solid = false;
        ui.panel("main", "Menu", &mut |ui| {
            ui.label("hello");
            clicked = ui.button("Play");
            let _ = ui.button("Quit");
            ui.separator();
            let _ = ui.toggle("Solid", &mut solid);
            let _ = ui.slider("Volume", &mut volume, 0.0, 1.0);
        });

        assert!(clicked, "the armed button reports a click");
        assert!(solid, "the armed toggle flips");
        assert!(ui.drew("panel:main/Menu"));
        assert!(ui.drew("label:hello"));
        assert!(ui.drew("button:Quit"));
        assert_eq!(
            ui.calls().len(),
            7,
            "panel, label, two buttons, separator, toggle, slider"
        );
    }

    #[test]
    fn an_unarmed_button_does_not_click() {
        let mut ui = RecordingUi::new();
        let mut clicked = true;
        ui.panel("p", "t", &mut |ui| {
            clicked = ui.button("Play");
        });
        assert!(!clicked);
    }
}
