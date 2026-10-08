//! The windowed editor: an [`AppDriver`] that draws egui panels over the
//! scene's live viewport.

use ecs::assets::AssetRegistry;
use ecs::components::{Camera, LocalTransform};
use ecs::resources::{InputState, Key};
use ecs::world::{Entity, World};
use math::{Quat, vec3};
use platform::{AppDriver, PlatformError, WindowConfig, egui};

use crate::editor::Editor;

// `Key::Letter` indices for the WASD keys (A = 0).
const KEY_A: Key = Key::Letter(0);
const KEY_D: Key = Key::Letter(3);
const KEY_S: Key = Key::Letter(18);
const KEY_W: Key = Key::Letter(22);

/// A windowed editor: owns an [`Editor`] and renders it with egui panels.
pub struct EditorDriver {
    editor: Editor,
    input: InputState,
    selected: Option<Entity>,
    show_outliner: bool,
    show_inspector: bool,
    fly_speed: f32,
}

impl std::fmt::Debug for EditorDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditorDriver")
            .field("selected", &self.selected)
            .field("show_outliner", &self.show_outliner)
            .field("show_inspector", &self.show_inspector)
            .field("fly_speed", &self.fly_speed)
            .finish_non_exhaustive()
    }
}

impl EditorDriver {
    /// Wrap an editor for windowed display.
    pub fn new(editor: Editor) -> Self {
        Self {
            editor,
            input: InputState::new(),
            selected: None,
            show_outliner: true,
            show_inspector: true,
            fly_speed: 6.0,
        }
    }

    /// The wrapped editor.
    pub fn editor(&self) -> &Editor {
        &self.editor
    }

    /// The wrapped editor, mutably.
    pub fn editor_mut(&mut self) -> &mut Editor {
        &mut self.editor
    }

    fn axis(&self, positive: Key, negative: Key) -> f32 {
        (self.input.key_down(positive) as i32 - self.input.key_down(negative) as i32) as f32
    }

    fn outliner(&mut self, ui: &mut egui::Ui) {
        ui.heading("Outliner");
        ui.separator();
        let entities = self.editor.list_entities();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for info in &entities {
                let label = if info.name.is_empty() {
                    format!("entity {}", info.entity.index())
                } else {
                    info.name.clone()
                };
                let selected = self.selected == Some(info.entity);
                if ui.selectable_label(selected, label).clicked() {
                    self.selected = Some(info.entity);
                    let _ = self.editor.select(info.entity);
                }
            }
        });
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        ui.heading("Inspector");
        ui.separator();
        let Some(entity) = self.selected else {
            ui.label("Nothing selected.");
            return;
        };
        if !self.editor.scene().world.is_alive(entity) {
            ui.label("Entity no longer exists.");
            self.selected = None;
            return;
        }

        let name = self
            .editor
            .list_entities()
            .iter()
            .find(|info| info.entity == entity)
            .map(|info| info.name.clone())
            .unwrap_or_default();
        let mut name_buffer = name;
        ui.horizontal(|ui| {
            ui.label("Name");
            if ui.text_edit_singleline(&mut name_buffer).changed() {
                let _ = self.editor.rename(entity, name_buffer.clone());
            }
        });

        ui.separator();
        ui.label("Transform");

        let mut translation = self
            .editor
            .translation(entity)
            .unwrap_or_else(|| vec3(0.0, 0.0, 0.0));
        let mut moved = false;
        ui.horizontal(|ui| {
            ui.label("Position");
            moved |= ui
                .add(egui::DragValue::new(&mut translation.x).speed(0.05))
                .changed();
            moved |= ui
                .add(egui::DragValue::new(&mut translation.y).speed(0.05))
                .changed();
            moved |= ui
                .add(egui::DragValue::new(&mut translation.z).speed(0.05))
                .changed();
        });
        if moved {
            let _ = self.editor.set_translation(entity, translation);
        }

        let mut rotation = self
            .editor
            .rotation_euler_degrees(entity)
            .unwrap_or_else(|| vec3(0.0, 0.0, 0.0));
        let mut rotated = false;
        ui.horizontal(|ui| {
            ui.label("Rotation");
            rotated |= ui
                .add(egui::DragValue::new(&mut rotation.x).speed(0.5))
                .changed();
            rotated |= ui
                .add(egui::DragValue::new(&mut rotation.y).speed(0.5))
                .changed();
            rotated |= ui
                .add(egui::DragValue::new(&mut rotation.z).speed(0.5))
                .changed();
        });
        if rotated {
            let _ = self.editor.set_rotation_euler(entity, rotation);
        }

        let mut scale = self
            .editor
            .scale(entity)
            .unwrap_or_else(|| vec3(1.0, 1.0, 1.0));
        let mut scaled = false;
        ui.horizontal(|ui| {
            ui.label("Scale");
            scaled |= ui
                .add(egui::DragValue::new(&mut scale.x).speed(0.02))
                .changed();
            scaled |= ui
                .add(egui::DragValue::new(&mut scale.y).speed(0.02))
                .changed();
            scaled |= ui
                .add(egui::DragValue::new(&mut scale.z).speed(0.02))
                .changed();
        });
        if scaled {
            let _ = self.editor.set_scale(entity, scale);
        }

        ui.separator();
        if let Some(json) = self.editor.inspect(entity) {
            egui::CollapsingHeader::new("Components").show(ui, |ui| {
                ui.monospace(json);
            });
        }
    }

    /// Select the nearest entity under a click in the viewport.
    fn pick_at(&mut self, ui: &egui::Ui, rect: egui::Rect, pointer: egui::Pos2) {
        let pixels_per_point = ui.ctx().pixels_per_point();
        let width = rect.width() * pixels_per_point;
        let height = rect.height() * pixels_per_point;
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        let px = (pointer.x - rect.min.x) * pixels_per_point;
        let py = (pointer.y - rect.min.y) * pixels_per_point;
        let ndc_x = 2.0 * px / width - 1.0;
        let ndc_y = 1.0 - 2.0 * py / height;

        let world = &self.editor.scene().world;
        let Some(camera) = render::first_active_camera(world) else {
            return;
        };
        let Some(view) = render::extract_camera(world, camera, width as u32, height as u32) else {
            return;
        };
        let ray = crate::picking::ray_from_ndc(&view.inverse_view_projection, ndc_x, ndc_y);
        if let Some(entity) = crate::picking::pick_entity(
            &self.editor.scene().world,
            self.editor.assets(),
            &ray,
            1000.0,
        ) {
            self.selected = Some(entity);
            let _ = self.editor.select(entity);
        }
    }
}

impl AppDriver for EditorDriver {
    fn prepare(&mut self) {
        let world = &mut self.editor.scene_mut().world;
        if render::first_active_camera(world).is_none() {
            let camera = world.spawn();
            world.insert(
                camera,
                LocalTransform::from_translation(vec3(0.0, 2.0, 6.0)),
            );
            world.insert(camera, Camera::default());
        }
        self.editor.scene_mut().update_transforms();
    }

    fn update(&mut self, dt: f32) {
        let forward_axis = self.axis(KEY_W, KEY_S);
        let right_axis = self.axis(KEY_D, KEY_A);
        let world = &mut self.editor.scene_mut().world;
        let Some(camera) = render::first_active_camera(world) else {
            return;
        };
        let rotation = world
            .get::<LocalTransform>(camera)
            .map(|t| t.rotation)
            .unwrap_or_else(Quat::identity);
        let forward = rotation.rotate_vec3(vec3(0.0, 0.0, -1.0));
        let right = rotation.rotate_vec3(vec3(1.0, 0.0, 0.0));
        let delta = forward * forward_axis + right * right_axis;
        if delta.x != 0.0 || delta.y != 0.0 || delta.z != 0.0 {
            if let Some(transform) = world.get_mut::<LocalTransform>(camera) {
                transform.translation = transform.translation + delta * (self.fly_speed * dt);
            }
        }
    }

    fn world(&self) -> &World {
        &self.editor.scene().world
    }

    fn assets(&self) -> &AssetRegistry {
        self.editor.assets()
    }

    fn camera(&self) -> Option<Entity> {
        render::first_active_camera(&self.editor.scene().world)
    }

    fn input_mut(&mut self) -> Option<&mut InputState> {
        Some(&mut self.input)
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("noctis_menu").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong("Noctis Editor");
                ui.separator();
                ui.toggle_value(&mut self.show_outliner, "Outliner");
                ui.toggle_value(&mut self.show_inspector, "Inspector");
                ui.separator();
                ui.label(format!("{} entities", self.editor.list_entities().len()));
                ui.label("|  WASD: fly camera");
            });
        });

        if self.show_outliner {
            egui::Panel::left("noctis_outliner").show(ui, |ui| self.outliner(ui));
        }
        if self.show_inspector {
            egui::Panel::right("noctis_inspector").show(ui, |ui| self.inspector(ui));
        }
        egui::CentralPanel::default().show(ui, |ui| {
            ui.label("Viewport");
            ui.separator();
            ui.label("Click an object to select it. WASD flies the camera.");
            let rect = ui.max_rect();
            let response = ui.interact(rect, ui.id().with("viewport_pick"), egui::Sense::click());
            if response.clicked() {
                if let Some(pointer) = response.interact_pointer_pos() {
                    self.pick_at(ui, rect, pointer);
                }
            }
        });
    }
}

/// Open `editor` in a window with egui panels. Blocks until the window closes.
pub fn run(editor: Editor, config: WindowConfig) -> Result<(), PlatformError> {
    platform::run(EditorDriver::new(editor), config)
}
