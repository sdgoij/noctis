# Script entity handles: stable across frames

**Status: shipped.** A handle a script keeps in module state now names the same
entity on later frames.

## The problem

A script that spawned (or found) an entity and kept its handle could not use it on
a later frame. Handles were **per-call** indices into a table the host cleared at
the start of every call, so `spawned[i]` on frame N+1 named nothing — or, worse, a
different entity. `examples/showcase/scripts/spawner.js` stored crate handles
exactly this way, so its kill plane silently never fired (the old string-parsing
`vec3(undefined)` masked the failure as `NaN`).

## The design

The handle table is now **per instance** and persists for that instance's life,
keyed by the instance's own entity. Index 0 is always the instance's own entity;
every other index is allocated the first time the script addresses an entity and
then stays fixed. So a handle stored on one frame names the same entity on the
next. Handles are still small integers (a raw `EntityId` cannot round-trip a
`f64`), and they are never shared between instances — a script has no way to pass
one to another script.

Two details make it correct:

- **Placeholders.** `spawn`/`instantiate_prefab` hand back a *placeholder* id
  during the call (the real entity exists only after replay). `replay` now returns
  the placeholder → real map, and the host re-points its table through it, so a
  handle stored for a `spawn()` names the real entity next frame.
- **Cleanup.** The runtime calls `ScriptHost::forget_instance(owner)` when an
  instance ends (destroyed, stale, or re-subscribed), bounding the table. The key
  is the owning `EntityId` (index *and* generation), so a reused slot never
  collides even before cleanup.

Both backends share one table type (`bridge::HandleTables`), so JavaScript and
WebAssembly behave identically.

## Aliveness on reads

`get_position` (and the other vector getters) return `undefined` for a handle
whose entity has since been destroyed, so a script can tell "destroyed" from "at
the origin". The per-call snapshot carries an alive set (`bridge::Bridge::alive`:
every world entity, plus any placeholder spawned during the call) and the
JavaScript getters check it. `spawner.js` uses this to drop crates whose
lifetime already ended.

The WebAssembly ABI has no `undefined` (its vectors are multi-value floats), so a
destroyed entity reads as the zero vector there — a float-ABI limitation, not a
safety gap.

## Out of scope (deferred)

- **Table growth.** A table grows with the number of distinct entities a script
  addresses over its life (bounded in the showcase by the crate lifetime); a dead
  entity's entry is not reclaimed until the instance ends.

## Tests

`a_handle_stored_on_one_frame_names_the_same_entity_the_next`
(`crates/script/src/slag.rs`): a script keeps a `spawn()` handle in a module
global, reads and moves it over the next two frames, and the entity lands exactly
where a stable handle says it should.
