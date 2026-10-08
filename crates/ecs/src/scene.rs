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

/// A named [`World`].
#[derive(Default)]
pub struct Scene {
    /// The scene name.
    pub name: String,
    /// The entities and components.
    pub world: World,
}

impl Scene {
    /// An empty scene with a name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            world: World::new(),
        }
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
            let components = component_descs(&self.world, *entity, assets);
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
            entities: descs,
        }
    }

    /// Build a scene from its serializable form, resolving asset paths via
    /// `assets`.
    pub fn from_desc(desc: &SceneDesc, assets: &AssetRegistry) -> Scene {
        let mut scene = Scene::new(desc.name.clone());
        instantiate_into(&mut scene.world, desc, assets, None);
        scene.update_transforms();
        scene
    }

    /// Serialize the scene to pretty JSON, resolving asset paths via `assets`.
    pub fn to_json(&self, assets: &AssetRegistry) -> String {
        serde_json::to_string_pretty(&self.to_desc(assets)).expect("scene serializes")
    }

    /// Parse a scene from JSON and instantiate it.
    pub fn from_json(json: &str, assets: &AssetRegistry) -> Result<Scene, SceneError> {
        let desc: SceneDesc =
            serde_json::from_str(json).map_err(|e| SceneError::Parse(e.to_string()))?;
        Ok(Scene::from_desc(&desc, assets))
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
}

impl std::fmt::Display for SceneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SceneError::Parse(msg) => write!(f, "scene parse error: {msg}"),
            SceneError::Version(v) => write!(f, "unsupported scene format version: {v}"),
        }
    }
}

impl std::error::Error for SceneError {}

/// Spawn the entities in `desc` into `world` under an optional `parent`.
///
/// Returns the entities in document order.
pub fn instantiate_into(
    world: &mut World,
    desc: &SceneDesc,
    assets: &AssetRegistry,
    parent: Option<Entity>,
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
            apply_component(world, entity, component, assets);
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

fn component_descs(world: &World, entity: Entity, assets: &AssetRegistry) -> Vec<ComponentDesc> {
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
    out
}

fn apply_component(
    world: &mut World,
    entity: Entity,
    desc: &ComponentDesc,
    assets: &AssetRegistry,
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
}
