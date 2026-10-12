//! The windowed editor: an [`AppDriver`] that draws egui panels over the
//! scene's live viewport.

use std::collections::HashSet;
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
use crate::extension::{ExtensionHost, ExtensionReload, Interactions, Panel as ExtPanel, Widget};
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

/// One entry in the scene picker: a project scene's display name and its file.
#[derive(Clone, Debug)]
struct SceneChoice {
    name: String,
    path: PathBuf,
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

/// The dock a view opens in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dock {
    /// The left dock.
    Left,
    /// The right dock.
    Right,
    /// The bottom dock, between the side docks.
    Bottom,
}

/// One dockable UI panel: the unit a dock hosts and the menu toggles.
#[derive(Clone, Debug, PartialEq, Eq)]
enum View {
    /// The scene hierarchy.
    Outliner,
    /// The selected entity's components.
    Inspector,
    /// The verb console and its scrollback.
    Console,
    /// The Extensions enable/disable screen.
    Extensions,
    /// One extension panel: its stable id and its display title.
    Extension {
        /// The panel's stable id (`"{extension_id}:{title}"`).
        id: String,
        /// The panel's title (its hover tooltip).
        title: String,
    },
}

/// A small monochrome icon drawn in a tab or a status-bar button.
///
/// Placeholder representation: the glyph is drawn as text through egui's bundled
/// icon fonts (all monochrome). A future icon set can swap the glyph for an image
/// without changing the call sites.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Icon(&'static str);

impl Icon {
    /// The glyph this icon draws.
    const fn glyph(self) -> &'static str {
        self.0
    }
}

impl Default for Icon {
    /// The fallback a control draws when a view names no icon.
    fn default() -> Self {
        Icon("\u{26f6}")
    }
}

impl View {
    /// Every view, in the order the toggles appear.
    const ALL: [View; 4] = [
        View::Outliner,
        View::Inspector,
        View::Console,
        View::Extensions,
    ];

    /// The view's name: its identity and its hover tooltip. Never drawn inline.
    fn title(&self) -> &str {
        match self {
            View::Outliner => "Outliner",
            View::Inspector => "Inspector",
            View::Console => "Console",
            View::Extensions => "Extensions",
            View::Extension { title, .. } => title.as_str(),
        }
    }

    /// The icon the tab and status-bar button draw. An extension panel names none
    /// and so takes the default.
    fn icon(&self) -> Option<Icon> {
        match self {
            View::Outliner => Some(Icon("\u{1f5c0}")),
            View::Inspector => Some(Icon("\u{2699}")),
            View::Console => Some(Icon("\u{1f5cb}")),
            View::Extensions => Some(Icon("\u{1f517}")),
            View::Extension { .. } => None,
        }
    }

    /// An optional one-line description, shown under the title on hover.
    fn hint(&self) -> Option<&str> {
        match self {
            View::Outliner => Some("The scene hierarchy"),
            View::Inspector => Some("The selected entity's components"),
            View::Console => Some("Dispatch verb lines and read the log"),
            View::Extensions => Some("Enable or disable extensions"),
            View::Extension { .. } => None,
        }
    }

    /// The hover tooltip: the title, with `hint` appended when present.
    fn tooltip(&self) -> String {
        match self.hint() {
            Some(hint) => format!("{}\n{hint}", self.title()),
            None => self.title().to_owned(),
        }
    }

    /// The dock the view opens in.
    fn default_dock(&self) -> Dock {
        match self {
            View::Outliner => Dock::Left,
            View::Inspector => Dock::Right,
            View::Console | View::Extensions | View::Extension { .. } => Dock::Bottom,
        }
    }
}

/// A dock's hosted views and the active tab.
#[derive(Clone, Debug, Default)]
struct DockState {
    /// The views shown, in tab order. Empty means the dock is hidden.
    open: Vec<View>,
    /// The index of the active tab.
    active: usize,
}

/// Which views sit in which dock.
#[derive(Clone, Debug, Default)]
struct Layout {
    left: DockState,
    right: DockState,
    bottom: DockState,
}

impl Layout {
    /// The editor's default layout: the outliner left, the inspector right, and
    /// the bottom dock empty.
    fn editor_default() -> Self {
        Self {
            left: DockState {
                open: vec![View::Outliner],
                active: 0,
            },
            right: DockState {
                open: vec![View::Inspector],
                active: 0,
            },
            bottom: DockState::default(),
        }
    }

    /// The state of `dock`.
    fn dock(&self, dock: Dock) -> &DockState {
        match dock {
            Dock::Left => &self.left,
            Dock::Right => &self.right,
            Dock::Bottom => &self.bottom,
        }
    }

    /// The mutable state of `dock`.
    fn dock_mut(&mut self, dock: Dock) -> &mut DockState {
        match dock {
            Dock::Left => &mut self.left,
            Dock::Right => &mut self.right,
            Dock::Bottom => &mut self.bottom,
        }
    }

    /// Whether `view` is currently shown.
    fn is_open(&self, view: &View) -> bool {
        self.dock(view.default_dock()).open.contains(view)
    }

    /// Show `view` in its dock (and select it), or hide it if it is already shown.
    fn toggle(&mut self, view: View) {
        let state = self.dock_mut(view.default_dock());
        match state.open.iter().position(|open| open == &view) {
            Some(index) => {
                state.open.remove(index);
                // Keep the active tab in range after the removal.
                state.active = state.active.min(state.open.len().saturating_sub(1));
            }
            None => {
                state.open.push(view);
                state.active = state.open.len() - 1;
            }
        }
    }

    /// Show `view` in its dock and select it, if it is not already shown.
    fn open(&mut self, view: View) {
        let state = self.dock_mut(view.default_dock());
        if !state.open.contains(&view) {
            state.open.push(view);
            state.active = state.open.len() - 1;
        }
    }
}

/// A windowed editor: owns an [`Editor`] and renders it with egui panels.
pub struct EditorDriver {
    editor: Editor,
    input: InputState,
    selected: Option<Entity>,
    /// The dock layout: which views are open, and where.
    layout: Layout,
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
    /// The open project, if any: a dropped file is imported into it, and Save
    /// writes the current scene and prefabs back into it.
    project: Option<Project>,
    /// The current scene's file (absolute), for Save and the scene picker.
    scene_path: Option<PathBuf>,
    /// The project's scenes (name + absolute path), for the picker.
    scenes: Vec<SceneChoice>,
    /// The last Save / scene-switch result, shown in the menu bar.
    status: Option<String>,
    /// The extension host, when extensions are enabled.
    extensions: Option<ExtensionHost>,
    /// The enablement config and the file it is written to.
    extension_config: Option<(ExtensionConfig, PathBuf)>,
    /// When `--watch` was passed: the watcher and the inputs to rebuild the host.
    extension_reload: Option<ExtensionReload>,
    /// The directories extensions are discovered from, so the Extensions screen can
    /// re-discover and apply a toggle immediately.
    extension_dirs: Vec<PathBuf>,
    /// The project-level disabled ids.
    extension_disabled: Vec<String>,
    /// UI interactions from the last frame, fed to panels on the next draw.
    pending_interactions: Interactions,
    /// The console's input line and scrollback.
    console_line: String,
    console_log: Vec<String>,
    /// The extension panels drawn this frame (each is a dock view).
    extension_panels: Vec<ExtPanel>,
    /// Panel ids seen so far, so a panel opens in its dock only the first time.
    seen_panels: HashSet<String>,
}

impl std::fmt::Debug for EditorDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditorDriver")
            .field("selected", &self.selected)
            .field("layout", &self.layout)
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
            layout: Layout::editor_default(),
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
            scene_path: None,
            scenes: Vec::new(),
            status: None,
            extensions: None,
            extension_config: None,
            extension_reload: None,
            extension_dirs: Vec::new(),
            extension_disabled: Vec::new(),
            pending_interactions: Interactions::default(),
            console_line: String::new(),
            console_log: Vec::new(),
            extension_panels: Vec::new(),
            seen_panels: HashSet::new(),
        }
    }

    /// Associate an open project: a dropped file is imported into it, Save writes
    /// the scene and prefabs back into it, and its scenes populate the picker.
    pub fn with_project(mut self, project: Option<Project>) -> Self {
        self.scenes = project.as_ref().map(scene_choices).unwrap_or_default();
        self.project = project;
        self
    }

    /// Set the current scene's file (absolute), for Save and the scene picker.
    pub fn with_scene_path(mut self, path: Option<PathBuf>) -> Self {
        self.scene_path = path;
        self
    }

    /// Attach an extension host, its enablement config, the file the Extensions
    /// screen writes it to, and (under `--watch`) the reload inputs.
    pub fn with_extensions(
        mut self,
        host: ExtensionHost,
        config: ExtensionConfig,
        config_path: PathBuf,
        dirs: Vec<PathBuf>,
        disabled: Vec<String>,
        reload: Option<ExtensionReload>,
    ) -> Self {
        self.extension_config = Some((config, config_path));
        self.extension_dirs = dirs;
        self.extension_disabled = disabled;
        self.extension_reload = reload;
        self.extensions = Some(host);
        self
    }

    /// The extension host, if any.
    pub fn extensions(&self) -> Option<&ExtensionHost> {
        self.extensions.as_ref()
    }

    /// Fire the extensions' `"update"` hook and apply any edits it queued.
    fn run_extension_update(&mut self, dt: f32) {
        self.reload_extensions();
        if let Some(host) = self.extensions.as_mut() {
            let lines = host.update(&mut self.editor, dt);
            self.console_log.extend(lines);
            let logs = host.take_logs();
            self.console_log
                .extend(logs.into_iter().map(|log| format!("log: {log}")));
        }
    }

    /// Under `--watch`, rebuild the extensions when a file changed.
    fn reload_extensions(&mut self) {
        let Some(reload) = self.extension_reload.as_ref() else {
            return;
        };
        if !reload.watch.changed() {
            return;
        }
        let Some((config, _)) = self.extension_config.clone() else {
            return;
        };
        let disabled = self.extension_disabled.clone();
        let dirs = self.extension_dirs.clone();
        self.rebuild_extensions(&dirs, &config, &disabled);
    }

    /// Re-discover `dirs` and evaluate the enabled extensions afresh, replacing
    /// the host.
    ///
    /// A rebuild is a full reload: the old host is dropped and a fresh one
    /// re-evaluates every extension. Scene edits an extension already made live
    /// on in the editor's undo history.
    fn rebuild_extensions(
        &mut self,
        dirs: &[PathBuf],
        config: &ExtensionConfig,
        disabled: &[String],
    ) {
        let loader = extensions::Loader::discover(dirs);
        for error in loader.errors() {
            self.console_log
                .push(format!("ext: {}: {}", error.path.display(), error.message));
        }
        let enabled = |id: &str| config.is_enabled(id) && !disabled.iter().any(|d| d == id);
        let mut host = match ExtensionHost::new() {
            Ok(host) => host,
            Err(error) => {
                self.console_log.push(format!("extensions: {error}"));
                return;
            }
        };
        host.load(&loader, enabled);
        let lines = host.ready(&mut self.editor);
        self.console_log.extend(lines);
        for error in host.errors() {
            self.console_log.push(format!("ext: {error}"));
        }
        let logs = host.take_logs();
        self.console_log
            .extend(logs.into_iter().map(|log| format!("log: {log}")));
        self.extensions = Some(host);
        self.console_log.push("extensions reloaded".into());
    }

    /// Draw the extensions' panels once for this frame and reconcile the dock
    /// views with them.
    ///
    /// `draw_panels` has side effects — it runs the extensions and reads the
    /// interactions they recorded last frame — so it runs exactly once per frame,
    /// here, before the docks render the panels it produced. A panel seen for the
    /// first time opens in its dock; a panel that is gone (its extension was
    /// disabled or reloaded away) drops out of whichever dock held it.
    fn refresh_extension_panels(&mut self) {
        let interactions = std::mem::take(&mut self.pending_interactions);
        let panels = match self.extensions.as_mut() {
            Some(host) => host.draw_panels(&self.editor, &interactions),
            None => Vec::new(),
        };
        // A panel button (or the draw itself) may have queued edits; apply them now.
        if let Some(host) = self.extensions.as_mut() {
            let applied = host.apply_pending(&mut self.editor);
            self.console_log.extend(applied);
            let logs = host.take_logs();
            self.console_log
                .extend(logs.into_iter().map(|log| format!("log: {log}")));
        }

        // Drop views whose extension panel is no longer drawn.
        let live: HashSet<String> = panels.iter().map(|panel| panel.id.clone()).collect();
        for dock in [Dock::Left, Dock::Right, Dock::Bottom] {
            let state = self.layout.dock_mut(dock);
            state.open.retain(|view| match view {
                View::Extension { id, .. } => live.contains(id),
                _ => true,
            });
            state.active = state.active.min(state.open.len().saturating_sub(1));
        }
        // Open a panel in its dock the first time it appears.
        let first_seen: Vec<View> = panels
            .iter()
            .filter(|panel| !self.seen_panels.contains(&panel.id))
            .map(|panel| View::Extension {
                id: panel.id.clone(),
                title: panel.title.clone(),
            })
            .collect();
        for view in first_seen {
            self.layout.open(view);
        }
        self.seen_panels
            .extend(panels.iter().map(|panel| panel.id.clone()));
        self.extension_panels = panels;
    }

    /// Every view the status bar offers: the built-ins, then one per extension
    /// panel drawn this frame.
    fn views(&self) -> Vec<View> {
        let mut views = View::ALL.to_vec();
        views.extend(self.extension_panels.iter().map(|panel| View::Extension {
            id: panel.id.clone(),
            title: panel.title.clone(),
        }));
        views
    }

    /// Draw one extension panel (by its stable id) as a dock view.
    ///
    /// The interaction indices are the panel's position in the frame's panel list
    /// — the same global index the host computes (`panel_base` + local id).
    fn extension_panel(&mut self, id: &str, ui: &mut egui::Ui) {
        let Some(panel_index) = self
            .extension_panels
            .iter()
            .position(|panel| panel.id.as_str() == id)
        else {
            ui.label("This panel is no longer available.");
            return;
        };
        // Clone the widgets out: writing `pending_interactions` below must not
        // overlap a borrow of `self.extension_panels`.
        let widgets = self.extension_panels[panel_index].widgets.clone();
        let mut clicks: Vec<(usize, usize)> = Vec::new();
        let mut values: Vec<((usize, usize), f64)> = Vec::new();
        for widget in &widgets {
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
                Widget::Checkbox {
                    index,
                    label,
                    checked,
                } => {
                    let mut state = *checked;
                    if ui.checkbox(&mut state, label).changed() {
                        values.push(((panel_index, *index), if state { 1.0 } else { 0.0 }));
                    }
                }
                Widget::Slider {
                    index,
                    label,
                    value,
                    min,
                    max,
                } => {
                    let mut state = *value;
                    let slider = egui::Slider::new(&mut state, *min..=*max).text(label);
                    if ui.add(slider).changed() {
                        values.push(((panel_index, *index), state));
                    }
                }
            }
        }
        self.pending_interactions.activations.extend(clicks);
        self.pending_interactions.values.extend(values);
    }

    /// The console: a scrollback and a line that dispatches the verb vocabulary.
    fn console(&mut self, ui: &mut egui::Ui) {
        let mut submitted: Option<String> = None;
        {
            let log = &self.console_log;
            let line = &mut self.console_line;
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
        }
        if let Some(line) = submitted {
            self.run_console(&line);
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
    fn extensions_screen(&mut self, ui: &mut egui::Ui) {
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
        ui.label("Enablement is saved and applied immediately.");
        if let Some((id, enabled)) = toggled {
            self.set_extension_enabled(&id, enabled);
        }
    }

    /// Persist an extension's enablement to the config file and apply it now.
    fn set_extension_enabled(&mut self, id: &str, enabled: bool) {
        let Some((mut config, path)) = self.extension_config.clone() else {
            return;
        };
        config.set_enabled(id, enabled);
        if let Err(error) = config.save(&path) {
            self.console_log
                .push(format!("failed to save extension config: {error}"));
            return;
        }
        self.console_log.push(format!(
            "{} {id}",
            if enabled { "enabled" } else { "disabled" }
        ));
        // Rebuild so the change takes effect now, not on the next restart.
        self.extension_config = Some((config.clone(), path));
        let dirs = self.extension_dirs.clone();
        let disabled = self.extension_disabled.clone();
        self.rebuild_extensions(&dirs, &config, &disabled);
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

    /// Save the current scene (and the prefab library) back into the project, or
    /// to the scene's own file for a loose `--scene`.
    fn save(&mut self) {
        let target = match self.scene_path.clone() {
            Some(path) => path,
            None => match self.project.as_ref() {
                Some(project) => project.scenes_dir().join(format!(
                    "{}.scene.json",
                    scene_file_stem(self.editor.scene_name())
                )),
                None => {
                    self.status =
                        Some("Nothing to save to: open a project or pass --scene".to_string());
                    return;
                }
            },
        };

        if let Err(error) = self.editor.save(&target) {
            self.status = Some(format!("Save failed: {error}"));
            return;
        }

        let mut message = match self.project.as_ref() {
            Some(project) => format!(
                "Saved {}",
                target
                    .strip_prefix(project.root())
                    .unwrap_or(&target)
                    .display()
            ),
            None => format!("Saved {}", target.display()),
        };

        // The prefab library lives in the project, so save it there too.
        if let Some(project) = self.project.as_mut() {
            for prefab in self.editor.prefabs() {
                if let Err(error) = project.save_prefab(prefab) {
                    message = format!("Saved the scene, but a prefab failed: {error}");
                    break;
                }
            }
        }

        self.scene_path = Some(target);
        self.status = Some(message);
    }

    /// Destroy the selected entity (and its descendants), undoably.
    fn delete_selected(&mut self) {
        let Some(entity) = self.selected else {
            return;
        };
        self.editor.destroy(entity);
        self.selected = None;
        self.drag = None;
        self.hovered_axis = None;
    }

    /// The picker index of the current scene, when it is one of the project's.
    fn current_scene_index(&self) -> Option<usize> {
        let path = self.scene_path.as_ref()?;
        self.scenes.iter().position(|choice| &choice.path == path)
    }

    /// Switch to the project scene at `index`, replacing the current scene.
    fn switch_scene(&mut self, index: usize) {
        let Some(choice) = self.scenes.get(index) else {
            return;
        };
        let path = choice.path.clone();
        let name = choice.name.clone();
        match self.editor.open_or_new(&path) {
            Ok(()) => {
                self.scene_path = Some(path);
                self.selected = None;
                self.drag = None;
                self.hovered_axis = None;
                self.status = Some(format!("Opened {name}"));
            }
            Err(error) => self.status = Some(format!("Open failed: {error}")),
        }
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

    /// The title bar: the app name, the menus, and (on a custom-frame platform)
    /// the window controls. The bar's background drags the window.
    ///
    /// The drag and the window buttons send `ViewportCommand`s that reach winit
    /// only when the OS chrome is hidden (`WindowConfig::decorations`); the host
    /// applies them (see `crates/platform`).
    fn title_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("noctis_titlebar").show(ui, |ui| {
            // The bar's background is a drag handle. It is added before the
            // widgets, so the widgets (later in the same layer) win the pointer
            // where they are; the empty space drags the window.
            let bar = ui.interact(
                ui.max_rect(),
                ui.id().with("noctis_titlebar_drag"),
                egui::Sense::click_and_drag(),
            );
            if bar.drag_started_by(egui::PointerButton::Primary) {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if bar.double_clicked() {
                let maximized = ui.input(|input| input.viewport().maximized.unwrap_or(false));
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }

            let mut switch = None;
            ui.horizontal(|ui| {
                ui.strong("Noctis Editor");
                ui.separator();
                if !self.scenes.is_empty() {
                    ui.label("Scene:");
                    let current = self.current_scene_index();
                    let label = current
                        .map(|index| self.scenes[index].name.clone())
                        .unwrap_or_else(|| self.editor.scene_name().to_string());
                    egui::ComboBox::from_id_salt("noctis_scene")
                        .selected_text(label)
                        .show_ui(ui, |ui| {
                            for (index, choice) in self.scenes.iter().enumerate() {
                                if ui
                                    .selectable_label(current == Some(index), &choice.name)
                                    .clicked()
                                {
                                    switch = Some(index);
                                }
                            }
                        });
                    ui.separator();
                }
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
                let can_save = self.scene_path.is_some() || self.project.is_some();
                if ui
                    .add_enabled(can_save, egui::Button::new("Save"))
                    .clicked()
                {
                    self.save();
                }

                // The window buttons, right-aligned. Only drawn with the custom
                // frame (Windows/Linux); macOS keeps the native ones.
                #[cfg(not(target_os = "macos"))]
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("\u{1f5d9}").on_hover_text("Close").clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    let maximized = ui.input(|input| input.viewport().maximized.unwrap_or(false));
                    let hint = if maximized { "Restore" } else { "Maximize" };
                    if ui.button("\u{26f6}").on_hover_text(hint).clicked() {
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                    }
                    if ui.button("\u{2212}").on_hover_text("Minimize").clicked() {
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    }
                });
            });
            if let Some(index) = switch {
                self.switch_scene(index);
            }
        });
    }

    /// The status bar: the view toggles on the left, read-only status on the right.
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let views = self.views();
        egui::Panel::bottom("noctis_status").show(ui, |ui| {
            ui.horizontal(|ui| {
                // The left cluster: one icon toggle per view. The icon is always
                // drawn (a default when the view names none); the title is hover-only.
                let mut toggle = None;
                for view in &views {
                    let on = self.layout.is_open(view);
                    let icon = view.icon().unwrap_or_default();
                    if ui
                        .selectable_label(on, icon.glyph())
                        .on_hover_text(view.tooltip())
                        .clicked()
                    {
                        toggle = Some(view.clone());
                    }
                }
                if let Some(view) = toggle {
                    self.layout.toggle(view);
                }
                // The right cluster: the last save / scene note and the entity count.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("{} entities", self.editor.list_entities().len()));
                    if let Some(status) = &self.status {
                        ui.separator();
                        ui.label(status);
                    }
                });
            });
        });
    }

    /// Draw a dock's tab strip and its active view, if the dock holds any views.
    fn dock(&mut self, ui: &mut egui::Ui, dock: Dock) {
        if self.layout.dock(dock).open.is_empty() {
            return;
        }
        let panel = match dock {
            Dock::Left => egui::Panel::left("noctis_dock_left"),
            Dock::Right => egui::Panel::right("noctis_dock_right"),
            Dock::Bottom => egui::Panel::bottom("noctis_dock_bottom"),
        }
        .resizable(true)
        .default_size(match dock {
            Dock::Left | Dock::Right => 240.0,
            Dock::Bottom => 200.0,
        });
        panel.show(ui, |ui| {
            // Copy the open views out so the `&mut self` borrows inside
            // `dock_tabs` and `show_view` do not overlap `self.layout`.
            let views = self.layout.dock(dock).open.clone();
            let active = self.layout.dock(dock).active;
            self.dock_tabs(ui, dock, &views, active);
            if let Some(view) = views.get(active).cloned() {
                self.show_view(&view, ui);
            }
        });
    }

    /// A dock's tab strip: one selectable label per hosted view.
    fn dock_tabs(&mut self, ui: &mut egui::Ui, dock: Dock, views: &[View], active: usize) {
        let mut select = None;
        ui.horizontal_wrapped(|ui| {
            for (index, view) in views.iter().enumerate() {
                if ui.selectable_label(index == active, view.title()).clicked() {
                    select = Some(index);
                }
            }
        });
        ui.separator();
        if let Some(index) = select {
            self.layout.dock_mut(dock).active = index;
        }
    }

    /// Draw the view `view` into `ui`.
    fn show_view(&mut self, view: &View, ui: &mut egui::Ui) {
        match view {
            View::Outliner => self.outliner(ui),
            View::Inspector => self.inspector(ui),
            View::Console => self.console(ui),
            View::Extensions => self.extensions_screen(ui),
            View::Extension { id, .. } => self.extension_panel(id, ui),
        }
    }

    fn outliner(&mut self, ui: &mut egui::Ui) {
        let mut delete = false;
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
            ui.separator();
            if ui
                .add_enabled(self.selected.is_some(), egui::Button::new("Delete"))
                .clicked()
            {
                delete = true;
            }
        });
        if delete {
            self.delete_selected();
        }
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

        // Draw the extensions' panels first: the docks below render them, and the
        // draws must happen once, before the panels they produced are shown.
        self.refresh_extension_panels();

        self.title_bar(ui);

        // Tool and edit shortcuts. These go through egui's input (not the engine's
        // key state) so they are properly edge-triggered, and are ignored while a
        // text field has focus. The tools use 1/2/3 because W/E/R would collide
        // with the WASD fly camera.
        if !ui.ctx().egui_wants_keyboard_input() {
            let (tool, undo, redo, delete, save) = ui.ctx().input(|input| {
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
                let delete = key(egui::Key::Delete) || key(egui::Key::Backspace);
                let save = command && key(egui::Key::S);
                (
                    tool,
                    command && z && !shift,
                    command && z && shift,
                    delete,
                    save,
                )
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
            if delete {
                self.delete_selected();
            }
            if save {
                self.save();
            }
        }

        // The status bar is the bottom-most strip: claim it before the docks so it
        // spans the full width.
        self.status_bar(ui);

        // The docks claim space around the central viewport. Order matters: the
        // bottom dock is claimed after the sides, so it sits between them.
        self.dock(ui, Dock::Left);
        self.dock(ui, Dock::Right);
        self.dock(ui, Dock::Bottom);
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
            Some(mut project) => match project.import_file(path) {
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

/// The project's scenes as picker choices (name + absolute file).
fn scene_choices(project: &Project) -> Vec<SceneChoice> {
    project
        .scenes()
        .map(|scenes| {
            scenes
                .into_iter()
                .map(|info| SceneChoice {
                    path: project.file_for(&info.path),
                    name: info.name,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A file-safe stem for a scene named `name` (used when a new scene is saved).
fn scene_file_stem(name: &str) -> String {
    let mut slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed.to_string()
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
    /// The directories the host was discovered from.
    pub dirs: Vec<PathBuf>,
    /// The ids the project disables.
    pub disabled: Vec<String>,
    /// The `--watch` reload inputs, when watching.
    pub reload: Option<ExtensionReload>,
}

/// Open `editor` in a window with egui panels. Blocks until the window closes.
pub fn run(
    editor: Editor,
    config: WindowConfig,
    scene_path: Option<PathBuf>,
    project: Option<Project>,
    extensions: Option<AttachedExtensions>,
) -> Result<(), PlatformError> {
    let driver = EditorDriver::new(editor)
        .with_project(project)
        .with_scene_path(scene_path);
    platform::run(attach(driver, extensions), config)
}

/// Open `editor` in a window with explicit camera-navigation bindings.
pub fn run_with_controls(
    editor: Editor,
    config: WindowConfig,
    controls: CameraControls,
    scene_path: Option<PathBuf>,
    project: Option<Project>,
    extensions: Option<AttachedExtensions>,
) -> Result<(), PlatformError> {
    let mut driver = EditorDriver::new(editor)
        .with_project(project)
        .with_scene_path(scene_path);
    *driver.camera_controls_mut() = controls;
    platform::run(attach(driver, extensions), config)
}

fn attach(driver: EditorDriver, extensions: Option<AttachedExtensions>) -> EditorDriver {
    match extensions {
        Some(AttachedExtensions {
            host,
            config,
            config_path,
            dirs,
            disabled,
            reload,
        }) => driver.with_extensions(host, config, config_path, dirs, disabled, reload),
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

    /// Every text run the frame painted, flattened out of the shape tree.
    fn frame_texts(output: &egui::FullOutput) -> Vec<String> {
        fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut out);
        }
        out
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

    /// Run one editor frame at a fixed screen size and return the painted text.
    fn one_frame(driver: &mut EditorDriver, ctx: &egui::Context) -> Vec<String> {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            ..Default::default()
        };
        let output = ctx.run_ui(raw, |ui| driver.ui(ui));
        let texts = frame_texts(&output);
        output.drop_without_applying_deltas();
        texts
    }

    #[test]
    fn the_console_and_extensions_screen_show_without_a_host() {
        // No extension was discovered, so there is no host; the console and the
        // Extensions screen are bottom-dock views and must still draw without one.
        let mut driver = EditorDriver::new(Editor::new());
        driver.console_log.push("sentinel-console-line".to_string());
        let ctx = egui::Context::default();

        // The console opens in the bottom dock, alone, so it is the active view.
        driver.layout.toggle(View::Console);
        let texts = one_frame(&mut driver, &ctx);
        assert!(
            texts
                .iter()
                .any(|text| text.contains("sentinel-console-line")),
            "the console drew without a host: {texts:?}"
        );

        // Toggling the Extensions screen on makes it the active tab of that dock.
        driver.layout.toggle(View::Extensions);
        let texts = one_frame(&mut driver, &ctx);
        assert!(
            texts
                .iter()
                .any(|text| text.contains("No extensions discovered.")),
            "the Extensions screen drew without a host: {texts:?}"
        );
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

    fn project_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("noctis-window-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join("noctis.toml"), "[project]\nname = \"T\"\n").unwrap();
        dir
    }

    #[test]
    fn save_writes_a_new_scene_into_the_project() {
        let dir = project_dir("save");
        let project = Project::open(&dir).expect("project");
        let mut editor = Editor::new();
        editor.spawn("Thing", None);
        let mut driver = EditorDriver::new(editor).with_project(Some(project));

        driver.save();

        let written = dir.join("scenes/untitled.scene.json");
        assert!(written.is_file(), "the scene was written to {written:?}");
        assert_eq!(driver.scene_path.as_deref(), Some(written.as_path()));
        assert!(
            driver
                .status
                .as_deref()
                .is_some_and(|s| s.starts_with("Saved")),
            "{:?}",
            driver.status
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_without_a_target_reports_it() {
        let mut driver = EditorDriver::new(Editor::new());
        driver.save();
        assert!(
            driver
                .status
                .as_deref()
                .is_some_and(|s| s.contains("Nothing to save to")),
            "{:?}",
            driver.status
        );
    }

    #[test]
    fn delete_selected_removes_the_entity_and_undoes() {
        let mut editor = Editor::new();
        let entity = editor.spawn("Thing", None);
        let mut driver = EditorDriver::new(editor);
        driver.select(entity);
        assert_eq!(driver.editor().list_entities().len(), 1);

        driver.delete_selected();
        assert!(driver.editor().list_entities().is_empty());
        assert!(driver.selected.is_none());

        assert!(driver.editor_mut().undo(), "delete is undoable");
        assert_eq!(driver.editor().list_entities().len(), 1);
    }

    #[test]
    fn switching_scenes_loads_the_chosen_file() {
        let dir = project_dir("switch");
        std::fs::create_dir_all(dir.join("scenes")).unwrap();

        let a = Editor::new();
        a.save(dir.join("scenes/a.scene.json")).unwrap();
        let mut b = Editor::new();
        b.spawn("FromB", None);
        b.save(dir.join("scenes/b.scene.json")).unwrap();

        let project = Project::open(&dir).expect("project");
        let mut driver = EditorDriver::new(Editor::new()).with_project(Some(project));
        assert_eq!(driver.scenes.len(), 2, "{:?}", driver.scenes);

        let index = |driver: &EditorDriver, file: &str| {
            driver
                .scenes
                .iter()
                .position(|choice| choice.path.ends_with(file))
                .unwrap_or_else(|| panic!("{file} is listed"))
        };

        let b = index(&driver, "b.scene.json");
        driver.switch_scene(b);
        assert!(
            driver.editor().find_by_name("FromB").is_some(),
            "the loaded scene has its entity"
        );

        let a = index(&driver, "a.scene.json");
        driver.switch_scene(a);
        assert!(driver.editor().find_by_name("FromB").is_none());
        assert_eq!(driver.editor().list_entities().len(), 0);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scene_file_stem_slugifies() {
        assert_eq!(scene_file_stem("Main Menu"), "main-menu");
        assert_eq!(scene_file_stem("Level 1!"), "level-1");
        assert_eq!(scene_file_stem("  "), "untitled");
    }

    fn extension_root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("noctis-window-ext-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn write_extension(root: &Path, dir_name: &str, id: &str) {
        let dir = root.join(dir_name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("extension.json"),
            format!(
                r#"{{ "id": "{id}", "name": "{id}", "version": "1.0.0", "api": 1, "entry": "extension.js" }}"#
            ),
        )
        .unwrap();
        std::fs::write(dir.join("extension.js"), "noctis.log('loaded');").unwrap();
    }

    #[test]
    fn rebuilding_extensions_picks_up_and_drops_them() {
        let root = extension_root("reload");
        let config = ExtensionConfig::default();
        let mut driver = EditorDriver::new(Editor::new());

        // A rebuild over an empty directory loads nothing.
        driver.rebuild_extensions(std::slice::from_ref(&root), &config, &[]);
        assert!(
            driver
                .extensions()
                .is_some_and(|host| host.ids().is_empty())
        );

        // Creating an extension is picked up by the next rebuild.
        write_extension(&root, "adder", "com.example.adder");
        driver.rebuild_extensions(std::slice::from_ref(&root), &config, &[]);
        assert!(
            driver
                .extensions()
                .is_some_and(|host| host.ids().iter().any(|id| id == "com.example.adder"))
        );

        // Disabling it keeps it out of the rebuilt host.
        let disabled = vec!["com.example.adder".to_string()];
        driver.rebuild_extensions(std::slice::from_ref(&root), &config, &disabled);
        assert!(
            driver
                .extensions()
                .is_some_and(|host| host.ids().is_empty())
        );

        // Removing the directory drops it too.
        driver.rebuild_extensions(std::slice::from_ref(&root), &config, &[]);
        std::fs::remove_dir_all(root.join("adder")).unwrap();
        driver.rebuild_extensions(std::slice::from_ref(&root), &config, &[]);
        assert!(
            driver
                .extensions()
                .is_some_and(|host| host.ids().is_empty())
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_disabled_extension_stays_listed_and_re_enables_live() {
        let root = extension_root("toggle");
        write_extension(&root, "adder", "com.example.adder");
        let dirs = vec![root.clone()];

        // A host with the extension loaded, as the CLI would build it.
        let loader = extensions::Loader::discover(&dirs);
        let mut host = ExtensionHost::new().expect("host");
        host.load(&loader, |_| true);

        let config_path = root.join("extensions.toml");
        let mut driver = EditorDriver::new(Editor::new()).with_extensions(
            host,
            ExtensionConfig::default(),
            config_path.clone(),
            dirs,
            Vec::new(),
            None,
        );

        let enabled = |driver: &EditorDriver| {
            driver
                .extensions()
                .expect("host")
                .entries()
                .iter()
                .find(|entry| entry.id == "com.example.adder")
                .expect("still listed")
                .enabled
        };

        // Disabling it keeps it listed (disabled) while the host drops it.
        driver.set_extension_enabled("com.example.adder", false);
        assert!(!enabled(&driver), "listed as disabled");
        assert!(
            driver
                .extensions()
                .is_some_and(|host| host.ids().is_empty()),
            "not loaded"
        );

        // The screen can turn it back on, and it applies immediately.
        driver.set_extension_enabled("com.example.adder", true);
        assert!(enabled(&driver), "re-enabled");
        assert!(
            driver
                .extensions()
                .is_some_and(|host| host.ids().iter().any(|id| id == "com.example.adder")),
            "loaded again"
        );
        assert!(config_path.is_file(), "the config was written");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn toggling_a_view_opens_and_closes_it_in_its_default_dock() {
        let mut layout = Layout::default();

        // On: the view joins its default dock and becomes the active tab.
        layout.toggle(View::Outliner);
        assert!(layout.is_open(&View::Outliner));
        assert_eq!(layout.left.open, vec![View::Outliner]);
        assert_eq!(layout.left.active, 0);

        // A second view in one dock sits beside the first, newest selected.
        layout.toggle(View::Console);
        layout.toggle(View::Extensions);
        assert_eq!(layout.bottom.open, vec![View::Console, View::Extensions]);
        assert_eq!(layout.bottom.active, 1, "the newest view is selected");

        // Off: removed, and the active index falls back into range.
        layout.toggle(View::Extensions);
        assert!(!layout.is_open(&View::Extensions));
        assert_eq!(layout.bottom.active, 0);
        layout.toggle(View::Console);
        assert!(layout.bottom.open.is_empty(), "the dock empties and hides");
    }

    #[test]
    fn the_default_layout_is_outliner_left_and_inspector_right() {
        let layout = Layout::editor_default();
        assert_eq!(layout.left.open, vec![View::Outliner]);
        assert_eq!(layout.right.open, vec![View::Inspector]);
        assert!(layout.bottom.open.is_empty());
        assert!(layout.is_open(&View::Outliner));
        assert!(layout.is_open(&View::Inspector));
        assert!(!layout.is_open(&View::Console));
    }

    #[test]
    fn the_status_bar_toggles_a_dock_view() {
        let mut editor = Editor::new();
        editor.spawn("StatusSentinel", None);
        let mut driver = EditorDriver::new(editor);
        let ctx = egui::Context::default();

        // The outliner is open by default, so its entities are painted.
        let texts = one_frame(&mut driver, &ctx);
        assert!(
            texts.iter().any(|text| text.contains("StatusSentinel")),
            "the outliner drew its entities: {texts:?}"
        );

        // Toggling the view off — what the status-bar button does — hides the dock.
        driver.layout.toggle(View::Outliner);
        let texts = one_frame(&mut driver, &ctx);
        assert!(
            !texts.iter().any(|text| text.contains("StatusSentinel")),
            "the outliner is gone once toggled off: {texts:?}"
        );
    }

    #[test]
    fn an_extension_panel_opens_in_a_dock() {
        // An extension that draws a panel becomes a bottom-dock view (P4).
        let root = extension_root("panel");
        let dir = root.join("panelly");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("extension.json"),
            r#"{ "id": "com.example.panelly", "name": "Panelly", "version": "1.0.0", "api": 1, "entry": "extension.js" }"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("extension.js"),
            r#"noctis.ui.panel("PanelTools", function (ui) { ui.label("panel-body-sentinel"); });"#,
        )
        .unwrap();

        let dirs = vec![root.clone()];
        let loader = extensions::Loader::discover(&dirs);
        let mut host = ExtensionHost::new().expect("host");
        host.load(&loader, |_| true);
        let mut driver = EditorDriver::new(Editor::new()).with_extensions(
            host,
            ExtensionConfig::default(),
            root.join("extensions.toml"),
            dirs,
            Vec::new(),
            None,
        );

        let ctx = egui::Context::default();
        let texts = one_frame(&mut driver, &ctx);
        assert!(
            texts
                .iter()
                .any(|text| text.contains("panel-body-sentinel")),
            "the extension panel drew in its dock: {texts:?}"
        );
        // It opened in the bottom dock as an `Extension` view.
        assert_eq!(driver.layout.bottom.open.len(), 1);
        assert!(matches!(
            driver.layout.bottom.open[0],
            View::Extension { .. }
        ));

        std::fs::remove_dir_all(&root).ok();
    }
}
