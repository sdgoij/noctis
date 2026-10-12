# The editor UI: title bar, docks, and a status bar

> **Status:** design proposal; **P1–P4 are implemented** (the layout model, the
> status bar, the title bar, and extension panels in docks) — see *Status* at the
> end. P5 is not. It records the *decisions* and an *implementation plan* for
> reshaping `noctis editor`'s windowed layout into a Zed-style dock system. Before
> P1 the layout was fixed: `crates/editor/src/window.rs` drew a menu strip, an
> outliner panel, an inspector panel, a central viewport, and floating
> `egui::Window`s for the console and the Extensions screen, gated by four booleans
> (`show_outliner`, `show_inspector`, `show_console`, `show_extensions`).

The inspiration is Zed's workspace: a custom title bar, resizable docks around a
central pane, and a **status bar** whose buttons bring panels into view. We take
the *shape*, not the code — Zed is built on GPUI; this is egui 0.36.

## Goal

One fixed frame around the scene:

```
┌───────────────────────────────────────────────────────────────┐
│ TitleBar                                                       │
├───────────┬───────────────────────────────────────┬───────────┤
│           │                                       │           │
│  LDock    │              ViewPort                 │  RDock    │
│           │                                       │           │
│           ├───────────────────────────────────────┤           │
│           │              BDock                    │           │
├───────────┴───────────────────────────────────────┴───────────┤
│ StatusBar                                                      │
└───────────────────────────────────────────────────────────────┘
```

- **TitleBar** — top. The app name, the menus (already in the current strip: the
  scene picker, the gizmo tool/frame, Undo/Redo/Save), and the window controls.
- **ViewPort** — centre, exactly the current central panel: the scene is blitted
  to the surface *before* egui draws, and the central panel is transparent so the
  scene shows through.
- **LDock / RDock / BDock** — three resizable docks that host **views**.
- **StatusBar** — the very bottom, spanning the full width; a toggle button per
  view.

And the key idea that makes it extensible: **the outliner, the inspector, the
console, the Extensions screen and every extension panel are all just *views***.
A view has no fixed home; a dock hosts whatever it is given, and the status bar is
where the user turns views on and off.

## The view

A view is one dockable panel. Model it as an **enum**, not a trait object:

```rust
// crates/editor/src/window.rs (or a new `layout.rs`)

/// One dockable UI panel — the unit a dock hosts and the status bar toggles.
#[derive(Clone, Debug, PartialEq, Eq)]
enum View {
    /// The scene hierarchy (today's `outliner`).
    Outliner,
    /// The selected entity's components (today's `inspector`).
    Inspector,
    /// The verb console and its scrollback.
    Console,
    /// The Extensions enable/disable screen.
    Extensions,
    /// One panel an extension drew this frame, by its stable id.
    Extension(String),
}
```

Why an enum and not `Box<dyn DockView>`: the built-ins need `&mut Editor` and the
driver's other fields, and a trait object that hands out `&mut self` invites
borrow gymnastics the repo avoids. The repo's own rule is *"prefer a clear,
explicit implementation over a clever one"*. Extension panels are already
**data** (`extension::Panel`), not Rust objects, so `View::Extension(id)` covers
them without a trait. If a future need for third-party Rust views appears, the
enum can be promoted to a trait then — deliberately.

Each view carries four bits of metadata (plus a derived tooltip):

```rust
impl View {
    /// The view's name. It is **not drawn**: it is the hover tooltip, and the id
    /// used for tab identity and layout persistence.
    fn title(&self) -> &str;

    /// The icon drawn by the tab and status-bar button.
    ///
    /// `Icon` is a placeholder. When `None`, the default icon (`Icon::default()`)
    /// is drawn instead, so a control is never label-less and the title stays
    /// reachable only on hover.
    fn icon(&self) -> Option<Icon>;

    /// An optional one-line description, shown under the title on hover.
    fn hint(&self) -> Option<&str>;

    /// The hover tooltip: the title, with `hint` appended when present.
    fn tooltip(&self) -> String {
        match self.hint() {
            Some(hint) => format!("{}\n{hint}", self.title()),
            None => self.title().to_owned(),
        }
    }

    /// The dock a view opens in when first shown.
    fn default_dock(&self) -> Dock;
}
```

Defaults: Outliner → `Left`, Inspector → `Right`, Console / Extensions / an
extension panel → `Bottom`. A view's drawn label is **always an icon** — its own,
or the default when it names none; the title appears only on hover (with `hint`
as a second line when present).

## The dock

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dock { Left, Right, Bottom }

/// A dock's hosted views and which one is on top.
#[derive(Clone, Debug, Default)]
struct DockState {
    /// The views shown, in tab order. Empty ⇒ the dock is hidden.
    open: Vec<View>,
    /// The index of the active tab.
    active: usize,
}

#[derive(Clone, Debug, Default)]
struct Layout {
    left: DockState,
    right: DockState,
    bottom: DockState,
}

impl Layout {
    fn dock(&self, dock: Dock) -> &DockState {
        match dock {
            Dock::Left => &self.left,
            Dock::Right => &self.right,
            Dock::Bottom => &self.bottom,
        }
    }

    fn dock_mut(&mut self, dock: Dock) -> &mut DockState {
        match dock {
            Dock::Left => &mut self.left,
            Dock::Right => &mut self.right,
            Dock::Bottom => &mut self.bottom,
        }
    }
}
```

Semantics, kept deliberately small:

- A dock is **visible iff it holds at least one view** — there is no separate
  `visible` flag to keep in sync.
- **Toggling a view on** appends it to its dock (or reveals it if already there),
  selects it, and shows the dock. **Toggling it off** removes it; the dock hides
  when its last view leaves.
- A view lives in exactly one dock. Moving it is a **"Send to ▸ Left/Right/Bottom"**
  right-click menu on its tab — no drag-and-drop in the first cut (drag between
  docks is a nice follow-up, and is where Zed's `Pane`/`Item` model earns its
  weight; we do not need it yet).
- With several views open, a dock draws a small **tab strip** across its top and
  the active view beneath it — **tabs** are the chosen form (*Decisions*); stacked,
  collapsible sections were the alternative.

Drawing a dock, with the panel-order rule that produces the layout above:

```rust
fn dock(&mut self, ui: &mut egui::Ui, dock: Dock) {
    if self.layout.dock(dock).open.is_empty() {
        return;
    }
    // `Panel` in egui 0.36 is one builder for all four sides; `top`/`bottom` are
    // non-resizable by default, so the bottom dock opts in.
    let panel = match dock {
        Dock::Left => egui::Panel::left("noctis_dock_left"),
        Dock::Right => egui::Panel::right("noctis_dock_right"),
        Dock::Bottom => egui::Panel::bottom("noctis_dock_bottom"),
    }
    .resizable(true)
    .default_size(match dock {
        Dock::Left | Dock::Right => 240.0,
        Dock::Bottom => 200.0,
    });
    panel.show(ui, |ui| {
        // Tab strip, then the active view. Copy the views out first so the
        // `&mut self` borrow in `show_view` does not overlap `self.layout`.
        let views = self.layout.dock(dock).open.clone();
        let active = self.layout.dock(dock).active;
        self.dock_tabs(ui, dock, &views, active);
        if let Some(view) = views.get(active) {
            self.show_view(view, ui);
        }
    });
}

fn show_view(&mut self, view: &View, ui: &mut egui::Ui) {
    match view {
        View::Outliner   => self.outliner(ui),
        View::Inspector  => self.inspector(ui),
        View::Console    => self.console(ui),
        View::Extensions => self.extensions_screen(ui),
        View::Extension(id) => self.extension_panel(id, ui),
    }
}

/// The tab strip above a dock's active view: one tab per hosted view.
///
/// A tab follows the same rule as a status-bar button — an icon label (the
/// default when the view names none), with the title on hover.
fn dock_tabs(&mut self, ui: &mut egui::Ui, dock: Dock, views: &[View], active: usize) {
    let (mut select, mut send, mut close) = (None, None, None);
    ui.horizontal_wrapped(|ui| {
        for (index, view) in views.iter().enumerate() {
            let tab = ui
                .selectable_label(index == active, view.icon().unwrap_or_default())
                .on_hover_text(view.tooltip());

            if tab.clicked() {
                select = Some(index);
            }
            if tab.middle_clicked() {
                close = Some(view.clone());
            }
            // "Send to ▸ <dock>" — the first-cut way to move a view between docks.
            tab.context_menu(|ui| {
                ui.label(view.title());
                ui.separator();
                for target in [Dock::Left, Dock::Right, Dock::Bottom] {
                    if ui.button(format!("Send to {target:?}")).clicked() {
                        send = Some(target);
                        ui.close();
                    }
                }
            });
        }
    });
    // Apply the clicks outside the closure: they borrow `self`.
    if let Some(index) = select {
        self.layout.dock_mut(dock).active = index;
    }
    if let Some(view) = close {
        self.layout.toggle(view, dock);            // toggling off removes it
    }
    if let Some(target) = send {
        if let Some(view) = views.get(active) {
            self.layout.move_to(view, target);
        }
    }
}
```

The existing `outliner`/`inspector` bodies move over almost unchanged; their
`ui.heading("Outliner")` lines go away, since the **tab** now names the view (its
title on hover).

## The status bar

The status bar is where the user drives the docks. Zed's `StatusBar` keeps a
`left_items` and a `right_items` list, each an arbitrary renderable item, and the
workspace adds items by type (`add_left_item`, `insert_item_after`, ...). We take
the **left/right split and the "an item is just a small widget with a tooltip"**
shape, and drop the rest (the GPUI `AnyView` handles, the ARIA toolbar, the
`HideStatusItem` settings-writing trait).

```rust
fn status_bar(&mut self, ui: &mut egui::Ui) {
    egui::Panel::bottom("noctis_status").show(ui, |ui| {
        ui.horizontal(|ui| {
            // One toggle per view: the left cluster.
            let mut toggle = None;
            for view in self.views() {           // built-ins + extension panels
                let on = self.layout.is_open(&view);
                // The button is always an icon (a default when the view names
                // none); the title is hover-only.
                let icon = view.icon().unwrap_or_default();
                if ui
                    .selectable_label(on, icon)
                    .on_hover_text(view.tooltip())   // title (+ optional hint)
                    .clicked()
                {
                    toggle = Some(view);
                }
                ui.separator();
            }
            // The right cluster: read-only status.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(status) = &self.status { ui.label(status); }
                ui.label(format!("{} entities", self.editor.list_entities().len()));
            });
            if let Some(view) = toggle {
                self.layout.toggle(view, view.default_dock());
            }
        });
    });
}
```

`self.views()` returns the built-in list plus one `View::Extension(id)` per panel
the host drew this frame, so a newly-loaded extension's panel gets a button the
same frame.

For v1 the status bar is the **only** way to open and close a view. A `View ▸`
menu in the title bar and a command palette are a **separate story**, deliberately
out of scope here (*Decisions*).

## The title bar

A custom window frame (the egui `custom_window_frame` example is the template):
the OS chrome is hidden and the driver paints the bar itself.

```rust
fn title_bar(&mut self, ui: &mut egui::Ui) {
    egui::Panel::top("noctis_titlebar")
        .frame(title_frame(ui.style()))          // window fill + a bottom rule
        .show(ui, |ui| {
            let rect = ui.max_rect();
            let bar = ui.interact(rect, egui::Id::unique(), egui::Sense::click_and_drag());
            if bar.drag_started_by(egui::PointerButton::Primary) {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if bar.double_clicked() {
                let max = ui.input(|i| i.viewport().maximized.unwrap_or(false));
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
            }
            // Left: the app name and the menus (left-to-right).
            // Right: the window buttons (min / max / close).
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("✕").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                // … maximise/minimise …
            });
        });
}
```

The current `noctis_menu` strip *becomes* the title bar's content: the scene
picker moves left next to the app name, and the tool/frame/Undo/Redo/Save
controls sit after it. So this frees a whole row rather than adding one.

**macOS:** the example keeps the native traffic lights with
`with_fullsize_content_view(true)` and repositions them with
`eframe::Frame::set_traffic_lights_position`. That helper is **eframe-only** — we
run bare `egui-winit`/`egui-wgpu`, so it is not available. Since macOS is
**unsupported** (see `AGENTS.md`), the honest choice is: **on macOS, do not use a
custom frame** — leave the native decorations. The custom frame is Windows/Linux
only, gated on `cfg(not(target_os = "macos"))` where relevant.

## Platform changes

Two changes in `crates/platform`; both are additive and do not touch a game.

### 1. A window can ask for no decorations

`WindowConfig` gains one field:

```rust
/// Whether the OS draws the window's decorations (title bar, border, buttons).
/// The editor turns this off to draw its own; a game keeps the default.
pub decorations: bool,   // default: true
```

`create_window` passes it to winit (`.with_decorations(config.decorations)`), and
`crates/editor/src/cli.rs::window_config` sets `decorations: false`. Round-tripped
corners/transparency (`with_transparent(true)`) are a **follow-up**, not v1: a
transparent surface needs an alpha present format and a transparent clear, which
is a renderer change out of scope here — the first cut is square corners.

### 2. The host must apply egui's viewport commands

This one is load-bearing and was verified against egui-winit 0.36: the title bar
moves/minimises/closes the window by sending `ViewportCommand`s
(`ui.ctx().send_viewport_cmd(...)`), which land in `FullOutput::viewport_output`
for the ROOT viewport. **`egui_winit::State::handle_platform_output` does not
apply them** — it destructures the platform output and ignores `commands`. eframe
normally does this step; we are not eframe, so the host must:

```rust
// in WindowHost::redraw, after `ctx.run_ui`:
let mut info = self.egui.as_ref().unwrap().viewport_info.clone();
egui_winit::update_viewport_info(&mut info, &egui.ctx, &window, /*is_init=*/ false);
let commands = output
    .viewport_output
    .get(&egui::ViewportId::ROOT)
    .map(|viewport| viewport.commands.clone())
    .unwrap_or_default();
let mut actions = Vec::new();
egui_winit::process_viewport_commands(&egui.ctx, &mut info, commands, &window, &mut actions);
```

with `ViewportInfo` stored on `EguiLayer` (seeded by `update_viewport_info(...,
is_init = true)` right after the window exists). `process_viewport_commands`
handles `StartDrag`, `Maximized`, `Minimized`, `Close`, `Decorations`, and the
rest; the returned `ActionRequested`s (Cut/Copy/Paste screenshot) can be ignored
in v1. Without this, the custom title bar's buttons silently do nothing on a
platform whose integration we wrote ourselves.

## Extensions in docks

An extension draws **panels** (`extension::Panel { title, widgets }`), which today
become floating `egui::Window`s in `EditorDriver::extension_panels`. Under the new
model each panel is a `View::Extension(id)` and lives in a dock like anything
else, so the user places extension UI wherever they want.

This needs one refactor: **separate "collect the panels" from "render them"**.
`host.draw_panels(&editor, &interactions)` runs the extension and must run
**once** per frame (it has side effects and returns the interactions the host
records). So the frame becomes:

1. `let panels = host.draw_panels(...)` at the top of `ui` — store `Vec<Panel>` on
   the driver for the frame.
2. Draw docks; a `View::Extension(id)` renders from the stored panel.
3. `host.apply_pending(...)` and flush logs, as today.

Identity: `Panel` today is keyed by index (`(panel, widget)` in `Interactions`,
and the wasm host already offsets panel ids per context — see
`.notes/extensions.md`). An **index is not stable** across an extension reload, so
for dock placement and tab identity the panel gets a **stable id**: an
`extension::Panel` carries an `id: String` in addition to its display `title`
(`"{extension_id}:{title}"`), and `View::Extension` is keyed on that. The
`Interactions` channel keeps its indices; only the view identity changes.

The id is `"{extension_id}:{title}"` (*Decisions*): the extension's manifest `id`,
then the panel title it passed to `noctis.ui.panel`. Renaming a panel is therefore
a **new** panel — the old dock slot is dropped and the new one opens by default.
**P4 amends `.notes/extensions.md`**: its UI section says a panel "registers an
egui window" and the WASM `ui_panel` import "Register a panel titled by those
bytes" — both become a **dockable** panel.

## Persistence

The layout — which views are open, their dock and tab order, dock sizes — should
persist, or a docked editor is annoying to restart. Proposal: a small
`layout.json` next to the extension config (the user config dir), written on
change and read at startup, defaulting to today's layout (Outliner left,
Inspector right, console/extensions closed). **Dock sizes** are the wrinkle: egui
stores panel sizes in its own memory keyed by the panel id, and we do not enable
egui's persistence feature, so sizes reset each run unless we read them back
ourselves (`Layout` remembers each dock's last size and feeds it to
`Panel::default_size`). This is a *nice-to-have*; the first cut can ship without
persistence and add it once the shape settles.

## Migration plan

Each step keeps the workspace green and is testable on its own.

- **P1 — the layout model, no chrome.** Add `View`/`Dock`/`DockState`/`Layout`,
  replace the four `show_*` booleans, and draw the three docks with the existing
  outliner/inspector/console/extensions bodies. Keep the current top menu strip.
  Update the tests that poke `show_console`/`show_extensions`.
- **P2 — the status bar.** Move the toggles from the menu strip to a bottom
  status bar; add the view list and the left/right clusters.
- **P3 — the title bar.** Add `WindowConfig::decorations`, apply viewport commands
  in `platform`, move the menu strip into the title bar, add the window buttons.
- **P4 — extension panels as views.** Split `draw_panels` from the rendering, add
  stable panel ids, drop the floating extension windows.
- **P5 — polish.** "Send to ▸ dock" tab menu, layout persistence, tab drag.

P3 is the only step that touches `platform`; P1/P2/P4 are `crates/editor` only.

## Tests

The windowed loop is not unit-tested, but these headless tests exist and drive
`ui` directly, so they carry over:

- `editor_labels_are_not_selectable` — unchanged (the style is set at the top of
  `ui`).
- `the_console_and_extensions_screen_show_without_a_host` — **update**: set the
  layout to open `View::Console` / `View::Extensions` instead of the removed
  booleans. Panels draw in one pass (no floating-window sizing frame), so the
  second-frame dance can likely go.

New tests worth adding, following the `frame_texts` shape already in the file:

- **Layout logic** (pure, no egui): toggling a view adds it to its default dock
  and selects it; toggling again removes it; a view is in exactly one dock;
  "send to" moves it and empties the old dock; a dock is visible iff non-empty.
- **Status bar ↔ docks** (headless): opening `View::Outliner` and running a frame
  paints the outliner's entities; the status-bar button reflects the open state.
- A `platform` unit test for the decorations flag (`window_config_defaults`
  extends to assert `decorations`).

## Decisions

Resolved before implementation:

1. **Tabs, not stacked sections.** A dock with several views shows a **tab strip**
   (the Zed model); stacked, collapsible sections were rejected.
2. **Status-bar only in v1.** The status bar is the one surface that opens and
   closes views. A `View ▸` menu and a command palette are a **separate story**,
   out of scope here.
3. **Extension panel id = `"{extension_id}:{title}"`** — the manifest `id` and the
   panel title. A title change is a new panel, and `.notes/extensions.md`'s UI
   section is amended in P4 (see *Extensions in docks*).
4. **BDock starts hidden and empty.** It appears when its first view opens; the
   console stays closed by default, as today, with the status bar as the
   affordance.

## Status

- **P1 shipped.** `View` / `Dock` / `DockState` / `Layout` live in
  `crates/editor/src/window.rs`; the four `show_*` booleans are gone. The three
  docks draw around the central viewport — left, right, then bottom (claimed
  after the sides, so it sits between them) — each with a tab strip and its active
  view. The outliner, inspector, console and Extensions screen are all views; the
  console and the Extensions screen are no longer floating windows.
- **P2 shipped.** The status bar is a bottom strip claimed *before* the docks, so
  it spans the full width under the bottom dock. It carries one icon toggle per
  view on the left (the icon is always drawn; the title shows only on hover) and
  the entity count plus the last save/scene note on the right. The toggles have
  left the menu strip. `Icon` is implemented as a monochrome-glyph placeholder
  with a default (`⛶`) for a view that names none.
- **P3 shipped.** The title bar is a top panel (`noctis_titlebar`) hosting the app
  name and the menus (moved off the old `noctis_menu` strip) plus — on a
  custom-frame platform — the window buttons (`🗙`/`⛶`/`−`). Its background drags
  the window and a double-click toggles maximize. `WindowConfig` gained
  `decorations` (default `true`): the editor sets it `false` on Windows/Linux and
  keeps the native chrome on macOS. `crates/platform` now applies the frame's
  `ViewportCommand`s with `egui_winit::process_viewport_commands` (egui-winit's own
  `handle_platform_output` does not — eframe normally does), and presets the
  window's `viewport_info`.
- **P4 shipped.** `extension::Panel` carries a stable `id`
  (`"{extension_id}:{title}"`); the JS and WASM hosts stamp it, and wasm `load`
  takes the owning extension id. A panel is a `View::Extension { id, title }` that
  opens in the bottom dock the first time it appears and is dropped if its
  extension goes away; `draw_panels` runs once per frame (at the top of `ui`) and
  the docks render the panels it produced. The floating extension windows are gone.
- **P5 open:** the polish (send-to-dock menu, persistence, tab drag).
