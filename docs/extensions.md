# Editor extensions

> **Status:** the contract for making the editor extendible with JavaScript and
> WebAssembly. The pure loader is `crates/extensions`; the host lives in
> `crates/editor` (`editor::extension`, with the WASM half in
> `editor::extension::wasm`). The headless slice (load, discover,
> `noctis.command`, `noctis.edit`, `--run`), the windowed UI (`noctis.ui`) and
> native WASM execution are implemented; the remaining phase is `--watch`.

An **extension** is a directory of JavaScript, WebAssembly, JSON and assets. It is
discovered by the editor, evaluated against the open scene, and talks to the
editor through a single versioned global, `noctis`. This document defines that
global, the extension file format, the loading rules, and the safety limits.

It is the dynamic counterpart to the compiled [`[rust]` plugin](embedding.md): a
plugin is linked in and needs a rebuild, an extension is discovered at run time,
so an author (or an agent) can add a tool verb or a panel and see it without
recompiling anything. Both build on the same authoring surface — the
[`EditorCommand`](../crates/editor/src/command.rs) JSON API.

The design mirrors the Goat mod system (`goats/APIv1.md`) with the parts a
single-user editor does not need removed: there is **no `side`, no compatibility
digest, and no session sync** — extensions are local to this editor process.

## Decisions

- The API global is **`noctis`**, versioned by a single integer (`api`).
- Extensions extend the **editor only** in v1 (not `noctis run`).
- They are discovered **both** globally (a user/install directory) and from a
  project (an `[extensions]` table), so a tool can ship with a project.
- The API is designed for **JavaScript and WebAssembly** alike; the WASM ABI is
  specified here (§ WASM extensions) and implemented after the JS slice.
- Enablement is **remembered in a config file** (§ Persistence), not session-only.
- **The rule: JavaScript never touches the filesystem.** All I/O goes through
  Rust. Assets are read by the host and referenced by opaque, host-registered
  names; a name is never a path. This is the same wall the Goat mod API draws.
- **Additive first.** Within an `api` major, new events, members and slots are
  added; removing or changing meaning is a new major.
- **A broken extension never takes the editor down.** A manifest that fails to
  parse, or a handler that throws, is reported and skipped; everything else runs.

## Directory layout and the manifest

```
extensions/
  scatter/
    extension.json
    extension.js
    assets/grid.png
  compile-wasm/
    extension.json
    extension.wasm
```

An extension is a directory containing `extension.json`, or a `.zip` archive with
`extension.json` at its root. Both forms are read the same way.

### `extension.json`

```json
{
  "id": "com.example.scatter",
  "name": "Scatter",
  "version": "1.0.0",
  "api": 1,
  "description": "Scatter the selection on a grid, with a panel and a verb.",
  "author": "example",
  "loadAfter": ["com.example.gridkit"],
  "entry": "extension.js",
  "wasm": { "module": "extension.wasm" },
  "assets": { "grid": "assets/grid.png" }
}
```

| Field | Required | Meaning |
| --- | --- | --- |
| `id` | yes | Unique, stable, dot-separated. Ordering, reload and the config refer to it. |
| `name` | yes | Human name for the console and a future extensions screen. |
| `version` | yes | Semver of the extension itself. |
| `api` | yes | The `noctis` API major the extension targets. Must equal `noctis.api` (`1`). |
| `description` | no | One line. |
| `author` | no | Free text. |
| `loadAfter` | no | Ids that must load first. |
| `entry` | no | JS entry, relative to the directory. |
| `wasm` | no | A compiled module: `{ "module": "extension.wasm" }`. |
| `assets` | no | Slot → file (relative), read by the host into opaque names. |

An extension may have `entry`, `wasm`, both, or neither (a data-only extension
is just assets). A relative path that escapes the extension directory, a missing
required field, or an `api` mismatch is a load error **for that extension only**.

## Discovery

The editor scans, in order, the union of:

1. `--extensions <dir>` on the command line (repeatable). When given, these
   **replace** the default set — an explicit list is authoritative.
2. `$NOCTIS_EXTENSIONS` (one directory).
3. `extensions/` next to the executable.
4. `extensions/` in the working directory (so `cargo run` finds the repo's).
5. The open project's `[extensions].dirs` (project-relative).

`--no-extensions` skips discovery entirely. Every immediate subdirectory of a
scanned directory that contains an `extension.json` is an extension; a directory
without one is ignored, so `extensions/.disabled/` parks one. A `*.zip` file in
a scanned directory is an extension when it holds `extension.json` at its root;
its entries are read into memory under the same path rules and caps. A duplicate
`id` across directories is a warning and the first discovered wins.

The manifest is parsed strictly (`deny_unknown_fields`): a typo is an error, not
forward compatibility — the same rule `noctis.toml` follows.

## Load order

Extensions are sorted by `id`; a manifest may list `loadAfter: ["other.id"]`,
honoured topologically with `id` as the tiebreak. A cycle is a load error for the
extensions involved. Loading is all-or-nothing **per extension**.

## The `noctis` API

`noctis` is the only global an extension needs. The per-extension handle is the
wrapper argument; `noctis` is also global for convenience.

```js
noctis.api          // 1 — the API major
noctis.extension    // this extension's info: { id, name, version }
```

### Identity and lifecycle

| Member | Description |
| --- | --- |
| `noctis.begin(id)` | Host-internal: starts an extension instance. Not called by extensions. |
| `noctis.end(id)` | Host-internal: tears one down (reload/disable). |
| `noctis.freeze()` | Host-internal: closes registration; a later `register*` throws. |
| `noctis.off(handle)` | Alias for the unsubscribe function `on` returns. |
| `noctis.fail(message)` | Report a failure under this extension's id. |

Each entry is evaluated inside a private wrapper, so an extension cannot
accidentally leak a global and a reload discards its scope cleanly:

```js
(function (noctis) { "use strict"; /* extension source */ })(noctis.begin("com.example.scatter"));
```

### Events

```js
const off = noctis.on("update", (dt) => { /* ... */ });
off();   // unsubscribe
```

Handlers registered by an extension are removed when it is reloaded or disabled.
A handler that throws is logged with the extension's id and does not stop the
frame or the other extensions' handlers.

| Event | Fires when | Arguments |
| --- | --- | --- |
| `"ready"` | The editor has loaded the scene, assets and prefabs | *(none)* |
| `"update"` | Each frame, before render (`--headless` runs one `"update"` per frame) | `(dt)` |
| `"ui"` | Each frame, inside the egui pass (windowed only) | `(ui)` |
| `"command"` | A verb line was not a built-in | `(line, parts)` |
| `"select"` | The selection changed | `(entities)` |
| `"scene_loaded"` | A scene was opened | `(path)` |
| `"scene_saved"` | A scene was written | `(path)` |
| `"shutdown"` | The editor is closing, or this extension is reloading | *(none)* |

### Commands

```js
noctis.command("scatter", (parts) => {
  const n = Number(parts[1] ?? 5);
  const selection = JSON.parse(noctis.editor.selection());
  const batch = selection.flatMap((e) =>
    Array.from({ length: n - 1 }, () => ({ op: "duplicate", entity: e })));
  return `ok scatter ${noctis.edit(batch)}`;
});
```

- A verb is a single token. **Built-ins win**: registering `help`, `extensions`,
  `run`, `quit` (and the rest of the editor vocabulary) is rejected with a
  warning. Extensions extend the vocabulary; they do not shadow it.
- A handler returns a string (printed by the dispatcher), or
  `undefined`/`null`/`false` to decline, in which case the next handler and then
  the unknown-verb fallback see the line.
- A `"command"` observer runs after the built-in dispatch but before the
  fallback.

### Editor access

Reads are snapshots taken before the handler runs; they return **JSON strings**
(the pattern the script API already uses for composite values).

| Member | Returns |
| --- | --- |
| `noctis.editor.list_entities()` | The entity list (the same JSON `noctis editor` prints). |
| `noctis.editor.inspect(handle)` | One entity's components, or `""`. |
| `noctis.editor.selection()` | The selected entities (a JSON array of entity refs). |
| `noctis.editor.scene_name()` | The scene name. |
| `noctis.editor.mode()` | `"3d"` or `"2d"`. |
| `noctis.editor.find(name)` | A handle for the entity named `name`, or undefined. |

### Edits

```js
noctis.edit({ op: "spawn", name: "Box" });          // one command
noctis.edit([{ op: "spawn", name: "A" }, { op: "spawn", name: "B" }]);
```

`noctis.edit` queues `EditorCommand` objects (a single object or an array) and
returns the number queued. The host applies them through
[`Editor::apply`](../crates/editor/src/editor.rs) **after the handler returns**,
so:

- every edit is validated by the same code path the CLI and the AI agent use;
- a batch folds into **one undo step** (the editor's interactive-edit window);
- an edit that fails is reported and the rest of the batch still runs.

Edits never touch the world directly. An extension cannot corrupt the scene in a
way `undo` cannot reverse.

### Assets

```js
noctis.assets.get(slot)      // the opaque name for a declared asset, or undefined
```

The host reads a declared asset into memory and registers it under an opaque name
(`ext:<id>:<slot>`); the editor's materials/meshes reference it by that name. A
slot is not a closed set: an extension may invent one. JS never sees a path.

### UI (windowed)

```js
noctis.ui.panel("Scatter", (ui) => {
  if (ui.button("Scatter 5")) noctis.run("scatter 5");
  const on = ui.checkbox("Enabled", true);
  const n = ui.slider("Count", 5, 1, 20);
  ui.label(`enabled ${on}, count ${n}`);
});
noctis.ui.menu("Tools", "Scatter", () => noctis.run("scatter 5"));
```

`noctis.ui.panel(title, draw)` registers an egui window; `draw` receives a
recording `ui` handle — `text`/`label`, `heading`, `separator`, `button`,
`checkbox` and `slider`. `ui.button(label)` is live: it returns true the frame
after the click. `ui.checkbox(label, value)` and `ui.slider(label, value, min,
max)` are stateful: the host renders them and the next frame returns the value
the user set, falling back to the passed default. `noctis.ui.menu(menu, label,
run)` is a one-button panel. Panels are windowed-only; a headless run never
calls `draw`. The WASM equivalent is the planned `ui_panel` import (§ WASM).

## WASM extensions

A compiled extension is a WebAssembly module beside `entry` (either or both may
exist). The governing idea is the one `goats/ABIv1.md` states: **the import list
is the API**. A module can reach nothing it did not declare, and a module that
imports something the host does not grant fails to *link* — default-deny is a
property of the linker, not a runtime check. No WASI, so file I/O does not link.

The boundary is coarse: the host does **not** call per entity. It writes a record
buffer into the module's linear memory, calls `noctis_update(ptr, count, dt)`
once per frame, and reads the results out of the same memory.

**Imports** (namespace `noctis`):

| Import | Signature | Meaning |
| --- | --- | --- |
| `log` | `(ptr, len)` | Log UTF-8 bytes from the module's memory. |
| `editor_json` | `(out_ptr, out_cap) -> i32` | Write the entity list JSON; a length, or `-2` if the buffer is too small. |
| `edit` | `(ptr, len) -> i32` | Apply an `EditorCommand` batch (JSON) from memory; `0` ok, non-zero is the error count. |
| `run` | `(ptr, len)` | Dispatch a verb line. |
| `ui_panel` | `(ptr, len) -> i32` | Planned: register a panel whose draw is the export `noctis_ui(draw)`. Not yet linked. |

**Exports**: `noctis_abi() -> i32`, `noctis_init() -> i32`,
`noctis_update(ptr, count, dt) -> i32` (all three are required).
`noctis_alloc(len) -> i32` and `noctis_ui(ptr) -> i32` are reserved for the
buffer and panel ABI and not yet required.

A `noctis_abi` the host does not know is refused by name, and an import with a
signature the ABI does not fix is refused *before* the module runs, so a
mismatched module is rejected rather than called with misread arguments. The
module's bytes are read once by the loader (which validates the wasm magic) and
handed to the host; JS never sees them. The host instantiates the module, calls
`noctis_init` once, then `noctis_update(0, 0, dt)` each frame; `edit`/`run` queue
work the host replays after the call, in the order the module issued it, as one
undo step. `log` is served from the frame's read snapshot.

## Persistence

Enablement is remembered. A single user config, at
`$NOCTIS_CONFIG_DIR/extensions.toml` (default: `%APPDATA%\noctis\extensions.toml`
on Windows, `$XDG_CONFIG_HOME/noctis/extensions.toml` or
`~/.config/noctis/extensions.toml` elsewhere):

```toml
# Extensions the user disabled, by id. Everything discovered is enabled unless
# listed here. Written by the extensions screen; read at startup.
disabled = ["com.example.noisy"]
```

A project may additionally disable an id in `noctis.toml`:

```toml
[extensions]
dirs = ["tools/extensions"]   # extra search directories (project-relative)
disabled = ["com.example.wip"]
```

Effective enablement: discovered **and** not in the config's `disabled`
**and** not in the project's `disabled`. `--no-extensions` disables all; an
explicit `--extensions` limits the search set.

## Safety and limits

- **No `fs`** in the JS engine; JS gets no path. Assets are opaque names.
- The loader refuses a relative path that escapes the extension directory and
  caps the manifest (`256 KiB`), an entry (`4 MiB`), and an asset (`64 MiB`).
- A `.zip` extension is read into memory under the same rules, with a per-entry
  cap and a total-expansion cap (`256 MiB`, `1024` files) so a decompression bomb
  cannot exhaust memory.
- A manifest that fails, or a handler that throws, is reported under the
  extension's id and isolated; `noctis.fail` is the explicit form.
- JS runs on the editor's thread; the host is not `Send`. WASM runs in the same
  process through the engine's sandbox.

## Versioning

`noctis.api` is one integer, bumped only for a breaking change. A manifest
declares the major it targets; a mismatch is a load error with a clear message.
Within a major, additions keep existing names' meaning. A name scheduled for
removal logs a one-time warning and is removed no sooner than the next minor.

## Command line (planned)

| Flag | Effect |
| --- | --- |
| `--extensions <dir>` | Scan only this directory (repeatable). |
| `--no-extensions` | Scan nothing. |
| `--run <line>` | Dispatch a verb line after loading (repeatable). |

The windowed editor gains a console line that dispatches the same vocabulary, and
an **Extensions** screen listing the table with toggles that write the config.

## What already exists

- `crates/extensions` — discovery (directories and `.zip` archives),
  `extension.json`, ordering, caps, assets, and the enablement config, purely
  (no Slag, no editor) and unit-tested.
- `noctis.toml` — the `[extensions]` table (`dirs`, `disabled`).
- `editor::extension` — the JS host: the `noctis` global
  (`on`/`command`/`edit`/`editor`/`ui`/`log`/`freeze`), loading, `"ready"`, verb
  dispatch, `"update"`, panel drawing, and edit batching through `Editor::apply`.
- `editor::extension::wasm` — the WASM host: `decode`/`validate`, link the
  `noctis` imports (default-deny), instantiate, call `noctis_abi`/`noctis_init`,
  and drive `noctis_update` each frame — queueing `log`/`edit`/`run` through the
  same ordered bridge as the JS host.
- `editor::window` — the windowed UI: panels rendered as egui windows (buttons,
  checkboxes and sliders are live), the `"update"` hook each frame, a console
  with a scrollback, and an Extensions screen whose toggles write the config.
- `editor` CLI — `--extensions`, `--no-extensions`, `--run`.
- A fixture at `examples/extensions/`.

## Phasing

1. **Loader** (`crates/extensions`): discovery, manifest, ordering, caps, config.
   *(shipped)*
2. **Headless host**: the `noctis` global, `ready`/`command`, `edit`, `--run`.
   *(shipped)*
3. **Windowed UI**: `noctis.ui.panel`/`menu` in `EditorDriver::ui`, the console,
   and the Extensions screen writing the config. *(shipped)*
4. **WASM execution**: instantiate a module, drive `noctis_update`, provide the
   imports above. *(shipped for `log`/`editor_json`/`edit`/`run`; the
   `noctis_alloc`/`ui_panel` buffer and panel ABI remain)*
5. **Dev loop**: `--watch` hot-reload per extension (reusing `notify`).
6. **Distributed extensions**: `.zip` archives with `extension.json` at the root.
   *(shipped)*

## Non-goals for v1

- Extending the runtime (`noctis run`); extensions are editor-only here.
- A repository, installer or signature scheme. Extensions are local directories.
- Sandboxing untrusted JS: extensions are trusted code with an I/O wall, exactly
  as the Goat mod API draws it. WASM is the sandbox, and it is default-deny by
  construction.
- Native dynamic libraries.
- Replacing the editor's own panels. Extensions add panels; they do not take over
  the outliner or the inspector.
- Per-extension settings.

## Open questions

- Does the Extensions screen live in a project, globally, or both — and does a
  project `[extensions]` `disabled` list override the user config? (Design: they
  union; a project cannot *force-enable* what the user disabled.)
- Should `noctis.edit` be able to read the applied result synchronously, at the
  cost of a third phase in the host bridge? (Current: applied after the handler.)
