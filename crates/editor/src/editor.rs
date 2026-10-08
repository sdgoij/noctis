//! The [`Editor`]: a programmatic, AI-drivable authoring surface.
//!
//! An [`Editor`] owns a [`Scene`], an [`AssetRegistry`], a selection, a small
//! prefab library, and an undo/redo history. Every mutation is available both
//! as an imperative method (`spawn`, `set_translation`, `add_light`, ...) and
//! as a serializable [`EditorCommand`] applied through [`Editor::apply`]. Both
//! paths share one implementation and one history, so an agent using JSON
//! commands and a program using Rust methods see identical behaviour.
//!
//! # Undo/redo model
//!
//! History is snapshot-based: before a scene-mutating command runs, the scene is
//! serialized to a [`SceneDesc`] and pushed onto the undo stack. `undo` restores
//! that snapshot; `redo` re-applies the snapshot it displaced. Entity *ids* are
//! not stable across undo (a restored scene allocates fresh entities), so
//! callers should refer to entities by name after an undo — the editor
//! re-resolves the selection by name to soften this.
//!
//! Asset registrations and the prefab library live outside the snapshot, so
//! they are *not* undone. Scene edits that reference them are.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use ecs::assets::{AssetHandle, AssetRegistry, Material, Mesh, Texture};
use ecs::components::component_index::ComponentKind;
use ecs::components::{
    Active, AudioSource, Camera, Children, Collider, ColliderShape, Light, LightKind,
    LocalTransform, MeshRenderer, Name, Parent, Projection, RigidBody, Script, Visible,
};
use ecs::scene::{Prefab, Scene, SceneDesc, SceneMode, set_parent};
use ecs::world::{Entity, World};
use math::{Quat, Transform, Vec3, degrees, radians, vec3, zero3};

use crate::command::{
    AssetRef, AudioSourceParams, CameraParams, ColliderParams, CommandOutcome, ComponentName,
    EditorCommand, EntityRef, LightParams, MeshRendererParams, RigidBodyParams, arr3, arr4,
    material_from_params, mesh_from_positions, normalized_or, solid_texture, to_arr3, to_arr4,
};
use crate::error::EditorError;
use crate::info::{EntityDto, EntityInfo};

// ---------------------------------------------------------------------------
// Component bundle (used to copy entities for duplicate/prefab)
// ---------------------------------------------------------------------------

/// An owned copy of every (attachable) component on an entity.
///
/// Copying components out into an owned value lets us duplicate entities and
/// build prefabs without fighting the borrow checker over a single [`World`].
#[derive(Clone, Debug, Default)]
struct Bundle {
    transform: LocalTransform,
    name: Option<Name>,
    active: Option<Active>,
    visible: Option<Visible>,
    camera: Option<Camera>,
    light: Option<Light>,
    mesh_renderer: Option<MeshRenderer>,
    rigid_body: Option<RigidBody>,
    collider: Option<Collider>,
    audio: Option<AudioSource>,
    script: Option<Script>,
    children: Vec<Entity>,
}

impl Bundle {
    /// Read every component from `entity`, or `None` if it is not alive.
    fn extract(world: &World, entity: Entity) -> Option<Self> {
        if !world.is_alive(entity) {
            return None;
        }
        Some(Bundle {
            transform: world
                .get::<LocalTransform>(entity)
                .copied()
                .unwrap_or_default(),
            name: world.get::<Name>(entity).cloned(),
            active: world.get::<Active>(entity).copied(),
            visible: world.get::<Visible>(entity).copied(),
            camera: world.get::<Camera>(entity).copied(),
            light: world.get::<Light>(entity).copied(),
            mesh_renderer: world.get::<MeshRenderer>(entity).copied(),
            rigid_body: world.get::<RigidBody>(entity).copied(),
            collider: world.get::<Collider>(entity).copied(),
            audio: world.get::<AudioSource>(entity).copied(),
            script: world.get::<Script>(entity).cloned(),
            children: world
                .get::<Children>(entity)
                .map(|c| c.0.clone())
                .unwrap_or_default(),
        })
    }

    /// Write this bundle's components onto `destination`.
    fn apply_to(&self, world: &mut World, destination: Entity) {
        world.insert(destination, self.transform);
        if let Some(name) = &self.name {
            world.insert(destination, name.clone());
        }
        if let Some(active) = &self.active {
            world.insert(destination, *active);
        }
        if let Some(visible) = &self.visible {
            world.insert(destination, *visible);
        }
        if let Some(camera) = &self.camera {
            world.insert(destination, *camera);
        }
        if let Some(light) = &self.light {
            world.insert(destination, *light);
        }
        if let Some(mesh) = &self.mesh_renderer {
            world.insert(destination, *mesh);
        }
        if let Some(body) = &self.rigid_body {
            world.insert(destination, *body);
        }
        if let Some(collider) = &self.collider {
            world.insert(destination, *collider);
        }
        if let Some(audio) = &self.audio {
            world.insert(destination, *audio);
        }
        if let Some(script) = &self.script {
            world.insert(destination, script.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// Geometry helpers
// ---------------------------------------------------------------------------

fn sub3(a: Vec3, b: Vec3) -> Vec3 {
    vec3(a.x - b.x, a.y - b.y, a.z - b.z)
}

fn add3(a: Vec3, b: Vec3) -> Vec3 {
    vec3(a.x + b.x, a.y + b.y, a.z + b.z)
}

// ---------------------------------------------------------------------------
// Editor
// ---------------------------------------------------------------------------

/// The editor: a scene, its assets, a selection, and undo/redo history.
pub struct Editor {
    scene: Scene,
    assets: AssetRegistry,
    prefabs: HashMap<String, Prefab>,
    selection: Vec<Entity>,
    undo_stack: Vec<SceneDesc>,
    redo_stack: Vec<SceneDesc>,
    /// An open interactive edit (a gizmo drag): the snapshot taken when it began,
    /// and whether any command has changed the scene since. See
    /// [`Editor::begin_interactive_edit`].
    interactive: Option<(SceneDesc, bool)>,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Editor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Editor")
            .field("scene", &self.scene.name)
            .field("entities", &self.scene.world.entities().len())
            .field("assets", &self.assets.len())
            .field("prefabs", &self.prefabs.len())
            .field("undo", &self.undo_stack.len())
            .field("redo", &self.redo_stack.len())
            .finish()
    }
}

impl Editor {
    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    /// A new editor with an empty scene named `"Untitled"` and the engine's
    /// default placeholder assets (`$default` material, `$white`/`$normal`
    /// textures).
    pub fn new() -> Self {
        Self {
            scene: Scene::new("Untitled"),
            assets: AssetRegistry::with_builtins(),
            prefabs: HashMap::new(),
            selection: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            interactive: None,
        }
    }

    /// Load a scene from `path`, replacing the current scene and history.
    pub fn open(&mut self, path: impl AsRef<Path>) -> Result<(), EditorError> {
        let text = fs::read_to_string(path).map_err(|e| EditorError::Io(e.to_string()))?;
        self.load_json(&text)
    }

    /// Open `path` if it exists, otherwise start a new (empty) scene named after
    /// the file stem.
    ///
    /// This is what the CLI uses, so opening a scene that has not been created
    /// yet does something sensible instead of failing.
    pub fn open_or_new(&mut self, path: impl AsRef<Path>) -> Result<(), EditorError> {
        self.open_or_new_with_mode(path, SceneMode::default())
    }

    /// Like [`Editor::open_or_new`], but a newly created scene uses `mode`.
    pub fn open_or_new_with_mode(
        &mut self,
        path: impl AsRef<Path>,
        mode: SceneMode,
    ) -> Result<(), EditorError> {
        let path = path.as_ref();
        if path.exists() {
            self.open(path)
        } else {
            let name = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .filter(|stem| !stem.is_empty())
                .unwrap_or("Untitled");
            self.new_scene_with_mode(name, mode);
            Ok(())
        }
    }

    /// Load a scene from a JSON string, replacing the current scene and history.
    pub fn load_json(&mut self, json: &str) -> Result<(), EditorError> {
        let scene =
            Scene::from_json(json, &self.assets).map_err(|e| EditorError::Scene(e.to_string()))?;
        self.scene = scene;
        self.selection.clear();
        self.undo_stack.clear();
        self.redo_stack.clear();
        Ok(())
    }

    /// Save the scene (resolving asset handles to paths) to `path`.
    ///
    /// Missing parent directories are created, so `--out scenes/level.json`
    /// works without `scenes/` having to exist first.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), EditorError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|e| EditorError::Io(e.to_string()))?;
        }
        fs::write(path, self.to_json()).map_err(|e| EditorError::Io(e.to_string()))
    }

    /// The scene serialized to pretty JSON.
    pub fn to_json(&self) -> String {
        self.scene.to_json(&self.assets)
    }

    /// Build an editor from a JSON scene document, resolving assets against an
    /// empty default registry.
    pub fn from_json(json: &str) -> Result<Self, EditorError> {
        let mut editor = Editor::new();
        editor.load_json(json)?;
        Ok(editor)
    }

    /// Replace the scene with a fresh, empty one. Undoable.
    pub fn new_scene(&mut self, name: impl Into<String>) {
        self.new_scene_with_mode(name, SceneMode::default());
    }

    /// Replace the scene with a fresh, empty one in a given presentation mode.
    /// Undoable.
    pub fn new_scene_with_mode(&mut self, name: impl Into<String>, mode: SceneMode) {
        let before = self.snapshot();
        self.scene = Scene::with_mode(name, mode);
        self.selection.clear();
        self.push_history(before);
    }

    /// The scene's presentation mode (a hint; see [`SceneMode`]).
    pub fn mode(&self) -> SceneMode {
        self.scene.mode
    }

    /// Set the scene's presentation mode. Undoable.
    pub fn set_mode(&mut self, mode: SceneMode) {
        if self.scene.mode == mode {
            return;
        }
        let before = self.snapshot();
        self.scene.mode = mode;
        self.push_history(before);
    }

    /// The scene name.
    pub fn scene_name(&self) -> &str {
        &self.scene.name
    }

    /// Read-only access to the underlying scene.
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    /// Mutable access to the underlying scene.
    ///
    /// Mutations here bypass command history; prefer the typed methods. This is
    /// for host code (the windowed play mode ensuring a camera exists, for
    /// example).
    pub fn scene_mut(&mut self) -> &mut Scene {
        &mut self.scene
    }

    /// Read-only access to the asset registry.
    pub fn assets(&self) -> &AssetRegistry {
        &self.assets
    }

    /// Mutable access to the asset registry.
    ///
    /// Registering assets this way bypasses command history and the
    /// named-path convention, so prefer [`Editor::register_mesh`] and friends.
    pub fn assets_mut(&mut self) -> &mut AssetRegistry {
        &mut self.assets
    }

    /// Consume the editor, returning its scene and asset registry.
    ///
    /// This is how the editor hands a finished scene to the runtime's windowed
    /// play mode ([`crate`]-external code calls `runtime::run_in_window`).
    pub fn into_parts(self) -> (Scene, AssetRegistry) {
        (self.scene, self.assets)
    }

    // -----------------------------------------------------------------------
    // History
    // -----------------------------------------------------------------------

    fn snapshot(&self) -> SceneDesc {
        self.scene.to_desc(&self.assets)
    }

    fn push_history(&mut self, before: SceneDesc) {
        self.undo_stack.push(before);
        self.redo_stack.clear();
    }

    /// Record `before` as an undo step, or fold it into the open interactive edit
    /// so a multi-frame interaction (a gizmo drag) leaves one history entry.
    fn record_history(&mut self, before: SceneDesc) {
        match self.interactive.as_mut() {
            Some((_, changed)) => *changed = true,
            None => self.push_history(before),
        }
    }

    /// Begin an interactive, multi-frame edit such as a gizmo drag.
    ///
    /// While an interactive edit is open, every scene-mutating command is folded
    /// into a *single* undo step, recorded when [`Editor::end_interactive_edit`]
    /// is called. That makes a drag undo in one step rather than one step per
    /// frame. Interactive edits do not nest: opening one replaces any previous
    /// session, and every [`Editor::begin_interactive_edit`] should be paired
    /// with an [`Editor::end_interactive_edit`].
    pub fn begin_interactive_edit(&mut self) {
        self.interactive = Some((self.snapshot(), false));
    }

    /// Finish an interactive edit, recording one undo step if the scene changed
    /// since [`Editor::begin_interactive_edit`]. Does nothing if no edit is open.
    pub fn end_interactive_edit(&mut self) {
        if let Some((before, changed)) = self.interactive.take()
            && changed
        {
            self.push_history(before);
        }
    }

    /// Whether an interactive edit is currently open.
    pub fn in_interactive_edit(&self) -> bool {
        self.interactive.is_some()
    }

    /// Whether there is anything to undo.
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    /// Whether there is anything to redo.
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// The number of undoable steps.
    pub fn history_len(&self) -> usize {
        self.undo_stack.len()
    }

    /// Undo the most recent scene-mutating command. Returns `false` if the
    /// history is empty.
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo_stack.pop() else {
            return false;
        };
        let names = self.selection_names();
        let current = self.snapshot();
        self.redo_stack.push(current);
        self.scene = Scene::from_desc(&previous, &self.assets);
        self.restore_selection(names);
        true
    }

    /// Redo the most recently undone command. Returns `false` if there is
    /// nothing to redo.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo_stack.pop() else {
            return false;
        };
        let names = self.selection_names();
        let current = self.snapshot();
        self.undo_stack.push(current);
        self.scene = Scene::from_desc(&next, &self.assets);
        self.restore_selection(names);
        true
    }

    fn selection_names(&self) -> Vec<String> {
        self.selection
            .iter()
            .filter_map(|e| self.name_of(*e))
            .collect()
    }

    fn restore_selection(&mut self, names: Vec<String>) {
        let mut selection = Vec::new();
        for name in names {
            if let Some(entity) = self.find_by_name(&name) {
                selection.push(entity);
            }
        }
        self.selection = selection;
    }

    fn prune_selection(&mut self) {
        self.selection.retain(|e| self.scene.world.is_alive(*e));
    }

    // -----------------------------------------------------------------------
    // Command application
    // -----------------------------------------------------------------------

    /// Apply a command, recording history for scene-mutating commands.
    ///
    /// On failure the scene is rolled back to the pre-command snapshot so a
    /// partially applied command never leaves a broken scene behind.
    pub fn apply(&mut self, command: EditorCommand) -> Result<CommandOutcome, EditorError> {
        if !command.records_history() {
            return self.dispatch(&command);
        }
        let before = self.snapshot();
        match self.dispatch(&command) {
            Ok(outcome) => {
                self.prune_selection();
                self.record_history(before);
                Ok(outcome)
            }
            Err(err) => {
                let names = self.selection_names();
                self.scene = Scene::from_desc(&before, &self.assets);
                self.restore_selection(names);
                Err(err)
            }
        }
    }

    /// Apply a JSON array (or single object) of commands in order.
    pub fn apply_json(&mut self, json: &str) -> Result<Vec<CommandOutcome>, EditorError> {
        let commands = parse_commands(json)?;
        let mut outcomes = Vec::with_capacity(commands.len());
        for command in commands {
            outcomes.push(self.apply(command)?);
        }
        Ok(outcomes)
    }

    fn dispatch(&mut self, command: &EditorCommand) -> Result<CommandOutcome, EditorError> {
        match command {
            EditorCommand::NewScene { name } => {
                self.scene = Scene::new(name.clone());
                self.selection.clear();
                Ok(CommandOutcome::None)
            }
            EditorCommand::Spawn { name, parent } => {
                let parent = self.resolve_optional(parent)?;
                Ok(CommandOutcome::Entity(self.op_spawn(name.clone(), parent)))
            }
            EditorCommand::Destroy { entity } => {
                let entity = self.resolve(entity)?;
                self.op_destroy(entity);
                Ok(CommandOutcome::None)
            }
            EditorCommand::Duplicate { entity } => {
                let entity = self.resolve(entity)?;
                let parent = self.scene.world.get::<Parent>(entity).map(|p| p.0);
                Ok(CommandOutcome::Entity(self.copy_subtree(entity, parent)))
            }
            EditorCommand::Rename { entity, name } => {
                let entity = self.resolve(entity)?;
                self.scene.world.insert(entity, Name::new(name.clone()));
                Ok(CommandOutcome::None)
            }
            EditorCommand::SetActive { entity, active } => {
                let entity = self.resolve(entity)?;
                self.scene.world.insert(entity, Active(*active));
                Ok(CommandOutcome::None)
            }
            EditorCommand::SetVisible { entity, visible } => {
                let entity = self.resolve(entity)?;
                self.scene.world.insert(entity, Visible(*visible));
                Ok(CommandOutcome::None)
            }
            EditorCommand::SetParent { entity, parent } => {
                let entity = self.resolve(entity)?;
                let parent = self.resolve_optional(parent)?;
                set_parent(&mut self.scene.world, entity, parent);
                Ok(CommandOutcome::None)
            }
            EditorCommand::SetTranslation {
                entity,
                translation,
            } => {
                let entity = self.resolve(entity)?;
                self.transform_mut(entity)?.translation = arr3(*translation);
                Ok(CommandOutcome::None)
            }
            EditorCommand::Translate { entity, delta } => {
                let entity = self.resolve(entity)?;
                let transform = self.transform_mut(entity)?;
                transform.translation = add3(transform.translation, arr3(*delta));
                Ok(CommandOutcome::None)
            }
            EditorCommand::SetRotationEuler {
                entity,
                degrees: deg,
            } => {
                let entity = self.resolve(entity)?;
                self.transform_mut(entity)?.rotation =
                    Quat::from_euler_xyz(radians(deg[0]), radians(deg[1]), radians(deg[2]));
                Ok(CommandOutcome::None)
            }
            EditorCommand::SetRotationQuat { entity, quaternion } => {
                let entity = self.resolve(entity)?;
                self.transform_mut(entity)?.rotation = arr4(*quaternion);
                Ok(CommandOutcome::None)
            }
            EditorCommand::SetScale { entity, scale } => {
                let entity = self.resolve(entity)?;
                self.transform_mut(entity)?.scale = arr3(*scale);
                Ok(CommandOutcome::None)
            }
            EditorCommand::LookAt {
                entity,
                target,
                up: _,
            } => {
                let entity = self.resolve(entity)?;
                self.op_look_at(entity, arr3(*target))?;
                Ok(CommandOutcome::None)
            }
            EditorCommand::AddCamera { entity, params } => {
                let entity = self.resolve(entity)?;
                self.scene.world.insert(entity, camera_from(params));
                Ok(CommandOutcome::None)
            }
            EditorCommand::AddLight { entity, params } => {
                let entity = self.resolve(entity)?;
                self.scene.world.insert(entity, light_from(params));
                Ok(CommandOutcome::None)
            }
            EditorCommand::SetMeshRenderer { entity, params } => {
                let entity = self.resolve(entity)?;
                let mesh = self.resolve_mesh(&params.mesh)?;
                let material = self.resolve_material(&params.material)?;
                self.scene.world.insert(
                    entity,
                    MeshRenderer {
                        mesh,
                        material,
                        cast_shadows: params.cast_shadows,
                        receive_shadows: params.receive_shadows,
                    },
                );
                Ok(CommandOutcome::None)
            }
            EditorCommand::AddRigidBody { entity, params } => {
                let entity = self.resolve(entity)?;
                self.scene.world.insert(entity, rigid_body_from(params));
                Ok(CommandOutcome::None)
            }
            EditorCommand::AddCollider { entity, params } => {
                let entity = self.resolve(entity)?;
                self.scene.world.insert(entity, collider_from(params));
                Ok(CommandOutcome::None)
            }
            EditorCommand::AddAudioSource { entity, params } => {
                let entity = self.resolve(entity)?;
                let clip = self.resolve_audio(&params.clip)?;
                self.scene.world.insert(
                    entity,
                    AudioSource {
                        clip,
                        volume: params.volume,
                        pitch: params.pitch,
                        looped: params.looped,
                        playing: params.playing,
                        spatial: params.spatial,
                        max_distance: params.max_distance,
                        rolloff: params.rolloff,
                    },
                );
                Ok(CommandOutcome::None)
            }
            EditorCommand::AttachScriptSource { entity, source } => {
                let entity = self.resolve(entity)?;
                self.scene
                    .world
                    .insert(entity, Script::from_source(source.clone()));
                Ok(CommandOutcome::None)
            }
            EditorCommand::AttachScriptPath { entity, path } => {
                let entity = self.resolve(entity)?;
                self.scene
                    .world
                    .insert(entity, Script::from_path(path.clone()));
                Ok(CommandOutcome::None)
            }
            EditorCommand::RemoveComponent { entity, component } => {
                let entity = self.resolve(entity)?;
                self.op_remove_component(entity, component.kind());
                Ok(CommandOutcome::None)
            }
            EditorCommand::RegisterMesh {
                name,
                positions,
                indices,
            } => {
                let mesh = mesh_from_positions(name.clone(), positions, indices)?;
                let handle = self.assets.meshes.insert_named(name.clone(), mesh);
                Ok(CommandOutcome::Asset(handle.id()))
            }
            EditorCommand::RegisterMaterial { name, params } => {
                let material = material_from_params(name, params);
                let handle = self.assets.materials.insert_named(name.clone(), material);
                Ok(CommandOutcome::Asset(handle.id()))
            }
            EditorCommand::RegisterTexture { name, color } => {
                let texture = solid_texture(name, *color);
                let handle = self.assets.textures.insert_named(name.clone(), texture);
                Ok(CommandOutcome::Asset(handle.id()))
            }
            EditorCommand::ImportGltf { path, parent } => {
                let parent = self.resolve_optional(parent)?;
                let loaded =
                    render::load_gltf(path).map_err(|e| EditorError::Gltf(e.to_string()))?;
                Ok(CommandOutcome::Entities(
                    self.instantiate_loaded(loaded, parent),
                ))
            }
            EditorCommand::CreatePrefab { name, entities } => {
                let mut resolved = Vec::with_capacity(entities.len());
                for reference in entities {
                    resolved.push(self.resolve(reference)?);
                }
                let prefab = self.create_prefab(name.clone(), &resolved)?;
                self.prefabs.insert(name.clone(), prefab);
                Ok(CommandOutcome::Prefab(name.clone()))
            }
            EditorCommand::InstantiatePrefab { name, parent } => {
                let parent = self.resolve_optional(parent)?;
                let prefab = self
                    .prefabs
                    .get(name)
                    .cloned()
                    .ok_or_else(|| EditorError::Invalid(format!("no prefab named {name:?}")))?;
                let entities = self.instantiate_prefab_raw(&prefab, parent);
                Ok(CommandOutcome::Entities(entities))
            }
            EditorCommand::Select { entity } => {
                let entity = self.resolve(entity)?;
                if !self.selection.contains(&entity) {
                    self.selection.push(entity);
                }
                Ok(CommandOutcome::None)
            }
            EditorCommand::ClearSelection => {
                self.selection.clear();
                Ok(CommandOutcome::None)
            }
        }
    }

    // -----------------------------------------------------------------------
    // Entity ops
    // -----------------------------------------------------------------------

    /// Spawn an entity with a name and identity transform, optionally parented.
    ///
    /// A `parent` that is not alive is ignored (the entity is spawned at the
    /// root), so this method is infallible.
    pub fn spawn(&mut self, name: impl Into<String>, parent: Option<Entity>) -> Entity {
        let parent = parent.filter(|p| self.scene.world.is_alive(*p));
        let before = self.snapshot();
        let entity = self.op_spawn(name.into(), parent);
        self.push_history(before);
        entity
    }

    /// Destroy an entity and its descendants. Returns `false` if it was not
    /// alive.
    pub fn destroy(&mut self, entity: Entity) -> bool {
        if !self.scene.world.is_alive(entity) {
            return false;
        }
        let before = self.snapshot();
        self.op_destroy(entity);
        self.prune_selection();
        self.push_history(before);
        true
    }

    /// Duplicate an entity and its descendants, keeping their hierarchy.
    /// Returns the duplicated root, or `None` if `entity` is not alive.
    pub fn duplicate(&mut self, entity: Entity) -> Option<Entity> {
        if !self.scene.world.is_alive(entity) {
            return None;
        }
        let before = self.snapshot();
        let parent = self.scene.world.get::<Parent>(entity).map(|p| p.0);
        let copy = self.copy_subtree(entity, parent);
        self.push_history(before);
        Some(copy)
    }

    /// Rename an entity.
    pub fn rename(&mut self, entity: Entity, name: impl Into<String>) -> Result<(), EditorError> {
        self.apply(EditorCommand::Rename {
            entity: entity.into(),
            name: name.into(),
        })
        .map(|_| ())
    }

    /// Set the entity's active flag.
    pub fn set_active(&mut self, entity: Entity, active: bool) -> Result<(), EditorError> {
        self.apply(EditorCommand::SetActive {
            entity: entity.into(),
            active,
        })
        .map(|_| ())
    }

    /// Set the entity's visible flag.
    pub fn set_visible(&mut self, entity: Entity, visible: bool) -> Result<(), EditorError> {
        self.apply(EditorCommand::SetVisible {
            entity: entity.into(),
            visible,
        })
        .map(|_| ())
    }

    /// Reparent an entity, or unparent it with `None`.
    pub fn set_parent(
        &mut self,
        entity: Entity,
        parent: Option<Entity>,
    ) -> Result<(), EditorError> {
        self.apply(EditorCommand::SetParent {
            entity: entity.into(),
            parent: parent.map(EntityRef::from),
        })
        .map(|_| ())
    }

    // -----------------------------------------------------------------------
    // Transform ops
    // -----------------------------------------------------------------------

    /// Set an entity's local translation.
    pub fn set_translation(
        &mut self,
        entity: Entity,
        translation: Vec3,
    ) -> Result<(), EditorError> {
        self.apply(EditorCommand::SetTranslation {
            entity: entity.into(),
            translation: to_arr3(translation),
        })
        .map(|_| ())
    }

    /// Add a delta to an entity's local translation.
    pub fn translate(&mut self, entity: Entity, delta: Vec3) -> Result<(), EditorError> {
        self.apply(EditorCommand::Translate {
            entity: entity.into(),
            delta: to_arr3(delta),
        })
        .map(|_| ())
    }

    /// Set an entity's local rotation from XYZ Euler angles, in degrees.
    pub fn set_rotation_euler(&mut self, entity: Entity, degrees: Vec3) -> Result<(), EditorError> {
        self.apply(EditorCommand::SetRotationEuler {
            entity: entity.into(),
            degrees: to_arr3(degrees),
        })
        .map(|_| ())
    }

    /// Set an entity's local rotation from a quaternion.
    pub fn set_rotation_quat(&mut self, entity: Entity, rotation: Quat) -> Result<(), EditorError> {
        self.apply(EditorCommand::SetRotationQuat {
            entity: entity.into(),
            quaternion: to_arr4(rotation),
        })
        .map(|_| ())
    }

    /// Set an entity's local scale.
    pub fn set_scale(&mut self, entity: Entity, scale: Vec3) -> Result<(), EditorError> {
        self.apply(EditorCommand::SetScale {
            entity: entity.into(),
            scale: to_arr3(scale),
        })
        .map(|_| ())
    }

    /// Orient an entity so its `-Z` axis points at `target`. `up` defaults to
    /// `+Y`.
    pub fn look_at(
        &mut self,
        entity: Entity,
        target: Vec3,
        up: Option<Vec3>,
    ) -> Result<(), EditorError> {
        self.apply(EditorCommand::LookAt {
            entity: entity.into(),
            target: to_arr3(target),
            up: up.map(to_arr3),
        })
        .map(|_| ())
    }

    /// An entity's local translation, if alive and transformed.
    pub fn translation(&self, entity: Entity) -> Option<Vec3> {
        self.scene
            .world
            .get::<LocalTransform>(entity)
            .map(|t| t.translation)
    }

    /// An entity's local rotation, if alive and transformed.
    pub fn rotation(&self, entity: Entity) -> Option<Quat> {
        self.scene
            .world
            .get::<LocalTransform>(entity)
            .map(|t| t.rotation)
    }

    /// An entity's local rotation as XYZ Euler angles, in degrees.
    pub fn rotation_euler_degrees(&self, entity: Entity) -> Option<Vec3> {
        self.scene.world.get::<LocalTransform>(entity).map(|t| {
            let (roll, pitch, yaw) = t.rotation.to_euler_xyz();
            vec3(degrees(roll), degrees(pitch), degrees(yaw))
        })
    }

    /// An entity's local scale, if alive and transformed.
    pub fn scale(&self, entity: Entity) -> Option<Vec3> {
        self.scene
            .world
            .get::<LocalTransform>(entity)
            .map(|t| t.scale)
    }

    /// A copy of an entity's local transform.
    pub fn local_transform(&self, entity: Entity) -> Option<Transform> {
        self.scene.world.get::<LocalTransform>(entity).copied()
    }

    // -----------------------------------------------------------------------
    // Component ops
    // -----------------------------------------------------------------------

    /// Add or replace a camera component.
    pub fn add_camera(&mut self, entity: Entity, params: CameraParams) -> Result<(), EditorError> {
        self.apply(EditorCommand::AddCamera {
            entity: entity.into(),
            params,
        })
        .map(|_| ())
    }

    /// Add or replace a light component.
    pub fn add_light(&mut self, entity: Entity, params: LightParams) -> Result<(), EditorError> {
        self.apply(EditorCommand::AddLight {
            entity: entity.into(),
            params,
        })
        .map(|_| ())
    }

    /// Add or replace a mesh renderer component.
    pub fn set_mesh_renderer(
        &mut self,
        entity: Entity,
        params: MeshRendererParams,
    ) -> Result<(), EditorError> {
        self.apply(EditorCommand::SetMeshRenderer {
            entity: entity.into(),
            params,
        })
        .map(|_| ())
    }

    /// Add or replace a rigid body component.
    pub fn add_rigid_body(
        &mut self,
        entity: Entity,
        params: RigidBodyParams,
    ) -> Result<(), EditorError> {
        self.apply(EditorCommand::AddRigidBody {
            entity: entity.into(),
            params,
        })
        .map(|_| ())
    }

    /// Add or replace a collider component.
    pub fn add_collider(
        &mut self,
        entity: Entity,
        params: ColliderParams,
    ) -> Result<(), EditorError> {
        self.apply(EditorCommand::AddCollider {
            entity: entity.into(),
            params,
        })
        .map(|_| ())
    }

    /// Add or replace an audio source component.
    pub fn add_audio_source(
        &mut self,
        entity: Entity,
        params: AudioSourceParams,
    ) -> Result<(), EditorError> {
        self.apply(EditorCommand::AddAudioSource {
            entity: entity.into(),
            params,
        })
        .map(|_| ())
    }

    /// Attach a script component from inline source.
    pub fn attach_script_source(
        &mut self,
        entity: Entity,
        source: impl Into<String>,
    ) -> Result<(), EditorError> {
        self.apply(EditorCommand::AttachScriptSource {
            entity: entity.into(),
            source: source.into(),
        })
        .map(|_| ())
    }

    /// Attach a script component from a script asset path.
    pub fn attach_script_path(
        &mut self,
        entity: Entity,
        path: impl Into<String>,
    ) -> Result<(), EditorError> {
        self.apply(EditorCommand::AttachScriptPath {
            entity: entity.into(),
            path: path.into(),
        })
        .map(|_| ())
    }

    /// Remove a component. Returns `Ok(())` even if the entity never had it.
    pub fn remove_component(
        &mut self,
        entity: Entity,
        component: ComponentName,
    ) -> Result<(), EditorError> {
        self.apply(EditorCommand::RemoveComponent {
            entity: entity.into(),
            component,
        })
        .map(|_| ())
    }

    /// Whether an entity carries a component (by canonical name).
    pub fn has_component(&self, entity: Entity, component: ComponentName) -> bool {
        match component.kind() {
            ComponentKind::LocalTransform => self.scene.world.has::<LocalTransform>(entity),
            ComponentKind::GlobalTransform => self
                .scene
                .world
                .has::<ecs::components::GlobalTransform>(entity),
            ComponentKind::Name => self.scene.world.has::<Name>(entity),
            ComponentKind::Active => self.scene.world.has::<Active>(entity),
            ComponentKind::Visible => self.scene.world.has::<Visible>(entity),
            ComponentKind::Camera => self.scene.world.has::<Camera>(entity),
            ComponentKind::Light => self.scene.world.has::<Light>(entity),
            ComponentKind::MeshRenderer => self.scene.world.has::<MeshRenderer>(entity),
            ComponentKind::RigidBody => self.scene.world.has::<RigidBody>(entity),
            ComponentKind::Collider => self.scene.world.has::<Collider>(entity),
            ComponentKind::AudioSource => self.scene.world.has::<AudioSource>(entity),
            ComponentKind::Script => self.scene.world.has::<Script>(entity),
        }
    }

    /// Borrow a component of an entity, if it is alive and carries it.
    pub fn component<C: Send + Sync + 'static>(&self, entity: Entity) -> Option<&C> {
        self.scene.world.get::<C>(entity)
    }

    /// Insert (or replace) a component, recording one undo step.
    ///
    /// Returns `false` (recording nothing) if the entity is not alive.
    pub fn insert_component<C: Send + Sync + 'static>(
        &mut self,
        entity: Entity,
        component: C,
    ) -> bool {
        if !self.scene.world.is_alive(entity) {
            return false;
        }
        let before = self.snapshot();
        self.scene.world.insert(entity, component);
        self.record_history(before);
        true
    }

    /// Edit a component in place, recording one undo step.
    ///
    /// Returns `false` (recording nothing) if the entity does not carry the
    /// component. In-place editing preserves fields the command path would reset
    /// (a light's `shadow_bias`, a collider's `rotation`, a body's damping).
    pub fn edit_component<C: Send + Sync + 'static>(
        &mut self,
        entity: Entity,
        edit: impl FnOnce(&mut C),
    ) -> bool {
        let before = self.snapshot();
        match self.scene.world.get_mut::<C>(entity) {
            Some(component) => {
                edit(component);
                self.record_history(before);
                true
            }
            None => false,
        }
    }

    // -----------------------------------------------------------------------
    // Asset ops
    // -----------------------------------------------------------------------

    /// Register a mesh under `name` (also its lookup path).
    pub fn register_mesh(&mut self, name: impl Into<String>, mesh: Mesh) -> AssetHandle<Mesh> {
        self.assets.meshes.insert_named(name.into(), mesh)
    }

    /// Register a material under `name` (also its lookup path).
    pub fn register_material(
        &mut self,
        name: impl Into<String>,
        material: Material,
    ) -> AssetHandle<Material> {
        self.assets.materials.insert_named(name.into(), material)
    }

    /// Register a texture under `name` (also its lookup path).
    pub fn register_texture(
        &mut self,
        name: impl Into<String>,
        texture: Texture,
    ) -> AssetHandle<Texture> {
        self.assets.textures.insert_named(name.into(), texture)
    }

    /// Import a glTF document from a file and instantiate its hierarchy.
    pub fn import_gltf(&mut self, path: impl AsRef<Path>) -> Result<Vec<Entity>, EditorError> {
        let loaded = render::load_gltf(path).map_err(|e| EditorError::Gltf(e.to_string()))?;
        let before = self.snapshot();
        let entities = self.instantiate_loaded(loaded, None);
        self.push_history(before);
        Ok(entities)
    }

    /// Import a glTF document from an in-memory `.gltf`/`.glb` buffer and
    /// instantiate its hierarchy.
    pub fn import_gltf_bytes(&mut self, bytes: &[u8]) -> Result<Vec<Entity>, EditorError> {
        let loaded =
            render::load_gltf_bytes(bytes).map_err(|e| EditorError::Gltf(e.to_string()))?;
        let before = self.snapshot();
        let entities = self.instantiate_loaded(loaded, None);
        self.push_history(before);
        Ok(entities)
    }

    /// Load an image file and register it as a texture asset under its path.
    ///
    /// `srgb` selects the texture's color format: `true` for color maps (albedo,
    /// emissive), `false` for data maps (normal, metallic/roughness, occlusion).
    pub fn import_texture(
        &mut self,
        path: impl AsRef<Path>,
        srgb: bool,
    ) -> Result<AssetHandle<Texture>, EditorError> {
        let path = path.as_ref();
        let texture =
            render::load_texture(path, srgb).map_err(|e| EditorError::Image(e.to_string()))?;
        Ok(self.register_texture(path.to_string_lossy().into_owned(), texture))
    }

    /// Register a glTF's assets and instantiate its nodes, parenting the roots
    /// to `parent` when given. Shared by the command and method paths.
    fn instantiate_loaded(
        &mut self,
        loaded: render::LoadedGltf,
        parent: Option<Entity>,
    ) -> Vec<Entity> {
        let roots = loaded.roots.clone();
        let handles = loaded.register(&mut self.assets);
        let nodes = loaded.instantiate(&mut self.scene.world, &handles);
        if let Some(parent) = parent {
            for &root in &roots {
                if let Some(&node) = nodes.get(root) {
                    set_parent(&mut self.scene.world, node, Some(parent));
                }
            }
        }
        self.scene.update_transforms();
        nodes
    }

    /// Build a prefab from `entities` (their components and internal hierarchy).
    pub fn create_prefab(
        &self,
        name: impl Into<String>,
        entities: &[Entity],
    ) -> Result<Prefab, EditorError> {
        let name = name.into();
        let mut temp = Scene::new(name.clone());
        let mut map: HashMap<Entity, Entity> = HashMap::new();

        for &source in entities {
            let Some(bundle) = Bundle::extract(&self.scene.world, source) else {
                return Err(EditorError::NoSuchEntity(EntityRef::from(source)));
            };
            let destination = temp.world.spawn();
            map.insert(source, destination);
            bundle.apply_to(&mut temp.world, destination);
        }
        for &source in entities {
            let parent = self.scene.world.get::<Parent>(source).map(|p| p.0);
            if let Some(parent) = parent
                && let Some(&destination_parent) = map.get(&parent)
            {
                set_parent(&mut temp.world, map[&source], Some(destination_parent));
            }
        }

        Ok(Prefab::from_scene_desc(name, temp.to_desc(&self.assets)))
    }

    /// Instantiate a prefab into the scene under an optional parent.
    pub fn instantiate_prefab(
        &mut self,
        prefab: &Prefab,
        parent: Option<Entity>,
    ) -> Result<Vec<Entity>, EditorError> {
        let before = self.snapshot();
        let entities = self.instantiate_prefab_raw(prefab, parent);
        self.push_history(before);
        Ok(entities)
    }

    /// Instantiate a prefab without touching history (used by the command path,
    /// which records history itself).
    fn instantiate_prefab_raw(&mut self, prefab: &Prefab, parent: Option<Entity>) -> Vec<Entity> {
        let entities = prefab.instantiate(&mut self.scene.world, &self.assets, parent);
        self.scene.update_transforms();
        entities
    }

    /// Build a prefab from entities and store it under `name` in the editor's
    /// prefab library.
    pub fn store_prefab(
        &mut self,
        name: impl Into<String>,
        entities: &[Entity],
    ) -> Result<String, EditorError> {
        let name = name.into();
        let prefab = self.create_prefab(name.clone(), entities)?;
        self.prefabs.insert(name.clone(), prefab);
        Ok(name)
    }

    /// Instantiate a prefab from the editor's library by name.
    pub fn instantiate_stored_prefab(
        &mut self,
        name: &str,
        parent: Option<Entity>,
    ) -> Result<Vec<Entity>, EditorError> {
        let prefab = self
            .prefabs
            .get(name)
            .cloned()
            .ok_or_else(|| EditorError::Invalid(format!("no prefab named {name:?}")))?;
        self.instantiate_prefab(&prefab, parent)
    }

    /// The prefab stored under `name`, if any.
    pub fn prefab(&self, name: &str) -> Option<&Prefab> {
        self.prefabs.get(name)
    }

    // -----------------------------------------------------------------------
    // Introspection
    // -----------------------------------------------------------------------

    /// A flat description of every entity in the scene, sorted by entity id.
    pub fn list_entities(&self) -> Vec<EntityInfo> {
        let mut out: Vec<EntityInfo> = self
            .scene
            .world
            .entities()
            .iter()
            .map(|entity| self.entity_info(entity))
            .collect();
        out.sort_by_key(|info| info.entity);
        out
    }

    /// A JSON description of one entity, or `None` if it is not alive.
    pub fn inspect(&self, entity: Entity) -> Option<String> {
        if !self.scene.world.is_alive(entity) {
            return None;
        }
        let value = self.inspect_value(entity);
        serde_json::to_string_pretty(&value).ok()
    }

    /// The current selection.
    pub fn selection(&self) -> &[Entity] {
        &self.selection
    }

    /// Add an entity to the selection.
    pub fn select(&mut self, entity: Entity) -> Result<(), EditorError> {
        self.apply(EditorCommand::Select {
            entity: entity.into(),
        })
        .map(|_| ())
    }

    /// Clear the selection.
    pub fn clear_selection(&mut self) {
        self.selection.clear();
    }

    /// The first entity with the given name.
    pub fn find_by_name(&self, name: &str) -> Option<Entity> {
        self.scene
            .world
            .entities()
            .iter()
            .find(|e| self.name_of(*e).as_deref() == Some(name))
    }

    /// The number of entities in the scene.
    pub fn entity_count(&self) -> usize {
        self.scene.world.entities().len()
    }

    /// An entity's name, if it has one.
    pub fn name_of(&self, entity: Entity) -> Option<String> {
        self.scene.world.get::<Name>(entity).map(|n| n.0.clone())
    }

    /// An entity's parent, if any.
    pub fn parent_of(&self, entity: Entity) -> Option<Entity> {
        self.scene.world.get::<Parent>(entity).map(|p| p.0)
    }

    /// Whether an entity is active (defaults to `true` when unset).
    pub fn is_active(&self, entity: Entity) -> bool {
        self.scene.world.get::<Active>(entity).is_none_or(|a| a.0)
    }

    /// Whether an entity is visible (defaults to `true` when unset).
    pub fn is_visible(&self, entity: Entity) -> bool {
        self.scene.world.get::<Visible>(entity).is_none_or(|v| v.0)
    }

    /// The canonical names of the attachable components on an entity.
    pub fn components_of(&self, entity: Entity) -> Vec<&'static str> {
        let world = &self.scene.world;
        let mut out = Vec::new();
        if world.has::<Camera>(entity) {
            out.push("Camera");
        }
        if world.has::<Light>(entity) {
            out.push("Light");
        }
        if world.has::<MeshRenderer>(entity) {
            out.push("MeshRenderer");
        }
        if world.has::<RigidBody>(entity) {
            out.push("RigidBody");
        }
        if world.has::<Collider>(entity) {
            out.push("Collider");
        }
        if world.has::<AudioSource>(entity) {
            out.push("AudioSource");
        }
        if world.has::<Script>(entity) {
            out.push("Script");
        }
        out
    }

    fn entity_info(&self, entity: Entity) -> EntityInfo {
        EntityInfo {
            entity,
            name: self.name_of(entity).unwrap_or_default(),
            parent: self.parent_of(entity),
            components: self.components_of(entity),
            translation: self
                .scene
                .world
                .get::<LocalTransform>(entity)
                .map(|t| t.translation)
                .unwrap_or_else(zero3),
        }
    }

    fn inspect_value(&self, entity: Entity) -> serde_json::Value {
        use serde_json::json;

        let world = &self.scene.world;
        let transform = world
            .get::<LocalTransform>(entity)
            .copied()
            .unwrap_or_default();

        let mut components = serde_json::Map::new();
        if let Some(camera) = world.get::<Camera>(entity) {
            components.insert(
                "Camera".into(),
                json!({
                    "projection": projection_json(&camera.projection),
                    "is_active": camera.is_active,
                    "priority": camera.priority,
                    "clear_color": camera.clear_color,
                    "hdr": camera.hdr,
                }),
            );
        }
        if let Some(light) = world.get::<Light>(entity) {
            components.insert(
                "Light".into(),
                json!({
                    "kind": light_kind_json(&light.kind),
                    "color": to_arr3(light.color),
                    "intensity": light.intensity,
                    "cast_shadows": light.cast_shadows,
                }),
            );
        }
        if let Some(mesh) = world.get::<MeshRenderer>(entity) {
            components.insert(
                "MeshRenderer".into(),
                json!({
                    "mesh": mesh.mesh.id().0,
                    "material": mesh.material.id().0,
                    "cast_shadows": mesh.cast_shadows,
                    "receive_shadows": mesh.receive_shadows,
                }),
            );
        }
        if let Some(body) = world.get::<RigidBody>(entity) {
            components.insert(
                "RigidBody".into(),
                json!({
                    "body_type": format!("{:?}", body.body_type).to_lowercase(),
                    "mass": body.mass,
                    "restitution": body.restitution,
                    "friction": body.friction,
                    "gravity_scale": body.gravity_scale,
                }),
            );
        }
        if let Some(collider) = world.get::<Collider>(entity) {
            components.insert(
                "Collider".into(),
                json!({
                    "shape": collider_shape_json(&collider.shape),
                    "offset": to_arr3(collider.offset),
                    "is_sensor": collider.is_sensor,
                }),
            );
        }
        if let Some(audio) = world.get::<AudioSource>(entity) {
            components.insert(
                "AudioSource".into(),
                json!({
                    "clip": audio.clip.id().0,
                    "volume": audio.volume,
                    "pitch": audio.pitch,
                    "looped": audio.looped,
                    "playing": audio.playing,
                    "spatial": audio.spatial,
                }),
            );
        }
        if let Some(script) = world.get::<Script>(entity) {
            components.insert(
                "Script".into(),
                json!({
                    "source": script.source,
                    "path": script.path,
                    "enabled": script.enabled,
                }),
            );
        }

        json!({
            "entity": EntityDto::new(entity),
            "name": self.name_of(entity),
            "parent": self.parent_of(entity).map(EntityDto::new),
            "active": self.is_active(entity),
            "visible": self.is_visible(entity),
            "transform": {
                "translation": to_arr3(transform.translation),
                "rotation": to_arr4(transform.rotation),
                "scale": to_arr3(transform.scale),
            },
            "components": components,
        })
    }

    // -----------------------------------------------------------------------
    // Internal op helpers (no history; history is managed by the callers)
    // -----------------------------------------------------------------------

    fn op_spawn(&mut self, name: String, parent: Option<Entity>) -> Entity {
        let entity = self.scene.world.spawn();
        self.scene.world.insert(entity, Name::new(name));
        self.scene.world.insert(entity, LocalTransform::identity());
        if let Some(parent) = parent {
            set_parent(&mut self.scene.world, entity, Some(parent));
        }
        entity
    }

    fn op_destroy(&mut self, entity: Entity) {
        let children = self
            .scene
            .world
            .get::<Children>(entity)
            .map(|c| c.0.clone())
            .unwrap_or_default();
        for child in children {
            self.op_destroy(child);
        }
        self.scene.world.despawn(entity);
    }

    fn copy_subtree(&mut self, source: Entity, parent: Option<Entity>) -> Entity {
        let Some(bundle) = Bundle::extract(&self.scene.world, source) else {
            // `source` vanished mid-copy; spawn a placeholder so the caller still
            // gets a live entity.
            return self.op_spawn("Copy".into(), parent);
        };
        let destination = self.scene.world.spawn();
        bundle.apply_to(&mut self.scene.world, destination);
        if let Some(parent) = parent {
            set_parent(&mut self.scene.world, destination, Some(parent));
        }
        for child in bundle.children {
            if self.scene.world.is_alive(child) {
                self.copy_subtree(child, Some(destination));
            }
        }
        destination
    }

    /// Orient `entity` so its `-Z` axis points from its position toward
    /// `target`, with `+Y` kept up.
    ///
    /// The convention matches the engine's forward axis (`-Z`). Because the
    /// framing is always kept upright, only yaw and pitch are solved for; roll
    /// is zero. (The `up` field of [`EditorCommand::LookAt`] is accepted for
    /// forward compatibility but currently ignored.)
    fn op_look_at(&mut self, entity: Entity, target: Vec3) -> Result<(), EditorError> {
        let position = self
            .scene
            .world
            .get::<LocalTransform>(entity)
            .map(|t| t.translation)
            .ok_or_else(|| EditorError::MissingComponent {
                entity: EntityRef::from(entity),
                component: "LocalTransform",
            })?;
        let forward = normalized_or(sub3(target, position), vec3(0.0, 0.0, -1.0));
        let pitch = forward.y.clamp(-1.0, 1.0).asin();
        let yaw = (-forward.x).atan2(-forward.z);
        let rotation = Quat::from_rotation_y(yaw).mul(Quat::from_rotation_x(pitch));
        if let Some(transform) = self.scene.world.get_mut::<LocalTransform>(entity) {
            transform.rotation = rotation;
        }
        Ok(())
    }

    fn op_remove_component(&mut self, entity: Entity, kind: ComponentKind) {
        let world = &mut self.scene.world;
        match kind {
            ComponentKind::LocalTransform => {
                world.remove::<LocalTransform>(entity);
            }
            ComponentKind::GlobalTransform => {
                world.remove::<ecs::components::GlobalTransform>(entity);
            }
            ComponentKind::Name => {
                world.remove::<Name>(entity);
            }
            ComponentKind::Active => {
                world.remove::<Active>(entity);
            }
            ComponentKind::Visible => {
                world.remove::<Visible>(entity);
            }
            ComponentKind::Camera => {
                world.remove::<Camera>(entity);
            }
            ComponentKind::Light => {
                world.remove::<Light>(entity);
            }
            ComponentKind::MeshRenderer => {
                world.remove::<MeshRenderer>(entity);
            }
            ComponentKind::RigidBody => {
                world.remove::<RigidBody>(entity);
            }
            ComponentKind::Collider => {
                world.remove::<Collider>(entity);
            }
            ComponentKind::AudioSource => {
                world.remove::<AudioSource>(entity);
            }
            ComponentKind::Script => {
                world.remove::<Script>(entity);
            }
        }
    }

    fn transform_mut(&mut self, entity: Entity) -> Result<&mut LocalTransform, EditorError> {
        self.scene
            .world
            .get_mut::<LocalTransform>(entity)
            .ok_or_else(|| EditorError::MissingComponent {
                entity: EntityRef::from(entity),
                component: "LocalTransform",
            })
    }

    fn resolve(&self, reference: &EntityRef) -> Result<Entity, EditorError> {
        match reference {
            EntityRef::Id { index, generation } => {
                let entity = Entity::from_raw(*index, *generation);
                if self.scene.world.is_alive(entity) {
                    Ok(entity)
                } else {
                    Err(EditorError::NoSuchEntity(reference.clone()))
                }
            }
            EntityRef::Name { name } => self
                .find_by_name(name)
                .ok_or_else(|| EditorError::UnknownName(name.clone())),
        }
    }

    fn resolve_optional(
        &self,
        reference: &Option<EntityRef>,
    ) -> Result<Option<Entity>, EditorError> {
        match reference {
            Some(reference) => Ok(Some(self.resolve(reference)?)),
            None => Ok(None),
        }
    }

    fn resolve_mesh(&self, reference: &Option<AssetRef>) -> Result<AssetHandle<Mesh>, EditorError> {
        match reference {
            None => Ok(AssetHandle::NONE),
            Some(AssetRef::Path { path }) => self
                .assets
                .meshes
                .handle(path)
                .ok_or_else(|| EditorError::MissingAsset(path.clone())),
            Some(AssetRef::Id { id }) => Ok(AssetHandle::from_raw(*id)),
        }
    }

    fn resolve_material(
        &self,
        reference: &Option<AssetRef>,
    ) -> Result<AssetHandle<Material>, EditorError> {
        match reference {
            None => Ok(AssetHandle::NONE),
            Some(AssetRef::Path { path }) => self
                .assets
                .materials
                .handle(path)
                .ok_or_else(|| EditorError::MissingAsset(path.clone())),
            Some(AssetRef::Id { id }) => Ok(AssetHandle::from_raw(*id)),
        }
    }

    fn resolve_audio(
        &self,
        reference: &Option<AssetRef>,
    ) -> Result<AssetHandle<ecs::assets::AudioClip>, EditorError> {
        match reference {
            None => Ok(AssetHandle::NONE),
            Some(AssetRef::Path { path }) => self
                .assets
                .audio
                .handle(path)
                .ok_or_else(|| EditorError::MissingAsset(path.clone())),
            Some(AssetRef::Id { id }) => Ok(AssetHandle::from_raw(*id)),
        }
    }
}

// ---------------------------------------------------------------------------
// Component constructors shared by the method and command paths
// ---------------------------------------------------------------------------

fn camera_from(params: &CameraParams) -> Camera {
    Camera {
        projection: params.projection.into(),
        is_active: params.is_active,
        priority: params.priority,
        clear_color: params.clear_color,
        hdr: params.hdr,
    }
}

fn light_from(params: &LightParams) -> Light {
    Light {
        kind: params.kind.into(),
        color: arr3(params.color),
        intensity: params.intensity,
        cast_shadows: params.cast_shadows,
        shadow_bias: Light::default().shadow_bias,
    }
}

fn rigid_body_from(params: &RigidBodyParams) -> RigidBody {
    RigidBody {
        body_type: params.body_type.into(),
        mass: params.mass,
        restitution: params.restitution,
        friction: params.friction,
        gravity_scale: params.gravity_scale,
        linear_velocity: arr3(params.linear_velocity),
        angular_velocity: arr3(params.angular_velocity),
        ..RigidBody::default()
    }
}

fn collider_from(params: &ColliderParams) -> Collider {
    Collider {
        shape: params.shape.into(),
        offset: arr3(params.offset),
        is_sensor: params.is_sensor,
        friction: params.friction,
        restitution: params.restitution,
        ..Collider::default()
    }
}

// ---------------------------------------------------------------------------
// JSON helpers for `inspect`
// ---------------------------------------------------------------------------

fn projection_json(projection: &Projection) -> serde_json::Value {
    match projection {
        Projection::Perspective { fov_y, near, far } => serde_json::json!({
            "type": "perspective",
            "fov_y": fov_y,
            "fov_y_deg": degrees(*fov_y),
            "near": near,
            "far": far,
        }),
        Projection::Orthographic { height, near, far } => serde_json::json!({
            "type": "orthographic",
            "height": height,
            "near": near,
            "far": far,
        }),
    }
}

fn light_kind_json(kind: &LightKind) -> serde_json::Value {
    match kind {
        LightKind::Directional { direction } => serde_json::json!({
            "type": "directional",
            "direction": to_arr3(*direction),
        }),
        LightKind::Point { range } => serde_json::json!({
            "type": "point",
            "range": range,
        }),
        LightKind::Spot {
            direction,
            inner_angle,
            outer_angle,
            range,
        } => serde_json::json!({
            "type": "spot",
            "direction": to_arr3(*direction),
            "inner_angle_deg": degrees(*inner_angle),
            "outer_angle_deg": degrees(*outer_angle),
            "range": range,
        }),
    }
}

fn collider_shape_json(shape: &ColliderShape) -> serde_json::Value {
    match shape {
        ColliderShape::Cuboid { half_extents } => serde_json::json!({
            "type": "cuboid",
            "half_extents": to_arr3(*half_extents),
        }),
        ColliderShape::Ball { radius } => serde_json::json!({
            "type": "ball",
            "radius": radius,
        }),
        ColliderShape::Capsule {
            half_height,
            radius,
        } => serde_json::json!({
            "type": "capsule",
            "half_height": half_height,
            "radius": radius,
        }),
        ColliderShape::Cylinder {
            half_height,
            radius,
        } => serde_json::json!({
            "type": "cylinder",
            "half_height": half_height,
            "radius": radius,
        }),
    }
}

/// Parse a JSON array (or single object) of [`EditorCommand`]s.
pub fn parse_commands(json: &str) -> Result<Vec<EditorCommand>, EditorError> {
    let trimmed = json.trim_start();
    if trimmed.starts_with('[') {
        serde_json::from_str(json).map_err(|e| EditorError::Json(e.to_string()))
    } else {
        let single: EditorCommand =
            serde_json::from_str(json).map_err(|e| EditorError::Json(e.to_string()))?;
        Ok(vec![single])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{
        BodyTypeSpec, ColliderShapeSpec, LightSpec, MaterialParams, ProjectionSpec,
    };

    fn editor() -> Editor {
        Editor::new()
    }

    #[test]
    fn spawn_rename_and_parent() {
        let mut ed = editor();
        let root = ed.spawn("Root", None);
        let child = ed.spawn("Child", Some(root));

        assert_eq!(ed.find_by_name("Root"), Some(root));
        assert_eq!(ed.find_by_name("Child"), Some(child));
        assert_eq!(ed.parent_of(child), Some(root));
        assert_eq!(ed.list_entities().len(), 2);

        ed.rename(child, "Renamed").unwrap();
        assert_eq!(ed.name_of(child).as_deref(), Some("Renamed"));
        assert!(ed.find_by_name("Child").is_none());
        assert_eq!(ed.find_by_name("Renamed"), Some(child));
    }

    #[test]
    fn spawn_ignores_dead_parent() {
        let mut ed = editor();
        let temp = ed.spawn("Temp", None);
        ed.destroy(temp);
        let orphan = ed.spawn("Orphan", Some(temp));
        assert_eq!(ed.parent_of(orphan), None);
    }

    #[test]
    fn transform_round_trips() {
        let mut ed = editor();
        let e = ed.spawn("E", None);

        ed.set_translation(e, vec3(1.0, 2.0, 3.0)).unwrap();
        assert_eq!(ed.translation(e), Some(vec3(1.0, 2.0, 3.0)));

        ed.set_scale(e, vec3(2.0, 2.0, 2.0)).unwrap();
        assert_eq!(ed.scale(e), Some(vec3(2.0, 2.0, 2.0)));

        ed.set_rotation_euler(e, vec3(0.0, 0.0, 90.0)).unwrap();
        let euler = ed.rotation_euler_degrees(e).unwrap();
        assert!((euler.z - 90.0).abs() < 1e-3, "got {euler:?}");

        let q = Quat::from_axis_angle(vec3(0.0, 1.0, 0.0), radians(45.0));
        ed.set_rotation_quat(e, q).unwrap();
        let got = ed.rotation(e).unwrap();
        assert!((got.x - q.x).abs() < 1e-6);
        assert!((got.w - q.w).abs() < 1e-6);
    }

    #[test]
    fn translate_and_look_at() {
        let mut ed = editor();
        let e = ed.spawn("Cam", None);
        ed.translate(e, vec3(1.0, 0.0, 0.0)).unwrap();
        assert_eq!(ed.translation(e), Some(vec3(1.0, 0.0, 0.0)));

        // Looking at +X should rotate roughly -90 degrees about Y.
        ed.look_at(e, vec3(5.0, 0.0, 0.0), None).unwrap();
        let forward = ed.rotation(e).unwrap().rotate_vec3(vec3(0.0, 0.0, -1.0));
        assert!(forward.x > 0.99, "forward should point +X, got {forward:?}");
    }

    #[test]
    fn every_component_type_appears_in_list_entities() {
        let mut ed = editor();
        let e = ed.spawn("All", None);

        ed.add_camera(e, CameraParams::default()).unwrap();
        ed.add_light(e, LightParams::default()).unwrap();
        ed.set_mesh_renderer(e, MeshRendererParams::default())
            .unwrap();
        ed.add_rigid_body(e, RigidBodyParams::default()).unwrap();
        ed.add_collider(e, ColliderParams::default()).unwrap();
        ed.add_audio_source(e, AudioSourceParams::default())
            .unwrap();
        ed.attach_script_source(e, "function update() {}").unwrap();

        let info = ed
            .list_entities()
            .into_iter()
            .find(|i| i.entity == e)
            .unwrap();
        for expected in [
            "Camera",
            "Light",
            "MeshRenderer",
            "RigidBody",
            "Collider",
            "AudioSource",
            "Script",
        ] {
            assert!(info.components.contains(&expected), "missing {expected}");
        }
    }

    #[test]
    fn command_sequence_applies() {
        let mut ed = editor();
        let commands = vec![
            EditorCommand::Spawn {
                name: "Sun".into(),
                parent: None,
            },
            EditorCommand::AddLight {
                entity: EntityRef::named("Sun"),
                params: LightParams {
                    kind: LightSpec::Directional {
                        direction: [0.0, -1.0, 0.0],
                    },
                    intensity: 3.0,
                    ..LightParams::default()
                },
            },
            EditorCommand::Spawn {
                name: "Camera".into(),
                parent: None,
            },
            EditorCommand::AddCamera {
                entity: EntityRef::named("Camera"),
                params: CameraParams::default(),
            },
            EditorCommand::SetTranslation {
                entity: EntityRef::named("Camera"),
                translation: [0.0, 3.0, -8.0],
            },
        ];
        for command in commands {
            ed.apply(command).unwrap();
        }
        let sun = ed.find_by_name("Sun").unwrap();
        assert!(ed.has_component(sun, ComponentName::Light));
        assert_eq!(ed.list_entities().len(), 2);
    }

    #[test]
    fn undo_and_redo_restore_state() {
        let mut ed = editor();
        let e = ed.spawn("E", None);
        ed.set_translation(e, vec3(5.0, 0.0, 0.0)).unwrap();
        assert_eq!(ed.translation(e), Some(vec3(5.0, 0.0, 0.0)));
        assert!(ed.can_undo());

        assert!(ed.undo());
        // Entity ids change after undo; look it up by name.
        let e2 = ed.find_by_name("E").unwrap();
        assert_eq!(ed.translation(e2), Some(vec3(0.0, 0.0, 0.0)));

        assert!(ed.redo());
        let e3 = ed.find_by_name("E").unwrap();
        assert_eq!(ed.translation(e3), Some(vec3(5.0, 0.0, 0.0)));
    }

    #[test]
    fn undo_removes_a_component() {
        let mut ed = editor();
        let e = ed.spawn("E", None);
        ed.add_light(e, LightParams::default()).unwrap();
        assert!(ed.has_component(e, ComponentName::Light));
        assert!(ed.undo());
        let e2 = ed.find_by_name("E").unwrap();
        assert!(!ed.has_component(e2, ComponentName::Light));
    }

    #[test]
    fn interactive_edit_folds_commands_into_one_undo_step() {
        let mut ed = editor();
        let e = ed.spawn("E", None);
        let baseline = ed.history_len();

        ed.begin_interactive_edit();
        assert!(ed.in_interactive_edit());
        for _ in 0..5 {
            ed.translate(e, vec3(1.0, 0.0, 0.0)).unwrap();
        }
        assert_eq!(ed.history_len(), baseline, "nothing recorded mid-drag");
        assert_eq!(ed.translation(e), Some(vec3(5.0, 0.0, 0.0)));

        ed.end_interactive_edit();
        assert!(!ed.in_interactive_edit());
        assert_eq!(
            ed.history_len(),
            baseline + 1,
            "one step for the whole drag"
        );

        assert!(ed.undo());
        let e2 = ed.find_by_name("E").unwrap();
        assert_eq!(ed.translation(e2), Some(vec3(0.0, 0.0, 0.0)));
    }

    #[test]
    fn interactive_edit_without_changes_records_nothing() {
        let mut ed = editor();
        ed.spawn("E", None);
        let baseline = ed.history_len();
        ed.begin_interactive_edit();
        ed.end_interactive_edit();
        assert_eq!(ed.history_len(), baseline);
    }

    #[test]
    fn end_interactive_edit_without_begin_is_a_no_op() {
        let mut ed = editor();
        ed.spawn("E", None);
        let baseline = ed.history_len();
        ed.end_interactive_edit();
        assert_eq!(ed.history_len(), baseline);
    }

    #[test]
    fn commands_after_an_interaction_record_history_again() {
        let mut ed = editor();
        let e = ed.spawn("E", None);
        ed.begin_interactive_edit();
        ed.translate(e, vec3(1.0, 0.0, 0.0)).unwrap();
        ed.end_interactive_edit();
        let after_drag = ed.history_len();

        let e2 = ed.find_by_name("E").unwrap();
        ed.translate(e2, vec3(0.0, 1.0, 0.0)).unwrap();
        assert_eq!(ed.history_len(), after_drag + 1);
    }

    #[test]
    fn save_load_save_is_stable() {
        let mut ed = editor();
        let root = ed.spawn("Root", None);
        let child = ed.spawn("Child", Some(root));
        ed.set_translation(child, vec3(1.0, 2.0, 3.0)).unwrap();
        ed.add_camera(child, CameraParams::default()).unwrap();
        ed.add_rigid_body(root, RigidBodyParams::default()).unwrap();
        let json1 = ed.to_json();

        let reopened = Editor::from_json(&json1).unwrap();
        let json2 = reopened.to_json();
        assert_eq!(json1, json2, "save -> load -> save must be stable");
    }

    #[test]
    fn import_gltf_bytes_creates_entities() {
        let mut ed = editor();
        let entities = ed.import_gltf_bytes(&triangle_glb()).unwrap();
        assert_eq!(entities.len(), 1, "one node");
        // The node plus its mesh-renderer child.
        assert!(
            ed.list_entities()
                .iter()
                .any(|i| i.components.contains(&"MeshRenderer"))
        );
        let root = entities[0];
        assert_eq!(ed.name_of(root).as_deref(), Some("Tri"));
    }

    #[test]
    fn import_texture_registers_the_file() {
        let dir = std::env::temp_dir().join(format!("noctis-editor-tex-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wood.png");
        let mut pixels = image::RgbaImage::new(2, 3);
        pixels.put_pixel(0, 0, image::Rgba([1, 2, 3, 255]));
        pixels.save(&path).unwrap();

        let mut ed = editor();
        let handle = ed.import_texture(&path, true).unwrap();
        let texture = ed.assets().textures.get(handle).expect("registered");
        assert_eq!((texture.width, texture.height), (2, 3));
        assert_eq!(
            ed.assets().textures.path_of(handle),
            Some(path.to_str().unwrap())
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn import_texture_of_a_missing_file_errors() {
        let mut ed = editor();
        let err = ed
            .import_texture("definitely-missing.png", true)
            .unwrap_err();
        assert!(matches!(err, EditorError::Image(_)), "{err:?}");
    }

    #[test]
    fn duplicate_copies_hierarchy() {
        let mut ed = editor();
        let root = ed.spawn("Root", None);
        let child = ed.spawn("Child", Some(root));
        ed.add_light(child, LightParams::default()).unwrap();

        let copy = ed.duplicate(root).unwrap();
        assert_ne!(copy, root);
        let children = ecs::scene::children_of(&ed.scene.world, copy);
        assert_eq!(children.len(), 1);
        let copied_child = children[0];
        assert_eq!(ed.name_of(copied_child).as_deref(), Some("Child"));
        assert!(ed.has_component(copied_child, ComponentName::Light));
    }

    #[test]
    fn prefab_create_and_instantiate() {
        let mut ed = editor();
        let a = ed.spawn("A", None);
        let b = ed.spawn("B", Some(a));
        ed.set_translation(b, vec3(0.0, 1.0, 0.0)).unwrap();

        let name = ed.store_prefab("Pair", &[a, b]).unwrap();
        assert!(ed.prefab(&name).is_some());

        let instances = ed.instantiate_stored_prefab("Pair", None).unwrap();
        assert_eq!(instances.len(), 2);
        let names: Vec<String> = ed.list_entities().into_iter().map(|i| i.name).collect();
        assert_eq!(names.iter().filter(|n| n.as_str() == "A").count(), 2);
        assert_eq!(names.iter().filter(|n| n.as_str() == "B").count(), 2);
    }

    #[test]
    fn remove_component_works() {
        let mut ed = editor();
        let e = ed.spawn("E", None);
        ed.add_collider(e, ColliderParams::default()).unwrap();
        assert!(ed.has_component(e, ComponentName::Collider));
        ed.remove_component(e, ComponentName::Collider).unwrap();
        assert!(!ed.has_component(e, ComponentName::Collider));
    }

    #[test]
    fn edit_component_edits_in_place_and_is_undoable() {
        let mut ed = editor();
        let e = ed.spawn("Sun", None);
        ed.add_light(e, LightParams::default()).unwrap();

        // Fields the command path would reset survive an in-place edit.
        assert!(ed.edit_component::<Light>(e, |light| {
            light.intensity = 5.0;
            light.shadow_bias = 0.02;
        }));
        let light = ed.component::<Light>(e).unwrap();
        assert_eq!(light.intensity, 5.0);
        assert_eq!(light.shadow_bias, 0.02);

        assert!(ed.undo());
        let light = ed.component::<Light>(e).unwrap();
        assert_eq!(light.intensity, 1.0);
        assert_eq!(light.shadow_bias, 0.001);
    }

    #[test]
    fn edit_component_without_the_component_records_nothing() {
        let mut ed = editor();
        let e = ed.spawn("E", None);
        let history = ed.history_len();
        assert!(!ed.edit_component::<Light>(e, |_| {}));
        assert_eq!(ed.history_len(), history);
    }

    #[test]
    fn insert_component_adds_and_undoes() {
        let mut ed = editor();
        let e = ed.spawn("E", None);
        assert!(!ed.has_component(e, ComponentName::Camera));
        assert!(ed.insert_component(e, Camera::default()));
        assert!(ed.has_component(e, ComponentName::Camera));
        assert!(ed.undo());
        assert!(!ed.has_component(e, ComponentName::Camera));
    }

    #[test]
    fn selection_and_find_by_name() {
        let mut ed = editor();
        let a = ed.spawn("A", None);
        let b = ed.spawn("B", None);
        ed.select(b).unwrap();
        ed.select(a).unwrap();
        ed.select(a).unwrap(); // idempotent
        assert_eq!(ed.selection(), &[b, a]);
        ed.clear_selection();
        assert!(ed.selection().is_empty());
    }

    #[test]
    fn inspect_reports_components() {
        let mut ed = editor();
        let e = ed.spawn("E", None);
        ed.add_light(
            e,
            LightParams {
                kind: LightSpec::Point { range: 7.0 },
                ..LightParams::default()
            },
        )
        .unwrap();
        let json = ed.inspect(e).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["name"], "E");
        assert_eq!(value["components"]["Light"]["kind"]["type"], "point");
        assert_eq!(value["components"]["Light"]["kind"]["range"], 7.0);
        assert!(ed.inspect(Entity::from_raw(9999, 0)).is_none());
    }

    #[test]
    fn registered_assets_resolve_by_path() {
        let mut ed = editor();
        let e = ed.spawn("E", None);
        ed.apply(EditorCommand::RegisterMesh {
            name: "models/tri".into(),
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
        })
        .unwrap();
        ed.set_mesh_renderer(
            e,
            MeshRendererParams {
                mesh: Some(AssetRef::path("models/tri")),
                material: Some(AssetRef::path("$default")),
                ..MeshRendererParams::default()
            },
        )
        .unwrap();
        let renderer = ed.scene.world.get::<MeshRenderer>(e).copied().unwrap();
        assert!(!renderer.mesh.is_none());
        assert!(!renderer.material.is_none());
    }

    #[test]
    fn missing_asset_path_errors() {
        let mut ed = editor();
        let e = ed.spawn("E", None);
        let err = ed
            .set_mesh_renderer(
                e,
                MeshRendererParams {
                    mesh: Some(AssetRef::path("nope")),
                    ..MeshRendererParams::default()
                },
            )
            .unwrap_err();
        assert!(matches!(err, EditorError::MissingAsset(_)));
    }

    #[test]
    fn unknown_entity_errors() {
        let mut ed = editor();
        let err = ed
            .apply(EditorCommand::Rename {
                entity: EntityRef::named("ghost"),
                name: "x".into(),
            })
            .unwrap_err();
        assert!(matches!(err, EditorError::UnknownName(_)));
    }

    #[test]
    fn failed_command_rolls_back_scene() {
        let mut ed = editor();
        ed.spawn("E", None);
        // A command that mutates then fails: spawn one entity, then destroy a
        // name that does not exist partway through a batch.
        let before = ed.list_entities().len();
        let batch = r#"[
            { "op": "spawn", "name": "New1" },
            { "op": "rename", "entity": { "name": "ghost" }, "name": "x" }
        ]"#;
        let err = ed.apply_json(batch).unwrap_err();
        assert!(matches!(err, EditorError::UnknownName(_)));
        // `New1` was spawned by the first command and is a legitimate change;
        // the *failing* command must not have left a partial edit.
        assert_eq!(ed.list_entities().len(), before + 1);
        assert!(ed.find_by_name("E").is_some());
    }

    #[test]
    fn new_scene_clears_entities_but_is_undoable() {
        let mut ed = editor();
        ed.spawn("A", None);
        assert_eq!(ed.entity_count(), 1);
        ed.new_scene("Fresh");
        assert_eq!(ed.entity_count(), 0);
        assert_eq!(ed.scene_name(), "Fresh");
        assert!(ed.undo());
        assert_eq!(ed.entity_count(), 1);
    }

    #[test]
    fn save_creates_missing_parent_directories() {
        let dir = std::env::temp_dir().join(format!(
            "noctis-editor-save-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let target = dir.join("scenes/level.scene.json");

        let ed = editor();
        ed.save(&target).expect("save creates the parent dir");
        assert!(target.is_file());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scenes_default_to_3d_and_the_mode_is_undoable() {
        let mut ed = editor();
        assert_eq!(ed.mode(), SceneMode::ThreeD);

        ed.new_scene_with_mode("Menu", SceneMode::TwoD);
        assert_eq!(ed.mode(), SceneMode::TwoD);
        assert!(ed.scene().is_2d());

        ed.set_mode(SceneMode::ThreeD);
        assert_eq!(ed.mode(), SceneMode::ThreeD);
        assert!(ed.undo(), "set_mode is undoable");
        assert_eq!(ed.mode(), SceneMode::TwoD);
        assert!(ed.undo(), "the preceding new_scene is undoable");
        assert_eq!(ed.mode(), SceneMode::ThreeD);
    }

    #[test]
    fn set_mode_to_the_same_value_makes_no_history_entry() {
        let mut ed = editor();
        ed.set_mode(SceneMode::ThreeD);
        assert!(!ed.can_undo(), "a no-op set_mode records nothing");
    }

    #[test]
    fn scene_mode_survives_save_and_reload() {
        let mut ed = editor();
        ed.new_scene_with_mode("Level", SceneMode::TwoD);
        ed.spawn("Sprite", None);
        let json = ed.to_json();
        let reopened = Editor::from_json(&json).unwrap();
        assert_eq!(reopened.mode(), SceneMode::TwoD);
    }

    #[test]
    fn json_command_array_round_trips() {
        let json = r#"[
            { "op": "spawn", "name": "Box" },
            { "op": "add_collider", "entity": { "name": "Box" },
              "params": { "shape": { "type": "ball", "radius": 2.0 } } },
            { "op": "set_translation", "entity": { "name": "Box" },
              "translation": [1, 2, 3] }
        ]"#;
        let mut ed = editor();
        ed.apply_json(json).unwrap();
        let box_entity = ed.find_by_name("Box").unwrap();
        assert_eq!(ed.translation(box_entity), Some(vec3(1.0, 2.0, 3.0)));
        assert!(ed.has_component(box_entity, ComponentName::Collider));
        let collider = ed.scene.world.get::<Collider>(box_entity).unwrap();
        assert!(
            matches!(collider.shape, ColliderShape::Ball { radius } if (radius - 2.0).abs() < 1e-6)
        );
    }

    #[test]
    fn body_type_and_alpha_specs_convert() {
        let mut ed = editor();
        let e = ed.spawn("E", None);
        ed.add_rigid_body(
            e,
            RigidBodyParams {
                body_type: BodyTypeSpec::Fixed,
                ..RigidBodyParams::default()
            },
        )
        .unwrap();
        let body = ed.scene.world.get::<RigidBody>(e).unwrap();
        assert_eq!(body.body_type, ecs::components::BodyType::Fixed);

        ed.apply(EditorCommand::RegisterMaterial {
            name: "mat".into(),
            params: MaterialParams::default(),
        })
        .unwrap();
        assert!(ed.assets.materials.handle("mat").is_some());
    }

    #[test]
    fn look_at_and_collider_helpers_are_consistent() {
        // Exercise the collider-shape conversion helper directly.
        let shape: ColliderShape = ColliderShapeSpec::Capsule {
            half_height: 1.0,
            radius: 0.25,
        }
        .into();
        assert!(matches!(shape, ColliderShape::Capsule { .. }));
    }

    #[test]
    fn projection_spec_version_stays_persistent() {
        let p: Projection = ProjectionSpec::Orthographic {
            height: 4.0,
            near: 0.1,
            far: 100.0,
        }
        .into();
        assert!(
            matches!(p, Projection::Orthographic { height, .. } if (height - 4.0).abs() < 1e-6)
        );
    }

    // -- test fixture -------------------------------------------------------

    /// A minimal binary glTF (GLB) with one triangle, matching the renderer's
    /// own test fixture.
    fn triangle_glb() -> Vec<u8> {
        let positions: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let normals: [[f32; 3]; 3] = [[0.0, 0.0, 1.0]; 3];
        let uvs: [[f32; 2]; 3] = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        let indices: [u16; 3] = [0, 1, 2];

        let mut bin = Vec::new();
        for p in positions {
            for c in p {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        let normal_offset = bin.len();
        for n in normals {
            for c in n {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        let uv_offset = bin.len();
        for uv in uvs {
            for c in uv {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        let index_offset = bin.len();
        for i in indices {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        let total = bin.len();

        let json = format!(
            r#"{{
  "asset": {{ "version": "2.0" }},
  "scene": 0,
  "scenes": [ {{ "name": "Main", "nodes": [0] }} ],
  "nodes": [ {{ "name": "Tri", "mesh": 0 }} ],
  "meshes": [ {{ "name": "TriMesh", "primitives": [
     {{ "attributes": {{ "POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2 }}, "indices": 3, "material": 0 }}
  ] }} ],
  "materials": [ {{ "name": "Red", "pbrMetallicRoughness": {{
     "baseColorFactor": [1.0, 0.0, 0.0, 1.0], "metallicFactor": 0.1, "roughnessFactor": 0.7
  }} }} ],
  "buffers": [ {{ "byteLength": {total} }} ],
  "bufferViews": [
    {{ "buffer": 0, "byteOffset": 0, "byteLength": 36 }},
    {{ "buffer": 0, "byteOffset": {normal_offset}, "byteLength": 36 }},
    {{ "buffer": 0, "byteOffset": {uv_offset}, "byteLength": 24 }},
    {{ "buffer": 0, "byteOffset": {index_offset}, "byteLength": 6 }}
  ],
  "accessors": [
    {{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0,0,0], "max": [1,1,0] }},
    {{ "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3" }},
    {{ "bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC2" }},
    {{ "bufferView": 3, "componentType": 5123, "count": 3, "type": "SCALAR" }}
  ]
}}"#
        );
        make_glb(&json, &bin)
    }

    fn make_glb(json: &str, bin: &[u8]) -> Vec<u8> {
        let mut json_bytes = json.as_bytes().to_vec();
        while !json_bytes.len().is_multiple_of(4) {
            json_bytes.push(b' ');
        }
        let mut bin_bytes = bin.to_vec();
        while !bin_bytes.len().is_multiple_of(4) {
            bin_bytes.push(0);
        }
        let total = 12 + 8 + json_bytes.len() + 8 + bin_bytes.len();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes()); // "JSON"
        out.extend_from_slice(&json_bytes);
        out.extend_from_slice(&(bin_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&0x004E_4942u32.to_le_bytes()); // "BIN\0"
        out.extend_from_slice(&bin_bytes);
        out
    }
}
