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

1. **The build profile decides how scripts travel.** A `release` build bakes the
   scripts into the client executable and does *not* copy them into `dist`; a
   `dev` build does not copy the project at all — it writes a manifest that points
   at the project, so the game runs it in place and its scripts stay live.
   `noctis run` (the development tool) also loads from disk.
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

## How a script reaches a running game (release)

1. `noctis build` resolves a scene, imports the project's assets, and computes the
   files the scene references ([`referenced_files`](../crates/project)).
2. The scene's scripts are written to an embed pack and baked into the client; the
   rest of the referenced files are copied into the folder by
   [`export_game`](../crates/runtime/src/export.rs), preserving their
   project-relative paths, plus a `noctis.toml` the runtime opens.
3. The client (`noctis-game`) runs `runtime::cli`, seeds the embedded scripts into
   the registry, opens the bundled project, imports its assets, and runs. The
   script runtime resolves each `Script.path` from the registry's script store.

## Profiles

`[build] profile` in the manifest (`dev` | `release`, default `release`) selects
the shape, and `noctis build --profile` overrides it. The mapping:

| profile | client | scripts | output |
|---|---|---|---|
| `release` | `--release` | embedded in the client (`NOCTIS_EMBED_PACK`) | `dist/release/` |
| `dev` | debug | the live project's, watched | `dist/dev/` |

A dev build never shares a folder with a release build: the default destination is
organised by profile. A named `[[build]]` target writes to `dist/<profile>/<target>`;
an explicit `--out`, or a target's `output`, is used verbatim instead.

`noctis run` is always the development shape, regardless of any manifest.

## Development runs the project in place

A dev build copies **nothing**. [`export_dev_game`](../crates/runtime/src/export.rs)
writes only the client and a `noctis.toml` whose `[project] root` is the absolute
path of the project it was built from, plus `noctis-game.json`. The runtime opens
that root, so the game runs the project's live scenes, assets, and scripts — the
same files you edit — and the watcher (below) reloads a script the moment it
changes. This is what makes `dist/dev/<game>` a usable iteration loop rather than a
frozen copy.

`[project] root` is a general manifest field: a manifest may name the directory
its files live in, so it can sit beside a client while the project stays put. An
absent `root` means the manifest's own directory.

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
noctis build --project <dir> --profile dev   # dev: client runs the project in place
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
- `runtime::build::build_project` drives `runtime::export::export_game` (release)
  or `export_dev_game` (dev); `noctis build` runs it.

## Open questions

- Should the scene (and other assets) embed too? Deferred; a separate design.
- Should a dev build watch when run `--headless`? Currently no.
