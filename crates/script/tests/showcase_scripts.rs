//! The example project's JavaScript, executed by the real Slag engine.
//!
//! The scripts under `examples/showcase/scripts` are a deliverable, so this test
//! loads each one under its project asset path, drives a world for a couple of
//! seconds, presses the keys the scripts listen for, and requires the engine to
//! report no script errors and to have seen the effects.

use std::path::PathBuf;

use ecs::prelude::*;
use script::api::ScriptCommand;
use script::engine_host::EngineHost;
use script::runtime::ScriptRuntime;

/// The example project's scripts directory.
fn scripts_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join("showcase")
        .join("scripts")
}

/// The `.js` files in the example, sorted for determinism.
fn script_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(scripts_dir())
        .expect("the showcase ships a scripts/ directory")
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("js"))
        .collect();
    files.sort();
    files
}

fn key(name: &str) -> Key {
    Key::from_name(name).unwrap_or_else(|| panic!("{name:?} is a known key name"))
}

#[test]
fn every_showcase_script_runs_without_errors() {
    let files = script_files();
    assert!(
        !files.is_empty(),
        "the showcase ships JavaScript behaviours"
    );

    // Register each script under its project asset path, and attach it to one
    // entity — exactly how the runtime resolves `Script::from_path`.
    let mut assets = AssetRegistry::with_builtins();
    let mut world = World::new();
    for file in &files {
        let name = file
            .file_name()
            .and_then(|n| n.to_str())
            .expect("a UTF-8 file name");
        let asset_path = format!("scripts/{name}");
        let source = std::fs::read_to_string(file).expect("the script is readable");
        assets.scripts.insert_named(
            &asset_path,
            ScriptAsset {
                name: asset_path.clone(),
                source,
            },
        );
        let entity = world.spawn();
        world.insert(entity, Name::new(asset_path.clone()));
        world.insert(entity, LocalTransform::default());
        world.insert(entity, Script::from_path(&asset_path));
    }

    let host = EngineHost::new().expect("the Slag engine starts");
    let mut runtime = ScriptRuntime::with_host(host);
    runtime.run_start(&mut world, &assets);

    let dt = 1.0 / 60.0;
    for _ in 0..10 {
        runtime.run_update(&mut world, &assets, dt);
    }

    // Press each key the scripts listen for (one frame held, one released) so the
    // input, spawn, prefab, and destroy paths all execute.
    for name in ["Space", "KeyW", "KeyR", "KeyX"] {
        runtime.input_mut().press_key(key(name));
        runtime.run_update(&mut world, &assets, dt);
        runtime.input_mut().release_key(key(name));
        runtime.run_update(&mut world, &assets, dt);
    }

    let errors = runtime.host().errors().to_vec();
    assert!(errors.is_empty(), "script errors:\n{}", errors.join("\n"));
    // One instance per file, plus whatever the scripts spawned at runtime.
    assert!(
        runtime.instance_count() >= files.len(),
        "every script is running"
    );

    // The scripts reached the engine: spawning, transformation, prefabs,
    // destruction, behaviour attachment, and logging all appear in the log.
    let log = runtime.command_log();
    let saw = |predicate: fn(&ScriptCommand) -> bool| log.iter().any(predicate);
    for (name, predicate) in [
        (
            "log()",
            (|c| matches!(c, ScriptCommand::Log { .. })) as fn(&ScriptCommand) -> bool,
        ),
        ("spawn()", |c| matches!(c, ScriptCommand::Spawn { .. })),
        ("destroy()", |c| matches!(c, ScriptCommand::Destroy { .. })),
        ("translate()", |c| {
            matches!(c, ScriptCommand::Translate { .. })
        }),
        ("set_position()", |c| {
            matches!(c, ScriptCommand::SetPosition { .. })
        }),
        ("set_rotation_euler()", |c| {
            matches!(c, ScriptCommand::SetRotationEuler { .. })
        }),
        ("add_script()", |c| {
            matches!(c, ScriptCommand::AddScript { .. })
        }),
        ("instantiate_prefab()", |c| {
            matches!(c, ScriptCommand::InstantiatePrefab { .. })
        }),
    ] {
        assert!(saw(predicate), "{name} never reached the engine");
    }
}
