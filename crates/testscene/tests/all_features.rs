//! End-to-end integration tests for the all-features scene.
//!
//! Everything here is guarded so the suite passes on a machine with no GPU and
//! no audio device: physics, scripting, and audio run on the CPU, and the
//! renderer test skips cleanly when [`render::RenderError::NoAdapter`] is
//! returned.

use audio::AudioEngine;
use ecs::components::component_index::ComponentKind;
use ecs::prelude::*;
use math::vec3;
use physics::{PhysicsConfig, PhysicsWorld};
use render::{
    RenderError, Renderer, RendererConfig, extract_camera, extract_draw_list, first_active_camera,
};
use script::{RecordingHost, ScriptApi, ScriptRuntime};
use testscene::{build, coverage, sample_scripts};

/// Find the first entity with the given [`Name`].
fn entity_named(world: &World, name: &str) -> Option<Entity> {
    world
        .query::<Name>()
        .find(|(_, n)| n.0 == name)
        .map(|(e, _)| e)
}

#[test]
fn scene_uses_every_component_kind() {
    let present = coverage();
    for kind in ComponentKind::ALL {
        assert!(
            present.contains(&kind.name()),
            "scene is missing component kind {}",
            kind.name()
        );
    }
}

#[test]
fn scene_has_transform_hierarchy_with_rotation_and_scale() {
    let (scene, _assets) = build();
    let world = &scene.world;

    let root = entity_named(world, "Root").expect("Root");
    let child = entity_named(world, "ChildA").expect("ChildA");
    let grandchild = entity_named(world, "Grandchild").expect("Grandchild");

    assert_eq!(world.get::<Parent>(child).unwrap().0, root);
    assert_eq!(world.get::<Parent>(grandchild).unwrap().0, child);

    let local = world.get::<LocalTransform>(root).unwrap();
    assert!(
        !math::vec3_approx_eq(
            local.rotation.rotate_vec3(vec3(0.0, 0.0, 1.0)),
            vec3(0.0, 0.0, 1.0),
            1e-3
        ),
        "root rotation should be non-identity"
    );
    assert!(
        !math::vec3_approx_eq(local.scale, vec3(1.0, 1.0, 1.0), 1e-3),
        "root scale should be non-uniform"
    );

    // Global transforms were propagated, so the grandchild's world position
    // reflects the whole chain.
    let g = world.get::<GlobalTransform>(grandchild).unwrap();
    assert!(g.matrix[3].w.abs() > 0.5 || g.position().y > 0.5);
}

#[test]
fn scene_serializes_and_round_trips() {
    let (scene, assets) = build();
    let json = scene.to_json(&assets);
    let parsed = Scene::from_json(&json, &assets).expect("scene parses");

    let before = scene.to_desc(&assets);
    let after = parsed.to_desc(&assets);
    assert_eq!(before, after, "scene -> json -> scene must be stable");
    assert!(before.entities.len() > 10);
}

#[test]
fn prefab_instantiation_adds_the_expected_entities() {
    let (scene, assets) = build();
    let desc = scene.to_desc(&assets);
    let prefab = Prefab::from_scene_desc("copy", desc.clone());

    let mut world = World::new();
    let created = prefab.instantiate(&mut world, &assets, None);
    assert_eq!(created.len(), desc.entities.len());
    assert_eq!(world.entities().len(), desc.entities.len());

    // The prefab survives a JSON round-trip too.
    let reparsed = Prefab::from_json(&prefab.to_json()).expect("prefab parses");
    let mut world2 = World::new();
    assert_eq!(
        reparsed.instantiate(&mut world2, &assets, None).len(),
        desc.entities.len()
    );
}

#[test]
fn physics_bodies_fall_and_floor_is_fixed() {
    let (mut scene, _assets) = build();
    let ball = entity_named(&scene.world, "Ball").expect("Ball");
    let floor = entity_named(&scene.world, "Floor").expect("Floor");

    let mut physics = PhysicsWorld::new(PhysicsConfig::default());
    physics.sync_from_world(&scene.world);
    assert!(physics.body_count() >= 5, "expected the sandbox bodies");

    let ball_start = scene
        .world
        .get::<GlobalTransform>(ball)
        .unwrap()
        .position()
        .y;
    let floor_start = scene
        .world
        .get::<GlobalTransform>(floor)
        .unwrap()
        .position()
        .y;

    for _ in 0..120 {
        physics.step(&mut scene.world, 1.0 / 60.0);
    }

    let ball_end = scene
        .world
        .get::<GlobalTransform>(ball)
        .unwrap()
        .position()
        .y;
    let floor_end = scene
        .world
        .get::<GlobalTransform>(floor)
        .unwrap()
        .position()
        .y;

    assert!(
        ball_end < ball_start,
        "dynamic body should fall: {ball_start} -> {ball_end}"
    );
    assert!(
        (floor_end - floor_start).abs() < 1e-4,
        "fixed floor must not move: {floor_start} -> {floor_end}"
    );

    // A downward ray from above the floor hits something real.
    let hit = physics
        .raycast(vec3(0.0, 9.0, 0.0), vec3(0.0, -1.0, 0.0), 100.0)
        .expect("ray should hit the floor or a resting body");
    let hit_entity = hit.entity.expect("hit should map back to an entity");
    assert!(scene.world.is_alive(hit_entity));
    assert!((hit.point.y - floor_start).abs() < 3.0);
}

#[test]
fn script_runtime_runs_on_update_over_the_scene() {
    let (mut scene, assets) = build();

    let mut host = RecordingHost::new();
    host.on("on_update", |_api: &mut ScriptApi<'_, '_>| {});
    let mut runtime = ScriptRuntime::new(host);

    runtime.run_update(&mut scene.world, &assets, 1.0 / 60.0);

    assert!(
        runtime.instance_count() >= 3,
        "expected the scene's scripts to be adopted"
    );
    assert!(runtime.host().count_calls("on_update") > 0);
}

#[test]
fn script_can_spawn_an_entity_into_the_world() {
    let (mut scene, assets) = build();

    let mut host = RecordingHost::new();
    host.on("on_update", |api: &mut ScriptApi<'_, '_>| {
        api.spawn(Some("SpawnedByScript".into()));
    });
    let mut runtime = ScriptRuntime::new(host);

    let before = scene.world.entities().len();
    runtime.run_update(&mut scene.world, &assets, 1.0 / 60.0);

    assert!(scene.world.entities().len() > before);
    assert!(
        entity_named(&scene.world, "SpawnedByScript").is_some(),
        "the spawned entity should be present"
    );
}

#[test]
fn script_set_position_round_trips() {
    let (mut scene, assets) = build();
    let target = vec3(7.0, 8.0, 9.0);

    let mut host = RecordingHost::new();
    host.on("on_update", move |api: &mut ScriptApi<'_, '_>| {
        if let Some(entity) = api.current_entity() {
            api.set_position(entity, target);
            assert_eq!(api.get_position(entity), target);
        }
    });
    let mut runtime = ScriptRuntime::new(host);

    runtime.run_update(&mut scene.world, &assets, 1.0 / 60.0);

    assert!(
        scene
            .world
            .query::<LocalTransform>()
            .any(|(_, t)| t.translation == target),
        "no entity ended up at the scripted position"
    );
}

#[test]
fn sample_scripts_are_attached_and_registered() {
    let (scene, assets) = build();
    let scripts = sample_scripts();
    assert_eq!(scripts.len(), 3);
    for (file, _) in &scripts {
        let path = format!("scripts/{file}");
        assert!(
            assets.scripts.get_by_path(&path).is_some(),
            "missing {path}"
        );
    }

    // Both attachment styles are represented.
    let by_path = scene
        .world
        .query::<Script>()
        .filter(|(_, s)| s.path.as_deref() == Some("scripts/spawner.js"))
        .count();
    let by_source = scene
        .world
        .query::<Script>()
        .filter(|(_, s)| s.source.is_some())
        .count();
    assert!(by_path >= 1, "expected a path-based script");
    assert!(by_source >= 1, "expected an inline script");
}

#[test]
fn audio_engine_tracks_playing_sources() {
    let (scene, assets) = build();
    let mut engine = AudioEngine::silent();
    engine.update(&scene.world, &assets);

    assert!(
        engine.sound_count() >= 2,
        "both playing sources should be tracked"
    );

    let spatial = entity_named(&scene.world, "SpatialSound").expect("SpatialSound");
    let handle = engine
        .handle_for_entity(spatial)
        .expect("spatial sound tracked");
    assert!(engine.is_playing(handle));

    let (left, right) = engine.channel_gains(handle).expect("pan gains");
    assert!((0.0..=1.0).contains(&left), "left gain {left}");
    assert!((0.0..=1.0).contains(&right), "right gain {right}");

    let attenuation = engine.sound_attenuation(handle).expect("attenuation");
    assert!(
        (0.0..=1.0).contains(&attenuation),
        "attenuation {attenuation}"
    );

    let volume = engine.effective_volume(handle).expect("effective volume");
    assert!(volume >= 0.0 && volume.is_finite(), "volume {volume}");
}

#[test]
fn render_extracts_a_sorted_non_empty_draw_list() {
    let (scene, assets) = build();
    let camera = first_active_camera(&scene.world).expect("an active camera");
    let view = extract_camera(&scene.world, camera, 640, 360).expect("camera view");
    let list = extract_draw_list(&scene.world, &assets, &view);

    assert!(!list.is_empty(), "the scene should produce draw items");

    assert!(
        list.opaque
            .windows(2)
            .all(|w| w[0].distance <= w[1].distance),
        "opaque items must be sorted front-to-back"
    );
    assert!(
        list.transparent
            .windows(2)
            .all(|w| w[0].distance >= w[1].distance),
        "transparent items must be sorted back-to-front"
    );
    assert!(
        !list.transparent.is_empty(),
        "the alpha-blended prop should be present"
    );

    // Hidden/inactive entities must never appear.
    for item in list.iter() {
        assert!(
            render::draw_list::is_renderable(&scene.world, item.entity),
            "non-renderable entity in the draw list"
        );
    }
}

#[test]
fn render_headless_frame_or_skip() {
    let (scene, assets) = build();
    let camera = first_active_camera(&scene.world).expect("an active camera");

    let config = RendererConfig {
        width: 64,
        height: 64,
        ..RendererConfig::default()
    };

    match Renderer::headless(config) {
        Ok(mut renderer) => {
            let frame = renderer
                .render_frame(&scene.world, &assets, camera)
                .expect("render a frame");
            assert_eq!(frame.width, 64);
            assert_eq!(frame.height, 64);
            assert_eq!(frame.rgba.len(), 64 * 64 * 4);
        }
        Err(RenderError::NoAdapter) => {
            eprintln!("skipping GPU render test: no adapter available");
        }
        Err(other) => panic!("unexpected renderer error: {other}"),
    }
}
