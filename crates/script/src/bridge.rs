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
//!
//! The reflected-component part of the snapshot (the JSON of every entity's
//! components) is the expensive part, and only a script that *reads* it can
//! observe it. So the host builds it only for a script that names a reflection
//! read entry point (see [`source_reads_components`]); a script that never does
//! pays nothing per frame. Transforms, names, input, timing, and the physics
//! snapshot are always taken, since any script may read those.

use std::collections::{HashMap, HashSet};

use ecs::components::{LocalTransform, Name};
use ecs::resources::InputState;
use ecs::scene::reflect_components;
use math::{Quat, Vec3, vec3};

use crate::api::{EntityId, ScriptApi, ScriptCommand, ScriptContext, entity_id};
use crate::wasm_host::{decode_source, is_wasm_source};

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
    /// Entities the physics snapshot marked grounded this frame.
    pub(crate) grounded: HashSet<EntityId>,
    /// Linear velocities from the physics snapshot, keyed by entity handle.
    pub(crate) velocities: HashMap<EntityId, Vec3>,
    /// Sensor overlap pairs from the physics snapshot.
    pub(crate) overlaps: Vec<(EntityId, EntityId)>,
    /// The number of placeholders handed out so far this call.
    pub(crate) next_placeholder: u64,
    /// Commands queued during the call, replayed afterwards.
    pub(crate) commands: Vec<ScriptCommand>,
}

impl Bridge {
    /// Reset the bridge to a fresh snapshot of `ctx`'s world.
    ///
    /// `reflect` controls whether the reflected-component JSON is built (the
    /// dominant cost of a snapshot). Pass `false` for a script that never names
    /// a reflection read entry point — see [`source_reads_components`] — and the
    /// component map stays empty. Everything else (transforms, names, input,
    /// timing, the physics snapshot) is always taken.
    pub(crate) fn begin(&mut self, ctx: &ScriptContext<'_>, reflect: bool) {
        self.entity = ctx.entity;
        self.delta = ctx.delta;
        self.elapsed = ctx.elapsed;
        self.input = ctx.input.clone();
        self.positions.clear();
        self.rotations.clear();
        self.scales.clear();
        self.names.clear();
        self.components.clear();
        self.grounded.clear();
        self.velocities.clear();
        self.overlaps.clear();
        self.next_placeholder = 0;
        self.commands.clear();

        if let Some(view) = ctx.physics {
            self.grounded.extend(view.grounded.iter().copied());
            self.velocities
                .extend(view.velocities.iter().map(|(id, v)| (*id, *v)));
            self.overlaps.extend(view.overlaps.iter().copied());
        }

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
            if !reflect {
                continue;
            }
            // Reflected components (built-ins in scene form, registered customs in
            // their own JSON) are serialized once per call, for every entity — the
            // cost of the generic path, so only taken when the script can read it.
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

    // -- physics ------------------------------------------------------------

    /// Whether the snapshot marked `id` grounded.
    pub(crate) fn is_grounded(&self, id: EntityId) -> bool {
        self.grounded.contains(&id)
    }

    /// The snapshot's linear velocity for `id`, or zero.
    pub(crate) fn velocity(&self, id: EntityId) -> Vec3 {
        self.velocities
            .get(&id)
            .copied()
            .unwrap_or_else(|| vec3(0.0, 0.0, 0.0))
    }

    /// Whether the snapshot has `a` and `b` overlapping (in either order).
    pub(crate) fn is_overlapping(&self, a: EntityId, b: EntityId) -> bool {
        self.overlaps
            .iter()
            .any(|(x, y)| (*x == a && *y == b) || (*x == b && *y == a))
    }

    /// Queue a linear-velocity overwrite (applied by the runtime).
    pub(crate) fn set_velocity(&mut self, id: EntityId, velocity: Vec3) {
        self.velocities.insert(id, velocity);
        self.commands.push(ScriptCommand::SetVelocity {
            entity: id,
            velocity,
        });
    }

    /// Queue a linear-velocity offset (applied by the runtime).
    pub(crate) fn add_velocity(&mut self, id: EntityId, delta: Vec3) {
        self.commands.push(ScriptCommand::AddVelocity {
            entity: id,
            velocity: delta,
        });
    }

    /// Queue an impulse (applied by the runtime).
    pub(crate) fn apply_impulse(&mut self, id: EntityId, impulse: Vec3) {
        self.commands.push(ScriptCommand::ApplyImpulse {
            entity: id,
            velocity: impulse,
        });
    }

    /// Queue a scene load (applied by the runtime).
    pub(crate) fn load_scene(&mut self, path: String) {
        self.commands.push(ScriptCommand::LoadScene { path });
    }

    /// Queue a scene load that carries a named entity (applied by the runtime).
    pub(crate) fn teleport_scene(&mut self, path: String, carry: String) {
        self.commands
            .push(ScriptCommand::TeleportScene { path, carry });
    }

    /// Queue a cursor-capture request (applied by the runtime).
    pub(crate) fn set_mouse_captured(&mut self, captured: bool) {
        self.commands
            .push(ScriptCommand::SetMouseCaptured { captured });
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
            ScriptCommand::SetVelocity { entity, velocity } => {
                api.set_velocity(resolve(&map, entity), velocity);
            }
            ScriptCommand::AddVelocity { entity, velocity } => {
                api.add_velocity(resolve(&map, entity), velocity);
            }
            ScriptCommand::ApplyImpulse { entity, velocity } => {
                api.apply_impulse(resolve(&map, entity), velocity);
            }
            ScriptCommand::LoadScene { path } => api.load_scene(path),
            ScriptCommand::TeleportScene { path, carry } => api.teleport_scene(path, carry),
            ScriptCommand::SetMouseCaptured { captured } => api.set_mouse_captured(captured),
        }
    }
}

/// Whether a script source may *read* reflected components, and so needs the
/// per-call component snapshot built (see [`Bridge::begin`]).
///
/// The bridge must answer reflection reads from its snapshot — its host
/// callbacks cannot borrow the world — so whether to build the snapshot has to be
/// decided *before* the call runs. This inspects the source for the read entry
/// points the hosts register (`has_component`, `get_component`,
/// `remove_component`); a script that names none can never observe the snapshot,
/// so the host skips building it. `set_component` is deliberately not a trigger:
/// a write needs no snapshot.
///
/// A `.wasm` module travels as a hex descriptor (see
/// [`is_wasm_source`](crate::wasm_host::is_wasm_source)), so its import names are
/// searched in the decoded binary instead of in the text.
///
/// The check is conservative: a false positive (a name in a string or comment)
/// only costs the snapshot, whereas a false negative would be a correctness bug —
/// so only a script that assembles an entry-point name dynamically (e.g.
/// `globalThis["get_" + "component"]`) could evade it, which is pathological and
/// discounted.
pub(crate) fn source_reads_components(source: &str) -> bool {
    const READS: [&[u8]; 3] = [b"has_component", b"get_component", b"remove_component"];
    if is_wasm_source(source) {
        return decode_source(source)
            .is_some_and(|bytes| READS.iter().any(|read| contains(&bytes, read)));
    }
    READS.iter().any(|read| contains(source.as_bytes(), read))
}

/// Whether `haystack` contains the byte substring `needle`.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ScriptContext;
    use ecs::assets::AssetRegistry;
    use ecs::reflect::ComponentRegistry;
    use ecs::resources::InputState;
    use ecs::world::World;

    #[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize, ecs::Component)]
    struct Health {
        current: f32,
        max: f32,
    }

    #[test]
    fn a_script_that_names_no_read_does_not_need_components() {
        assert!(!source_reads_components(
            "function on_update(e, dt) { set_position(e, 1, 2, 3); }"
        ));
        // A write alone needs no snapshot.
        assert!(!source_reads_components("set_component(e, 'x', '{}');"));
    }

    #[test]
    fn any_reflection_read_needs_components() {
        assert!(source_reads_components("has_component(e, 'x');"));
        assert!(source_reads_components(
            "var h = get_component(e, 'Health');"
        ));
        assert!(source_reads_components("remove_component(e, 'x');"));
    }

    #[test]
    fn a_wasm_module_needs_components_only_when_it_imports_a_read() {
        let with_read = crate::wasm_host::encode(b"\0asm\x01\0\0\0get_component");
        let without_read = crate::wasm_host::encode(b"\0asm\x01\0\0\0set_component");
        assert!(source_reads_components(&with_read));
        assert!(!source_reads_components(&without_read));
    }

    #[test]
    fn begin_builds_component_json_only_when_reflecting() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(
            entity,
            Health {
                current: 10.0,
                max: 20.0,
            },
        );
        let id = entity_id(entity);

        let assets = AssetRegistry::new();
        let input = InputState::new();
        let mut registry = ComponentRegistry::new();
        registry.register::<Health>();
        let ctx = ScriptContext::new(&mut world, &assets, &registry, &input);

        let mut bridge = Bridge::default();
        bridge.begin(&ctx, false);
        assert!(
            bridge.components.is_empty(),
            "a non-reflecting call must build no component JSON"
        );

        bridge.begin(&ctx, true);
        assert!(
            bridge
                .components
                .get(&id)
                .is_some_and(|map| map.contains_key("Health")),
            "a reflecting call must snapshot the entity's components"
        );
    }
}
