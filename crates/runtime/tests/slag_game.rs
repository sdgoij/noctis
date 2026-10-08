//! End-to-end proof that a game run by the runtime executes JavaScript.
//!
//! Compiled only with the `slag` feature. Run it with:
//!
//! ```text
//! cargo test --manifest-path crates/runtime/Cargo.toml --features slag
//! ```
//!
//! The manifest path is used because `-p runtime` is ambiguous: the Slag engine
//! ships a transitive crate that is also named `runtime`.

#![cfg(feature = "slag")]

use ecs::components::{LocalTransform, Script};
use ecs::scene::Scene;
use math::{vec3, vec3_approx_eq};
use runtime::{Runtime, RuntimeConfig};
use script::slag::SlagHost;

#[test]
fn a_game_runs_javascript_end_to_end() {
    let mut scene = Scene::new("ScriptedGame");
    let entity = scene.world.spawn();
    scene.world.insert(
        entity,
        LocalTransform::from_translation(vec3(0.0, 0.0, 0.0)),
    );
    scene.world.insert(
        entity,
        Script::from_source("function on_update(entity, dt) { translate(entity, 0, 2, 0); }"),
    );

    let mut runtime = Runtime::with_host(
        scene,
        RuntimeConfig {
            audio: false,
            ..RuntimeConfig::default()
        },
        SlagHost::new().expect("slag engine starts"),
    );
    runtime.run_headless(2, 1.0 / 60.0);

    // Two updates of `+2` on y: the JavaScript actually ran inside the game loop.
    let translation = runtime
        .scene()
        .world
        .get::<LocalTransform>(entity)
        .expect("the scripted entity still exists")
        .translation;
    assert!(
        vec3_approx_eq(translation, vec3(0.0, 4.0, 0.0), 1e-4),
        "expected two updates of +2 on y, got {translation:?}"
    );
    assert!(
        runtime.script().host().errors().is_empty(),
        "unexpected script errors: {:?}",
        runtime.script().host().errors()
    );
}
