//! # ecs — the Noctis engine core
//!
//! This crate is the shared foundation every other Noctis crate builds on: a
//! small hand-written ECS, the built-in components, the scene/hierarchy model,
//! asset stores, frame resources, and the application loop.
//!
//! Nothing here talks to the GPU, the OS window, or a physics engine, so the
//! whole crate is headlessly testable. The renderer, physics, audio, and
//! scripting crates consume these types and are driven by [`app::App`].
//!
//! ```
//! use ecs::prelude::*;
//! use math::vec3;
//!
//! let mut world = World::new();
//! let sun = world.spawn_with(Name::new("Sun"));
//! world.insert(sun, Light::directional(vec3(0.0, -1.0, 0.0)));
//! assert_eq!(world.count::<Light>(), 1);
//! ```

#![forbid(unsafe_code)]

pub mod app;
pub mod assets;
pub mod components;
pub mod primitives;
pub mod reflect;
pub mod resources;
pub mod scene;
pub mod world;

/// Derive the `Component` trait (see [`reflect::Component`]).
pub use ecs_macros::Component;

/// The most commonly used imports.
pub mod prelude {
    pub use crate::app::{App, EngineContext, EngineEvent, Stage, System};
    pub use crate::assets::{
        AssetHandle, AssetId, AssetRegistry, Assets, AudioClip, FontAsset, HdrTexture, Material,
        Mesh, ScriptAsset, Texture, TextureFormat, Vertex,
    };
    pub use crate::components::{
        Active, AudioSource, BodyType, Camera, Children, Collider, ColliderShape, GlobalTransform,
        Light, LightKind, LocalTransform, MeshRenderer, Name, Parent, PrefabInstance, Projection,
        RigidBody, Script, Text, TextAnchor, Visible,
    };
    pub use crate::reflect::{Component, ComponentRegistry, Field, FieldKind};
    pub use crate::resources::{ActionMap, Events, InputState, Key, MouseButton, MouseState, Time};
    pub use crate::scene::{
        Prefab, Scene, SceneDesc, SceneError, SceneMode, children_of, instantiate_into,
        propagate_transforms, set_parent,
    };
    pub use crate::world::{Entities, Entity, Query, QueryMut, QueryWith, World};
    pub use ecs_macros::Component;
    pub use math;
    pub use math::{Aabb, Mat4, Quat, Transform, Vec2, Vec3, Vec4};
}
