//! The browser **editor** entry (`wasm32`): open a project bundle and run the
//! editor on the canvas.
//!
//! It reuses the web host, whose egui overlay (`platform/src/web_egui.rs`) draws
//! the same docks, title bar and status bar as the native window. Opening a project
//! goes through `vfs`; saving back to OPFS is a later step (see `.notes/web.md`).

use std::path::PathBuf;

use ecs::reflect::ComponentRegistry;
use platform::{DEFAULT_CANVAS_ID, WindowConfig};
use project::{AssetPath, Project};
use vfs::ZipStore;
use wasm_bindgen::prelude::*;

/// The browser editor entry point: open the bundle at `bundle_url` in `canvas_id`.
///
/// Gated on the `entry` feature, like the client's `start`, so a generated client
/// (which supplies its own `#[wasm_bindgen]` entry) does not export it.
#[cfg(feature = "entry")]
#[wasm_bindgen]
pub fn start_editor(canvas_id: &str, bundle_url: &str) {
    run_editor_bundle(canvas_id, bundle_url);
}

/// Run the editor on the project bundle at `bundle_url` (an empty `canvas_id` uses
/// `#noctis-canvas`). Returns immediately; the editor runs until the page unloads.
pub fn run_editor_bundle(canvas_id: &str, bundle_url: &str) {
    let canvas_id = if canvas_id.is_empty() {
        DEFAULT_CANVAS_ID.to_string()
    } else {
        canvas_id.to_string()
    };
    let bundle_url = bundle_url.to_string();
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(error) = open(&canvas_id, &bundle_url).await {
            log::error!("noctis editor: {error}");
        }
    });
}

/// Fetch the bundle, build an [`Editor`](::editor::Editor) from it, and run the
/// windowed editor on the web host.
async fn open(canvas_id: &str, bundle_url: &str) -> Result<(), String> {
    let bytes = crate::fetch(bundle_url).await?;
    let store = ZipStore::new(&bytes).map_err(|error| format!("not a readable bundle: {error}"))?;
    let project = Project::open_in(Box::new(store), format!("noctis-bundle/{canvas_id}"))
        .map_err(|error| format!("failed to open the bundle: {error}"))?;

    let imported = project
        .import_assets()
        .map_err(|error| format!("failed to import the bundle's assets: {error}"))?;
    for error in &imported.errors {
        log::warn!(
            "asset '{}' failed to import: {}",
            error.path.as_str(),
            error.message
        );
    }

    // No plugin on the web (`.notes/web.md`): the component registry stays empty.
    let components = ComponentRegistry::new();
    let mut editor = ::editor::Editor::with_component_registry(components.clone());
    *editor.assets_mut() = imported.registry;
    for prefab in project
        .load_prefabs()
        .map_err(|error| format!("failed to load the project's prefabs: {error}"))?
    {
        editor.add_prefab(prefab);
    }

    // Open the default scene through the project's store (a bundle's zip).
    let scene = project.default_scene().map(str::to_string);
    if let Some(scene) = &scene {
        let asset = AssetPath::parse(scene).map_err(|error| error.to_string())?;
        editor
            .open_in(project.store(), asset.file_path())
            .map_err(|error| format!("failed to open the scene {scene:?}: {error}"))?;
    }

    let config = WindowConfig {
        canvas_id: canvas_id.to_string(),
        ..WindowConfig::default()
    };
    // No `--controls` file and no layout persistence on the web (both use `std::fs`).
    ::editor::window::run(
        editor,
        config,
        scene.map(PathBuf::from),
        Some(project),
        None,
        None,
    )
    .map_err(|error| format!("failed to start the web host: {error}"))
}
