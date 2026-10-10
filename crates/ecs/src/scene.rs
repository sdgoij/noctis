//! Scenes, the transform hierarchy, and the serializable scene model.
//!
//! A [`Scene`] is a [`World`] plus a name. The [`SceneDesc`] type is the
//! serializable form used for saving, prefabs, and game export; converting
//! between the two keeps all asset references as human-readable paths.

use crate::components::*;
use crate::world::{Entity, World};
use math::{Mat4, Quat, Transform, vec3};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::assets::AssetRegistry;
use crate::reflect::ComponentRegistry;

// ---------------------------------------------------------------------------
// Transform hierarchy
// ---------------------------------------------------------------------------

/// Parent `child` to `parent`, updating both the child's [`Parent`] and the old
/// and new parents' [`Children`] lists. Passing `None` unparents the child.
pub fn set_parent(world: &mut World, child: Entity, parent: Option<Entity>) {
    if !world.is_alive(child) {
        return;
    }
    // Detach from the previous parent.
    if let Some(Parent(old)) = world.get::<Parent>(child).copied()
        && let Some(children) = world.get_mut::<Children>(old)
    {
        children.0.retain(|c| *c != child);
    }
    match parent {
        Some(p) if world.is_alive(p) && p != child => {
            world.insert(child, Parent(p));
            match world.get_mut::<Children>(p) {
                Some(children) => {
                    if !children.0.contains(&child) {
                        children.0.push(child);
                    }
                }
                None => {
                    world.insert(p, Children(vec![child]));
                }
            }
        }
        _ => {
            world.remove::<Parent>(child);
        }
    }
}

/// The children of an entity, or an empty vector.
pub fn children_of(world: &World, entity: Entity) -> Vec<Entity> {
    world
        .get::<Children>(entity)
        .map(|c| c.0.clone())
        .unwrap_or_default()
}

/// Recompute every entity's [`GlobalTransform`] from the hierarchy.
///
/// Cycles are tolerated: a guard bounds the ancestor walk.
pub fn propagate_transforms(world: &mut World) {
    let entities: Vec<Entity> = world.entities_with::<LocalTransform>();
    let mut updates: Vec<(Entity, Mat4)> = Vec::with_capacity(entities.len());
    for entity in entities {
        updates.push((entity, compute_global_matrix(world, entity)));
    }
    for (entity, matrix) in updates {
        match world.get_mut::<GlobalTransform>(entity) {
            Some(g) => g.matrix = matrix,
            None => {
                world.insert(entity, GlobalTransform { matrix });
            }
        }
    }
}

fn compute_global_matrix(world: &World, entity: Entity) -> Mat4 {
    // Build the ancestor chain from the entity up to its root.
    let mut chain = Vec::new();
    let mut cursor = Some(entity);
    let mut guard = 0;
    while let Some(current) = cursor {
        chain.push(current);
        cursor = world
            .get::<Parent>(current)
            .map(|p| p.0)
            .filter(|p| world.is_alive(*p));
        guard += 1;
        if guard > 4096 {
            break; // defensive: a cycle
        }
    }
    // Compose from the root down: root * ... * entity.
    let mut matrix = math::identity();
    for &node in chain.iter().rev() {
        if let Some(local) = world.get::<LocalTransform>(node) {
            matrix = matrix * local.to_matrix();
        }
    }
    // The entity itself always contributes its local transform, even if the
    // ancestor walk was cut short.
    if !chain.contains(&entity)
        && let Some(local) = world.get::<LocalTransform>(entity)
    {
        matrix = matrix * local.to_matrix();
    }
    matrix
}

// ---------------------------------------------------------------------------
// Scene
// ---------------------------------------------------------------------------

/// A scene's presentation mode: 3D, or 2D as a hint (not a schema fork).
///
/// `mode` is **per scene** because mixing dimensions is a real use case — a
/// project may hold a 2D menu scene and a 3D level. It is a *presentation hint*
/// that drives editor defaults (an orthographic camera, a top-down grid, a
/// planar gizmo), not a change of format: there is exactly one scene schema.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SceneMode {
    /// Two-dimensional presentation.
    #[serde(rename = "2d")]
    TwoD,
    /// Three-dimensional presentation (the default).
    #[default]
    #[serde(rename = "3d")]
    ThreeD,
}

impl SceneMode {
    /// The canonical string (`"2d"` / `"3d"`).
    pub fn as_str(self) -> &'static str {
        match self {
            SceneMode::TwoD => "2d",
            SceneMode::ThreeD => "3d",
        }
    }

    /// Whether this is the 2D mode.
    pub fn is_2d(self) -> bool {
        self == SceneMode::TwoD
    }
}

impl std::fmt::Display for SceneMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for SceneMode {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.to_ascii_lowercase().as_str() {
            "2d" | "2" => Ok(SceneMode::TwoD),
            "3d" | "3" => Ok(SceneMode::ThreeD),
            other => Err(format!("unknown scene mode '{other}' (expected 2d or 3d)")),
        }
    }
}

/// A named [`World`].
#[derive(Default)]
pub struct Scene {
    /// The scene name.
    pub name: String,
    /// The presentation mode (a hint; see [`SceneMode`]).
    pub mode: SceneMode,
    /// The entities and components.
    pub world: World,
    /// The custom component types this scene may carry (see [`ComponentRegistry`]).
    pub components: ComponentRegistry,
}

impl Scene {
    /// An empty scene with a name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            mode: SceneMode::default(),
            world: World::new(),
            components: ComponentRegistry::new(),
        }
    }

    /// An empty scene with a name and a presentation mode.
    pub fn with_mode(name: impl Into<String>, mode: SceneMode) -> Self {
        Self {
            mode,
            ..Self::new(name)
        }
    }

    /// Carry `components`, so registered custom components load and save.
    pub fn with_component_registry(mut self, components: ComponentRegistry) -> Self {
        self.components = components;
        self
    }

    /// Whether this scene is 2D (a presentation hint).
    pub fn is_2d(&self) -> bool {
        self.mode.is_2d()
    }

    /// Propagate all transforms in the scene.
    pub fn update_transforms(&mut self) {
        propagate_transforms(&mut self.world);
    }

    /// Convert to the serializable form, resolving asset handles to paths via
    /// `assets`.
    pub fn to_desc(&self, assets: &AssetRegistry) -> SceneDesc {
        // Only entities that participate in the scene (have a transform and/or
        // a name) are serialized.
        let mut entities: Vec<Entity> = self
            .world
            .entities()
            .iter()
            .filter(|e| self.world.has::<LocalTransform>(*e) || self.world.has::<Name>(*e))
            .collect();
        entities.sort_unstable();

        let mut id_of: HashMap<Entity, u32> = HashMap::new();
        for (i, e) in entities.iter().enumerate() {
            id_of.insert(*e, i as u32);
        }

        let mut descs = Vec::with_capacity(entities.len());
        for (id, entity) in entities.iter().enumerate() {
            let transform = self
                .world
                .get::<LocalTransform>(*entity)
                .copied()
                .unwrap_or_default();
            let parent = self
                .world
                .get::<Parent>(*entity)
                .and_then(|p| id_of.get(&p.0).copied());
            let name = self.world.get::<Name>(*entity).map(|n| n.0.clone());
            let active = self.world.get::<Active>(*entity).is_none_or(|a| a.0);
            let visible = self.world.get::<Visible>(*entity).is_none_or(|v| v.0);
            let components = component_descs(&self.world, *entity, assets, &self.components);
            descs.push(EntityDesc {
                id: id as u32,
                name,
                parent,
                active,
                visible,
                transform: TransformDesc::from(&transform),
                components,
            });
        }

        SceneDesc {
            name: self.name.clone(),
            format_version: SCENE_FORMAT_VERSION,
            mode: self.mode,
            entities: descs,
        }
    }

    /// Build a scene from its serializable form, resolving asset paths via
    /// `assets`.
    ///
    /// Custom components are not applied; use [`Scene::from_desc_with`] to load a
    /// scene that carries registered custom components.
    pub fn from_desc(desc: &SceneDesc, assets: &AssetRegistry) -> Scene {
        Scene::from_desc_with(desc, assets, &ComponentRegistry::new())
    }

    /// Like [`Scene::from_desc`], but with a component registry, so registered
    /// custom components are instantiated.
    pub fn from_desc_with(
        desc: &SceneDesc,
        assets: &AssetRegistry,
        components: &ComponentRegistry,
    ) -> Scene {
        let mut scene = Scene::with_mode(desc.name.clone(), desc.mode)
            .with_component_registry(components.clone());
        instantiate_into_with(&mut scene.world, desc, assets, None, components);
        scene.update_transforms();
        scene
    }

    /// Serialize the scene to pretty JSON, resolving asset paths via `assets`.
    pub fn to_json(&self, assets: &AssetRegistry) -> String {
        serde_json::to_string_pretty(&self.to_desc(assets)).expect("scene serializes")
    }

    /// Parse a scene from JSON and instantiate it.
    ///
    /// Every asset reference must resolve against `assets`: a reference to a
    /// missing asset is an error that names the path, never a silent blank.
    pub fn from_json(json: &str, assets: &AssetRegistry) -> Result<Scene, SceneError> {
        Scene::from_json_with(json, assets, &ComponentRegistry::new())
    }

    /// Like [`Scene::from_json`], but with a component registry, so registered
    /// custom components load.
    pub fn from_json_with(
        json: &str,
        assets: &AssetRegistry,
        components: &ComponentRegistry,
    ) -> Result<Scene, SceneError> {
        let desc: SceneDesc =
            serde_json::from_str(json).map_err(|e| SceneError::Parse(e.to_string()))?;
        Scene::try_from_desc_with(&desc, assets, components)
    }

    /// Build a scene from its serializable form, erroring if any asset reference
    /// does not resolve against `assets`.
    ///
    /// This is the strict counterpart of [`Scene::from_desc`]: use it wherever a
    /// document is loaded (so a broken reference is loud), and the lenient
    /// `from_desc` for restoring snapshots that are known to be resolvable.
    pub fn try_from_desc(desc: &SceneDesc, assets: &AssetRegistry) -> Result<Scene, SceneError> {
        Scene::try_from_desc_with(desc, assets, &ComponentRegistry::new())
    }

    /// Like [`Scene::try_from_desc`], but with a component registry.
    ///
    /// A custom component naming a type that is not registered is an error that
    /// names the type — the scene needs the crate that defines it to be linked in.
    pub fn try_from_desc_with(
        desc: &SceneDesc,
        assets: &AssetRegistry,
        components: &ComponentRegistry,
    ) -> Result<Scene, SceneError> {
        for entity in &desc.entities {
            for component in &entity.components {
                if let ComponentDesc::Custom { name, .. } = component
                    && !components.has(name)
                {
                    return Err(SceneError::UnknownComponent(name.clone()));
                }
                if let Some(path) = missing_asset(component, assets) {
                    return Err(SceneError::MissingAsset(path.to_string()));
                }
            }
        }
        Ok(Scene::from_desc_with(desc, assets, components))
    }
}

/// The current scene format version.
pub const SCENE_FORMAT_VERSION: u32 = 1;

/// Errors from loading a scene.
#[derive(Debug)]
pub enum SceneError {
    /// The document could not be parsed.
    Parse(String),
    /// The document was written by an incompatible format version.
    Version(u32),
    /// A component referenced an asset that is not in the registry.
    MissingAsset(String),
    /// A scene named a custom component whose type is not registered.
    UnknownComponent(String),
}

impl std::fmt::Display for SceneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SceneError::Parse(msg) => write!(f, "scene parse error: {msg}"),
            SceneError::Version(v) => write!(f, "unsupported scene format version: {v}"),
            SceneError::MissingAsset(path) => write!(f, "missing asset reference: {path}"),
            SceneError::UnknownComponent(name) => {
                write!(
                    f,
                    "unknown component type '{name}': is its crate linked in?"
                )
            }
        }
    }
}

impl std::error::Error for SceneError {}

/// Spawn the entities in `desc` into `world` under an optional `parent`.
///
/// Returns the entities in document order. Custom components are not applied; use
/// [`instantiate_into_with`] when the scene carries registered custom components.
pub fn instantiate_into(
    world: &mut World,
    desc: &SceneDesc,
    assets: &AssetRegistry,
    parent: Option<Entity>,
) -> Vec<Entity> {
    instantiate_into_with(world, desc, assets, parent, &ComponentRegistry::new())
}

/// Like [`instantiate_into`], but with a component registry.
pub fn instantiate_into_with(
    world: &mut World,
    desc: &SceneDesc,
    assets: &AssetRegistry,
    parent: Option<Entity>,
    components: &ComponentRegistry,
) -> Vec<Entity> {
    let mut id_to_entity: HashMap<u32, Entity> = HashMap::new();
    for e in &desc.entities {
        let entity = world.spawn();
        id_to_entity.insert(e.id, entity);
    }
    for e in &desc.entities {
        let entity = id_to_entity[&e.id];
        world.insert(entity, LocalTransform::from(&e.transform));
        if let Some(name) = &e.name {
            world.insert(entity, Name(name.clone()));
        }
        if !e.active {
            world.insert(entity, Active(false));
        }
        if !e.visible {
            world.insert(entity, Visible(false));
        }
        for component in &e.components {
            apply_component(world, entity, component, assets, components);
        }
    }
    // Parent links, then attach the whole subtree to `parent`.
    for e in &desc.entities {
        if let Some(parent_id) = e.parent {
            if let Some(&p) = id_to_entity.get(&parent_id) {
                set_parent(world, id_to_entity[&e.id], Some(p));
            }
        } else if let Some(outer) = parent {
            set_parent(world, id_to_entity[&e.id], Some(outer));
        }
    }
    desc.entities.iter().map(|e| id_to_entity[&e.id]).collect()
}

fn component_descs(
    world: &World,
    entity: Entity,
    assets: &AssetRegistry,
    components: &ComponentRegistry,
) -> Vec<ComponentDesc> {
    let mut out = Vec::new();
    if let Some(c) = world.get::<Camera>(entity) {
        out.push(ComponentDesc::Camera {
            projection: ProjectionDesc::from(&c.projection),
            is_active: c.is_active,
            priority: c.priority,
            clear_color: c.clear_color,
            hdr: c.hdr,
        });
    }
    if let Some(l) = world.get::<Light>(entity) {
        out.push(ComponentDesc::Light {
            kind: LightKindDesc::from(&l.kind),
            color: [l.color.x, l.color.y, l.color.z],
            intensity: l.intensity,
            cast_shadows: l.cast_shadows,
        });
    }
    if let Some(m) = world.get::<MeshRenderer>(entity) {
        out.push(ComponentDesc::MeshRenderer {
            mesh: assets.meshes.path_of(m.mesh).map(str::to_string),
            material: assets.materials.path_of(m.material).map(str::to_string),
            cast_shadows: m.cast_shadows,
            receive_shadows: m.receive_shadows,
        });
    }
    if let Some(r) = world.get::<RigidBody>(entity) {
        out.push(ComponentDesc::RigidBody {
            body_type: match r.body_type {
                BodyType::Dynamic => "dynamic",
                BodyType::Fixed => "fixed",
                BodyType::Kinematic => "kinematic",
            }
            .to_string(),
            mass: r.mass,
            restitution: r.restitution,
            friction: r.friction,
            gravity_scale: r.gravity_scale,
            linear_velocity: [
                r.linear_velocity.x,
                r.linear_velocity.y,
                r.linear_velocity.z,
            ],
            angular_velocity: [
                r.angular_velocity.x,
                r.angular_velocity.y,
                r.angular_velocity.z,
            ],
        });
    }
    if let Some(c) = world.get::<Collider>(entity) {
        out.push(ComponentDesc::Collider {
            shape: ColliderShapeDesc::from(&c.shape),
            offset: [c.offset.x, c.offset.y, c.offset.z],
            is_sensor: c.is_sensor,
            friction: c.friction,
            restitution: c.restitution,
        });
    }
    if let Some(a) = world.get::<AudioSource>(entity) {
        out.push(ComponentDesc::AudioSource {
            clip: assets.audio.path_of(a.clip).map(str::to_string),
            volume: a.volume,
            pitch: a.pitch,
            looped: a.looped,
            playing: a.playing,
            spatial: a.spatial,
            max_distance: a.max_distance,
            rolloff: a.rolloff,
        });
    }
    if let Some(s) = world.get::<Script>(entity) {
        out.push(ComponentDesc::Script {
            source: s.source.clone(),
            path: s.path.clone(),
            enabled: s.enabled,
        });
    }
    if let Some(t) = world.get::<Text>(entity) {
        out.push(ComponentDesc::Text {
            text: t.text.clone(),
            font: assets.fonts.path_of(t.font).map(str::to_string),
            size: t.size,
            color: t.color,
            anchor: TextAnchorDesc::from(&t.anchor),
            depth_test: t.depth_test,
        });
    }
    // Registered custom components, sorted by name (deterministic).
    for (name, data) in components.serialize_entity(world, entity) {
        out.push(ComponentDesc::Custom {
            name: name.to_string(),
            data,
        });
    }
    out
}

/// The first asset path referenced by `desc` that is absent from `assets`, if
/// any. A `None` reference (no mesh, no clip) is not an error.
fn missing_asset<'a>(desc: &'a ComponentDesc, assets: &AssetRegistry) -> Option<&'a str> {
    match desc {
        ComponentDesc::MeshRenderer { mesh, material, .. } => mesh
            .as_deref()
            .filter(|p| assets.meshes.handle(p).is_none())
            .or_else(|| {
                material
                    .as_deref()
                    .filter(|p| assets.materials.handle(p).is_none())
            }),
        ComponentDesc::AudioSource { clip, .. } => {
            clip.as_deref().filter(|p| assets.audio.handle(p).is_none())
        }
        ComponentDesc::Text { font, .. } => {
            font.as_deref().filter(|p| assets.fonts.handle(p).is_none())
        }
        _ => None,
    }
}

fn apply_component(
    world: &mut World,
    entity: Entity,
    desc: &ComponentDesc,
    assets: &AssetRegistry,
    components: &ComponentRegistry,
) {
    match desc {
        ComponentDesc::Camera {
            projection,
            is_active,
            priority,
            clear_color,
            hdr,
        } => {
            world.insert(
                entity,
                Camera {
                    projection: Projection::from(projection),
                    is_active: *is_active,
                    priority: *priority,
                    clear_color: *clear_color,
                    hdr: *hdr,
                },
            );
        }
        ComponentDesc::Light {
            kind,
            color,
            intensity,
            cast_shadows,
        } => {
            world.insert(
                entity,
                Light {
                    kind: LightKind::from(kind),
                    color: vec3(color[0], color[1], color[2]),
                    intensity: *intensity,
                    cast_shadows: *cast_shadows,
                    shadow_bias: Light::default().shadow_bias,
                },
            );
        }
        ComponentDesc::MeshRenderer {
            mesh,
            material,
            cast_shadows,
            receive_shadows,
        } => {
            let mesh_handle = mesh
                .as_deref()
                .and_then(|p| assets.meshes.handle(p))
                .unwrap_or_default();
            let material_handle = material
                .as_deref()
                .and_then(|p| assets.materials.handle(p))
                .unwrap_or_default();
            world.insert(
                entity,
                MeshRenderer {
                    mesh: mesh_handle,
                    material: material_handle,
                    cast_shadows: *cast_shadows,
                    receive_shadows: *receive_shadows,
                },
            );
        }
        ComponentDesc::RigidBody {
            body_type,
            mass,
            restitution,
            friction,
            gravity_scale,
            linear_velocity,
            angular_velocity,
        } => {
            world.insert(
                entity,
                RigidBody {
                    body_type: match body_type.as_str() {
                        "fixed" => BodyType::Fixed,
                        "kinematic" => BodyType::Kinematic,
                        _ => BodyType::Dynamic,
                    },
                    mass: *mass,
                    restitution: *restitution,
                    friction: *friction,
                    gravity_scale: *gravity_scale,
                    linear_velocity: vec3(
                        linear_velocity[0],
                        linear_velocity[1],
                        linear_velocity[2],
                    ),
                    angular_velocity: vec3(
                        angular_velocity[0],
                        angular_velocity[1],
                        angular_velocity[2],
                    ),
                    ..RigidBody::default()
                },
            );
        }
        ComponentDesc::Collider {
            shape,
            offset,
            is_sensor,
            friction,
            restitution,
        } => {
            world.insert(
                entity,
                Collider {
                    shape: ColliderShape::from(shape),
                    offset: vec3(offset[0], offset[1], offset[2]),
                    is_sensor: *is_sensor,
                    friction: *friction,
                    restitution: *restitution,
                    ..Collider::default()
                },
            );
        }
        ComponentDesc::AudioSource {
            clip,
            volume,
            pitch,
            looped,
            playing,
            spatial,
            max_distance,
            rolloff,
        } => {
            let clip_handle = clip
                .as_deref()
                .and_then(|p| assets.audio.handle(p))
                .unwrap_or_default();
            world.insert(
                entity,
                AudioSource {
                    clip: clip_handle,
                    volume: *volume,
                    pitch: *pitch,
                    looped: *looped,
                    playing: *playing,
                    spatial: *spatial,
                    max_distance: *max_distance,
                    rolloff: *rolloff,
                },
            );
        }
        ComponentDesc::Script {
            source,
            path,
            enabled,
        } => {
            world.insert(
                entity,
                Script {
                    source: source.clone(),
                    path: path.clone(),
                    enabled: *enabled,
                },
            );
        }
        ComponentDesc::Text {
            text,
            font,
            size,
            color,
            anchor,
            depth_test,
        } => {
            let font = font
                .as_deref()
                .and_then(|p| assets.fonts.handle(p))
                .unwrap_or_default();
            world.insert(
                entity,
                Text {
                    text: text.clone(),
                    font,
                    size: *size,
                    color: *color,
                    anchor: TextAnchor::from(anchor),
                    depth_test: *depth_test,
                },
            );
        }
        ComponentDesc::Custom { name, data } => {
            // The strict loader already validated the name; a lenient
            // `from_desc_with` on an unknown or malformed entry skips it.
            let _ = components.deserialize(world, entity, name, data);
        }
    }
}

// ---------------------------------------------------------------------------
// Component reflection
// ---------------------------------------------------------------------------
//
// The script bridge reads and writes components by name. A registered custom
// component's JSON is its own serde form (via `ComponentRegistry`); a built-in
// component's JSON is its scene descriptor ([`ComponentDesc`]) — the canonical,
// path-resolving form, since built-ins have no serde impls of their own. Both go
// through one pair of entry points, so a script sees one uniform API.

/// The reflection name of a scene component descriptor: a built-in's canonical
/// name (matching
/// [`ComponentKind`](crate::components::component_index::ComponentKind)) or a
/// custom component's registered name.
pub fn desc_name(desc: &ComponentDesc) -> &str {
    match desc {
        ComponentDesc::Camera { .. } => "Camera",
        ComponentDesc::Light { .. } => "Light",
        ComponentDesc::MeshRenderer { .. } => "MeshRenderer",
        ComponentDesc::RigidBody { .. } => "RigidBody",
        ComponentDesc::Collider { .. } => "Collider",
        ComponentDesc::AudioSource { .. } => "AudioSource",
        ComponentDesc::Script { .. } => "Script",
        ComponentDesc::Text { .. } => "Text",
        ComponentDesc::Custom { name, .. } => name,
    }
}

/// The built-in component of `entity` named `name`, in scene-descriptor form.
///
/// Only the built-ins that serialize into an entity's `components` array are
/// reflected; `LocalTransform`/`Name`/`Active`/`Visible` are entity-level and
/// have their own bindings.
fn builtin_desc(
    world: &World,
    entity: Entity,
    name: &str,
    assets: &AssetRegistry,
) -> Option<ComponentDesc> {
    component_descs(world, entity, assets, &ComponentRegistry::new())
        .into_iter()
        .find(|desc| !matches!(desc, ComponentDesc::Custom { .. }) && desc_name(desc) == name)
}

/// `true` if `entity` carries the reflected component `name` — a registered
/// custom (checked through `components`) or a scene-serializable built-in.
pub fn has_reflected_component(
    world: &World,
    entity: Entity,
    name: &str,
    assets: &AssetRegistry,
    components: &ComponentRegistry,
) -> bool {
    if components.has(name) {
        components.has_on(world, entity, name)
    } else {
        builtin_desc(world, entity, name, assets).is_some()
    }
}

/// The reflected component `name` on `entity` as JSON: a registered custom's own
/// JSON, or a built-in's scene-descriptor JSON (`{ "type": "...", ... }`).
pub fn get_reflected_component(
    world: &World,
    entity: Entity,
    name: &str,
    assets: &AssetRegistry,
    components: &ComponentRegistry,
) -> Option<serde_json::Value> {
    if components.has(name) {
        return components.serialize(world, entity, name);
    }
    builtin_desc(world, entity, name, assets).and_then(|desc| serde_json::to_value(&desc).ok())
}

/// Every reflected component on `entity` as `(name, JSON)` — built-ins first,
/// then registered customs (a custom name shadows a built-in of the same name).
pub fn reflect_components(
    world: &World,
    entity: Entity,
    assets: &AssetRegistry,
    components: &ComponentRegistry,
) -> Vec<(String, serde_json::Value)> {
    let mut out = Vec::new();
    for desc in component_descs(world, entity, assets, &ComponentRegistry::new()) {
        if let Ok(value) = serde_json::to_value(&desc) {
            out.push((desc_name(&desc).to_string(), value));
        }
    }
    for (name, data) in components.serialize_entity(world, entity) {
        out.push((name.to_string(), data));
    }
    out
}

/// Set (or insert) the reflected component `name` on `entity` from `value`.
///
/// A registered custom is deserialized through `components`; a built-in is parsed
/// as a [`ComponentDesc`] whose type must match `name`. A parse failure or a type
/// mismatch is an `Err`, never a panic.
pub fn set_reflected_component(
    world: &mut World,
    entity: Entity,
    name: &str,
    value: &serde_json::Value,
    assets: &AssetRegistry,
    components: &ComponentRegistry,
) -> Result<(), String> {
    if components.has(name) {
        return components.deserialize(world, entity, name, value);
    }
    let desc: ComponentDesc =
        serde_json::from_value(value.clone()).map_err(|error| format!("set {name:?}: {error}"))?;
    let actual = desc_name(&desc);
    if actual != name {
        return Err(format!("set {name:?}: the JSON is a {actual:?}"));
    }
    apply_component(world, entity, &desc, assets, components);
    Ok(())
}

/// Remove the reflected component `name` from `entity`.
///
/// Returns whether one was removed.
pub fn remove_reflected_component(
    world: &mut World,
    entity: Entity,
    name: &str,
    components: &ComponentRegistry,
) -> bool {
    if components.has(name) {
        return components.remove(world, entity, name);
    }
    match name {
        "Camera" => world.remove::<Camera>(entity).is_some(),
        "Light" => world.remove::<Light>(entity).is_some(),
        "MeshRenderer" => world.remove::<MeshRenderer>(entity).is_some(),
        "RigidBody" => world.remove::<RigidBody>(entity).is_some(),
        "Collider" => world.remove::<Collider>(entity).is_some(),
        "AudioSource" => world.remove::<AudioSource>(entity).is_some(),
        "Script" => world.remove::<Script>(entity).is_some(),
        "Text" => world.remove::<Text>(entity).is_some(),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Serializable model
// ---------------------------------------------------------------------------

/// A serializable scene.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SceneDesc {
    /// The scene name.
    pub name: String,
    /// The format version this document was written with.
    #[serde(default = "default_version")]
    pub format_version: u32,
    /// The presentation mode (a hint; see [`SceneMode`]).
    #[serde(default)]
    pub mode: SceneMode,
    /// The entities in the scene.
    #[serde(default)]
    pub entities: Vec<EntityDesc>,
}

fn default_version() -> u32 {
    SCENE_FORMAT_VERSION
}

/// A serializable entity.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct EntityDesc {
    /// A stable id, unique within the document.
    pub id: u32,
    /// The display name.
    #[serde(default)]
    pub name: Option<String>,
    /// The parent's id, if any.
    #[serde(default)]
    pub parent: Option<u32>,
    /// Whether the entity is active.
    #[serde(default = "default_true")]
    pub active: bool,
    /// Whether the entity's renderables are drawn.
    #[serde(default = "default_true")]
    pub visible: bool,
    /// The local transform.
    pub transform: TransformDesc,
    /// Additional components.
    #[serde(default)]
    pub components: Vec<ComponentDesc>,
}

fn default_true() -> bool {
    true
}

/// A serializable transform.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct TransformDesc {
    /// Translation `[x, y, z]`.
    pub translation: [f32; 3],
    /// Rotation quaternion `[x, y, z, w]`.
    pub rotation: [f32; 4],
    /// Scale `[x, y, z]`.
    pub scale: [f32; 3],
}

impl Default for TransformDesc {
    fn default() -> Self {
        Self::from(&Transform::identity())
    }
}

impl From<&Transform> for TransformDesc {
    fn from(t: &Transform) -> Self {
        Self {
            translation: [t.translation.x, t.translation.y, t.translation.z],
            rotation: [t.rotation.x, t.rotation.y, t.rotation.z, t.rotation.w],
            scale: [t.scale.x, t.scale.y, t.scale.z],
        }
    }
}

impl From<&TransformDesc> for Transform {
    fn from(d: &TransformDesc) -> Self {
        Transform {
            translation: vec3(d.translation[0], d.translation[1], d.translation[2]),
            rotation: Quat::new(d.rotation[0], d.rotation[1], d.rotation[2], d.rotation[3]),
            scale: vec3(d.scale[0], d.scale[1], d.scale[2]),
        }
    }
}

/// A serializable camera projection.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectionDesc {
    /// Perspective projection.
    Perspective {
        /// Vertical FOV, radians.
        fov_y: f32,
        /// Near plane.
        near: f32,
        /// Far plane.
        far: f32,
    },
    /// Orthographic projection.
    Orthographic {
        /// Visible height.
        height: f32,
        /// Near plane.
        near: f32,
        /// Far plane.
        far: f32,
    },
}

impl From<&Projection> for ProjectionDesc {
    fn from(p: &Projection) -> Self {
        match p {
            Projection::Perspective { fov_y, near, far } => ProjectionDesc::Perspective {
                fov_y: *fov_y,
                near: *near,
                far: *far,
            },
            Projection::Orthographic { height, near, far } => ProjectionDesc::Orthographic {
                height: *height,
                near: *near,
                far: *far,
            },
        }
    }
}

impl From<&ProjectionDesc> for Projection {
    fn from(d: &ProjectionDesc) -> Self {
        match d {
            ProjectionDesc::Perspective { fov_y, near, far } => Projection::Perspective {
                fov_y: *fov_y,
                near: *near,
                far: *far,
            },
            ProjectionDesc::Orthographic { height, near, far } => Projection::Orthographic {
                height: *height,
                near: *near,
                far: *far,
            },
        }
    }
}

/// A serializable light kind.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LightKindDesc {
    /// Directional light.
    Directional {
        /// Direction `[x, y, z]`.
        direction: [f32; 3],
    },
    /// Point light.
    Point {
        /// Falloff range.
        range: f32,
    },
    /// Spot light.
    Spot {
        /// Direction `[x, y, z]`.
        direction: [f32; 3],
        /// Inner cone angle, radians.
        inner_angle: f32,
        /// Outer cone angle, radians.
        outer_angle: f32,
        /// Falloff range.
        range: f32,
    },
}

impl From<&LightKind> for LightKindDesc {
    fn from(k: &LightKind) -> Self {
        match k {
            LightKind::Directional { direction } => LightKindDesc::Directional {
                direction: [direction.x, direction.y, direction.z],
            },
            LightKind::Point { range } => LightKindDesc::Point { range: *range },
            LightKind::Spot {
                direction,
                inner_angle,
                outer_angle,
                range,
            } => LightKindDesc::Spot {
                direction: [direction.x, direction.y, direction.z],
                inner_angle: *inner_angle,
                outer_angle: *outer_angle,
                range: *range,
            },
        }
    }
}

impl From<&LightKindDesc> for LightKind {
    fn from(d: &LightKindDesc) -> Self {
        match d {
            LightKindDesc::Directional { direction } => LightKind::Directional {
                direction: vec3(direction[0], direction[1], direction[2]),
            },
            LightKindDesc::Point { range } => LightKind::Point { range: *range },
            LightKindDesc::Spot {
                direction,
                inner_angle,
                outer_angle,
                range,
            } => LightKind::Spot {
                direction: vec3(direction[0], direction[1], direction[2]),
                inner_angle: *inner_angle,
                outer_angle: *outer_angle,
                range: *range,
            },
        }
    }
}

/// A serializable collider shape.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ColliderShapeDesc {
    /// Box collider.
    Cuboid {
        /// Half extents `[x, y, z]`.
        half_extents: [f32; 3],
    },
    /// Sphere collider.
    Ball {
        /// Radius.
        radius: f32,
    },
    /// Capsule collider.
    Capsule {
        /// Half height.
        half_height: f32,
        /// Radius.
        radius: f32,
    },
    /// Cylinder collider.
    Cylinder {
        /// Half height.
        half_height: f32,
        /// Radius.
        radius: f32,
    },
}

impl From<&ColliderShape> for ColliderShapeDesc {
    fn from(s: &ColliderShape) -> Self {
        match s {
            ColliderShape::Cuboid { half_extents } => ColliderShapeDesc::Cuboid {
                half_extents: [half_extents.x, half_extents.y, half_extents.z],
            },
            ColliderShape::Ball { radius } => ColliderShapeDesc::Ball { radius: *radius },
            ColliderShape::Capsule {
                half_height,
                radius,
            } => ColliderShapeDesc::Capsule {
                half_height: *half_height,
                radius: *radius,
            },
            ColliderShape::Cylinder {
                half_height,
                radius,
            } => ColliderShapeDesc::Cylinder {
                half_height: *half_height,
                radius: *radius,
            },
        }
    }
}

impl From<&ColliderShapeDesc> for ColliderShape {
    fn from(d: &ColliderShapeDesc) -> Self {
        match d {
            ColliderShapeDesc::Cuboid { half_extents } => ColliderShape::Cuboid {
                half_extents: vec3(half_extents[0], half_extents[1], half_extents[2]),
            },
            ColliderShapeDesc::Ball { radius } => ColliderShape::Ball { radius: *radius },
            ColliderShapeDesc::Capsule {
                half_height,
                radius,
            } => ColliderShape::Capsule {
                half_height: *half_height,
                radius: *radius,
            },
            ColliderShapeDesc::Cylinder {
                half_height,
                radius,
            } => ColliderShape::Cylinder {
                half_height: *half_height,
                radius: *radius,
            },
        }
    }
}

/// A serializable text anchor.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TextAnchorDesc {
    /// Centred on the entity.
    Center,
    /// The top-left corner at the entity.
    TopLeft,
    /// The top edge centred.
    TopCenter,
    /// The top-right corner at the entity.
    TopRight,
    /// The middle-left edge at the entity.
    MiddleLeft,
    /// The middle-right edge at the entity.
    MiddleRight,
    /// The bottom-left corner at the entity.
    BottomLeft,
    /// The bottom edge centred.
    BottomCenter,
    /// The bottom-right corner at the entity.
    BottomRight,
}

impl From<&TextAnchor> for TextAnchorDesc {
    fn from(anchor: &TextAnchor) -> Self {
        match anchor {
            TextAnchor::Center => TextAnchorDesc::Center,
            TextAnchor::TopLeft => TextAnchorDesc::TopLeft,
            TextAnchor::TopCenter => TextAnchorDesc::TopCenter,
            TextAnchor::TopRight => TextAnchorDesc::TopRight,
            TextAnchor::MiddleLeft => TextAnchorDesc::MiddleLeft,
            TextAnchor::MiddleRight => TextAnchorDesc::MiddleRight,
            TextAnchor::BottomLeft => TextAnchorDesc::BottomLeft,
            TextAnchor::BottomCenter => TextAnchorDesc::BottomCenter,
            TextAnchor::BottomRight => TextAnchorDesc::BottomRight,
        }
    }
}

impl From<&TextAnchorDesc> for TextAnchor {
    fn from(desc: &TextAnchorDesc) -> Self {
        match desc {
            TextAnchorDesc::Center => TextAnchor::Center,
            TextAnchorDesc::TopLeft => TextAnchor::TopLeft,
            TextAnchorDesc::TopCenter => TextAnchor::TopCenter,
            TextAnchorDesc::TopRight => TextAnchor::TopRight,
            TextAnchorDesc::MiddleLeft => TextAnchor::MiddleLeft,
            TextAnchorDesc::MiddleRight => TextAnchor::MiddleRight,
            TextAnchorDesc::BottomLeft => TextAnchor::BottomLeft,
            TextAnchorDesc::BottomCenter => TextAnchor::BottomCenter,
            TextAnchorDesc::BottomRight => TextAnchor::BottomRight,
        }
    }
}

/// A serializable component.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ComponentDesc {
    /// A camera.
    Camera {
        /// The projection.
        projection: ProjectionDesc,
        /// Whether active.
        is_active: bool,
        /// Render priority.
        priority: i32,
        /// Clear color RGBA.
        clear_color: [f32; 4],
        /// HDR rendering flag.
        hdr: bool,
    },
    /// A light.
    Light {
        /// The light kind.
        kind: LightKindDesc,
        /// Linear RGB color.
        color: [f32; 3],
        /// Intensity.
        intensity: f32,
        /// Whether it casts shadows.
        cast_shadows: bool,
    },
    /// A mesh renderer.
    MeshRenderer {
        /// Mesh asset path.
        mesh: Option<String>,
        /// Material asset path.
        material: Option<String>,
        /// Whether it casts shadows.
        cast_shadows: bool,
        /// Whether it receives shadows.
        receive_shadows: bool,
    },
    /// A rigid body.
    RigidBody {
        /// The body type: `dynamic`, `fixed`, or `kinematic`.
        body_type: String,
        /// Mass.
        mass: f32,
        /// Restitution.
        restitution: f32,
        /// Friction.
        friction: f32,
        /// Gravity scale.
        gravity_scale: f32,
        /// Initial linear velocity.
        linear_velocity: [f32; 3],
        /// Initial angular velocity.
        angular_velocity: [f32; 3],
    },
    /// A collider.
    Collider {
        /// The shape.
        shape: ColliderShapeDesc,
        /// Local offset.
        offset: [f32; 3],
        /// Whether it is a sensor.
        is_sensor: bool,
        /// Friction.
        friction: f32,
        /// Restitution.
        restitution: f32,
    },
    /// An audio source.
    AudioSource {
        /// Clip asset path.
        clip: Option<String>,
        /// Volume.
        volume: f32,
        /// Pitch.
        pitch: f32,
        /// Whether it loops.
        looped: bool,
        /// Whether it plays on start.
        playing: bool,
        /// Whether it is spatial.
        spatial: bool,
        /// Maximum audible distance.
        max_distance: f32,
        /// Rolloff factor.
        rolloff: f32,
    },
    /// A JavaScript behaviour.
    Script {
        /// Inline source.
        source: Option<String>,
        /// Script asset path.
        path: Option<String>,
        /// Whether enabled.
        enabled: bool,
    },
    /// A text label.
    Text {
        /// The string to draw.
        text: String,
        /// Font asset path.
        font: Option<String>,
        /// Line height, in world units.
        size: f32,
        /// Linear RGBA colour.
        color: [f32; 4],
        /// Where the text sits relative to the entity's position.
        anchor: TextAnchorDesc,
        /// Whether geometry in front hides the text.
        depth_test: bool,
    },
    /// A component defined outside the engine, registered by name.
    ///
    /// Serialized as `{ "type": "custom", "name": "Health", "data": { ... } }`.
    Custom {
        /// The registered component name.
        name: String,
        /// The component's data (its fields).
        data: serde_json::Value,
    },
}

/// A reusable, serializable subtree that can be instantiated into a scene.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Prefab {
    /// The prefab name.
    pub name: String,
    /// The entities that make up the prefab.
    pub scene: SceneDesc,
}

impl Prefab {
    /// A prefab from a scene description.
    pub fn from_scene_desc(name: impl Into<String>, scene: SceneDesc) -> Self {
        Self {
            name: name.into(),
            scene,
        }
    }

    /// Instantiate the prefab into `world` under an optional parent.
    pub fn instantiate(
        &self,
        world: &mut World,
        assets: &AssetRegistry,
        parent: Option<Entity>,
    ) -> Vec<Entity> {
        instantiate_into(world, &self.scene, assets, parent)
    }

    /// Like [`Prefab::instantiate`], but with a component registry, so registered
    /// custom components in the prefab are instantiated.
    pub fn instantiate_with(
        &self,
        world: &mut World,
        assets: &AssetRegistry,
        parent: Option<Entity>,
        components: &ComponentRegistry,
    ) -> Vec<Entity> {
        instantiate_into_with(world, &self.scene, assets, parent, components)
    }

    /// Serialize to pretty JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("prefab serializes")
    }

    /// Parse from JSON.
    pub fn from_json(json: &str) -> Result<Self, SceneError> {
        serde_json::from_str(json).map_err(|e| SceneError::Parse(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::{AssetRegistry, Mesh};

    fn make_scene() -> (Scene, AssetRegistry) {
        let mut assets = AssetRegistry::with_defaults();
        let mesh_handle = assets.meshes.insert_named(
            "models/cube.gltf",
            Mesh {
                name: "cube".into(),
                ..Default::default()
            },
        );
        let mat_handle = assets.materials.handle("$default").unwrap();

        let mut scene = Scene::new("Main");
        let root = scene.world.spawn();
        scene.world.insert(root, Name::new("Root"));
        scene
            .world
            .insert(root, LocalTransform::from_translation(vec3(10.0, 0.0, 0.0)));

        let child = scene.world.spawn();
        scene.world.insert(child, Name::new("Child"));
        scene
            .world
            .insert(child, LocalTransform::from_translation(vec3(0.0, 5.0, 0.0)));
        scene.world.insert(
            child,
            MeshRenderer {
                mesh: mesh_handle,
                material: mat_handle,
                ..Default::default()
            },
        );
        scene
            .world
            .insert(child, Light::directional(vec3(0.0, -1.0, 0.0)));
        scene.world.insert(child, RigidBody::default());
        scene.world.insert(child, Collider::default());
        scene
            .world
            .insert(child, Script::from_source("function update() {}"));
        set_parent(&mut scene.world, child, Some(root));

        let cam = scene.world.spawn();
        scene.world.insert(cam, Name::new("Camera"));
        scene.world.insert(cam, LocalTransform::default());
        scene.world.insert(cam, Camera::default());

        scene.update_transforms();
        (scene, assets)
    }

    #[test]
    fn scene_json_is_byte_deterministic() {
        let (scene, assets) = make_scene();
        let first = scene.to_json(&assets);

        // The same in-memory state serializes identically every time.
        assert_eq!(first, scene.to_json(&assets));

        // A round trip through JSON must not reorder anything: the reload rebuilds
        // every internal map with a fresh hash seed, so any order that leaked into
        // the output would show up here.
        let reparsed = Scene::from_json(&first, &assets).expect("parse");
        assert_eq!(
            first,
            reparsed.to_json(&assets),
            "json round trip reordered"
        );

        // Likewise through the desc form.
        let desc = scene.to_desc(&assets);
        let from_desc = Scene::from_desc(&desc, &assets);
        assert_eq!(
            first,
            from_desc.to_json(&assets),
            "desc round trip reordered"
        );
    }

    #[test]
    fn scenes_carry_a_presentation_mode_that_round_trips() {
        assert_eq!(Scene::new("S").mode, SceneMode::ThreeD);
        assert!(SceneMode::TwoD.is_2d());
        assert!(!SceneMode::ThreeD.is_2d());
        assert_eq!(SceneMode::TwoD.as_str(), "2d");
        assert_eq!("3D".parse::<SceneMode>().unwrap(), SceneMode::ThreeD);
        assert!("2d".parse::<SceneMode>().is_ok());
        assert!("sideways".parse::<SceneMode>().is_err());

        let (mut scene, assets) = make_scene();
        scene.mode = SceneMode::TwoD;
        let json = scene.to_json(&assets);
        assert!(json.contains("\"mode\": \"2d\""), "mode is written: {json}");

        let reloaded = Scene::from_json(&json, &assets).expect("parse");
        assert_eq!(reloaded.mode, SceneMode::TwoD, "mode survives a reload");
        assert!(reloaded.is_2d());
    }

    #[test]
    fn a_scene_without_a_mode_field_loads_as_3d() {
        // Documents written before the field existed stay valid, defaulting to 3D.
        let legacy = r#"{ "name": "Legacy", "entities": [] }"#;
        let scene = Scene::from_json(legacy, &AssetRegistry::with_defaults()).expect("parse");
        assert_eq!(scene.mode, SceneMode::ThreeD);
    }

    #[test]
    fn builtin_primitives_resolve() {
        let assets = AssetRegistry::with_builtins();
        for name in ["cube", "sphere", "plane", "cylinder", "capsule", "torus"] {
            assert!(
                assets
                    .meshes
                    .handle(&format!("primitives/{name}"))
                    .is_some(),
                "primitive {name} is registered"
            );
        }
        // ...but `with_defaults` stays minimal (placeholders only).
        assert!(AssetRegistry::with_defaults().meshes.is_empty());

        // A scene referencing a built-in primitive loads.
        let json = r#"{
            "name": "Prim",
            "entities": [{
                "id": 0, "name": "Box",
                "transform": { "translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] },
                "components": [{
                    "type": "mesh_renderer", "mesh": "primitives/cube", "material": "$default",
                    "cast_shadows": true, "receive_shadows": true
                }]
            }]
        }"#;
        let scene = Scene::from_json(json, &assets).expect("a built-in primitive resolves");
        let (_, renderer) = scene.world.query::<MeshRenderer>().next().unwrap();
        assert_eq!(
            renderer.mesh,
            assets.meshes.handle("primitives/cube").unwrap()
        );
    }

    #[test]
    fn a_missing_asset_reference_is_a_load_error() {
        let assets = AssetRegistry::with_builtins();
        let json = |mesh: &str, material: &str| {
            format!(
                r#"{{
                    "name": "Broken",
                    "entities": [{{
                        "id": 0, "name": "Tri",
                        "transform": {{ "translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] }},
                        "components": [{{
                            "type": "mesh_renderer", "mesh": "{mesh}", "material": "{material}",
                            "cast_shadows": true, "receive_shadows": true
                        }}]
                    }}]
                }}"#
            )
        };

        let missing_mesh = json("assets/models/gone.glb#mesh/Torso", "$default");
        let Err(err) = Scene::from_json(&missing_mesh, &assets) else {
            panic!("expected a missing-asset error");
        };
        assert!(
            matches!(err, SceneError::MissingAsset(ref p) if p == "assets/models/gone.glb#mesh/Torso"),
            "got {err:?}"
        );
        assert!(err.to_string().contains("gone.glb#mesh/Torso"), "{err}");

        // A missing material is caught too, even when the mesh resolves.
        let missing_material = json("primitives/cube", "materials/nope");
        let Err(err) = Scene::from_json(&missing_material, &assets) else {
            panic!("expected a missing-material error");
        };
        assert!(matches!(err, SceneError::MissingAsset(ref p) if p == "materials/nope"));

        // The lenient `from_desc` still tolerates it (used for known-good snapshots).
        let desc: SceneDesc = serde_json::from_str(&missing_mesh).unwrap();
        let _ = Scene::from_desc(&desc, &assets);
    }

    #[test]
    fn a_missing_audio_clip_is_a_load_error() {
        let assets = AssetRegistry::with_builtins();
        let json = r#"{
            "name": "Snd",
            "entities": [{
                "id": 0, "name": "Speaker",
                "transform": { "translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] },
                "components": [{
                    "type": "audio_source", "clip": "audio/gone.wav",
                    "volume": 1.0, "pitch": 1.0, "looped": false, "playing": true,
                    "spatial": true, "max_distance": 20.0, "rolloff": 1.0
                }]
            }]
        }"#;
        let err = match Scene::from_json(json, &assets) {
            Ok(_) => panic!("expected a missing-asset error"),
            Err(err) => err,
        };
        assert!(matches!(err, SceneError::MissingAsset(ref p) if p == "audio/gone.wav"));
    }

    #[test]
    fn prefab_json_is_byte_deterministic() {
        let (scene, assets) = make_scene();
        let prefab = Prefab::from_scene_desc("P", scene.to_desc(&assets));
        let json = prefab.to_json();
        let back = Prefab::from_json(&json).expect("parse");
        assert_eq!(json, back.to_json(), "prefab round trip reordered");
    }

    #[test]
    fn set_parent_updates_both_sides() {
        let mut world = World::new();
        let a = world.spawn();
        let b = world.spawn();
        set_parent(&mut world, b, Some(a));
        assert_eq!(world.get::<Parent>(b).unwrap().0, a);
        assert_eq!(children_of(&world, a), vec![b]);
        set_parent(&mut world, b, None);
        assert!(world.get::<Parent>(b).is_none());
        assert!(children_of(&world, a).is_empty());
    }

    #[test]
    fn reparenting_removes_from_old_parent() {
        let mut world = World::new();
        let p1 = world.spawn();
        let p2 = world.spawn();
        let c = world.spawn();
        set_parent(&mut world, c, Some(p1));
        set_parent(&mut world, c, Some(p2));
        assert!(children_of(&world, p1).is_empty());
        assert_eq!(children_of(&world, p2), vec![c]);
    }

    #[test]
    fn transforms_propagate_through_hierarchy() {
        let mut world = World::new();
        let root = world.spawn();
        world.insert(root, LocalTransform::from_translation(vec3(10.0, 0.0, 0.0)));
        let child = world.spawn();
        world.insert(child, LocalTransform::from_translation(vec3(0.0, 5.0, 0.0)));
        set_parent(&mut world, child, Some(root));
        propagate_transforms(&mut world);
        let g = world.get::<GlobalTransform>(child).unwrap();
        assert!((g.position().x - 10.0).abs() < 1e-5);
        assert!((g.position().y - 5.0).abs() < 1e-5);
    }

    #[test]
    fn scene_round_trips_through_desc() {
        let (scene, assets) = make_scene();
        let desc = scene.to_desc(&assets);
        assert_eq!(desc.entities.len(), 3);
        let rebuilt = Scene::from_desc(&desc, &assets);
        let desc2 = rebuilt.to_desc(&assets);
        assert_eq!(desc, desc2, "scene -> desc -> scene -> desc must be stable");
    }

    #[test]
    fn scene_round_trips_through_json() {
        let (scene, assets) = make_scene();
        let json = scene.to_json(&assets);
        let parsed = Scene::from_json(&json, &assets).unwrap();
        assert_eq!(parsed.name, "Main");
        let desc_a = scene.to_desc(&assets);
        let desc_b = parsed.to_desc(&assets);
        assert_eq!(desc_a, desc_b);
    }

    #[test]
    fn parent_ids_survive_serialization() {
        let (scene, assets) = make_scene();
        let desc = scene.to_desc(&assets);
        let rebuilt = Scene::from_desc(&desc, &assets);
        // The child's global transform must still reflect the root offset.
        let child = rebuilt
            .world
            .entities_with::<Name>()
            .into_iter()
            .find(|e| rebuilt.world.get::<Name>(*e).unwrap().as_str() == "Child")
            .unwrap();
        let g = rebuilt.world.get::<GlobalTransform>(child).unwrap();
        assert!((g.position().x - 10.0).abs() < 1e-4);
        assert!((g.position().y - 5.0).abs() < 1e-4);
    }

    #[test]
    fn prefab_instantiate_into_scene() {
        let (scene, assets) = make_scene();
        let prefab = Prefab::from_scene_desc("Main", scene.to_desc(&assets));
        let json = prefab.to_json();
        let parsed = Prefab::from_json(&json).unwrap();

        let mut target = World::new();
        let spawned = parsed.instantiate(&mut target, &assets, None);
        assert_eq!(spawned.len(), 3);
        assert_eq!(target.entities().len(), 3);
    }

    #[test]
    fn instantiate_under_existing_parent() {
        let (scene, assets) = make_scene();
        let desc = scene.to_desc(&assets);
        let mut world = World::new();
        let root = world.spawn();
        world.insert(root, LocalTransform::default());
        let spawned = instantiate_into(&mut world, &desc, &assets, Some(root));
        // Every top-level entity (those without an in-doc parent) gains `root`
        // as its parent.
        let docs_without_parent = desc.entities.iter().filter(|e| e.parent.is_none()).count();
        let children = children_of(&world, root);
        assert_eq!(children.len(), docs_without_parent);
        assert_eq!(spawned.len(), desc.entities.len());
    }

    #[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Health {
        current: f32,
        max: f32,
    }

    impl crate::reflect::Component for Health {
        const NAME: &'static str = "Health";
    }

    fn health_registry() -> ComponentRegistry {
        let mut registry = ComponentRegistry::new();
        registry.register::<Health>();
        registry
    }

    #[test]
    fn builtin_reflection_round_trips_through_descriptors() {
        let assets = AssetRegistry::with_defaults();
        let registry = ComponentRegistry::new();
        let mut scene = Scene::new("R");
        let entity = scene.world.spawn();
        scene.world.insert(entity, LocalTransform::default());
        scene.world.insert(
            entity,
            RigidBody {
                mass: 4.0,
                ..RigidBody::default()
            },
        );

        assert!(has_reflected_component(
            &scene.world,
            entity,
            "RigidBody",
            &assets,
            &registry
        ));
        assert!(!has_reflected_component(
            &scene.world,
            entity,
            "Light",
            &assets,
            &registry
        ));

        let value = get_reflected_component(&scene.world, entity, "RigidBody", &assets, &registry)
            .expect("present");
        assert_eq!(value["type"], "rigid_body");
        assert_eq!(value["mass"], 4.0);

        // The descriptor is listed by `reflect_components` under its name.
        let listed = reflect_components(&scene.world, entity, &assets, &registry);
        assert!(listed.iter().any(|(name, _)| name == "RigidBody"));

        // Set a modified copy back.
        let mut next = value.clone();
        next["mass"] = serde_json::json!(8.0);
        set_reflected_component(
            &mut scene.world,
            entity,
            "RigidBody",
            &next,
            &assets,
            &registry,
        )
        .expect("set");
        assert_eq!(scene.world.get::<RigidBody>(entity).unwrap().mass, 8.0);

        // A descriptor of the wrong type is an error, not a silent wrong write.
        assert!(
            set_reflected_component(
                &mut scene.world,
                entity,
                "Light",
                &value,
                &assets,
                &registry
            )
            .is_err()
        );

        assert!(remove_reflected_component(
            &mut scene.world,
            entity,
            "RigidBody",
            &registry
        ));
        assert!(!has_reflected_component(
            &scene.world,
            entity,
            "RigidBody",
            &assets,
            &registry
        ));
    }

    #[test]
    fn a_custom_component_round_trips_through_json() {
        let assets = AssetRegistry::with_defaults();
        let registry = health_registry();

        let mut scene = Scene::new("Custom").with_component_registry(registry.clone());
        let entity = scene.world.spawn();
        scene.world.insert(entity, Name::new("Hero"));
        scene.world.insert(entity, LocalTransform::default());
        scene.world.insert(
            entity,
            Health {
                current: 3.0,
                max: 10.0,
            },
        );

        let json = scene.to_json(&assets);
        assert!(json.contains("\"type\": \"custom\""), "{json}");
        assert!(json.contains("\"name\": \"Health\""), "{json}");

        let reloaded = Scene::from_json_with(&json, &assets, &registry).expect("parse");
        let hero = reloaded
            .world
            .entities_with::<Health>()
            .into_iter()
            .next()
            .expect("the custom component survived");
        assert_eq!(
            reloaded.world.get::<Health>(hero),
            Some(&Health {
                current: 3.0,
                max: 10.0
            })
        );
        // A round trip through JSON is byte-identical.
        assert_eq!(json, reloaded.to_json(&assets));
    }

    #[test]
    fn an_unregistered_custom_component_is_a_load_error() {
        let json = r#"{
            "name": "S",
            "entities": [{
                "id": 0,
                "transform": { "translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] },
                "components": [
                    { "type": "custom", "name": "Nope", "data": {} }
                ]
            }]
        }"#;
        let assets = AssetRegistry::with_defaults();
        // Without a registry it is unknown; with one that lacks it, too.
        let Err(err) = Scene::from_json(json, &assets) else {
            panic!("expected an unknown-component error");
        };
        assert!(
            matches!(err, SceneError::UnknownComponent(ref n) if n == "Nope"),
            "{err:?}"
        );
        let Err(err) = Scene::from_json_with(json, &assets, &health_registry()) else {
            panic!("expected an unknown-component error");
        };
        assert!(matches!(err, SceneError::UnknownComponent(_)), "{err:?}");
    }
}
