//! The serializable command API.
//!
//! Everything the editor can do to a scene is expressible as an
//! [`EditorCommand`]. Because the command is a plain serde type, an agent can
//! write a JSON array of commands, feed it to the `noctis-editor` CLI, and get
//! a fully authored scene back — no GUI, no clicks.
//!
//! The command set mirrors the imperative [`crate::Editor`] methods one-to-one,
//! so the two entry points stay in lockstep.

use ecs::assets::{AlphaMode, AssetId, Material, Mesh, Texture, TextureFormat, Vertex};
use ecs::components::{BodyType, ColliderShape, LightKind, Projection};
use math::{Quat, Vec3, radians, vec3, zero3};
use serde::{Deserialize, Serialize};

// The engine exposes its stable component index under a nested module; import
// the enum directly so the rest of the module reads cleanly.
use ecs::components::component_index::ComponentKind;

// ---------------------------------------------------------------------------
// Small conversion helpers
// ---------------------------------------------------------------------------

/// Convert a `[f32; 3]` literal to a [`Vec3`].
pub fn arr3(a: [f32; 3]) -> Vec3 {
    vec3(a[0], a[1], a[2])
}

/// Convert a [`Vec3`] to a `[f32; 3]` literal.
pub fn to_arr3(v: Vec3) -> [f32; 3] {
    [v.x, v.y, v.z]
}

/// Convert a `[x, y, z, w]` literal to a [`Quat`].
pub fn arr4(q: [f32; 4]) -> Quat {
    Quat::new(q[0], q[1], q[2], q[3])
}

/// Convert a [`Quat`] to a `[x, y, z, w]` literal.
pub fn to_arr4(q: Quat) -> [f32; 4] {
    [q.x, q.y, q.z, q.w]
}

// ---------------------------------------------------------------------------
// Entity and asset references
// ---------------------------------------------------------------------------

/// A reference to an entity inside a command.
///
/// Agents usually do not know an entity's slot index, so commands may refer to
/// entities either by their stable `(index, generation)` pair or by name.
///
/// ```
/// # use editor::EntityRef;
/// let by_name: EntityRef = serde_json::from_str(r#"{ "name": "Sun" }"#).unwrap();
/// let by_id: EntityRef = serde_json::from_str(r#"{ "index": 0, "generation": 1 }"#).unwrap();
/// assert!(matches!(by_name, EntityRef::Name { .. }));
/// assert!(matches!(by_id, EntityRef::Id { .. }));
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EntityRef {
    /// Reference by slot index and generation.
    Id {
        /// The entity slot index.
        index: u32,
        /// The entity slot generation.
        generation: u32,
    },
    /// Reference by name (the first matching entity is used).
    Name {
        /// The entity's name.
        name: String,
    },
}

impl EntityRef {
    /// A by-name reference.
    pub fn named(name: impl Into<String>) -> Self {
        Self::Name { name: name.into() }
    }

    /// A by-id reference.
    pub fn id(index: u32, generation: u32) -> Self {
        Self::Id { index, generation }
    }

    /// The name this refers to, if it is a by-name reference.
    pub fn as_name(&self) -> Option<&str> {
        match self {
            EntityRef::Name { name } => Some(name),
            EntityRef::Id { .. } => None,
        }
    }
}

impl From<ecs::world::Entity> for EntityRef {
    fn from(entity: ecs::world::Entity) -> Self {
        Self::Id {
            index: entity.index(),
            generation: entity.generation(),
        }
    }
}

/// A reference to an asset, either by registration path or by raw id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AssetRef {
    /// Reference by the path an asset was registered under.
    Path {
        /// The registration path.
        path: String,
    },
    /// Reference by the numeric asset id.
    Id {
        /// The asset id.
        id: u32,
    },
}

impl AssetRef {
    /// A by-path reference.
    pub fn path(path: impl Into<String>) -> Self {
        Self::Path { path: path.into() }
    }

    /// A by-id reference.
    pub fn id(id: u32) -> Self {
        Self::Id { id }
    }
}

// ---------------------------------------------------------------------------
// Component names
// ---------------------------------------------------------------------------

/// The canonical name of a removable component, mirroring
/// [`ecs::components::component_index::ComponentKind`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComponentName {
    /// [`ecs::components::LocalTransform`]
    LocalTransform,
    /// [`ecs::components::GlobalTransform`]
    GlobalTransform,
    /// [`ecs::components::Name`]
    Name,
    /// [`ecs::components::Active`]
    Active,
    /// [`ecs::components::Visible`]
    Visible,
    /// [`ecs::components::Camera`]
    Camera,
    /// [`ecs::components::Light`]
    Light,
    /// [`ecs::components::MeshRenderer`]
    MeshRenderer,
    /// [`ecs::components::RigidBody`]
    RigidBody,
    /// [`ecs::components::Collider`]
    Collider,
    /// [`ecs::components::AudioSource`]
    AudioSource,
    /// [`ecs::components::Script`]
    Script,
    /// [`ecs::components::Text`]
    Text,
}

impl ComponentName {
    /// The engine's stable component index for this name.
    pub fn kind(self) -> ComponentKind {
        match self {
            ComponentName::LocalTransform => ComponentKind::LocalTransform,
            ComponentName::GlobalTransform => ComponentKind::GlobalTransform,
            ComponentName::Name => ComponentKind::Name,
            ComponentName::Active => ComponentKind::Active,
            ComponentName::Visible => ComponentKind::Visible,
            ComponentName::Camera => ComponentKind::Camera,
            ComponentName::Light => ComponentKind::Light,
            ComponentName::MeshRenderer => ComponentKind::MeshRenderer,
            ComponentName::RigidBody => ComponentKind::RigidBody,
            ComponentName::Collider => ComponentKind::Collider,
            ComponentName::AudioSource => ComponentKind::AudioSource,
            ComponentName::Script => ComponentKind::Script,
            ComponentName::Text => ComponentKind::Text,
        }
    }

    /// The canonical string name.
    pub fn as_str(self) -> &'static str {
        self.kind().name()
    }

    /// Look a component name up by its canonical string.
    pub fn from_str_name(name: &str) -> Option<Self> {
        Some(match ComponentKind::from_name(name)? {
            ComponentKind::LocalTransform => ComponentName::LocalTransform,
            ComponentKind::GlobalTransform => ComponentName::GlobalTransform,
            ComponentKind::Name => ComponentName::Name,
            ComponentKind::Active => ComponentName::Active,
            ComponentKind::Visible => ComponentName::Visible,
            ComponentKind::Camera => ComponentName::Camera,
            ComponentKind::Light => ComponentName::Light,
            ComponentKind::MeshRenderer => ComponentName::MeshRenderer,
            ComponentKind::RigidBody => ComponentName::RigidBody,
            ComponentKind::Collider => ComponentName::Collider,
            ComponentKind::AudioSource => ComponentName::AudioSource,
            ComponentKind::Script => ComponentName::Script,
            ComponentKind::Text => ComponentName::Text,
        })
    }
}

// ---------------------------------------------------------------------------
// Serde defaults
// ---------------------------------------------------------------------------

fn default_true() -> bool {
    true
}
fn default_one() -> f32 {
    1.0
}
fn default_half() -> f32 {
    0.5
}
fn default_fov_deg() -> f32 {
    60.0
}
fn default_near() -> f32 {
    0.1
}
fn default_far() -> f32 {
    1000.0
}
fn default_range() -> f32 {
    50.0
}
fn default_down() -> [f32; 3] {
    [0.0, -1.0, 0.0]
}
fn default_white3() -> [f32; 3] {
    [1.0, 1.0, 1.0]
}
fn default_white4() -> [f32; 4] {
    [1.0, 1.0, 1.0, 1.0]
}
fn default_clear_color() -> [f32; 4] {
    [0.02, 0.02, 0.03, 1.0]
}
fn default_inner_angle_deg() -> f32 {
    15.0
}
fn default_outer_angle_deg() -> f32 {
    30.0
}
fn default_half_extents() -> [f32; 3] {
    [0.5, 0.5, 0.5]
}

// ---------------------------------------------------------------------------
// Camera
// ---------------------------------------------------------------------------

/// A camera projection specification.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProjectionSpec {
    /// A perspective projection; `fov_y_deg` is the vertical field of view.
    Perspective {
        /// Vertical field of view, degrees.
        #[serde(default = "default_fov_deg")]
        fov_y_deg: f32,
        /// Near plane.
        #[serde(default = "default_near")]
        near: f32,
        /// Far plane.
        #[serde(default = "default_far")]
        far: f32,
    },
    /// An orthographic projection; `height` is the visible world height.
    Orthographic {
        /// Visible world height.
        #[serde(default = "default_one")]
        height: f32,
        /// Near plane.
        #[serde(default = "default_near")]
        near: f32,
        /// Far plane.
        #[serde(default = "default_far")]
        far: f32,
    },
}

impl Default for ProjectionSpec {
    fn default() -> Self {
        ProjectionSpec::Perspective {
            fov_y_deg: default_fov_deg(),
            near: default_near(),
            far: default_far(),
        }
    }
}

impl From<ProjectionSpec> for Projection {
    fn from(spec: ProjectionSpec) -> Self {
        match spec {
            ProjectionSpec::Perspective {
                fov_y_deg,
                near,
                far,
            } => Projection::Perspective {
                fov_y: radians(fov_y_deg),
                near,
                far,
            },
            ProjectionSpec::Orthographic { height, near, far } => {
                Projection::Orthographic { height, near, far }
            }
        }
    }
}

/// Parameters for [`EditorCommand::AddCamera`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CameraParams {
    /// Projection mode.
    #[serde(default)]
    pub projection: ProjectionSpec,
    /// Whether the camera renders.
    #[serde(default = "default_true")]
    pub is_active: bool,
    /// Render priority (lower renders first).
    #[serde(default)]
    pub priority: i32,
    /// Linear RGBA clear color.
    #[serde(default = "default_clear_color")]
    pub clear_color: [f32; 4],
    /// Whether to render through the HDR pipeline.
    #[serde(default = "default_true")]
    pub hdr: bool,
}

impl Default for CameraParams {
    fn default() -> Self {
        Self {
            projection: ProjectionSpec::default(),
            is_active: true,
            priority: 0,
            clear_color: default_clear_color(),
            hdr: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Light
// ---------------------------------------------------------------------------

/// A light shape specification.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LightSpec {
    /// A directional (sun) light.
    Directional {
        /// Direction from the light toward the scene.
        #[serde(default = "default_down")]
        direction: [f32; 3],
    },
    /// A point light.
    Point {
        /// Falloff range.
        #[serde(default = "default_range")]
        range: f32,
    },
    /// A spot light.
    Spot {
        /// Direction from the light toward the scene.
        #[serde(default = "default_down")]
        direction: [f32; 3],
        /// Inner cone angle, degrees.
        #[serde(default = "default_inner_angle_deg")]
        inner_angle_deg: f32,
        /// Outer cone angle, degrees.
        #[serde(default = "default_outer_angle_deg")]
        outer_angle_deg: f32,
        /// Falloff range.
        #[serde(default = "default_range")]
        range: f32,
    },
}

impl Default for LightSpec {
    fn default() -> Self {
        LightSpec::Directional {
            direction: default_down(),
        }
    }
}

impl From<LightSpec> for LightKind {
    fn from(spec: LightSpec) -> Self {
        match spec {
            LightSpec::Directional { direction } => LightKind::Directional {
                direction: arr3(direction),
            },
            LightSpec::Point { range } => LightKind::Point { range },
            LightSpec::Spot {
                direction,
                inner_angle_deg,
                outer_angle_deg,
                range,
            } => LightKind::Spot {
                direction: arr3(direction),
                inner_angle: radians(inner_angle_deg),
                outer_angle: radians(outer_angle_deg),
                range,
            },
        }
    }
}

/// Parameters for [`EditorCommand::AddLight`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LightParams {
    /// The light shape.
    #[serde(default)]
    pub kind: LightSpec,
    /// Linear RGB color.
    #[serde(default = "default_white3")]
    pub color: [f32; 3],
    /// Intensity.
    #[serde(default = "default_one")]
    pub intensity: f32,
    /// Whether the light casts shadows.
    #[serde(default = "default_true")]
    pub cast_shadows: bool,
}

impl Default for LightParams {
    fn default() -> Self {
        Self {
            kind: LightSpec::default(),
            color: default_white3(),
            intensity: 1.0,
            cast_shadows: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Mesh renderer
// ---------------------------------------------------------------------------

/// Parameters for [`EditorCommand::SetMeshRenderer`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MeshRendererParams {
    /// The mesh asset to render.
    #[serde(default)]
    pub mesh: Option<AssetRef>,
    /// The material to shade with.
    #[serde(default)]
    pub material: Option<AssetRef>,
    /// Whether the mesh casts shadows.
    #[serde(default = "default_true")]
    pub cast_shadows: bool,
    /// Whether the mesh receives shadows.
    #[serde(default = "default_true")]
    pub receive_shadows: bool,
}

// ---------------------------------------------------------------------------
// Physics
// ---------------------------------------------------------------------------

/// A rigid body simulation type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyTypeSpec {
    /// Fully simulated.
    #[default]
    Dynamic,
    /// Immovable.
    Fixed,
    /// Moved by code, pushes dynamic bodies.
    Kinematic,
}

impl From<BodyTypeSpec> for BodyType {
    fn from(spec: BodyTypeSpec) -> Self {
        match spec {
            BodyTypeSpec::Dynamic => BodyType::Dynamic,
            BodyTypeSpec::Fixed => BodyType::Fixed,
            BodyTypeSpec::Kinematic => BodyType::Kinematic,
        }
    }
}

/// A collider shape specification.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ColliderShapeSpec {
    /// A box, half extents per axis.
    Cuboid {
        /// Half extents per axis.
        #[serde(default = "default_half_extents")]
        half_extents: [f32; 3],
    },
    /// A sphere.
    Ball {
        /// Radius.
        #[serde(default = "default_half")]
        radius: f32,
    },
    /// A capsule aligned to the local Y axis.
    Capsule {
        /// Half the height of the cylindrical section.
        #[serde(default = "default_half")]
        half_height: f32,
        /// Radius.
        #[serde(default = "default_half")]
        radius: f32,
    },
    /// A cylinder aligned to the local Y axis.
    Cylinder {
        /// Half height.
        #[serde(default = "default_half")]
        half_height: f32,
        /// Radius.
        #[serde(default = "default_half")]
        radius: f32,
    },
}

impl Default for ColliderShapeSpec {
    fn default() -> Self {
        ColliderShapeSpec::Cuboid {
            half_extents: default_half_extents(),
        }
    }
}

impl From<ColliderShapeSpec> for ColliderShape {
    fn from(spec: ColliderShapeSpec) -> Self {
        match spec {
            ColliderShapeSpec::Cuboid { half_extents } => ColliderShape::Cuboid {
                half_extents: arr3(half_extents),
            },
            ColliderShapeSpec::Ball { radius } => ColliderShape::Ball { radius },
            ColliderShapeSpec::Capsule {
                half_height,
                radius,
            } => ColliderShape::Capsule {
                half_height,
                radius,
            },
            ColliderShapeSpec::Cylinder {
                half_height,
                radius,
            } => ColliderShape::Cylinder {
                half_height,
                radius,
            },
        }
    }
}

/// Parameters for [`EditorCommand::AddRigidBody`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RigidBodyParams {
    /// Simulation type.
    #[serde(default)]
    pub body_type: BodyTypeSpec,
    /// Mass, kilograms.
    #[serde(default = "default_one")]
    pub mass: f32,
    /// Bounciness.
    #[serde(default)]
    pub restitution: f32,
    /// Friction coefficient.
    #[serde(default = "default_half")]
    pub friction: f32,
    /// Per-body gravity multiplier.
    #[serde(default = "default_one")]
    pub gravity_scale: f32,
    /// Initial linear velocity.
    #[serde(default)]
    pub linear_velocity: [f32; 3],
    /// Initial angular velocity.
    #[serde(default)]
    pub angular_velocity: [f32; 3],
}

impl Default for RigidBodyParams {
    fn default() -> Self {
        Self {
            body_type: BodyTypeSpec::Dynamic,
            mass: 1.0,
            restitution: 0.2,
            friction: 0.5,
            gravity_scale: 1.0,
            linear_velocity: [0.0, 0.0, 0.0],
            angular_velocity: [0.0, 0.0, 0.0],
        }
    }
}

/// Parameters for [`EditorCommand::AddCollider`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColliderParams {
    /// The collider shape.
    #[serde(default)]
    pub shape: ColliderShapeSpec,
    /// Local offset from the entity origin.
    #[serde(default)]
    pub offset: [f32; 3],
    /// Whether the collider is a trigger.
    #[serde(default)]
    pub is_sensor: bool,
    /// Friction override.
    #[serde(default = "default_half")]
    pub friction: f32,
    /// Restitution override.
    #[serde(default)]
    pub restitution: f32,
}

impl Default for ColliderParams {
    fn default() -> Self {
        Self {
            shape: ColliderShapeSpec::default(),
            offset: [0.0, 0.0, 0.0],
            is_sensor: false,
            friction: 0.5,
            restitution: 0.2,
        }
    }
}

// ---------------------------------------------------------------------------
// Audio
// ---------------------------------------------------------------------------

/// Parameters for [`EditorCommand::AddAudioSource`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioSourceParams {
    /// The audio clip to play.
    #[serde(default)]
    pub clip: Option<AssetRef>,
    /// Volume multiplier.
    #[serde(default = "default_one")]
    pub volume: f32,
    /// Playback rate multiplier.
    #[serde(default = "default_one")]
    pub pitch: f32,
    /// Whether playback loops.
    #[serde(default)]
    pub looped: bool,
    /// Whether playback starts active.
    #[serde(default)]
    pub playing: bool,
    /// Whether the sound is spatialized.
    #[serde(default)]
    pub spatial: bool,
    /// Distance at which the sound is inaudible.
    #[serde(default = "default_range")]
    pub max_distance: f32,
    /// Rolloff factor.
    #[serde(default = "default_one")]
    pub rolloff: f32,
}

impl Default for AudioSourceParams {
    fn default() -> Self {
        Self {
            clip: None,
            volume: 1.0,
            pitch: 1.0,
            looped: false,
            playing: false,
            spatial: false,
            max_distance: 50.0,
            rolloff: 1.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Materials
// ---------------------------------------------------------------------------

/// How a material treats its alpha channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlphaModeSpec {
    /// Alpha is ignored.
    #[default]
    Opaque,
    /// Alpha below a cutoff is discarded.
    Mask,
    /// Alpha blends.
    Blend,
}

impl From<AlphaModeSpec> for AlphaMode {
    fn from(spec: AlphaModeSpec) -> Self {
        match spec {
            AlphaModeSpec::Opaque => AlphaMode::Opaque,
            AlphaModeSpec::Mask => AlphaMode::Mask,
            AlphaModeSpec::Blend => AlphaMode::Blend,
        }
    }
}

/// Parameters for [`EditorCommand::RegisterMaterial`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaterialParams {
    /// Linear RGBA base color.
    #[serde(default = "default_white4")]
    pub base_color: [f32; 4],
    /// Metalness.
    #[serde(default)]
    pub metallic: f32,
    /// Roughness.
    #[serde(default = "default_half")]
    pub roughness: f32,
    /// Linear emissive color.
    #[serde(default)]
    pub emissive: [f32; 3],
    /// Emissive multiplier.
    #[serde(default = "default_one")]
    pub emissive_strength: f32,
    /// Alpha handling.
    #[serde(default)]
    pub alpha_mode: AlphaModeSpec,
    /// Whether back faces render.
    #[serde(default)]
    pub double_sided: bool,
}

impl Default for MaterialParams {
    fn default() -> Self {
        Self {
            base_color: default_white4(),
            metallic: 0.0,
            roughness: 0.5,
            emissive: [0.0, 0.0, 0.0],
            emissive_strength: 1.0,
            alpha_mode: AlphaModeSpec::Opaque,
            double_sided: false,
        }
    }
}

// ---------------------------------------------------------------------------
// The command enum
// ---------------------------------------------------------------------------

/// Every authoring operation, in a form that round-trips through JSON.
///
/// The JSON tag is `op`, e.g.:
///
/// ```json
/// { "op": "spawn", "name": "Sun", "parent": null }
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum EditorCommand {
    /// Replace the current scene with a fresh, empty one of the given name.
    NewScene {
        /// The new scene name.
        name: String,
    },
    /// Spawn an entity, optionally parented.
    Spawn {
        /// The new entity's name.
        name: String,
        /// The parent, if any.
        #[serde(default)]
        parent: Option<EntityRef>,
    },
    /// Destroy an entity and its descendants.
    Destroy {
        /// The entity to destroy.
        entity: EntityRef,
    },
    /// Duplicate an entity and its descendants.
    Duplicate {
        /// The entity to duplicate.
        entity: EntityRef,
    },
    /// Rename an entity.
    Rename {
        /// The entity to rename.
        entity: EntityRef,
        /// The new name.
        name: String,
    },
    /// Set an entity's active flag.
    SetActive {
        /// The entity.
        entity: EntityRef,
        /// The new value.
        active: bool,
    },
    /// Set an entity's visible flag.
    SetVisible {
        /// The entity.
        entity: EntityRef,
        /// The new value.
        visible: bool,
    },
    /// Reparent an entity (or unparent it).
    SetParent {
        /// The entity to reparent.
        entity: EntityRef,
        /// The new parent, or `null` to unparent.
        #[serde(default)]
        parent: Option<EntityRef>,
    },
    /// Set an entity's local translation.
    SetTranslation {
        /// The entity.
        entity: EntityRef,
        /// The new translation.
        translation: [f32; 3],
    },
    /// Add a delta to an entity's local translation.
    Translate {
        /// The entity.
        entity: EntityRef,
        /// The delta to add.
        delta: [f32; 3],
    },
    /// Set an entity's local rotation from XYZ Euler angles (degrees).
    SetRotationEuler {
        /// The entity.
        entity: EntityRef,
        /// `[roll_x, pitch_y, yaw_z]` in degrees.
        degrees: [f32; 3],
    },
    /// Set an entity's local rotation from a quaternion.
    SetRotationQuat {
        /// The entity.
        entity: EntityRef,
        /// `[x, y, z, w]`.
        quaternion: [f32; 4],
    },
    /// Set an entity's local scale.
    SetScale {
        /// The entity.
        entity: EntityRef,
        /// The new scale.
        scale: [f32; 3],
    },
    /// Orient an entity so its `-Z` axis points at `target`.
    LookAt {
        /// The entity.
        entity: EntityRef,
        /// The world-space point to look at.
        target: [f32; 3],
        /// The up reference (defaults to `+Y`).
        #[serde(default)]
        up: Option<[f32; 3]>,
    },
    /// Add or replace a camera component.
    AddCamera {
        /// The entity.
        entity: EntityRef,
        /// Camera parameters.
        #[serde(default)]
        params: CameraParams,
    },
    /// Add or replace a light component.
    AddLight {
        /// The entity.
        entity: EntityRef,
        /// Light parameters.
        #[serde(default)]
        params: LightParams,
    },
    /// Add or replace a mesh renderer component.
    SetMeshRenderer {
        /// The entity.
        entity: EntityRef,
        /// Mesh renderer parameters.
        #[serde(default)]
        params: MeshRendererParams,
    },
    /// Add or replace a rigid body component.
    AddRigidBody {
        /// The entity.
        entity: EntityRef,
        /// Rigid body parameters.
        #[serde(default)]
        params: RigidBodyParams,
    },
    /// Add or replace a collider component.
    AddCollider {
        /// The entity.
        entity: EntityRef,
        /// Collider parameters.
        #[serde(default)]
        params: ColliderParams,
    },
    /// Add or replace an audio source component.
    AddAudioSource {
        /// The entity.
        entity: EntityRef,
        /// Audio source parameters.
        #[serde(default)]
        params: AudioSourceParams,
    },
    /// Attach a script component from inline source.
    AttachScriptSource {
        /// The entity.
        entity: EntityRef,
        /// The inline JavaScript source.
        source: String,
    },
    /// Attach a script component from a script asset path.
    AttachScriptPath {
        /// The entity.
        entity: EntityRef,
        /// The script asset path.
        path: String,
    },
    /// Remove a component (a no-op if the entity never had it).
    RemoveComponent {
        /// The entity.
        entity: EntityRef,
        /// The component to remove.
        component: ComponentName,
    },
    /// Register a triangle-indexed mesh from raw positions.
    RegisterMesh {
        /// The registration path.
        name: String,
        /// Vertex positions.
        positions: Vec<[f32; 3]>,
        /// Triangle indices into `positions`.
        indices: Vec<u32>,
    },
    /// Register a material.
    RegisterMaterial {
        /// The registration path.
        name: String,
        /// Material parameters.
        #[serde(default)]
        params: MaterialParams,
    },
    /// Register a 1x1 solid-color texture.
    RegisterTexture {
        /// The registration path.
        name: String,
        /// The RGBA fill color.
        color: [u8; 4],
    },
    /// Import a glTF file and instantiate its node hierarchy.
    ImportGltf {
        /// Path to a `.gltf` or `.glb` file.
        path: String,
        /// The parent to attach the imported roots to, if any.
        #[serde(default)]
        parent: Option<EntityRef>,
    },
    /// Build a prefab from a set of entities and store it under `name`.
    CreatePrefab {
        /// The prefab name.
        name: String,
        /// The entities to include.
        entities: Vec<EntityRef>,
    },
    /// Instantiate a previously created prefab.
    InstantiatePrefab {
        /// The prefab name.
        name: String,
        /// The parent to attach the instantiated root to, if any.
        #[serde(default)]
        parent: Option<EntityRef>,
    },
    /// Add an entity to the selection.
    Select {
        /// The entity to select.
        entity: EntityRef,
    },
    /// Clear the selection.
    ClearSelection,
}

impl EditorCommand {
    /// The canonical `op` name of this command.
    pub fn op_name(&self) -> &'static str {
        use EditorCommand::*;
        match self {
            NewScene { .. } => "new_scene",
            Spawn { .. } => "spawn",
            Destroy { .. } => "destroy",
            Duplicate { .. } => "duplicate",
            Rename { .. } => "rename",
            SetActive { .. } => "set_active",
            SetVisible { .. } => "set_visible",
            SetParent { .. } => "set_parent",
            SetTranslation { .. } => "set_translation",
            Translate { .. } => "translate",
            SetRotationEuler { .. } => "set_rotation_euler",
            SetRotationQuat { .. } => "set_rotation_quat",
            SetScale { .. } => "set_scale",
            LookAt { .. } => "look_at",
            AddCamera { .. } => "add_camera",
            AddLight { .. } => "add_light",
            SetMeshRenderer { .. } => "set_mesh_renderer",
            AddRigidBody { .. } => "add_rigid_body",
            AddCollider { .. } => "add_collider",
            AddAudioSource { .. } => "add_audio_source",
            AttachScriptSource { .. } => "attach_script_source",
            AttachScriptPath { .. } => "attach_script_path",
            RemoveComponent { .. } => "remove_component",
            RegisterMesh { .. } => "register_mesh",
            RegisterMaterial { .. } => "register_material",
            RegisterTexture { .. } => "register_texture",
            ImportGltf { .. } => "import_gltf",
            CreatePrefab { .. } => "create_prefab",
            InstantiatePrefab { .. } => "instantiate_prefab",
            Select { .. } => "select",
            ClearSelection => "clear_selection",
        }
    }

    /// Whether applying this command should push an undo checkpoint.
    ///
    /// Commands that only mutate editor bookkeeping (asset registration,
    /// prefab construction, selection) are not undoable, so they do not create
    /// history entries.
    pub fn records_history(&self) -> bool {
        use EditorCommand::*;
        matches!(
            self,
            NewScene { .. }
                | Spawn { .. }
                | Destroy { .. }
                | Duplicate { .. }
                | Rename { .. }
                | SetActive { .. }
                | SetVisible { .. }
                | SetParent { .. }
                | SetTranslation { .. }
                | Translate { .. }
                | SetRotationEuler { .. }
                | SetRotationQuat { .. }
                | SetScale { .. }
                | LookAt { .. }
                | AddCamera { .. }
                | AddLight { .. }
                | SetMeshRenderer { .. }
                | AddRigidBody { .. }
                | AddCollider { .. }
                | AddAudioSource { .. }
                | AttachScriptSource { .. }
                | AttachScriptPath { .. }
                | RemoveComponent { .. }
                | ImportGltf { .. }
                | InstantiatePrefab { .. }
        )
    }
}

// ---------------------------------------------------------------------------
// Outcome
// ---------------------------------------------------------------------------

/// What a successfully applied command produced.
#[derive(Clone, Debug, PartialEq)]
pub enum CommandOutcome {
    /// The command produced no value (a mutation or a no-op).
    None,
    /// The command produced a single entity.
    Entity(ecs::world::Entity),
    /// The command produced several entities (glTF import, prefab instantiation).
    Entities(Vec<ecs::world::Entity>),
    /// The command registered or looked up an asset.
    Asset(AssetId),
    /// The command produced or referenced a named prefab.
    Prefab(String),
}

// ---------------------------------------------------------------------------
// Mesh helpers
// ---------------------------------------------------------------------------

/// Build a [`Mesh`] from raw positions and triangle indices, computing flat-ish
/// vertex normals from the incident faces. UVs are zeroed and tangents point
/// along `+Z`.
///
/// This is the authoring convenience behind [`EditorCommand::RegisterMesh`].
pub fn mesh_from_positions(
    name: impl Into<String>,
    positions: &[[f32; 3]],
    indices: &[u32],
) -> Result<Mesh, crate::EditorError> {
    use crate::EditorError;
    if positions.is_empty() {
        return Err(EditorError::Invalid(
            "register_mesh: positions must not be empty".into(),
        ));
    }
    if indices.is_empty() || !indices.len().is_multiple_of(3) {
        return Err(EditorError::Invalid(
            "register_mesh: indices must be a non-empty multiple of 3".into(),
        ));
    }
    for &i in indices {
        if i as usize >= positions.len() {
            return Err(EditorError::Invalid(format!(
                "register_mesh: index {i} is out of range for {} positions",
                positions.len()
            )));
        }
    }

    let mut acc = vec![zero3(); positions.len()];
    for tri in indices.as_chunks::<3>().0 {
        let a = arr3(positions[tri[0] as usize]);
        let b = arr3(positions[tri[1] as usize]);
        let c = arr3(positions[tri[2] as usize]);
        let n = normalized_or(cross3(sub3(b, a), sub3(c, a)), vec3(0.0, 1.0, 0.0));
        for &i in tri {
            acc[i as usize] = add3(acc[i as usize], n);
        }
    }

    let vertices = positions
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let n = normalized_or(acc[i], vec3(0.0, 1.0, 0.0));
            Vertex {
                position: *p,
                normal: [n.x, n.y, n.z],
                uv: [0.0, 0.0],
                tangent: [0.0, 0.0, 0.0, 1.0],
            }
        })
        .collect();

    let mut mesh = Mesh {
        name: name.into(),
        vertices,
        indices: indices.to_vec(),
        bounds: None,
    };
    mesh.bounds = mesh.compute_bounds();
    Ok(mesh)
}

fn sub3(a: Vec3, b: Vec3) -> Vec3 {
    vec3(a.x - b.x, a.y - b.y, a.z - b.z)
}

fn add3(a: Vec3, b: Vec3) -> Vec3 {
    vec3(a.x + b.x, a.y + b.y, a.z + b.z)
}

fn cross3(a: Vec3, b: Vec3) -> Vec3 {
    vec3(
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    )
}

/// Normalize `v`, falling back to `fallback` when it is (near) zero-length.
pub(crate) fn normalized_or(v: Vec3, fallback: Vec3) -> Vec3 {
    let len = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
    if len > 1e-8 {
        vec3(v.x / len, v.y / len, v.z / len)
    } else {
        fallback
    }
}

/// A material from [`MaterialParams`] and a name.
pub(crate) fn material_from_params(name: &str, params: &MaterialParams) -> Material {
    Material {
        name: name.to_string(),
        base_color: params.base_color,
        metallic: params.metallic,
        roughness: params.roughness,
        emissive: params.emissive,
        emissive_strength: params.emissive_strength,
        alpha_mode: params.alpha_mode.into(),
        double_sided: params.double_sided,
        ..Material::default()
    }
}

/// A 1x1 sRGB texture from a solid color.
pub(crate) fn solid_texture(name: &str, color: [u8; 4]) -> Texture {
    Texture::solid(name.to_string(), color, TextureFormat::Rgba8Srgb)
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

/// A machine-readable description of the command API, emitted by
/// `noctis-editor --print-schema`.
pub fn schema() -> serde_json::Value {
    use serde_json::json;

    let commands = vec![
        json!({"op": "new_scene", "fields": {"name": "string"}, "desc": "Replace the scene with a new empty one."}),
        json!({"op": "spawn", "fields": {"name": "string", "parent?": "EntityRef"}, "desc": "Spawn an entity.", "out": "Entity"}),
        json!({"op": "destroy", "fields": {"entity": "EntityRef"}, "desc": "Destroy an entity and its descendants."}),
        json!({"op": "duplicate", "fields": {"entity": "EntityRef"}, "desc": "Duplicate an entity subtree.", "out": "Entity"}),
        json!({"op": "rename", "fields": {"entity": "EntityRef", "name": "string"}, "desc": "Rename an entity."}),
        json!({"op": "set_active", "fields": {"entity": "EntityRef", "active": "bool"}, "desc": "Set the active flag."}),
        json!({"op": "set_visible", "fields": {"entity": "EntityRef", "visible": "bool"}, "desc": "Set the visible flag."}),
        json!({"op": "set_parent", "fields": {"entity": "EntityRef", "parent?": "EntityRef|null"}, "desc": "Reparent or unparent an entity."}),
        json!({"op": "set_translation", "fields": {"entity": "EntityRef", "translation": "[f32;3]"}, "desc": "Set local translation."}),
        json!({"op": "translate", "fields": {"entity": "EntityRef", "delta": "[f32;3]"}, "desc": "Add to local translation."}),
        json!({"op": "set_rotation_euler", "fields": {"entity": "EntityRef", "degrees": "[f32;3]"}, "desc": "Set rotation from XYZ Euler degrees."}),
        json!({"op": "set_rotation_quat", "fields": {"entity": "EntityRef", "quaternion": "[x,y,z,w]"}, "desc": "Set rotation from a quaternion."}),
        json!({"op": "set_scale", "fields": {"entity": "EntityRef", "scale": "[f32;3]"}, "desc": "Set local scale."}),
        json!({"op": "look_at", "fields": {"entity": "EntityRef", "target": "[f32;3]", "up?": "[f32;3]"}, "desc": "Orient -Z toward a point."}),
        json!({"op": "add_camera", "fields": {"entity": "EntityRef", "params?": "CameraParams"}, "desc": "Add/replace a camera."}),
        json!({"op": "add_light", "fields": {"entity": "EntityRef", "params?": "LightParams"}, "desc": "Add/replace a light."}),
        json!({"op": "set_mesh_renderer", "fields": {"entity": "EntityRef", "params?": "MeshRendererParams"}, "desc": "Add/replace a mesh renderer."}),
        json!({"op": "add_rigid_body", "fields": {"entity": "EntityRef", "params?": "RigidBodyParams"}, "desc": "Add/replace a rigid body."}),
        json!({"op": "add_collider", "fields": {"entity": "EntityRef", "params?": "ColliderParams"}, "desc": "Add/replace a collider."}),
        json!({"op": "add_audio_source", "fields": {"entity": "EntityRef", "params?": "AudioSourceParams"}, "desc": "Add/replace an audio source."}),
        json!({"op": "attach_script_source", "fields": {"entity": "EntityRef", "source": "string"}, "desc": "Attach a script from inline source."}),
        json!({"op": "attach_script_path", "fields": {"entity": "EntityRef", "path": "string"}, "desc": "Attach a script from a path."}),
        json!({"op": "remove_component", "fields": {"entity": "EntityRef", "component": "ComponentName"}, "desc": "Remove a component."}),
        json!({"op": "register_mesh", "fields": {"name": "string", "positions": "[[f32;3]]", "indices": "[u32]"}, "desc": "Register a mesh.", "out": "AssetId"}),
        json!({"op": "register_material", "fields": {"name": "string", "params?": "MaterialParams"}, "desc": "Register a material.", "out": "AssetId"}),
        json!({"op": "register_texture", "fields": {"name": "string", "color": "[u8;4]"}, "desc": "Register a 1x1 texture.", "out": "AssetId"}),
        json!({"op": "import_gltf", "fields": {"path": "string", "parent?": "EntityRef"}, "desc": "Import and instantiate a glTF file.", "out": "[Entity]"}),
        json!({"op": "create_prefab", "fields": {"name": "string", "entities": "[EntityRef]"}, "desc": "Build and store a prefab.", "out": "Prefab"}),
        json!({"op": "instantiate_prefab", "fields": {"name": "string", "parent?": "EntityRef"}, "desc": "Instantiate a stored prefab.", "out": "[Entity]"}),
        json!({"op": "select", "fields": {"entity": "EntityRef"}, "desc": "Add to the selection."}),
        json!({"op": "clear_selection", "fields": {}, "desc": "Clear the selection."}),
    ];

    json!({
        "description": "Noctis editor command API. Apply a JSON array of these with \
            `noctis-editor --command <file>`.",
        "entity_ref": {
            "forms": [
                {"name": "string"},
                {"index": "u32", "generation": "u32"}
            ],
            "note": "Reference entities by name or by their (index, generation) pair."
        },
        "asset_ref": {
            "forms": [
                {"path": "string"},
                {"id": "u32"}
            ]
        },
        "component_name": [
            "LocalTransform", "GlobalTransform", "Name", "Active", "Visible",
            "Camera", "Light", "MeshRenderer", "RigidBody", "Collider",
            "AudioSource", "Script"
        ],
        "commands": commands,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_ref_round_trips_both_forms() {
        let by_name: EntityRef = serde_json::from_str(r#"{ "name": "Sun" }"#).unwrap();
        assert_eq!(by_name, EntityRef::named("Sun"));
        assert_eq!(by_name.as_name(), Some("Sun"));

        let by_id: EntityRef = serde_json::from_str(r#"{ "index": 4, "generation": 2 }"#).unwrap();
        assert_eq!(by_id, EntityRef::id(4, 2));
        assert_eq!(by_id.as_name(), None);
    }

    #[test]
    fn spawn_command_parses_with_default_parent() {
        let cmd: EditorCommand =
            serde_json::from_str(r#"{ "op": "spawn", "name": "Box" }"#).unwrap();
        assert_eq!(
            cmd,
            EditorCommand::Spawn {
                name: "Box".into(),
                parent: None
            }
        );
        assert_eq!(cmd.op_name(), "spawn");
        assert!(cmd.records_history());
    }

    #[test]
    fn asset_only_commands_do_not_record_history() {
        let cmd: EditorCommand = serde_json::from_str(
            r#"{ "op": "register_texture", "name": "t", "color": [255, 0, 0, 255] }"#,
        )
        .unwrap();
        assert!(!cmd.records_history());
    }

    #[test]
    fn projection_spec_converts_degrees_to_radians() {
        let p: Projection = ProjectionSpec::Perspective {
            fov_y_deg: 90.0,
            near: 0.1,
            far: 10.0,
        }
        .into();
        match p {
            Projection::Perspective { fov_y, .. } => {
                assert!((fov_y - std::f32::consts::FRAC_PI_2).abs() < 1e-6)
            }
            _ => panic!("expected perspective"),
        }
    }

    #[test]
    fn light_spec_converts_spot_angles() {
        let k: LightKind = LightSpec::Spot {
            direction: [0.0, -1.0, 0.0],
            inner_angle_deg: 30.0,
            outer_angle_deg: 60.0,
            range: 5.0,
        }
        .into();
        match k {
            LightKind::Spot {
                inner_angle,
                outer_angle,
                ..
            } => {
                assert!((inner_angle - radians(30.0)).abs() < 1e-6);
                assert!((outer_angle - radians(60.0)).abs() < 1e-6);
            }
            _ => panic!("expected spot"),
        }
    }

    #[test]
    fn component_names_match_engine_names() {
        for name in [
            "Camera",
            "Light",
            "MeshRenderer",
            "RigidBody",
            "Collider",
            "AudioSource",
            "Script",
        ] {
            let cn = ComponentName::from_str_name(name).expect("known name");
            assert_eq!(cn.as_str(), name);
        }
        assert!(ComponentName::from_str_name("Nope").is_none());
    }

    #[test]
    fn mesh_from_positions_rejects_bad_input() {
        assert!(mesh_from_positions("m", &[], &[0, 1, 2]).is_err());
        assert!(mesh_from_positions("m", &[[0.0, 0.0, 0.0]], &[0, 1, 2]).is_err());
    }

    #[test]
    fn mesh_from_positions_builds_triangle() {
        let mesh = mesh_from_positions(
            "tri",
            &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            &[0, 1, 2],
        )
        .unwrap();
        assert_eq!(mesh.triangle_count(), 1);
        assert_eq!(mesh.vertices.len(), 3);
        let n = mesh.vertices[0].normal;
        assert!((n[2] - 1.0).abs() < 1e-6, "normal points +Z, got {n:?}");
    }

    #[test]
    fn schema_lists_every_op() {
        let schema = schema();
        let ops: Vec<&str> = schema["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["op"].as_str().unwrap())
            .collect();
        for op in [
            "spawn",
            "add_camera",
            "add_light",
            "import_gltf",
            "remove_component",
            "clear_selection",
        ] {
            assert!(ops.contains(&op), "schema is missing {op}");
        }
    }
}
