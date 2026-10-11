#![cfg(target_arch = "wasm32")]
//! # web — the Noctis browser client (`wasm32`)
//!
//! A generic wasm client: one export, [`start`], runs a packaged project **bundle**
//! on a `<canvas>`. It fetches the `.zip` (`noctis pack`), opens it through
//! [`Project::open_in`] (a `ZipStore`), imports its assets — decoded from bytes —
//! loads the default scene, builds a [`Runtime`], and hands it to the web host
//! (`platform::run`). The project comes from the **bundle**, not the binary, so
//! one module runs any project. See `.notes/web.md`.
//!
//! Build the artifact with:
//!
//! ```sh
//! cargo build -p web --target wasm32-unknown-unknown
//! wasm-bindgen --target web --out-dir dist target/wasm32-unknown-unknown/debug/web.wasm
//! ```
//!
//! then, from JS:
//!
//! ```js
//! import init, { start } from "./dist/web.js";
//! await init();
//! start("noctis-canvas", "https://example.com/game.zip");
//! ```
//!
//! Slag imports its clock from a wasm `env` module
//! (`slag_host_now_ms`/`slag_host_now_monotonic_ms`), which wasm-bindgen leaves
//! as a bare `import * as … from "env"` — unresolvable in a browser without a map.
//! The host page must provide one; `crates/web/web/index.html` does, pointing
//! `env` at `crates/web/web/env.js`.
//!
//! (No `#![forbid(unsafe_code)]` here: `#[wasm_bindgen]` generates the FFI glue.)

use ecs::assets::AssetRegistry;
use ecs::reflect::ComponentRegistry;
use ecs::scene::Scene;
use platform::{DEFAULT_CANVAS_ID, WindowConfig};
use project::{AssetPath, Project};
use runtime::{Runtime, RuntimeConfig};
use script::engine_host::EngineHost;
use vfs::ZipStore;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

/// Run the project bundle at `bundle_url` in the canvas `canvas_id` (an empty
/// id uses `#noctis-canvas`).
///
/// Returns immediately; the game runs until the page unloads. A failure is logged
/// to the browser console — there is no caller left to return to.
#[wasm_bindgen]
pub fn start(canvas_id: &str, bundle_url: &str) {
    let canvas_id = if canvas_id.is_empty() {
        DEFAULT_CANVAS_ID.to_string()
    } else {
        canvas_id.to_string()
    };
    let bundle_url = bundle_url.to_string();
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(error) = run(&canvas_id, &bundle_url).await {
            log::error!("noctis: {error}");
        }
    });
}

/// Fetch the bundle, open it, and run its default scene.
async fn run(canvas_id: &str, bundle_url: &str) -> Result<(), String> {
    let bytes = fetch(bundle_url).await?;
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

    let scene = load_scene(&project, &imported.registry)?;

    // Gravity and the `[render] environment` (IBL) the manifest names — without
    // the latter the renderer falls back to its white neutral IBL and looks
    // washed out.
    let mut config = RuntimeConfig::default();
    config
        .apply_project(&project, &imported.registry)
        .map_err(|error| format!("failed to apply the project settings: {error}"))?;

    let host =
        EngineHost::new().map_err(|error| format!("failed to start the script engine: {error}"))?;
    let mut runtime: Runtime<EngineHost> =
        Runtime::from_parts(scene, imported.registry, config, host);

    // A script switches scenes with `load_scene`/`teleport_scene`; the path is
    // read through the bundle's store, so its other scenes resolve too. Without a
    // loader the menu's Play is ignored and the game is stuck on the menu.
    let home = project.default_scene().map(str::to_string);
    let bundle = project.clone();
    runtime.set_scene_loader(Box::new(move |path: &str| {
        let asset = AssetPath::parse(path).map_err(|error| error.to_string())?;
        match bundle.read_document(asset.file_path()) {
            Ok(Some(json)) => Ok(json),
            Ok(None) => Err(format!("scene {path:?} is not in the bundle")),
            Err(error) => Err(error.to_string()),
        }
    }));
    // `Esc` pauses to the project's default scene (the menu) and back.
    if let Some(home) = home {
        runtime.set_current_scene(home.clone());
        runtime.set_home_scene(home);
    }

    let config = WindowConfig {
        canvas_id: canvas_id.to_string(),
        ..WindowConfig::default()
    };
    platform::run(runtime, config).map_err(|error| format!("failed to start the web host: {error}"))
}

/// Load the project's default scene through the bundle's store.
fn load_scene(project: &Project, assets: &AssetRegistry) -> Result<Scene, String> {
    let scene = project
        .default_scene()
        .ok_or_else(|| "the bundle's manifest names no default scene".to_string())?;
    let asset = AssetPath::parse(scene).map_err(|error| error.to_string())?;
    let json = project
        .read_document(asset.file_path())
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("scene {scene:?} is not in the bundle"))?;
    Scene::from_json_with(&json, assets, &ComponentRegistry::new())
        .map_err(|error| format!("failed to load the scene {scene:?}: {error}"))
}

/// Fetch `url` and return the body bytes.
async fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let window = web_sys::window().ok_or_else(|| "no window".to_string())?;
    let response = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|_| format!("failed to fetch {url:?}"))?;
    let response: web_sys::Response = response
        .dyn_into()
        .map_err(|_| format!("{url:?} did not return a response"))?;
    if !response.ok() {
        return Err(format!(
            "fetching {url:?} failed: HTTP {}",
            response.status()
        ));
    }
    let body = response
        .array_buffer()
        .map_err(|_| format!("{url:?} has no body"))?;
    let buffer = wasm_bindgen_futures::JsFuture::from(body)
        .await
        .map_err(|_| format!("failed to read the body of {url:?}"))?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}
