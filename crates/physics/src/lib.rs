//! # physics — rapier3d integration for Noctis
//!
//! This crate bridges the [`ecs`] world to [rapier 3d] 0.36. It is deliberately
//! thin: rapier owns the simulation state, and this crate keeps it in sync with
//! the ECS components defined by the engine core.
//!
//! The entry point is [`PhysicsWorld`]:
//!
//! * [`PhysicsWorld::sync_from_world`] mirrors `RigidBody` + `GlobalTransform`
//!   (+ `Collider`) components into rapier, creating, updating and removing
//!   rapier objects as entities appear and disappear.
//! * [`PhysicsWorld::step`] advances the simulation and writes the resulting
//!   poses back into `GlobalTransform` (and `LocalTransform` for root
//!   entities).
//! * [`PhysicsWorld::raycast`], [`PhysicsWorld::apply_impulse`] and friends
//!   expose the common gameplay queries and controls.
//!
//! ```
//! use ecs::prelude::*;
//! use math::vec3;
//! use physics::{PhysicsConfig, PhysicsWorld};
//!
//! let mut world = World::new();
//! let mut physics = PhysicsWorld::new(PhysicsConfig::default());
//!
//! // Ground.
//! let ground = world.spawn();
//! world.insert(ground, GlobalTransform::identity());
//! world.insert(ground, RigidBody { body_type: BodyType::Fixed, ..Default::default() });
//! world.insert(ground, Collider { shape: ColliderShape::cube(), ..Default::default() });
//!
//! // A ball above it.
//! let ball = world.spawn();
//! let local = LocalTransform::from_translation(vec3(0.0, 4.0, 0.0));
//! world.insert(ball, local);
//! world.insert(ball, GlobalTransform::from_local(&local));
//! world.insert(ball, RigidBody::default());
//! world.insert(ball, Collider { shape: ColliderShape::sphere(), ..Default::default() });
//!
//! physics.sync_from_world(&world);
//! for _ in 0..120 {
//!     physics.step(&mut world, 1.0 / 60.0);
//! }
//!
//! let hit = physics.raycast(vec3(0.0, 10.0, 0.0), vec3(0.0, -1.0, 0.0), 100.0);
//! assert!(hit.is_some());
//! ```
//!
//! [rapier 3d]: https://docs.rs/rapier3d/0.36.0/rapier3d/

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

mod config;
mod convert;
mod world;

pub use config::PhysicsConfig;
pub use world::{PhysicsWorld, RayHit};

/// The most commonly used imports.
pub mod prelude {
    pub use crate::{PhysicsConfig, PhysicsWorld, RayHit};
    pub use ecs::prelude::*;
    pub use math;
}
