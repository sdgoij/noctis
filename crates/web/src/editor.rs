//! The browser **editor** entry (`wasm32`): open a project from the browser's
//! Origin Private File System and run the editor on the canvas.
//!
//! It reuses the web host, whose egui overlay (`platform/src/web_egui.rs`) draws
//! the same docks, title bar and status bar as the native window. The project lives
//! in OPFS: on the first run the bundle is **unpacked** into it, and Save writes the
//! store back (see `.notes/web.md`).

use ecs::reflect::ComponentRegistry;
use platform::{DEFAULT_CANVAS_ID, WindowConfig};
use project::{AssetPath, Project};
use vfs::ZipStore;
use wasm_bindgen::prelude::*;

/// The OPFS directory the editor's project lives in.
const PROJECT_DIR: &str = "noctis-project";

/// The browser editor entry point: open `bundle_url` in `canvas_id`.
///
/// The bundle seeds OPFS on the first run; afterwards the OPFS copy is opened.
/// Gated on the `entry` feature, like the client's `start`, so a generated client
/// (which supplies its own `#[wasm_bindgen]` entry) does not export it.
#[cfg(feature = "entry")]
#[wasm_bindgen]
pub fn start_editor(canvas_id: &str, bundle_url: &str) {
    run_editor_bundle(canvas_id, bundle_url);
}

/// Run the editor on the project in OPFS (seeding it from `bundle_url` on the
/// first run). Returns immediately; the editor runs until the page unloads.
pub fn run_editor_bundle(canvas_id: &str, bundle_url: &str) {
    // Install the console logger first: opening the project's assets is where a
    // failure is most likely, and it runs before the host is built.
    platform::init_logging();
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

/// Open the OPFS project, build an [`Editor`](::editor::Editor) from it, and run
/// the windowed editor on the web host.
async fn open(canvas_id: &str, bundle_url: &str) -> Result<(), String> {
    // The first run unpacks the bundle into OPFS; later runs open the OPFS copy.
    let mut files = vfs::opfs::load_tree(PROJECT_DIR)
        .await
        .map_err(|error| format!("failed to read OPFS: {error}"))?;
    if !files.contains_key("noctis.toml") {
        let bytes = crate::fetch(bundle_url).await?;
        let zip =
            ZipStore::new(&bytes).map_err(|error| format!("not a readable bundle: {error}"))?;
        files =
            vfs::snapshot(&zip).map_err(|error| format!("failed to read the bundle: {error}"))?;
        vfs::opfs::save_tree(PROJECT_DIR, &files)
            .await
            .map_err(|error| format!("failed to unpack into OPFS: {error}"))?;
    }
    let project = Project::open_in(Box::new(vfs::MemoryStore::from_files(files)), PROJECT_DIR)
        .map_err(|error| format!("failed to open the project: {error}"))?;

    let imported = project
        .import_assets()
        .map_err(|error| format!("failed to import the project's assets: {error}"))?;
    for error in &imported.errors {
        log::warn!(
            "asset '{}' failed to import: {}",
            error.path.as_str(),
            error.message
        );
    }

    // No plugin on the web (`.notes/web.md`): an empty component registry.
    let components = ComponentRegistry::new();
    let mut editor = ::editor::Editor::with_component_registry(components);
    *editor.assets_mut() = imported.registry;
    for prefab in project
        .load_prefabs()
        .map_err(|error| format!("failed to load the project's prefabs: {error}"))?
    {
        editor.add_prefab(prefab);
    }

    // Open the default scene through the project's store, and remember its file so
    // Save writes back to the same place.
    let scene = project.default_scene().map(str::to_string);
    let asset = scene
        .as_ref()
        .map(|scene| AssetPath::parse(scene))
        .transpose()
        .map_err(|error| error.to_string())?;
    if let Some(asset) = &asset {
        editor
            .open_in(project.store(), asset.file_path())
            .map_err(|error| format!("failed to open the scene: {error}"))?;
    }
    let scene_path = asset.map(|asset| project.root().join(asset.file_path()));

    // Save writes the store back to OPFS: snapshot it, then persist asynchronously.
    // Exporting hands the bundled `.zip` to a browser download.
    let driver = ::editor::window::EditorDriver::new(editor)
        .with_project(Some(project))
        .with_scene_path(scene_path)
        .with_save_hook(|store| {
            let files = vfs::snapshot(store).unwrap_or_default();
            wasm_bindgen_futures::spawn_local(async move {
                if let Err(error) = vfs::opfs::save_tree(PROJECT_DIR, &files).await {
                    log::error!("noctis editor: failed to save to OPFS: {error}");
                }
            });
        })
        .with_download_hook(|name, bytes| {
            if let Err(error) = platform::browser_download(name, bytes) {
                log::error!("noctis editor: download failed: {error}");
            }
        });

    let config = WindowConfig {
        canvas_id: canvas_id.to_string(),
        ..WindowConfig::default()
    };
    platform::run(driver, config).map_err(|error| format!("failed to start the web host: {error}"))
}
