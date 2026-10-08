//! Smoke test for the real Slag JavaScript backend.
//!
//! This file compiles only when the `slag` feature is on; otherwise the default
//! testable host is used and this is empty. Run it with:
//!
//! ```text
//! cargo test -p script --features slag
//! ```
//!
//! It drives the full engine-side lifecycle ([`ScriptRuntime`]) through the real
//! JS engine, exercising transforms, spawning, the vector getters/setters, and
//! name lookup, so a break in the Slag binding or the replay bridge fails here.

#![cfg(feature = "slag")]

use ecs::assets::AssetRegistry;
use ecs::components::{LocalTransform, Name, Script};
use ecs::world::{Entity, World};
use math::{vec3, vec3_approx_eq};
use script::ScriptRuntime;
use script::slag::SlagHost;

/// A world with one scripted entity.
fn scene(source: &str) -> (World, Entity) {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, Script::from_source(source));
    (world, entity)
}

fn transform(world: &World, entity: Entity) -> LocalTransform {
    world
        .get::<LocalTransform>(entity)
        .copied()
        .expect("the entity has a transform")
}

fn named(world: &World, name: &str) -> Entity {
    world
        .query::<Name>()
        .find(|(_, n)| n.0 == name)
        .map(|(e, _)| e)
        .unwrap_or_else(|| panic!("no entity named {name:?}"))
}

const TRANSFORM_SCRIPT: &str = "
    function on_start(entity, dt) {
        set_scale(entity, 2, 3, 4);
        set_rotation_euler(entity, 0, 0.5, 0);
        log('started');
    }
    function on_update(entity, dt) {
        translate(entity, 0, dt, 0);
        var marker = spawn('Marker');
        set_position(marker, 1, -1, 0.5);
        set_scale(marker, 5, 6, 7);
        log(get_position(marker));
    }
";

#[test]
fn runs_a_full_lifecycle_through_the_real_engine() {
    let (mut world, entity) = scene(TRANSFORM_SCRIPT);
    let mut runtime = ScriptRuntime::with_host(SlagHost::new().expect("slag engine starts"));
    let assets = AssetRegistry::new();

    runtime.run_start(&mut world, &assets);
    runtime.run_update(&mut world, &assets, 0.5);

    // `set_scale` and `set_rotation_euler` (radians) reached the entity.
    let scripted = transform(&world, entity);
    assert!(vec3_approx_eq(scripted.scale, vec3(2.0, 3.0, 4.0), 1e-4));
    let (_, pitch, _) = scripted.rotation.to_euler_xyz();
    assert!((pitch - 0.5).abs() < 1e-3, "pitch = {pitch}");

    // `translate(entity, 0, dt, 0)` moved it up by dt.
    assert!(
        (scripted.translation.y - 0.5).abs() < 1e-4,
        "y = {}",
        scripted.translation.y
    );

    // The spawn + set_position/set_scale reached the real, named entity.
    let marker = transform(&world, named(&world, "Marker"));
    assert!(vec3_approx_eq(
        marker.translation,
        vec3(1.0, -1.0, 0.5),
        1e-4
    ));
    assert!(vec3_approx_eq(marker.scale, vec3(5.0, 6.0, 7.0), 1e-4));

    // `get_position` returned a JSON vector, and the script ran cleanly.
    assert!(
        runtime.command_log().iter().any(|c| matches!(
            c,
            script::ScriptCommand::Log { message, .. } if message.starts_with('[')
        )),
        "get_position should have produced a JSON vector log"
    );
    assert!(
        runtime.host().errors().is_empty(),
        "unexpected script errors: {:?}",
        runtime.host().errors()
    );
}

const LOOKUP_SCRIPT: &str = "
    function on_start(entity, dt) {
        var child = spawn('Target');
        set_position(child, 2, 0, 0);
        var found = find_entity('Target');
        set_scale(found, 3, 3, 3);
    }
";

#[test]
fn find_entity_resolves_to_the_real_entity_after_replay() {
    let (mut world, _) = scene(LOOKUP_SCRIPT);
    let mut runtime = ScriptRuntime::with_host(SlagHost::new().expect("slag engine starts"));
    let assets = AssetRegistry::new();

    runtime.run_start(&mut world, &assets);

    // `find_entity` handed back the placeholder for the spawned entity; the scale
    // written through it must land on the real entity.
    let target = transform(&world, named(&world, "Target"));
    assert!(vec3_approx_eq(target.scale, vec3(3.0, 3.0, 3.0), 1e-4));
    assert!(vec3_approx_eq(
        target.translation,
        vec3(2.0, 0.0, 0.0),
        1e-4
    ));
    assert!(
        runtime.host().errors().is_empty(),
        "unexpected script errors: {:?}",
        runtime.host().errors()
    );
}
