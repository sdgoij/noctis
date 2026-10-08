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
    /// A script component was added or replaced at runtime.
    AddScript {
        /// The entity.
        entity: EntityId,
        /// The new source.
        source: String,
    },
    /// A prefab was instantiated.
    InstantiatePrefab {
        /// The prefab JSON that was parsed.
        json: String,
        /// The parent the instances were attached to, if any.
        parent: Option<EntityId>,
        /// The entities that were created, in document order.
        spawned: Vec<EntityId>,
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
    /// Input state for the current frame.
    pub input: &'a InputState,
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
    pub fn new(world: &'a mut World, assets: &'a AssetRegistry, input: &'a InputState) -> Self {
        Self {
            world,
            assets,
            input,
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
        ScriptContext::new(world, assets, input)
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
}
