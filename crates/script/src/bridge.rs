//! The shared host bridge used by every script backend.
//!
//! A script engine's registered callbacks cannot borrow the [`ScriptContext`] for
//! the duration of a call (the engine owns the callback and the engine is not
//! `Send`, so the borrow cannot be threaded through it). So each call is
//! two-phase:
//!
//! 1. **Snapshot + queue.** The host copies the parts of the engine a script can
//!    read (transforms, names, input, timing) into a [`Bridge`], then runs the
//!    script. Its callbacks read the snapshot and append [`ScriptCommand`]s.
//!    Entities spawned during a call get *placeholder* handles, and the snapshot
//!    is updated in place so a later `set_position(spawn(), ...)` in the same
//!    call sees the change.
//! 2. **Replay.** After the script returns, [`replay`] applies every queued
//!    command through [`ScriptApi`], translating placeholder handles to the real
//!    entities they became.
//!
//! Keeping all world mutation in [`ScriptApi`] means no host needs `unsafe`, and
//! the JavaScript backend and the native WebAssembly host share one
//! implementation of the snapshot/replay protocol.

use std::collections::HashMap;

use ecs::components::{LocalTransform, Name};
use ecs::resources::InputState;
use ecs::scene::reflect_components;
use math::{Quat, Vec3, vec3};

use crate::api::{EntityId, ScriptApi, ScriptCommand, ScriptContext, entity_id};

/// The generation stamped on placeholder handles handed to a script for entities
/// spawned during a call. Chosen so it can never collide with a real entity's
/// generation.
pub(crate) const PLACEHOLDER_GENERATION: u32 = u32::MAX;

/// A placeholder handle for the `slot`-th entity spawned during a call.
pub(crate) fn placeholder_id(slot: u64) -> EntityId {
    ((PLACEHOLDER_GENERATION as u64) << 32) | slot
}

/// The opaque handle index for `id` within a call, allocating one if needed.
///
/// Script engines receive entity handles as small integers: a raw [`EntityId`] is
/// ~2^64 and would lose precision as a JavaScript `f64`, so a script addresses an
/// entity through this per-call index table instead. The table is cleared at the
/// start of each call and read back with [`entity_of`].
pub(crate) fn handle_of(handles: &mut Vec<EntityId>, id: EntityId) -> i32 {
    if let Some(index) = handles.iter().position(|h| *h == id) {
        return index as i32;
    }
    handles.push(id);
    (handles.len() - 1) as i32
}

/// The entity a call's handle index names, if the call handed one out.
pub(crate) fn entity_of(handles: &[EntityId], index: i32) -> Option<EntityId> {
    usize::try_from(index)
        .ok()
        .and_then(|i| handles.get(i))
        .copied()
}

/// The Euler angles `(roll, pitch, yaw)`, radians, of a rotation.
pub(crate) fn euler(q: Quat) -> Vec3 {
    let (roll, pitch, yaw) = q.to_euler_xyz();
    vec3(roll, pitch, yaw)
}

/// Shared, single-threaded state a script call reads and writes.
#[derive(Default)]
pub(crate) struct Bridge {
    /// The entity the current lifecycle call pertains to.
    pub(crate) entity: Option<EntityId>,
    /// Delta time for this call, seconds.
    pub(crate) delta: f32,
    /// Total elapsed time, seconds.
    pub(crate) elapsed: f32,
    /// A snapshot of the input state.
    pub(crate) input: InputState,
    /// Snapshot's local translations, keyed by entity handle.
    pub(crate) positions: HashMap<EntityId, Vec3>,
    /// Snapshot's local rotations (Euler, radians).
    pub(crate) rotations: HashMap<EntityId, Vec3>,
    /// Snapshot's local scales.
    pub(crate) scales: HashMap<EntityId, Vec3>,
    /// Names in the snapshot, for `find`.
    pub(crate) names: HashMap<String, EntityId>,
    /// Registered components in the snapshot, keyed by entity then component
    /// name, as JSON strings (the reflection registry's serialization).
    pub(crate) components: HashMap<EntityId, HashMap<String, String>>,
    /// The number of placeholders handed out so far this call.
    pub(crate) next_placeholder: u64,
    /// Commands queued during the call, replayed afterwards.
    pub(crate) commands: Vec<ScriptCommand>,
}

impl Bridge {
    /// Reset the bridge to a fresh snapshot of `ctx`'s world.
    pub(crate) fn begin(&mut self, ctx: &ScriptContext<'_>) {
        self.entity = ctx.entity;
        self.delta = ctx.delta;
        self.elapsed = ctx.elapsed;
        self.input = ctx.input.clone();
        self.positions.clear();
        self.rotations.clear();
        self.scales.clear();
        self.names.clear();
        self.components.clear();
        self.next_placeholder = 0;
        self.commands.clear();

        // Reflected components (built-ins in scene form, registered customs in
        // their own JSON) are serialized once per call — the cost of the generic
        // path; a lazily-populated cache is a later optimization.
        for entity in ctx.world.entities().iter() {
            let id = entity_id(entity);
            if let Some(name) = ctx.world.get::<Name>(entity) {
                self.names.insert(name.0.clone(), id);
            }
            if let Some(t) = ctx.world.get::<LocalTransform>(entity) {
                self.positions.insert(id, t.translation);
                self.rotations.insert(id, euler(t.rotation));
                self.scales.insert(id, t.scale);
            }
            let mut map = HashMap::new();
            for (name, value) in reflect_components(&*ctx.world, entity, ctx.assets, ctx.components)
            {
                if let Ok(text) = serde_json::to_string(&value) {
                    map.insert(name, text);
                }
            }
            if !map.is_empty() {
                self.components.insert(id, map);
            }
        }
    }

    /// Queue a spawn, returning its placeholder handle.
    pub(crate) fn spawn(&mut self, name: Option<String>) -> EntityId {
        let id = placeholder_id(self.next_placeholder);
        self.next_placeholder += 1;
        if let Some(name) = &name {
            self.names.insert(name.clone(), id);
        }
        self.positions.insert(id, vec3(0.0, 0.0, 0.0));
        self.rotations.insert(id, vec3(0.0, 0.0, 0.0));
        self.scales.insert(id, vec3(1.0, 1.0, 1.0));
        self.commands
            .push(ScriptCommand::Spawn { name, entity: id });
        id
    }

    /// The snapshot position of `id`.
    pub(crate) fn position(&self, id: EntityId) -> Vec3 {
        self.positions
            .get(&id)
            .copied()
            .unwrap_or_else(|| vec3(0.0, 0.0, 0.0))
    }

    /// Queue a position write.
    pub(crate) fn set_position(&mut self, id: EntityId, position: Vec3) {
        self.positions.insert(id, position);
        self.commands.push(ScriptCommand::SetPosition {
            entity: id,
            position,
        });
    }

    /// Queue a relative translation.
    pub(crate) fn translate(&mut self, id: EntityId, delta: Vec3) {
        let next = self.position(id) + delta;
        self.positions.insert(id, next);
        self.commands
            .push(ScriptCommand::Translate { entity: id, delta });
    }

    /// The snapshot rotation of `id`.
    pub(crate) fn rotation(&self, id: EntityId) -> Vec3 {
        self.rotations
            .get(&id)
            .copied()
            .unwrap_or_else(|| vec3(0.0, 0.0, 0.0))
    }

    /// Queue a rotation write.
    pub(crate) fn set_rotation(&mut self, id: EntityId, euler: Vec3) {
        self.rotations.insert(id, euler);
        self.commands
            .push(ScriptCommand::SetRotationEuler { entity: id, euler });
    }

    /// The snapshot scale of `id`.
    pub(crate) fn scale(&self, id: EntityId) -> Vec3 {
        self.scales
            .get(&id)
            .copied()
            .unwrap_or_else(|| vec3(1.0, 1.0, 1.0))
    }

    /// Queue a scale write.
    pub(crate) fn set_scale(&mut self, id: EntityId, scale: Vec3) {
        self.scales.insert(id, scale);
        self.commands
            .push(ScriptCommand::SetScale { entity: id, scale });
    }

    /// Queue a prefab instantiation, returning a placeholder for the prefab's
    /// root entity so the caller can address it immediately (the real entities
    /// appear at replay).
    pub(crate) fn instantiate_prefab(
        &mut self,
        json: String,
        parent: Option<EntityId>,
    ) -> EntityId {
        let root = placeholder_id(self.next_placeholder);
        self.next_placeholder += 1;
        self.commands.push(ScriptCommand::InstantiatePrefab {
            json,
            parent,
            spawned: Vec::new(),
            root: Some(root),
        });
        root
    }

    /// The handle of the entity named `name`, if the snapshot has one.
    pub(crate) fn find(&self, name: &str) -> Option<EntityId> {
        self.names.get(name).copied()
    }

    /// Whether the snapshot has the component `name` on `id`.
    pub(crate) fn has_component(&self, id: EntityId, name: &str) -> bool {
        self.components
            .get(&id)
            .is_some_and(|map| map.contains_key(name))
    }

    /// The snapshot's JSON for the component `name` on `id`.
    pub(crate) fn component_json(&self, id: EntityId, name: &str) -> Option<String> {
        self.components
            .get(&id)
            .and_then(|map| map.get(name))
            .cloned()
    }

    /// Queue a component write, updating the snapshot so a later read in the same
    /// call sees it.
    pub(crate) fn set_component(&mut self, id: EntityId, name: &str, json: String) {
        self.components
            .entry(id)
            .or_default()
            .insert(name.to_string(), json.clone());
        self.commands.push(ScriptCommand::SetComponent {
            entity: id,
            name: name.to_string(),
            json,
        });
    }

    /// Queue a component removal.
    pub(crate) fn remove_component(&mut self, id: EntityId, name: &str) {
        if let Some(map) = self.components.get_mut(&id) {
            map.remove(name);
        }
        self.commands.push(ScriptCommand::RemoveComponent {
            entity: id,
            name: name.to_string(),
        });
    }
}

/// Replay a call's queued commands against the real world, translating
/// placeholder handles spawned during the call to the entities they became.
pub(crate) fn replay(ctx: &mut ScriptContext<'_>, commands: Vec<ScriptCommand>) {
    fn resolve(map: &HashMap<EntityId, EntityId>, id: EntityId) -> EntityId {
        map.get(&id).copied().unwrap_or(id)
    }

    let mut map: HashMap<EntityId, EntityId> = HashMap::new();
    let mut api = ScriptApi::new(ctx);
    for command in commands {
        match command {
            ScriptCommand::Log { message, .. } => api.log(message),
            ScriptCommand::Spawn { name, entity } => {
                let real = api.spawn(name);
                map.insert(entity, real);
            }
            ScriptCommand::Destroy { entity } => api.destroy(resolve(&map, entity)),
            ScriptCommand::DestroyByName { name } => api.destroy_by_name(&name),
            ScriptCommand::SetPosition { entity, position } => {
                api.set_position(resolve(&map, entity), position);
            }
            ScriptCommand::Translate { entity, delta } => {
                api.translate(resolve(&map, entity), delta);
            }
            ScriptCommand::SetRotationEuler { entity, euler } => {
                api.set_rotation_euler(resolve(&map, entity), euler);
            }
            ScriptCommand::SetScale { entity, scale } => {
                api.set_scale(resolve(&map, entity), scale);
            }
            ScriptCommand::AddScript { entity, source } => {
                api.add_script(resolve(&map, entity), source);
            }
            ScriptCommand::InstantiatePrefab {
                json, parent, root, ..
            } => {
                let parent = parent.map(|p| resolve(&map, p));
                let created = api.instantiate_prefab(&json, parent);
                // The caller was handed `root` as the prefab's root entity; bind
                // it now that the real entities exist.
                if let (Some(placeholder), Some(first)) = (root, created.first()) {
                    map.insert(placeholder, *first);
                }
            }
            ScriptCommand::SetComponent { entity, name, json } => {
                // A bad JSON body or type mismatch is a logged script error, not a
                // panic; the script keeps running.
                if let Err(error) = api.set_component(resolve(&map, entity), &name, &json) {
                    log::warn!("script set_component failed: {error}");
                }
            }
            ScriptCommand::RemoveComponent { entity, name } => {
                api.remove_component(resolve(&map, entity), &name);
            }
        }
    }
}
