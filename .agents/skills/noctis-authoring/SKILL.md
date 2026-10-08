---
name: noctis-authoring
description: Author games and scenes with the Noctis engine without manual editing — build a game like Tetris, spawn entities, attach components, write JavaScript behaviours, set up renderer/physics/audio, and export. Use when asked to create a scene, prototype a game, write engine scripts, or drive the editor programmatically.
---

# Authoring a game with Noctis

You build games by **composing scenes** and **attaching scripts**, using the
`editor` command API and the `ecs` scene model. Nothing requires a human to click
a button — this is the whole point.

## Where things are

- Scene/component types: `crates/ecs/src/components.rs` and `crates/ecs/src/scene.rs`.
- Asset types (meshes, materials, textures, audio, scripts): `crates/ecs/src/assets.rs`.
- Procedural meshes and the reference scene: `crates/testscene`.
- Editor command API + CLI: `crates/editor` (`noctis-editor`).
- Runtime/export: `crates/runtime` (`noctis-runtime`).
- Script engine API (what JS can call): `crates/script` (`ScriptApi`).

## Typical flow

1. Create or load a scene (`editor` library / `Scene::from_json`).
2. Spawn entities and set their `LocalTransform` (use `math::Transform` builders).
3. Attach components: `Name`, `Camera`, `Light`, `MeshRenderer`, `RigidBody`,
   `Collider`, `AudioSource`, `Script`.
4. Parent entities with `ecs::scene::set_parent` to build hierarchies.
5. Write JavaScript behaviours in `Script` components: implement `on_start`,
   `on_update(entity, dt)`, `on_destroy`. Use the `ScriptApi`
   (`spawn`, `destroy`, `find_entity`, `get_position`/`set_position`,
   `translate`, `get_rotation_euler`/`set_rotation_euler`, `get_scale`/`set_scale`,
   `time_delta`, `key_down`, `add_script`, `instantiate_prefab`).
6. Save the scene as JSON (`Scene::to_json`) so it is data, not code.
7. Run it with `noctis-runtime`; export by shipping the runtime with the scene.

## Conventions

- Prefer data (scene JSON + scripts) over bespoke Rust for gameplay.
- Keep scripts small and event-driven; do the heavy work in engine systems.
- Every gameplay feature you add should be representable in the `testscene`
  crate so it is covered by `cargo test --workspace`.

## Verify

After authoring, run `cargo test --workspace` (the test scene is the regression
net) and, for scripts, `cargo test -p script --features slag` to execute them
with the real JavaScript engine.
