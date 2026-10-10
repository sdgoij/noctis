//! Editor extensions: the `noctis` JavaScript global, loading, and verb dispatch.
//!
//! This is the host half of `docs/extensions.md`. The pure loader
//! ([`extensions`]) discovers and reads extensions; here they are evaluated in
//! the Slag engine against a single versioned global, `noctis`, and their edits
//! are replayed through [`Editor::apply_json`] — the same validated, undoable
//! path the CLI and an AI agent use.
//!
//! The global is defined *in JavaScript* (the [`BOOTSTRAP`] source): it holds the
//! registered verbs and hooks and calls a handful of Rust primitives
//! (`__host_edit`, `__host_reads`, ...) for the things that need the editor. That
//! mirrors how the Goat mod API wraps the scene in a bootstrap script: the Rust
//! side stays small and the JS side owns the registry.
//!
//! JS gets no filesystem: an extension can only touch the editor through
//! `noctis.edit` (an `EditorCommand` batch) and the read snapshots. It cannot
//! corrupt the scene in a way `undo` cannot reverse.

use std::cell::RefCell;
use std::rc::Rc;

use ecs::scene::SceneMode;
use extensions::{Loader, Manifest};

use crate::Editor;
use crate::info::EntityDto;

mod wasm;
mod watch;

use wasm::WasmHost;

pub use watch::{ExtensionReload, ExtensionWatch};

/// The `noctis` global, defined in JavaScript. See the module docs.
const BOOTSTRAP: &str = r#"
(function () {
  "use strict";
  var RESERVED = ["help", "extensions", "run", "quit", "exit"];
  var current = null;
  var frozen = false;
  var extensions = {};
  var commands = {};
  var hooks = {};

  var noctis = {
    api: 1,
    extension: null,
    begin: function (id) { current = id; return noctis; },
    end: function (id) { if (current === id) { current = null; } },
    freeze: function () { current = null; frozen = true; },
    on: function (event, fn) {
      if (frozen) { throw new Error("registration is closed"); }
      (hooks[event] = hooks[event] || []).push({ id: current, fn: fn });
      return function () {};
    },
    off: function () {},
    command: function (name, fn) {
      if (frozen) { throw new Error("registration is closed"); }
      if (RESERVED.indexOf(name) >= 0) {
        __host_log("extension " + current + " may not shadow the built-in verb '" + name + "'");
        return;
      }
      commands[name] = { id: current, fn: fn };
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
      selection: function () { return JSON.stringify(this._reads().selection); },
      scene_name: function () { return this._reads().scene; },
      mode: function () { return this._reads().mode; },
      find: function (name) {
        var list = this._reads().entities;
        for (var i = 0; i < list.length; i++) {
          if (list[i].name === name) { return list[i].entity; }
        }
        return undefined;
      }
    }
  };
  globalThis.noctis = noctis;

  globalThis.__noctis_register_extension = function (id, info) {
    extensions[id] = JSON.parse(info);
  };

  globalThis.__noctis_fire = function (event, arg) {
    var fns = hooks[event] || [];
    for (var i = 0; i < fns.length; i++) {
      noctis.extension = extensions[fns[i].id] || null;
      try { fns[i].fn(arg); } catch (e) { __host_log("hook '" + event + "' threw: " + e); }
    }
  };

  globalThis.__noctis_dispatch = function (line) {
    var text = String(line).trim();
    var parts = text.length ? text.split(/\s+/) : [];
    var verb = parts[0] || "";
    var reply;
    if (verb === "help" || verb === "") {
      reply = "ok verbs: " + Object.keys(commands).sort().join(", ");
    } else if (verb === "extensions") {
      reply = "ok extensions: " + JSON.parse(__host_ids()).join(", ");
    } else {
      var observers = hooks["command"] || [];
      for (var i = 0; i < observers.length; i++) {
        noctis.extension = extensions[observers[i].id] || null;
        try {
          var observed = observers[i].fn(text, parts);
          if (typeof observed === "string") { reply = observed; break; }
        } catch (e) { __host_log("command observer threw: " + e); }
      }
      if (reply === undefined) {
        var entry = commands[verb];
        if (entry) {
          noctis.extension = extensions[entry.id] || null;
          try {
            var out = entry.fn(parts, text);
            reply = (typeof out === "string") ? out : "";
          } catch (e) { reply = "error: verb '" + verb + "' threw: " + e; }
        }
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
        }
      };
      try { panels[p].draw(ui); } catch (e) { __host_log("panel '" + panels[p].title + "' threw: " + e); }
      __host_panel(String(panels[p].title), JSON.stringify(ops));
    }
  };
})();
"#;

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
}

/// A panel an extension drew this frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Panel {
    /// The panel title (and its egui window id).
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
    /// The loaded extension ids, as JSON.
    ids: String,
    /// Messages from `noctis.log`/`noctis.fail` and caught handler errors.
    logs: Vec<String>,
    /// The reply from the last dispatch (`None` means "no verb matched").
    reply: Option<String>,
    /// The panels drawn this frame.
    panels: Vec<Panel>,
    /// The (panel, widget) activations and value changes from the previous frame.
    interactions: Interactions,
}

/// The JavaScript host for editor extensions.
pub struct ExtensionHost {
    context: slag::Context,
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
            .field("ids", &self.ids)
            .field("wasm", &self.wasm)
            .field("errors", &self.errors)
            .finish_non_exhaustive()
    }
}

impl ExtensionHost {
    /// Start the engine, install the primitives, and define the `noctis` global.
    pub fn new() -> Result<Self, String> {
        let context = slag::Context::new().map_err(|error| error.to_string())?;
        let bridge = Rc::new(RefCell::new(Bridge::default()));
        let wasm = WasmHost::new(Rc::clone(&bridge));
        let mut host = Self {
            context,
            bridge,
            wasm,
            ids: Vec::new(),
            entries: Vec::new(),
            errors: Vec::new(),
        };
        host.install()?;
        host.eval(BOOTSTRAP)?;
        Ok(host)
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

    /// Load every extension in `loader` for which `enabled` is true, evaluating
    /// its entry inside a private wrapper, then close registration.
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
        self.bridge.borrow_mut().ids =
            serde_json::to_string(&self.ids).unwrap_or_else(|_| "[]".to_string());

        for manifest in &manifests {
            let info = serde_json::json!({
                "id": manifest.id,
                "name": manifest.name,
                "version": manifest.version,
            })
            .to_string();
            let program = format!(
                "__noctis_register_extension({}, {});",
                json_literal(&manifest.id),
                json_literal(&info)
            );
            if let Err(error) = self.eval(&program) {
                self.errors.push(format!("{}: {error}", manifest.id));
                continue;
            }

            if let Some(source) = &manifest.entry_source {
                let program = format!(
                    "(function (noctis) {{\n\"use strict\";\n{source}\n}})(noctis.begin({}));",
                    json_literal(&manifest.id)
                );
                if let Err(error) = self.eval(&program) {
                    self.errors.push(format!("{}: {error}", manifest.id));
                }
            }

            if let Some(module) = &manifest.wasm
                && let Err(error) = self.wasm.load(&module.bytes)
            {
                self.errors.push(format!("{}: {error}", manifest.id));
            }
        }

        if let Err(error) = self.eval("noctis.freeze();") {
            self.errors.push(format!("freeze: {error}"));
        }
    }

    /// Fire the `"ready"` hook and apply any edits it queued.
    pub fn ready(&mut self, editor: &mut Editor) -> Vec<String> {
        self.snapshot(editor);
        if let Err(error) = self.eval("__noctis_fire(\"ready\", undefined);") {
            self.errors.push(format!("ready: {error}"));
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
    fn dispatch_line(&mut self, editor: &mut Editor, line: &str) -> Result<String, String> {
        self.snapshot(editor);
        self.bridge.borrow_mut().reply = None;
        let program = format!("__noctis_dispatch_line({});", json_literal(line));
        self.eval(&program)?;
        match self.bridge.borrow_mut().reply.take() {
            Some(reply) => Ok(reply),
            None => Err(format!("no extension verb matched {line:?}")),
        }
    }

    /// Fire the `"update"` hook for this frame and apply any edits it queued.
    pub fn update(&mut self, editor: &mut Editor, dt: f32) -> Vec<String> {
        self.snapshot(editor);
        let program = format!("__noctis_fire(\"update\", {dt});");
        if let Err(error) = self.eval(&program) {
            self.errors.push(format!("update: {error}"));
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
        if let Err(error) = self.eval("__noctis_draw_panels();") {
            self.errors.push(format!("panels: {error}"));
        }
        std::mem::take(&mut self.bridge.borrow_mut().panels)
    }

    /// Apply any work a panel draw (or `noctis.run`) queued.
    pub fn apply_pending(&mut self, editor: &mut Editor) -> Vec<String> {
        self.apply_queued(editor)
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

    fn install(&mut self) -> Result<(), String> {
        let b = Rc::clone(&self.bridge);
        self.register(
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

        let b = Rc::clone(&self.bridge);
        self.register(
            "__host_reads",
            0,
            Box::new(move |_call| Ok(slag::JsValue::string(b.borrow().reads.clone()))),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "__host_log",
            1,
            Box::new(move |call| {
                let message = arg_string(call, 0).unwrap_or_default();
                b.borrow_mut().logs.push(message);
                Ok(slag::JsValue::undefined())
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "__host_ids",
            0,
            Box::new(move |_call| Ok(slag::JsValue::string(b.borrow().ids.clone()))),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "__host_reply",
            2,
            Box::new(move |call| {
                let ok = arg_number(call, 0).unwrap_or(0.0);
                let text = arg_string(call, 1).unwrap_or_default();
                b.borrow_mut().reply = if ok != 0.0 { Some(text) } else { None };
                Ok(slag::JsValue::undefined())
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "__host_panel",
            2,
            Box::new(move |call| {
                let title = arg_string(call, 0).unwrap_or_default();
                let ops = arg_string(call, 1).unwrap_or_default();
                b.borrow_mut().panels.push(Panel {
                    title,
                    widgets: parse_ops(&ops),
                });
                Ok(slag::JsValue::undefined())
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "__host_activated",
            2,
            Box::new(move |call| {
                let panel = arg_number(call, 0).unwrap_or(0.0) as usize;
                let widget = arg_number(call, 1).unwrap_or(0.0) as usize;
                let hit = b
                    .borrow()
                    .interactions
                    .activations
                    .contains(&(panel, widget));
                Ok(slag::JsValue::number(if hit { 1.0 } else { 0.0 }))
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "__host_value",
            2,
            Box::new(move |call| {
                let panel = arg_number(call, 0).unwrap_or(0.0) as usize;
                let widget = arg_number(call, 1).unwrap_or(0.0) as usize;
                let value = b
                    .borrow()
                    .interactions
                    .values
                    .iter()
                    .find(|(key, _)| *key == (panel, widget))
                    .map_or(f64::NAN, |(_, value)| *value);
                Ok(slag::JsValue::number(value))
            }),
        )?;

        Ok(())
    }

    fn register(&mut self, name: &str, arity: u32, f: slag::HostFn) -> Result<(), String> {
        self.context
            .register_fn(name, arity, f)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn eval(&mut self, program: &str) -> Result<(), String> {
        self.context
            .eval(program)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
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
            _ => None,
        })
        .collect()
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
              return "ok " + noctis.editor.mode() + " " + names;
            });
            "#,
        );
        let loader = Loader::discover(std::slice::from_ref(&root));
        let mut host = ExtensionHost::new().unwrap();
        host.load(&loader, |_| true);
        let mut editor = Editor::new();
        editor.spawn("Sun", None);

        let reply = host.dispatch(&mut editor, "who").unwrap();
        assert_eq!(reply, "ok 3d Sun");
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
              ui.label("enable " + on + " gain " + gain);
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

        // A change fed back on the next draw is what the panel reads and draws.
        let panels = host.draw_panels(
            &editor,
            &Interactions {
                activations: Vec::new(),
                values: vec![((0, 0), 1.0), ((0, 1), 5.0)],
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
        assert!(
            panels[0].widgets.iter().any(
                |widget| matches!(widget, Widget::Label(text) if text == "enable true gain 5")
            ),
            "{:?}",
            panels[0].widgets
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
}
