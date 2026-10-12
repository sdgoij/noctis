//! The `examples/reverie` level, run the way the game runs it.
//!
//! `crates/project` proves the scene *loads*; this proves it *plays*: the
//! player falls from its spawn, lands on the tile grid (the generated script's
//! own collision, ported from the original `player.rs`), and walks when a key is
//! held — with the real Slag engine and no script errors.

use std::path::PathBuf;

use ecs::assets::AssetRegistry;
use ecs::components::{Camera, LocalTransform, Name};
use ecs::resources::Key;
use ecs::scene::Scene;
use ecs::world::World;
use platform::AppDriver;
use project::{AssetPath, Project};
use render::{RenderError, Renderer, RendererConfig};
use runtime::{Runtime, RuntimeConfig};
use script::engine_host::EngineHost;

/// The example project directory, from the crate root.
fn reverie_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join("reverie")
}

/// Load a scene with the project's assets, the way the runtime does.
fn load(path: &str) -> (Scene, AssetRegistry) {
    let project = Project::open(reverie_dir()).expect("the reverie project opens");
    let imported = project.import_assets().expect("assets import");
    assert!(
        imported.errors.is_empty(),
        "every reverie asset imports: {:#?}",
        imported.errors
    );
    let file = project.file_for(&AssetPath::parse(path).expect("a valid asset path"));
    let json = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let scene =
        Scene::from_json(&json, &imported.registry).unwrap_or_else(|e| panic!("load {path}: {e}"));
    (scene, imported.registry)
}

/// The local transform of the (single) entity named `name`.
fn transform(runtime: &Runtime<EngineHost>, name: &str) -> LocalTransform {
    let world = &runtime.scene().world;
    let entity = world
        .entities_with::<Name>()
        .into_iter()
        .find(|e| world.get::<Name>(*e).is_some_and(|n| n.as_str() == name))
        .unwrap_or_else(|| panic!("no entity named {name:?}"));
    *world
        .get::<LocalTransform>(entity)
        .unwrap_or_else(|| panic!("{name:?} has no transform"))
}

#[test]
fn the_player_falls_lands_and_walks() {
    let (scene, assets) = load("scenes/level1.scene.json");
    let config = RuntimeConfig {
        audio: false,
        ..RuntimeConfig::default()
    };
    let mut runtime = Runtime::from_parts(scene, assets, config, EngineHost::new().expect("Slag"));

    // The development loader, as `noctis run` wires it (so `load_scene` resolves).
    let dir = reverie_dir();
    runtime.set_scene_loader(Box::new(move |path: &str| {
        std::fs::read_to_string(dir.join(path)).map_err(|error| error.to_string())
    }));

    let start = transform(&runtime, "Player").translation;

    // Fall for a second: the player drops from its spawn and lands.
    runtime.run_headless(90, 1.0 / 60.0);
    let landed = transform(&runtime, "Player").translation;
    assert!(
        landed.y < start.y - 1.0,
        "the player did not fall: {start:?} -> {landed:?}"
    );
    assert!(
        landed.y > 0.0 && landed.y < 17.0 * 16.0,
        "the player fell through the floor: {landed:?}"
    );

    // It settles: another second barely moves it vertically.
    runtime.run_headless(60, 1.0 / 60.0);
    let settled = transform(&runtime, "Player").translation;
    assert!(
        (settled.y - landed.y).abs() < 1.0,
        "the player did not settle on the ground: {landed:?} -> {settled:?}"
    );

    // Holding left walks it toward the level's west wall (and no ledge).
    runtime
        .input_mut()
        .expect("the runtime takes input")
        .press_key(Key::letter('A').expect("A is a letter key"));
    runtime.run_headless(30, 1.0 / 60.0);
    let walked = transform(&runtime, "Player").translation;
    assert!(
        walked.x < settled.x - 2.0,
        "the player did not walk: {settled:?} -> {walked:?}"
    );

    assert!(
        runtime.script().host().errors().is_empty(),
        "the level reported script errors: {:?}",
        runtime.script().host().errors()
    );
}

/// A headless renderer, or `None` when no adapter exists (the test then skips).
fn headless(width: u32, height: u32) -> Option<Renderer> {
    match Renderer::headless(RendererConfig {
        width,
        height,
        ..Default::default()
    }) {
        Ok(renderer) => Some(renderer),
        Err(RenderError::NoAdapter) => {
            eprintln!("skipping GPU test: no adapter");
            None
        }
        Err(other) => panic!("unexpected renderer error: {other}"),
    }
}

/// The RGBA pixels of a frame.
fn pixels(frame: &render::Frame) -> &[[u8; 4]] {
    frame.rgba.as_chunks::<4>().0
}

/// The channel-wise maximum difference between two pixels.
fn diff(a: [u8; 4], b: [u8; 4]) -> u8 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

/// The level must actually rasterize geometry over the empty background — the
/// check a black window would fail (e.g. a camera pointing the wrong way).
#[test]
fn the_level_renders_visible_geometry() {
    let (scene, assets) = load("scenes/level1.scene.json");
    let Some(mut renderer) = headless(256, 256) else {
        return;
    };

    // Learn the empty-scene background colour (clear colour + tonemap).
    let mut empty = World::new();
    let empty_camera = empty.spawn_with(Camera::default());
    let empty_assets = AssetRegistry::with_defaults();
    let background = *pixels(
        &renderer
            .render_frame(&empty, &empty_assets, empty_camera)
            .expect("render an empty scene"),
    )
    .first()
    .expect("a frame has pixels");

    let camera = render::first_active_camera(&scene.world).expect("the level has a camera");
    let frame = renderer
        .render_frame(&scene.world, &assets, camera)
        .expect("render the level");

    let total = (frame.width * frame.height) as usize;
    let detailed = pixels(&frame)
        .iter()
        .filter(|pixel| diff(**pixel, background) > 12)
        .count();
    assert!(
        detailed > total / 50,
        "the level drew almost nothing over the background: {detailed}/{total} pixels"
    );
}
