# Example extensions

A worked **editor extension** for Noctis. Extensions are JavaScript (or
WebAssembly) discovered at run time, so the editor grows new verbs and panels
without a rebuild — the dynamic counterpart to a compiled
[`[rust]` plugin](../../docs/embedding.md). See [`docs/extensions.md`](../../docs/extensions.md)
for the design.

```
extensions/
  scatter/
    extension.json   # id, name, version, api, entry
    extension.js     # the `scatter` verb
```

`scatter` duplicates the current selection `n` times through the editor's own
command API (`noctis.edit`), applied as a single undo step.

## Try it

Point the editor at this directory and dispatch the verb headlessly:

```sh
noctis editor --headless --extensions examples/extensions \
    --command '[{ "op": "spawn", "name": "Source" }, { "op": "select", "entity": { "name": "Source" } }]' \
    --run "scatter 4"
```

The reply goes to stderr; stdout is the resulting scene, now with five entities.
Drop `--headless` to open the editor window with the extension loaded.

An extension is found by default in `$NOCTIS_EXTENSIONS`, `extensions/` beside
the executable, `extensions/` in the working directory, and a project's
`[extensions].dirs`. The `--extensions` flag replaces that set.
