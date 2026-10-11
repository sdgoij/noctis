//! The engine API exposed to scripts, and the bridge that drives it.
//!
//! [`ScriptApi`] is the object scripts talk to. The Rust side drives it: a
//! [`ScriptHost`](crate::host::ScriptHost) receives a [`ScriptContext`] for the
//! current lifecycle call and constructs a [`ScriptApi`] over it. Every
//! operation the API performs is *both* applied to the [`World`] and recorded
//! as a [`ScriptCommand`], which gives tests a deterministic, engine-free way
//! to observe what a script did.
//!
//! Entity handles crossing the script boundary are [`EntityId`]s: a `u64`
//! packing an entity's index and generation. Packing the generation lets the
//! API reject stale handles left behind by a despawned entity.

use ecs::assets::AssetRegistry;
use ecs::components::{LocalTransform, Name, Script};
use ecs::reflect::ComponentRegistry;
use ecs::resources::{InputState, Key, MouseButton};
use ecs::scene::Prefab;
use ecs::world::{Entity, World};
use math::{Quat, Vec3, vec3, zero3};

/// A script-facing entity handle.
///
/// Encodes an [`Entity`]'s index in the low 32 bits and its generation in the
/// high 32 bits, so a handle to a despawned (and possibly reused) slot is
/// recognised as stale rather than silently pointing at a different entity.
pub type EntityId = u64;

/// Pack a world [`Entity`] into an [`EntityId`].
#[inline]
pub const fn entity_id(entity: Entity) -> EntityId {
    ((entity.generation() as u64) << 32) | entity.index() as u64
}

/// Unpack an [`EntityId`], returning `Some` only if the entity is still alive.
#[inline]
pub fn entity_from_id(world: &World, id: EntityId) -> Option<Entity> {
    let entity = Entity::from_raw(id as u32, (id >> 32) as u32);
    world.is_alive(entity).then_some(entity)
}

/// A record of one side effect a script asked for.
///
/// The API applies each of these to the [`World`] immediately *and* appends the
/// command to [`ScriptContext::commands`], so a host (or a test) can inspect
/// exactly what happened without re-reading the world.
#[derive(Clone, Debug, PartialEq)]
pub enum ScriptCommand {
    /// A message emitted with `log`.
    Log {
        /// The message.
        message: String,
        /// The entity the call was made for, if any.
        entity: Option<EntityId>,
    },
    /// An entity was spawned.
    Spawn {
        /// The name it was given, if any.
        name: Option<String>,
        /// The new entity.
        entity: EntityId,
    },
    /// An entity was destroyed by id.
    Destroy {
        /// The destroyed entity.
        entity: EntityId,
    },
    /// An entity was destroyed by name.
    DestroyByName {
        /// The name searched for.
        name: String,
    },
    /// An entity's local position was replaced.
    SetPosition {
        /// The entity.
        entity: EntityId,
        /// The new position.
        position: Vec3,
    },
    /// An entity's local position was offset.
    Translate {
        /// The entity.
        entity: EntityId,
        /// The offset applied.
        delta: Vec3,
    },
    /// An entity's local rotation was set from Euler angles.
    SetRotationEuler {
        /// The entity.
        entity: EntityId,
        /// The Euler angles (roll, pitch, yaw), radians.
        euler: Vec3,
    },
    /// An entity's local scale was replaced.
    SetScale {
        /// The entity.
        entity: EntityId,
        /// The new scale.
        scale: Vec3,
    },
    /// An entity's body linear velocity was overwritten.
    ///
    /// Queued only: the script API has no physics handle, so the runtime applies
    /// it to the [`PhysicsWorld`](physics) after the call.
    SetVelocity {
        /// The entity.
        entity: EntityId,
        /// The new linear velocity.
        velocity: Vec3,
    },
    /// An entity's body linear velocity was offset.
    ///
    /// Queued only, applied by the runtime (see [`ScriptCommand::SetVelocity`]).
    AddVelocity {
        /// The entity.
        entity: EntityId,
        /// The offset applied to the velocity.
        velocity: Vec3,
    },
    /// An instantaneous impulse was applied to an entity's body.
    ///
    /// Queued only, applied by the runtime (see [`ScriptCommand::SetVelocity`]).
    ApplyImpulse {
        /// The entity.
        entity: EntityId,
        /// The impulse (a change of momentum).
        velocity: Vec3,
    },
    /// A request to load another scene, replacing the current one.
    ///
    /// Queued only: only the runtime owns scene loading and the physics/audio
    /// that must be rebuilt, so it acts on this at the end of the frame.
    LoadScene {
        /// The scene path, resolved by the runtime's scene loader.
        path: String,
    },
    /// A script component was added or replaced at runtime.
    AddScript {
        /// The entity.
        entity: EntityId,
        /// The new source.
        source: String,
    },
    /// A registered component was set from JSON.
    SetComponent {
        /// The entity.
        entity: EntityId,
        /// The component's registered name.
        name: String,
        /// The JSON the component was (or was to be) set from.
        json: String,
    },
    /// A registered component was removed.
    RemoveComponent {
        /// The entity.
        entity: EntityId,
        /// The component's registered name.
        name: String,
    },
    /// A prefab was instantiated.
    InstantiatePrefab {
        /// The prefab JSON that was parsed.
        json: String,
        /// The parent the instances were attached to, if any.
        parent: Option<EntityId>,
        /// The entities that were created, in document order.
        spawned: Vec<EntityId>,
        /// The handle the caller was handed for the prefab's root entity, when
        /// the command was queued by a host (a sandboxed engine cannot know the
        /// real id until it is replayed, so it names a placeholder). `None` when
        /// the command was produced by a direct, eager call.
        root: Option<EntityId>,
    },
}

/// Everything a script call has access to for one lifecycle invocation.
///
/// A context borrows the engine mutably for the duration of a single
/// `on_start` / `on_update` / `on_destroy` call. Construct one per call; do not
/// hold it across frames.
pub struct ScriptContext<'a> {
    /// The world the scripts operate on.
    pub world: &'a mut World,
    /// The asset registry, used to resolve script paths and prefab assets.
    pub assets: &'a AssetRegistry,
    /// The registered custom components scripts read and write by name (the
    /// reflection registry). Built-in components are reflected too, from the
    /// scene-form descriptors; this registry holds only the application's customs.
    pub components: &'a ComponentRegistry,
    /// Input state for the current frame.
    pub input: &'a InputState,
    /// A read-only snapshot of the physics state (grounding, velocities), or
    /// `None` when the caller has no physics to offer. The host supplies it per
    /// frame; a bare [`ScriptContext::new`] leaves it empty.
    pub physics: Option<&'a PhysicsView>,
    /// Total elapsed scaled time, seconds.
    pub elapsed: f32,
    /// Delta time of the current frame, seconds.
    pub delta: f32,
    /// The entity the current lifecycle call pertains to.
    pub entity: Option<EntityId>,
    /// Side effects recorded during this call.
    pub commands: Vec<ScriptCommand>,
}

impl<'a> ScriptContext<'a> {
    /// A context with no current entity and an empty command log.
    ///
    /// `components` is the reflection registry scripts read and write by name;
    /// pass an empty one when there are no registered components. There is no
    /// physics snapshot; use [`ScriptContext::with_physics`] to add one.
    pub fn new(
        world: &'a mut World,
        assets: &'a AssetRegistry,
        components: &'a ComponentRegistry,
        input: &'a InputState,
    ) -> Self {
        Self::with_physics(world, assets, components, input, None)
    }

    /// Like [`ScriptContext::new`], but with a physics snapshot for the reads
    /// `is_grounded` / `get_velocity`.
    pub fn with_physics(
        world: &'a mut World,
        assets: &'a AssetRegistry,
        components: &'a ComponentRegistry,
        input: &'a InputState,
        physics: Option<&'a PhysicsView>,
    ) -> Self {
        Self {
            world,
            assets,
            components,
            input,
            physics,
            elapsed: 0.0,
            delta: 0.0,
            entity: None,
            commands: Vec::new(),
        }
    }

    /// The API object for this context.
    ///
    /// The returned [`ScriptApi`] borrows the context; drop it before using the
    /// context again.
    pub fn api(&mut self) -> ScriptApi<'_, 'a> {
        ScriptApi { ctx: self }
    }
}

/// A read-only snapshot of the physics state scripts may read this frame:
/// whether a body is grounded and its linear velocity, keyed by entity handle.
///
/// The runtime fills one from the [`PhysicsWorld`](physics) each frame; the
/// bridge copies it, so `is_grounded` / `get_velocity` answer from the snapshot
/// instead of reaching into physics mid-call.
#[derive(Clone, Debug, Default)]
pub struct PhysicsView {
    pub(crate) grounded: std::collections::HashSet<EntityId>,
    pub(crate) velocities: std::collections::HashMap<EntityId, Vec3>,
}

impl PhysicsView {
    /// An empty view.
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop everything, for reuse between frames.
    pub fn clear(&mut self) {
        self.grounded.clear();
        self.velocities.clear();
    }

    /// Record whether `entity` is grounded.
    pub fn set_grounded(&mut self, entity: EntityId, grounded: bool) {
        if grounded {
            self.grounded.insert(entity);
        } else {
            self.grounded.remove(&entity);
        }
    }

    /// Record `entity`'s linear velocity.
    pub fn set_velocity(&mut self, entity: EntityId, velocity: Vec3) {
        self.velocities.insert(entity, velocity);
    }

    /// Whether `entity` is grounded.
    pub fn is_grounded(&self, entity: EntityId) -> bool {
        self.grounded.contains(&entity)
    }

    /// `entity`'s linear velocity, or zero when unknown.
    pub fn velocity(&self, entity: EntityId) -> Vec3 {
        self.velocities.get(&entity).copied().unwrap_or_else(zero3)
    }
}

/// The object scripts call into.
///
/// Every method applies its effect to the world (or reads from it) and, where
/// it mutates state, records a matching [`ScriptCommand`] in the context's
/// command log.
pub struct ScriptApi<'api, 'ctx> {
    ctx: &'api mut ScriptContext<'ctx>,
}

impl<'api, 'ctx> ScriptApi<'api, 'ctx> {
    /// Wrap a context.
    pub fn new(ctx: &'api mut ScriptContext<'ctx>) -> Self {
        Self { ctx }
    }

    /// The entity the current lifecycle call pertains to.
    pub fn current_entity(&self) -> Option<EntityId> {
        self.ctx.entity
    }

    // -- logging ------------------------------------------------------------

    /// Emit a message. Recorded as [`ScriptCommand::Log`]; the engine does not
    /// write it anywhere by default.
    pub fn log(&mut self, msg: impl Into<String>) {
        self.ctx.commands.push(ScriptCommand::Log {
            message: msg.into(),
            entity: self.ctx.entity,
        });
    }

    // -- entity lifecycle ---------------------------------------------------

    /// Spawn a new entity, optionally named, and return its handle.
    ///
    /// The entity gets a default [`LocalTransform`] so it is immediately
    /// positionable. Scripts attached to it start on the next frame.
    pub fn spawn(&mut self, name: Option<String>) -> EntityId {
        let entity = self.ctx.world.spawn();
        self.ctx.world.insert(entity, LocalTransform::default());
        if let Some(name) = &name {
            self.ctx.world.insert(entity, Name::new(name.clone()));
        }
        let id = entity_id(entity);
        self.ctx
            .commands
            .push(ScriptCommand::Spawn { name, entity: id });
        id
    }

    /// Despawn an entity and all of its components. Unknown or stale handles
    /// are ignored.
    pub fn destroy(&mut self, id: EntityId) {
        if let Some(entity) = entity_from_id(self.ctx.world, id) {
            self.ctx.world.despawn(entity);
            self.ctx
                .commands
                .push(ScriptCommand::Destroy { entity: id });
        }
    }

    /// Despawn the first entity with the given [`Name`], if any.
    pub fn destroy_by_name(&mut self, name: &str) {
        if let Some(id) = self.find_entity(name) {
            self.ctx.commands.push(ScriptCommand::DestroyByName {
                name: name.to_string(),
            });
            self.destroy(id);
        }
    }

    /// Find the first entity named `name`.
    pub fn find_entity(&self, name: &str) -> Option<EntityId> {
        self.ctx
            .world
            .query::<Name>()
            .find(|(_, n)| n.0 == name)
            .map(|(e, _)| entity_id(e))
    }

    /// `true` if the entity handle is still alive.
    pub fn is_alive(&self, id: EntityId) -> bool {
        entity_from_id(self.ctx.world, id).is_some()
    }

    // -- registered components (reflection) --------------------------------

    /// `true` if `entity` carries the component registered as `name` (a custom
    /// component) or the built-in component `name` that serializes into a scene.
    pub fn has_component(&self, id: EntityId, name: &str) -> bool {
        entity_from_id(self.ctx.world, id).is_some_and(|entity| {
            ecs::scene::has_reflected_component(
                &*self.ctx.world,
                entity,
                name,
                self.ctx.assets,
                self.ctx.components,
            )
        })
    }

    /// The reflected component `name` on `entity` as a JSON string, or `None`
    /// when the entity or component is absent. A built-in is its scene descriptor;
    /// a custom is its own JSON.
    pub fn get_component(&self, id: EntityId, name: &str) -> Option<String> {
        let entity = entity_from_id(self.ctx.world, id)?;
        let value = ecs::scene::get_reflected_component(
            &*self.ctx.world,
            entity,
            name,
            self.ctx.assets,
            self.ctx.components,
        )?;
        serde_json::to_string(&value).ok()
    }

    /// Set the reflected component `name` on `entity` from a JSON string.
    ///
    /// A parse failure, an unknown name, or a type/field mismatch is a `String`
    /// error (logged by the host), never a panic. Recorded as
    /// [`ScriptCommand::SetComponent`].
    pub fn set_component(&mut self, id: EntityId, name: &str, json: &str) -> Result<(), String> {
        let entity = entity_from_id(self.ctx.world, id)
            .ok_or_else(|| format!("set_component: no entity for handle {id}"))?;
        let value: serde_json::Value = serde_json::from_str(json)
            .map_err(|error| format!("set_component {name:?}: {error}"))?;
        ecs::scene::set_reflected_component(
            &mut *self.ctx.world,
            entity,
            name,
            &value,
            self.ctx.assets,
            self.ctx.components,
        )?;
        self.ctx.commands.push(ScriptCommand::SetComponent {
            entity: id,
            name: name.to_string(),
            json: json.to_string(),
        });
        Ok(())
    }

    /// Remove the reflected component `name` from `entity`.
    ///
    /// Returns whether one was removed. Recorded as
    /// [`ScriptCommand::RemoveComponent`].
    pub fn remove_component(&mut self, id: EntityId, name: &str) -> bool {
        let Some(entity) = entity_from_id(self.ctx.world, id) else {
            return false;
        };
        if ecs::scene::remove_reflected_component(
            &mut *self.ctx.world,
            entity,
            name,
            self.ctx.components,
        ) {
            self.ctx.commands.push(ScriptCommand::RemoveComponent {
                entity: id,
                name: name.to_string(),
            });
            true
        } else {
            false
        }
    }

    // -- transform ----------------------------------------------------------

    /// The entity's local position, or the origin if it is unknown.
    pub fn get_position(&self, id: EntityId) -> Vec3 {
        self.local(id).map_or_else(zero3, |t| t.translation)
    }

    /// Replace the entity's local position.
    pub fn set_position(&mut self, id: EntityId, position: Vec3) {
        if let Some(entity) = entity_from_id(self.ctx.world, id) {
            self.with_transform(entity, |t| t.translation = position);
            self.ctx.commands.push(ScriptCommand::SetPosition {
                entity: id,
                position,
            });
        }
    }

    /// Offset the entity's local position by `delta`.
    pub fn translate(&mut self, id: EntityId, delta: Vec3) {
        if let Some(entity) = entity_from_id(self.ctx.world, id) {
            self.with_transform(entity, |t| t.translation = t.translation + delta);
            self.ctx
                .commands
                .push(ScriptCommand::Translate { entity: id, delta });
        }
    }

    /// The entity's local rotation as Euler angles `(roll, pitch, yaw)`.
    pub fn get_rotation_euler(&self, id: EntityId) -> Vec3 {
        match self.local(id) {
            Some(t) => {
                let (roll, pitch, yaw) = t.rotation.to_euler_xyz();
                vec3(roll, pitch, yaw)
            }
            None => vec3(0.0, 0.0, 0.0),
        }
    }

    /// Set the entity's local rotation from Euler angles `(roll, pitch, yaw)`.
    pub fn set_rotation_euler(&mut self, id: EntityId, euler: Vec3) {
        if let Some(entity) = entity_from_id(self.ctx.world, id) {
            let rotation = Quat::from_euler_xyz(euler.x, euler.y, euler.z);
            self.with_transform(entity, |t| t.rotation = rotation);
            self.ctx
                .commands
                .push(ScriptCommand::SetRotationEuler { entity: id, euler });
        }
    }

    /// The entity's local scale, or one if it is unknown.
    pub fn get_scale(&self, id: EntityId) -> Vec3 {
        match self.local(id) {
            Some(t) => t.scale,
            None => vec3(1.0, 1.0, 1.0),
        }
    }

    /// Replace the entity's local scale.
    pub fn set_scale(&mut self, id: EntityId, scale: Vec3) {
        if let Some(entity) = entity_from_id(self.ctx.world, id) {
            self.with_transform(entity, |t| t.scale = scale);
            self.ctx
                .commands
                .push(ScriptCommand::SetScale { entity: id, scale });
        }
    }

    // -- time ---------------------------------------------------------------

    /// The current frame's delta time, seconds.
    pub fn time_delta(&self) -> f32 {
        self.ctx.delta
    }

    /// Total elapsed scaled time, seconds.
    pub fn time_elapsed(&self) -> f32 {
        self.ctx.elapsed
    }

    // -- input --------------------------------------------------------------

    /// `true` if the named key is held. Names match [`Key::name`] (`"KeyW"`,
    /// `"Space"`, `"ArrowLeft"`, ...).
    pub fn key_down(&self, name: &str) -> bool {
        Key::from_name(name).is_some_and(|key| self.ctx.input.key_down(key))
    }

    /// `true` if the named mouse button is held (`"left"`, `"right"`, or
    /// `"middle"`, case-insensitive).
    pub fn mouse_down(&self, button: &str) -> bool {
        let button = match button.to_ascii_lowercase().as_str() {
            "left" => MouseButton::Left,
            "right" => MouseButton::Right,
            "middle" => MouseButton::Middle,
            _ => return false,
        };
        self.ctx.input.mouse.is_down(button)
    }

    /// The cursor movement since the previous frame, logical pixels.
    pub fn mouse_delta(&self) -> [f32; 2] {
        self.ctx.input.mouse.delta
    }

    /// The cursor position in logical pixels.
    pub fn mouse_position(&self) -> [f32; 2] {
        self.ctx.input.mouse.position
    }

    /// The scroll wheel delta since the previous frame.
    pub fn mouse_scroll(&self) -> f32 {
        self.ctx.input.mouse.scroll
    }

    // -- physics ------------------------------------------------------------

    /// Overwrite the entity's body linear velocity.
    ///
    /// Recorded as [`ScriptCommand::SetVelocity`]; the script API has no physics
    /// handle, so the runtime applies it after the call. Does nothing when the
    /// entity has no dynamic body.
    pub fn set_velocity(&mut self, id: EntityId, velocity: Vec3) {
        self.ctx.commands.push(ScriptCommand::SetVelocity {
            entity: id,
            velocity,
        });
    }

    /// Offset the entity's body linear velocity by `delta`.
    ///
    /// Recorded as [`ScriptCommand::AddVelocity`]; applied by the runtime.
    pub fn add_velocity(&mut self, id: EntityId, delta: Vec3) {
        self.ctx.commands.push(ScriptCommand::AddVelocity {
            entity: id,
            velocity: delta,
        });
    }

    /// Apply an instantaneous impulse to the entity's body.
    ///
    /// Recorded as [`ScriptCommand::ApplyImpulse`]; applied by the runtime.
    pub fn apply_impulse(&mut self, id: EntityId, impulse: Vec3) {
        self.ctx.commands.push(ScriptCommand::ApplyImpulse {
            entity: id,
            velocity: impulse,
        });
    }

    /// The entity's body linear velocity from this frame's snapshot, or zero.
    pub fn get_velocity(&self, id: EntityId) -> Vec3 {
        self.ctx
            .physics
            .map_or_else(zero3, |view| view.velocity(id))
    }

    /// Whether the entity is grounded, from this frame's snapshot.
    pub fn is_grounded(&self, id: EntityId) -> bool {
        self.ctx.physics.is_some_and(|view| view.is_grounded(id))
    }

    // -- scenes -------------------------------------------------------------

    /// Ask the runtime to load `path`, replacing the current scene.
    ///
    /// Recorded as [`ScriptCommand::LoadScene`]; the runtime performs the swap at
    /// the end of the frame, because the script API cannot replace the world it
    /// is running against mid-call.
    pub fn load_scene(&mut self, path: impl Into<String>) {
        self.ctx
            .commands
            .push(ScriptCommand::LoadScene { path: path.into() });
    }

    // -- scenes and prefabs -------------------------------------------------

    /// Attach (or replace) a [`Script`] component on an entity at runtime.
    ///
    /// Returns `true` if the entity existed. The script starts on the next
    /// frame.
    pub fn add_script(&mut self, id: EntityId, source: impl Into<String>) -> bool {
        let Some(entity) = entity_from_id(self.ctx.world, id) else {
            return false;
        };
        let source = source.into();
        self.ctx
            .world
            .insert(entity, Script::from_source(source.clone()));
        self.ctx
            .commands
            .push(ScriptCommand::AddScript { entity: id, source });
        true
    }

    /// Instantiate a prefab from its JSON under an optional parent, returning
    /// the new entities in document order.
    ///
    /// A malformed document logs an error and returns an empty list rather than
    /// panicking.
    pub fn instantiate_prefab(&mut self, json: &str, parent: Option<EntityId>) -> Vec<EntityId> {
        let prefab = match Prefab::from_json(json) {
            Ok(prefab) => prefab,
            Err(err) => {
                self.ctx.commands.push(ScriptCommand::Log {
                    message: format!("instantiate_prefab: {err}"),
                    entity: self.ctx.entity,
                });
                return Vec::new();
            }
        };
        let parent = parent.and_then(|id| entity_from_id(self.ctx.world, id));
        let created = prefab.instantiate(self.ctx.world, self.ctx.assets, parent);
        let spawned: Vec<EntityId> = created.iter().map(|e| entity_id(*e)).collect();
        self.ctx.commands.push(ScriptCommand::InstantiatePrefab {
            json: json.to_string(),
            parent: parent.map(entity_id),
            spawned: spawned.clone(),
            root: None,
        });
        spawned
    }

    // -- internals ----------------------------------------------------------

    fn local(&self, id: EntityId) -> Option<&LocalTransform> {
        entity_from_id(self.ctx.world, id).and_then(|e| self.ctx.world.get::<LocalTransform>(e))
    }

    fn with_transform<F: FnOnce(&mut LocalTransform)>(&mut self, entity: Entity, f: F) {
        if let Some(t) = self.ctx.world.get_mut::<LocalTransform>(entity) {
            f(t);
            return;
        }
        let mut t = LocalTransform::default();
        f(&mut t);
        self.ctx.world.insert(entity, t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build<'a>(
        world: &'a mut World,
        assets: &'a AssetRegistry,
        input: &'a InputState,
    ) -> ScriptContext<'a> {
        ScriptContext::new(world, assets, empty_registry(), input)
    }

    /// A process-wide empty registry, so the transform/entity tests need no owned
    /// one (only the reflection tests do).
    fn empty_registry() -> &'static ComponentRegistry {
        static EMPTY: std::sync::OnceLock<ComponentRegistry> = std::sync::OnceLock::new();
        EMPTY.get_or_init(ComponentRegistry::new)
    }

    #[test]
    fn entity_id_round_trips_and_rejects_stale_handles() {
        let mut world = World::new();
        let e = world.spawn();
        let id = entity_id(e);
        assert_eq!(entity_from_id(&world, id), Some(e));
        world.despawn(e);
        assert_eq!(entity_from_id(&world, id), None);
    }

    #[test]
    fn spawn_adds_transform_and_name() {
        let mut world = World::new();
        let assets = AssetRegistry::new();
        let input = InputState::new();
        let mut ctx = build(&mut world, &assets, &input);

        let id = ctx.api().spawn(Some("Child".into()));

        assert_eq!(
            ctx.commands,
            vec![ScriptCommand::Spawn {
                name: Some("Child".into()),
                entity: id
            }]
        );
        drop(ctx);

        let entity = entity_from_id(&world, id).expect("alive");
        assert!(world.has::<LocalTransform>(entity));
        assert_eq!(
            world.get::<Name>(entity).map(|n| n.0.as_str()),
            Some("Child")
        );
    }

    #[test]
    fn set_and_get_position_round_trips() {
        let mut world = World::new();
        let assets = AssetRegistry::new();
        let input = InputState::new();
        let mut ctx = build(&mut world, &assets, &input);

        let id = ctx.api().spawn(None);
        {
            let mut api = ctx.api();
            api.set_position(id, vec3(1.0, 2.0, 3.0));
            assert_eq!(api.get_position(id), vec3(1.0, 2.0, 3.0));
        }
        assert!(matches!(
            ctx.commands.last(),
            Some(ScriptCommand::SetPosition { .. })
        ));
        drop(ctx);

        let entity = entity_from_id(&world, id).unwrap();
        assert_eq!(
            world.get::<LocalTransform>(entity).unwrap().translation,
            vec3(1.0, 2.0, 3.0)
        );
    }

    #[test]
    fn translate_accumulates() {
        let mut world = World::new();
        let assets = AssetRegistry::new();
        let input = InputState::new();
        let mut ctx = build(&mut world, &assets, &input);
        let id = ctx.api().spawn(None);

        let mut api = ctx.api();
        api.translate(id, vec3(1.0, 0.0, 0.0));
        api.translate(id, vec3(0.0, 2.0, 0.0));
        assert_eq!(api.get_position(id), vec3(1.0, 2.0, 0.0));
    }

    #[test]
    fn rotation_euler_round_trips() {
        let mut world = World::new();
        let assets = AssetRegistry::new();
        let input = InputState::new();
        let mut ctx = build(&mut world, &assets, &input);
        let id = ctx.api().spawn(None);

        let mut api = ctx.api();
        api.set_rotation_euler(id, vec3(0.1, 0.2, 0.3));
        let read = api.get_rotation_euler(id);
        assert!((read.x - 0.1).abs() < 1e-4);
        assert!((read.y - 0.2).abs() < 1e-4);
        assert!((read.z - 0.3).abs() < 1e-4);
    }

    #[test]
    fn scale_round_trips_and_defaults_to_one() {
        let mut world = World::new();
        let assets = AssetRegistry::new();
        let input = InputState::new();
        let mut ctx = build(&mut world, &assets, &input);
        let id = ctx.api().spawn(None);

        let mut api = ctx.api();
        assert_eq!(api.get_scale(id), vec3(1.0, 1.0, 1.0));
        api.set_scale(id, vec3(2.0, 2.0, 2.0));
        assert_eq!(api.get_scale(id), vec3(2.0, 2.0, 2.0));
    }

    #[test]
    fn set_position_on_missing_entity_inserts_transform() {
        let mut world = World::new();
        let assets = AssetRegistry::new();
        let input = InputState::new();
        let bare = world.spawn();
        let mut ctx = build(&mut world, &assets, &input);

        ctx.api().set_position(entity_id(bare), vec3(5.0, 0.0, 0.0));
        drop(ctx);
        assert_eq!(
            world.get::<LocalTransform>(bare).unwrap().translation,
            vec3(5.0, 0.0, 0.0)
        );
    }

    #[test]
    fn find_and_destroy_by_name() {
        let mut world = World::new();
        let assets = AssetRegistry::new();
        let input = InputState::new();
        let mut ctx = build(&mut world, &assets, &input);
        let id = ctx.api().spawn(Some("Target".into()));

        {
            let mut api = ctx.api();
            assert_eq!(api.find_entity("Target"), Some(id));
            assert_eq!(api.find_entity("Missing"), None);
            api.destroy_by_name("Target");
            assert!(!api.is_alive(id));
        }
        drop(ctx);
        assert_eq!(world.count::<Name>(), 0);
    }

    #[test]
    fn destroy_removes_components() {
        let mut world = World::new();
        let assets = AssetRegistry::new();
        let input = InputState::new();
        let mut ctx = build(&mut world, &assets, &input);
        let id = ctx.api().spawn(Some("Gone".into()));

        ctx.api().destroy(id);
        drop(ctx);
        let entity = Entity::from_raw(id as u32, (id >> 32) as u32);
        assert!(!world.is_alive(entity));
        assert!(!world.has::<LocalTransform>(entity));
    }

    #[test]
    fn time_and_input_accessors() {
        let mut world = World::new();
        let assets = AssetRegistry::new();
        let mut input = InputState::new();
        input.press_key(Key::Space);
        input.press_mouse(MouseButton::Left);
        let mut ctx = build(&mut world, &assets, &input);
        ctx.delta = 0.016;
        ctx.elapsed = 1.5;

        let api = ctx.api();
        assert!((api.time_delta() - 0.016).abs() < 1e-6);
        assert!((api.time_elapsed() - 1.5).abs() < 1e-6);
        assert!(api.key_down("Space"));
        assert!(!api.key_down("KeyW"));
        assert!(!api.key_down("NotAKey"));
        assert!(api.mouse_down("left"));
        assert!(!api.mouse_down("middle"));
        assert!(!api.mouse_down("bogus"));
    }

    #[test]
    fn add_script_replaces_component() {
        let mut world = World::new();
        let assets = AssetRegistry::new();
        let input = InputState::new();
        let mut ctx = build(&mut world, &assets, &input);
        let id = ctx.api().spawn(None);

        let added = ctx.api().add_script(id, "function on_start(){}");
        let rejected = ctx.api().add_script(999_999, "x");
        assert!(added);
        assert!(!rejected);
        drop(ctx);

        let entity = entity_from_id(&world, id).unwrap();
        assert_eq!(
            world.get::<Script>(entity).unwrap().source.as_deref(),
            Some("function on_start(){}")
        );
    }

    #[test]
    fn instantiate_prefab_creates_entities() {
        let mut world = World::new();
        let assets = AssetRegistry::new();
        let input = InputState::new();
        let mut ctx = build(&mut world, &assets, &input);
        let parent = ctx.api().spawn(Some("Root".into()));

        let json = r#"{
            "name": "Blob",
            "scene": {
                "name": "Blob",
                "entities": [
                    { "id": 0, "name": "A",
                      "transform": { "translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] } },
                    { "id": 1, "name": "B", "parent": 0,
                      "transform": { "translation": [1,1,1], "rotation": [0,0,0,1], "scale": [1,1,1] } }
                ]
            }
        }"#;

        let spawned = ctx.api().instantiate_prefab(json, Some(parent));
        assert_eq!(spawned.len(), 2);
        drop(ctx);
        for id in &spawned {
            let entity = entity_from_id(&world, *id).expect("alive");
            assert!(world.has::<LocalTransform>(entity));
        }
        assert!(
            world
                .get::<Name>(entity_from_id(&world, spawned[0]).unwrap())
                .is_some()
        );
    }

    #[test]
    fn instantiate_prefab_reports_parse_errors() {
        let mut world = World::new();
        let assets = AssetRegistry::new();
        let input = InputState::new();
        let mut ctx = build(&mut world, &assets, &input);

        let spawned = ctx.api().instantiate_prefab("not json", None);
        assert!(spawned.is_empty());
        assert!(matches!(
            ctx.commands.last(),
            Some(ScriptCommand::Log { .. })
        ));
    }

    #[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize, ecs::Component)]
    struct Health {
        current: f32,
        max: f32,
    }

    fn health_registry() -> ComponentRegistry {
        let mut registry = ComponentRegistry::new();
        registry.register::<Health>();
        registry
    }

    #[test]
    fn reflection_reads_writes_and_removes_a_component() {
        let mut world = World::new();
        let assets = AssetRegistry::new();
        let input = InputState::new();
        let registry = health_registry();
        let mut ctx = ScriptContext::new(&mut world, &assets, &registry, &input);
        let id = ctx.api().spawn(Some("Hero".into()));

        assert!(!ctx.api().has_component(id, "Health"));
        assert_eq!(ctx.api().get_component(id, "Health"), None);

        {
            let mut api = ctx.api();
            api.set_component(id, "Health", r#"{"current":3.0,"max":10.0}"#)
                .expect("set");
            assert!(api.has_component(id, "Health"));
            assert_eq!(
                api.get_component(id, "Health").as_deref(),
                Some(r#"{"current":3.0,"max":10.0}"#)
            );
            assert!(api.remove_component(id, "Health"));
            assert!(!api.has_component(id, "Health"));
            assert!(
                !api.remove_component(id, "Health"),
                "removing again is false"
            );
        }

        assert!(matches!(
            ctx.commands.first(),
            Some(ScriptCommand::Spawn { .. })
        ));
        assert!(ctx.commands.iter().any(|c| matches!(
            c,
            ScriptCommand::SetComponent { name, .. } if name == "Health"
        )));
        assert!(ctx.commands.iter().any(|c| matches!(
            c,
            ScriptCommand::RemoveComponent { name, .. } if name == "Health"
        )));

        let entity = Entity::from_raw(id as u32, (id >> 32) as u32);
        assert!(
            world.get::<Health>(entity).is_none(),
            "the component was removed"
        );
    }

    #[test]
    fn a_bad_set_component_is_an_error_not_a_panic() {
        let mut world = World::new();
        let assets = AssetRegistry::new();
        let input = InputState::new();
        let registry = health_registry();
        let mut ctx = ScriptContext::new(&mut world, &assets, &registry, &input);
        let id = ctx.api().spawn(None);
        let mut api = ctx.api();

        // Unparseable JSON.
        let err = api.set_component(id, "Health", "not json").unwrap_err();
        assert!(err.contains("Health"), "{err}");
        // Withheld field.
        let err = api
            .set_component(id, "Health", r#"{"current":3.0}"#)
            .unwrap_err();
        assert!(!err.is_empty());
        // Unregistered name.
        let err = api.set_component(id, "Nope", "{}").unwrap_err();
        assert!(err.contains("Nope"), "{err}");
        // A dead handle.
        assert!(api.set_component(999_999, "Health", "{}").is_err());
        // None of that recorded a command or mutated the world.
        assert!(!api.has_component(id, "Health"));
    }

    #[test]
    fn reflection_reads_and_writes_a_builtin_component() {
        use ecs::components::RigidBody;

        let mut world = World::new();
        let assets = AssetRegistry::new();
        let input = InputState::new();
        let registry = ComponentRegistry::new();
        let mut ctx = ScriptContext::new(&mut world, &assets, &registry, &input);
        let id = ctx.api().spawn(None);

        assert!(!ctx.api().has_component(id, "RigidBody"));

        let json = r#"{"type":"rigid_body","body_type":"dynamic","mass":5.0,"restitution":0.0,"friction":0.5,"gravity_scale":1.0,"linear_velocity":[0.0,0.0,0.0],"angular_velocity":[0.0,0.0,0.0]}"#;
        {
            let mut api = ctx.api();
            api.set_component(id, "RigidBody", json).expect("set");
            assert!(api.has_component(id, "RigidBody"));

            let read = api.get_component(id, "RigidBody").expect("get");
            assert!(read.contains("\"type\":\"rigid_body\""), "{read}");
            assert!(read.contains("\"mass\":5.0"), "{read}");

            // Round-trip: change a field and set it back.
            let mut value: serde_json::Value = serde_json::from_str(&read).unwrap();
            value["mass"] = serde_json::json!(9.0);
            api.set_component(id, "RigidBody", &value.to_string())
                .expect("re-set");

            // A mismatched type is an error, not a panic.
            assert!(api.set_component(id, "Light", json).is_err());

            assert!(api.remove_component(id, "RigidBody"));
            assert!(!api.has_component(id, "RigidBody"));
        }

        assert!(
            ctx.commands.iter().any(
                |c| matches!(c, ScriptCommand::SetComponent { name, .. } if name == "RigidBody")
            )
        );

        let entity = Entity::from_raw(id as u32, (id >> 32) as u32);
        assert!(world.get::<RigidBody>(entity).is_none());
    }

    #[test]
    fn a_builtin_mass_survives_a_round_trip() {
        use ecs::components::RigidBody;

        let mut world = World::new();
        let assets = AssetRegistry::new();
        let input = InputState::new();
        let registry = ComponentRegistry::new();
        let mut ctx = ScriptContext::new(&mut world, &assets, &registry, &input);
        let id = ctx.api().spawn(None);

        {
            let mut api = ctx.api();
            api.set_component(
                id,
                "RigidBody",
                r#"{"type":"rigid_body","body_type":"fixed","mass":3.0,"restitution":0.2,"friction":0.4,"gravity_scale":0.0,"linear_velocity":[1.0,2.0,3.0],"angular_velocity":[0.0,0.0,0.0]}"#,
            )
            .expect("set");
        }

        let entity = Entity::from_raw(id as u32, (id >> 32) as u32);
        let body = world
            .get::<RigidBody>(entity)
            .expect("the built-in was inserted");
        assert_eq!(body.body_type, ecs::components::BodyType::Fixed);
        assert_eq!(body.mass, 3.0);
        assert_eq!(body.linear_velocity, vec3(1.0, 2.0, 3.0));
    }
}
