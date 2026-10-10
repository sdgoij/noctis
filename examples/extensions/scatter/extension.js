// Scatter — a worked Noctis editor extension (see docs/extensions.md).
//
// It adds one verb, `scatter`, which duplicates the current selection `n` times
// through the editor's own command API. The duplicates are queued with
// `noctis.edit` and applied as a single undo step after the handler returns, so
// one Ctrl+Z removes the whole scatter.

// `on("ready")` fires once the editor has loaded its scene, assets and prefabs.
noctis.on("ready", function () {
  noctis.log("scatter " + noctis.extension.version + " ready");
});

// A verb is a single token. Built-ins (`help`, `extensions`, ...) win, so a verb
// here must be new. `parts` is the whitespace-split line; `parts[0]` is the verb.
noctis.command("scatter", function (parts) {
  var count = parseInt(parts[1], 10);
  if (!(count > 0)) {
    count = 3;
  }

  // Reads are JSON strings — the same shape the CLI prints.
  var selection = JSON.parse(noctis.editor.selection());
  if (selection.length === 0) {
    return "ok scatter: nothing selected";
  }

  var batch = [];
  for (var s = 0; s < selection.length; s++) {
    for (var i = 0; i < count; i++) {
      // `selection[s]` is an entity reference ({ index, generation }) that an
      // EditorCommand accepts directly.
      batch.push({ op: "duplicate", entity: selection[s] });
    }
  }

  return "ok scatter: " + noctis.edit(batch) + " edit(s) queued";
});

// A panel in the windowed editor. `ui` records widgets; the host renders them,
// and `ui.button` returns true the frame after it is clicked. `noctis.run`
// dispatches the same verb the console uses.
noctis.ui.panel("Scatter", function (ui) {
  ui.heading("Scatter");
  var selection = JSON.parse(noctis.editor.selection());
  ui.label("selected: " + selection.length);
  ui.separator();
  if (ui.button("Scatter x3")) {
    noctis.run("scatter 3");
  }
});
