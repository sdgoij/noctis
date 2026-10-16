# Noctis embedding and project Rust

How a Rust application **embeds** Noctis, and how a Noctis **project carries Rust**
that the tool compiles in — custom components and systems — alongside JavaScript.
This document is the contract for the `Plugin` seam, the component registry, and
the script reflection API; update it deliberately when the design changes.

Background: `noctis` is the multicall tool *and* the embedding facade
(`noctis::prelude`); a `Runtime` drives an `App` of staged systems
(`crates/ecs/src/app.rs`). See [`.notes/project.md`](project.md) for what a project
is and [`.notes/core-api.md`](core-api.md) for the frozen `math`/`ecs` contract.

## Decisions

1. **Custom components are scene-authorable.** A component defined in project Rust
   can appear in a `.scene.json`, be created and edited in the editor, and be
   carried by a prefab — exactly like a built-in component.
2. **Scripts read and write custom components.** JavaScript reaches any registered
   component by name, through one generic, data-driven API, not a per-type binding.
3. **Shape B: the tool owns `main`; the project supplies a plugin.** A project with
   Rust is a library crate reached from its root (a `Cargo.toml` there is the
   signal — the root crate itself, or a workspace member the root names as its
   first default member) whose type is marked `#[noctis::plugin]`; the tool builds
   a per-project binary that links the crate and registers the plugin before
   loading the scene. Dynamic libraries (`cdylib`) are rejected: they need
   `unsafe` and a stable ABI, which the engine forbids.
4. **The component registry is a first-class engine object.** Component types are
   registered (name, default, serialize, deserialize, editor schema), and the scene
   loader/saver, the editor, the bundler, and the script bridge all consult it.

## The plugin

A project's Rust is a **library crate reached from the project root**: a
`Cargo.toml` there is what makes `noctis` treat the project as carrying Rust — no
manifest table is needed. The root manifest is either the plugin crate itself, or
a **virtual workspace** whose first `default-members` (or, failing that, first
`members`) entry is the plugin crate — so a project can keep its simulation in
sibling crates and let one member be the plugin (as `examples/goats` does). The
crate is a **library** that depends on `noctis` **without the `editor` feature**
and exports one value. The feature matters: the exported game links this crate, so
`default-features = false` keeps the editor and its authoring UI out of a shipped
game (the tool that links the same crate enables the feature itself).

```toml
# Cargo.toml (the project root)
[package]
name = "mygame"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[dependencies]
noctis = { path = "../../crates/noctis", default-features = false }
serde = { version = "1", features = ["derive"] }

# A project nested in another cargo workspace detaches with an empty table.
[workspace]
```

```rust
// src/lib.rs
use noctis::prelude::*;

#[derive(Debug, Default, Serialize, Deserialize, Component)]
pub struct Health {
    pub current: f32,
    pub max: f32,
}

/// `#[noctis::plugin]` exports the `noctis_plugin` symbol the tool links; the
/// behaviour is written by hand in the `impl`.
#[noctis::plugin]
pub struct MyGame;

impl Plugin for MyGame {
    // Components first: a scene that uses them must resolve when it loads.
    fn register(&self, components: &mut ComponentRegistry) {
        components.register::<Health>();
    }
    // Systems after: they run after the engine's own within each stage.
    fn build(&self, app: &mut App) {
        app.on_update("regen", |ctx| {
            for (entity, health) in ctx.scene.world.query_mut::<Health>() {
                health.current = (health.current + 1.0).min(health.max);
            }
        });
    }
}
```

`#[noctis::plugin]` must be applied to the plugin type at the crate root; it keeps
the type and appends

```rust,ignore
#[doc(hidden)]
pub fn noctis_plugin() -> impl ::noctis::ecs::app::Plugin { MyGame }
```

The type must be a unit struct (used as its own value) or implement `Default`.

**How the tool runs it.** For `run`/`editor`/`build` with `--project` whose root
holds a `Cargo.toml`, the stock `noctis` builds a per-project **tool** — a small
generated `main` (under the project's gitignored `.noctis/tool/`) that depends on
`noctis` and the plugin crate and calls
`noctis::dispatch_with_plugin(<crate>::noctis_plugin())` — then runs it as a
subprocess with the same arguments, so the subcommand behaves exactly as the stock
tool but with the plugin registered. The plugin's crate name is the plugin crate's
`[package].name` (the root crate, or the workspace default member), with `-`
normalized to `_`. The binary is cached and
rebuilt only when the plugin crate or the engine changes, the same way `game_client`
builds the client on demand today. The generated crate carries an empty `[workspace]`
table and path dependencies on the plugin (the plugin crate's directory) and on
`crates/noctis`,
so it compiles the plugin's component types in.

A `run` wants a playable frame rate, so its tool is built **optimized** whenever
`noctis` itself is a release build (`cargo run -r -- run …`, an installed release
binary) or `--release` is passed; the plugin is a separate cargo workspace, so
without this its debug build would run the game at ~25 fps. An `editor` session
follows the same rule: its validation runs on the UI thread, and a debug tool is
slow enough to stall a frame (and keeps the debug-only panic watchdogs — epaint's
lock watchdog, Slag's verifier — armed). `--debug` opts either back into the
fast-to-build debug tool, and `noctis build` follows its `[build] profile` as
before.

**How a plugin project builds.** `noctis build --project <game>` (dispatched to the
plugin-aware tool) resolves the scene, bundles the referenced files — including a
registered custom component's asset references, from the plugin's registry — and
writes a per-project **client** instead of the stock `noctis-game`: a generated
crate under `.noctis/client/` that depends on the `runtime` and the plugin (never
on `noctis` directly, and the plugin's `noctis` has no editor) and calls
`runtime::cli::run_cli_with_plugin`. A release client bakes the scene's scripts into
`embedded.rs` by absolute path (`include_bytes!`); a development client leaves it
empty and loads scripts from disk. The exported game therefore links the plugin's
component types and nothing from the authoring tool chain.

A project with no root `Cargo.toml` is unchanged: the stock `noctis` binary handles
it, and `noctis build` ships the stock `noctis-game` client.

A complete worked example is `examples/rustball`: a `Spin` custom component turned
by a system, a system that flies the active camera from input, and a simulated
ball that bounces and respawns.

## Custom components

Storage is already open — `World` is generic over any `C: Send + Sync + 'static`
and keys columns by `TypeId` (`crates/ecs/src/world.rs`). What is missing is
*registration and reflection*, not storage.

### The `Component` trait

```rust
pub trait Component: Send + Sync + 'static + Serialize + DeserializeOwned + Default {
    /// The stable name used in scene files, the editor, and scripts.
    const NAME: &'static str;
    /// The editor/reflection schema (see `Field`).
    fn fields() -> Vec<Field> {
        Vec::new()
    }
    /// Asset references this component contributes to a build (usually derived
    /// from `fields`, where asset fields are declared).
    fn references(&self) -> Vec<String> {
        Vec::new()
    }
}
```

Built-in components already serialize through the closed `ComponentDesc` enum and
stay that way; `Component` is the *extension* path. The `ecs::reflect` types
(`Component`, `ComponentRegistry`, `Field`, `FieldKind`) are re-exported from
`ecs::prelude`, and `#[derive(Component)]` (from `ecs-macros`) implements the trait:
`NAME` is the struct name or `#[component(name = "...")]`; each named field becomes
a `Field` whose kind is inferred from the Rust type (`f32`/`f64` → number, the
integer types → integer, `bool` → boolean, `String` → text, `[f32; 3]` → vector,
`[f32; 4]` → colour, anything else → text), overridable with
`#[field(number|integer|boolean|text|vec3|color|asset|options = "a, b, c")]`;
`#[field(skip)]` omits a field; and a `#[field(asset)]` field (a `String` or
`Option<String>`) is returned from `references`, so the bundler ships it.

### The registry

```rust
pub struct ComponentRegistry { /* name -> registration vtable */ }

impl ComponentRegistry {
    pub fn register<C: Component>(&mut self);
    pub fn names(&self) -> Vec<&'static str>;
    pub fn has(&self, name: &str) -> bool;
    pub fn fields(&self, name: &str) -> Option<Vec<Field>>;
    // Used by the scene loader/saver, the editor, and the script bridge:
    pub fn insert_default(&self, world: &mut World, entity: Entity, name: &str) -> Result<(), String>;
    pub fn serialize(&self, world: &World, entity: Entity, name: &str) -> Option<serde_json::Value>;
    pub fn deserialize(&self, world: &mut World, entity: Entity, name: &str, data: &serde_json::Value) -> Result<(), String>;
    pub fn remove(&self, world: &mut World, entity: Entity, name: &str) -> bool;
}
```

The registry is app-scoped: construct it, register the plugin's components, then
load scenes. The editor and the runtime hold the same registry, which is why the
plugin is linked into each.

### Scene serialization

`ComponentDesc` gains one variant:

```rust
ComponentDesc::Custom { name: String, data: serde_json::Value }
// serialized (internally tagged, like every variant) as:
// { "type": "custom", "name": "Health", "data": { "current": 10, "max": 20 } }
```

(The sketch earlier showed `type_name` spread with the data; serde's internal
`type` tag makes an explicit `name`/`data` pair the clean form, which keeps every
component in one `components` list.)

- **Save** (`Scene::to_desc`/`to_json`): after the built-in components,
  `component_descs` appends a `Custom` entry for every registered type the entity
  has, via `registry.serialize_entity` (sorted by name, so the output stays
  deterministic). Built-ins keep their exact current JSON, so existing files do not
  change.
- **Load** (`Scene::from_json_with`/`try_from_desc_with`): a `Custom` entry is
  applied with `registry.deserialize`. An entry naming a type the registry does not
  have is a **load error** (`SceneError::UnknownComponent`) that names the type —
  so a scene that needs the plugin fails clearly when the plugin is absent.
- **Prefabs** carry components the same way, so custom components work in prefabs
  unchanged.

The registry travels **on the scene** (`Scene::components`), which keeps
`to_desc`/`to_json` signatures unchanged. The registry-aware load path is the
`_with` variants — `Scene::from_json_with`, `from_desc_with`, `try_from_desc_with`,
`instantiate_into_with`, `Prefab::instantiate_with` — while the plain ones use an
empty registry (built-ins only). The chosen shape is simpler than threading the
registry through every call site; the trade is that a scene holds a snapshot of the
registry it was built with. This is a deliberate, additive `ecs` contract change,
mirrored in [`.notes/core-api.md`](core-api.md).

### Editor

The plugin binary is the editor, so its registry is populated. The inspector
renders a **schema-driven form** from `Field`s for any custom component: a name and
a remove button like a built-in, with widgets chosen from `FieldKind`. Editing goes
through the existing `Editor::edit_component`/`insert_component` so it folds into
one undo step. The Add-component menu lists registered custom names beside the
built-ins.

```rust
pub struct Field {
    pub name: &'static str,
    pub kind: FieldKind,
}
pub enum FieldKind {
    Number,
    Integer,
    Boolean,
    Text,
    Vec3,
    Color,
    Asset,       // an asset path: shown as a picker, counted by the bundler
    Enum(&'static [&'static str]),
}
```

### Bundling and export

`project::referenced_files_with` (the bundler's reference scan,
`crates/project/src/resolve.rs`) takes the component registry, so a `Custom`
component's asset references (`FieldKind::Asset` fields, via `Component::references`)
are bundled like a mesh or clip — otherwise a shipped game would lose the files a
custom component points at. `noctis build` for a Rust project builds a
plugin-linked client (`.noctis/client/`) instead of the stock `noctis-game`, so the
exported game has the component types.

## Scripts read and write components

The JavaScript boundary is value-based: a fixed-shape tuple (a vector, a pointer
delta, the viewport size) comes back as a real array, and a component's
open-ended structure travels as **JSON** — one generic pair, not a binding per
type. It works for a registered custom component and a built-in alike:

```js
if (has_component(entity, "Health")) {
  const health = JSON.parse(get_component(entity, "Health")); // {"current":10,"max":20}
  health.current -= damage;
  set_component(entity, "Health", JSON.stringify(health));
}
```

- `has_component(entity, name) -> bool`
- `get_component(entity, name) -> string` (JSON) or `""` when absent
- `set_component(entity, name, json)` — parsed with the registry; a type/field
  mismatch is a logged script error, not a panic
- `remove_component(entity, name)`

These are uniform across **registered custom components and built-in components**:
a registered custom uses its own JSON (via `ComponentRegistry`); a built-in uses
its scene descriptor — the canonical, asset-path-resolving `ComponentDesc` JSON
(`{ "type": "rigid_body", ... }`) — since built-ins have no serde impls of their own
(`ecs::scene::{reflect_components, has_reflected_component, get_reflected_component,
set_reflected_component, remove_reflected_component}`). The entity-level
`LocalTransform`/`Name`/`Active`/`Visible` are not part of that list and keep their
dedicated bindings (`get_position`, `find_entity`, …). On the engine side, reads come
from the snapshot/bridge (`crates/script/src/bridge.rs`, which snapshots each
reflected component's JSON per entity at the start of a call) and writes become
`ScriptCommand::SetComponent`/`RemoveComponent` the runtime applies after the
callback, exactly like `translate`/`set_scale`. A bad JSON body or type mismatch is
a logged script error, never a panic.

Native WebAssembly scripts reach the same registry through the import ABI
(`wasm_host::Abi`), with **JSON over linear memory**:

- `has_component(entity, name_ptr, name_len) -> i32`
- `get_component(entity, name_ptr, name_len, out_ptr, out_cap) -> i32` — writes the
  JSON into the module's exported `memory` and returns its byte length, or `-1`
  (absent) / `-2` (buffer too small)
- `set_component(entity, name_ptr, name_len, json_ptr, json_len)`
- `remove_component(entity, name_ptr, name_len) -> i32`

Both backends share one path on the engine side, so a custom component's JSON is
identical whichever language reads it.

## Rejected alternatives

- **Dynamic libraries (`cdylib`) as plugins.** Require `unsafe` and a stable ABI,
  against `#![forbid(unsafe_code)]` and the "no hacks" stance; they also make ABI
  drift a runtime crash instead of a compile error.
- **Custom components outside `ecs`.** They must be scene-authorable, which means
  the loader/saver and the bundler must know them, which means they belong in the
  core with a registry — not in a side crate.
- **A per-type script binding.** A generic JSON reflection keeps the script API
  small and works for components the engine has never seen.

## What already exists

- `noctis::prelude` embeds the engine; `Runtime::app_mut` registers systems;
  `World` already stores arbitrary `Send + Sync + 'static` component types.
- `Scene::to_desc`/`from_desc`/`to_json`/`from_json`, `component_descs`,
  `apply_component`, and `ComponentDesc` are the serialization seams to extend.
- `project::component_references` is the bundler's reference scan.
- The script bridge and `ScriptCommand` are the read/write seams for scripting.
- `game_client()` already builds a binary on demand — the pattern the per-project
  plugin binary follows.

## Phasing

1. **Component registry + `ComponentDesc::Custom`.** *(Shipped.)* The `Component`
   trait, the `ComponentRegistry`, `#[derive(Component)]`, scene round-trip via the
   `Scene::components` registry and the `_with` loaders, and `ComponentDesc::Custom`
   with an unknown-type load error. The `Component` derive resolves its crate path for
   both `ecs` consumers and `noctis`-only plugin crates.
2. **The `Plugin` seam.** *(Shipped.)* The `Plugin` trait and `NullPlugin`
   (`ecs::app`); `runtime::cli::run_cli_with_plugin` and `build_runtime` register the
   plugin's components before the scene loads and schedule its systems after; the
   editor's `run_cli_with_plugin` gives `Editor` the registry so scenes that use
   custom components load, save, and instantiate from prefabs; the inspector draws
   the schema-driven form and lists custom names in the Add menu. A `Cargo.toml` at
   the project root (whose `#[noctis::plugin]` type exports the `noctis_plugin`
   symbol) drives `runtime::plugin`, which generates and builds the per-project
   **tool** under `.noctis/tool/` for `noctis run`/`editor` and the per-project
   **client** under `.noctis/client/` for `noctis build` (a plugin-linked game with no
   editor; scripts baked in for a release build).
   `project::referenced_files_with` bundles a custom component's asset references.
3. **Script reflection.** *(Shipped for JavaScript and WebAssembly, custom and
   built-in.)* `has/get/set/remove_component` plus
   `ScriptCommand::SetComponent`/`RemoveComponent`, over the scene's reflection
   layer (`ecs::scene`): a registered custom uses its own JSON, a built-in its
   `ComponentDesc` JSON. JavaScript uses globals with JSON strings; WebAssembly uses
   the import ABI with JSON over linear memory.

## Open questions

- **Registry plumbing.** *Resolved for phase 1:* the registry lives on the scene
  (`Scene::components`) and the registry-aware loaders are `_with` variants; this
  keeps `to_desc`/`to_json` and every existing call site unchanged. Revisit only if
  the per-scene snapshot becomes a problem.
- **Derive vs. manual.** How much `#[derive(Component)]` should infer (field kinds
  from Rust types) before `#[field(as = "asset")]`-style attributes are needed.
- **Schema stability.** A registered component's `NAME` becomes part of the scene
  format; renaming it is a scene migration. A plugin-version policy is probably
  warranted.
- **Reflection cost.** *Resolved:* the bridge snapshots every reflected component's
  JSON (built-in descriptors and registered customs) per entity once per call
  (`Bridge::begin`). Simple and correct; a lazily-populated cache (only the
  components a script reads) is a later optimization to measure.
