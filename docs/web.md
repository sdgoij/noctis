# Noctis in the browser (WebAssembly)

> **Status:** the design and contract for a browser target. **Nothing is built
> yet.** This document records the decisions, the seams to introduce, and the
> phasing; update it deliberately when the design changes, as with
> [`docs/project.md`](project.md) and [`docs/extensions.md`](extensions.md).

Noctis is native-first: `wgpu` for rendering, `rapier3d` for physics, `rodio` for
audio, `winit` for the window, and Slag for scripting. The goal of this document
is to run the same engine **in a browser**, as a WebAssembly module, driven by
the same `AppDriver` loop and loading the same authors' projects — first as a
**game client**, then (later) the **editor**.

Two things make this tractable rather than a rewrite:

- The engine logic is clean. `math`, `ecs` and `physics` touch **no** filesystem,
  thread or network API, and the asset decoders (`image`, `gltf`, `ab_glyph`) are
  pure Rust that work on bytes.
- The host boundary already exists. `platform` drives any `AppDriver` into a
  surface-backed viewport (`crates/platform/src/window.rs`); a web host is a
  second implementation of the same seam. And the extension loader's
  `Source` (`Dir | Zip`) is already "read a tree from a directory *or* an
  in-memory archive" — exactly what a browser project needs.

Nothing in this document is implemented; it is the plan and the contract.

## Decisions

1. **The browser client comes first; the editor follows.** The generic wasm
   **client** — the stripped runtime with a web platform host, loading a project
   bundle — is the first target. It is a fraction of the surface (no outliner,
   inspector, gizmos or extension editing) and delivers the headline "play my
   game in the browser". The browser **editor** reuses the same web host.
2. **WebGPU only, for now.** The renderer targets `wgpu`'s WebGPU backend. The
   WebGL2 fallback stays *open* — behind `wgpu`'s `webgl` feature and a renderer
   feature flag — but is not built until WebGPU reach proves insufficient.
3. **`noctis build` stays native; the browser only *runs*.** A project is compiled
   and packed by the native tool into a **bundle**; the browser fetches and runs
   the bundle. We do not run cargo (or any compiler) in the browser.
4. **No Rust plugins on the web.** A `[rust]` plugin is compiled and linked by the
   native tool (see [`docs/embedding.md`](embedding.md)); that path cannot run in
   a browser. A web game uses **JavaScript** (and native `.wasm` scripts) only.
   That is acceptable: the plugin seam is an authoring/embedding feature, not a
   runtime requirement.
5. **One `Store` abstraction for all file access.** A new `vfs` crate defines a
   byte- and path-oriented `Store` trait with a native filesystem backend and web
   backends (OPFS, IndexedDB). `project`, `extensions`, `runtime` and `editor`
   read through it instead of `std::fs`. A **project bundle is a `.zip`**, read
   with the same `Source` the extension loader already uses.
6. **Slag runs interpreted on the web.** The Cranelift JIT cannot run in the
   browser (no executable memory), so web scripting is the interpreter; scripts
   are otherwise the same Slag engine, and Slag already builds for
   `wasm32-unknown-unknown` (its own browser demo drives the DOM from Slag). The
   JIT stays a native-only feature.
7. **The web target is feature- and target-gated; native is the default.** Web
   backends live behind `cfg(target_arch = "wasm32")` (or dedicated crates) and
   never change the native build or its dependency graph.

## What ports, what does not

| Layer | Crates | Web readiness |
| --- | --- | --- |
| Core sim | `math`, `ecs`, `physics` | **Clean.** No fs/thread/net; `rapier3d` has real `wasm32` builds. |
| Asset decode | `render` (CPU side), `project` (parse) | **Clean if fed bytes.** `image`/`gltf`/`ab_glyph` and `toml` are pure Rust. |
| GPU | `render` | wgpu has a WebGPU backend, but device init is **synchronous today** (`pollster::block_on(request_adapter/request_device)`) — it must become `await`-able. |
| Window / input / UI | `platform` | winit has a wasm backend and `egui-wgpu` paints to a canvas, but this is the least-travelled path; a new web host implements the same `AppDriver` seam. |
| Scripting | `script` (Slag) | Slag builds for `wasm32-unknown-unknown` already; the **JIT is dropped** on web. |
| Audio | `audio` (`rodio`→`cpal`) | **Weakest link.** Needs a WebAudio backend and decoding from bytes, not `File`. |
| Project / data | `project`, `extensions`, `editor`, `runtime` | All use `std::fs` → move behind `vfs`. `notify` (watch), `std::process`, and cargo/plugin builds are **native-only**. |
| Tooling | `noctis`, `client` build | `noctis build`/`pack`, `[rust]` plugins and `--watch` stay native. |

## The seams to introduce

New crates (names provisional; they never leak into the native-only build):

- **`crates/vfs`** — the `Store` trait and backends.
  ```rust
  /// A byte- and path-oriented filesystem view (native dir, web storage, or an
  /// in-memory archive).
  pub trait Store {
      fn read(&self, path: &str) -> Result<Vec<u8>, StoreError>;
      fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), StoreError>;
      fn exists(&self, path: &str) -> bool;
      fn list(&self, dir: &str) -> Result<Vec<String>, StoreError>;
      fn remove(&mut self, path: &str) -> Result<(), StoreError>;
  }
  ```
  Backends: `DirStore` (native `std::fs`), `ZipStore` (read an archive — lift the
  extension loader's `Source::Zip` into this crate), `MemoryStore` (tests),
  `OpfsStore` and `IdbStore` (web, behind `cfg(wasm32)`). Relative-path validation
  (no `..`, no absolute paths) already exists in `extensions::source` and moves
  here as the shared rule.
- **`crates/platform-web`** — a web `AppDriver` host: acquire the canvas, create
  the wgpu surface, run `requestAnimationFrame`, and drive `prepare`/`update`/`ui`
  exactly as the native host does. `egui-wgpu` paints the same UI. The native
  `platform` stays untouched.
- **`crates/web`** (the `noctis-web` module) — the wasm entry point and its JS
  glue: instantiate, expose `start(canvas, bundleUrl)`, and own the runtime. This
  is the generic client; the project comes from the bundle, not the binary.
- **`render` async init** — split device creation behind an `async fn` so the web
  host can `await` `request_adapter`/`request_device`; the native path keeps its
  `pollster::block_on` wrapper.
- **`audio` web backend** — a WebAudio source/sink behind the same engine API
  (`AudioEngine`), with `silent()` already available for a no-audio fallback;
  decoding via a pure-Rust decoder fed bytes.

## The project bundle

A **project bundle** is a `.zip` with the project at its root:

```
noctis.toml
scenes/*.scene.json     (or the compact scene format scripts.md describes)
prefabs/*.prefab.json
assets/…                (meshes, textures, audio, fonts, HDRIs)
scripts/…               (`.js`, `.wasm`)         [dev profile; see docs/scripts.md]
```

This is the same shape `noctis build` already writes as a folder — a bundle is
the folder zipped. It reuses:

- the `Source::Zip` reader from `crates/extensions` (promoted into `vfs`), so a
  bundle and an extension archive are read by one code path;
- [`docs/project.md`](project.md)'s manifest and canonical asset paths — the same
  `noctis.toml`, resolved against the bundle root instead of a directory;
- [`docs/scripts.md`](scripts.md)'s profiles — a `release` bundle bakes scripts in
  (like the embedded pack), a `dev` bundle ships them loose and the client may
  reload them.

The native tool gains a small `noctis pack` (or `noctis build --bundle`): zip the
export folder into `name.zip`. Nothing about the *runtime* is browser-specific —
the bundle is a normal project.

## Loading and storing (URL / OPFS / IndexedDB)

Three `Store` backends cover the browser's needs:

- **URL (read-only).** `start(canvas, "https://…/game.zip")` fetches the bundle and
  reads it through a `ZipStore` in memory. This is the "run a published game"
  path: a static file, correct CORS, no storage.
- **OPFS (read/write).** The Origin Private File System is a real, private
  directory tree — the closest match to `std::fs` — so a `DirStore`-shaped
  backend over OPFS lets the editor open, edit and **save** a project in the
  browser, and lets an imported bundle be **unpacked** for editing.
- **IndexedDB (read/write, KV).** A whole-file KV store keyed by path, for
  browsers without OPFS or for a simpler "save a blob per file" model. It is the
  wider-support fallback.

Open and save flows:

- **Open from URL:** fetch → `ZipStore` (read-only) or unpack into OPFS.
- **Import a folder:** the user drags files onto the canvas; the JS glue reads
  them as bytes and writes them through the `Store` (the editor's existing
  drag-and-drop shapes onto this).
- **Save:** serialize the `Store` back to a `.zip` → a `Blob` download, or persist
  to OPFS/IndexedDB.

The asset **import pipeline** (`project::import_assets`) changes from "walk the
filesystem and decode" to "walk the `Store` and decode the bytes" — `image` and
`gltf` already accept bytes.

## The browser client (generic)

The wasm module is **project-agnostic**; the bundle is the payload. The entry:

1. The page (or JS glue) creates a canvas and calls
   `start(canvas, bundleUrl, { headless })`.
2. The glue fetches the bundle; the web host reads `noctis.toml` through the
   `Store` (the same `Project::open` / manifest parse as native).
3. The client builds the scene and asset registry in memory (the same `runtime`
   import path, fed bytes), then enters the `AppDriver` loop in a
   `requestAnimationFrame` callback.
4. Scripts run on Slag (interpreter), reading and writing components through the
   same reflection API.

Essentially `noctis run` with a `Store` backend that fetches, and a web
`platform`. `client` (the native stripped binary) and the web module share
`runtime`; only the host and the storage differ.

## Scripting on the web

Slag already builds for `wasm32-unknown-unknown` with **no wasm-bindgen**: its
browser demo is a dependency-free module that imports three `env` functions
(`now_ms`, `now_monotonic_ms`, `console`) and exports an eval/drain surface, with
an optional DOM bridge gated by a host flag. That is the model for us:

- **Interpreted, not JIT'd.** Web uses Slag's interpreter (no `jit` feature); the
  JIT remains native-only. Script semantics are otherwise identical.
- **The DOM bridge is off for game scripts.** Slag can drive the DOM, but a game's
  scripts should not reach the page; the host grants the bridge only if it wants
  document access (an editor extension might). This mirrors the engine's I/O wall.
- **Native `.wasm` scripts** already travel the engine as descriptors and run in
  Slag's `Store`; on web they are one nested engine inside another — verify cost,
  but no new design.
- **Where scripts come from:** a `dev` bundle ships `scripts/` and the client
  reads them through the `Store`; a `release` bundle bakes them in (the embedded
  pack, [`docs/scripts.md`](scripts.md)). Hot reload is native-only.

## The frame loop, async init, and audio

- **Async entry.** `render`'s device/adapter creation becomes `async` so the web
  host can await it; the native path wraps it in `pollster::block_on`. The wasm
  entry is an `async fn` driven by `wasm-bindgen-futures`.
- **`requestAnimationFrame`.** The web `AppDriver` runs one frame per rAF, with
  `dt` from a `performance.now()` clock.
- **Time.** `std::time::Instant` on `wasm32-unknown-unknown` is a known footgun;
  the web host supplies a monotonic clock shim for the loop and for Slag's
  `now`/`now_monotonic` imports.
- **Audio needs a gesture.** `AudioContext` must be resumed from a user gesture;
  the first canvas interaction resumes it. `AudioEngine::silent()` already lets a
  build run without a device, which is the fallback if WebAudio misbehaves.

## Threads and headers

wasm threads (and `SharedArrayBuffer`, if Slag's worker support is used) require
cross-origin isolation: the server must send `Cross-Origin-Opener-Policy:
same-origin` and `Cross-Origin-Embedder-Policy: require-corp`. This is a **later**
concern — the interpreter is single-threaded — but it constrains hosting, so it
is recorded here. Without those headers, avoid `SharedArrayBuffer` and threads.

## Packaging and serving

Two viable build shapes; we prefer the first:

- **`wasm-bindgen` + `web-sys`.** Generate JS bindings for the entry point; the
  glue is small and our crates stay `#![forbid(unsafe_code)]` (the bindings are
  safe wrappers). Pin the `wasm-bindgen` crate and CLI versions together.
  `wasm-pack` or `trunk` can drive it.
- **Dependency-free FFI** (Slag's pattern): raw `env` imports and exports, no
  bindings. Smaller and allocation-transparent, but it needs `unsafe` at the
  boundary, which the engine forbids — so only in a leaf crate if we choose it.

Serve `noctis-web.wasm` + glue + an `index.html`; the bundle is a separate static
`.zip`. Watch the usual wasm gotchas: `getrandom` needs its `js` feature on
`wasm32-unknown-unknown` (check the transitive tree from `wgpu`/`rapier`/`rodio`),
and `wgpu` should be built for `wasm32-unknown-unknown` with its default features
(WebGPU); add its `webgl` feature only for the deferred fallback.

## Non-goals

- Running a Rust compiler or the `[rust]` plugin seam in the browser.
- `noctis build`, `--watch`, or any cargo/`std::process` step on the web.
- WebGL2 as a first-class target (kept as an option, not built).
- A web-based *authoring* workflow in this phase (the editor follows the client).
- wasm threads on the first cut (they gate on COOP/COEP hosting).

## Open questions

- **Audio.** Does `rodio`/`cpal` publish a usable WebAudio backend at the pinned
  version, or do we write a small `web-sys` sink behind `AudioEngine`? Resolve by
  spike.
- **OPFS vs IndexedDB.** OPFS is the better model but newer; do we need both, or
  is IndexedDB enough to start? (Design: `Store` hides it; ship OPFS, keep IDB as
  the fallback.)
- **WebGPU reach.** When do we pull the WebGL2 fallback in, and does the current
  pipeline (soft shadows, SSAO, HDR) fit WebGL2's format limits? (Deferred.)
- **Bundle layout.** Is `noctis.toml`-at-root enough, or does a bundle want a
  manifest that lists its parts (so a client can stream assets)? (Start with the
  plain zip.)
- **Interaction with `docs/scripts.md` snapshots.** A baked script snapshot could
  cut web startup cost; it is a native-tool concern either way.

## Phasing

1. **`vfs` + `Store`.** Extract the `Source`/zip reader from `extensions`; move
   `project`/`extensions`/`editor`/`runtime` off `std::fs` onto `Store`. Native is
   unchanged; `MemoryStore`/`ZipStore` cover tests and the extension loader.
   *(This is the bulk of the refactor and is web-agnostic — it lands natively.)*
2. **`noctis pack`.** Zip an export folder into a project bundle (native tool).
3. **`platform-web` + async render init.** A web host that clears a WebGPU canvas
   through the real `AppDriver`; the two spikes (Slag eval on wasm; a wgpu canvas)
   are folded in here.
4. **`crates/web`: the generic client.** Fetch a bundle from a URL, build the
   scene/assets through the `Store`, run the loop, run Slag-interpreted scripts.
5. **Storage backends.** OPFS (read/write) and the import/save flows.
6. **Audio web backend** and the user-gesture resume.
7. **The browser editor.** Reuse the web host; add drag-drop import, in-browser
   save, and (optionally) the DOM bridge for extensions.
8. **WebGL2 fallback** (deferred) and wasm threads (COOP/COEP), if needed.
