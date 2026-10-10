//! The windowed editor: an [`AppDriver`] that draws egui panels over the
//! scene's live viewport.

use std::path::{Path, PathBuf};

use ecs::assets::AssetRegistry;
use ecs::components::{Camera, GlobalTransform, LocalTransform, Parent};
use ecs::resources::InputState;
use ecs::world::{Entity, World};
use extensions::ExtensionConfig;
use math::{Mat4, Quat, Vec3, vec3};
use platform::{AppDriver, PlatformError, WindowConfig, egui};
use project::{AssetKind, Project, detect};
use render::{GizmoAxis, GizmoGeometry, GizmoMode};

use crate::camera::{CameraControls, PAN_SPEED, dolly_transform, orbit_transform, pan_transform};
use crate::editor::Editor;
use crate::extension::{ExtensionHost, Widget};
use crate::gizmo;
use crate::inspector::InspectorState;
use crate::{AssetRef, CameraParams, LightParams, MeshRendererParams, ProjectionSpec};

/// The kind of object the "Add" buttons create.
#[derive(Clone, Copy, Debug)]
enum NewObject {
    Cube,
    Sphere,
    Plane,
    Light,
    Camera,
}

/// The gizmo shaft length as a fraction of the viewport height.
const GIZMO_SCREEN_FRACTION: f32 = 0.15;
/// How close (as a fraction of the shaft length) a ray must pass to a handle to
/// grab it.
const GIZMO_HIT_FRACTION: f32 = 0.09;

/// An in-progress gizmo drag.
///
/// The gizmo is *frozen* for the duration of a drag: its origin, frame, length
/// and handle directions are captured at grab time and reused every frame, so
/// the handles do not chase the entity as it moves.
#[derive(Clone, Copy, Debug)]
struct GizmoDrag {
    /// The entity being transformed.
    entity: Entity,
    /// The tool that began the drag.
    mode: GizmoMode,
    /// The handle being dragged.
    axis: GizmoAxis,
    /// Whether the gizmo used the entity's own frame at grab time.
    local: bool,
    /// The gizmo frame's world rotation at grab time (identity when world-aligned).
    frame: Mat4,
    /// The gizmo's world origin at grab time.
    origin: Vec3,
    /// The (unit) world direction of the handle: the axis for translate and
    /// scale, the ring normal for rotate.
    direction: Vec3,
    /// The ring basis at grab time (rotate only), frozen in world space.
    ring_u: Vec3,
    ring_v: Vec3,
    /// The rotation axis in the space `start_rotation` lives (rotate only).
    rotate_axis: Vec3,
    /// The gizmo shaft length at grab time (the scale drag reference).
    length: f32,
    /// The axis parameter at grab time (translate and scale).
    grab_linear: f32,
    /// The ring angle at grab time (rotate).
    grab_angle: f32,
    /// The entity's local rotation at grab time.
    start_rotation: Quat,
    /// The entity's local scale at grab time.
    start_scale: Vec3,
    /// The parent's world matrix at grab time, for world-to-local conversion.
    parent: Option<Mat4>,
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
    gizmo_orientation: gizmo::GizmoOrientation,
    hovered_axis: Option<GizmoAxis>,
    drag: Option<GizmoDrag>,
    /// Remappable camera-navigation bindings.
    controls: CameraControls,
    /// Whether the orbit/pan button was held last frame, for edge detection.
    nav_down: bool,
    /// The world pivot an orbit turns about, captured when the drag begins so
    /// the point stays fixed on screen for the whole gesture.
    orbit_pivot: Option<Vec3>,
    /// Inspector UI state (the add-component selection).
    inspector: InspectorState,
    /// The open project, if any: a dropped file is imported into it.
    project: Option<Project>,
    /// The extension host, when extensions are enabled.
    extensions: Option<ExtensionHost>,
    /// The enablement config and the file it is written to.
    extension_config: Option<(ExtensionConfig, PathBuf)>,
    /// Button clicks from the last frame, fed to panels on the next draw.
    pending_activations: Vec<(usize, usize)>,
    /// The console's input line and scrollback.
    console_line: String,
    console_log: Vec<String>,
    /// Whether the console and Extensions windows are shown.
    show_console: bool,
    show_extensions: bool,
}

impl std::fmt::Debug for EditorDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditorDriver")
            .field("selected", &self.selected)
            .field("show_outliner", &self.show_outliner)
            .field("show_inspector", &self.show_inspector)
            .field("fly_speed", &self.fly_speed)
            .field("gizmo_mode", &self.gizmo_mode)
            .field("gizmo_orientation", &self.gizmo_orientation)
            .field("hovered_axis", &self.hovered_axis)
            .field("drag", &self.drag)
            .field("controls", &self.controls)
            .field("nav_down", &self.nav_down)
            .field("orbit_pivot", &self.orbit_pivot)
            .finish_non_exhaustive()
    }
}

impl EditorDriver {
    /// Wrap an editor for windowed display.
    pub fn new(editor: Editor) -> Self {
        let selected = editor.selection().first().copied();
        Self {
            editor,
            input: InputState::new(),
            selected,
            show_outliner: true,
            show_inspector: true,
            fly_speed: 6.0,
            gizmo_mode: GizmoMode::Translate,
            gizmo_orientation: gizmo::GizmoOrientation::World,
            hovered_axis: None,
            drag: None,
            controls: CameraControls::default(),
            nav_down: false,
            orbit_pivot: None,
            inspector: InspectorState::default(),
            project: None,
            extensions: None,
            extension_config: None,
            pending_activations: Vec::new(),
            console_line: String::new(),
            console_log: Vec::new(),
            show_console: false,
            show_extensions: false,
        }
    }

    /// Associate an open project, so a dropped file is imported into it.
    pub fn with_project(mut self, project: Option<Project>) -> Self {
        self.project = project;
        self
    }

    /// Attach an extension host, its enablement config, and the file the
    /// Extensions screen writes it to.
    pub fn with_extensions(
        mut self,
        host: ExtensionHost,
        config: ExtensionConfig,
        config_path: PathBuf,
    ) -> Self {
        self.extension_config = Some((config, config_path));
        self.extensions = Some(host);
        self
    }

    /// The extension host, if any.
    pub fn extensions(&self) -> Option<&ExtensionHost> {
        self.extensions.as_ref()
    }

    /// Fire the extensions' `"update"` hook and apply any edits it queued.
    fn run_extension_update(&mut self, dt: f32) {
        if let Some(host) = self.extensions.as_mut() {
            let lines = host.update(&mut self.editor, dt);
            self.console_log.extend(lines);
            let logs = host.take_logs();
            self.console_log
                .extend(logs.into_iter().map(|log| format!("log: {log}")));
        }
    }

    /// Draw the extension UI: their panels, the console, and the Extensions screen.
    fn extension_ui(&mut self, ui: &mut egui::Ui) {
        if self.extensions.is_none() {
            return;
        }

        let activations = std::mem::take(&mut self.pending_activations);
        let panels = {
            let host = self.extensions.as_mut().expect("checked above");
            host.draw_panels(&self.editor, &activations)
        };
        // A panel button may have run a verb, queueing edits.
        if let Some(host) = self.extensions.as_mut() {
            let applied = host.apply_pending(&mut self.editor);
            self.console_log.extend(applied);
            let logs = host.take_logs();
            self.console_log
                .extend(logs.into_iter().map(|log| format!("log: {log}")));
        }

        for (panel_index, panel) in panels.iter().enumerate() {
            let mut clicks: Vec<(usize, usize)> = Vec::new();
            egui::Window::new(&panel.title)
                .id(egui::Id::new(("noctis_ext_panel", panel_index)))
                .default_width(220.0)
                .show(ui.ctx(), |ui| {
                    for widget in &panel.widgets {
                        match widget {
                            Widget::Heading(text) => {
                                ui.heading(text);
                            }
                            Widget::Label(text) => {
                                ui.label(text);
                            }
                            Widget::Separator => {
                                ui.separator();
                            }
                            Widget::Button { index, label } => {
                                if ui.button(label).clicked() {
                                    clicks.push((panel_index, *index));
                                }
                            }
                        }
                    }
                });
            self.pending_activations.extend(clicks);
        }

        self.console_window(ui);
        self.extensions_window(ui);
    }

    /// The console: a scrollback and a line that dispatches the verb vocabulary.
    fn console_window(&mut self, ui: &mut egui::Ui) {
        if !self.show_console {
            return;
        }
        let mut open = true;
        let mut submitted: Option<String> = None;
        let log = &self.console_log;
        let line = &mut self.console_line;
        egui::Window::new("Console")
            .id(egui::Id::new("noctis_console"))
            .open(&mut open)
            .default_width(360.0)
            .show(ui.ctx(), |ui| {
                egui::ScrollArea::vertical()
                    .max_height(160.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for entry in log.iter() {
                            ui.label(entry);
                        }
                    });
                let response = ui.text_edit_singleline(line);
                if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                    submitted = Some(std::mem::take(line));
                    response.request_focus();
                }
            });
        if let Some(line) = submitted {
            self.run_console(&line);
        }
        if !open {
            self.show_console = false;
        }
    }

    /// Dispatch a console line and record the reply.
    fn run_console(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        self.console_log.push(format!("> {line}"));
        match self.extensions.as_mut() {
            Some(host) => match host.dispatch(&mut self.editor, line) {
                Ok(reply) => {
                    if !reply.is_empty() {
                        self.console_log.push(reply);
                    }
                }
                Err(error) => self.console_log.push(format!("error: {error}")),
            },
            None => self.console_log.push("no extension host".to_string()),
        }
        if let Some(host) = self.extensions.as_mut() {
            let logs = host.take_logs();
            self.console_log
                .extend(logs.into_iter().map(|log| format!("log: {log}")));
        }
    }

    /// The Extensions screen: a list with enable/disable toggles that persist.
    fn extensions_window(&mut self, ui: &mut egui::Ui) {
        if !self.show_extensions {
            return;
        }
        let mut open = true;
        let mut toggled: Option<(String, bool)> = None;
        let entries: Vec<(String, String, String, bool)> = self
            .extensions
            .as_ref()
            .map(|host| {
                host.entries()
                    .iter()
                    .map(|entry| {
                        (
                            entry.id.clone(),
                            entry.name.clone(),
                            entry.version.clone(),
                            entry.enabled,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let disabled: Vec<String> = self
            .extension_config
            .as_ref()
            .map(|(config, _)| config.disabled.clone())
            .unwrap_or_default();
        egui::Window::new("Extensions")
            .id(egui::Id::new("noctis_extensions"))
            .open(&mut open)
            .default_width(320.0)
            .show(ui.ctx(), |ui| {
                if entries.is_empty() {
                    ui.label("No extensions discovered.");
                }
                for (id, name, version, _enabled) in &entries {
                    let mut on = !disabled.iter().any(|entry| entry == id);
                    if ui
                        .checkbox(&mut on, format!("{name} {version}  ({id})"))
                        .changed()
                    {
                        toggled = Some((id.clone(), on));
                    }
                }
                ui.separator();
                ui.label("Enablement is saved and applies on restart.");
            });
        if let Some((id, enabled)) = toggled {
            self.set_extension_enabled(&id, enabled);
        }
        if !open {
            self.show_extensions = false;
        }
    }

    /// Persist an extension's enablement to the config file.
    fn set_extension_enabled(&mut self, id: &str, enabled: bool) {
        let Some((config, path)) = self.extension_config.as_mut() else {
            return;
        };
        config.set_enabled(id, enabled);
        match config.save(&*path) {
            Ok(()) => self.console_log.push(format!(
                "{} {id} (restart to apply)",
                if enabled { "enabled" } else { "disabled" }
            )),
            Err(error) => self
                .console_log
                .push(format!("failed to save extension config: {error}")),
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

    /// The active transform tool.
    pub fn gizmo_mode(&self) -> GizmoMode {
        self.gizmo_mode
    }

    /// Switch the active transform tool.
    pub fn set_gizmo_mode(&mut self, mode: GizmoMode) {
        if self.gizmo_mode != mode {
            self.gizmo_mode = mode;
            self.hovered_axis = None;
        }
    }

    /// Whether the gizmo handles follow the world axes or the entity's frame.
    pub fn gizmo_orientation(&self) -> gizmo::GizmoOrientation {
        self.gizmo_orientation
    }

    /// Set whether the gizmo handles follow the world axes or the entity frame.
    pub fn set_gizmo_orientation(&mut self, orientation: gizmo::GizmoOrientation) {
        if self.gizmo_orientation != orientation {
            self.gizmo_orientation = orientation;
            self.hovered_axis = None;
        }
    }

    /// Re-point the driver's selection at the editor's, and drop any transient
    /// interaction state. Undo/redo rebuild the world with fresh entity ids, so
    /// the previously selected id is stale afterwards.
    fn sync_selection(&mut self) {
        self.selected = self.editor.selection().first().copied();
        self.drag = None;
        self.hovered_axis = None;
    }

    /// The camera-navigation bindings, for remapping.
    pub fn camera_controls(&self) -> &CameraControls {
        &self.controls
    }

    /// The camera-navigation bindings, mutably, for remapping.
    pub fn camera_controls_mut(&mut self) -> &mut CameraControls {
        &mut self.controls
    }

    /// The world point an orbit turns about: the selected entity's position if
    /// there is a selection, otherwise the world origin.
    fn orbit_pivot_position(&self) -> Vec3 {
        self.selected
            .and_then(|entity| self.world_position(entity))
            .unwrap_or_else(|| vec3(0.0, 0.0, 0.0))
    }

    /// Turn the active camera rigidly about `pivot` by a navigation drag,
    /// preserving the pivot's position on screen (a Blender-style orbit).
    fn orbit(&mut self, pivot: Vec3, dx: f32, dy: f32) {
        let world = &mut self.editor.scene_mut().world;
        let Some(camera) = render::first_active_camera(world) else {
            return;
        };
        if let Some(transform) = world.get_mut::<LocalTransform>(camera) {
            *transform = orbit_transform(*transform, pivot, dx, dy);
        }
    }

    /// Slide the active camera in its own right/up plane by a pan drag. The rate
    /// scales with the distance to `pivot`, so it feels the same at any zoom.
    fn pan(&mut self, pivot: Vec3, dx: f32, dy: f32) {
        let world = &mut self.editor.scene_mut().world;
        let Some(camera) = render::first_active_camera(world) else {
            return;
        };
        if let Some(transform) = world.get_mut::<LocalTransform>(camera) {
            let distance = math::length(transform.translation - pivot).max(1.0);
            *transform = pan_transform(*transform, dx, dy, distance * PAN_SPEED);
        }
    }

    /// Move the active camera along its view direction by a scroll amount.
    fn dolly(&mut self, scroll: f32) {
        let world = &mut self.editor.scene_mut().world;
        let Some(camera) = render::first_active_camera(world) else {
            return;
        };
        if let Some(transform) = world.get_mut::<LocalTransform>(camera) {
            *transform = dolly_transform(*transform, scroll);
        }
    }

    /// Read the pointer navigation gesture from the engine input: an orbit, a
    /// pan (when the modifier is held) or a dolly. `over_viewport` gates starting
    /// a drag so gestures over the panels do not move the camera.
    fn navigate(&mut self, over_viewport: bool) {
        let down = self.input.mouse.is_down(self.controls.orbit_button);
        if down && !self.nav_down {
            if over_viewport {
                self.orbit_pivot = Some(self.orbit_pivot_position());
            } else {
                self.orbit_pivot = None;
            }
        } else if !down {
            self.orbit_pivot = None;
        }
        self.nav_down = down;

        if let Some(pivot) = self.orbit_pivot {
            let [dx, dy] = self.input.mouse.delta;
            if self.controls.panning(&self.input) {
                self.pan(pivot, dx, dy);
            } else {
                self.orbit(pivot, dx, dy);
            }
        }

        if over_viewport {
            let scroll = self.input.mouse.scroll;
            if scroll != 0.0 {
                self.dolly(scroll);
            }
        }
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
                // A 2D scene gets an orthographic camera by default.
                let params = if self.editor.mode().is_2d() {
                    CameraParams {
                        projection: ProjectionSpec::Orthographic {
                            height: 10.0,
                            near: 0.1,
                            far: 100.0,
                        },
                        ..CameraParams::default()
                    }
                } else {
                    CameraParams::default()
                };
                let _ = self.editor.add_camera(entity, params);
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

        // Fold a continuous edit (a slider drag, a typing session) into a single
        // undo step: keep one interactive edit open while the pointer is down or a
        // widget holds focus, so every change folds into it. Skipped during a gizmo
        // drag, which manages its own step.
        if self.drag.is_none() {
            let interacting = ui.ctx().memory(|memory| memory.focused().is_some())
                || ui.ctx().input(|input| input.pointer.any_down());
            if interacting && !self.editor.in_interactive_edit() {
                self.editor.begin_interactive_edit();
            } else if !interacting && self.editor.in_interactive_edit() {
                self.editor.end_interactive_edit();
            }
        }

        // A glTF import hands back the root of the new object so it gets focused.
        let pick = egui::ScrollArea::vertical()
            .show(ui, |ui| {
                crate::inspector::show(&mut self.editor, entity, &mut self.inspector, ui)
            })
            .inner;
        if let Some(entity) = pick {
            self.select(entity);
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

    /// An entity's world matrix, if it has a world transform.
    fn world_matrix(&self, entity: Entity) -> Option<Mat4> {
        self.editor
            .scene()
            .world
            .get::<GlobalTransform>(entity)
            .map(|g| g.matrix)
    }

    /// Whether the gizmo uses the entity's own frame for `mode`.
    ///
    /// Scale is *always* local: scaling a rotated object along an arbitrary world
    /// axis is not representable without shear, so the scale handles follow the
    /// entity regardless of the orientation setting.
    fn uses_local_frame(&self, mode: GizmoMode) -> bool {
        mode == GizmoMode::Scale || self.gizmo_orientation == gizmo::GizmoOrientation::Local
    }

    /// The entity's world rotation matrix (identity if it has no world transform).
    fn frame_of(&self, entity: Entity) -> Mat4 {
        match self.world_matrix(entity) {
            Some(matrix) => gizmo::rotation_of(&matrix),
            None => math::identity(),
        }
    }

    /// The unit world direction of handle `axis` for `entity` under `mode`.
    fn handle_direction(&self, entity: Entity, axis: GizmoAxis, mode: GizmoMode) -> Vec3 {
        let base = gizmo::axis_vector(axis);
        if self.uses_local_frame(mode) {
            gizmo::rotate_axis(&self.frame_of(entity), base)
        } else {
            base
        }
    }

    /// The gizmo handle the ray is over for the selected `entity`, if any.
    fn pick_handle(&self, entity: Entity, ray: &crate::picking::Ray) -> Option<GizmoAxis> {
        let origin = self.world_position(entity)?;
        let length = self.gizmo_length(origin);
        let threshold = length * GIZMO_HIT_FRACTION;
        let dirs = gizmo::AXES.map(|axis| self.handle_direction(entity, axis, self.gizmo_mode));
        match self.gizmo_mode {
            GizmoMode::Rotate => gizmo::pick_ring(ray, origin, dirs, length, threshold),
            _ => gizmo::pick_axis(ray, origin, dirs, length, threshold),
        }
    }

    /// Capture the state a drag needs, freezing the gizmo, or `None` if the grab
    /// ray is degenerate.
    fn begin_drag(
        &self,
        entity: Entity,
        axis: GizmoAxis,
        ray: &crate::picking::Ray,
    ) -> Option<GizmoDrag> {
        let mode = self.gizmo_mode;
        let local = self.uses_local_frame(mode);
        let frame = if local {
            self.frame_of(entity)
        } else {
            math::identity()
        };
        let origin = self.world_position(entity)?;
        let direction = self.handle_direction(entity, axis, mode);
        let (ring_u, ring_v, rotate_axis) = if mode == GizmoMode::Rotate {
            let (u, v) = gizmo::basis_for_axis(axis);
            let (u, v) = if local {
                (gizmo::rotate_axis(&frame, u), gizmo::rotate_axis(&frame, v))
            } else {
                (u, v)
            };
            // The rotation axis, expressed in the space `start_rotation` lives.
            let rotate_axis = if local {
                gizmo::axis_vector(axis)
            } else {
                match self.parent_world(entity) {
                    Some(parent) => gizmo::axis_into_frame(&gizmo::rotation_of(&parent), direction),
                    None => direction,
                }
            };
            (u, v, rotate_axis)
        } else {
            (math::zero3(), math::zero3(), math::zero3())
        };
        let (grab_linear, grab_angle) = if mode == GizmoMode::Rotate {
            (
                0.0,
                gizmo::ring_angle(ray, origin, direction, ring_u, ring_v)?,
            )
        } else {
            (gizmo::axis_parameter(ray, origin, direction)?, 0.0)
        };
        Some(GizmoDrag {
            entity,
            mode,
            axis,
            local,
            frame,
            origin,
            direction,
            ring_u,
            ring_v,
            rotate_axis,
            length: self.gizmo_length(origin),
            grab_linear,
            grab_angle,
            start_rotation: self.editor.rotation(entity)?,
            start_scale: self.editor.scale(entity)?,
            parent: self.parent_world(entity),
        })
    }

    /// Apply an in-progress drag for the current pointer ray.
    fn update_drag(&mut self, drag: GizmoDrag, ray: &crate::picking::Ray) {
        match drag.mode {
            GizmoMode::Translate => {
                let Some(current) = gizmo::axis_parameter(ray, drag.origin, drag.direction) else {
                    return;
                };
                let world =
                    gizmo::dragged_position(drag.origin, drag.direction, drag.grab_linear, current);
                let local = gizmo::local_from_world(drag.parent.as_ref(), world);
                let _ = self.editor.set_translation(drag.entity, local);
            }
            GizmoMode::Rotate => {
                let Some(now) =
                    gizmo::ring_angle(ray, drag.origin, drag.direction, drag.ring_u, drag.ring_v)
                else {
                    return;
                };
                let angle = gizmo::wrap_angle(now - drag.grab_angle);
                let delta = Quat::from_axis_angle(drag.rotate_axis, angle);
                let _ = self
                    .editor
                    .set_rotation_quat(drag.entity, delta * drag.start_rotation);
            }
            GizmoMode::Scale => {
                let Some(current) = gizmo::axis_parameter(ray, drag.origin, drag.direction) else {
                    return;
                };
                let factor = (1.0 + (current - drag.grab_linear) / drag.length).max(0.01);
                let mut scale = drag.start_scale;
                match drag.axis {
                    GizmoAxis::X => scale.x = drag.start_scale.x * factor,
                    GizmoAxis::Y => scale.y = drag.start_scale.y * factor,
                    GizmoAxis::Z => scale.z = drag.start_scale.z * factor,
                }
                let _ = self.editor.set_scale(drag.entity, scale);
            }
        }
    }

    /// Handle camera navigation, gizmo hover, handle dragging, and click-to-select
    /// in the viewport.
    fn viewport_interaction(&mut self, ui: &egui::Ui, rect: egui::Rect, response: &egui::Response) {
        // Camera navigation reads the engine input (see `navigate`); egui only
        // tells us whether a drag begins over the viewport rather than a panel.
        self.navigate(response.contains_pointer());

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
            (Some(ray), Some(entity)) => self.pick_handle(entity, ray),
            _ => None,
        };
        if self.drag.is_none() {
            self.hovered_axis = handle;
        }

        // Begin a drag when a primary-button press lands on a handle.
        if response.drag_started_by(egui::PointerButton::Primary)
            && let (Some(ray), Some(entity), Some(axis)) = (ray.as_ref(), self.selected, handle)
            && let Some(drag) = self.begin_drag(entity, axis, ray)
        {
            self.editor.begin_interactive_edit();
            self.drag = Some(drag);
            self.hovered_axis = Some(axis);
        }

        // Apply the drag as the pointer moves.
        if response.dragged_by(egui::PointerButton::Primary)
            && let (Some(drag), Some(ray)) = (self.drag, ray.as_ref())
        {
            self.update_drag(drag, ray);
            self.hovered_axis = Some(drag.axis);
        }

        // Commit the drag as a single undo step on release.
        if response.drag_stopped_by(egui::PointerButton::Primary) && self.drag.take().is_some() {
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

    /// Give the viewport the project's `[render] environment`, so the editor
    /// previews a scene under the same image-based lighting the game will use.
    ///
    /// This runs before the logger-independent CLI output, so it reports through
    /// `eprintln!` like the rest of `noctis editor` (the crate installs no
    /// logger).
    fn configure_renderer(&self, renderer: &mut render::Renderer) {
        let Some(path) = self
            .project
            .as_ref()
            .and_then(|project| project.manifest().render.environment.as_deref())
        else {
            return;
        };
        match self.editor.assets().hdr.get_by_path(path) {
            Some(hdr) => {
                eprintln!(
                    "noctis editor: environment '{}' ({}x{})",
                    hdr.name, hdr.width, hdr.height
                );
                renderer.set_environment(hdr, render::IblConfig::default());
            }
            None => eprintln!(
                "noctis editor: warning: environment {path:?} is not loaded; \
                 using the neutral environment"
            ),
        }
    }

    fn update(&mut self, dt: f32) {
        self.run_extension_update(dt);

        let forward_axis = self.controls.forward_axis(&self.input);
        let right_axis = self.controls.right_axis(&self.input);
        let up_axis = self.controls.up_axis(&self.input);
        let speed = self.fly_speed;
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
        let up = rotation.rotate_vec3(vec3(0.0, 1.0, 0.0));
        let delta = forward * forward_axis + right * right_axis + up * up_axis;
        if (delta.x != 0.0 || delta.y != 0.0 || delta.z != 0.0)
            && let Some(transform) = world.get_mut::<LocalTransform>(camera)
        {
            transform.translation = transform.translation + delta * (speed * dt);
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
        // While dragging, the gizmo is frozen at its grab-time pose so the handles
        // do not chase the entity as it moves.
        if let Some(drag) = self.drag {
            let geometry = render::build_gizmo(drag.mode, drag.length, Some(drag.axis));
            let mut transform = math::translation(drag.origin);
            if drag.local {
                transform = transform * drag.frame;
            }
            return Some((geometry, transform));
        }
        let entity = self.selected?;
        let origin = self.world_position(entity)?;
        let length = self.gizmo_length(origin);
        let geometry = render::build_gizmo(self.gizmo_mode, length, self.hovered_axis);
        let mut transform = math::translation(origin);
        if self.uses_local_frame(self.gizmo_mode) {
            transform = transform * self.frame_of(entity);
        }
        Some((geometry, transform))
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        // The editor's text is read-only chrome, not content: don't let the user
        // drag-select the labels. Panels created from this `ui` inherit the style.
        ui.style_mut().interaction.selectable_labels = false;

        egui::Panel::top("noctis_menu").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong("Noctis Editor");
                ui.separator();
                ui.toggle_value(&mut self.show_outliner, "Outliner");
                ui.toggle_value(&mut self.show_inspector, "Inspector");
                ui.toggle_value(&mut self.show_console, "Console");
                ui.toggle_value(&mut self.show_extensions, "Extensions");
                ui.separator();
                ui.label("Tool:");
                ui.selectable_value(&mut self.gizmo_mode, GizmoMode::Translate, "Move (1)");
                ui.selectable_value(&mut self.gizmo_mode, GizmoMode::Rotate, "Rotate (2)");
                ui.selectable_value(&mut self.gizmo_mode, GizmoMode::Scale, "Scale (3)");
                ui.separator();
                ui.label("Frame:");
                ui.selectable_value(
                    &mut self.gizmo_orientation,
                    gizmo::GizmoOrientation::World,
                    "World",
                );
                ui.selectable_value(
                    &mut self.gizmo_orientation,
                    gizmo::GizmoOrientation::Local,
                    "Local",
                );
                ui.separator();
                if ui
                    .add_enabled(self.editor.can_undo(), egui::Button::new("Undo"))
                    .clicked()
                    && self.editor.undo()
                {
                    self.sync_selection();
                }
                if ui
                    .add_enabled(self.editor.can_redo(), egui::Button::new("Redo"))
                    .clicked()
                    && self.editor.redo()
                {
                    self.sync_selection();
                }
                ui.separator();
                ui.label(format!("{} entities", self.editor.list_entities().len()));
                ui.label("|  WASD/QE: fly  ·  MMB: orbit  ·  Shift+MMB: pan  ·  wheel: zoom");
            });
        });

        // Tool and edit shortcuts. These go through egui's input (not the engine's
        // key state) so they are properly edge-triggered, and are ignored while a
        // text field has focus. The tools use 1/2/3 because W/E/R would collide
        // with the WASD fly camera.
        if !ui.ctx().egui_wants_keyboard_input() {
            let (tool, undo, redo) = ui.ctx().input(|input| {
                let key = |k: egui::Key| input.key_pressed(k);
                let tool = if key(egui::Key::Num1) {
                    Some(GizmoMode::Translate)
                } else if key(egui::Key::Num2) {
                    Some(GizmoMode::Rotate)
                } else if key(egui::Key::Num3) {
                    Some(GizmoMode::Scale)
                } else {
                    None
                };
                let command = input.modifiers.command;
                let shift = input.modifiers.shift;
                let z = key(egui::Key::Z);
                (tool, command && z && !shift, command && z && shift)
            });
            if let Some(mode) = tool {
                self.set_gizmo_mode(mode);
            }
            if undo && self.editor.undo() {
                self.sync_selection();
            }
            if redo && self.editor.redo() {
                self.sync_selection();
            }
        }

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
                ui.label(
                    "WASD and Q/E fly the camera; drag the middle mouse button to orbit \
                     (hold Shift to pan); the wheel zooms.\n\
                     Click to select. Drag a handle to transform.",
                );
                let rect = ui.max_rect();
                let response = ui.interact(
                    rect,
                    ui.id().with("viewport_pick"),
                    egui::Sense::click_and_drag(),
                );
                self.viewport_interaction(ui, rect, &response);
            });

        self.extension_ui(ui);

        // Flush any transform edit made this frame (a gizmo drag, an inspector
        // field) into the derived world transforms the renderer draws from, so
        // the frame we are about to present is already up to date.
        self.editor.scene_mut().update_transforms();

        // The driver owns its input state, and this is the end of its frame: drop
        // the per-frame mouse deltas/buttons and key edges now that `update` and
        // this UI have consumed them. Held keys survive, so flying keeps working.
        self.input.begin_frame();
    }

    fn drop_file(&mut self, path: &Path) {
        // With an open project, a dropped file is imported into it (and a model is
        // instantiated); without one, a model is imported directly.
        match self.project.clone() {
            Some(project) => match project.import_file(path) {
                Ok(asset) => {
                    eprintln!("noctis editor: imported {}", asset.as_str());
                    if matches!(detect(asset.as_str()), AssetKind::Model) {
                        let file = project.file_for(&asset);
                        if let Err(error) = self.editor.import_gltf(&file) {
                            eprintln!(
                                "noctis editor: failed to instantiate {}: {error}",
                                asset.as_str()
                            );
                        }
                    }
                }
                Err(error) => eprintln!("noctis editor: drop import failed: {error}"),
            },
            None => {
                if let Err(error) = self.editor.import_gltf(path) {
                    eprintln!("noctis editor: drop import failed: {error}");
                }
            }
        }
    }
}

/// The extension host plus the config the Extensions screen writes, attached to
/// a windowed run.
#[derive(Debug)]
pub struct AttachedExtensions {
    /// The live host (loaded extensions; fired each frame).
    pub host: ExtensionHost,
    /// The enablement config the Extensions screen edits.
    pub config: ExtensionConfig,
    /// The file the config is saved to.
    pub config_path: PathBuf,
}

/// Open `editor` in a window with egui panels. Blocks until the window closes.
pub fn run(
    editor: Editor,
    config: WindowConfig,
    project: Option<Project>,
    extensions: Option<AttachedExtensions>,
) -> Result<(), PlatformError> {
    let driver = EditorDriver::new(editor).with_project(project);
    platform::run(attach(driver, extensions), config)
}

/// Open `editor` in a window with explicit camera-navigation bindings.
pub fn run_with_controls(
    editor: Editor,
    config: WindowConfig,
    controls: CameraControls,
    project: Option<Project>,
    extensions: Option<AttachedExtensions>,
) -> Result<(), PlatformError> {
    let mut driver = EditorDriver::new(editor).with_project(project);
    *driver.camera_controls_mut() = controls;
    platform::run(attach(driver, extensions), config)
}

fn attach(driver: EditorDriver, extensions: Option<AttachedExtensions>) -> EditorDriver {
    match extensions {
        Some(AttachedExtensions {
            host,
            config,
            config_path,
        }) => driver.with_extensions(host, config, config_path),
        None => driver,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run one editor frame against a headless egui context and report whether
    /// the root UI's labels were left selectable.
    fn selectable_after_a_frame(driver: &mut EditorDriver, ctx: &egui::Context) -> bool {
        let mut selectable = None;
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            driver.ui(ui);
            selectable = Some(ui.style().interaction.selectable_labels);
        });
        output.drop_without_applying_deltas();
        selectable.expect("the editor UI ran")
    }

    #[test]
    fn editor_labels_are_not_selectable() {
        let mut driver = EditorDriver::new(Editor::new());
        let ctx = egui::Context::default();

        // The editor disables selectable labels each frame, so it holds across
        // frames rather than just the first.
        assert!(!selectable_after_a_frame(&mut driver, &ctx));
        assert!(!selectable_after_a_frame(&mut driver, &ctx));
    }

    #[test]
    fn a_continuous_edit_folds_into_one_undo_step() {
        use crate::command::LightParams;
        use ecs::components::Light;

        let mut editor = Editor::new();
        let entity = editor.spawn("Sun", None);
        editor.add_light(entity, LightParams::default()).unwrap();
        let mut driver = EditorDriver::new(editor);
        driver.select(entity);

        // Simulate a drag: open one interactive edit and make several changes.
        driver.editor_mut().begin_interactive_edit();
        for _ in 0..5 {
            driver
                .editor_mut()
                .edit_component::<Light>(entity, |light| light.intensity += 1.0);
        }

        // One frame with no pointer down and no focus closes the step.
        let ctx = egui::Context::default();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| driver.ui(ui));
        output.drop_without_applying_deltas();

        // spawn + add_light + the single folded drag step.
        assert_eq!(driver.editor().history_len(), 3);
        assert!(driver.editor_mut().undo());
        let light = driver.editor().component::<Light>(entity).unwrap();
        assert_eq!(light.intensity, 1.0, "the whole drag undoes at once");
    }
}
