//! The [`ecs::ui::Ui`] seam, implemented over egui.
//!
//! A plugin draws through [`ecs::ui::Ui`]; the host wraps its egui context in
//! this adapter and hands it to the plugin's callback. Keeping the trait in
//! `ecs` and this implementation here is what lets a plugin (a `noctis`
//! consumer) draw a menu without `ecs` depending on egui.

use ecs::ui::Ui;

/// An [`ecs::ui::Ui`] that draws into an egui [`egui::Ui`].
pub struct EguiUi<'a> {
    ui: &'a mut egui::Ui,
}

impl<'a> EguiUi<'a> {
    /// Wrap an egui `Ui` so it can be handed to a plugin's callback.
    pub fn new(ui: &'a mut egui::Ui) -> EguiUi<'a> {
        EguiUi { ui }
    }
}

impl Ui for EguiUi<'_> {
    fn panel(&mut self, id: &str, title: &str, body: &mut dyn FnMut(&mut dyn Ui)) {
        // A titled floating window; `id` tells two same-titled panels apart.
        let mut window = egui::Window::new(title);
        if !id.is_empty() {
            window = window.id(egui::Id::new(id));
        }
        window.show(self.ui.ctx(), |ui| {
            let mut nested = EguiUi { ui };
            body(&mut nested);
        });
    }

    fn label(&mut self, text: &str) {
        self.ui.label(text);
    }

    fn separator(&mut self) {
        self.ui.separator();
    }

    fn button(&mut self, label: &str) -> bool {
        self.ui.button(label).clicked()
    }

    fn toggle(&mut self, label: &str, value: &mut bool) -> bool {
        self.ui.checkbox(value, label).changed()
    }

    fn slider(&mut self, label: &str, value: &mut f32, min: f32, max: f32) -> bool {
        self.ui
            .add(egui::Slider::new(value, min..=max).text(label))
            .changed()
    }

    fn combo(&mut self, label: &str, selected: &mut usize, options: &[&str]) -> bool {
        let chosen = options.get(*selected).copied().unwrap_or("");
        let mut changed = false;
        egui::ComboBox::from_label(label)
            .selected_text(chosen)
            .show_ui(self.ui, |ui| {
                for (index, option) in options.iter().enumerate() {
                    if ui.selectable_value(selected, index, *option).changed() {
                        changed = true;
                    }
                }
            });
        changed
    }

    fn text_field(&mut self, id: &str, value: &mut String) -> bool {
        self.ui
            .add(egui::TextEdit::singleline(value).id(egui::Id::new(id)))
            .changed()
    }
}
