//! The `examples/showcase` scenes, run the way a game runs them.
//!
//! `crates/project/tests/showcase.rs` proves the scenes *load*; this proves they
//! *run*. Each scene is driven through the runtime with the real Slag engine and
//! its scripts are required to move the entities they are attached to, without
//! reporting errors — so a change to a component, a script or the engine API that
//! quietly stops the demo moving fails `cargo test --workspace`.

use std::path::PathBuf;

use ecs::assets::AssetRegistry;
use ecs::components::{LocalTransform, Name};
use ecs::scene::Scene;
use math::{vec3, vec3_approx_eq};
use project::{AssetPath, Project};
use runtime::{Runtime, RuntimeConfig};
use script::engine_host::EngineHost;

/// The example project directory, from the crate root.
fn showcase_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join("showcase")
}

/// Load a scene with the project's assets, the way the runtime does.
fn load(path: &str) -> (Scene, AssetRegistry) {
    let project = Project::open(showcase_dir()).expect("the showcase project opens");
    let imported = project.import_assets().expect("assets import");
    assert!(
        imported.errors.is_empty(),
        "every showcase asset imports: {:#?}",
        imported.errors
    );
    let file = project.file_for(&AssetPath::parse(path).expect("a valid asset path"));
    let json = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let scene =
        Scene::from_json(&json, &imported.registry).unwrap_or_else(|e| panic!("load {path}: {e}"));
    (scene, imported.registry)
}

/// Load `path` and run it for `frames` at 60 Hz, asserting the scripts ran clean.
fn run(path: &str, frames: u32) -> Runtime<EngineHost> {
    let (scene, assets) = load(path);
    let config = RuntimeConfig {
        audio: false,
        ..RuntimeConfig::default()
    };
    let mut runtime = Runtime::from_parts(scene, assets, config, EngineHost::new().expect("Slag"));
    runtime.run_headless(frames, 1.0 / 60.0);
    assert!(
        runtime.script().host().errors().is_empty(),
        "{path} reported script errors: {:?}",
        runtime.script().host().errors()
    );
    runtime
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
fn the_showcase_scene_scripts_move_their_entities() {
    let runtime = run("scenes/showcase.scene.json", 60);

    // `spinner.js` rotates the Crate about Y, so a fixed axis no longer maps to
    // itself.
    let spun = transform(&runtime, "Crate")
        .rotation
        .rotate_vec3(vec3(1.0, 0.0, 0.0));
    assert!(
        !vec3_approx_eq(spun, vec3(1.0, 0.0, 0.0), 1e-3),
        "the Crate did not spin"
    );

    // `bobber.js` floats the Hover Torus off its authored height of 2.
    let y = transform(&runtime, "Hover Torus").translation.y;
    assert!(
        (y - 2.0).abs() > 1e-3,
        "the Hover Torus did not bob (y = {y})"
    );
}

#[test]
fn the_menu_scene_spinner_rotates_in_the_plane() {
    let runtime = run("scenes/menu.scene.json", 60);

    // `spin2d.js` rotates about Z, so the screen's up axis tilts.
    let turned = transform(&runtime, "Spinner")
        .rotation
        .rotate_vec3(vec3(0.0, 1.0, 0.0));
    assert!(
        !vec3_approx_eq(turned, vec3(0.0, 1.0, 0.0), 1e-3),
        "the menu Spinner did not rotate"
    );
}

#[test]
fn the_physics_scene_platform_bobs() {
    let runtime = run("scenes/physics.scene.json", 60);

    // `bobber.js` floats the Moving Platform off its authored height of 1.
    let y = transform(&runtime, "Moving Platform").translation.y;
    assert!(
        (y - 1.0).abs() > 1e-3,
        "the Moving Platform did not bob (y = {y})"
    );
}
