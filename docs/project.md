# Noctis projects

**Status: partly implemented.** This document defines what a Noctis *project* is and how the engine resolves it. The manifest, canonical asset paths, and the asset importer are implemented in the `project` crate and wired into both CLIs; the rest (rename-rewrite, drag & drop, the `.noctis/` cache, build targets, per-scene `mode`, the `fonts` store, the UI picker) is design only. Treat this as the contract to implement against and update deliberately.

A project is a **directory with a `noctis.toml` manifest**. It owns one shared
asset registry, zero or more scenes, prefabs, and scripts, and it defines how the
game is built and exported.

## Decisions

These were chosen deliberately; changing them is a design change, not a tweak.

1. **References are project-relative paths**, with rename-rewrite to keep them
   valid. No GUIDs.
2. **The import cache is generated and gitignored** (`.noctis/`). Revisit if a
   committed cache proves useful.
3. **Convention-first layout with overrides** in the manifest.
4. **The project is the build/export unit**; `noctis.toml` defines the build
   targets.
5. **The project owns the asset registry**; a scene may bring its own to stay
   self-contained.

## Directory layout (convention + overrides)

```
mygame/
  noctis.toml           # the manifest (required)
  assets/               # imported source files
    models/             #   .glb/.gltf
    textures/           #   .png/.jpg/.hdr/...
    audio/              #   .wav/.ogg
    fonts/              #   .ttf/.otf
    materials/          #   material definitions, if authored separately
  scenes/               # *.scene.json
  prefabs/              # *.prefab.json
  scripts/              # *.js / *.wasm
  .noctis/              # GENERATED import cache, thumbnails (gitignored)
```

Every directory is a convention, not a requirement: `[paths]` in the manifest
overrides any of them, and assets are referenced by their path, so a developer
may organise the tree however they like. The conventions exist so drag & drop and
`noctis` tooling have a sensible default.

## The manifest: `noctis.toml`

```toml
[project]
name = "MyGame"
version = "0.1.0"
description = "A small platformer."

[project.settings]
default_scene = "scenes/main.scene.json"
mode = "3d"                       # default mode for new scenes; a scene may override
gravity = [0.0, -9.81, 0.0]

# Optional: override the conventional directories.
[paths]
assets = "assets"
scenes = "scenes"
prefabs = "prefabs"
scripts = "scripts"

[window]
title = "MyGame"
width = 1280
height = 720

[render]
hdr = true                        # HDR pipeline + tonemapping
sample_count = 1

# The input bindings the editor viewport loads (see `--controls` today).
[input]
controls = "input/controls.json"

# Build/export targets. Each is a self-contained artifact.
[[build]]
name = "mygame"
scene = "scenes/main.scene.json"  # defaults to project.settings.default_scene
platform = "host"                 # "host" | "windows" | "macos" | "linux"
profile = "release"               # "dev" | "release"
output = "dist/mygame"
```

Notes:

- `mode` is a **presentation hint**, not a schema fork (see [Scenes](#scenes)).
  It sets the default for new scenes; each scene records its own (a project may
  mix a 2D menu scene with a 3D level).
- Build targets describe *what to produce*. The first cut produces a
  self-contained folder exactly like today's
  `noctis-runtime --export`, but bundling every referenced asset rather than just
  the scene JSON.
- Unknown keys should be a load **error** (typos in a manifest are bugs), with the
  exception of forward-compatible tables we explicitly allow.

## Assets

### Canonical asset paths

An **asset path** is a logical, project-relative path:

- POSIX (`/`) separators, no leading `./`, no `..`, no leading `/`.
- Case-sensitive and exact.
- Extensions preserved: `assets/textures/wood.png`.

Asset paths are what appears in scene/prefab JSON and in the editor. They are
**not** OS filesystem paths; the project maps a logical path to a file (by
default, the file at `<project-root>/<logical-path>`).

A single file may yield several sub-assets. Those get a **fragment**:

```
assets/models/robot.glb                 # the model
assets/models/robot.glb#mesh/Torso      # a mesh inside it
assets/models/robot.glb#material/Red    # a material inside it
assets/models/robot.glb#texture/0       # a texture inside it
```

Fragments make glTF import deterministic and give every sub-asset a stable,
greppable reference.

### References

Scene and prefab documents reference assets by asset path (this is already how
`Scene::to_json` works — handles are written as paths and resolved against the
registry on load). A reference resolves to a registry handle at load time. A
missing reference is a load error that names the path, never a silent blank.

*Gap*: today `apply_component` (in `ecs::scene`) silently falls back to
`AssetHandle::NONE` when a `MeshRenderer`'s mesh or material path is absent from
the registry. Making a missing reference a load error that names the path — as
this doc requires — is pending work; until then, import every referenced asset
(the project's `import_assets` does) so the fallback is not hit.

### Rename and move: rewrite references

Renaming or moving an asset:

1. moves the file on disk,
2. updates the import cache mapping,
3. **rewrites every reference** to the old path in `scenes/`, `prefabs/`, and any
   script/asset metadata that carries asset paths.

This is a first-class operation (`Editor::rename_asset` / a `noctis` command),
not a manual find-and-replace, because a stale path is a broken scene. Sub-asset
fragments are rewritten together with their source file.

### Type detection (drag & drop)

Auto-detect by extension, confirm, then import:

| Extension(s)                         | Kind    | Default location      |
| ------------------------------------ | ------- | --------------------- |
| `.glb`, `.gltf`, `.fbx`              | model   | `assets/models/`      |
| `.png`, `.jpg`, `.jpeg`, `.bmp`, `.tga`, `.hdr` | texture | `assets/textures/`    |
| `.wav`, `.ogg`, `.mp3`               | audio   | `assets/audio/`       |
| `.ttf`, `.otf`                       | font    | `assets/fonts/`       |
| `.js`, `.mjs`                        | script  | `scripts/`            |
| `.wasm`                              | script  | `scripts/`            |
| `.scene.json`                        | scene   | `scenes/`             |
| `.prefab.json`                       | prefab  | `prefabs/`            |
| anything else                        | ask     | —                     |

Drop targets differ: dropping on the **viewport** imports *and* instantiates;
dropping on the **outliner** instantiates (a model or prefab); dropping anywhere
else just imports the file into the project. If a file with the target name
exists, import must not clobber it — suffix a counter or ask.

### Import pipeline and cache

Importing decodes a source file into engine assets and records how. Because
decoding can be expensive (a glTF with textures), the results are cached under
`.noctis/` keyed by the source file's content hash plus import settings. The cache
is generated and gitignored.

*Implemented today without the cache*: `Project::asset_map` scans `assets/` and
`scripts/`, classifies each file by extension, and `Project::import_assets`
decodes models (via `render::load_gltf`, registering each sub-asset under both its
in-file name and its `file#kind/name` fragment path) and scripts, into a registry
seeded with the engine defaults. A file that fails to import is recorded in
`ImportedAssets::errors` (a warning, not a fatal error), so one corrupt asset does
ot stop a project from opening. Audio and fonts are classified but not yet
decoded. Both `noctis-editor` and `noctis-runtime` build their registry from
`import_assets()` when `--project` is given.

Per-asset **import settings** (persisted, not in the gitignored cache) control
decoding: a texture is sRGB (color) or linear (data) — the inspector already has
this toggle; a model may set scale and up-axis. The project records settings as
sidecar metadata next to the asset (e.g. `wood.png.import.toml`) so they are
versioned and portable.

## Scenes

### One scene type; 2D is a mode

There is exactly one scene format (`*.scene.json`, `SceneDesc` with a
`format_version`). **Do not fork it into 2D and 3D.** The engine is 3D today; a
"2D scene" would only be *3D with an orthographic camera*, and a forked schema
would duplicate every component, the renderer, physics, and the editor.

`mode` is a **presentation hint** that drives editor defaults (orthographic
camera, top-down grid, planar gizmo, snapping). It is **per scene**, not just per
project, because mixing dimensions is a real use case: a project may hold a 2D
menu scene and a 3D level. Concretely, `SceneDesc` gains a `mode` field
(`"2d" | "3d"`, `#[serde(default)]`), so a scene is self-describing and loads
correctly even opened on its own. The project's `mode` is the default applied to
new scenes; a scene may override it.

If genuine 2D is wanted later, it arrives as a **component set** (`SpriteRenderer`,
`Collider2D`, Z-ordering) on the same format — 2D vs 3D then means "which
components you use".

### Scene list and the default scene

`project.settings.default_scene` names the scene the runtime runs when none is
given. A project discovers scenes by scanning `[paths].scenes`; the manifest may
also list them explicitly to set names and order.

## Prefabs

A prefab is a `Prefab { name, scene: SceneDesc }` — already serializable today.
Prefabs become project assets: `prefabs/*.prefab.json`, referenced from scenes as
`assets` are, by project-relative path. (Today they live only in the editor's
in-memory library and are lost on save; a project gives them a home.)

## Scripts

- `.js` scripts are project assets resolved by asset path; the scene's `Script`
  component references them by path (already the case).
- `.wasm` scripts use the existing descriptor path (`wasm_host`); the project
  registers them the way `noctis-runtime --wasm name=path` does today, but from
  the asset map.
- Scripts resolve assets and each other by the same project-relative paths.

## Fonts

Fonts are a first-class asset type: `assets/fonts/*.ttf|*.otf`, registered in a
`fonts` store on the registry.

They are for **in-world and game text** — a future `Text`/`TextRenderer`
component in both 2D and 3D scenes — **not** the editor UI, which uses egui's own
font stack. Glyph atlas / signed-distance-field generation is an import step,
produced on demand and cached under `.noctis/` like other decoded assets.

Adding a `fonts` store to `AssetRegistry` is a deliberate, additive extension of
the `ecs` core API; it is listed here so it is a contract change, not a surprise.

## Asset registry ownership

The **project owns one shared `AssetRegistry`**, loaded from its asset map and
shared by every scene. Scenes reference assets by path and resolve against it.

A scene **may bring its own** assets to stay self-contained (as the test scene
does today by building its own registry). Such scene-local assets are layered
*over* the project registry: project entries are the base, scene entries win on
collision. This lets a scene be opened and run on its own — useful for tests,
demos, and sharing a single scene file — without dragging the whole project.

```
resolve(path) = scene_registry.get(path) or project_registry.get(path)
```

Loading a scene therefore needs *a* registry; the project supplies the default
one.

## Build and export

`[[build]]` targets define artifacts. A build:

1. resolves the target's scene (and every asset/prefab/script it references,
   transitively),
2. writes a self-contained folder: the runtime executable, the scene, the
   referenced assets, and a manifest (`noctis-game.json`) — an extension of
   today's `export_game`, which currently bundles only the scene,
3. applies render/window/input settings from the manifest and the target.

The exported game loads its bundled default scene when run with no arguments
(already true today).

## Version control

- Manifests, scenes, prefabs, and import settings are **text** (TOML/JSON) and
  belong in version control.
- `.noctis/` is generated and **gitignored**; a fresh clone regenerates it by
  importing.
- Serialization is **deterministic** (stable key/entity order, stable float
  formatting): a no-op save produces no diff. This holds today for `SceneDesc`
  and `Prefab` — `to_desc` sorts entities and emits components in a fixed order,
  and it is now locked by tests (`scene_json_is_byte_deterministic`,
  `prefab_json_is_byte_deterministic` in `ecs::scene`). The future project asset
  map must hold the same bar (do not serialize a hash map in iteration order).

## Command line (planned)

```
noctis-editor  --project <dir> [--scene <path>] [--window]
noctis-runtime --project <dir> [--scene <path>] [--window] [--export ...]
noctis build   --project <dir> --target <name>
```

`--project` locates the manifest, loads the project registry, and makes
`--scene` a project-relative path. `--scene` alone still works for a loose,
self-contained scene file (the current behaviour).

## What already exists

Built on (reuse, do not reinvent):

- `ecs::scene::SceneDesc` — scene document with `format_version`; asset handles
  serialize as **paths** (`Scene::to_json` / `from_json` take an `AssetRegistry`).
- `ecs::scene::Prefab` — `{ name, scene: SceneDesc }`, `to_json` / `from_json`.
- `ecs::assets::AssetRegistry` — typed stores: `meshes`, `materials`, `textures`,
  `audio`, `scripts` (a `fonts` store is new, see [Fonts](#fonts)).
- `runtime::export_game` — writes `scene.json` + `noctis-game.json` + the exe
  (to be extended to bundle assets).
- `RuntimeConfig` — `physics`, `audio`, `gravity`, `timestep`.
- `editor::EditorCommand` + `--print-schema` — the agent-drivable edit API.
- `render::load_gltf` / `load_gltf_bytes`, `render::load_texture` — importers.
- `editor --controls <file>` — camera bindings (to move under the project).

New work this design implies (✔ = implemented):

- ✔ A `project` crate depending on `ecs`/`render`: manifest parsing, asset-path
  canonicalization, discovery, and asset import. Remaining: the resolver as a
  public API (path + fragment → handle is currently done by registering assets
  under both names), the `.noctis/` cache, and build.
- Rename-rewrite across scene/prefab documents.
- Drag & drop in the editor, wired to project import.
- A `fonts` asset store on `AssetRegistry` (additive core-API change) and a font
  importer that bakes a glyph atlas into the cache.
- A `mode` field on `SceneDesc` (`"2d" | "3d"`, defaulted) so scenes carry their
  own dimension.
- ✔ `--project` in `noctis-editor` / `noctis-runtime` (both now import the
  project's assets). Remaining: a `noctis` build CLI and the UI project picker.

Deterministic serialization already holds for `SceneDesc`/`Prefab` and is locked
by tests; keep it that way as the project model is added.

## Open questions

- **Import settings location**: sidecar `*.import.toml` next to the asset (this
  doc's proposal) vs a single `imports.toml` at the project root.
- **Cache key**: content hash only, or content hash + settings? (Both, to keep
  settings changes invalidating.)
- **Loose `.scene.json` without a project**: keep working forever as the
  "self-contained scene" case, or require a project from some version onward?
- **Asset packs / virtual file system**: defer; bundles are a build output, and
  the project always works from loose files on disk.
- **`.fbx` import**: listed as a model type, but there is no FBX decoder today —
  either add one or drop it from the table until there is.
- **Fonts**: bitmap atlas vs signed-distance field, and whether the baked atlas
  is a cache entry (regenerated) or a committed asset. Also which text components
  ship first (`TextRenderer` in 3D vs a 2D UI layer).
