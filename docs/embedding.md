# Noctis embedding and project Rust

How a Rust application **embeds** Noctis, and how a Noctis **project carries Rust**
that the tool compiles in — custom components and systems — alongside JavaScript.
This document is the contract for the `Plugin` seam, the component registry, and
the script reflection API; update it deliberately when the design changes.

Background: `noctis` is the multicall tool *and* the embedding facade
(`noctis::prelude`); a `Runtime` drives an `App` of staged systems
(`crates/ecs/src/app.rs`). See [`docs/project.md`](project.md) for what a project
is and [`docs/core-api.md`](core-api.md) for the frozen `math`/`ecs` contract.

## Decisions

1. **Custom components are scene-authorable.** A component defined in project Rust
   can appear in a `.scene.json`, be created and edited in the editor, and be
   carried by a prefab — exactly like a built-in component.
2. **Scripts read and write custom components.** JavaScript reaches any registered
   component by name, through one generic, data-driven API, not a per-type binding.
3. **Shape B: the tool owns `main`; the project supplies a plugin.** A project with
   Rust has a library crate that implements [`Plugin`](#the-plugin); the tool builds
   a per-project binary that links the plugin and registers it before loading the
   scene. Dynamic libraries (`cdylib`) are rejected: they need `unsafe` and a stable
   ABI, which the engine forbids.
4. **The component registry is a first-class engine object.** Component types are
   registered (name, default, serialize, deserialize, editor schema), and the scene
   loader/saver, the editor, the bundler, and the script bridge all consult it.

## The plugin

A project's Rust is a **library** crate (by convention `rust/`, override with
`[rust]` in the manifest) that depends on `noctis` and exports one value:

```rust
// rust/src/lib.rs
use noctis::prelude::*;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Health {
    pub current: f32,
    pub max: f32,
}

impl Component for Health {
    const NAME: &'static str = "Health";
    fn fields() -> Vec<Field> {
        vec![Field::number("current"), Field::number("max")]
    }
}

#[derive(Default)]
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

/// The tool calls this to obtain the project's plugin.
pub fn plugin() -> MyGame {
    MyGame
}
```

`noctis.toml` opts in:

```toml
[rust]
crate = "rust"            # a directory with a Cargo.toml (default: "rust")
plugin = "mygame::plugin" # the function returning the plugin (default: as above)
```

**How the tool runs it.** For any subcommand with `--project` whose manifest has
`[rust]`, the tool ensures a per-project binary is built — a small generated `main`
(under the project's gitignored `.noctis/`) that depends on `noctis` and the
plugin crate, calls `plugin()`, and dispatches the requested subcommand
(`run`/`editor`/`build`) through the same CLIs the stock tool uses. The binary is
cached and rebuilt only when the plugin crate or its dependencies change, the same
way `game_client` builds the client on demand today. Everything runs statically
linked: the editor, the runtime, and the exported game each include the plugin's
component types, so a scene that uses them loads everywhere.

A project with no `[rust]` is unchanged: the stock `noctis` binary handles it.

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
mirrored in [`docs/core-api.md`](core-api.md).

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

`project::component_references` (the bundler's reference scan,
`crates/project/src/resolve.rs`) matches the closed enum. Once the registry is
available at the bundler (phase 2, when the plugin is linked), a `Custom`
component's asset references (`FieldKind::Asset` fields, via `Component::references`)
are bundled like a mesh or clip — otherwise a shipped game would lose the files a
custom component points at. In phase 1 a `Custom` entry contributes no references
(the registry is not yet at the bundler). `noctis build` already produces a
per-project client (the embed pack); it now links the plugin too, so the exported
game has the component types.

## Scripts read and write custom components

The JavaScript boundary is string/number based (vector getters already return
`"[x,y,z]"` strings), so reflection is **JSON**, one generic pair, not a binding per
type:

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

These go through the same `ComponentRegistry`, so they work for **any** registered
component, built-in included (a uniform reflection path). On the engine side, reads
come from the existing snapshot/bridge (`crates/script/src/bridge.rs`) and writes
become a new `ScriptCommand::SetComponent { entity, name, json }` the runtime
applies after the callback, exactly like `translate`/`set_scale`.

Native WebAssembly scripts use a fixed import ABI (`wasm_host::Abi`). Generic
reflection there needs a JSON-over-linear-memory call; it is a follow-up, not part
of the first cut, and is called out in the ABI documentation when added.

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

1. **Component registry + `ComponentDesc::Custom`.** *(Shipped, except the bundler
   scan.)* The `Component` trait, the `ComponentRegistry`, `#[derive(Component)]`,
   scene round-trip via the `Scene::components` registry and the `_with` loaders, and
   `ComponentDesc::Custom` with an unknown-type load error. Tested with a
   host-registered component (a hand-written impl and a derived one); no project Rust
   yet. Still to do: the bundler scan of `Custom` references (needs the registry at
   the bundler, which arrives with the plugin in phase 2).
2. **The `Plugin` seam.** *(Seam shipped; `[rust]` build integration pending.)* The
   `Plugin` trait and `NullPlugin` (`ecs::app`); `runtime::cli::run_cli_with_plugin`
   and `build_runtime` register the plugin's components before the scene loads and
   schedule its systems after; the editor's `run_cli_with_plugin` gives `Editor` the
   registry so scenes that use custom components load, save, and instantiate from
   prefabs; the inspector draws the schema-driven form and lists custom names in the
   Add menu. Still to do: the `[rust]` manifest and the generated per-project binary
   the tool builds and runs for `run`/`editor`/`build`, and the bundler scan of
   `Custom` references (which needs the registry at the bundler).
3. **Script reflection.** `has/get/set/remove_component` plus
   `ScriptCommand::SetComponent`, for JavaScript.
4. **WebAssembly reflection** over the memory ABI.

## Open questions

- **Registry plumbing.** *Resolved for phase 1:* the registry lives on the scene
  (`Scene::components`) and the registry-aware loaders are `_with` variants; this
  keeps `to_desc`/`to_json` and every existing call site unchanged. Revisit only if
  the per-scene snapshot becomes a problem.
- **Derive vs. manual.** How much `#[derive(Component)]` should infer (field kinds
  from Rust types) before `#[field(as = "asset")]`-style attributes are needed.
- **Schema stability.** A registered component's `NAME` becomes part of the scene
  format; renaming it is a scene migration. A `[rust] version` and a documented
  policy are probably warranted.
- **Reflection cost.** `get_component` serializes on demand; whether the bridge
  caches JSON for the frame or serializes per call is an implementation detail to
  measure.
