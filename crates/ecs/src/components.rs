//! The engine's built-in components.
//!
//! Every type here is a plain data struct or enum. Systems in the renderer,
//! physics, audio, and scripting crates read and write them; the editor and the
//! scripting API expose them by name.

use crate::assets::{
    AnimationClip, AssetHandle, AudioClip, FontAsset, Material, Mesh, ShaderMaterial, Skin, Texture,
};
use crate::world::Entity;
use math::{Mat4, Quat, Transform, Vec2, Vec3, vec3};

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

/// A renderable mesh drawn by a custom [`ShaderMaterial`] instead of the engine's
/// PBR material — the custom-shader twin of [`MeshRenderer`]. The shader is
/// compiled against the engine's fixed prelude ([`.notes/shaders.md`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShaderMeshRenderer {
    /// The mesh asset.
    pub mesh: AssetHandle<Mesh>,
    /// The shader material asset.
    pub material: AssetHandle<ShaderMaterial>,
    /// Whether this entity casts shadows.
    pub cast_shadows: bool,
    /// Whether this entity receives shadows.
    pub receive_shadows: bool,
}

impl Default for ShaderMeshRenderer {
    fn default() -> Self {
        Self {
            mesh: AssetHandle::NONE,
            material: AssetHandle::NONE,
            cast_shadows: true,
            receive_shadows: true,
        }
    }
}

/// Runs a full-screen [`ShaderMaterial`] fragment pass over the render target
/// before the scene draws — a sky ([`.notes/shaders.md`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ScreenShaderMaterial {
    /// The shader material asset.
    pub material: AssetHandle<ShaderMaterial>,
}

/// A mirror plane on a **receiver** entity (a [`ShaderMeshRenderer`], such as the
/// water): the renderer mirrors the primary camera about the plane and renders the
/// scene into an offscreen target, which the receiver's shader samples
/// ([`.notes/reflections.md`], tier 2).
///
/// A frame renders **one** reflection — the first enabled one — because it costs a
/// whole extra scene pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanarReflection {
    /// A point on the mirror plane, in world space (the water's level).
    pub point: Vec3,
    /// The plane's unit normal (the water's is `+Y`).
    pub normal: Vec3,
    /// The offscreen target's size as a fraction of the frame (`0.5` = half res).
    pub resolution: f32,
    /// Metres the clip plane is nudged above `point`, so a surface never reflects
    /// its own edge.
    pub clip_bias: f32,
    /// Whether this reflection is rendered.
    pub enabled: bool,
}

impl Default for PlanarReflection {
    fn default() -> Self {
        Self {
            point: vec3(0.0, 0.0, 0.0),
            normal: vec3(0.0, 1.0, 0.0),
            resolution: 1.0,
            clip_bias: 0.02,
            enabled: true,
        }
    }
}

impl PlanarReflection {
    /// A horizontal mirror at `level` with its normal up — a water plane.
    pub fn water(level: f32) -> Self {
        Self {
            point: vec3(0.0, level, 0.0),
            ..Self::default()
        }
    }

    /// `point` nudged `clip_bias` along the normal: the plane the pass clips
    /// against, just above the surface.
    pub fn clip_point(&self) -> Vec3 {
        let b = self.clip_bias;
        vec3(
            self.point.x + self.normal.x * b,
            self.point.y + self.normal.y * b,
            self.point.z + self.normal.z * b,
        )
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

    /// The distance from the shape's local origin down to its lowest point on the
    /// Y axis, used to place a ground probe just below a body.
    pub fn half_height(&self) -> f32 {
        match self {
            ColliderShape::Cuboid { half_extents } => half_extents.y,
            ColliderShape::Ball { radius } => *radius,
            ColliderShape::Capsule {
                half_height,
                radius,
                ..
            } => half_height + radius,
            ColliderShape::Cylinder { half_height, .. } => *half_height,
        }
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

/// Where a text label sits relative to its entity's position.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAnchor {
    /// Centred, with the middle of the text at the entity (the default).
    #[default]
    Center,
    /// The top-left corner at the entity.
    TopLeft,
    /// The top edge centred horizontally.
    TopCenter,
    /// The top-right corner at the entity.
    TopRight,
    /// The middle of the left edge at the entity.
    MiddleLeft,
    /// The middle of the right edge at the entity.
    MiddleRight,
    /// The bottom-left corner at the entity.
    BottomLeft,
    /// The bottom edge centred horizontally.
    BottomCenter,
    /// The bottom-right corner at the entity.
    BottomRight,
}

/// A text label drawn at the entity's position (in-world game text, not UI).
///
/// The glyphs are rasterized on demand from the [`font`](Self::font) asset; the
/// renderer builds and caches an atlas per font and size. `size` is the height of
/// one line in world units, so the label scales with the scene.
#[derive(Clone, Debug, PartialEq)]
pub struct Text {
    /// The string to draw (`\n` starts a new line).
    pub text: String,
    /// The font to draw it with.
    pub font: AssetHandle<FontAsset>,
    /// The height of one line, in world units.
    pub size: f32,
    /// The linear RGBA colour.
    pub color: [f32; 4],
    /// Where the text sits relative to the entity's position.
    pub anchor: TextAnchor,
    /// Whether geometry in front of the text hides it.
    pub depth_test: bool,
}

impl Default for Text {
    fn default() -> Self {
        Self {
            text: String::new(),
            font: AssetHandle::NONE,
            size: 1.0,
            color: [1.0, 1.0, 1.0, 1.0],
            anchor: TextAnchor::Center,
            depth_test: true,
        }
    }
}

impl Text {
    /// A label with `text` in the default colour at the default size.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }
}

/// How a [`Billboard`]'s quad blends with the scene.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BillboardBlend {
    /// Depth-written like an opaque surface.
    Opaque,
    /// Straight alpha blending (the default): smoke, a splash, a "tell".
    #[default]
    Alpha,
    /// Additive blending: fire and embers.
    Additive,
}

/// A camera-facing textured quad — a sprite (`.notes/billboards.md`).
///
/// Drawn at the entity's world position, always facing the camera, as
/// `texture * color` (unlit, tone-mapped with the frame). This is the general
/// billboard path: the engine draws the quad, the game moves it — a bang's fire,
/// smoke and debris, a static mine "tell", the water's wake and splashes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Billboard {
    /// The sprite's texture (a `Texture` asset, sRGB or linear).
    pub texture: AssetHandle<Texture>,
    /// World-space size in metres (width, height).
    pub size: Vec2,
    /// Linear RGBA tint, multiplied into the texture.
    pub color: [f32; 4],
    /// An in-plane rotation, radians (a spinning ember or debris).
    pub rotation: f32,
    /// The sub-rectangle of the texture to sample, `[u0, v0, u1, v1]` (an atlas
    /// frame; the whole texture is `[0, 0, 1, 1]`).
    pub uv: [f32; 4],
    /// Alpha, additive or opaque.
    pub blend: BillboardBlend,
    /// Whether geometry in front of the quad hides it.
    pub depth_test: bool,
}

impl Default for Billboard {
    fn default() -> Self {
        Self {
            texture: AssetHandle::NONE,
            size: math::vec2(1.0, 1.0),
            color: [1.0, 1.0, 1.0, 1.0],
            rotation: 0.0,
            uv: [0.0, 0.0, 1.0, 1.0],
            blend: BillboardBlend::Alpha,
            depth_test: true,
        }
    }
}

/// A renderable mesh whose vertices are deformed by a [`Skin`].
///
/// The skinned twin of [`MeshRenderer`]: the same mesh and material, but drawn
/// through the skinning pipeline, with the joint entities listed by [`Skeleton`]
/// resolving each vertex's influence (`Mesh::influences`) to a joint's world
/// transform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkinnedMeshRenderer {
    /// The mesh asset (its `influences` name the joints).
    pub mesh: AssetHandle<Mesh>,
    /// The skin binding the mesh to its joints.
    pub skin: AssetHandle<Skin>,
    /// The material asset.
    pub material: AssetHandle<Material>,
    /// Whether this entity casts shadows.
    pub cast_shadows: bool,
    /// Whether this entity receives shadows.
    pub receive_shadows: bool,
}

impl Default for SkinnedMeshRenderer {
    fn default() -> Self {
        Self {
            mesh: AssetHandle::NONE,
            skin: AssetHandle::NONE,
            material: AssetHandle::NONE,
            cast_shadows: true,
            receive_shadows: true,
        }
    }
}

/// The joint entities a [`SkinnedMeshRenderer`] deforms with.
///
/// Structural, like [`Parent`]/[`Children`]: not scene-serialized, resolved by the
/// glTF loader when it instantiates a skinned node. It holds *every* imported node
/// entity, in node-index order, because both a skin's joints and an animation
/// channel's targets index into it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Skeleton(pub Vec<Entity>);

/// Animation playback: which clip an entity plays and where its playhead is.
///
/// The engine's `update`-stage sampler (`ecs::animation::sample_scene`) advances
/// it and writes the sampled pose into the [`Skeleton`]'s joint transforms. A
/// skinned mesh plays clips by carrying this beside its [`SkinnedMeshRenderer`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Animator {
    /// The clip to play.
    pub clip: AssetHandle<AnimationClip>,
    /// The playhead, in seconds.
    pub time: f32,
    /// Playback rate (1.0 is the clip's own speed).
    pub speed: f32,
    /// Whether the clip repeats.
    pub looped: bool,
    /// Whether the playhead advances.
    pub playing: bool,
}

impl Default for Animator {
    fn default() -> Self {
        Self {
            clip: AssetHandle::NONE,
            time: 0.0,
            speed: 1.0,
            looped: true,
            playing: true,
        }
    }
}

impl Animator {
    /// An animator playing `clip` looping from its start.
    pub fn new(clip: AssetHandle<AnimationClip>) -> Self {
        Self {
            clip,
            ..Self::default()
        }
    }
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
        /// [`super::ShaderMeshRenderer`]
        ShaderMeshRenderer,
        /// [`super::ScreenShaderMaterial`]
        ScreenShaderMaterial,
        /// [`super::SkinnedMeshRenderer`]
        SkinnedMeshRenderer,
        /// [`super::Animator`]
        Animator,
        /// [`super::RigidBody`]
        RigidBody,
        /// [`super::Collider`]
        Collider,
        /// [`super::AudioSource`]
        AudioSource,
        /// [`super::Script`]
        Script,
        /// [`super::Text`]
        Text,
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
                ComponentKind::ShaderMeshRenderer => "ShaderMeshRenderer",
                ComponentKind::ScreenShaderMaterial => "ScreenShaderMaterial",
                ComponentKind::SkinnedMeshRenderer => "SkinnedMeshRenderer",
                ComponentKind::Animator => "Animator",
                ComponentKind::RigidBody => "RigidBody",
                ComponentKind::Collider => "Collider",
                ComponentKind::AudioSource => "AudioSource",
                ComponentKind::Script => "Script",
                ComponentKind::Text => "Text",
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
                "ShaderMeshRenderer" => ComponentKind::ShaderMeshRenderer,
                "ScreenShaderMaterial" => ComponentKind::ScreenShaderMaterial,
                "SkinnedMeshRenderer" => ComponentKind::SkinnedMeshRenderer,
                "Animator" => ComponentKind::Animator,
                "RigidBody" => ComponentKind::RigidBody,
                "Collider" => ComponentKind::Collider,
                "AudioSource" => ComponentKind::AudioSource,
                "Script" => ComponentKind::Script,
                "Text" | "TextRenderer" => ComponentKind::Text,
                _ => return None,
            })
        }

        /// Every kind the coverage scenes must contain, in a stable order.
        ///
        /// `SkinnedMeshRenderer` and `Animator` are deliberately absent until the
        /// animation draw path is proven ([`.notes/animation.md`]), and
        /// `ShaderMeshRenderer` and `ScreenShaderMaterial` until the shader seam's
        /// draw path is proven ([`.notes/shaders.md`]); all are still addressable by
        /// name.
        pub const ALL: [ComponentKind; 13] = [
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
            ComponentKind::Text,
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
        assert!(!world.has::<Camera>(e));
        assert_eq!(world.count::<MeshRenderer>(), 1);
    }
}
