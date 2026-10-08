//! The engine's built-in components.
//!
//! Every type here is a plain data struct or enum. Systems in the renderer,
//! physics, audio, and scripting crates read and write them; the editor and the
//! scripting API expose them by name.

use crate::assets::{AssetHandle, AudioClip, Material, Mesh};
use crate::world::Entity;
use math::{Mat4, Quat, Transform, Vec3, vec3};

/// The local transform component. A type alias so authoring code can use the
/// math crate's builders directly.
pub type LocalTransform = Transform;

/// The world-space transform, recomputed each frame from the entity's
/// [`LocalTransform`] and its parent chain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlobalTransform {
    /// The composed world matrix.
    pub matrix: Mat4,
}

impl Default for GlobalTransform {
    fn default() -> Self {
        Self::identity()
    }
}

impl GlobalTransform {
    /// The identity world transform.
    pub fn identity() -> Self {
        Self {
            matrix: math::identity(),
        }
    }

    /// The world transform of a root entity with local transform `local`.
    pub fn from_local(local: &Transform) -> Self {
        Self {
            matrix: local.to_matrix(),
        }
    }

    /// The world-space position (the matrix translation column).
    pub fn position(&self) -> Vec3 {
        let c = self.matrix[3];
        vec3(c.x, c.y, c.z)
    }

    /// The composed world matrix.
    pub fn to_matrix(&self) -> Mat4 {
        self.matrix
    }

    /// Compose a parent's world matrix with a child's local transform.
    pub fn compose(parent: &GlobalTransform, child: &Transform) -> Self {
        Self {
            matrix: parent.matrix * child.to_matrix(),
        }
    }
}

/// A human-readable name for an entity (shown in the editor outliner).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Name(pub String);

impl Name {
    /// Construct a name.
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// The name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for Name {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

/// Whether an entity and its descendants are processed (updated/rendered).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Active(pub bool);

impl Default for Active {
    fn default() -> Self {
        Self(true)
    }
}

/// Whether an entity's renderables are drawn, independent of [`Active`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Visible(pub bool);

impl Default for Visible {
    fn default() -> Self {
        Self(true)
    }
}

/// The parent link in the transform hierarchy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parent(pub Entity);

/// The child links in the transform hierarchy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Children(pub Vec<Entity>);

impl Children {
    /// `true` if the entity has no children.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The number of children.
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

/// A camera's projection mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Projection {
    /// A perspective projection. `fov_y` is the vertical field of view in
    /// radians.
    Perspective {
        /// Vertical field of view, radians.
        fov_y: f32,
        /// Near plane distance.
        near: f32,
        /// Far plane distance.
        far: f32,
    },
    /// An orthographic projection. `height` is the visible world height and the
    /// width is derived from the aspect ratio.
    Orthographic {
        /// Visible world-space height.
        height: f32,
        /// Near plane distance.
        near: f32,
        /// Far plane distance.
        far: f32,
    },
}

impl Default for Projection {
    fn default() -> Self {
        Projection::Perspective {
            fov_y: math::radians(60.0),
            near: 0.1,
            far: 1000.0,
        }
    }
}

impl Projection {
    /// The near plane distance.
    pub fn near(&self) -> f32 {
        match self {
            Projection::Perspective { near, .. } | Projection::Orthographic { near, .. } => *near,
        }
    }

    /// The far plane distance.
    pub fn far(&self) -> f32 {
        match self {
            Projection::Perspective { far, .. } | Projection::Orthographic { far, .. } => *far,
        }
    }
}

/// A camera component. Entities with a camera are rendered from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Projection mode.
    pub projection: Projection,
    /// Whether this camera renders. Multiple active cameras render in priority
    /// order.
    pub is_active: bool,
    /// Lower priorities render first.
    pub priority: i32,
    /// The clear color (linear RGBA).
    pub clear_color: [f32; 4],
    /// Whether the camera renders into the HDR (float) pipeline before
    /// tonemapping.
    pub hdr: bool,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            projection: Projection::default(),
            is_active: true,
            priority: 0,
            clear_color: [0.02, 0.02, 0.03, 1.0],
            hdr: true,
        }
    }
}

/// A light's shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LightKind {
    /// A directional light (sun). `direction` points from the light toward the
    /// scene.
    Directional {
        /// The light direction.
        direction: Vec3,
    },
    /// A point light.
    Point {
        /// Falloff range.
        range: f32,
    },
    /// A spot light.
    Spot {
        /// The light direction.
        direction: Vec3,
        /// Inner cone angle, radians.
        inner_angle: f32,
        /// Outer cone angle, radians.
        outer_angle: f32,
        /// Falloff range.
        range: f32,
    },
}

/// A light component.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    /// The light shape.
    pub kind: LightKind,
    /// Linear RGB color.
    pub color: Vec3,
    /// Intensity (candela for point/spot, lux for directional).
    pub intensity: f32,
    /// Whether this light casts shadows.
    pub cast_shadows: bool,
    /// Shadow map bias.
    pub shadow_bias: f32,
}

impl Default for Light {
    fn default() -> Self {
        Self {
            kind: LightKind::Directional {
                direction: vec3(0.0, -1.0, 0.0),
            },
            color: vec3(1.0, 1.0, 1.0),
            intensity: 1.0,
            cast_shadows: true,
            shadow_bias: 0.001,
        }
    }
}

impl Light {
    /// A directional light aimed along `direction`.
    pub fn directional(direction: Vec3) -> Self {
        Self {
            kind: LightKind::Directional { direction },
            ..Self::default()
        }
    }

    /// A point light.
    pub fn point(color: Vec3, intensity: f32, range: f32) -> Self {
        Self {
            kind: LightKind::Point { range },
            color,
            intensity,
            ..Self::default()
        }
    }
}

/// A renderable mesh with a material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeshRenderer {
    /// The mesh asset.
    pub mesh: AssetHandle<Mesh>,
    /// The material asset.
    pub material: AssetHandle<Material>,
    /// Whether this entity casts shadows.
    pub cast_shadows: bool,
    /// Whether this entity receives shadows.
    pub receive_shadows: bool,
}

impl Default for MeshRenderer {
    fn default() -> Self {
        Self {
            mesh: AssetHandle::NONE,
            material: AssetHandle::NONE,
            cast_shadows: true,
            receive_shadows: true,
        }
    }
}

/// A rigid body's simulation type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BodyType {
    /// Fully simulated.
    #[default]
    Dynamic,
    /// Immovable (infinite mass).
    Fixed,
    /// Moved by code, but pushes dynamic bodies.
    Kinematic,
}

/// A physics rigid body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RigidBody {
    /// Simulation type.
    pub body_type: BodyType,
    /// Mass in kilograms (ignored for fixed/kinematic bodies).
    pub mass: f32,
    /// Bounciness in `[0, 1]`.
    pub restitution: f32,
    /// Friction coefficient.
    pub friction: f32,
    /// Per-body gravity multiplier.
    pub gravity_scale: f32,
    /// Initial linear velocity.
    pub linear_velocity: Vec3,
    /// Initial angular velocity.
    pub angular_velocity: Vec3,
    /// Linear damping.
    pub linear_damping: f32,
    /// Angular damping.
    pub angular_damping: f32,
    /// Whether continuous collision detection is enabled.
    pub ccd: bool,
}

impl Default for RigidBody {
    fn default() -> Self {
        Self {
            body_type: BodyType::Dynamic,
            mass: 1.0,
            restitution: 0.2,
            friction: 0.5,
            gravity_scale: 1.0,
            linear_velocity: vec3(0.0, 0.0, 0.0),
            angular_velocity: vec3(0.0, 0.0, 0.0),
            linear_damping: 0.0,
            angular_damping: 0.0,
            ccd: false,
        }
    }
}

/// A collider's shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColliderShape {
    /// A box, half extents per axis.
    Cuboid {
        /// Half extents per axis.
        half_extents: Vec3,
    },
    /// A sphere.
    Ball {
        /// Radius.
        radius: f32,
    },
    /// A capsule aligned to the local Y axis.
    Capsule {
        /// Half the height of the cylindrical section.
        half_height: f32,
        /// Radius.
        radius: f32,
    },
    /// A cylinder aligned to the local Y axis.
    Cylinder {
        /// Half height.
        half_height: f32,
        /// Radius.
        radius: f32,
    },
}

impl ColliderShape {
    /// A unit cube collider (half extents `0.5`).
    pub fn cube() -> Self {
        ColliderShape::Cuboid {
            half_extents: vec3(0.5, 0.5, 0.5),
        }
    }

    /// A unit-radius sphere.
    pub fn sphere() -> Self {
        ColliderShape::Ball { radius: 0.5 }
    }
}

/// A physics collider attached to a [`RigidBody`] (or to a static entity).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Collider {
    /// The collider shape.
    pub shape: ColliderShape,
    /// Offset from the entity origin, in local space.
    pub offset: Vec3,
    /// Local rotation relative to the entity.
    pub rotation: Quat,
    /// Whether this collider detects overlaps without resolving them.
    pub is_sensor: bool,
    /// Friction override.
    pub friction: f32,
    /// Restitution override.
    pub restitution: f32,
}

impl Default for Collider {
    fn default() -> Self {
        Self {
            shape: ColliderShape::cube(),
            offset: vec3(0.0, 0.0, 0.0),
            rotation: Quat::identity(),
            is_sensor: false,
            friction: 0.5,
            restitution: 0.2,
        }
    }
}

/// A positional audio emitter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioSource {
    /// The clip to play.
    pub clip: AssetHandle<AudioClip>,
    /// Volume multiplier.
    pub volume: f32,
    /// Playback rate multiplier.
    pub pitch: f32,
    /// Whether playback loops.
    pub looped: bool,
    /// Whether playback is currently active.
    pub playing: bool,
    /// Whether the sound is spatialized in 3D.
    pub spatial: bool,
    /// Distance at which the sound is inaudible (when spatial).
    pub max_distance: f32,
    /// Rolloff factor (when spatial).
    pub rolloff: f32,
}

impl Default for AudioSource {
    fn default() -> Self {
        Self {
            clip: AssetHandle::NONE,
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

/// A JavaScript behaviour attached to an entity.
///
/// Exactly one of `source` (inline code) or `path` (a script asset path) is
/// expected to be set.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Script {
    /// Inline JavaScript source.
    pub source: Option<String>,
    /// Path to a `.js` script asset.
    pub path: Option<String>,
    /// Whether the script runs.
    pub enabled: bool,
}

impl Script {
    /// A script from inline source.
    pub fn from_source(source: impl Into<String>) -> Self {
        Self {
            source: Some(source.into()),
            path: None,
            enabled: true,
        }
    }

    /// A script from a file path.
    pub fn from_path(path: impl Into<String>) -> Self {
        Self {
            source: None,
            path: Some(path.into()),
            enabled: true,
        }
    }

    /// `true` if the script has any code to run.
    pub fn has_code(&self) -> bool {
        self.source.is_some() || self.path.is_some()
    }
}

/// Marks an entity as an instance of a named prefab (for re-instantiation and
/// editor bookkeeping).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrefabInstance {
    /// The prefab's asset path.
    pub prefab: String,
}

/// The engine's list of "registered" component names, used by the editor and
/// the scripting layer to build a stable, human-readable component index.
pub mod component_index {
    /// A stable index for a built-in component type, used by the scripting
    /// bridge to address components without a string lookup per access.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub enum ComponentKind {
        /// [`super::LocalTransform`]
        LocalTransform,
        /// [`super::GlobalTransform`]
        GlobalTransform,
        /// [`super::Name`]
        Name,
        /// [`super::Active`]
        Active,
        /// [`super::Visible`]
        Visible,
        /// [`super::Camera`]
        Camera,
        /// [`super::Light`]
        Light,
        /// [`super::MeshRenderer`]
        MeshRenderer,
        /// [`super::RigidBody`]
        RigidBody,
        /// [`super::Collider`]
        Collider,
        /// [`super::AudioSource`]
        AudioSource,
        /// [`super::Script`]
        Script,
    }

    impl ComponentKind {
        /// The canonical name, matching the component's type name.
        pub const fn name(self) -> &'static str {
            match self {
                ComponentKind::LocalTransform => "LocalTransform",
                ComponentKind::GlobalTransform => "GlobalTransform",
                ComponentKind::Name => "Name",
                ComponentKind::Active => "Active",
                ComponentKind::Visible => "Visible",
                ComponentKind::Camera => "Camera",
                ComponentKind::Light => "Light",
                ComponentKind::MeshRenderer => "MeshRenderer",
                ComponentKind::RigidBody => "RigidBody",
                ComponentKind::Collider => "Collider",
                ComponentKind::AudioSource => "AudioSource",
                ComponentKind::Script => "Script",
            }
        }

        /// Look a component kind up by its canonical name.
        pub fn from_name(name: &str) -> Option<Self> {
            Some(match name {
                "LocalTransform" | "Transform" => ComponentKind::LocalTransform,
                "GlobalTransform" => ComponentKind::GlobalTransform,
                "Name" => ComponentKind::Name,
                "Active" => ComponentKind::Active,
                "Visible" => ComponentKind::Visible,
                "Camera" => ComponentKind::Camera,
                "Light" => ComponentKind::Light,
                "MeshRenderer" => ComponentKind::MeshRenderer,
                "RigidBody" => ComponentKind::RigidBody,
                "Collider" => ComponentKind::Collider,
                "AudioSource" => ComponentKind::AudioSource,
                "Script" => ComponentKind::Script,
                _ => return None,
            })
        }

        /// Every kind, in a stable order.
        pub const ALL: [ComponentKind; 12] = [
            ComponentKind::LocalTransform,
            ComponentKind::GlobalTransform,
            ComponentKind::Name,
            ComponentKind::Active,
            ComponentKind::Visible,
            ComponentKind::Camera,
            ComponentKind::Light,
            ComponentKind::MeshRenderer,
            ComponentKind::RigidBody,
            ComponentKind::Collider,
            ComponentKind::AudioSource,
            ComponentKind::Script,
        ];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::World;

    #[test]
    fn global_transform_from_local_uses_translation() {
        let local = Transform::from_translation(vec3(1.0, 2.0, 3.0));
        let g = GlobalTransform::from_local(&local);
        assert_eq!(g.position().x, 1.0);
        assert_eq!(g.position().y, 2.0);
        assert_eq!(g.position().z, 3.0);
    }

    #[test]
    fn global_transform_compose_matches_strs() {
        let parent_local = Transform::from_translation(vec3(10.0, 0.0, 0.0));
        let parent = GlobalTransform::from_local(&parent_local);
        let child_local = Transform::from_translation(vec3(0.0, 5.0, 0.0));
        let composed = GlobalTransform::compose(&parent, &child_local);
        let p = composed.position();
        assert!((p.x - 10.0).abs() < 1e-6);
        assert!((p.y - 5.0).abs() < 1e-6);
    }

    #[test]
    fn defaults_are_sensible() {
        assert!(Active::default().0);
        assert!(Visible::default().0);
        assert_eq!(Camera::default().projection.near(), 0.1);
        assert_eq!(RigidBody::default().body_type, BodyType::Dynamic);
        assert!(matches!(
            Collider::default().shape,
            ColliderShape::Cuboid { .. }
        ));
        assert!(!AudioSource::default().playing);
        assert_eq!(MeshRenderer::default().mesh, AssetHandle::NONE);
    }

    #[test]
    fn script_constructors() {
        let s = Script::from_source("function update() {}");
        assert!(s.has_code());
        assert!(s.enabled);
        let s2 = Script::from_path("scripts/spin.js");
        assert_eq!(s2.path.as_deref(), Some("scripts/spin.js"));
        assert!(!Script::default().has_code());
    }

    #[test]
    fn component_kind_lookup() {
        assert_eq!(
            component_index::ComponentKind::from_name("Transform"),
            Some(component_index::ComponentKind::LocalTransform)
        );
        assert_eq!(
            component_index::ComponentKind::from_name("MeshRenderer"),
            Some(component_index::ComponentKind::MeshRenderer)
        );
        assert_eq!(component_index::ComponentKind::from_name("Nope"), None);
        for kind in component_index::ComponentKind::ALL {
            assert_eq!(
                component_index::ComponentKind::from_name(kind.name()),
                Some(kind)
            );
        }
    }

    #[test]
    fn components_live_in_world() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, Name::new("Player"));
        world.insert(e, LocalTransform::default());
        world.insert(e, MeshRenderer::default());
        world.insert(e, RigidBody::default());
        world.insert(e, Collider::default());
        world.insert(e, Light::directional(vec3(-1.0, -1.0, 0.0)));
        assert_eq!(world.get::<Name>(e).unwrap().as_str(), "Player");
        assert!(world.has::<Camera>(e) == false);
        assert_eq!(world.count::<MeshRenderer>(), 1);
    }
}
