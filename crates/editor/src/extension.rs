//! Editor extensions: the `noctis` JavaScript global, loading, and verb dispatch.
//!
//! This is the host half of `.notes/extensions.md`. The pure loader
//! ([`extensions`]) discovers and reads extensions; here each one is evaluated in
//! its own Slag [`Context`](slag::Context) — Slag's "one context per realm"
//! model — against a versioned `noctis` global, and their edits are replayed
//! through [`Editor::apply_json`], the same validated, undoable path the CLI and
//! an AI agent use.
//!
//! The global is defined *in JavaScript* (the [`BOOTSTRAP`] source): it holds the
//! registered verbs and hooks and calls a handful of Rust primitives
//! (`__host_edit`, `__host_reads`, ...) for the things that need the editor. That
//! mirrors how the Goat mod API wraps the scene in a bootstrap script: the Rust
//! side stays small and the JS side owns the registry.
//!
//! A context per extension isolates them: one extension cannot see another's
//! verbs, hooks or globals, and a throw that leaks past a wrapper cannot corrupt
//! another's realm. The host keeps the ordering and the cross-cutting built-ins
//! (`help`, `extensions`) and iterates the contexts for the per-frame work.
//!
//! JS gets no ambient filesystem: an extension can only touch the editor
//! through `noctis.edit` (an `EditorCommand` batch) and the read snapshots, so
//! it cannot corrupt the scene in a way `undo` cannot reverse. Two narrow
//! channels reach further, both mediated entirely by Rust: `noctis.files` reads
//! and writes the **project files its manifest declares by slot** (the host does
//! the I/O through the project's own store, and JavaScript never sees a path),
//! and `noctis.ui.viewport` paints an overlay in the central viewport.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use ecs::scene::SceneMode;
use extensions::{Loader, MAX_ASSET_BYTES, Manifest};
use vfs::Store;

use crate::Editor;
use crate::info::EntityDto;

mod wasm;
#[cfg(not(target_arch = "wasm32"))]
mod watch;
#[cfg(target_arch = "wasm32")]
mod watch {
    //! The web build has no filesystem watcher (`--watch` is native-only), so the
    //! `--watch` types are inert stand-ins that keep the driver's shape uniform.

    /// Nothing is watched on the web.
    #[derive(Debug, Default)]
    pub struct ExtensionWatch;

    impl ExtensionWatch {
        /// Always "nothing changed" — there is nothing to watch.
        pub(crate) fn changed(&self) -> bool {
            false
        }
    }

    /// The `--watch` reload inputs; a web build never has any.
    #[derive(Debug, Default)]
    pub struct ExtensionReload {
        pub(crate) watch: ExtensionWatch,
    }
}

use wasm::WasmHost;

pub use watch::{ExtensionReload, ExtensionWatch};

/// The `noctis` global, defined in JavaScript. See the module docs.
const BOOTSTRAP: &str = r##"
(function () {
  "use strict";
  var RESERVED = ["help", "extensions", "run", "quit", "exit"];
  var current = null;
  var frozen = false;
  var commands = {};
  var hooks = {};
  var viewports = [];
  // This extension's id, for the calls that must name their owner (the file
  // channel). `noctis.extension` is set by the host before the entry is run, so
  // it is available to every verb and hook, long after `freeze` cleared
  // `current`.
  var extension_id = function () { return noctis.extension ? noctis.extension.id : ""; };

  var noctis = {
    api: 1,
    extension: null,
    begin: function (id) { current = id; return noctis; },
    end: function (id) { if (current === id) { current = null; } },
    freeze: function () { current = null; frozen = true; },
    on: function (event, fn) {
      if (frozen) { throw new Error("registration is closed"); }
      (hooks[event] = hooks[event] || []).push(fn);
      return function () {};
    },
    off: function () {},
    command: function (name, fn) {
      if (frozen) { throw new Error("registration is closed"); }
      if (RESERVED.indexOf(name) >= 0) {
        __host_log("extension " + current + " may not shadow the built-in verb '" + name + "'");
        return;
      }
      commands[name] = fn;
    },
    fail: function (message) { __host_log("extension " + current + " failed: " + message); },
    log: function () { __host_log(Array.prototype.slice.call(arguments).join(" ")); },
    edit: function (batch) {
      var list = Array.isArray(batch) ? batch : [batch];
      __host_edit(JSON.stringify(list));
      return list.length;
    },
    editor: {
      _reads: function () { return JSON.parse(__host_reads()); },
      list_entities: function () { return JSON.stringify(this._reads().entities); },
      selection: function () { return this._reads().selection; },
      scene_name: function () { return this._reads().scene; },
      mode: function () { return this._reads().mode; },
      find: function (name) {
        var list = this._reads().entities;
        for (var i = 0; i < list.length; i++) {
          if (list[i].name === name) { return list[i].entity; }
        }
        return undefined;
      }
    },
    // Declared project files. A slot is resolved by the manifest to a
    // project-relative path and the host does the I/O through the project's own
    // store, so JavaScript never sees a path. `read` is undefined when the slot
    // is undeclared or the file is missing; `write` reports success.
    files: {
      read: function (slot) {
        var text = __host_file_read(extension_id(), String(slot));
        return (text === undefined) ? undefined : text;
      },
      write: function (slot, text) {
        return __host_file_write(extension_id(), String(slot), String(text)) !== 0;
      },
      slots: function () {
        return JSON.parse(__host_file_slots(extension_id()));
      }
    }
  };
  globalThis.noctis = noctis;

  globalThis.__noctis_register_extension = function (id, details) {
    noctis.extension = JSON.parse(details);
  };

  // The host aggregates these across contexts for the `help` verb.
  globalThis.__noctis_verb_names = function () {
    return JSON.stringify(Object.keys(commands).sort());
  };

  globalThis.__noctis_fire = function (event, arg) {
    var fns = hooks[event] || [];
    for (var i = 0; i < fns.length; i++) {
      try { fns[i](arg); } catch (e) { __host_log("hook '" + event + "' threw: " + e); }
    }
  };

  // Dispatch within this extension's own context. The console, the `help`
  // built-in and other extensions' verbs are host-level; an extension sees only
  // its own verbs.
  globalThis.__noctis_dispatch = function (line) {
    var text = String(line).trim();
    var parts = text.length ? text.split(/\s+/) : [];
    var verb = parts[0] || "";
    var reply;
    var observers = hooks["command"] || [];
    for (var i = 0; i < observers.length; i++) {
      try {
        var observed = observers[i](text, parts);
        if (typeof observed === "string") { reply = observed; break; }
      } catch (e) { __host_log("command observer threw: " + e); }
    }
    if (reply === undefined) {
      var handler = commands[verb];
      if (handler) {
        try {
          var out = handler(parts, text);
          reply = (typeof out === "string") ? out : "";
        } catch (e) { reply = "error: verb '" + verb + "' threw: " + e; }
      }
    }
    return (reply === undefined || reply === null) ? null : String(reply);
  };

  // The host calls this to dispatch a line; `null` means no verb matched.
  globalThis.__noctis_dispatch_line = function (line) {
    var reply = __noctis_dispatch(line);
    if (reply === null) { __host_reply(0, ""); }
    else { __host_reply(1, reply); }
  };

  // Editor panels. `panel(title, draw)` registers a floating panel; `draw`
  // receives a recording `ui` and the host renders the ops it records. A
  // recorded button is live: `ui.button(label)` returns true on the frame after
  // it is clicked. `ui.checkbox(label, value)` and `ui.slider(label, value, min,
  // max)` return the value the user set on the previous frame. `menu(menu, label,
  // run)` is a one-button panel.
  var panels = [];
  noctis.ui = {
    panel: function (title, draw) {
      if (frozen) { throw new Error("registration is closed"); }
      panels.push({ title: String(title), draw: draw });
    },
    menu: function (menu, label, run) {
      if (frozen) { throw new Error("registration is closed"); }
      panels.push({ title: String(menu), draw: function (ui) { if (ui.button(label)) { run(); } } });
    },
    // A viewport overlay: `draw(vui)` runs each frame while the central viewport
    // is drawn. `vui` paints in viewport pixels (origin top-left) and reads the
    // pointer; `vui.capture()` claims the pointer for the frame, so the editor
    // skips its own camera navigation and click-to-select. Returning `false`
    // reports that the overlay wants nothing this frame (the editor then keeps
    // the viewport to itself).
    viewport: function (draw) {
      if (frozen) { throw new Error("registration is closed"); }
      viewports.push(draw);
    }
  };

  // Run a verb line from a panel (the same vocabulary as the console).
  noctis.run = function (line) { return __noctis_dispatch(String(line)); };

  globalThis.__noctis_draw_panels = function () {
    for (var p = 0; p < panels.length; p++) {
      var ops = [];
      var n = 0;
      var ui = {
        text: function (s) { ops.push({ kind: "label", text: String(s) }); },
        label: function (s) { ops.push({ kind: "label", text: String(s) }); },
        heading: function (s) { ops.push({ kind: "heading", text: String(s) }); },
        separator: function () { ops.push({ kind: "separator" }); },
        button: function (label) {
          var id = n++;
          var hit = __host_activated(p, id) !== 0;
          ops.push({ kind: "button", id: id, label: String(label) });
          return hit;
        },
        checkbox: function (label, value) {
          var id = n++;
          var v = __host_value(p, id);
          var checked = (v === v) ? (v !== 0) : !!value;
          ops.push({ kind: "checkbox", id: id, label: String(label), value: checked ? 1 : 0 });
          return checked;
        },
        slider: function (label, value, min, max) {
          var id = n++;
          var v = __host_value(p, id);
          var current = (v === v) ? v : Number(value);
          ops.push({ kind: "slider", id: id, label: String(label), value: current, min: Number(min), max: Number(max) });
          return current;
        },
        // A menu of `options`: the value is the chosen index, or 0 for an empty
        // list. The chosen option's text is the menu's own label.
        select: function (label, value, options) {
          var id = n++;
          var v = __host_value(p, id);
          var list = [];
          for (var o = 0; o < options.length; o++) {
            list.push(String(options[o]));
          }
          var current = (v === v) ? v : Number(value);
          if (!(current >= 0 && current < list.length)) {
            current = 0;
          }
          ops.push({ kind: "select", id: id, label: String(label), value: current, options: list });
          return current;
        }
      };
      try { panels[p].draw(ui); } catch (e) { __host_log("panel '" + panels[p].title + "' threw: " + e); }
      __host_panel(String(panels[p].title), JSON.stringify(ops));
    }
  };

  // A colour: "#rrggbb", "#rrggbbaa", "#rgb" or [r, g, b, a] (0-255). Normalized
  // to a flat [r, g, b, a] the host parses.
  function channel(value, fallback) {
    var n = Number(value);
    if (n !== n) { return fallback; }
    return n < 0 ? 0 : (n > 255 ? 255 : Math.round(n));
  }

  function parse_color(value) {
    if (typeof value === "string") {
      var text = value.trim();
      if (text.charAt(0) === "#") { text = text.slice(1); }
      if (text.length === 3) {
        text = text.charAt(0) + text.charAt(0) + text.charAt(1) + text.charAt(1) + text.charAt(2) + text.charAt(2);
      }
      if (text.length === 6) { text = text + "ff"; }
      if (text.length !== 8 || !/^[0-9a-fA-F]{8}$/.test(text)) { return [255, 255, 255, 255]; }
      var n = parseInt(text, 16);
      return [(n >>> 24) & 255, (n >>> 16) & 255, (n >>> 8) & 255, n & 255];
    }
    if (Object.prototype.toString.call(value) === "[object Array]") {
      return [
        channel(value[0], 255), channel(value[1], 255), channel(value[2], 255),
        value[3] === undefined ? 255 : channel(value[3], 255)
      ];
    }
    return [255, 255, 255, 255];
  }

  globalThis.__noctis_draw_viewport = function () {
    if (viewports.length === 0) { return; }
    var input = JSON.parse(__host_viewport_input());
    for (var v = 0; v < viewports.length; v++) {
      var ops = [];
      var claimed = false;
      var vui = {
        size: function () { return { width: input.width, height: input.height }; },
        pointer: function () { return input; },
        capture: function () { claimed = true; },
        rect: function (x, y, w, h, color, filled) {
          ops.push({ kind: "rect", x: Number(x), y: Number(y), w: Number(w), h: Number(h),
                     c: parse_color(color), filled: filled !== false, t: 1 });
        },
        stroke: function (x, y, w, h, color, thickness) {
          ops.push({ kind: "rect", x: Number(x), y: Number(y), w: Number(w), h: Number(h),
                     c: parse_color(color), filled: false,
                     t: thickness === undefined ? 1 : Number(thickness) });
        },
        line: function (x1, y1, x2, y2, color, thickness) {
          ops.push({ kind: "line", x1: Number(x1), y1: Number(y1), x2: Number(x2), y2: Number(y2),
                     c: parse_color(color), t: thickness === undefined ? 1 : Number(thickness) });
        },
        text: function (x, y, text, color, size) {
          ops.push({ kind: "text", x: Number(x), y: Number(y), text: String(text),
                     c: parse_color(color), size: size === undefined ? 12 : Number(size) });
        }
      };
      try { if (viewports[v](vui) === false) { continue; } }
      catch (e) { __host_log("viewport threw: " + e); }
      __host_viewport(JSON.stringify(ops), claimed ? 1 : 0);
    }
  };
})();
"##;

/// One widget a panel recorded this frame.
#[derive(Clone, Debug, PartialEq)]
pub enum Widget {
    /// A section heading.
    Heading(String),
    /// A line of text.
    Label(String),
    /// A horizontal rule.
    Separator,
    /// A clickable button; `index` is the widget's position within the panel.
    Button {
        /// The button's index (its activation id).
        index: usize,
        /// The button's label.
        label: String,
    },
    /// A toggle, `index` is the widget's position within the panel.
    Checkbox {
        /// The checkbox's index (its value id).
        index: usize,
        /// The checkbox's label.
        label: String,
        /// Its checked state.
        checked: bool,
    },
    /// A numeric slider, `index` is the widget's position within the panel.
    Slider {
        /// The slider's index (its value id).
        index: usize,
        /// The slider's label.
        label: String,
        /// Its current value.
        value: f64,
        /// Its lower bound.
        min: f64,
        /// Its upper bound.
        max: f64,
    },
    /// A menu of choices, `index` is the widget's position within the panel.
    Select {
        /// The menu's index (its value id).
        index: usize,
        /// The menu's label.
        label: String,
        /// The chosen option's index.
        value: f64,
        /// The options, in order.
        options: Vec<String>,
    },
}

/// A panel an extension drew this frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Panel {
    /// A stable id: `"{extension_id}:{title}"`. It survives a reload and keys the
    /// panel's dock view.
    pub id: String,
    /// The panel title — its display name and hover tooltip.
    pub title: String,
    /// The widgets, in draw order.
    pub widgets: Vec<Widget>,
}

/// A discovered extension, for the Extensions screen.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtensionEntry {
    /// The extension id.
    pub id: String,
    /// Its human name.
    pub name: String,
    /// Its version.
    pub version: String,
    /// Whether it loaded (not disabled).
    pub enabled: bool,
}

/// What the extension UI reported for the previous frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Interactions {
    /// Buttons clicked, as `(panel, widget)`.
    pub activations: Vec<(usize, usize)>,
    /// Controls whose value changed, as `((panel, widget), value)`.
    pub values: Vec<((usize, usize), f64)>,
}

/// What the pointer did over the viewport this frame, in viewport pixels.
///
/// `x`/`y` are measured from the viewport rect's top-left corner and are the
/// pointer's last position whether or not it is inside; `inside` says whether it
/// is. `wheel` is the scroll delta this frame and `dx`/`dy` the movement while a
/// button is held. `clicked`/`right_clicked` are edge-triggered.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewportInput {
    /// The viewport's width, in pixels.
    pub width: f32,
    /// The viewport's height, in pixels.
    pub height: f32,
    /// The pointer's x, from the viewport's left edge.
    pub x: f32,
    /// The pointer's y, from the viewport's top edge.
    pub y: f32,
    /// Whether the pointer is over the viewport.
    pub inside: bool,
    /// Whether the primary button is held.
    pub down: bool,
    /// Whether the primary button was pressed this frame.
    pub clicked: bool,
    /// Whether the secondary button is held.
    pub right_down: bool,
    /// Whether the secondary button was pressed this frame.
    pub right_clicked: bool,
    /// The scroll wheel's delta this frame.
    pub wheel: f32,
    /// The pointer's x movement this frame.
    pub dx: f32,
    /// The pointer's y movement this frame.
    pub dy: f32,
    /// Whether Shift is held.
    pub shift: bool,
    /// Whether Ctrl (or Command) is held.
    pub ctrl: bool,
    /// Whether Alt is held.
    pub alt: bool,
}

/// One thing a viewport overlay paints, in viewport-pixel space (origin at the
/// viewport's top-left). Colours are straight 8-bit RGBA.
#[derive(Clone, Debug, PartialEq)]
pub enum ViewportOp {
    /// A rectangle, filled or outlined.
    Rect {
        /// Left edge.
        x: f32,
        /// Top edge.
        y: f32,
        /// Width.
        w: f32,
        /// Height.
        h: f32,
        /// Its colour.
        color: [u8; 4],
        /// Whether it is filled (otherwise its border is stroked).
        filled: bool,
        /// The border's width, when outlined.
        thickness: f32,
    },
    /// A line segment.
    Line {
        /// The start x.
        x1: f32,
        /// The start y.
        y1: f32,
        /// The end x.
        x2: f32,
        /// The end y.
        y2: f32,
        /// Its colour.
        color: [u8; 4],
        /// Its width.
        thickness: f32,
    },
    /// A line of text, anchored at its top-left corner.
    Text {
        /// The left edge.
        x: f32,
        /// The top edge.
        y: f32,
        /// The text.
        text: String,
        /// Its colour.
        color: [u8; 4],
        /// Its height, in points.
        size: f32,
    },
}

/// One frame's viewport overlay: what to paint, and whether an extension claimed
/// the pointer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewportFrame {
    /// Whether any extension registered a viewport overlay at all. The editor
    /// then leaves the viewport to the extension instead of drawing its hints.
    pub registered: bool,
    /// Whether an extension claimed the pointer this frame, so the editor skips
    /// its own camera navigation and click-to-select.
    pub capture: bool,
    /// The ops to paint, in draw order.
    pub ops: Vec<ViewportOp>,
}

/// One piece of work a callback queued, replayed after the call in order.
enum Queued {
    /// An `EditorCommand` batch (JSON text).
    Edit(String),
    /// A verb line to dispatch through the registry.
    Run(String),
}

/// The shared state the JS and WebAssembly primitives read and write.
///
/// The registered host functions are `'static` closures that cannot borrow the
/// [`Editor`], so — like the script backend — the host snapshots the reads it
/// exposes before a call and queues the work to replay after it.
#[derive(Default)]
struct Bridge {
    /// Queued edits and verbs, applied after the call in order.
    queued: Vec<Queued>,
    /// The read snapshot (scene, mode, entities, selection) as JSON.
    reads: String,
    /// Messages from `noctis.log`/`noctis.fail` and caught handler errors.
    logs: Vec<String>,
    /// The reply from the last dispatch (`None` means "no verb matched").
    reply: Option<String>,
    /// The panels drawn this frame.
    panels: Vec<Panel>,
    /// The id of the extension currently drawing; a panel's stable id is
    /// `"{owner}:{title}"`.
    owner: String,
    /// The (panel, widget) activations and value changes from the previous frame.
    interactions: Interactions,
    /// The global index of the context currently drawing; a context's panel-local
    /// `ui` ids are offset by it (see [`ExtensionHost::draw_panels`]).
    panel_base: usize,
    /// The viewport ops recorded this frame, in load order.
    viewport: Vec<ViewportOp>,
    /// Whether any context registered a viewport overlay.
    viewport_registered: bool,
    /// Whether a viewport claimed the pointer this frame.
    viewport_capture: bool,
    /// The pointer state handed to the viewport draws this frame.
    viewport_input: ViewportInput,
    /// Declared project files: `(extension id, slot) -> project-relative path`.
    file_slots: BTreeMap<(String, String), String>,
    /// The open project's store, for the `noctis.files` channel. `None` headless
    /// or without a project, in which case a read is `undefined` and a write is
    /// reported as failed.
    store: Option<Box<dyn Store>>,
}

/// One extension's JavaScript realm.
struct Extension {
    /// The extension id (for errors and reload).
    id: String,
    /// Its Slag context — its own `noctis` global, verbs, hooks and panels.
    context: slag::Context,
}

/// The JavaScript host for editor extensions: one [`Extension`] context each.
pub struct ExtensionHost {
    extensions: Vec<Extension>,
    bridge: Rc<RefCell<Bridge>>,
    wasm: WasmHost,
    ids: Vec<String>,
    entries: Vec<ExtensionEntry>,
    errors: Vec<String>,
}

impl std::fmt::Debug for ExtensionHost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExtensionHost")
            .field("extensions", &self.extensions.len())
            .field("wasm", &self.wasm)
            .field("errors", &self.errors)
            .finish_non_exhaustive()
    }
}

impl ExtensionHost {
    /// Create the host. Each extension gets its own context when it is loaded.
    pub fn new() -> Result<Self, String> {
        let bridge = Rc::new(RefCell::new(Bridge::default()));
        let wasm = WasmHost::new(Rc::clone(&bridge));
        Ok(Self {
            extensions: Vec::new(),
            bridge,
            wasm,
            ids: Vec::new(),
            entries: Vec::new(),
            errors: Vec::new(),
        })
    }

    /// The ids of the extensions that were loaded.
    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    /// Every discovered extension, with its enablement — for the Extensions screen.
    pub fn entries(&self) -> &[ExtensionEntry] {
        &self.entries
    }

    /// Failures from loading or running extensions.
    pub fn errors(&self) -> &[String] {
        &self.errors
    }

    /// Take the pending log lines.
    pub fn take_logs(&mut self) -> Vec<String> {
        std::mem::take(&mut self.bridge.borrow_mut().logs)
    }

    /// Give the host the open project's store — the channel `noctis.files` reads
    /// and writes through.
    ///
    /// The store is a second handle to the same project data
    /// ([`Store::duplicate`]), so a write here is visible to the editor's own
    /// reads. Without a project the channel stays closed: a read is `undefined`
    /// and a write is reported as failed.
    pub fn set_project(&mut self, store: Box<dyn Store>) {
        self.bridge.borrow_mut().store = Some(store);
    }

    /// Load every extension in `loader` for which `enabled` is true.
    ///
    /// Each JavaScript entry gets its own context, evaluated inside a private
    /// wrapper; a failure is recorded against its id and the others still load.
    pub fn load(&mut self, loader: &Loader, enabled: impl Fn(&str) -> bool) {
        self.entries = loader
            .extensions()
            .iter()
            .map(|manifest| ExtensionEntry {
                id: manifest.id.clone(),
                name: manifest.name.clone(),
                version: manifest.version.clone(),
                enabled: enabled(&manifest.id),
            })
            .collect();
        let manifests: Vec<&Manifest> = loader
            .extensions()
            .iter()
            .filter(|manifest| enabled(&manifest.id))
            .collect();
        self.ids = manifests
            .iter()
            .map(|manifest| manifest.id.clone())
            .collect();

        // Declare the project files the enabled extensions name. Reading and
        // writing them happens later, through the store `set_project` hands us.
        {
            let mut bridge = self.bridge.borrow_mut();
            for manifest in &manifests {
                for file in &manifest.files {
                    bridge
                        .file_slots
                        .insert((manifest.id.clone(), file.slot.clone()), file.path.clone());
                }
            }
        }

        for manifest in &manifests {
            if let Some(source) = &manifest.entry_source
                && let Err(error) = self.load_javascript(manifest, source)
            {
                self.errors.push(format!("{}: {error}", manifest.id));
            }
            if let Some(module) = &manifest.wasm
                && let Err(error) = self.wasm.load(&manifest.id, &module.bytes)
            {
                self.errors.push(format!("{}: {error}", manifest.id));
            }
        }
    }

    /// Build a context for `manifest`, evaluate its entry, and freeze it.
    fn load_javascript(&mut self, manifest: &Manifest, source: &str) -> Result<(), String> {
        let info = serde_json::json!({
            "id": manifest.id,
            "name": manifest.name,
            "version": manifest.version,
        })
        .to_string();
        let mut context = slag::Context::new().map_err(|error| error.to_string())?;
        install(&mut context, Rc::clone(&self.bridge))?;
        context.eval(BOOTSTRAP).map_err(|error| error.to_string())?;
        // The bootstrap's own recorder shapes, before the extension's code (and its
        // allocations) can promote the shared shape map.
        context.eval(WARMUP).map_err(|error| error.to_string())?;
        let register = format!(
            "__noctis_register_extension({}, {});",
            json_literal(&manifest.id),
            json_literal(&info)
        );
        context.eval(&register).map_err(|error| error.to_string())?;
        let program = format!(
            "(function (noctis) {{\n\"use strict\";\n{source}\n}})(noctis.begin({}));",
            json_literal(&manifest.id)
        );
        context.eval(&program).map_err(|error| error.to_string())?;
        context
            .eval("noctis.freeze();")
            .map_err(|error| error.to_string())?;
        self.extensions.push(Extension {
            id: manifest.id.clone(),
            context,
        });
        Ok(())
    }

    /// Fire the `"ready"` hook on every context and apply any edits it queued.
    pub fn ready(&mut self, editor: &mut Editor) -> Vec<String> {
        self.snapshot(editor);
        for index in 0..self.extensions.len() {
            if let Err(error) = self.extensions[index]
                .context
                .eval("__noctis_fire(\"ready\", undefined);")
            {
                let id = self.extensions[index].id.clone();
                self.errors.push(format!("{id}: ready: {error}"));
            }
        }
        self.apply_queued(editor)
    }

    /// Dispatch a verb line, applying any edits the handler queued.
    ///
    /// Returns the reply, or an error when no extension verb matched.
    pub fn dispatch(&mut self, editor: &mut Editor, line: &str) -> Result<String, String> {
        let result = self.dispatch_line(editor, line);
        self.apply_queued(editor);
        result
    }

    /// Dispatch a verb line, queueing — but not applying — the work it makes.
    ///
    /// The built-ins (`help`, `extensions`) are host-level; anything else is
    /// offered to each context in load order, and the first match wins.
    fn dispatch_line(&mut self, editor: &mut Editor, line: &str) -> Result<String, String> {
        self.snapshot(editor);
        let verb = line.split_whitespace().next().unwrap_or("");
        if verb.is_empty() || verb == "help" {
            return Ok(format!("ok verbs: {}", self.verb_names().join(", ")));
        }
        if verb == "extensions" {
            return Ok(format!("ok extensions: {}", self.ids.join(", ")));
        }
        for index in 0..self.extensions.len() {
            self.bridge.borrow_mut().reply = None;
            let program = format!("__noctis_dispatch_line({});", json_literal(line));
            if let Err(error) = self.extensions[index].context.eval(&program) {
                let id = self.extensions[index].id.clone();
                self.errors.push(format!("{id}: dispatch: {error}"));
                continue;
            }
            if let Some(reply) = self.bridge.borrow_mut().reply.take() {
                return Ok(reply);
            }
        }
        Err(format!("no extension verb matched {line:?}"))
    }

    /// Every verb registered by any context, sorted and deduplicated.
    fn verb_names(&mut self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for index in 0..self.extensions.len() {
            if let Ok(value) = self.extensions[index].context.eval("__noctis_verb_names()")
                && let Some(text) = value.as_string()
                && let Ok(mut list) = serde_json::from_str::<Vec<String>>(&text)
            {
                names.append(&mut list);
            }
        }
        names.sort();
        names.dedup();
        names
    }

    /// Fire the `"update"` hook on every context and apply any edits it queued.
    pub fn update(&mut self, editor: &mut Editor, dt: f32) -> Vec<String> {
        self.snapshot(editor);
        for index in 0..self.extensions.len() {
            let program = format!("__noctis_fire(\"update\", {dt});");
            if let Err(error) = self.extensions[index].context.eval(&program) {
                let id = self.extensions[index].id.clone();
                self.errors.push(format!("{id}: update: {error}"));
            }
        }
        self.wasm.update(dt);
        for error in self.wasm.take_errors() {
            self.errors.push(error);
        }
        self.apply_queued(editor)
    }

    /// Draw the registered panels, using the interactions from the previous frame.
    pub fn draw_panels(&mut self, editor: &Editor, interactions: &Interactions) -> Vec<Panel> {
        self.snapshot(editor);
        {
            let mut bridge = self.bridge.borrow_mut();
            bridge.panels.clear();
            bridge.interactions = interactions.clone();
        }
        // Each context draws its own panels; the host offsets their panel-local
        // ids into the frame's global array so a click maps back to its source.
        for index in 0..self.extensions.len() {
            let owner = self.extensions[index].id.clone();
            let base = self.bridge.borrow().panels.len();
            {
                let mut bridge = self.bridge.borrow_mut();
                bridge.panel_base = base;
                bridge.owner = owner;
            }
            if let Err(error) = self.extensions[index]
                .context
                .eval("__noctis_draw_panels();")
            {
                let id = self.extensions[index].id.clone();
                self.errors.push(format!("{id}: panels: {error}"));
            }
        }
        // WebAssembly panels follow the JavaScript ones the same way.
        self.wasm.draw();
        for error in self.wasm.take_errors() {
            self.errors.push(error);
        }
        std::mem::take(&mut self.bridge.borrow_mut().panels)
    }

    /// Apply any work a panel draw (or `noctis.run`) queued.
    pub fn apply_pending(&mut self, editor: &mut Editor) -> Vec<String> {
        self.apply_queued(editor)
    }

    /// Draw the viewport overlays for this frame with `input`, and return the ops.
    ///
    /// Like [`ExtensionHost::draw_panels`], this runs each context's registered
    /// draw once per frame. A context that registered no viewport records nothing,
    /// so `registered` stays false and the editor keeps the viewport to itself.
    pub fn draw_viewport(&mut self, editor: &Editor, input: ViewportInput) -> ViewportFrame {
        self.snapshot(editor);
        {
            let mut bridge = self.bridge.borrow_mut();
            bridge.viewport.clear();
            bridge.viewport_registered = false;
            bridge.viewport_capture = false;
            bridge.viewport_input = input;
        }
        for index in 0..self.extensions.len() {
            if let Err(error) = self.extensions[index]
                .context
                .eval("__noctis_draw_viewport();")
            {
                let id = self.extensions[index].id.clone();
                self.errors.push(format!("{id}: viewport: {error}"));
            }
        }
        let mut bridge = self.bridge.borrow_mut();
        ViewportFrame {
            registered: bridge.viewport_registered,
            capture: bridge.viewport_capture,
            ops: std::mem::take(&mut bridge.viewport),
        }
    }

    /// Copy the editor reads the API exposes into the bridge.
    fn snapshot(&self, editor: &Editor) {
        let entities = editor.list_entities();
        let selection: Vec<EntityDto> = editor
            .selection()
            .iter()
            .copied()
            .map(EntityDto::new)
            .collect();
        let mode = match editor.mode() {
            SceneMode::TwoD => "2d",
            SceneMode::ThreeD => "3d",
        };
        let value = serde_json::json!({
            "scene": editor.scene_name(),
            "mode": mode,
            "entities": entities,
            "selection": selection,
        });
        self.bridge.borrow_mut().reads = value.to_string();
    }

    /// Apply the queued work as one undo step, in the order it was queued.
    ///
    /// Dispatching a queued verb can queue more work (a verb that edits or runs
    /// another verb), so the queue is drained until empty. Returns a line per edit
    /// batch.
    fn apply_queued(&mut self, editor: &mut Editor) -> Vec<String> {
        if self.bridge.borrow().queued.is_empty() {
            return Vec::new();
        }
        let mut results = Vec::new();
        editor.begin_interactive_edit();
        loop {
            let queued = std::mem::take(&mut self.bridge.borrow_mut().queued);
            if queued.is_empty() {
                break;
            }
            for work in queued {
                match work {
                    Queued::Edit(json) => match editor.apply_json(&json) {
                        Ok(outcomes) => results.push(format!("ok {} edit(s)", outcomes.len())),
                        Err(error) => {
                            results.push(format!("error: {error}"));
                            self.errors.push(format!("edit failed: {error}"));
                        }
                    },
                    Queued::Run(line) => {
                        if let Err(error) = self.dispatch_line(editor, &line) {
                            results.push(format!("error: {error}"));
                        }
                    }
                }
            }
        }
        editor.end_interactive_edit();
        results
    }
}

/// Install the host primitives on a freshly created context, all sharing one
/// [`Bridge`] so the queued edits, replies, panels and interactions are common.
fn install(context: &mut slag::Context, bridge: Rc<RefCell<Bridge>>) -> Result<(), String> {
    let b = Rc::clone(&bridge);
    register(
        context,
        "__host_edit",
        1,
        Box::new(move |call| {
            let json = arg_string(call, 0).unwrap_or_default();
            let count = {
                let mut bridge = b.borrow_mut();
                bridge.queued.push(Queued::Edit(json));
                bridge.queued.len()
            };
            Ok(slag::JsValue::number(count as f64))
        }),
    )?;

    let b = Rc::clone(&bridge);
    register(
        context,
        "__host_reads",
        0,
        Box::new(move |_call| Ok(slag::JsValue::string(b.borrow().reads.clone()))),
    )?;

    let b = Rc::clone(&bridge);
    register(
        context,
        "__host_log",
        1,
        Box::new(move |call| {
            let message = arg_string(call, 0).unwrap_or_default();
            b.borrow_mut().logs.push(message);
            Ok(slag::JsValue::undefined())
        }),
    )?;

    let b = Rc::clone(&bridge);
    register(
        context,
        "__host_reply",
        2,
        Box::new(move |call| {
            let ok = arg_number(call, 0).unwrap_or(0.0);
            let text = arg_string(call, 1).unwrap_or_default();
            b.borrow_mut().reply = if ok != 0.0 { Some(text) } else { None };
            Ok(slag::JsValue::undefined())
        }),
    )?;

    let b = Rc::clone(&bridge);
    register(
        context,
        "__host_panel",
        2,
        Box::new(move |call| {
            let title = arg_string(call, 0).unwrap_or_default();
            let ops = arg_string(call, 1).unwrap_or_default();
            let mut bridge = b.borrow_mut();
            let id = format!("{}:{title}", bridge.owner);
            bridge.panels.push(Panel {
                id,
                title,
                widgets: parse_ops(&ops),
            });
            Ok(slag::JsValue::undefined())
        }),
    )?;

    let b = Rc::clone(&bridge);
    register(
        context,
        "__host_activated",
        2,
        Box::new(move |call| {
            let panel = arg_number(call, 0).unwrap_or(0.0) as usize;
            let widget = arg_number(call, 1).unwrap_or(0.0) as usize;
            let hit = {
                let bridge = b.borrow();
                bridge
                    .interactions
                    .activations
                    .contains(&(bridge.panel_base + panel, widget))
            };
            Ok(slag::JsValue::number(if hit { 1.0 } else { 0.0 }))
        }),
    )?;

    let b = Rc::clone(&bridge);
    register(
        context,
        "__host_value",
        2,
        Box::new(move |call| {
            let panel = arg_number(call, 0).unwrap_or(0.0) as usize;
            let widget = arg_number(call, 1).unwrap_or(0.0) as usize;
            let value = {
                let bridge = b.borrow();
                bridge
                    .interactions
                    .values
                    .iter()
                    .find(|(key, _)| *key == (bridge.panel_base + panel, widget))
                    .map_or(f64::NAN, |(_, value)| *value)
            };
            Ok(slag::JsValue::number(value))
        }),
    )?;

    let b = Rc::clone(&bridge);
    register(
        context,
        "__host_viewport_input",
        0,
        Box::new(move |_call| {
            let input = b.borrow().viewport_input;
            let value = serde_json::json!({
                "width": input.width,
                "height": input.height,
                "x": input.x,
                "y": input.y,
                "inside": input.inside,
                "down": input.down,
                "clicked": input.clicked,
                "right_down": input.right_down,
                "right_clicked": input.right_clicked,
                "wheel": input.wheel,
                "dx": input.dx,
                "dy": input.dy,
                "shift": input.shift,
                "ctrl": input.ctrl,
                "alt": input.alt,
            });
            Ok(slag::JsValue::string(value.to_string()))
        }),
    )?;

    let b = Rc::clone(&bridge);
    register(
        context,
        "__host_viewport",
        2,
        Box::new(move |call| {
            let ops = arg_string(call, 0).unwrap_or_default();
            let claimed = arg_number(call, 1).unwrap_or(0.0) != 0.0;
            let mut bridge = b.borrow_mut();
            bridge.viewport_registered = true;
            bridge.viewport_capture |= claimed;
            bridge.viewport.extend(parse_viewport_ops(&ops));
            Ok(slag::JsValue::undefined())
        }),
    )?;

    let b = Rc::clone(&bridge);
    register(
        context,
        "__host_file_read",
        2,
        Box::new(move |call| {
            let id = arg_string(call, 0).unwrap_or_default();
            let slot = arg_string(call, 1).unwrap_or_default();
            let result = {
                let bridge = b.borrow();
                match bridge
                    .file_slots
                    .get(&(id.clone(), slot.clone()))
                    .zip(bridge.store.as_ref())
                {
                    Some((path, store)) => match store.read_capped(path, MAX_ASSET_BYTES) {
                        Ok(bytes) => match String::from_utf8(bytes) {
                            Ok(text) => Ok(text),
                            Err(_) => Err(format!("file {id}:{slot} is not UTF-8")),
                        },
                        Err(error) => Err(format!("file {id}:{slot} ({path}): {error}")),
                    },
                    None => return Ok(slag::JsValue::undefined()),
                }
            };
            match result {
                Ok(text) => Ok(slag::JsValue::string(text)),
                Err(message) => {
                    b.borrow_mut().logs.push(message);
                    Ok(slag::JsValue::undefined())
                }
            }
        }),
    )?;

    let b = Rc::clone(&bridge);
    register(
        context,
        "__host_file_write",
        3,
        Box::new(move |call| {
            let id = arg_string(call, 0).unwrap_or_default();
            let slot = arg_string(call, 1).unwrap_or_default();
            let text = arg_string(call, 2).unwrap_or_default();
            let path = {
                b.borrow()
                    .file_slots
                    .get(&(id.clone(), slot.clone()))
                    .cloned()
            };
            let Some(path) = path else {
                b.borrow_mut()
                    .logs
                    .push(format!("file {id}:{slot}: no such slot"));
                return Ok(slag::JsValue::number(0.0));
            };
            let written = {
                let mut bridge = b.borrow_mut();
                match bridge.store.as_mut() {
                    Some(store) => store.write(&path, text.as_bytes()).is_ok(),
                    None => false,
                }
            };
            if !written {
                b.borrow_mut().logs.push(format!(
                    "file {id}:{slot} ({path}): no open project to write to"
                ));
            }
            Ok(slag::JsValue::number(if written { 1.0 } else { 0.0 }))
        }),
    )?;

    let b = Rc::clone(&bridge);
    register(
        context,
        "__host_file_slots",
        1,
        Box::new(move |call| {
            let id = arg_string(call, 0).unwrap_or_default();
            let slots: Vec<String> = b
                .borrow()
                .file_slots
                .keys()
                .filter(|(owner, _)| owner == &id)
                .map(|(_, slot)| slot.clone())
                .collect();
            let text = serde_json::to_string(&slots).unwrap_or_else(|_| "[]".into());
            Ok(slag::JsValue::string(text))
        }),
    )?;

    Ok(())
}

/// Build one of every object shape the bootstrap's panel and viewport recorders
/// evaluate, once, at load.
///
/// Slag caches an object literal's whole shape on the shared canonical empty map
/// and that insert (`crux::map::Map::set_composite_child`) takes no write barrier,
/// so a shape first built *after* a minor collection leaves an unrecorded
/// old→young edge — which Slag's own verifier (on in a debug build) reports as a
/// "write-barrier miss". A panel is drawn and a viewport is painted every frame,
/// and the op objects they record are built lazily on the first draw, so without
/// this a long-running editor panics on whichever op it built first afterwards.
/// Warming them here, while the heap is still young, leaves the cache complete.
/// The real fix is one line in Slag — a `write_barrier_handle(self, child)` call
/// in `set_composite_child`; see `.notes/extensions.md`.
const WARMUP: &str = r##"
(function () {
  "use strict";
  // The recording handles the host passes to a `panel`/`viewport` draw, and one
  // of every op each of them can record, with the same key order the bootstrap
  // builds them in.
  var ui = {
    text: null, label: null, heading: null, separator: null,
    button: null, checkbox: null, slider: null, select: null
  };
  var vui = {
    size: null, pointer: null, capture: null,
    rect: null, stroke: null, line: null, text: null
  };
  return [
    { kind: "label", text: "" },
    { kind: "heading", text: "" },
    { kind: "separator" },
    { kind: "button", id: 0, label: "" },
    { kind: "checkbox", id: 0, label: "", value: 0 },
    { kind: "slider", id: 0, label: "", value: 0, min: 0, max: 0 },
    { kind: "select", id: 0, label: "", value: 0, options: null },
    { kind: "rect", x: 0, y: 0, w: 0, h: 0, c: null, filled: true, t: 1 },
    { kind: "line", x1: 0, y1: 0, x2: 0, y2: 0, c: null, t: 1 },
    { kind: "text", x: 0, y: 0, text: "", c: null, size: 0 },
    { width: 0, height: 0 },
    { title: "", draw: null },
    ui,
    vui
  ];
})();
"##;

/// Register one host function on `context`.
fn register(
    context: &mut slag::Context,
    name: &str,
    arity: u32,
    f: slag::HostFn,
) -> Result<(), String> {
    context
        .register_fn(name, arity, f)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// A JSON string literal (a valid JS string literal).
fn json_literal(text: &str) -> String {
    serde_json::to_string(text).expect("a string serializes")
}

/// Parse the ops a panel's draw recorded into [`Widget`]s.
fn parse_ops(json: &str) -> Vec<Widget> {
    let Ok(serde_json::Value::Array(ops)) = serde_json::from_str(json) else {
        return Vec::new();
    };
    ops.iter()
        .filter_map(|op| match op.get("kind")?.as_str()? {
            "heading" => Some(Widget::Heading(op.get("text")?.as_str()?.to_string())),
            "label" => Some(Widget::Label(op.get("text")?.as_str()?.to_string())),
            "separator" => Some(Widget::Separator),
            "button" => Some(Widget::Button {
                index: op.get("id")?.as_u64()? as usize,
                label: op.get("label")?.as_str()?.to_string(),
            }),
            "checkbox" => Some(Widget::Checkbox {
                index: op.get("id")?.as_u64()? as usize,
                label: op.get("label")?.as_str()?.to_string(),
                checked: op.get("value")?.as_f64()? != 0.0,
            }),
            "slider" => Some(Widget::Slider {
                index: op.get("id")?.as_u64()? as usize,
                label: op.get("label")?.as_str()?.to_string(),
                value: op.get("value")?.as_f64()?,
                min: op.get("min")?.as_f64()?,
                max: op.get("max")?.as_f64()?,
            }),
            "select" => Some(Widget::Select {
                index: op.get("id")?.as_u64()? as usize,
                label: op.get("label")?.as_str()?.to_string(),
                value: op.get("value")?.as_f64()?,
                options: op
                    .get("options")?
                    .as_array()?
                    .iter()
                    .filter_map(|option| option.as_str().map(str::to_string))
                    .collect(),
            }),
            _ => None,
        })
        .collect()
}

/// Parse the ops a viewport overlay recorded into [`ViewportOp`]s.
fn parse_viewport_ops(json: &str) -> Vec<ViewportOp> {
    let Ok(serde_json::Value::Array(ops)) = serde_json::from_str(json) else {
        return Vec::new();
    };
    ops.iter()
        .filter_map(|op| {
            let color = parse_color(op.get("c")?)?;
            match op.get("kind")?.as_str()? {
                "rect" => Some(ViewportOp::Rect {
                    x: op.get("x")?.as_f64()? as f32,
                    y: op.get("y")?.as_f64()? as f32,
                    w: op.get("w")?.as_f64()? as f32,
                    h: op.get("h")?.as_f64()? as f32,
                    color,
                    filled: op.get("filled")?.as_bool()?,
                    thickness: op.get("t")?.as_f64()? as f32,
                }),
                "line" => Some(ViewportOp::Line {
                    x1: op.get("x1")?.as_f64()? as f32,
                    y1: op.get("y1")?.as_f64()? as f32,
                    x2: op.get("x2")?.as_f64()? as f32,
                    y2: op.get("y2")?.as_f64()? as f32,
                    color,
                    thickness: op.get("t")?.as_f64()? as f32,
                }),
                "text" => Some(ViewportOp::Text {
                    x: op.get("x")?.as_f64()? as f32,
                    y: op.get("y")?.as_f64()? as f32,
                    text: op.get("text")?.as_str()?.to_string(),
                    color,
                    size: op.get("size")?.as_f64()? as f32,
                }),
                _ => None,
            }
        })
        .collect()
}

/// A `[r, g, b, a]` array of 0-255 numbers, as the bootstrap normalizes a colour.
fn parse_color(value: &serde_json::Value) -> Option<[u8; 4]> {
    let list = value.as_array()?;
    if list.len() != 4 {
        return None;
    }
    let mut color = [255u8; 4];
    for (index, channel) in list.iter().enumerate() {
        color[index] = channel.as_f64()?.clamp(0.0, 255.0) as u8;
    }
    Some(color)
}

fn arg_string(call: &slag::FunctionCall<'_>, index: usize) -> Option<String> {
    let value = call.arg(index)?;
    call.to_string(&value).ok()
}

fn arg_number(call: &slag::FunctionCall<'_>, index: usize) -> Option<f64> {
    call.arg(index).and_then(|value| value.as_number())
}

#[cfg(test)]
mod tests {
    use super::*;
    use extensions::Loader;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("noctis-editor-ext-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn write_extension(root: &std::path::Path, dir_name: &str, id: &str, js: &str) {
        let dir = root.join(dir_name);
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(
            dir.join("extension.json"),
            format!(
                r#"{{ "id": "{id}", "name": "{id}", "version": "1.0.0", "api": 1, "entry": "extension.js" }}"#
            ),
        )
        .expect("manifest");
        std::fs::write(dir.join("extension.js"), js).expect("entry");
    }

    #[test]
    fn a_command_spawns_through_the_editor() {
        let root = temp_dir("spawn");
        write_extension(
            &root,
            "spawner",
            "com.example.spawner",
            r#"
            noctis.command("box", function (parts) {
              var name = parts[1] || "Box";
              noctis.edit({ op: "spawn", name: name });
              return "ok box " + name;
            });
            "#,
        );
        let loader = Loader::discover(std::slice::from_ref(&root));

        let mut host = ExtensionHost::new().expect("host");
        host.load(&loader, |_| true);
        let mut editor = Editor::new();

        let reply = host.dispatch(&mut editor, "box Thing").expect("dispatch");
        assert_eq!(reply, "ok box Thing");
        assert!(editor.find_by_name("Thing").is_some());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_batch_is_one_undo_step() {
        let root = temp_dir("batch");
        write_extension(
            &root,
            "grid",
            "com.example.grid",
            r#"
            noctis.command("grid", function () {
              var batch = [];
              for (var i = 0; i < 3; i++) { batch.push({ op: "spawn", name: "Grid " + i }); }
              return "ok " + noctis.edit(batch);
            });
            "#,
        );
        let loader = Loader::discover(std::slice::from_ref(&root));
        let mut host = ExtensionHost::new().unwrap();
        host.load(&loader, |_| true);
        let mut editor = Editor::new();

        host.dispatch(&mut editor, "grid").unwrap();
        assert_eq!(editor.list_entities().len(), 3);
        assert!(editor.can_undo());
        editor.undo();
        assert_eq!(
            editor.list_entities().len(),
            0,
            "one undo removes the batch"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reads_are_exposed_to_the_extension() {
        let root = temp_dir("reads");
        write_extension(
            &root,
            "reader",
            "com.example.reader",
            r#"
            noctis.command("who", function () {
              var list = JSON.parse(noctis.editor.list_entities());
              var names = list.map(function (e) { return e.name; }).join(",");
              var sel = noctis.editor.selection();
              return "ok " + noctis.editor.mode() + " " + names +
                " sel=" + Array.isArray(sel) + ":" + sel.length;
            });
            "#,
        );
        let loader = Loader::discover(std::slice::from_ref(&root));
        let mut host = ExtensionHost::new().unwrap();
        host.load(&loader, |_| true);
        let mut editor = Editor::new();
        let sun = editor.spawn("Sun", None);
        editor.select(sun).unwrap();

        let reply = host.dispatch(&mut editor, "who").unwrap();
        assert_eq!(reply, "ok 3d Sun sel=true:1");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_disabled_extension_is_not_loaded() {
        let root = temp_dir("disabled");
        write_extension(
            &root,
            "loud",
            "com.example.loud",
            r#"noctis.command("loud", function () { return "ok loud"; });"#,
        );
        let loader = Loader::discover(std::slice::from_ref(&root));
        let mut host = ExtensionHost::new().unwrap();
        host.load(&loader, |_| false);
        assert!(host.ids().is_empty());
        let mut editor = Editor::new();
        assert!(host.dispatch(&mut editor, "loud").is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_help_verb_lists_extension_verbs() {
        let root = temp_dir("help");
        write_extension(
            &root,
            "a",
            "com.example.a",
            r#"noctis.command("alpha", function () { return "ok"; });"#,
        );
        let loader = Loader::discover(std::slice::from_ref(&root));
        let mut host = ExtensionHost::new().unwrap();
        host.load(&loader, |_| true);
        let mut editor = Editor::new();
        let reply = host.dispatch(&mut editor, "help").unwrap();
        assert!(reply.contains("alpha"), "{reply}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_built_in_verb_cannot_be_shadowed() {
        let root = temp_dir("shadow");
        write_extension(
            &root,
            "sneaky",
            "com.example.sneaky",
            r#"noctis.command("help", function () { return "hijacked"; });"#,
        );
        let loader = Loader::discover(std::slice::from_ref(&root));
        let mut host = ExtensionHost::new().unwrap();
        host.load(&loader, |_| true);
        let mut editor = Editor::new();
        let reply = host.dispatch(&mut editor, "help").unwrap();
        assert!(!reply.contains("hijacked"), "{reply}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_panel_records_widgets_and_its_button_activates() {
        let root = temp_dir("panel");
        write_extension(
            &root,
            "panel",
            "com.example.panel",
            r#"
            var clicks = 0;
            noctis.ui.panel("Tools", function (ui) {
              ui.heading("Tools");
              ui.label("clicks: " + clicks);
              ui.separator();
              if (ui.button("Bump")) { clicks++; noctis.edit({ op: "spawn", name: "Bumped" }); }
            });
            "#,
        );
        let loader = Loader::discover(std::slice::from_ref(&root));
        let mut host = ExtensionHost::new().unwrap();
        host.load(&loader, |_| true);
        let mut editor = Editor::new();

        // The first draw records the widgets; nothing is activated yet.
        let panels = host.draw_panels(&editor, &Interactions::default());
        assert_eq!(panels.len(), 1);
        assert_eq!(panels[0].title, "Tools");
        assert!(matches!(panels[0].widgets[0], Widget::Heading(_)));
        assert!(
            panels[0]
                .widgets
                .iter()
                .any(|widget| matches!(widget, Widget::Separator))
        );
        assert!(
            panels[0]
                .widgets
                .iter()
                .any(|widget| matches!(widget, Widget::Button { index: 0, .. }))
        );
        assert!(editor.find_by_name("Bumped").is_none());

        // Clicking widget 0 activates the button on the next draw.
        host.draw_panels(
            &editor,
            &Interactions {
                activations: vec![(0, 0)],
                values: Vec::new(),
            },
        );
        host.apply_pending(&mut editor);
        assert!(editor.find_by_name("Bumped").is_some());

        // The label is drawn before the button, so it reflects the click on the
        // draw after the activation.
        let panels = host.draw_panels(&editor, &Interactions::default());
        assert!(
            panels[0]
                .widgets
                .iter()
                .any(|widget| matches!(widget, Widget::Label(text) if text.contains("clicks: 1")))
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_update_hook_runs_and_edits() {
        let root = temp_dir("update");
        write_extension(
            &root,
            "tick",
            "com.example.tick",
            r#"
            var t = 0;
            noctis.on("update", function (dt) {
              t += dt;
              if (t >= 0.5) { t = 0; noctis.edit({ op: "spawn", name: "Tick" }); }
            });
            "#,
        );
        let loader = Loader::discover(std::slice::from_ref(&root));
        let mut host = ExtensionHost::new().unwrap();
        host.load(&loader, |_| true);
        let mut editor = Editor::new();

        host.update(&mut editor, 0.6);
        assert_eq!(editor.list_entities().len(), 1);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_panel_round_trips_checkbox_and_slider_values() {
        let root = temp_dir("values");
        write_extension(
            &root,
            "values",
            "com.example.values",
            r#"
            noctis.ui.panel("Tune", function (ui) {
              var on = ui.checkbox("Enable", false);
              var gain = ui.slider("Gain", 0, 0, 10);
              var track = ui.select("Track", 1, ["one", "two", "three"]);
              ui.label("enable " + on + " gain " + gain + " track " + track);
            });
            "#,
        );
        let loader = Loader::discover(std::slice::from_ref(&root));
        let mut host = ExtensionHost::new().unwrap();
        host.load(&loader, |_| true);
        let editor = Editor::new();

        // No interactions yet: the widgets carry the defaults the panel passed.
        let panels = host.draw_panels(&editor, &Interactions::default());
        assert_eq!(
            panels[0].widgets[0],
            Widget::Checkbox {
                index: 0,
                label: "Enable".into(),
                checked: false,
            }
        );
        assert_eq!(
            panels[0].widgets[1],
            Widget::Slider {
                index: 1,
                label: "Gain".into(),
                value: 0.0,
                min: 0.0,
                max: 10.0,
            }
        );
        assert_eq!(
            panels[0].widgets[2],
            Widget::Select {
                index: 2,
                label: "Track".into(),
                value: 1.0,
                options: vec!["one".into(), "two".into(), "three".into()],
            }
        );

        // A change fed back on the next draw is what the panel reads and draws.
        let panels = host.draw_panels(
            &editor,
            &Interactions {
                activations: Vec::new(),
                values: vec![((0, 0), 1.0), ((0, 1), 5.0), ((0, 2), 2.0)],
            },
        );
        assert_eq!(
            panels[0].widgets[0],
            Widget::Checkbox {
                index: 0,
                label: "Enable".into(),
                checked: true,
            }
        );
        assert_eq!(
            panels[0].widgets[1],
            Widget::Slider {
                index: 1,
                label: "Gain".into(),
                value: 5.0,
                min: 0.0,
                max: 10.0,
            }
        );
        assert_eq!(
            panels[0].widgets[2],
            Widget::Select {
                index: 2,
                label: "Track".into(),
                value: 2.0,
                options: vec!["one".into(), "two".into(), "three".into()],
            }
        );
        assert!(
            panels[0].widgets.iter().any(
                |widget| matches!(widget, Widget::Label(text) if text == "enable true gain 5 track 2")
            ),
            "{:?}",
            panels[0].widgets
        );

        // A menu always names one of its options: an index fed back out of range
        // (a shorter list than the one that produced it) falls back to the first.
        let panels = host.draw_panels(
            &editor,
            &Interactions {
                activations: Vec::new(),
                values: vec![((0, 2), 9.0)],
            },
        );
        assert_eq!(
            panels[0].widgets[2],
            Widget::Select {
                index: 2,
                label: "Track".into(),
                value: 0.0,
                options: vec!["one".into(), "two".into(), "three".into()],
            }
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_entries_table_carries_enablement() {
        let root = temp_dir("entries");
        write_extension(&root, "a", "com.example.a", "");
        write_extension(&root, "b", "com.example.b", "");
        let loader = Loader::discover(std::slice::from_ref(&root));
        let mut host = ExtensionHost::new().unwrap();
        host.load(&loader, |id| id != "com.example.b");
        assert_eq!(host.entries().len(), 2);
        assert!(host.entries()[0].enabled);
        assert!(!host.entries()[1].enabled);
        std::fs::remove_dir_all(&root).ok();
    }

    // -----------------------------------------------------------------------
    // WebAssembly extensions. The fixtures are built with a tiny inline encoder
    // so the tests need no toolchain and no binary blobs in the tree.
    // -----------------------------------------------------------------------

    const I32: u8 = 0x7f;
    const F32: u8 = 0x7d;
    const F64: u8 = 0x7c;

    fn leb(mut value: u32, out: &mut Vec<u8>) {
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }

    fn section(id: u8, content: &[u8], out: &mut Vec<u8>) {
        out.push(id);
        leb(content.len() as u32, out);
        out.extend_from_slice(content);
    }

    fn wasm_name(text: &str, out: &mut Vec<u8>) {
        leb(text.len() as u32, out);
        out.extend_from_slice(text.as_bytes());
    }

    fn func_type(params: &[u8], results: &[u8], out: &mut Vec<u8>) {
        out.push(0x60);
        leb(params.len() as u32, out);
        out.extend_from_slice(params);
        leb(results.len() as u32, out);
        out.extend_from_slice(results);
    }

    fn import(module: &str, field: &str, type_index: u32, out: &mut Vec<u8>) {
        wasm_name(module, out);
        wasm_name(field, out);
        out.push(0x00); // import kind: function
        leb(type_index, out);
    }

    fn wasm_header() -> Vec<u8> {
        vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]
    }

    /// Signed LEB128, for `i32.const` operands (which are signed).
    fn sleb(mut value: i32, out: &mut Vec<u8>) {
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            let done = (value == 0 && byte & 0x40 == 0) || (value == -1 && byte & 0x40 != 0);
            if done {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }

    fn i32_const(value: i32, out: &mut Vec<u8>) {
        out.push(0x41);
        sleb(value, out);
    }

    /// A module that imports `log`, `edit` and `run`; its `noctis_update` logs a
    /// byte string, queues a spawn, and dispatches the `bump` verb.
    fn wasm_kitchen_sink_module() -> Vec<u8> {
        let json = br#"[{"op":"spawn","name":"Wasm"}]"#;
        let bump = b"bump";
        let json_at = 2;
        let bump_at = json_at + json.len();

        let mut out = wasm_header();

        // types: 0 (i32,i32)->(), 1 (i32,i32)->i32, 2 ()->i32, 3 (i32,i32,f32)->()
        let mut types = Vec::new();
        leb(4, &mut types);
        func_type(&[I32, I32], &[], &mut types);
        func_type(&[I32, I32], &[I32], &mut types);
        func_type(&[], &[I32], &mut types);
        func_type(&[I32, I32, F32], &[], &mut types);
        section(1, &types, &mut out);

        // imports: log -> 0, edit -> 1, run -> 2
        let mut imports = Vec::new();
        leb(3, &mut imports);
        import("noctis", "log", 0, &mut imports);
        import("noctis", "edit", 1, &mut imports);
        import("noctis", "run", 0, &mut imports);
        section(2, &imports, &mut out);

        // functions: noctis_abi (type 2), noctis_init (type 2), noctis_update (type 3)
        let mut functions = Vec::new();
        leb(3, &mut functions);
        leb(2, &mut functions);
        leb(2, &mut functions);
        leb(3, &mut functions);
        section(3, &functions, &mut out);

        // memory: 1 page
        let mut memory = Vec::new();
        leb(1, &mut memory);
        memory.push(0x00);
        leb(1, &mut memory);
        section(5, &memory, &mut out);

        // exports: memory, then the three ABI functions (indices 3, 4, 5)
        let mut exports = Vec::new();
        leb(4, &mut exports);
        wasm_name("memory", &mut exports);
        exports.push(0x02);
        leb(0, &mut exports);
        wasm_name("noctis_abi", &mut exports);
        exports.push(0x00);
        leb(3, &mut exports);
        wasm_name("noctis_init", &mut exports);
        exports.push(0x00);
        leb(4, &mut exports);
        wasm_name("noctis_update", &mut exports);
        exports.push(0x00);
        leb(5, &mut exports);
        section(7, &exports, &mut out);

        // code
        let mut code = Vec::new();
        leb(3, &mut code);
        for body in [
            abi_body(1),
            abi_body(0),
            update_body(json, json_at, bump_at, bump.len()),
        ] {
            leb(body.len() as u32, &mut code);
            code.extend_from_slice(&body);
        }
        section(10, &code, &mut out);

        // data: "hi" at 0, the spawn JSON at 2, "bump" after it
        let mut data = Vec::new();
        leb(3, &mut data);
        data.push(0x00);
        data.extend_from_slice(&[0x41, 0x00, 0x0b]);
        leb(2, &mut data);
        data.extend_from_slice(b"hi");
        data.push(0x00);
        i32_const_segment(json_at, &mut data);
        leb(json.len() as u32, &mut data);
        data.extend_from_slice(json);
        data.push(0x00);
        i32_const_segment(bump_at, &mut data);
        leb(bump.len() as u32, &mut data);
        data.extend_from_slice(bump);
        section(11, &data, &mut out);

        out
    }

    /// An active data segment's offset expression: `i32.const offset; end`.
    fn i32_const_segment(offset: usize, out: &mut Vec<u8>) {
        i32_const(offset as i32, out);
        out.push(0x0b);
    }

    /// A body with no locals returning the constant `value`.
    fn abi_body(value: i32) -> Vec<u8> {
        let mut body = Vec::new();
        leb(0, &mut body); // no locals
        i32_const(value, &mut body);
        body.push(0x0b);
        body
    }

    /// `noctis_update`: `log("hi")`, `edit(json)`, `run("bump")`.
    fn update_body(json: &[u8], json_at: usize, bump_at: usize, bump_len: usize) -> Vec<u8> {
        let mut body = Vec::new();
        leb(0, &mut body); // no locals
        i32_const(0, &mut body);
        i32_const(2, &mut body);
        body.extend_from_slice(&[0x10, 0x00]); // call log
        i32_const(json_at as i32, &mut body);
        i32_const(json.len() as i32, &mut body);
        body.extend_from_slice(&[0x10, 0x01, 0x1a]); // call edit; drop
        i32_const(bump_at as i32, &mut body);
        i32_const(bump_len as i32, &mut body);
        body.extend_from_slice(&[0x10, 0x02]); // call run
        body.push(0x0b);
        body
    }

    /// A module importing `edit` with a signature the ABI does not fix — it must
    /// be refused by name, not called with misread arguments.
    fn wasm_wrong_signature_module() -> Vec<u8> {
        let mut out = wasm_header();
        let mut types = Vec::new();
        leb(1, &mut types);
        func_type(&[I32], &[], &mut types);
        section(1, &types, &mut out);

        let mut imports = Vec::new();
        leb(1, &mut imports);
        import("noctis", "edit", 0, &mut imports);
        section(2, &imports, &mut out);
        out
    }

    /// Write an extension carrying both a JavaScript entry and a WASM module.
    fn write_wasm_extension(
        root: &std::path::Path,
        dir_name: &str,
        id: &str,
        js: &str,
        module: &[u8],
    ) {
        let dir = root.join(dir_name);
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(
            dir.join("extension.json"),
            format!(
                r#"{{ "id": "{id}", "name": "{id}", "version": "1.0.0", "api": 1, "entry": "extension.js", "wasm": {{ "module": "extension.wasm" }} }}"#
            ),
        )
        .expect("manifest");
        std::fs::write(dir.join("extension.js"), js).expect("entry");
        std::fs::write(dir.join("extension.wasm"), module).expect("module");
    }

    #[test]
    fn a_wasm_extension_logs_edits_and_runs_a_verb() {
        let root = temp_dir("wasm-full");
        let js = r#"noctis.command("bump", function () {
          noctis.edit({ op: "spawn", name: "Bumped" });
          return "ok bump";
        });"#;
        write_wasm_extension(
            &root,
            "full",
            "com.example.wasmfull",
            js,
            &wasm_kitchen_sink_module(),
        );
        let loader = Loader::discover(std::slice::from_ref(&root));
        let mut host = ExtensionHost::new().unwrap();
        host.load(&loader, |_| true);
        assert!(host.errors().is_empty(), "{:?}", host.errors());

        let mut editor = Editor::new();
        host.update(&mut editor, 0.016);
        assert!(editor.find_by_name("Wasm").is_some(), "the module's edit");
        assert!(
            editor.find_by_name("Bumped").is_some(),
            "the dispatched verb's edit"
        );
        assert_eq!(editor.list_entities().len(), 2);
        assert!(editor.can_undo());
        editor.undo();
        assert_eq!(editor.list_entities().len(), 0, "one undo covers the frame");

        let logs = host.take_logs();
        assert!(logs.iter().any(|line| line == "hi"), "{logs:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_wasm_module_with_a_wrong_signature_is_reported() {
        let root = temp_dir("wasm-bad");
        write_wasm_extension(
            &root,
            "bad",
            "com.example.wasmbad",
            "",
            &wasm_wrong_signature_module(),
        );
        let loader = Loader::discover(std::slice::from_ref(&root));
        let mut host = ExtensionHost::new().unwrap();
        host.load(&loader, |_| true);
        assert!(
            host.errors()
                .iter()
                .any(|error| error.contains("wrong signature")),
            "{:?}",
            host.errors()
        );
        std::fs::remove_dir_all(&root).ok();
    }

    fn f64_const(value: f64, out: &mut Vec<u8>) {
        out.push(0x44);
        out.extend_from_slice(&value.to_bits().to_le_bytes());
    }

    /// A body with no locals returning `value`.
    fn const_body(value: i32) -> Vec<u8> {
        let mut body = Vec::new();
        leb(0, &mut body);
        i32_const(value, &mut body);
        body.push(0x0b);
        body
    }

    /// `noctis_init`: register a panel titled at `[ptr, ptr+len)`.
    fn panel_init_body(ptr: usize, len: usize) -> Vec<u8> {
        let mut body = Vec::new();
        leb(0, &mut body);
        i32_const(ptr as i32, &mut body);
        i32_const(len as i32, &mut body);
        body.extend_from_slice(&[0x10, 0x00, 0x1a]); // call ui_panel; drop
        i32_const(0, &mut body); // return 0
        body.push(0x0b);
        body
    }

    /// `noctis_alloc(len) -> scratch`, ignoring `len` (a fixed scratch region).
    fn panel_alloc_body(scratch: usize) -> Vec<u8> {
        let mut body = Vec::new();
        leb(0, &mut body);
        i32_const(scratch as i32, &mut body);
        body.push(0x0b);
        body
    }

    /// `noctis_ui(panel, out, cap) -> len`: copy the `hit` op-string when the
    /// panel's button was activated or its slider has a value, else the `miss`
    /// one, into `out`; return the length.
    fn panel_ui_body(hit_at: usize, len: usize) -> Vec<u8> {
        let mut body = Vec::new();
        // One local: `src` is local 3 (after the three params).
        leb(1, &mut body);
        leb(1, &mut body);
        body.push(I32);

        // src = select(hit_at, miss_at, activated | (value > 0))
        i32_const(hit_at as i32, &mut body);
        i32_const(0, &mut body); // miss at 0
        body.extend_from_slice(&[0x20, 0x00, 0x41, 0x00, 0x10, 0x01]); // ui_activated(panel, 0)
        body.extend_from_slice(&[0x20, 0x00, 0x41, 0x00, 0x10, 0x02]); // ui_value(panel, 0)
        f64_const(0.0, &mut body);
        body.extend_from_slice(&[0x64, 0x72]); // f64.gt; i32.or
        body.push(0x1b); // select
        body.extend_from_slice(&[0x21, 0x03]); // local.set 3

        // Copy `len` bytes from src to out, one at a time.
        for index in 0..len {
            body.extend_from_slice(&[0x20, 0x01]); // out
            i32_const(index as i32, &mut body);
            body.push(0x6a); // i32.add
            body.extend_from_slice(&[0x20, 0x03]); // src
            i32_const(index as i32, &mut body);
            body.push(0x6a); // i32.add
            body.extend_from_slice(&[0x2d, 0x00, 0x00]); // i32.load8_u
            body.extend_from_slice(&[0x3a, 0x00, 0x00]); // i32.store8
        }
        i32_const(len as i32, &mut body);
        body.push(0x0b);
        body
    }

    /// A module that registers one panel and draws it, reading its button/slider
    /// state back through `ui_activated`/`ui_value`.
    fn wasm_panel_module() -> Vec<u8> {
        let miss = br#"[{"kind":"label","text":"off"}]"#;
        let hit = br#"[{"kind":"label","text":"on!"}]"#;
        let title = b"Tools";
        assert_eq!(miss.len(), hit.len(), "the two op strings must match");
        let len = miss.len();
        let title_at = len * 2;
        let scratch = 2048;

        let mut out = wasm_header();

        // types: 0 (i32,i32)->i32, 1 (i32,i32)->f64, 2 ()->i32,
        //        3 (i32,i32,f32)->(), 4 (i32)->i32, 5 (i32,i32,i32)->i32
        let mut types = Vec::new();
        leb(6, &mut types);
        func_type(&[I32, I32], &[I32], &mut types);
        func_type(&[I32, I32], &[F64], &mut types);
        func_type(&[], &[I32], &mut types);
        func_type(&[I32, I32, F32], &[], &mut types);
        func_type(&[I32], &[I32], &mut types);
        func_type(&[I32, I32, I32], &[I32], &mut types);
        section(1, &types, &mut out);

        // imports: ui_panel -> 0, ui_activated -> 1, ui_value -> 2
        let mut imports = Vec::new();
        leb(3, &mut imports);
        import("noctis", "ui_panel", 0, &mut imports);
        import("noctis", "ui_activated", 0, &mut imports);
        import("noctis", "ui_value", 1, &mut imports);
        section(2, &imports, &mut out);

        // functions: abi(2), init(2), update(3), alloc(4), ui(5)
        let mut functions = Vec::new();
        leb(5, &mut functions);
        for type_index in [2u32, 2, 3, 4, 5] {
            leb(type_index, &mut functions);
        }
        section(3, &functions, &mut out);

        // memory: 1 page
        let mut memory = Vec::new();
        leb(1, &mut memory);
        memory.push(0x00);
        leb(1, &mut memory);
        section(5, &memory, &mut out);

        // exports: memory, noctis_abi(3), noctis_init(4), noctis_update(5),
        //          noctis_alloc(6), noctis_ui(7)
        let mut exports = Vec::new();
        leb(6, &mut exports);
        wasm_name("memory", &mut exports);
        exports.push(0x02);
        leb(0, &mut exports);
        for (name, index) in [
            ("noctis_abi", 3u32),
            ("noctis_init", 4),
            ("noctis_update", 5),
            ("noctis_alloc", 6),
            ("noctis_ui", 7),
        ] {
            wasm_name(name, &mut exports);
            exports.push(0x00);
            leb(index, &mut exports);
        }
        section(7, &exports, &mut out);

        // code
        let mut code = Vec::new();
        let bodies = [
            const_body(1),
            panel_init_body(title_at, title.len()),
            {
                let mut body = Vec::new();
                leb(0, &mut body);
                body.push(0x0b);
                body
            },
            panel_alloc_body(scratch),
            panel_ui_body(len, len),
        ];
        leb(bodies.len() as u32, &mut code);
        for body in bodies {
            leb(body.len() as u32, &mut code);
            code.extend_from_slice(&body);
        }
        section(10, &code, &mut out);

        // data: the miss ops at 0, the hit ops at len, the title after them
        let mut data = Vec::new();
        leb(3, &mut data);
        data.push(0x00);
        i32_const_segment(0, &mut data);
        leb(miss.len() as u32, &mut data);
        data.extend_from_slice(miss);
        data.push(0x00);
        i32_const_segment(len, &mut data);
        leb(hit.len() as u32, &mut data);
        data.extend_from_slice(hit);
        data.push(0x00);
        i32_const_segment(title_at, &mut data);
        leb(title.len() as u32, &mut data);
        data.extend_from_slice(title);
        section(11, &data, &mut out);

        out
    }

    #[test]
    fn a_wasm_extension_draws_a_panel_and_reads_interactions() {
        let root = temp_dir("wasm-panel");
        write_wasm_extension(
            &root,
            "panel",
            "com.example.wasmpanel",
            "",
            &wasm_panel_module(),
        );
        let loader = Loader::discover(std::slice::from_ref(&root));
        let mut host = ExtensionHost::new().unwrap();
        host.load(&loader, |_| true);
        assert!(host.errors().is_empty(), "{:?}", host.errors());
        let editor = Editor::new();

        // No interaction: the module draws its "off" ops.
        let panels = host.draw_panels(&editor, &Interactions::default());
        assert_eq!(panels.len(), 1);
        assert_eq!(panels[0].title, "Tools");
        assert!(
            panels[0]
                .widgets
                .iter()
                .any(|widget| matches!(widget, Widget::Label(text) if text == "off")),
            "{:?}",
            panels[0].widgets
        );

        // A button click at the panel's global index is seen by ui_activated.
        let panels = host.draw_panels(
            &editor,
            &Interactions {
                activations: vec![(0, 0)],
                values: Vec::new(),
            },
        );
        assert!(
            panels[0]
                .widgets
                .iter()
                .any(|widget| matches!(widget, Widget::Label(text) if text == "on!")),
            "{:?}",
            panels[0].widgets
        );

        // A control value is seen by ui_value.
        let panels = host.draw_panels(
            &editor,
            &Interactions {
                activations: Vec::new(),
                values: vec![((0, 0), 3.0)],
            },
        );
        assert!(
            panels[0]
                .widgets
                .iter()
                .any(|widget| matches!(widget, Widget::Label(text) if text == "on!")),
            "{:?}",
            panels[0].widgets
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn wasm_panel_ids_are_offset_past_the_javascript_panels() {
        let root = temp_dir("wasm-panel-offset");
        let js = r#"noctis.ui.panel("JS Panel", function (ui) { ui.label("js"); });"#;
        write_wasm_extension(
            &root,
            "panel",
            "com.example.wasmpanel",
            js,
            &wasm_panel_module(),
        );
        let loader = Loader::discover(std::slice::from_ref(&root));
        let mut host = ExtensionHost::new().unwrap();
        host.load(&loader, |_| true);
        assert!(host.errors().is_empty(), "{:?}", host.errors());
        let editor = Editor::new();

        // The JavaScript panel comes first; the module's panel follows it.
        let panels = host.draw_panels(&editor, &Interactions::default());
        assert_eq!(panels.len(), 2);
        assert_eq!(panels[0].title, "JS Panel");
        assert_eq!(panels[1].title, "Tools");

        // Global index 1 is the module's panel, so its local id 0 maps there.
        let see = |panels: &[Panel], text: &str| {
            panels
                .iter()
                .find(|panel| panel.title == "Tools")
                .is_some_and(|panel| {
                    panel
                        .widgets
                        .iter()
                        .any(|widget| matches!(widget, Widget::Label(t) if t == text))
                })
        };
        let panels = host.draw_panels(
            &editor,
            &Interactions {
                activations: vec![(1, 0)],
                values: Vec::new(),
            },
        );
        assert!(
            see(&panels, "on!"),
            "index 1 reaches the module: {panels:?}"
        );

        // Global index 0 is the JavaScript panel and must not reach it.
        let panels = host.draw_panels(
            &editor,
            &Interactions {
                activations: vec![(0, 0)],
                values: Vec::new(),
            },
        );
        assert!(see(&panels, "off"), "index 0 is not the module: {panels:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn each_extension_runs_in_its_own_context() {
        let root = temp_dir("isolate");
        write_extension(
            &root,
            "alpha",
            "com.example.alpha",
            r#"
            globalThis.polluted = 42;
            noctis.command("alpha", function () { return "ok alpha"; });
            "#,
        );
        write_extension(
            &root,
            "beta",
            "com.example.beta",
            r#"
            noctis.command("beta", function () {
              var leak = (typeof globalThis.polluted === "number") ? "shared" : "isolated";
              var cross = noctis.run("alpha");
              return "ok " + leak + " cross=" + String(cross);
            });
            "#,
        );
        let loader = Loader::discover(std::slice::from_ref(&root));
        let mut host = ExtensionHost::new().unwrap();
        host.load(&loader, |_| true);
        assert!(host.errors().is_empty(), "{:?}", host.errors());
        let mut editor = Editor::new();

        // Each verb is reachable through the host's per-context dispatch.
        assert_eq!(host.dispatch(&mut editor, "alpha").unwrap(), "ok alpha");

        // beta sees neither alpha's global nor alpha's verb: separate realms.
        assert_eq!(
            host.dispatch(&mut editor, "beta").unwrap(),
            "ok isolated cross=null"
        );

        // The built-ins aggregate across contexts.
        let help = host.dispatch(&mut editor, "help").unwrap();
        assert!(help.contains("alpha") && help.contains("beta"), "{help}");
        assert_eq!(
            host.dispatch(&mut editor, "extensions").unwrap(),
            "ok extensions: com.example.alpha, com.example.beta"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// Write an extension directory whose manifest is given verbatim.
    fn write_manifest_extension(root: &std::path::Path, dir_name: &str, manifest: &str, js: &str) {
        let dir = root.join(dir_name);
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("extension.json"), manifest).expect("manifest");
        std::fs::write(dir.join("extension.js"), js).expect("entry");
    }

    #[test]
    fn a_viewport_overlay_records_primitives_and_reads_the_pointer() {
        let root = temp_dir("viewport");
        write_extension(
            &root,
            "overlay",
            "com.example.overlay",
            r##"
            noctis.ui.viewport(function (vui) {
              var p = vui.pointer();
              if (p.clicked) { vui.capture(); }
              vui.rect(0, 0, p.width, p.height, "#101010");
              vui.stroke(4, 4, 16, 16, [255, 0, 0, 128], 2);
              vui.line(0, 0, 10, 10, "#fff", 3);
              vui.text(4, 6, "hi " + Math.round(p.x), "#ffffff", 14);
            });
            "##,
        );
        let loader = Loader::discover(std::slice::from_ref(&root));
        let mut host = ExtensionHost::new().unwrap();
        host.load(&loader, |_| true);
        assert!(host.errors().is_empty(), "{:?}", host.errors());
        let editor = Editor::new();

        let mut input = ViewportInput {
            width: 100.0,
            height: 50.0,
            x: 3.4,
            y: 8.0,
            inside: true,
            clicked: true,
            ..ViewportInput::default()
        };
        let frame = host.draw_viewport(&editor, input);
        assert!(frame.registered, "the overlay registered itself");
        assert!(frame.capture, "a click claims the pointer");
        assert_eq!(
            frame.ops,
            vec![
                ViewportOp::Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 100.0,
                    h: 50.0,
                    color: [16, 16, 16, 255],
                    filled: true,
                    thickness: 1.0,
                },
                ViewportOp::Rect {
                    x: 4.0,
                    y: 4.0,
                    w: 16.0,
                    h: 16.0,
                    color: [255, 0, 0, 128],
                    filled: false,
                    thickness: 2.0,
                },
                ViewportOp::Line {
                    x1: 0.0,
                    y1: 0.0,
                    x2: 10.0,
                    y2: 10.0,
                    color: [255, 255, 255, 255],
                    thickness: 3.0,
                },
                ViewportOp::Text {
                    x: 4.0,
                    y: 6.0,
                    text: "hi 3".into(),
                    color: [255, 255, 255, 255],
                    size: 14.0,
                },
            ]
        );

        // A frame with no click leaves the pointer to the editor.
        input.clicked = false;
        let frame = host.draw_viewport(&editor, input);
        assert!(frame.registered);
        assert!(!frame.capture);
        assert_eq!(frame.ops.len(), 4, "the overlay redraws every frame");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_extension_without_a_viewport_leaves_the_editor_its_viewport() {
        let root = temp_dir("no-viewport");
        write_extension(
            &root,
            "panels-only",
            "com.example.panels",
            "noctis.ui.panel('P', function () {});",
        );
        let loader = Loader::discover(std::slice::from_ref(&root));
        let mut host = ExtensionHost::new().unwrap();
        host.load(&loader, |_| true);
        let editor = Editor::new();

        let frame = host.draw_viewport(&editor, ViewportInput::default());
        assert!(!frame.registered);
        assert!(!frame.capture);
        assert!(frame.ops.is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_viewport_that_declines_leaves_the_editor_its_viewport() {
        let root = temp_dir("declining-viewport");
        write_extension(
            &root,
            "hidden",
            "com.example.hidden",
            "noctis.ui.viewport(function (vui) {
              if (vui.pointer().inside) {
                vui.rect(0, 0, 10, 10, \"#ffffff\");
                return;
              }
              return false;
            });",
        );
        let loader = Loader::discover(std::slice::from_ref(&root));
        let mut host = ExtensionHost::new().unwrap();
        host.load(&loader, |_| true);
        let editor = Editor::new();

        // The pointer is outside: the draw declined, so the editor keeps its own.
        let frame = host.draw_viewport(&editor, ViewportInput::default());
        assert!(!frame.registered);
        assert!(frame.ops.is_empty());

        // The pointer is inside: the overlay paints.
        let inside = ViewportInput {
            inside: true,
            ..ViewportInput::default()
        };
        let frame = host.draw_viewport(&editor, inside);
        assert!(frame.registered);
        assert_eq!(frame.ops.len(), 1);
        std::fs::remove_dir_all(&root).ok();
    }

    /// A level editor in miniature: a project file read and written by slot.
    ///
    /// `root` is the directory discovery scans; the extension lands in
    /// `root/levels-ext` and the project in `root/project`.
    fn project_files_extension(root: &std::path::Path) -> std::path::PathBuf {
        let project = root.join("project");
        std::fs::create_dir_all(project.join("levels")).expect("project dir");
        std::fs::write(project.join("levels/level1.txt"), "name: One\n").expect("level");

        write_manifest_extension(
            root,
            "levels-ext",
            r#"{ "id": "com.example.levels", "name": "Levels", "version": "1.0.0", "api": 1,
                "entry": "extension.js", "files": { "level1": "levels/level1.txt" } }"#,
            r#"
            noctis.command("read", function () {
              var text = noctis.files.read("level1");
              return "ok " + (text === undefined ? "missing" : JSON.stringify(text));
            });
            noctis.command("missing", function () {
              return "ok " + (noctis.files.read("nope") === undefined);
            });
            noctis.command("slots", function () { return "ok " + JSON.stringify(noctis.files.slots()); });
            noctis.command("write", function (parts) {
              return "ok " + noctis.files.write("level1", "name: " + parts[1] + "\n");
            });
            "#,
        );
        project
    }

    #[test]
    fn project_files_are_read_and_written_through_a_declared_slot() {
        let root = temp_dir("project-files");
        let project = project_files_extension(&root);
        let loader = Loader::discover(std::slice::from_ref(&root));
        let mut host = ExtensionHost::new().unwrap();
        host.load(&loader, |_| true);
        assert!(host.errors().is_empty(), "{:?}", host.errors());
        host.set_project(Box::new(vfs::DirStore::new(project.clone())));
        let mut editor = Editor::new();

        assert_eq!(
            host.dispatch(&mut editor, "slots").unwrap(),
            "ok [\"level1\"]"
        );
        assert_eq!(
            host.dispatch(&mut editor, "read").unwrap(),
            "ok \"name: One\\n\""
        );
        // An undeclared slot is `undefined` — not an error.
        assert_eq!(host.dispatch(&mut editor, "missing").unwrap(), "ok true");

        // The write goes through the project's own store to the real file.
        assert_eq!(host.dispatch(&mut editor, "write Two").unwrap(), "ok true");
        assert_eq!(
            std::fs::read_to_string(project.join("levels/level1.txt")).unwrap(),
            "name: Two\n"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_file_write_without_a_project_is_refused_and_reported() {
        let root = temp_dir("no-project");
        let _ = project_files_extension(&root);
        let loader = Loader::discover(std::slice::from_ref(&root));
        let mut host = ExtensionHost::new().unwrap();
        host.load(&loader, |_| true);
        // No `set_project`: the channel is closed.
        let mut editor = Editor::new();

        assert_eq!(host.dispatch(&mut editor, "write Two").unwrap(), "ok false");
        let logs = host.take_logs();
        assert!(
            logs.iter().any(|log| log.contains("no open project")),
            "{logs:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
