# Gameplay controls, scene flow, and the scripting gaps they need

This note is the design contract for making the `examples/showcase` play like a
third-person game, and for the engine features that need to exist first. It was
written from a review of the showcase and the engine, and it records both the
*decisions* and the *implementation plan*. Update it deliberately when the design
changes, exactly like the other `.notes/` contracts.

## Goal

Turn the showcase into a playable third-person demo:

- the game **starts on the menu scene**, and **Play** hands off to the showcase;
- the **camera follows the player**; mouse **X** turns the player, mouse **Y** pans
  the camera;
- **WASD** move the player, **`[Space]`** jumps;
- the **trigger pad** teleports (scene-switches) to the physics playground;
- **newly spawned crates are physical**: the player pushes them, they fall off the
  floor, and they despawn;
- **`[Esc]` pauses**: it releases the cursor and returns to the menu; `[Esc]` again
  resumes.

Most of that is gameplay (scripts + scenes). A few pieces are **missing engine
features**, which is the real point of this note: building the demo is how we find
and fill them.

## The gap

The script API is a *scene-graph* API: it reads and writes transforms, spawns and
destroys, reflects components, and reads key/button state. It cannot:

1. read **continuous pointer input** (mouse motion, scroll);
2. drive a **physical** body (velocity / impulse) — so a script cannot move or jump
   a dynamic body;
3. **query physics** (is the player grounded?);
4. **load another scene** at runtime;
5. receive **sensor / trigger** events.

## The design of a script verb

Every verb lives in **three** places, and all three must be updated together:

1. `crates/script/src/slag.rs` — `self.register("name", arity, …)` (JavaScript).
2. `crates/script/src/wasm_host.rs` — an `Abi` variant, its `for_name` arm, **and**
   its dispatch (native WebAssembly).
3. `crates/script/src/bridge.rs` — the snapshot accessor and/or the queued
   `ScriptCommand`.

The bridge is a two-phase protocol: the host **snapshots** the parts of the engine
a script may read (transforms, names, input, timing) into a `Bridge`, runs the
script, and **queues** `ScriptCommand`s; `replay` then applies them through
`ScriptApi`. Writes go through the command log; reads come from the snapshot.

## Decisions

These were agreed before implementation:

1. **Physics writes are queued commands**, applied by the runtime — *not* a live
   `&mut PhysicsWorld` in the script callback.
2. **Physics reads are a per-frame snapshot** (`is_grounded`), not a general
   `raycast` verb (deferred until a use appears).
3. **`Esc` pauses** the game and returns to the menu; `Esc` again resumes. (The
   cursor is captured while playing and released while paused.)
4. Provide **both** `set_velocity` (absolute) **and** `add_velocity`/`apply_impulse`
   (one-shot) — different call sites want different semantics.
5. An exported build bundles **every** `[[scene]]` in the manifest, so `load_scene`
   can reach any of them. (Revisit if packs get large.)
6. The third-person controller is a **JavaScript** showcase script, not a built-in
   Rust component — it keeps the showcase a scripting demo.

## Features

### F1 — Pointer input + cursor capture  *(milestone: scripts)*

**Goal.** Scripts read per-frame mouse motion and scroll; the windowed game
captures the cursor so look-controls do not stall at the window edge.

**Surface.** `mouse_move() -> "[dx, dy]"`, `mouse_position() -> "[x, y]"`,
`mouse_scroll() -> number`.

**State.** `InputState.mouse` already carries `position`/`delta`/`scroll`
(`crates/ecs/src/resources.rs`), fed from `CursorMoved`/`MouseWheel`
(`crates/platform/src/window.rs`); the `Bridge` already snapshots the whole
`InputState`, so the verbs are pure additions in `slag.rs` + `wasm_host.rs`. There
is **no** cursor grab today, and **no** `Esc` handling.

**Plan.**
- Register the three verbs in both backends.
- `platform`: add `AppDriver::cursor_captured(&self) -> bool` (default `false`) and
  `AppDriver::on_escape(&mut self)` (default no-op); the window host applies
  `set_cursor_grab(Locked)` + `set_cursor_visible(false)` to match
  `cursor_captured()`, calls `on_escape()` on the `Esc` key, and follows raw
  `DeviceEvent::MouseMotion` while captured.
- The runtime driver captures while playing and releases while paused; `Esc`
  toggles its paused flag (see F4 for the menu load).
- Web: the browser demo needs the Pointer Lock API — a smaller follow-up.

### F2 — Drive a physical body  *(milestone: scripts)*

**Goal.** A script sets a dynamic body's velocity (WASD) and applies an impulse
(jump).

**State.** `PhysicsWorld::set_linear_velocity` / `apply_impulse` /
`linear_velocity_of` exist (`crates/physics/src/world.rs`); the script API has no
physics handle, and `sync_bodies` applies `RigidBody` fields only at body creation
— so writing `RigidBody.linear_velocity` post-hoc does nothing.

**Surface.** `set_velocity(entity, x, y, z)` (absolute), `add_velocity(entity, x, y, z)`
(additive), `apply_impulse(entity, x, y, z)`, `get_velocity(entity)` (read).

**Plan.**
- `ScriptCommand::SetVelocity` / `AddVelocity` / `ApplyImpulse` (`crates/script/src/api.rs`).
- `ScriptApi` queues them (no world mutation; documented as runtime-applied).
- `ScriptRuntime` exposes the drained commands (`take_commands()`), and the runtime
  `update` system applies them via `physics.set_linear_velocity` / `apply_impulse`
  after `run_update`.

### F3 — Read physics (grounding)  *(milestone: scripts)*

**Goal.** A script knows whether the player is grounded, so `[Space]` only jumps
from the floor; and reads its velocity for `get_velocity`.

**State.** `PhysicsWorld::raycast` exists; nothing reaches scripts.

**Plan.** Build a small read-only `PhysicsSnapshot` per frame in the runtime — for
each dynamic body, `grounded` (one short downward ray, floor normal up) and its
`linear_velocity` — thread it into `ScriptRuntime::run_start/run_update`, and copy
it into the `Bridge` like `input`. Register `is_grounded(entity)` and
`get_velocity(entity)`.

### F4 — Runtime scene switching

**Goal.** A running game loads another scene (menu → showcase; trigger → physics).

**State.** `Runtime` owns exactly one `Scene`; `runtime::cli::load_scene` resolves
the scene *before* the runtime exists; builds bundle **one** scene.

**Surface.** `load_scene(path)` → `ScriptCommand::LoadScene { path }`.

**Plan.**
- `Runtime` gains a `SceneSource` (path → `(Scene, AssetRegistry)`) that works from
  a project dir *and* from the bundled pack; it holds the loader + registry.
- **Swap algorithm**, run at the end of the frame (after `late`): run `on_destroy`
  for outgoing scripts → rebuild `PhysicsWorld` (and audio refs) from the new scene
  → load it into `app.context.scene` → `propagate_transforms` →
  `PhysicsWorld::sync_from_world` → reset `ScriptRuntime` instances → `on_start`.
  A request **replaces** the current scene rather than stacking.
- `noctis build` bundles every `[[scene]]` (+ referenced assets) and writes a scene
  table into the generated `noctis.toml`.
- Flip `[project.settings] default_scene` to the menu once hand-off works.

### F5 — Sensor / trigger events

**Goal.** A sensor volume reports enter/leave, so a pad can act.

**State.** `PhysicsWorld` has `narrow_phase`; sensors emit nothing.

**Plan.** Extend the per-frame `PhysicsSnapshot` with trigger data — read rapier's
`narrow_phase` intersection pairs, map colliders → entities, diff against last
frame for enter/leave. Expose `on_trigger(entity, other)` on enter (fits the
`on_start`/`on_update`/`on_destroy` model), plus `is_overlapping(a, b)` if the
query form is wanted.

## Showcase work

- **S1 — `third_person.js`.** Player becomes a **dynamic capsule with locked
  rotation** (physics rewrites dynamic poses, so the script cannot rotate the body
  directly) plus a **child mesh** the script yaws; WASD → horizontal velocity
  relative to yaw; `[Space]` → jump when `is_grounded`; mouse X → player yaw, mouse
  Y → camera pitch; the camera follows at a fixed offset. *(F1, F2, F3)*
- **S2 — menu default + Play.** Default scene → the menu; the Play button calls
  `load_scene("scenes/showcase.scene.json")`. `Esc` returns to the menu. *(F4)*
- **S3 — spawner rework.** Spawn crates with `mesh_renderer` + dynamic `rigid_body`
  + `collider` via `set_component`, a random `angular_velocity` to tumble, and a
  kill-plane despawn. Fixes "the boxes don't spin". *(F2)*
- **S4 — trigger pad.** The sensor's script calls
  `load_scene("scenes/physics.scene.json")` when the player enters. *(F4, F5)*
- **S5 —** docs (`AGENTS.md`, showcase `README.md`, `.notes/scripts.md`) and the
  integration tests (`crates/runtime/tests/showcase_scenes.rs`,
  `crates/script/tests/showcase_scripts.rs`).

## Sequencing

```
F1 (pointer) ─┐
F2 (velocity) ┼─► S1 (third person) ─┐
F3 (grounding)┘                      ├─► S5
F2 ─────────────► S3 (spawner) ──────┘
F4 (scenes) ─┬─► S2 (menu) ──────────┘
F5 (triggers)┴─► S4 (trigger pad) ───┘
```

Start with the **F1–F3 milestone** (input & physics for scripts — all three touch
the same three registration files), then **S1 + S3**, then **F4 → S2**, then
**F5 → S4**.

## Status

- **F1, F2, F3 shipped:** pointer verbs (`mouse_move`/`mouse_position`/
  `mouse_scroll`), the physics writes (`set_velocity`/`add_velocity`/
  `apply_impulse`, queued and applied by the runtime), the reads (`get_velocity`/
  `is_grounded`, from a per-frame `PhysicsView` snapshot), and cursor capture with
  `Esc` toggling it. Both the JavaScript and the native-WebAssembly backends
  expose the verbs.
- **S1, S3 shipped:** the showcase Player is a kinematic capsule driven by
  `third_person.js` (WASD in the yaw frame, `Space` jump when grounded, mouse X
  yaw / Y camera pitch, a follow camera, and a "Player Facing" child that shows
  the heading), and `spawner.js` now spawns physical, tumbling crates (mesh +
  dynamic body + collider through `set_component`, a lifetime via `add_script`,
  and a kill-plane despawn) the player can push.
- **F4 shipped (dev + engine):** `load_scene(path)` queues a `ScriptCommand::LoadScene`; the runtime swaps the scene at the frame boundary (re-parsing through its own asset registry, rebuilding physics, restarting scripting) via a `set_scene_loader` closure wired by `noctis run` from the open project.
- **S2 shipped (core):** the showcase's `default_scene` is the menu; `menu.js` releases the cursor (`set_mouse_captured(false)`), drives a custom `Cursor` entity to the pointer using `viewport_size`, and calls `load_scene` to start the showcase, while `third_person.js` captures the cursor on start. `Esc` pauses to the home (default) scene and `Esc` again resumes (`Runtime::toggle_pause`; `noctis run` sets the home scene from the project's default).
- **S2 (export) shipped:** `noctis build` bundles **every** scene the project ships (each scene document and the files it references), and the bundled `noctis.toml` starts on the project's default scene, so an exported game can `load_scene` the rest — not just the built scene.
- **F5 + S4 shipped:** `PhysicsWorld::overlaps` reports the sensor overlap pairs each frame; a script reads them with `is_overlapping(a, b)`, and the showcase's **Trigger Pad** (`trigger.js`) teleports to the physics scene when the Player stands on it (the script latches on entry).
- **F6 + S6 shipped:** `teleport_scene(path, carry)` carries a named entity and its subtree across a scene load, landing at the target scene's `Entry`; the showcase's Trigger Pad moves the Player into the physics playground and a Sensor Zone moves it back. See [`.notes/portals.md`](portals.md).
- **Open:** the whole plan is delivered. `on_trigger(entity, other)` enter-events remain a possible richer API (scripts latch on entry themselves today).
