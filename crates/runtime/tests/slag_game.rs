//! End-to-end proof that a game run by the runtime executes scripts.
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
use script::engine_host::EngineHost;

fn config() -> RuntimeConfig {
    RuntimeConfig {
        audio: false,
        ..RuntimeConfig::default()
    }
}

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

    let mut runtime = Runtime::with_host(scene, config(), EngineHost::new().expect("hosts"));
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

/// A WebAssembly module (the same fixture the `script` crate builds in
/// `tests/wasm_abi.rs`): `on_update` moves its own entity to `(1, 2, 3)`, spawns
/// a child named `Child` and moves it to `(4, 5, 6)`, and logs `"hi"`.
///
/// It travels as a `wasm:` descriptor (hex bytes) because the engine's script
/// channel is text; a project would `script::wasm_host::encode` a `.wasm` file.
const WASM_SCRIPT: &str = "wasm:0061736d0100000001180460047f7d7d7d0060027f7f0060027f7f017f60027f7d00023303066e6f637469730c7365745f706f736974696f6e0000066e6f63746973036c6f670001066e6f6374697305737061776e0002030201030503010001071602066d656d6f72790200096f6e5f75706461746500030a3a013801017f2000430000803f430000004043000040401000411041051002210220024300008040430000a040430000c04010004100410210010b0b12020041000b0268690041100b054368696c64";

#[test]
fn a_game_runs_a_webassembly_script_end_to_end() {
    let mut scene = Scene::new("WasmGame");
    let entity = scene.world.spawn();
    scene.world.insert(
        entity,
        LocalTransform::from_translation(vec3(0.0, 0.0, 0.0)),
    );
    scene.world.insert(entity, Script::from_source(WASM_SCRIPT));

    let mut runtime = Runtime::with_host(scene, config(), EngineHost::new().expect("hosts"));
    runtime.run_headless(1, 1.0 / 60.0);

    assert!(
        runtime.script().host().errors().is_empty(),
        "unexpected script errors: {:?}",
        runtime.script().host().errors()
    );

    // The module moved its own entity, and the child it spawned.
    let translation = runtime
        .scene()
        .world
        .get::<LocalTransform>(entity)
        .expect("the scripted entity still exists")
        .translation;
    assert!(
        vec3_approx_eq(translation, vec3(1.0, 2.0, 3.0), 1e-4),
        "expected the module to move its entity to (1,2,3), got {translation:?}"
    );

    let child = runtime
        .scene()
        .world
        .entities_with::<ecs::components::Name>()
        .into_iter()
        .find(|e| {
            runtime
                .scene()
                .world
                .get::<ecs::components::Name>(*e)
                .is_some_and(|n| n.as_str() == "Child")
        })
        .expect("the module spawned a child named Child");
    let child_translation = runtime
        .scene()
        .world
        .get::<LocalTransform>(child)
        .expect("the child has a transform")
        .translation;
    assert!(
        vec3_approx_eq(child_translation, vec3(4.0, 5.0, 6.0), 1e-4),
        "expected the child at (4,5,6), got {child_translation:?}"
    );
}
