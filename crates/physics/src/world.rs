//! The rapier bridge: [`PhysicsWorld`] keeps rapier's simulation state in sync
//! with an [`ecs::World`].

use std::collections::{HashMap, HashSet};
use std::fmt;

use ecs::components::{
    BodyType, Collider, ColliderShape, GlobalTransform, LocalTransform, Parent, RigidBody,
};
use ecs::world::{Entity, World};
use math::{Vec3, one3, trs};

use rapier3d::dynamics::{
    CCDSolver, ImpulseJointSet, IntegrationParameters, IslandManager, MultibodyJointSet,
    RigidBody as RapierRigidBody, RigidBodyBuilder, RigidBodyHandle, RigidBodySet, RigidBodyType,
    SoftBodySet,
};
use rapier3d::geometry::{
    BroadPhaseBvh, ColliderBuilder, ColliderHandle, ColliderSet, NarrowPhase, Ray,
};
use rapier3d::math::{Pose, Vector};
use rapier3d::pipeline::{PhysicsPipeline, QueryFilter, QueryPipeline};

use crate::config::PhysicsConfig;
use crate::convert;

/// The result of [`PhysicsWorld::raycast`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    /// The ECS entity that owns the hit collider, if it is bridge-managed.
    ///
    /// Colliders inserted directly through rapier (not via the bridge) have no
    /// entity and yield `None`.
    pub entity: Option<Entity>,
    /// The world-space point of impact.
    pub point: Vec3,
    /// The surface normal at the point of impact.
    pub normal: Vec3,
    /// The time of impact (distance along the ray for a unit direction).
    pub toi: f32,
}

/// A rapier collider tracked for one entity.
#[derive(Clone, Copy, Debug)]
struct ColliderRecord {
    handle: ColliderHandle,
    shape: ColliderShape,
}

/// A bridged rapier 3D physics world.
///
/// `PhysicsWorld` owns rapier's `RigidBodySet`, `ColliderSet`, `IslandManager`,
/// broad/narrow phase, joint sets, CCD solver and `IntegrationParameters`, plus
/// the entity <-> handle maps that connect them to the ECS:
///
/// * [`sync_from_world`](Self::sync_from_world) mirrors `RigidBody` +
///   `GlobalTransform` (+ `Collider`) components into rapier, creating,
///   updating and removing rapier objects as entities appear and disappear.
/// * [`step`](Self::step) advances the simulation and writes the resulting
///   poses back into `GlobalTransform` (and `LocalTransform` for root
///   entities).
///
/// ```
/// use ecs::prelude::*;
/// use math::vec3;
/// use physics::{PhysicsConfig, PhysicsWorld};
///
/// let mut world = World::new();
/// let mut physics = PhysicsWorld::new(PhysicsConfig::default());
///
/// let e = world.spawn();
/// let local = LocalTransform::from_translation(vec3(0.0, 5.0, 0.0));
/// world.insert(e, local);
/// world.insert(e, GlobalTransform::from_local(&local));
/// world.insert(e, RigidBody::default());
/// world.insert(e, Collider { shape: ColliderShape::sphere(), ..Default::default() });
///
/// physics.sync_from_world(&world);
/// for _ in 0..60 {
///     physics.step(&mut world, 1.0 / 60.0);
/// }
/// assert!(world.get::<GlobalTransform>(e).unwrap().position().y < 5.0);
/// ```
pub struct PhysicsWorld {
    config: PhysicsConfig,
    gravity: Vector,

    integration_parameters: IntegrationParameters,
    physics_pipeline: PhysicsPipeline,
    islands: IslandManager,
    broad_phase: BroadPhaseBvh,
    narrow_phase: NarrowPhase,
    bodies: RigidBodySet,
    collider_set: ColliderSet,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    soft_bodies: SoftBodySet,
    ccd_solver: CCDSolver,

    entity_to_body: HashMap<Entity, RigidBodyHandle>,
    body_to_entity: HashMap<RigidBodyHandle, Entity>,
    collider_records: HashMap<Entity, ColliderRecord>,
    collider_to_entity: HashMap<ColliderHandle, Entity>,
}

impl fmt::Debug for PhysicsWorld {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PhysicsWorld")
            .field("gravity", &self.config.gravity)
            .field("timestep", &self.integration_parameters.dt)
            .field("substeps", &self.config.substeps)
            .field("bodies", &self.bodies.len())
            .field("colliders", &self.collider_set.len())
            .finish()
    }
}

impl Default for PhysicsWorld {
    fn default() -> Self {
        Self::new(PhysicsConfig::default())
    }
}

impl PhysicsWorld {
    /// Create a new, empty physics world from `config`.
    pub fn new(config: PhysicsConfig) -> Self {
        let integration_parameters = IntegrationParameters {
            dt: config.timestep,
            ..IntegrationParameters::default()
        };

        Self {
            gravity: convert::to_vector(config.gravity),
            config,
            integration_parameters,
            physics_pipeline: PhysicsPipeline::new(),
            islands: IslandManager::new(),
            broad_phase: BroadPhaseBvh::new(),
            narrow_phase: NarrowPhase::new(),
            bodies: RigidBodySet::new(),
            collider_set: ColliderSet::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            soft_bodies: SoftBodySet::new(),
            ccd_solver: CCDSolver::new(),
            entity_to_body: HashMap::new(),
            body_to_entity: HashMap::new(),
            collider_records: HashMap::new(),
            collider_to_entity: HashMap::new(),
        }
    }

    /// The configuration this world was built with.
    #[inline]
    pub fn config(&self) -> &PhysicsConfig {
        &self.config
    }

    /// The gravity vector currently applied to dynamic bodies.
    #[inline]
    pub fn gravity(&self) -> Vec3 {
        convert::from_vector(self.gravity)
    }

    /// Replace the gravity vector. Takes effect on the next [`step`](Self::step).
    pub fn set_gravity(&mut self, gravity: Vec3) {
        self.config.gravity = gravity;
        self.gravity = convert::to_vector(gravity);
    }

    /// The rapier integration parameters (timestep, solver iterations, ...).
    #[inline]
    pub fn integration_parameters(&self) -> &IntegrationParameters {
        &self.integration_parameters
    }

    /// The underlying rapier rigid body set.
    #[inline]
    pub fn bodies(&self) -> &RigidBodySet {
        &self.bodies
    }

    /// The underlying rapier collider set.
    #[inline]
    pub fn colliders(&self) -> &ColliderSet {
        &self.collider_set
    }

    /// The rapier body handle backing `entity`, if any.
    #[inline]
    pub fn body_handle(&self, entity: Entity) -> Option<RigidBodyHandle> {
        self.entity_to_body.get(&entity).copied()
    }

    /// The entity backing a rapier body handle, if it is bridge-managed.
    #[inline]
    pub fn entity_of_body(&self, handle: RigidBodyHandle) -> Option<Entity> {
        self.body_to_entity.get(&handle).copied()
    }

    /// The number of rapier rigid bodies.
    #[inline]
    pub fn body_count(&self) -> usize {
        self.bodies.len()
    }

    /// The number of rapier colliders.
    #[inline]
    pub fn collider_count(&self) -> usize {
        self.collider_set.len()
    }

    /// A borrowed rapier query pipeline over the current world state.
    ///
    /// rapier 0.36 models the query pipeline as a *view* over the broad phase,
    /// narrow phase dispatcher and collider sets, so it cannot be stored as an
    /// owned field. This method rebuilds the equivalent view on demand; it is
    /// what [`raycast`](Self::raycast) and
    /// [`intersections_with_point`](Self::intersections_with_point) use.
    pub fn query_pipeline(&self) -> QueryPipeline<'_> {
        self.broad_phase.as_query_pipeline(
            self.narrow_phase.query_dispatcher(),
            &self.bodies,
            &self.collider_set,
            QueryFilter::default(),
        )
    }

    // ------------------------------------------------------------------
    // ECS -> rapier
    // ------------------------------------------------------------------

    /// Mirror the ECS into rapier.
    ///
    /// For every live entity with a `RigidBody` and a `GlobalTransform`, a
    /// rapier body is created (if new) or updated (position, rotation, body
    /// type, gravity scale, damping, CCD). For every entity with a `Collider`,
    /// a rapier collider is created or updated, attached to the entity's body
    /// when one exists, and standalone otherwise. Entities that lost their
    /// components — or were despawned — have their rapier objects removed.
    ///
    /// This is idempotent: syncing twice in a row produces the same rapier
    /// state without creating duplicates or moving anything.
    pub fn sync_from_world(&mut self, world: &World) {
        self.sync_bodies(world);
        self.sync_colliders(world);
    }

    fn sync_bodies(&mut self, world: &World) {
        let mut desired: Vec<(Entity, RigidBody, Pose)> = Vec::new();
        for (entity, rb) in world.query::<RigidBody>() {
            if let Some(gt) = world.get::<GlobalTransform>(entity) {
                let position = gt.position();
                let rotation = convert::rotation_from_matrix(&gt.matrix);
                desired.push((entity, *rb, convert::to_pose(position, rotation)));
            }
        }

        for (entity, rb, pose) in &desired {
            match self.entity_to_body.get(entity).copied() {
                Some(handle) => {
                    if let Some(body) = self.bodies.get_mut(handle) {
                        body.set_position(*pose, false);
                        let ty = rapier_body_type(rb.body_type);
                        if body.body_type() != ty {
                            body.set_body_type(ty, false);
                        }
                        body.set_gravity_scale(rb.gravity_scale, false);
                        body.set_linear_damping(rb.linear_damping);
                        body.set_angular_damping(rb.angular_damping);
                        body.enable_ccd(rb.ccd);
                    }
                }
                None => {
                    let handle = self.bodies.insert(build_body(rb, *pose));
                    self.entity_to_body.insert(*entity, handle);
                    self.body_to_entity.insert(handle, *entity);
                }
            }
        }

        let live: HashSet<Entity> = desired.iter().map(|(e, _, _)| *e).collect();
        let stale: Vec<Entity> = self
            .entity_to_body
            .keys()
            .copied()
            .filter(|e| !live.contains(e))
            .collect();
        for entity in stale {
            if let Some(handle) = self.entity_to_body.remove(&entity) {
                self.body_to_entity.remove(&handle);
                self.bodies.remove(
                    handle,
                    &mut self.islands,
                    &mut self.collider_set,
                    &mut self.impulse_joints,
                    &mut self.multibody_joints,
                    &mut self.soft_bodies,
                    true,
                );
            }
            if let Some(record) = self.collider_records.remove(&entity) {
                self.collider_to_entity.remove(&record.handle);
            }
        }
    }

    fn sync_colliders(&mut self, world: &World) {
        struct Desired {
            entity: Entity,
            collider: Collider,
            mass: Option<f32>,
            world_pose: Option<Pose>,
        }

        let mut desired: Vec<Desired> = Vec::new();
        for (entity, collider) in world.query::<Collider>() {
            let world_pose = world.get::<GlobalTransform>(entity).map(|gt| {
                convert::to_pose(gt.position(), convert::rotation_from_matrix(&gt.matrix))
            });
            let mass = world.get::<RigidBody>(entity).map(|rb| rb.mass);
            desired.push(Desired {
                entity,
                collider: *collider,
                mass,
                world_pose,
            });
        }

        for d in &desired {
            let parent = self.entity_to_body.get(&d.entity).copied();
            let local_pose = convert::to_pose(d.collider.offset, d.collider.rotation);

            match self.collider_records.get(&d.entity).copied() {
                Some(record) if record.shape == d.collider.shape => {
                    if let Some(collider) = self.collider_set.get_mut(record.handle) {
                        collider.set_friction(d.collider.friction);
                        collider.set_restitution(d.collider.restitution);
                        collider.set_sensor(d.collider.is_sensor);
                        if parent.is_some() {
                            collider.set_position_wrt_parent(local_pose);
                        } else if let Some(world_pose) = d.world_pose {
                            collider.set_position(world_pose);
                        }
                    }
                }
                Some(record) => {
                    self.collider_to_entity.remove(&record.handle);
                    self.collider_set.remove(
                        record.handle,
                        &mut self.islands,
                        &mut self.bodies,
                        &mut self.soft_bodies,
                        false,
                    );
                    let handle = insert_collider(
                        &mut self.collider_set,
                        &mut self.bodies,
                        &d.collider,
                        parent,
                        local_pose,
                        d.world_pose,
                        d.mass,
                    );
                    self.collider_records.insert(
                        d.entity,
                        ColliderRecord {
                            handle,
                            shape: d.collider.shape,
                        },
                    );
                    self.collider_to_entity.insert(handle, d.entity);
                }
                None => {
                    let handle = insert_collider(
                        &mut self.collider_set,
                        &mut self.bodies,
                        &d.collider,
                        parent,
                        local_pose,
                        d.world_pose,
                        d.mass,
                    );
                    self.collider_records.insert(
                        d.entity,
                        ColliderRecord {
                            handle,
                            shape: d.collider.shape,
                        },
                    );
                    self.collider_to_entity.insert(handle, d.entity);
                }
            }
        }

        let live: HashSet<Entity> = desired.iter().map(|d| d.entity).collect();
        let stale: Vec<Entity> = self
            .collider_records
            .keys()
            .copied()
            .filter(|e| !live.contains(e))
            .collect();
        for entity in stale {
            if let Some(record) = self.collider_records.remove(&entity) {
                self.collider_to_entity.remove(&record.handle);
                self.collider_set.remove(
                    record.handle,
                    &mut self.islands,
                    &mut self.bodies,
                    &mut self.soft_bodies,
                    false,
                );
            }
        }
    }

    // ------------------------------------------------------------------
    // Simulation
    // ------------------------------------------------------------------

    /// Advance the simulation by `dt` seconds, then write the result back into
    /// the ECS.
    ///
    /// The frame is split into [`PhysicsConfig::substeps`](crate::PhysicsConfig)
    /// equal rapier sub-steps. After simulating, every bridged body writes its
    /// pose into the entity's `GlobalTransform`, and into `LocalTransform` for
    /// root entities (those without a `Parent`).
    ///
    /// A non-positive `dt` falls back to the configured timestep.
    pub fn step(&mut self, world: &mut World, dt: f32) {
        self.integration_parameters.dt = self.config.effective_substep(dt);
        for _ in 0..self.config.substeps.max(1) {
            self.step_once();
        }
        self.write_back(world);
    }

    fn step_once(&mut self) {
        self.physics_pipeline.step(
            self.gravity,
            &self.integration_parameters,
            &mut self.islands,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.bodies,
            &mut self.collider_set,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.soft_bodies,
            &mut self.ccd_solver,
            &(),
            &(),
        );
    }

    fn write_back(&mut self, world: &mut World) {
        let PhysicsWorld {
            bodies,
            entity_to_body,
            ..
        } = self;

        for (entity, handle) in entity_to_body.iter() {
            let Some(body) = bodies.get(*handle) else {
                continue;
            };
            let position = convert::from_vector(body.translation());
            let rotation = convert::from_rotation(*body.rotation());
            let scale = world
                .get::<LocalTransform>(*entity)
                .map(|local| local.scale)
                .unwrap_or_else(one3);

            if let Some(global) = world.get_mut::<GlobalTransform>(*entity) {
                global.matrix = trs(position, rotation, scale);
            }

            if world.get::<Parent>(*entity).is_none()
                && let Some(local) = world.get_mut::<LocalTransform>(*entity)
            {
                local.translation = position;
                local.rotation = rotation;
            }
        }
    }

    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    /// Cast a ray from `origin` along `dir`, up to `max_toi` units.
    ///
    /// Returns the closest hit, or `None`. `dir` need not be normalized, but
    /// `toi` is then measured in multiples of `dir`'s length.
    pub fn raycast(&self, origin: Vec3, dir: Vec3, max_toi: f32) -> Option<RayHit> {
        let ray = Ray::new(convert::to_vector(origin), convert::to_vector(dir));
        let (handle, hit) = self
            .query_pipeline()
            .cast_ray_and_get_normal(&ray, max_toi, true)?;
        Some(RayHit {
            entity: self.collider_to_entity.get(&handle).copied(),
            point: origin + dir * hit.time_of_impact,
            normal: convert::from_vector(hit.normal),
            toi: hit.time_of_impact,
        })
    }

    /// Every entity whose collider contains `point`.
    pub fn intersections_with_point(&self, point: Vec3) -> Vec<Entity> {
        self.query_pipeline()
            .intersect_point(convert::to_vector(point))
            .filter_map(|(handle, _)| self.collider_to_entity.get(&handle).copied())
            .collect()
    }

    // ------------------------------------------------------------------
    // Forces / velocities
    // ------------------------------------------------------------------

    /// Apply an instantaneous impulse (change of momentum) to `entity`'s body.
    ///
    /// Does nothing if the entity has no body or the body is not dynamic.
    pub fn apply_impulse(&mut self, _world: &World, entity: Entity, impulse: Vec3) {
        if let Some(handle) = self.entity_to_body.get(&entity).copied()
            && let Some(body) = self.bodies.get_mut(handle)
        {
            body.apply_impulse(convert::to_vector(impulse), true);
        }
    }

    /// Overwrite `entity`'s body linear velocity.
    pub fn set_linear_velocity(&mut self, _world: &World, entity: Entity, velocity: Vec3) {
        if let Some(handle) = self.entity_to_body.get(&entity).copied()
            && let Some(body) = self.bodies.get_mut(handle)
        {
            body.set_linvel(convert::to_vector(velocity), true);
        }
    }

    /// The current linear velocity of `entity`'s body, if it has one.
    pub fn linear_velocity_of(&self, entity: Entity) -> Option<Vec3> {
        let handle = self.entity_to_body.get(&entity).copied()?;
        self.bodies
            .get(handle)
            .map(|body| convert::from_vector(body.linvel()))
    }
}

/// Map an ECS [`BodyType`] onto rapier's body type.
fn rapier_body_type(body_type: BodyType) -> RigidBodyType {
    match body_type {
        BodyType::Dynamic => RigidBodyType::Dynamic,
        BodyType::Fixed => RigidBodyType::Fixed,
        BodyType::Kinematic => RigidBodyType::KinematicVelocityBased,
    }
}

/// Build a rapier rigid body from an ECS [`RigidBody`] component.
fn build_body(rb: &RigidBody, pose: Pose) -> RapierRigidBody {
    let builder = match rb.body_type {
        BodyType::Dynamic => RigidBodyBuilder::dynamic(),
        BodyType::Fixed => RigidBodyBuilder::fixed(),
        BodyType::Kinematic => RigidBodyBuilder::kinematic_velocity_based(),
    };
    builder
        .pose(pose)
        .linvel(convert::to_vector(rb.linear_velocity))
        .angvel(convert::to_vector(rb.angular_velocity))
        .gravity_scale(rb.gravity_scale)
        .linear_damping(rb.linear_damping)
        .angular_damping(rb.angular_damping)
        .ccd_enabled(rb.ccd)
        .build()
}

/// The rapier collider builder for an ECS [`Collider`] at `pose`.
fn build_collider(collider: &Collider, pose: Pose, mass: Option<f32>) -> ColliderBuilder {
    let builder = match collider.shape {
        ColliderShape::Cuboid { half_extents } => {
            ColliderBuilder::cuboid(half_extents.x, half_extents.y, half_extents.z)
        }
        ColliderShape::Ball { radius } => ColliderBuilder::ball(radius),
        ColliderShape::Capsule {
            half_height,
            radius,
        } => ColliderBuilder::capsule_y(half_height, radius),
        ColliderShape::Cylinder {
            half_height,
            radius,
        } => ColliderBuilder::cylinder(half_height, radius),
    };

    let builder = builder
        .position(pose)
        .friction(collider.friction)
        .restitution(collider.restitution)
        .sensor(collider.is_sensor);

    match mass {
        Some(mass) => builder.mass(mass),
        None => builder,
    }
}

/// Insert a collider, attached to `parent` when present.
#[allow(clippy::too_many_arguments)]
fn insert_collider(
    collider_set: &mut ColliderSet,
    bodies: &mut RigidBodySet,
    collider: &Collider,
    parent: Option<RigidBodyHandle>,
    local_pose: Pose,
    world_pose: Option<Pose>,
    mass: Option<f32>,
) -> ColliderHandle {
    match parent {
        Some(parent) => {
            let builder = build_collider(collider, local_pose, mass);
            collider_set.insert_with_parent(builder, parent, bodies)
        }
        None => {
            let pose = world_pose.unwrap_or(Pose::IDENTITY);
            let builder = build_collider(collider, pose, None);
            collider_set.insert(builder)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ecs::prelude::*;
    use math::{vec3, vec3_approx_eq};

    fn spawn_body(
        world: &mut World,
        position: Vec3,
        body: RigidBody,
        collider: Collider,
    ) -> Entity {
        let entity = world.spawn();
        let local = LocalTransform::from_translation(position);
        world.insert(entity, local);
        world.insert(entity, GlobalTransform::from_local(&local));
        world.insert(entity, body);
        world.insert(entity, collider);
        entity
    }

    fn ball(radius: f32) -> Collider {
        Collider {
            shape: ColliderShape::Ball { radius },
            ..Collider::default()
        }
    }

    fn floor(half_extents: Vec3) -> Collider {
        Collider {
            shape: ColliderShape::Cuboid { half_extents },
            ..Collider::default()
        }
    }

    fn fixed() -> RigidBody {
        RigidBody {
            body_type: BodyType::Fixed,
            ..RigidBody::default()
        }
    }

    fn y_of(world: &World, entity: Entity) -> f32 {
        world.get::<GlobalTransform>(entity).unwrap().position().y
    }

    #[test]
    fn sync_creates_bodies_and_colliders() {
        let mut world = World::new();
        let mut physics = PhysicsWorld::default();
        spawn_body(
            &mut world,
            vec3(0.0, 1.0, 0.0),
            RigidBody::default(),
            ball(0.5),
        );

        physics.sync_from_world(&world);
        assert_eq!(physics.body_count(), 1);
        assert_eq!(physics.collider_count(), 1);
    }

    #[test]
    fn dynamic_body_falls_under_gravity() {
        let mut world = World::new();
        let mut physics = PhysicsWorld::default();
        let e = spawn_body(
            &mut world,
            vec3(0.0, 10.0, 0.0),
            RigidBody::default(),
            ball(0.5),
        );
        physics.sync_from_world(&world);

        let start = y_of(&world, e);
        for _ in 0..30 {
            physics.step(&mut world, 1.0 / 60.0);
        }
        let end = y_of(&world, e);
        assert!(end < start - 0.5, "body did not fall: {start} -> {end}");
    }

    #[test]
    fn ball_comes_to_rest_on_floor() {
        let mut world = World::new();
        let mut physics = PhysicsWorld::default();
        spawn_body(
            &mut world,
            vec3(0.0, 0.0, 0.0),
            fixed(),
            floor(vec3(5.0, 0.5, 5.0)),
        );
        let ball_e = spawn_body(
            &mut world,
            vec3(0.0, 5.0, 0.0),
            RigidBody::default(),
            ball(0.5),
        );
        physics.sync_from_world(&world);

        for _ in 0..300 {
            physics.step(&mut world, 1.0 / 60.0);
        }

        // Floor top at y = 0.5, ball radius 0.5 -> rest at y = 1.0.
        let rest = y_of(&world, ball_e);
        assert!(
            (rest - 1.0).abs() < 0.2,
            "ball did not rest on floor: y = {rest}"
        );
    }

    #[test]
    fn fixed_body_never_moves() {
        let mut world = World::new();
        let mut physics = PhysicsWorld::default();
        let e = spawn_body(&mut world, vec3(0.0, 3.0, 0.0), fixed(), ball(0.5));
        physics.sync_from_world(&world);

        for _ in 0..120 {
            physics.step(&mut world, 1.0 / 60.0);
        }
        let position = world.get::<GlobalTransform>(e).unwrap().position();
        assert!(
            vec3_approx_eq(position, vec3(0.0, 3.0, 0.0), 1e-5),
            "fixed body moved to {position:?}"
        );
    }

    #[test]
    fn raycast_down_hits_box_at_expected_distance() {
        let mut world = World::new();
        let mut physics = PhysicsWorld::default();
        let boxed = spawn_body(
            &mut world,
            vec3(0.0, 0.0, 0.0),
            fixed(),
            floor(vec3(0.5, 0.5, 0.5)),
        );
        physics.sync_from_world(&world);
        physics.step(&mut world, 1.0 / 60.0);

        let hit = physics
            .raycast(vec3(0.0, 5.0, 0.0), vec3(0.0, -1.0, 0.0), 100.0)
            .expect("ray should hit the box");
        assert_eq!(hit.entity, Some(boxed));
        assert!((hit.toi - 4.5).abs() < 0.05, "toi = {}", hit.toi);
        assert!(hit.normal.y > 0.9, "normal = {:?}", hit.normal);
        assert!(
            vec3_approx_eq(hit.point, vec3(0.0, 0.5, 0.0), 0.05),
            "point = {:?}",
            hit.point
        );
    }

    #[test]
    fn raycast_returns_none_when_nothing_is_hit() {
        let mut world = World::new();
        let mut physics = PhysicsWorld::default();
        spawn_body(&mut world, vec3(0.0, 0.0, 0.0), fixed(), ball(0.5));
        physics.sync_from_world(&world);
        physics.step(&mut world, 1.0 / 60.0);

        assert!(
            physics
                .raycast(vec3(0.0, 5.0, 0.0), vec3(0.0, 1.0, 0.0), 100.0)
                .is_none()
        );
    }

    #[test]
    fn sensor_collider_does_not_push() {
        let mut world = World::new();
        let mut physics = PhysicsWorld::default();
        let sensor = Collider {
            shape: ColliderShape::Cuboid {
                half_extents: vec3(5.0, 0.5, 5.0),
            },
            is_sensor: true,
            ..Collider::default()
        };
        spawn_body(&mut world, vec3(0.0, 0.0, 0.0), fixed(), sensor);
        let ball_e = spawn_body(
            &mut world,
            vec3(0.0, 3.0, 0.0),
            RigidBody::default(),
            ball(0.5),
        );
        physics.sync_from_world(&world);

        for _ in 0..120 {
            physics.step(&mut world, 1.0 / 60.0);
        }
        let y = y_of(&world, ball_e);
        assert!(y < -0.5, "sensor pushed the ball: y = {y}");
    }

    #[test]
    fn sync_from_world_is_idempotent() {
        let mut world = World::new();
        let mut physics = PhysicsWorld::default();
        let e = spawn_body(
            &mut world,
            vec3(1.0, 2.0, 3.0),
            RigidBody::default(),
            ball(0.5),
        );

        physics.sync_from_world(&world);
        let bodies = physics.body_count();
        let colliders = physics.collider_count();
        let pose = world.get::<GlobalTransform>(e).unwrap().position();

        physics.sync_from_world(&world);
        assert_eq!(physics.body_count(), bodies);
        assert_eq!(physics.collider_count(), colliders);
        assert!(vec3_approx_eq(
            world.get::<GlobalTransform>(e).unwrap().position(),
            pose,
            1e-6
        ));
    }

    #[test]
    fn despawning_an_entity_removes_its_body() {
        let mut world = World::new();
        let mut physics = PhysicsWorld::default();
        let a = spawn_body(
            &mut world,
            vec3(0.0, 1.0, 0.0),
            RigidBody::default(),
            ball(0.5),
        );
        let b = spawn_body(
            &mut world,
            vec3(3.0, 1.0, 0.0),
            RigidBody::default(),
            ball(0.5),
        );
        physics.sync_from_world(&world);
        assert_eq!(physics.body_count(), 2);

        assert!(world.despawn(a));
        physics.sync_from_world(&world);

        assert_eq!(physics.body_count(), 1);
        assert_eq!(physics.collider_count(), 1);
        assert!(physics.body_handle(a).is_none());
        assert!(physics.body_handle(b).is_some());
    }

    #[test]
    fn removing_the_rigid_body_component_removes_the_body() {
        let mut world = World::new();
        let mut physics = PhysicsWorld::default();
        let e = spawn_body(
            &mut world,
            vec3(0.0, 1.0, 0.0),
            RigidBody::default(),
            ball(0.5),
        );
        physics.sync_from_world(&world);
        assert_eq!(physics.body_count(), 1);

        world.remove::<RigidBody>(e);
        physics.sync_from_world(&world);
        assert_eq!(physics.body_count(), 0);
    }

    #[test]
    fn standalone_collider_without_body_is_supported() {
        let mut world = World::new();
        let mut physics = PhysicsWorld::default();
        let entity = world.spawn();
        let local = LocalTransform::from_translation(vec3(0.0, 0.0, 0.0));
        world.insert(entity, local);
        world.insert(entity, GlobalTransform::from_local(&local));
        world.insert(entity, ball(1.0));

        physics.sync_from_world(&world);
        assert_eq!(physics.body_count(), 0);
        assert_eq!(physics.collider_count(), 1);
    }

    #[test]
    fn impulse_and_velocity_round_trip() {
        let mut world = World::new();
        let mut physics = PhysicsWorld::default();
        let e = spawn_body(
            &mut world,
            vec3(0.0, 0.0, 0.0),
            RigidBody::default(),
            ball(0.5),
        );
        physics.sync_from_world(&world);

        physics.apply_impulse(&world, e, vec3(0.0, 5.0, 0.0));
        let after_impulse = physics.linear_velocity_of(e).unwrap();
        assert!(
            after_impulse.y > 3.0,
            "impulse had no effect: {after_impulse:?}"
        );

        physics.set_linear_velocity(&world, e, vec3(1.0, 2.0, 3.0));
        let set = physics.linear_velocity_of(e).unwrap();
        assert!(vec3_approx_eq(set, vec3(1.0, 2.0, 3.0), 1e-5));
    }

    #[test]
    fn intersections_with_point_finds_containing_entity() {
        let mut world = World::new();
        let mut physics = PhysicsWorld::default();
        let e = spawn_body(
            &mut world,
            vec3(0.0, 0.0, 0.0),
            fixed(),
            floor(vec3(1.0, 1.0, 1.0)),
        );
        physics.sync_from_world(&world);
        physics.step(&mut world, 1.0 / 60.0);

        let hits = physics.intersections_with_point(vec3(0.0, 0.0, 0.0));
        assert!(hits.contains(&e), "expected {e:?} in {hits:?}");
        assert!(
            physics
                .intersections_with_point(vec3(10.0, 10.0, 10.0))
                .is_empty()
        );
    }

    #[test]
    fn changing_body_type_to_fixed_stops_movement() {
        let mut world = World::new();
        let mut physics = PhysicsWorld::default();
        let e = spawn_body(
            &mut world,
            vec3(0.0, 5.0, 0.0),
            RigidBody::default(),
            ball(0.5),
        );
        physics.sync_from_world(&world);
        physics.step(&mut world, 1.0 / 60.0);
        let moved = y_of(&world, e);
        assert!(moved < 5.0);

        world.get_mut::<RigidBody>(e).unwrap().body_type = BodyType::Fixed;
        physics.sync_from_world(&world);
        for _ in 0..60 {
            physics.step(&mut world, 1.0 / 60.0);
        }
        assert!(
            (y_of(&world, e) - moved).abs() < 1e-5,
            "fixed body still moved"
        );
    }

    #[test]
    fn write_back_updates_local_and_global_transforms() {
        let mut world = World::new();
        let mut physics = PhysicsWorld::default();
        let e = spawn_body(
            &mut world,
            vec3(0.0, 10.0, 0.0),
            RigidBody::default(),
            ball(0.5),
        );
        physics.sync_from_world(&world);
        for _ in 0..30 {
            physics.step(&mut world, 1.0 / 60.0);
        }

        let local = world.get::<LocalTransform>(e).unwrap();
        let global = world.get::<GlobalTransform>(e).unwrap();
        assert!(vec3_approx_eq(local.translation, global.position(), 1e-5));
        assert!(local.translation.y < 10.0);
    }
}
