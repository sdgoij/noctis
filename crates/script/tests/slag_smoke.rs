//! Smoke test for the real Slag JavaScript backend.
//!
//! It drives the full engine-side lifecycle ([`ScriptRuntime`]) through the real
//! JS engine, exercising transforms, spawning, the vector getters/setters, and
//! name lookup, so a break in the Slag binding or the replay bridge fails here.

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

/// A prefab document the script instantiates; its first entity ("JsRoot", id 0)
/// is the prefab root the call should hand back.
const PREFAB_SCRIPT: &str = r#"
    var PREFAB = '{"name":"Blob","scene":{"name":"Blob","entities":[' +
        '{"id":0,"name":"JsRoot","transform":{"translation":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]}},' +
        '{"id":1,"name":"JsChild","parent":0,"transform":{"translation":[1,1,1],"rotation":[0,0,0,1],"scale":[1,1,1]}}' +
        ']}}';
    function on_start(entity, dt) {
        var root = instantiate_prefab(PREFAB, undefined);
        set_position(root, 7, 8, 9);
        set_scale(root, 4, 4, 4);
    }
"#;

#[test]
fn instantiate_prefab_returns_the_root_handle() {
    let (mut world, _) = scene(PREFAB_SCRIPT);
    let mut runtime = ScriptRuntime::with_host(SlagHost::new().expect("slag engine starts"));
    let assets = AssetRegistry::new();

    runtime.run_start(&mut world, &assets);

    // `instantiate_prefab` handed back the root's handle; the writes through it
    // must land on the real, named prefab root (not the child).
    let root = transform(&world, named(&world, "JsRoot"));
    assert!(vec3_approx_eq(root.translation, vec3(7.0, 8.0, 9.0), 1e-4));
    assert!(vec3_approx_eq(root.scale, vec3(4.0, 4.0, 4.0), 1e-4));

    let child = transform(&world, named(&world, "JsChild"));
    assert!(vec3_approx_eq(child.translation, vec3(1.0, 1.0, 1.0), 1e-4));
    assert!(
        runtime.host().errors().is_empty(),
        "unexpected script errors: {:?}",
        runtime.host().errors()
    );
}

/// Multiple entities created in one call must each get their own handle: a
/// script engine's `f64` numbers cannot represent a raw `EntityId`, so the
/// backend hands out small per-call indices instead.
const MULTI_SPAWN_SCRIPT: &str = "
    function on_start(entity, dt) {
        var a = spawn('A');
        var b = spawn('B');
        var c = spawn('C');
        set_position(a, 1, 0, 0);
        set_position(b, 2, 0, 0);
        set_position(c, 3, 0, 0);
    }
";

#[test]
fn each_spawned_handle_addresses_its_own_entity() {
    let (mut world, _) = scene(MULTI_SPAWN_SCRIPT);
    let mut runtime = ScriptRuntime::with_host(SlagHost::new().expect("slag engine starts"));
    let assets = AssetRegistry::new();

    runtime.run_start(&mut world, &assets);

    for (name, x) in [("A", 1.0), ("B", 2.0), ("C", 3.0)] {
        let moved = transform(&world, named(&world, name)).translation;
        assert!(
            vec3_approx_eq(moved, vec3(x, 0.0, 0.0), 1e-4),
            "{name} should sit at x = {x}, got x = {}",
            moved.x
        );
    }
    assert!(
        runtime.host().errors().is_empty(),
        "unexpected script errors: {:?}",
        runtime.host().errors()
    );
}

/// A hot counting loop. With the `jit` feature on (which the Slag dependency
/// enables) `SlagHost::new` installs the Cranelift hook, so this drives
/// the compiled path; the result also guards correctness.
const JIT_SCRIPT: &str = "
    function on_start(entity, dt) {
        var total = 0;
        for (var i = 0; i < 200000; i++) { total += i; }
        // sum(0..200000) = 19999900000, exact in a double; store a flag so the
        // result survives the engine's f32 transform.
        set_position(entity, total === 19999900000 ? 1 : 0, 0, 0);
    }
";

#[test]
fn hot_loops_run_correctly_with_the_jit_installed() {
    let (mut world, entity) = scene(JIT_SCRIPT);
    let host = SlagHost::new().expect("slag engine starts");
    assert!(
        host.jit_enabled(),
        "the Cranelift JIT hook should have installed"
    );
    let mut runtime = ScriptRuntime::with_host(host);
    let assets = AssetRegistry::new();

    runtime.run_start(&mut world, &assets);

    let x = transform(&world, entity).translation.x;
    assert!((x - 1.0).abs() < 1e-4, "loop result flag = {x}");
    assert!(
        runtime.host().errors().is_empty(),
        "unexpected script errors: {:?}",
        runtime.host().errors()
    );
}

/// A hand-assembled WASM module: `(module (func (export "f") (result i32)
/// i32.const 42))`. Written as byte values so the test needs no toolchain.
const WASM_SCRIPT: &str = "
    function on_start(entity, dt) {
        var bytes = [
            0,97,115,109, 1,0,0,0,
            1,5,1,96,0,1,127,
            3,2,1,0,
            7,5,1,1,102,0,0,
            10,6,1,4,0,65,42,11
        ];
        var module = new WebAssembly.Module(new Uint8Array(bytes));
        var instance = new WebAssembly.Instance(module);
        set_position(entity, instance.exports.f(), 0, 0);
    }
";

#[test]
fn a_script_can_instantiate_a_webassembly_module() {
    let (mut world, entity) = scene(WASM_SCRIPT);
    let mut runtime = ScriptRuntime::with_host(SlagHost::new().expect("slag engine starts"));
    let assets = AssetRegistry::new();

    runtime.run_start(&mut world, &assets);
    assert!(
        runtime.host().errors().is_empty(),
        "unexpected script errors: {:?}",
        runtime.host().errors()
    );

    // The module's `f()` returned 42 and the engine API consumed it.
    let x = transform(&world, entity).translation.x;
    assert!((x - 42.0).abs() < 1e-4, "wasm export result = {x}");
}
