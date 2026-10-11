---
name: noctis-authoring
description: Author games and scenes with the Noctis engine without manual editing — build a game like Tetris, spawn entities, attach components, write JavaScript behaviours, set up renderer/physics/audio, and export. Use when asked to create a scene, prototype a game, write engine scripts, or drive the editor programmatically.
---

# Authoring a game with Noctis

You build games by **composing scenes** and **attaching scripts**, using the
`editor` command API and the `ecs` scene model. Nothing requires a human to click
a button — this is the whole point.

Start from the worked reference: **`examples/showcase/`** is a complete project
(manifest, scenes, prefabs, scripts, generated assets) that exercises every
component and the whole scripting API. Copy its shape.

For project **Rust** — a `Cargo.toml` at the project root whose type is marked
`#[noctis::plugin]`, adding custom components and systems — the smaller worked
reference is **`examples/rustball/`** (see
[`.notes/embedding.md`](.notes/embedding.md)).

## Where things are

- Scene/component types: `crates/ecs/src/components.rs` and `crates/ecs/src/scene.rs`.
- Asset types (meshes, materials, textures, audio, scripts, fonts): `crates/ecs/src/assets.rs`.
- Procedural meshes and the reference scene: `crates/testscene`.
- Script engine API (what JS can call): `crates/script` (`ScriptApi`, `slag.rs`).
- Editor command API + CLI: `crates/editor` (the `noctis editor` command).
- Runtime/export: `crates/runtime` (the game loop and the stripped `noctis-game`
  binary); the `noctis` tool that drives it all is `crates/noctis`.
- Worked example project: `examples/showcase` (see its `README.md`); a Rust
  project is `examples/rustball`.

## Typical flow

1. Create or load a scene (`editor` library / `Scene::from_json`), or open a
   project with `--project <dir>` (a directory with a `noctis.toml`).
2. Spawn entities and set their `LocalTransform` (use `math::Transform` builders).
3. Attach components: `Name`, `Camera`, `Light`, `MeshRenderer`, `RigidBody`,
   `Collider`, `AudioSource`, `Script`, `Text`.
4. Parent entities with `ecs::scene::set_parent` to build hierarchies.
5. Write JavaScript behaviours (see *Scripts* below) and attach them by path
   (`Script::from_path("scripts/x.js")`) or inline (`Script::from_source`).
6. Save the scene as JSON (`Scene::to_json`) so it is data, not code.
7. Run it with `noctis run`; export with `noctis build`.

## Scenes (the data model)

- A scene is `SceneDesc { name, format_version, mode, entities }`. Each entity
  has `id`, optional `name`/`parent`, `active`, `visible`, a `transform`
  (`translation`, `rotation` = quaternion `[x,y,z,w]`, `scale`), and a list of
  typed components.
- `mode` is `"2d" | "3d"` — a hint, not a schema fork. In a 2D scene the editor
  adds an orthographic camera.
- `GlobalTransform` is **derived** (propagated from the hierarchy), so it never
  appears in a scene document.
- `Active`/`Visible` are only materialised when an entity is `active: false` /
  `visible: false`. A scene that must exercise those kinds needs such an entity.
- Reference assets by **project-relative path**: `assets/textures/wood.png`, or a
  glTF sub-asset as `assets/models/robot.glb#mesh/Torso` (the bare in-file name,
  e.g. `Torso` or `Robot_primitive0`, also resolves). Built-ins are `$default`,
  `$white`, `$normal`, and `primitives/cube|sphere|plane|cylinder|capsule|torus`.
- A reference to a missing asset is a **hard load error** naming the path — never
  a silent blank. A primitive mesh with no `material` falls back to `$default`.

## Scripts

Behaviours are **bare global functions** — there is **no `api` object and no
`export`**:

```js
function on_start(entity, dt)  {}   // once, when the script first runs
function on_update(entity, dt) {}   // every frame
function on_destroy(entity, dt) {}  // when the entity is removed
```

`entity` is a numeric handle; `dt` is the frame time in seconds. The engine wraps
each source in a module, so top-level state is per-script (shared by every entity
that runs it — key per-entity data by the `entity` handle).

The API (see `crates/script/src/slag.rs` for the exact surface):

| Call | Notes |
|---|---|
| `spawn(name) -> handle` | new entity with a `LocalTransform` |
| `destroy(handle)`, `destroy_by_name(name)` | |
| `find_entity(name) -> handle \| undefined` | |
| `get_position(h)`, `get_rotation_euler(h)`, `get_scale(h)` | **return `"[x,y,z]"` strings**, not arrays |
| `set_position(h,x,y,z)`, `translate(h,dx,dy,dz)` | |
| `set_rotation_euler(h,x,y,z)`, `set_scale(h,x,y,z)` | |
| `time_delta()`, `time_elapsed()` | seconds |
| `key_down("KeyW"\|"Space"\|"ArrowUp"\|"ShiftLeft"\|…)` | winit key names |
| `mouse_down("left"\|"right"\|"middle")` | |
| `add_script(handle, source)` | attaches inline source; starts next frame |
| `instantiate_prefab(json, parentHandle) -> handle` | parent is `undefined`/omitted for none; the prefab travels as **JSON text** |
| `has_component(h, name) -> bool` | a **registered** component (a plugin's custom one; see below) |
| `get_component(h, name) -> string` | the component as **JSON text**, or `""` when absent |
| `set_component(h, name, json)` | from JSON text; a type/field mismatch is a logged error, not a crash |
| `remove_component(h, name)` | |
| `log(message)` | |

The component reflection calls work for any component — a Rust project's registered
custom components **and** the built-ins that appear in a scene's
`components` list (`Camera`, `Light`, `MeshRenderer`, `RigidBody`, `Collider`,
`AudioSource`, `Script`, `Text`). A built-in's JSON is its scene-descriptor form
(`{"type": "rigid_body", ...}`, asset paths, not handles); the transform and other
entity-level fields keep their dedicated calls (`get_position`, …). Round-trip
through JSON:

```js
if (has_component(entity, "Health")) {
  const health = JSON.parse(get_component(entity, "Health")); // {"current":10,"max":20}
  health.current -= damage;
  set_component(entity, "Health", JSON.stringify(health));
}
```

Because the vector getters return strings, parse them:

```js
function vec3(raw) {
  return Array.isArray(raw) ? raw : String(raw).slice(1, -1).split(",").map(Number);
}
```

`key_down` is level-triggered (true every held frame); track the previous frame
to act on the edge. There is no per-entity or "pressed-this-frame" binding — use
the `entity` argument, not a `current_entity()` call.

Scripts run out of the box: the engine always builds the Slag JavaScript backend,
so `noctis run` and `noctis build` execute `.js` and `.wasm` scripts with no
feature to enable.

## Prefabs

`prefabs/<name>.prefab.json` hold a reusable subtree (`{ name, scene }`). The
editor loads the library so `instantiate_prefab` resolves a name; at runtime a
script instantiates a prefab from inline JSON (the verbatim document as a string),
because the script channel is text. `noctis build` bundles assets a *scene*
references, not prefabs reached only by script.

## Verify

After authoring:

- `cargo test --workspace` — the test scene and the `examples/showcase`
  integration test are the regression net; a scene that references a missing
  asset, or an asset that fails to import, fails here.
- For scripts, `cargo test -p script --test showcase_scripts` runs the example's
  JavaScript in the real engine.
- Load your scene through the runtime before claiming success:
  `cargo run -- run --headless --project <dir> --frames 120`.
