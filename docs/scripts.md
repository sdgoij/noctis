# Noctis script packaging

How a game's **scripts** (JavaScript and WebAssembly) travel from a project to a
running game, and how a development build reloads them as they change. This
document is the contract for the `runtime::embed`, `runtime::watch`, and
`runtime::build` code; update it deliberately when the design changes.

Background: a *script* is a `Script` component on an entity, resolved by the
script runtime from either inline `source` or a project-relative `path` looked up
in the asset registry's script store. A native WebAssembly module travels as the
text descriptor `wasm:<hex>` because the script channel is text. See
[`docs/project.md`](project.md) for what a project is.

## Decisions

1. **The build profile decides the script source.** A `release` build bakes the
   scripts into the client executable and does *not* copy them into `dist`; a
   `dev` build copies them into `dist` and loads them from disk. `noctis run`
   (the development tool) always loads from disk.
2. **Embedding is scripts only, for now.** Scenes, models, textures, audio,
   fonts, and HDR environments stay as loose files in `dist`. Packaging and
   streaming larger assets is a separate design.
3. **Development loads from disk and watches.** When scripts are not embedded, a
   windowed run starts a `notify` watcher on the project root; a changed `.js` /
   `.mjs` / `.wasm` file is re-read into the registry and the script runtime
   reloads it on the next frame.
4. **The embed pack is generated, not hand-written.** `noctis build` writes a
   small JSON pack naming each referenced script and its absolute source path,
   then builds the client with `NOCTIS_EMBED_PACK` set. `crates/client/build.rs`
   turns that into a `&'static [EmbeddedScript]` with `include_bytes!`.

## How a script reaches a running game today

1. `noctis build` resolves a scene, imports the project's assets, and computes
   the files the scene references ([`referenced_files`](../crates/project)).
2. [`export_game`](../crates/runtime/src/export.rs) copies the client executable,
   writes `scene.json`, and copies each referenced file — **including scripts** —
   preserving its project-relative path, plus a `noctis.toml` when any file was
   bundled.
3. The client (`noctis-game`) runs `runtime::cli`, opens the bundled project,
   imports its assets, and runs. The script runtime resolves each `Script.path`
   from the registry's script store.

Scripts are therefore loose files under `dist/<game>/scripts/`. This is the
**development** shape; a release build changes step 2 and 3.

## Profiles

`[build] profile` in the manifest (`dev` | `release`, default `release`) selects
the shape, and `noctis build --profile` overrides it. The mapping:

| profile | client | scripts in `dist` | script source | directory |
|---|---|---|---|---|
| `release` | `--release` | no | embedded (`NOCTIS_EMBED_PACK`) | as resolved |
| `dev` | debug | yes | disk, watched | `…-dev` |

A dev build never shares a folder with a release build: unless the destination is
given explicitly with `--out`, the profile appends `-dev` to the resolved
directory (the default `dist/<name>`, or a target's `output`), so building dev
cannot overwrite the release build. `--out` is used verbatim.

`noctis run` is always the development shape, regardless of any manifest.

## The embed pack

A JSON file written under the project's `.noctis/` cache directory (gitignored):

```json
{ "scripts": [ { "path": "scripts/spinner.js", "source": "C:\\game\\scripts\\spinner.js" } ] }
```

`crates/client/build.rs` reads the pack named by `NOCTIS_EMBED_PACK`, emits
`cargo:rerun-if-env-changed` and `cargo:rerun-if-changed` for the pack and every
source, and writes `$OUT_DIR/embedded.rs`:

```rust
pub static EMBEDDED: &[EmbeddedScript] = &[
    EmbeddedScript { path: "scripts/spinner.js", bytes: include_bytes!("C:\\game\\scripts\\spinner.js") },
];
```

With no pack set, the generated table is empty, so `cargo build --workspace`
produces the development client. At startup the client seeds these into the asset
registry's script store (`ScriptAsset`), decoding JavaScript as UTF-8 and a
WebAssembly module (magic `\0asm`) into a `wasm:` descriptor. Embedded scripts
take precedence over any on-disk file of the same path.

## Hot reload

`ScriptRuntime` already re-instantiates an entity's script when the resolved
*source string* changes, and caches compiled modules by source string. So
reloading is only a matter of refreshing the registry:

1. `notify` watches the project root recursively; events are filtered to
   `.js`/`.mjs`/`.wasm`.
2. At the top of each frame the runtime drains changed paths, maps them to
   project-relative paths, re-reads the file, and replaces the `ScriptAsset`.
3. The next frame sees a changed source, recompiles the module, and runs the new
   script's `on_start`.

Watching is enabled only for a windowed run (a headless run is not interactive).

## WebAssembly

A `.wasm` file is detected as `AssetKind::Script`, so it is a first-class script
asset: it is embedded like a `.js` file, watched like one, and referenced by path
in a scene. The `wasm:` descriptor encode/decode lives in `ecs::assets` (next to
`ScriptAsset`) so both the `project` importer and the `script` host share one
definition.

## Command line

```sh
noctis build --project <dir>                 # release: scripts embedded, not in dist
noctis build --project <dir> --profile dev   # dev: scripts in dist, watched at run time
noctis run   --project <dir>                 # dev: never embedded
```

## Snapshot (later)

A follow-on can compile all scripts into a Slag startup snapshot and carry it in
the pack alongside the sources, so a release client skips parsing and warming up
at boot. The pack format is versioned to allow this additively.

## What already exists

- `project::import_assets` imports `.js` and `.wasm` (both `AssetKind::Script`)
  into the script store.
- `ScriptRuntime` reloads a script when its resolved source changes.
- `runtime::build::build_project` / `runtime::export::export_game` produce a game
  folder; `noctis build` drives them.

## Open questions

- Should the scene (and other assets) embed too? Deferred; a separate design.
- Should a dev build watch when run `--headless`? Currently no.
