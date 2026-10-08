//! The `examples/showcase` scenes, rendered on a headless GPU.
//!
//! This is the visual half of the example's integration test: it loads each
//! scene the way the engine does, renders a frame through the real PBR pipeline
//! (glTF material, shadows, SSAO, text atlas, tonemapping), and requires the
//! frame to contain actual geometry — not an empty clear colour. A scene whose
//! camera is broken, whose geometry fails to extract, or whose font fails to
//! bake produces a blank frame and fails here.
//!
//! Set `NOCTIS_PREVIEWS=<dir>` to also write each frame out as a PNG for
//! eyeballing, e.g.:
//!
//! ```text
//! NOCTIS_PREVIEWS=target/previews cargo test -p project --test render_previews
//! ```

use std::path::PathBuf;

use ecs::scene::Scene;
use render::{Renderer, RendererConfig};

fn showcase_dir() -> PathBuf {
    workspace_root().join("examples").join("showcase")
}

/// The repository root, from this crate's manifest directory.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// How many pixels must differ from the clear colour for the frame to count as
/// "has geometry". A conservative floor: the scenes fill a third of the frame.
const MIN_DIFFERENT_FRACTION: f64 = 0.05;

/// Whether `frame` shows anything beyond its background colour.
fn shows_geometry(frame: &render::Frame) -> bool {
    let background = &frame.rgba[0..4];
    let different = frame
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| {
            pixel
                .iter()
                .zip(background)
                .any(|(a, b)| a.abs_diff(*b) > 2)
        })
        .count();
    let total = (frame.width * frame.height) as usize;
    (different as f64) / (total as f64) >= MIN_DIFFERENT_FRACTION
}

#[test]
fn every_showcase_scene_renders() {
    let root = showcase_dir();
    let project = project::Project::open(&root).expect("the showcase project opens");
    let imported = project.import_assets().expect("assets import");
    assert!(imported.errors.is_empty(), "{:#?}", imported.errors);

    let mut renderer = match Renderer::headless(RendererConfig {
        width: 960,
        height: 540,
        ..RendererConfig::default()
    }) {
        Ok(renderer) => renderer,
        Err(render::RenderError::NoAdapter) => {
            eprintln!("skipping: no graphics adapter available");
            return;
        }
        Err(error) => panic!("renderer: {error}"),
    };

    let previews = std::env::var_os("NOCTIS_PREVIEWS").map(|value| {
        let path = PathBuf::from(value);
        // A relative path is resolved against the repository root, so
        // `NOCTIS_PREVIEWS=target/previews` lands in the workspace target dir
        // rather than the crate directory `cargo test` runs in.
        if path.is_absolute() {
            path
        } else {
            workspace_root().join(path)
        }
    });
    if let Some(dir) = &previews {
        std::fs::create_dir_all(dir).expect("preview dir");
    }

    for (relative, name) in [
        ("scenes/showcase.scene.json", "showcase"),
        ("scenes/physics.scene.json", "physics"),
        ("scenes/menu.scene.json", "menu"),
    ] {
        let json = std::fs::read_to_string(root.join(relative)).expect("read the scene");
        let scene = Scene::from_json(&json, &imported.registry)
            .unwrap_or_else(|e| panic!("load {relative}: {e}"));
        let camera = render::first_active_camera(&scene.world)
            .unwrap_or_else(|| panic!("{relative} has an active camera"));

        let frame = renderer
            .render_frame(&scene.world, &imported.registry, camera)
            .unwrap_or_else(|e| panic!("render {relative}: {e}"));
        assert_eq!(frame.rgba.len(), (frame.width * frame.height * 4) as usize);
        assert!(shows_geometry(&frame), "{relative} rendered a blank frame");

        if let Some(dir) = &previews {
            let image = image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba)
                .expect("frame pixels");
            let path = dir.join(format!("{name}.png"));
            image.save(&path).expect("save the preview");
            eprintln!("wrote {}", path.display());
        }
    }
}
