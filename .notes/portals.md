# Portals — carrying entities across a scene load

The showcase's **Trigger Pad** switches to the physics scene, but the physics
scene declares no Player, so there is nothing to control on the far side. The
intent was never "reload a scene"; it was **teleport the actual Player object**
into the target scene. That is a missing engine feature: today a `load_scene`
*replaces* the world and provides no way to move an entity across the load.

The physics scene already carries an inert **Sensor Zone** (id 12) — a sensor
with no script — which is plainly a *return* portal. So the feature should serve
a two-way transition.

This document is the contract for that feature. It extends
[`controls.md`](controls.md) (F1–F5 / S1–S5); the work here is **F6 / S6**.

## Goal

Let a `load_scene` **carry** a named entity (and its subtree) from the outgoing
scene into the incoming one, and place it at a declared **entry** in the target
scene, so a gameplay object — the Player — persists across a transition.

## Why it is missing today

`Runtime::apply_scene_request` (`crates/runtime/src/runtime.rs`) does, at the
frame boundary:

1. parse the target scene with `Scene::from_json_with`,
2. `self.app.context.scene = scene` (the whole world is replaced),
3. rebuild `PhysicsWorld` and re-`sync_from_world`,
4. `script.reset()` and re-register the new scene's scripts.

Nothing survives step 2. The script API cannot fix this on its own: `load_scene`
is queued and applied *after* the script that called it has finished, by which
point its world is gone. A script cannot re-spawn "the same object" either — it
would be a *copy*, and there is no object to copy from in the target scene.

## Design

Two concepts, both already expressible in scene data:

* **Carry** — an entity to move across the load, identified by name. Its whole
  subtree (every descendant via `Parent`) travels with it; its components are
  re-instantiated in the target scene. Scripts re-run `on_start` there (the
  script runtime resets anyway).
* **Entry** — where arrivals land in the target scene: an entity named
  `Entry`. The carried root adopts the entry's world transform; its descendants
  keep their local transforms. With no `Entry` in the target, the carried root
  keeps its pre-switch transform.

This reuses the scene serialization already in `crates/ecs/src/scene.rs`:
`Scene::to_desc` emits `EntityDesc`s with `parent` ids, and
`instantiate_into_with` remaps those ids onto fresh entities in any world. The
new engine piece is only a way to serialize **one subtree** from the *current*
scene and add it to the target before/after building it.

### Script API

A new verb (queued like `load_scene`, applied at the frame boundary):

```js
teleport_scene(path, carry)   // load `path`, carrying the entity named `carry`
```

Kept distinct from `load_scene` rather than overloading it by arity, so both are
trivially discoverable and no reliance is placed on multi-arity registration.

The showcase's pad becomes:

```js
if (armed && is_overlapping(entity, player)) {
  armed = false;
  teleport_scene(PHYSICS, "Player");
}
```

and the physics scene's **Sensor Zone** gets a matching script that carries the
Player back to the showcase.

## Implementation plan

### `crates/ecs/src/scene.rs`

1. Factor the per-entity serializer out of `Scene::to_desc` into a helper, then
   add `Scene::subtree_desc(&self, assets, root: Entity) -> Option<SceneDesc>`
   that serializes `root` and its descendants with stable local ids and the
   root treated as parentless.
2. Add `Scene::entity_named(&self, name: &str) -> Option<Entity>` (a name
   lookup) for the runtime to resolve the carry/entry by name.
3. Tests: a parent/child/grandchild subtree round-trips through
   `subtree_desc` → `instantiate_into_with`; a leaf root works; an unknown name
   yields `None`.

### `crates/runtime/src/runtime.rs`

4. Extend the queued request so it can name a carry target — either a second
   `Arc<Mutex<Option<String>>>` or a small `SceneRequest { path, carry }`.
5. In `apply_scene_request`: when a carry is named, resolve it in the *current*
   scene (`entity_named` + `subtree_desc`) **before** replacing the world; then
   build the target scene, `instantiate_into_with` the carried desc, and set the
   carried root's translation to the target's `Entry` (if present). Rebuild
   physics and reset scripting as today — the carried entity's own `Script`
   component lands in the new scene, so its `on_start` runs next frame.
6. Tests: `a_scene_switch_carries_the_player_to_the_entry` — a scene with a
   named Player and a script that calls the verb, a target scene with an
   `Entry`; assert a Player exists in the new scene **at the entry's position**
   and that its child came along. Also a test that no `Entry` keeps the source
   position, and that an unknown carry name loads the target with no carry
   (logged, not fatal).

### `crates/script/src/{api,bridge,slag,wasm_host}.rs`

7. New `ScriptCommand::TeleportScene { path, carry }` (api.rs), a `Bridge`
   helper (bridge.rs), a `teleport_scene`-of-arity-2 registration (slag.rs), and
   the `Abi::TeleportScene` variant with `for_name` / `func_type` / dispatch
   (wasm_host.rs) — the usual three registration points.

### Showcase (S6)

8. `showcase.scene.json`: the pad's `trigger.js` calls
   `teleport_scene(PHYSICS, "Player")`.
9. `physics.scene.json`: add an **`Entry`** marker (a transform-only entity on
   the ground, clear of the ramp) where the Player arrives; give the **Sensor
   Zone** a `return.js` that calls `teleport_scene(SHOWCASE, "Player")`; add an
   `Entry` to the showcase scene too (near the pad) for the trip back.
10. Docs: `examples/showcase/README.md`, `AGENTS.md` (scripting ABI + known
    limitations), and this file's status.

## Decisions

All four resolved as recommended:

1. **Selection** — carry by an entity named on the verb (`teleport_scene`).
2. **Verb shape** — a distinct `teleport_scene(path, carry)`; `load_scene`
   stays 1-arg.
3. **Entry** — a target entity named `Entry`.
4. **Showcase scope** — both directions (pad → physics, Sensor Zone → showcase).

### Loop guard (two portals facing each other)

A pad must never fire on the frame the Player *arrives* on it, or two pads would
bounce the Player back and forth forever. Two things prevent it:

* The portal scripts arm only after observing a *non-overlap* (`armed` starts
  `false`, set once the Player is off the pad — a genuine enter edge), so an
  arrival that already overlaps does not fire.
* The `Entry` markers are placed **clear of** the return sensor, so a normal
  arrival does not overlap a pad in the first place.
* The pads are **tall trigger volumes** (a `Collider` whose `offset` lifts a
  full-height shape over a thin visual mesh), not thin slabs: a thin slab only
  overlaps the capsule at ground level, so a small hop clears it and the pad
  never fires. A volume the height of the player is caught standing, walking or
  jumping.

The engine applies at most one switch per frame regardless (the request is taken
at the frame boundary), so a pathological script cannot stall the loop — it would
only flicker scenes.

## Status

**Shipped.** The ECS carries a subtree (`Scene::subtree_desc`, `Scene::entity_named`,
`instantiate_into_with`); the runtime lifts the named entity out of the outgoing
scene and drops it into the incoming one at the target's `Entry`
(`Runtime::apply_scene_request`); the verb is `teleport_scene(path, carry)` in
JavaScript and native WebAssembly. The showcase's Trigger Pad and the physics
Sensor Zone are wired both ways, and both tests drive them: a runtime unit test
(`a_scene_switch_carries_the_player_to_the_entry`) and an end-to-end one
(`the_trigger_pad_teleports_the_player_to_the_physics_scene`).

## Out of scope (tracked)

* Carrying more than one entity per transition.
* Carrying a camera when the target scene has none.
* Preserving transient runtime state (velocity, heading) across the load —
  `script.reset()` clears script state, so the Player arrives at rest, facing
  `yaw = 0`.
* Firing `on_destroy` for the carried entity's scripts on the outgoing side.
