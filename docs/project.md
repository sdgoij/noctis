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
    textures/           #   .png/.jpg/.jpeg/.bmp/.tga
    hdris/              #   .hdr equirectangular environments (for IBL)
    audio/              #   .wav/.ogg
    fonts/              #   .ttf/.otf
    materials/          #   material definitions, if authored separately
  scenes/               # *.scene.json
  prefabs/              # *.prefab.json
  scripts/              # *.js / *.wasm
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
sample_count = 4                  # MSAA samples (1 disables); clamped to what the device supports
environment = "assets/hdris/sky.hdr"  # IBL source; omit for a neutral environment

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

# Optional: list scenes explicitly, to set a display name and order. Discovered
# scenes not listed here are appended, named after their file.
[[scene]]
path = "scenes/main.scene.json"
name = "Main Menu"
```

Notes:

- `mode` is a **presentation hint**, not a schema fork (see [Scenes](#scenes)).
  It sets the default for new scenes; each scene records its own (a project may
  mix a 2D menu scene with a 3D level).
- Build targets describe *what to produce*. The first cut produces a
  self-contained folder exactly like today's
  `noctis run --export`, but bundling every referenced asset rather than just
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

*Implemented*: `Scene::from_json` (and `Scene::try_from_desc`) validate every
`MeshRenderer` mesh/material and `AudioSource` clip against the registry and
return `SceneError::MissingAsset(<path>)` on a miss, naming the offending path.
The engine's own primitive meshes are installed by `AssetRegistry::with_builtins()`
(used by the project importer and both CLIs), so a reference to a built-in
`primitives/*` mesh never trips the check. The lenient `Scene::from_desc`/
`instantiate_into` remain for restoring known-good snapshots (editor undo/redo).
Script `path` references are not validated yet — they carry their own source and
are resolved by the script host.

### Rename and move: rewrite references

Renaming or moving an asset:

1. moves the file on disk,
2. updates the import cache mapping,
3. **rewrites every reference** to the old path in `scenes/`, `prefabs/`, and any
   script/asset metadata that carries asset paths.

This is a first-class operation (`Editor::rename_asset` / a `noctis` command),
not a manual find-and-replace, because a stale path is a broken scene. Sub-asset
fragments are rewritten together with their source file.

*Implemented*: `Project::rename_asset(from, to)` moves the file (creating parent
directories) and rewrites `from` and every `from#fragment` reference in the
project's scenes and prefabs. The rewrite is a textual substitution of the quoted
path, so documents keep their exact formatting and key order — a rename that
touches no reference changes no bytes. It is exposed as `noctis rename --project
<dir> <from> <to>`. Not yet: rewriting references inside other *assets* (a script
that hardcodes an asset path), and updating a sidecar import-settings file.

### Type detection (drag & drop)

Auto-detect by extension, confirm, then import:

| Extension(s)                         | Kind    | Default location      |
| ------------------------------------ | ------- | --------------------- |
| `.glb`, `.gltf`, `.fbx`              | model   | `assets/models/`      |
| `.png`, `.jpg`, `.jpeg`, `.bmp`, `.tga` | texture | `assets/textures/`    |
| `.hdr`                               | environment | `assets/hdris/`   |
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

*Implemented*: `Project::import_file(source)` detects the kind, copies the file
into the kind's conventional directory, and never clobbers (a `-1`, `-2`, … suffix
on collision); an unrecognised type is an error. The window host forwards winit's
`DroppedFile` to the driver (`AppDriver::drop_file`), and the editor imports a
dropped file into the open project (and instantiates a model). Distinguishing the
**drop target** (viewport vs. outliner vs. anywhere) needs the drop position and
is not yet wired — a drop always imports (and instantiates a model).

### Import pipeline and cache

Importing decodes a source file into engine assets and records how. Because
decoding can be expensive (a glTF with textures), the results are cached under
`.noctis/` keyed by the source file's content hash plus import settings. The cache
is generated and gitignored.

*Implemented*: `Project::asset_map` scans `assets/` and `scripts/`, classifies
each file by extension, and `Project::import_assets` decodes models (via
`render::load_gltf`, registering each sub-asset under both its in-file name and
its `file#kind/name` fragment path), textures, environments, and scripts into a
registry seeded with the engine defaults. A file that fails to import is recorded
in `ImportedAssets::errors` (a warning, not a fatal error), so one corrupt asset
does not stop a project from opening. Audio files decode to `AudioClip`s
(WAV/Ogg/MP3 via the `audio` crate); fonts are registered as validated source
bytes; `.hdr` environments decode via `render::load_hdr` to linear `f32` radiance
in the `hdr` store, ready for the IBL precompute. Both `noctis editor` and
`noctis run` build their registry from `import_assets()` when `--project` is
given.

*Implemented (environment)*: `[render] environment` names an imported `.hdr`; the
runtime resolves it to a handle at startup (a named environment that did not
import is an error, not a silent downgrade) and calls
`Renderer::set_environment`, so both the headless and the windowed paths
precompute IBL from it, and the editor previews the same lighting. `noctis build`
bundles the environment and writes it into the exported `noctis.toml` and
`noctis-game.json`, so a shipped game keeps its lighting. Without the setting the
renderer keeps its neutral (fully white) fallback.

Per-asset **import settings** (persisted, not in the gitignored cache) control
decoding: a texture is sRGB (color) or linear (data) — the inspector already has
this toggle; a model may set scale and up-axis. The project records settings as
sidecar metadata next to the asset (e.g. `wood.png.import.toml`) so they are
versioned and portable.

*Implemented*: texture import settings are read from a sidecar
(`<file>.import.toml`, `srgb = true|false`; an unknown key is an error), so a data
texture imports linear, and decoded textures are cached under
`<project>/.noctis/textures/` keyed by the file's content (a 64-bit FNV-1a hash
plus its byte length) and the sRGB setting — a changed file or setting misses and
re-decodes, and a corrupt entry is treated as a miss. Model import settings
(scale, up-axis) and model/audio caching are not yet implemented. `.noctis/` is
created on demand and belongs in the project's `.gitignore`.

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

*Implemented*: `SceneMode` lives in `ecs::scene`, stored on both `Scene` and
`SceneDesc` (default `3d`, so pre-existing documents stay valid). The project's
`mode` seeds a scene created by the editor CLI (`--project` with no scene), the
`--mode 2d|3d` flag overrides it, and a 2D scene's default camera (the editor's
"Add → Camera") is orthographic. Top-down grid, planar gizmo, and snapping are
still pending.

If genuine 2D is wanted later, it arrives as a **component set** (`SpriteRenderer`,
`Collider2D`, Z-ordering) on the same format — 2D vs 3D then means "which
components you use".

### Scene list and the default scene

`project.settings.default_scene` names the scene the runtime runs when none is
given. A project discovers scenes by scanning `[paths].scenes`; the manifest may
also list them explicitly to set names and order.

*Implemented*: `Project::scenes` returns the manifest's explicit `[[scene]]`
entries first (in order, with their names) followed by any discovered scene not
listed (named after its file, without `.scene.json`), deduplicated and
deterministic. `Project::discover_scenes` alone is the raw scan.

## Prefabs

A prefab is a `Prefab { name, scene: SceneDesc }` — already serializable today.
Prefabs become project assets: `prefabs/*.prefab.json`, referenced from scenes as
`assets` are, by project-relative path. (Today they live only in the editor's
in-memory library and are lost on save; a project gives them a home.)

*Implemented*: `Project::save_prefab` writes `prefabs/<name>.prefab.json` (the
name must be a plain file name), `Project::load_prefabs` parses every prefab
document (`discover_prefabs`), and the editor loads a project's prefabs on open so
`instantiate_prefab` works; `noctis editor --project <dir> --save-prefabs` writes
the edited library back. A prefab is still referenced from a scene by
*instantiating* it (the entities are copied in), not by an `assets`-style path
reference — a live `PrefabInstance` reference is future work.

## Scripts

*Implemented*:

- `.js` and `.wasm` scripts are project assets resolved by asset path; the
  scene's `Script` component references them by path. A `.wasm` file imports as a
  `wasm:` hex descriptor (the binary bytes encoded into the text script channel),
  so a native module is a first-class script asset rather than only a
  `noctis run --wasm name=path` registration. The descriptor encode/decode lives
  in `ecs::assets`, shared by the importer and the script host.
- Scripts resolve assets and each other by the same project-relative paths.

**Shipping** (see [`docs/scripts.md`](scripts.md)): the `[build] profile` decides
how scripts travel. A `release` build embeds the scene's scripts in the client and
omits them from the game folder; a `dev` build copies them in, and the client
watches and hot-reloads them. Embedded scripts are seeded into the registry at
startup, so a scene's script paths resolve with no file on disk.

## Fonts

Fonts are a first-class asset type: `assets/fonts/*.ttf|*.otf`, registered in a
`fonts` store on the registry.

They are for **in-world and game text** — a future `Text`/`TextRenderer`
component in both 2D and 3D scenes — **not** the editor UI, which uses egui's own
font stack. Glyph atlas / signed-distance-field generation is an import step,
produced on demand and cached under `.noctis/` like other decoded assets.

Adding a `fonts` store to `AssetRegistry` is a deliberate, additive extension of
the `ecs` core API; it is listed here so it is a contract change, not a surprise.

*Implemented*: the `fonts` store exists on `AssetRegistry`, holding a
`FontAsset { name, data }` (the font bytes, validated by signature) that the
project registers from `assets/fonts/*.ttf|*.otf`. `render::text` bakes a glyph
atlas on demand (`FontAtlas::bake`, byte-coverage atlas + per-glyph metrics using
`ab_glyph`), lays text out (`measure`/`layout`), and builds billboarded glyph
quads; the renderer keeps a baked atlas per font and caches it for its lifetime,
and draws every `Text` component in a dedicated depth-tested, alpha-blended pass.
Still pending: persisting the baked atlas under `.noctis/` (it is currently
re-baked per process), a non-billboard/attached orientation, and a 2D UI text
layer.

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

*Implemented*: `noctis build --project <dir> [--target <name>]` (and
`noctis run --project <dir> --export <dir> [--target <name>]`) resolves the
target's scene (or `--scene`, or the project default), imports the assets, and
bundles every file the scene references — resolved by
`project::referenced_files` through the import index, so a glTF sub-asset
referenced by name maps back to its file. Engine built-ins (`$…`,
`primitives/…`) are not bundled; they are recreated at load time. The folder gets
the runtime executable, `scene.json`, the referenced files at their relative
paths, a generated `noctis.toml` (so the exported runtime imports its assets and
runs the bundled scene), and `noctis-game.json`. Step 3 is partial: window/size
come from the manifest via the bundled `noctis.toml`, but per-target render/input
overrides are not yet applied.

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
noctis editor --project <dir> [--scene <path>] [--mode 2d|3d] [--headless]
noctis run    --project <dir> [--scene <path>] [--headless] [--export ...] [--target <name>]
noctis build  --project <dir> [--target <name>] [--scene <path>] [--out <dir>]
```

`--project` locates the manifest, loads the project registry, and makes
`--scene` a project-relative path. `--scene` alone still works for a loose,
self-contained scene file (the current behaviour). `noctis build` and
`noctis run --target` honor `[[build]]` targets (the target's scene, name, and
output directory).

## What already exists

Built on (reuse, do not reinvent):

- `ecs::scene::SceneDesc` — scene document with `format_version`; asset handles
  serialize as **paths** (`Scene::to_json` / `from_json` take an `AssetRegistry`).
- `ecs::scene::Prefab` — `{ name, scene: SceneDesc }`, `to_json` / `from_json`.
- `ecs::assets::AssetRegistry` — typed stores: `meshes`, `materials`, `textures`,
  `audio`, `scripts`, `fonts`, `hdr`.
- `runtime::export_game` — writes `scene.json` + `noctis-game.json` + the exe
  (to be extended to bundle assets).
- `RuntimeConfig` — `physics`, `audio`, `gravity`, `timestep`.
- `editor::EditorCommand` + `--print-schema` — the agent-drivable edit API.
- `render::load_gltf` / `load_gltf_bytes`, `render::load_texture`, `render::load_hdr` — importers.
- `render::Renderer::set_environment` / `render::IblData::generate` — the IBL precompute, driven by `[render] environment`.
- `editor --controls <file>` — camera bindings (to move under the project).

New work this design implies (✔ = implemented):

- ✔ A `project` crate depending on `ecs`/`render`: manifest parsing, asset-path
  canonicalization, discovery, asset import, the resolver (`referenced_files`:
  a scene's asset references → the project files behind them), build, and the
  `.noctis/` cache (texture decoding; sidecar `*.import.toml` settings).
- ✔ Missing asset references are a load error that names the path (the built-in
  primitives moved to `ecs::primitives` and are installed by
  `AssetRegistry::with_builtins()`).
- ✔ Rename-rewrite across scene/prefab documents (`Project::rename_asset`, the
  `noctis rename` command).
- ✔ Prefabs as project assets: `Project::save_prefab` / `load_prefabs`, and the
  editor's `--save-prefabs` and prefab loading.
- Drag & drop in the editor: ✔ `Project::import_file` (the import half,
  auto-detect + place + no-clobber) and the winit `DroppedFile` → `AppDriver::drop_file`
  → project-import path. Remaining: the viewport/outliner **drop-target**
  distinction (needs the drop position).
- ✔ A `fonts` asset store on `AssetRegistry` (additive core-API change) and a
  glyph-atlas + text renderer (`render::text`, the `Text` component). Remaining:
  persisting the atlas to `.noctis/`, and a 2D UI text layer.
- ✔ A `mode` field on `SceneDesc` (`"2d" | "3d"`, defaulted) so scenes carry their
  own dimension.
- ✔ An explicit `[[scene]]` list in the manifest (`Project::scenes`): names and
  order, with discovered scenes appended.
- ✔ `--project` in `noctis editor` / `noctis run` (both import the project's
  assets), and a `noctis build` CLI that bundles a project's referenced assets.
  Remaining: the UI project picker.

Deterministic serialization already holds for `SceneDesc`/`Prefab` and is locked
by tests; keep it that way as the project model is added.

## Open questions

- **Import settings location**: *resolved* — a sidecar `*.import.toml` next to the
  asset (implemented for textures).
- **Cache key**: *resolved* — content hash (64-bit FNV-1a) + byte length + the
  import setting, so a settings change invalidates.
- **Loose `.scene.json` without a project**: keep working forever as the
  "self-contained scene" case, or require a project from some version onward?
- **Asset packs / virtual file system**: defer; bundles are a build output, and
  the project always works from loose files on disk.
- **`.fbx` import**: listed as a model type, but there is no FBX decoder today —
  either add one or drop it from the table until there is.
- **Model import settings** (scale, up-axis): the texture sidecar exists; the model
  equivalent is not implemented yet.
- **Audio decoding**: *resolved* — `audio::load_clip` decodes a project's
  `.wav`/`.ogg`/`.mp3` into an `AudioClip` (no separate cache; decoding is fast).
- **UI project picker**: `--project` is the entry point today; opening a project
  from the editor UI (rather than a flag) is designed but not built, so
  `Project::open` remains the single drop-in point.
- **Fonts**: *partly resolved* — a bitmap coverage atlas (not SDF), baked on
  demand at a fixed pixel size and scaled per label; the atlas is cached in the
  renderer for the process, not yet persisted to `.noctis/` or committed. The
  in-world `Text` component ships first; a 2D UI text layer is future.
