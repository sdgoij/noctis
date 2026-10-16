# Editor extensions

> **Status:** the contract for making the editor extendible with JavaScript and
> WebAssembly. The pure loader is `crates/extensions`; the host lives in
> `crates/editor` (`editor::extension`, with the WASM half in
> `editor::extension::wasm`). The headless slice (load, discover,
> `noctis.command`, `noctis.edit`, `--run`), the windowed UI (`noctis.ui`),
> native WASM execution and `--watch` reload are implemented.

An **extension** is a directory of JavaScript, WebAssembly, JSON and assets. It is
discovered by the editor, evaluated against the open scene, and talks to the
editor through a single versioned global, `noctis`. This document defines that
global, the extension file format, the loading rules, and the safety limits.

It is the dynamic counterpart to the compiled [Rust plugin](embedding.md): a
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
  "assets": { "grid": "assets/grid.png" },
  "files": { "level1": "levels/level1.txt" }
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
| `files` | no | Slot → project-relative file the extension reads and writes (§ Project files). |

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
  const selection = noctis.editor.selection();
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

Reads are snapshots taken before the handler runs. A fixed-shape list of handles
(`selection`) comes back as a **real array**; a structured record (`list_entities`,
`inspect`) stays a **JSON string** — the convention the script API and the
WebAssembly ABI follow.

| Member | Returns |
| --- | --- |
| `noctis.editor.list_entities()` | The entity list (the same JSON `noctis editor` prints). |
| `noctis.editor.inspect(handle)` | One entity's components, or `""`. |
| `noctis.editor.selection()` | The selected entities (an array of entity refs). |
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

### Project files

```js
noctis.files.read(slot)           // the file's text, or undefined
noctis.files.write(slot, text)    // true when the write landed
noctis.files.slots()              // the declared slot names
```

A manifest may declare `"files": { "level1": "levels/level1.txt" }`: a slot the
extension reads and writes, resolved **relative to the open project** — not the
extension directory, because this is the channel for editing project *data* (a
level, a tile map, a dialogue script) rather than for shipping an asset. The host
does the I/O through the project's own store, so the same extension works over a
native directory or a bundle, and JavaScript still touches no filesystem: it
names a slot, never a path. A path that escapes the project is refused at load,
exactly as an asset path is.

`read` is `undefined` for an undeclared slot, for a file that is missing, and
when no project is open (`write` then fails and logs why). A write is
**immediate**: it is not an `EditorCommand`, so it is not undoable and not part of
an edit batch.

### UI (windowed)

```js
noctis.ui.panel("Scatter", (ui) => {
  if (ui.button("Scatter 5")) noctis.run("scatter 5");
  const on = ui.checkbox("Enabled", true);
  const n = ui.slider("Count", 5, 1, 20);
  const shape = ui.select("Shape", 0, ["cone", "box", "sphere"]);
  ui.label(`enabled ${on}, count ${n}, shape ${shape}`);
});
noctis.ui.menu("Tools", "Scatter", () => noctis.run("scatter 5"));
```

`noctis.ui.panel(title, draw)` registers a **dockable panel** — a view the user
places in a dock, through a status-bar button; it opens in the bottom dock when it
first appears. Its stable view id is `"{extension_id}:{title}"`, so two extensions
may share a title, and a title change is a new panel. `draw` receives a recording
`ui` handle — `text`/`label`, `heading`, `separator`, `button`, `checkbox`,
`slider` and `select`. `ui.button(label)` is live: it returns true the frame after
the click. `ui.checkbox(label, value)`, `ui.slider(label, value, min, max)` and
`ui.select(label, index, options)` are stateful: the host renders them and the next
frame returns the value the user set, falling back to the passed default — a menu
returns the chosen option's *index*, and falls back to the first option when the
index it is given is out of the list's range (a shorter list than the one that
produced it). `noctis.ui.menu(menu, label, run)` is a one-button panel. Panels are
windowed-only; a headless run never calls `draw`. The WASM equivalent is the
`ui_panel` import (§ WASM panels). The dock layout is designed in
[`.notes/editor-ui.md`](editor-ui.md).

### The viewport overlay (windowed)

```js
noctis.ui.viewport(function (vui) {
  var p = vui.pointer();
  if (p.inside) { vui.capture(); }
  vui.rect(0, 0, p.width, p.height, "#000000c0");
  vui.text(8, 8, "hello", "#ffffff", 14);
});
```

`noctis.ui.viewport(draw)` registers an **overlay** drawn in the editor's central
viewport, over the rendered scene (or over the frame a driver published). `draw`
runs once per frame with a recording `vui`; returning `false` reports that the
overlay wants nothing this frame, and the editor then keeps the viewport — and
its own hints — to itself.

It is **screen space**, not world space: every primitive is measured in pixels
from the viewport's top-left corner, and `vui.size()` is the viewport's size. The
overlay is therefore a canvas the extension lays out itself — a tile grid, a HUD,
a debug drawing.

| Member | Description |
| --- | --- |
| `vui.size()` | The viewport's `{ width, height }`, in pixels. |
| `vui.pointer()` | `{ x, y, inside, down, clicked, right_down, right_clicked, wheel, dx, dy, shift, ctrl, alt }`; `x`/`y` are viewport pixels, `dx`/`dy` the movement this frame, `clicked`/`right_clicked` edge-triggered. |
| `vui.capture()` | Claim the pointer for the frame: the editor skips its own camera navigation, gizmo handling and click-to-select. |
| `vui.rect(x, y, w, h, color, filled?)` | A rectangle, filled unless `filled` is `false`. |
| `vui.stroke(x, y, w, h, color, thickness?)` | A rectangle's outline. |
| `vui.line(x1, y1, x2, y2, color, thickness?)` | A line segment. |
| `vui.text(x, y, text, color, size?)` | Text anchored at its top-left. |

A colour is `"#rrggbb"`, `"#rrggbbaa"`, `"#rgb"` or `[r, g, b, a]` with 0-255
channels. The ops are clipped to the viewport and painted in registration order,
after the scene and before the editor's own overlay chrome. A `draw` that throws
is logged against its extension and skipped for that frame.

The overlay is **JavaScript-only** for now; the WASM ABI (see the WASM panels)
does not expose it.

#### A note on object literals

Slag's whole-literal shape cache (`crux::map::Map::set_composite_child`, reached
from `Step::ObjectFast`) inserts a shape map with **no write barrier**, so a
literal shape whose first evaluation lands *after* a minor collection leaves an
unrecorded old→young edge in the heap. The engine's verifier is on in a debug
build, so an extension that allocates enough to force a collection panics the
editor with `write-barrier miss`; in a release build the shape can instead be
collected while that cache still points at it.

Two places warm their shapes at load to stay clear of it, both before any
collection can promote the shared canonical map: `ExtensionHost` builds one of
every object the bootstrap's panel and viewport recorders make (`WARMUP` in
`editor::extension`), and an extension that allocates heavily in a hot path
builds its own (the level editor in `examples/reverie` does — see its
`warm_shapes`, which also fills long-lived objects in place rather than replacing
them, since a *fresh* object stored into a long-lived one is the same hazard).
The regression guard is
`editor::window::tests::the_reverie_level_editor_paints_and_validates_a_project_level`,
which drives the shipped overlay for two hundred frames.

The real fix is one line in Slag — a `write_barrier_handle(self, child)` call in
`set_composite_child` — in the pinned revision.

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
| `ui_panel` | `(ptr, len) -> i32` | Register a dockable panel titled by those bytes; returns its module-local id (idempotent by title). Its view id is `"{extension_id}:{title}"`. |
| `ui_activated` | `(panel, widget) -> i32` | Whether that button was clicked this frame. |
| `ui_value` | `(panel, widget) -> f64` | The value a checkbox/slider/menu was set to, or NaN when none. |

**Exports**: `noctis_abi() -> i32`, `noctis_init() -> i32`,
`noctis_update(ptr, count, dt) -> i32` (all three are required). A module that
registers a panel must also export `noctis_alloc(len) -> i32` (a buffer in its
memory) and `noctis_ui(panel, out_ptr, out_cap) -> i32`.

A `noctis_abi` the host does not know is refused by name, and an import with a
signature the ABI does not fix is refused *before* the module runs, so a
mismatched module is rejected rather than called with misread arguments. The
module's bytes are read once by the loader (which validates the wasm magic) and
handed to the host; JS never sees them. The host instantiates the module, calls
`noctis_init` once, then `noctis_update(0, 0, dt)` each frame; `edit`/`run` queue
work the host replays after the call, in the order the module issued it, as one
undo step. `log` is served from the frame's read snapshot.

### WASM panels

A panel is drawn with the same widget-op JSON the JavaScript host records: the
host calls `noctis_alloc(cap)` for a buffer, then `noctis_ui(panel, ptr, cap)`,
which writes the ops into that buffer and returns their byte length (`-2` when
they do not fit, so the host retries with a larger buffer). The host parses the
ops exactly as it parses a JavaScript panel's. Interaction state is pulled back
through `ui_activated` and `ui_value`, so a button, slider or menu reads its result
the way the JS `ui` handle does. `ui_panel` ids are module-local; the host lays a
module's panels out contiguously after the JavaScript ones and offsets the
arguments, so the ids the window uses are the frame's global ones.

## Hot reload (`--watch`)

A windowed run may pass `--watch`: the extension directories are watched
recursively (on a `notify` background thread), and a change seen during the frame
rebuilds the host from a fresh discovery. A rebuild is a full reload — the
previous host is dropped and every enabled extension is evaluated again — so
JavaScript module state is reset, exactly as a page reload resets a script. Scene
edits an extension already made are not undone; they live on in the editor's own
undo history. A newly created extension appears on the next change; removing or
disabling one drops it. `--watch` needs a window; `--headless` with it is a usage
error.

## Persistence

Enablement is remembered. A single user config, at
`$NOCTIS_CONFIG_DIR/extensions.toml` (default: `%APPDATA%\noctis\extensions.toml`
on Windows, `$XDG_CONFIG_HOME/noctis/extensions.toml` or
`~/.config/noctis/extensions.toml` elsewhere):

```toml
# Extensions the user disabled, by id. Everything discovered is enabled unless
# listed here. Written by the extensions screen, which applies a toggle
# immediately; read at startup.
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

- **No `fs`** in the JS engine; JS gets no path. Assets are opaque names, and a
  declared project file is a **slot** the host resolves against the project —
  JavaScript still names no path (`noctis.files`).
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

## Command line

| Flag | Effect |
| --- | --- |
| `--extensions <dir>` | Scan only this directory (repeatable). |
| `--no-extensions` | Scan nothing. |
| `--run <line>` | Dispatch a verb line after loading (repeatable). |
| `--watch` | Reload extensions when their files change (windowed). |

The windowed editor gains a console line that dispatches the same vocabulary, and
an **Extensions** screen listing the table with toggles that write the config. Both
are editor windows in their own right, so they are drawn even when no extension was
discovered (the screen then reports an empty discovery); only the extensions' own
panels need a live host.

## What already exists

- `crates/vfs` — the `Store` abstraction (`DirStore`, `MemoryStore`, `ZipStore`)
the loader reads through; relative-path validation and the archive caps live here.
- `crates/extensions` — discovery (directories and `.zip` archives),
  `extension.json`, ordering, caps, assets, and the enablement config, purely
  (no Slag, no editor) and unit-tested. It reads through `vfs`.
- `noctis.toml` — the `[extensions]` table (`dirs`, `disabled`).
- `editor::extension` — the JS host: the `noctis` global
  (`on`/`command`/`edit`/`editor`/`ui`/`log`/`freeze`), loading, `"ready"`, verb
  dispatch, `"update"`, panel drawing, and edit batching through `Editor::apply`.
  Each extension gets its own Slag context (a realm each).
- `editor::extension::wasm` — the WASM host: `decode`/`validate`, link the
  `noctis` imports (default-deny), instantiate, call `noctis_abi`/`noctis_init`,
  and drive `noctis_update` each frame — queueing `log`/`edit`/`run` through the
  same ordered bridge as the JS host. It also draws WASM panels (`ui_panel` +
  `noctis_alloc`/`noctis_ui` + `ui_activated`/`ui_value`).
- `editor::window` — the windowed UI: panels rendered as egui windows (buttons,
  checkboxes, sliders and menus are live), the `"update"` hook each frame, a console
  with a scrollback, an Extensions screen whose toggles write the config, and the
  extensions' **viewport overlays** painted in the central panel (a `captured`
  overlay also suppresses the editor's own camera navigation and picking).
- `editor::extension` — the `noctis.files` channel: a manifest's `files` slots
  resolved against the open project's store (reads and writes), so an extension
  can edit project data without seeing a path.
- `editor` CLI — `--extensions`, `--no-extensions`, `--run`, `--watch`.
- A fixture at `examples/extensions/`.

## Phasing

1. **Loader** (`crates/extensions`): discovery, manifest, ordering, caps, config.
   *(shipped)*
2. **Headless host**: the `noctis` global, `ready`/`command`, `edit`, `--run`.
   *(shipped)*
3. **Windowed UI**: `noctis.ui.panel`/`menu` in `EditorDriver::ui`, the console,
   and the Extensions screen writing the config. *(shipped)*
4. **WASM execution**: instantiate a module, drive `noctis_update`, provide the
   imports above, and draw panels through the buffer/panel ABI. *(shipped)*
5. **Dev loop**: `--watch` reload (reusing `notify`); a change to a watched
   extension directory rebuilds the host from a fresh discovery. *(shipped)*
6. **Distributed extensions**: `.zip` archives with `extension.json` at the root.
   *(shipped)*
7. **Per-extension contexts**: one Slag context (realm) each, host-level built-ins
   and panel-id offsets. *(shipped; a worker-thread host remains open)*
8. **Viewport overlays and project files**: `noctis.ui.viewport` (a screen-space
   overlay in the central panel, with pointer input and a capture flag) and
   `noctis.files` (manifest-declared project slots the host reads and writes).
   *(shipped, JavaScript-only; the WASM ABI exposes neither yet)*

## Contexts and threads

Each JavaScript extension runs in **its own Slag context** — its own `noctis`
global, its own verb, hook and panel registries. One extension cannot see
another's globals or verbs, and a throw that leaks past a wrapper cannot corrupt
another's realm. The host owns the contexts (`ExtensionHost::extensions`), keeps
the load order, and provides the cross-cutting built-ins:

- `help` aggregates every context's verbs (`__noctis_verb_names()`), sorted;
- `extensions` is answered from the host's id list;
- a verb line is offered to each context in load order, and the first match wins
  (`noctis.run` from inside an extension dispatches within that extension only);
- `"ready"`/`"update"` fire on every context, and panels are drawn per context.

Panel ids are context-local: the host lays the contexts' panels out contiguously
and offsets `ui` indices by `panel_base`, so a click maps back to its source (the
JS and WASM hosts share this mechanism).

Per-extension contexts depend on a Slag fix: an earlier rev crashed the process
(`STATUS_ACCESS_VIOLATION`) when two `embed::Context`s were live and the first was
evaluated. That is fixed in the pinned rev; the isolation is covered by the
`editor::extension` tests (`each_extension_runs_in_its_own_context`).

**Threads are still out of reach.** `Context`/`Agent` hold `Rc<RefCell<…>>`, so
they are `!Send`; a context can only be created and used on the thread that owns
the editor, which is single-threaded by design. Moving the host to a worker would
mean the worker owning the whole host (isolate included) and exchanging only the
[`Bridge`] with the editor — the read snapshot in, the edits/logs/reply/panels
out — an actor split the bridge is already shaped for, but which is not built.

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

- Per-extension contexts and a worker-thread extension host: blocked by the Slag
  `Context` model above; the route is a migration to `slag::api::Isolate` +
  realms. Worth doing once the single-context host is stable enough to split.
- Does the Extensions screen live in a project, globally, or both — and does a
  project `[extensions]` `disabled` list override the user config? (Design: they
  union; a project cannot *force-enable* what the user disabled.)
- Should `noctis.edit` be able to read the applied result synchronously, at the
  cost of a third phase in the host bridge? (Current: applied after the handler.)
