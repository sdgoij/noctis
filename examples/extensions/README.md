# Example extensions

A worked **editor extension** for Noctis. Extensions are JavaScript (or
WebAssembly) discovered at run time, so the editor grows new verbs and panels
without a rebuild — the dynamic counterpart to a compiled
[`[rust]` plugin](../../.notes/embedding.md). See [`.notes/extensions.md`](../../.notes/extensions.md)
for the design.

```
extensions/
  scatter/
    extension.json   # id, name, version, api, entry
    extension.js     # the `scatter` verb
```

`scatter` duplicates the current selection `n` times through the editor's own
command API (`noctis.edit`), applied as a single undo step. It also adds a
**Scatter** panel with a button, and logs to the console when it loads.

## WebAssembly extensions

An extension may ship a compiled module instead of (or beside) JavaScript. The
manifest declares it with `"wasm": { "module": "extension.wasm" }`. A module
imports the host API from the namespace `noctis` — `log(ptr, len)`,
`editor_json(out_ptr, out_cap)`, `edit(ptr, len)` and `run(ptr, len)` — and must
export `noctis_abi() -> i32` (returning `1`), `noctis_init() -> i32` and
`noctis_update(ptr, count, dt) -> i32`. The import list *is* the API: a module
can reach nothing it did not declare, and one importing an ungranted function
fails to link. `edit`/`run` queue work the host replays after the call, in
order, as one undo step. There is no WASI, so file I/O does not link. See
[`.notes/extensions.md`](../../.notes/extensions.md#wasm-extensions) for the full
ABI; the WASM host is covered by the tests in `crates/editor/src/extension`.

## Try it

Point the editor at this directory and dispatch the verb headlessly:

```sh
noctis editor --headless --extensions examples/extensions \
    --command '[{ "op": "spawn", "name": "Source" }, { "op": "select", "entity": { "name": "Source" } }]' \
    --run "scatter 4"
```

The reply goes to stderr; stdout is the resulting scene, now with five entities.
Drop `--headless` to open the editor window with the extension loaded.

While developing an extension, pass `--watch` to a windowed run: editing
`extension.js` (or a manifest, module or asset) rebuilds the host from a fresh
discovery, so a new verb or panel appears without a restart:

```sh
noctis editor --extensions examples/extensions --watch
```

An extension is found by default in `$NOCTIS_EXTENSIONS`, `extensions/` beside
the executable, `extensions/` in the working directory, and a project's
`[extensions].dirs`. The `--extensions` flag replaces that set.
