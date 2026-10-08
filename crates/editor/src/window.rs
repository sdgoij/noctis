//! The windowed editor: an [`AppDriver`] that draws egui panels over the
//! scene's live viewport.

use ecs::assets::AssetRegistry;
use ecs::components::{Camera, GlobalTransform, LocalTransform, Parent};
use ecs::resources::{InputState, Key};
use ecs::world::{Entity, World};
use math::{Mat4, Quat, Vec3, vec3};
use platform::{AppDriver, PlatformError, WindowConfig, egui};
use render::{GizmoAxis, GizmoGeometry, GizmoMode};

use crate::editor::Editor;
use crate::gizmo;
use crate::{AssetRef, CameraParams, LightParams, MeshRendererParams};

/// The kind of object the "Add" buttons create.
#[derive(Clone, Copy, Debug)]
enum NewObject {
    Cube,
    Sphere,
    Plane,
    Light,
    Camera,
}

// `Key::Letter` indices for the WASD keys (A = 0).
const KEY_A: Key = Key::Letter(0);
const KEY_D: Key = Key::Letter(3);
const KEY_S: Key = Key::Letter(18);
const KEY_W: Key = Key::Letter(22);

/// The gizmo shaft length as a fraction of the viewport height.
const GIZMO_SCREEN_FRACTION: f32 = 0.15;
/// How close (as a fraction of the shaft length) a ray must pass to a handle to
/// grab it.
const GIZMO_HIT_FRACTION: f32 = 0.09;

/// An in-progress gizmo drag.
#[derive(Clone, Copy, Debug)]
struct GizmoDrag {
    /// The entity being moved.
    entity: Entity,
    /// The axis handle being dragged.
    axis: GizmoAxis,
    /// The (unit) world direction of that axis.
    direction: Vec3,
    /// The entity's world position when the drag began.
    start: Vec3,
    /// The ray/axis parameter at grab time.
    grab: f32,
}

/// A windowed editor: owns an [`Editor`] and renders it with egui panels.
pub struct EditorDriver {
    editor: Editor,
    input: InputState,
    selected: Option<Entity>,
    show_outliner: bool,
    show_inspector: bool,
    fly_speed: f32,
    gizmo_mode: GizmoMode,
    hovered_axis: Option<GizmoAxis>,
    drag: Option<GizmoDrag>,
}

impl std::fmt::Debug for EditorDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditorDriver")
            .field("selected", &self.selected)
            .field("show_outliner", &self.show_outliner)
            .field("show_inspector", &self.show_inspector)
            .field("fly_speed", &self.fly_speed)
            .field("gizmo_mode", &self.gizmo_mode)
            .field("hovered_axis", &self.hovered_axis)
            .field("drag", &self.drag)
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
            gizmo_mode: GizmoMode::Translate,
            hovered_axis: None,
            drag: None,
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

    /// Select `entity` for editing: focuses the gizmo and inspector, and mirrors
    /// the selection into the editor.
    pub fn select(&mut self, entity: Entity) {
        self.selected = Some(entity);
        let _ = self.editor.select(entity);
    }

    fn axis(&self, positive: Key, negative: Key) -> f32 {
        (self.input.key_down(positive) as i32 - self.input.key_down(negative) as i32) as f32
    }

    /// Spawn a new object into the scene and select it.
    fn add_object(&mut self, kind: NewObject) {
        // Lay new objects out on a loose grid so they do not overlap at origin.
        let count = self.editor.list_entities().len() as f32;
        let column = count % 5.0;
        let row = (count / 5.0).floor();
        let offset = vec3((column - 2.0) * 2.0, 0.5, row * 2.0);

        match kind {
            NewObject::Cube => self.add_mesh_object(
                "Cube",
                "primitives/cube",
                testscene::primitives::cube(1.0),
                offset,
            ),
            NewObject::Sphere => self.add_mesh_object(
                "Sphere",
                "primitives/sphere",
                testscene::primitives::sphere(0.5, 16, 24),
                offset,
            ),
            NewObject::Plane => self.add_mesh_object(
                "Plane",
                "primitives/plane",
                testscene::primitives::plane(5.0, 2),
                offset,
            ),
            NewObject::Light => {
                let entity = self.editor.spawn("Sun", None);
                let _ = self.editor.set_translation(entity, vec3(0.0, 6.0, 0.0));
                let _ = self.editor.add_light(entity, LightParams::default());
                self.select(entity);
            }
            NewObject::Camera => {
                let entity = self.editor.spawn("Camera", None);
                let _ = self.editor.set_translation(entity, vec3(0.0, 2.0, 6.0));
                let _ = self.editor.add_camera(entity, CameraParams::default());
                self.select(entity);
            }
        }
        self.editor.scene_mut().update_transforms();
    }

    fn add_mesh_object(
        &mut self,
        label: &str,
        path: &str,
        mesh: ecs::assets::Mesh,
        position: math::Vec3,
    ) {
        // Register the primitive once; later objects reuse the same mesh asset.
        if self.editor.assets().meshes.handle(path).is_none() {
            self.editor.register_mesh(path, mesh);
        }
        let name = format!("{label} {}", self.editor.list_entities().len() + 1);
        let entity = self.editor.spawn(name, None);
        let _ = self.editor.set_translation(entity, position);
        let _ = self.editor.set_mesh_renderer(
            entity,
            MeshRendererParams {
                mesh: Some(AssetRef::path(path)),
                material: Some(AssetRef::path("$default")),
                ..Default::default()
            },
        );
        self.select(entity);
    }

    fn outliner(&mut self, ui: &mut egui::Ui) {
        ui.heading("Outliner");
        ui.horizontal_wrapped(|ui| {
            ui.label("Add:");
            if ui.button("Cube").clicked() {
                self.add_object(NewObject::Cube);
            }
            if ui.button("Sphere").clicked() {
                self.add_object(NewObject::Sphere);
            }
            if ui.button("Plane").clicked() {
                self.add_object(NewObject::Plane);
            }
            if ui.button("Light").clicked() {
                self.add_object(NewObject::Light);
            }
            if ui.button("Camera").clicked() {
                self.add_object(NewObject::Camera);
            }
        });
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
                    self.select(info.entity);
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

    /// A world-space ray through `pointer` inside the viewport `rect`, or `None`
    /// when there is no active camera or the rect is degenerate.
    fn ray_at(
        &self,
        ui: &egui::Ui,
        rect: egui::Rect,
        pointer: egui::Pos2,
    ) -> Option<crate::picking::Ray> {
        let pixels_per_point = ui.ctx().pixels_per_point();
        let width = rect.width() * pixels_per_point;
        let height = rect.height() * pixels_per_point;
        if width <= 0.0 || height <= 0.0 {
            return None;
        }
        let px = (pointer.x - rect.min.x) * pixels_per_point;
        let py = (pointer.y - rect.min.y) * pixels_per_point;
        let ndc_x = 2.0 * px / width - 1.0;
        let ndc_y = 1.0 - 2.0 * py / height;

        let world = &self.editor.scene().world;
        let camera = render::first_active_camera(world)?;
        let view = render::extract_camera(world, camera, width as u32, height as u32)?;
        Some(crate::picking::ray_from_ndc(
            &view.inverse_view_projection,
            ndc_x,
            ndc_y,
        ))
    }

    /// Select the nearest entity under a click in the viewport.
    fn pick_at(&mut self, ui: &egui::Ui, rect: egui::Rect, pointer: egui::Pos2) {
        let Some(ray) = self.ray_at(ui, rect, pointer) else {
            return;
        };
        let hit = crate::picking::pick_entity(
            &self.editor.scene().world,
            self.editor.assets(),
            &ray,
            1000.0,
        );
        if let Some(entity) = hit {
            self.select(entity);
        }
    }

    /// An entity's world-space position, if it has a transform.
    fn world_position(&self, entity: Entity) -> Option<Vec3> {
        let world = &self.editor.scene().world;
        if let Some(global) = world.get::<GlobalTransform>(entity) {
            return Some(global.position());
        }
        world.get::<LocalTransform>(entity).map(|t| t.translation)
    }

    /// The selected entity's parent world matrix, for converting a world-space
    /// drag back into local space. `None` for a root entity.
    fn parent_world(&self, entity: Entity) -> Option<Mat4> {
        let world = &self.editor.scene().world;
        let parent = world.get::<Parent>(entity)?.0;
        world.get::<GlobalTransform>(parent).map(|g| g.matrix)
    }

    /// The gizmo's world-space shaft length at `origin`, sized to stay a roughly
    /// constant fraction of the viewport.
    fn gizmo_length(&self, origin: Vec3) -> f32 {
        let world = &self.editor.scene().world;
        let Some(camera) = render::first_active_camera(world) else {
            return 1.0;
        };
        let Some(view) = render::extract_camera(world, camera, 1280, 720) else {
            return 1.0;
        };
        let Some(projection) = world.get::<Camera>(camera).map(|c| c.projection) else {
            return 1.0;
        };
        gizmo::gizmo_length(&view, &projection, origin, GIZMO_SCREEN_FRACTION)
    }

    /// The gizmo axis handle the ray is over, if any.
    fn axis_at(&self, entity: Entity, ray: &crate::picking::Ray) -> Option<GizmoAxis> {
        let origin = self.world_position(entity)?;
        let length = self.gizmo_length(origin);
        gizmo::pick_axis(ray, origin, length, length * GIZMO_HIT_FRACTION)
    }

    /// Handle gizmo hover, axis dragging, and click-to-select in the viewport.
    fn viewport_interaction(&mut self, ui: &egui::Ui, rect: egui::Rect, response: &egui::Response) {
        let pointer = if response.hovered() || response.dragged() {
            response
                .interact_pointer_pos()
                .or_else(|| ui.ctx().pointer_latest_pos())
        } else {
            None
        };
        let ray = pointer.and_then(|pos| self.ray_at(ui, rect, pos));

        // While idle, highlight whichever handle the cursor is over.
        let handle = match (ray.as_ref(), self.selected) {
            (Some(ray), Some(entity)) => self.axis_at(entity, ray),
            _ => None,
        };
        if self.drag.is_none() {
            self.hovered_axis = handle;
        }

        // Begin a drag when a press lands on a handle.
        if response.drag_started()
            && let (Some(ray), Some(entity), Some(axis)) = (ray.as_ref(), self.selected, handle)
            && let Some(start) = self.world_position(entity)
            && let Some(grab) = gizmo::axis_parameter(ray, start, gizmo::axis_vector(axis))
        {
            self.editor.begin_interactive_edit();
            self.drag = Some(GizmoDrag {
                entity,
                axis,
                direction: gizmo::axis_vector(axis),
                start,
                grab,
            });
            self.hovered_axis = Some(axis);
        }

        // Move the entity along the grabbed axis as the pointer moves.
        if response.dragged()
            && let (Some(drag), Some(ray)) = (self.drag, ray.as_ref())
            && let Some(current) = gizmo::axis_parameter(ray, drag.start, drag.direction)
        {
            let world = gizmo::dragged_position(drag.start, drag.direction, drag.grab, current);
            let parent = self.parent_world(drag.entity);
            let local = gizmo::local_from_world(parent.as_ref(), world);
            let _ = self.editor.set_translation(drag.entity, local);
            self.hovered_axis = Some(drag.axis);
        }

        // Commit the drag as a single undo step on release.
        if response.drag_stopped() && self.drag.take().is_some() {
            self.editor.end_interactive_edit();
        }

        // A plain click (not on a handle) selects the entity under the cursor.
        if response.clicked()
            && self.drag.is_none()
            && handle.is_none()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            self.pick_at(ui, rect, pointer);
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
        if (delta.x != 0.0 || delta.y != 0.0 || delta.z != 0.0)
            && let Some(transform) = world.get_mut::<LocalTransform>(camera)
        {
            transform.translation = transform.translation + delta * (self.fly_speed * dt);
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

    fn gizmo(&self) -> Option<(GizmoGeometry, Mat4)> {
        let entity = self.selected?;
        let origin = self.world_position(entity)?;
        let length = self.gizmo_length(origin);
        let geometry = render::build_gizmo(self.gizmo_mode, length, self.hovered_axis);
        Some((geometry, math::translation(origin)))
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
        // The 3D scene is blitted to the surface *before* egui draws, and egui's
        // default central panel paints an opaque background over it. Use a
        // transparent frame so the viewport shows the rendered scene.
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(egui::Color32::TRANSPARENT))
            .show(ui, |ui| {
                ui.label("Viewport");
                ui.separator();
                ui.label("Click to select. Drag the arrows to move. WASD flies the camera.");
                let rect = ui.max_rect();
                let response = ui.interact(
                    rect,
                    ui.id().with("viewport_pick"),
                    egui::Sense::click_and_drag(),
                );
                self.viewport_interaction(ui, rect, &response);
            });

        // Flush any transform edit made this frame (a gizmo drag, an inspector
        // field) into the derived world transforms the renderer draws from, so
        // the frame we are about to present is already up to date.
        self.editor.scene_mut().update_transforms();
    }
}

/// Open `editor` in a window with egui panels. Blocks until the window closes.
pub fn run(editor: Editor, config: WindowConfig) -> Result<(), PlatformError> {
    platform::run(EditorDriver::new(editor), config)
}
