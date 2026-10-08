//! End-to-end tests for the native WebAssembly script host.
//!
//! Compiled only with the `slag` feature:
//!
//! ```text
//! cargo test -p script --features slag
//! ```
//!
//! The fixtures are built with a tiny inline encoder so the tests need no
//! toolchain and no binary blobs in the tree.

#![cfg(feature = "slag")]

use ecs::assets::AssetRegistry;
use ecs::components::{LocalTransform, Name, Script};
use ecs::world::{Entity, World};
use math::{vec3, vec3_approx_eq};
use script::engine_host::EngineHost;
use script::wasm_host::{self, WasmHost};
use script::{ScriptCommand, ScriptRuntime};

const I32: u8 = 0x7f;
const F32: u8 = 0x7d;

// ---------------------------------------------------------------------------
// A minimal WebAssembly encoder: just the sections the fixtures use.
// ---------------------------------------------------------------------------

fn leb(mut value: u32, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn section(id: u8, content: &[u8], out: &mut Vec<u8>) {
    out.push(id);
    leb(content.len() as u32, out);
    out.extend_from_slice(content);
}

fn name(text: &str, out: &mut Vec<u8>) {
    leb(text.len() as u32, out);
    out.extend_from_slice(text.as_bytes());
}

fn func_type(params: &[u8], results: &[u8], out: &mut Vec<u8>) {
    out.push(0x60);
    leb(params.len() as u32, out);
    out.extend_from_slice(params);
    leb(results.len() as u32, out);
    out.extend_from_slice(results);
}

fn import(module: &str, field: &str, type_index: u32, out: &mut Vec<u8>) {
    name(module, out);
    name(field, out);
    out.push(0x00); // import kind: function
    leb(type_index, out);
}

fn header() -> Vec<u8> {
    vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]
}

/// A module with an `(i32, f32) -> ()` import — an ABI signature violation.
fn wrong_signature_module() -> Vec<u8> {
    let mut out = header();
    let mut types = Vec::new();
    leb(1, &mut types);
    func_type(&[I32, F32], &[], &mut types);
    section(1, &types, &mut out);

    let mut imports = Vec::new();
    leb(1, &mut imports);
    // `set_position` is ABI-typed `(i32, f32, f32, f32) -> ()`.
    import("noctis", "set_position", 0, &mut imports);
    section(2, &imports, &mut out);
    out
}

/// A module exercising the ABI: `on_update` moves its own entity, spawns a named
/// child, moves it, and logs a byte string from memory.
fn demo_module() -> Vec<u8> {
    let mut out = header();

    // types: 0 (i32,f32,f32,f32)->(), 1 (i32,i32)->(), 2 (i32,i32)->i32, 3 (i32,f32)->()
    let mut types = Vec::new();
    leb(4, &mut types);
    func_type(&[I32, F32, F32, F32], &[], &mut types);
    func_type(&[I32, I32], &[], &mut types);
    func_type(&[I32, I32], &[I32], &mut types);
    func_type(&[I32, F32], &[], &mut types);
    section(1, &types, &mut out);

    // imports: set_position -> 0, log -> 1, spawn -> 2
    let mut imports = Vec::new();
    leb(3, &mut imports);
    import("noctis", "set_position", 0, &mut imports);
    import("noctis", "log", 1, &mut imports);
    import("noctis", "spawn", 2, &mut imports);
    section(2, &imports, &mut out);

    // functions: one, of type 3 (on_update)
    let mut functions = Vec::new();
    leb(1, &mut functions);
    leb(3, &mut functions);
    section(3, &functions, &mut out);

    // memory: 1 page
    let mut memory = Vec::new();
    leb(1, &mut memory);
    memory.push(0x00); // limits: min only
    leb(1, &mut memory);
    section(5, &memory, &mut out);

    // exports: memory (index 0) and on_update (function index 3, after 3 imports)
    let mut exports = Vec::new();
    leb(2, &mut exports);
    name("memory", &mut exports);
    exports.push(0x02);
    leb(0, &mut exports);
    name("on_update", &mut exports);
    exports.push(0x00);
    leb(3, &mut exports);
    section(7, &exports, &mut out);

    // code: the on_update body
    let mut body = Vec::new();
    leb(1, &mut body); // one local declaration entry...
    leb(1, &mut body); // ...of one...
    body.push(I32); // ...i32 local
    body.extend_from_slice(&[
        0x20, 0x00, // local.get 0 (handle)
        0x43, 0x00, 0x00, 0x80, 0x3f, // f32.const 1
        0x43, 0x00, 0x00, 0x00, 0x40, // f32.const 2
        0x43, 0x00, 0x00, 0x40, 0x40, // f32.const 3
        0x10, 0x00, // call set_position
        0x41, 0x10, 0x41, 0x05, 0x10, 0x02, // i32.const 16; i32.const 5; call spawn
        0x21, 0x02, // local.set 2 (the child handle)
        0x20, 0x02, // local.get 2
        0x43, 0x00, 0x00, 0x80, 0x40, // f32.const 4
        0x43, 0x00, 0x00, 0xa0, 0x40, // f32.const 5
        0x43, 0x00, 0x00, 0xc0, 0x40, // f32.const 6
        0x10, 0x00, // call set_position
        0x41, 0x00, 0x41, 0x02, 0x10, 0x01, // i32.const 0; i32.const 2; call log
        0x0b, // end
    ]);
    let mut code = Vec::new();
    leb(1, &mut code);
    leb(body.len() as u32, &mut code);
    code.extend_from_slice(&body);
    section(10, &code, &mut out);

    // data: "hi" at 0, "Child" at 16
    let mut data = Vec::new();
    leb(2, &mut data);
    data.push(0x00);
    data.extend_from_slice(&[0x41, 0x00, 0x0b]); // i32.const 0
    leb(2, &mut data);
    data.extend_from_slice(b"hi");
    data.push(0x00);
    data.extend_from_slice(&[0x41, 0x10, 0x0b]); // i32.const 16
    leb(5, &mut data);
    data.extend_from_slice(b"Child");
    section(11, &data, &mut out);

    out
}

/// A module that reads its own transform, adds `1` to every component, and
/// writes it back — exercising the multi-value vector getters.
fn getter_module() -> Vec<u8> {
    let mut out = header();

    // types: 0 (i32)->(f32,f32,f32), 1 (i32,f32,f32,f32)->(), 2 (i32,f32)->()
    let mut types = Vec::new();
    leb(3, &mut types);
    func_type(&[I32], &[F32, F32, F32], &mut types);
    func_type(&[I32, F32, F32, F32], &[], &mut types);
    func_type(&[I32, F32], &[], &mut types);
    section(1, &types, &mut out);

    let mut imports = Vec::new();
    leb(2, &mut imports);
    import("noctis", "get_position", 0, &mut imports);
    import("noctis", "set_position", 1, &mut imports);
    section(2, &imports, &mut out);

    let mut functions = Vec::new();
    leb(1, &mut functions);
    leb(2, &mut functions);
    section(3, &functions, &mut out);

    let mut exports = Vec::new();
    leb(1, &mut exports);
    name("on_update", &mut exports);
    exports.push(0x00);
    leb(2, &mut exports);
    section(7, &exports, &mut out);

    let mut body = Vec::new();
    leb(1, &mut body); // one local declaration entry...
    leb(3, &mut body); // ...of three...
    body.push(F32); // ...f32 locals
    body.extend_from_slice(&[
        0x20, 0x00, // local.get 0
        0x10, 0x00, // call get_position -> x y z
        0x21, 0x04, // local.set 4 (z)
        0x21, 0x03, // local.set 3 (y)
        0x21, 0x02, // local.set 2 (x)
        0x20, 0x00, // local.get 0
        0x20, 0x02, 0x43, 0x00, 0x00, 0x80, 0x3f, 0x92, // x + 1
        0x20, 0x03, 0x43, 0x00, 0x00, 0x80, 0x3f, 0x92, // y + 1
        0x20, 0x04, 0x43, 0x00, 0x00, 0x80, 0x3f, 0x92, // z + 1
        0x10, 0x01, // call set_position
        0x0b, // end
    ]);
    let mut code = Vec::new();
    leb(1, &mut code);
    leb(body.len() as u32, &mut code);
    code.extend_from_slice(&body);
    section(10, &code, &mut out);

    out
}

/// A module that checks a key and, when it is held, moves its entity.
fn input_module() -> Vec<u8> {
    let mut out = header();

    // types: 0 (i32,i32)->i32, 1 (i32,f32,f32,f32)->(), 2 (i32,f32)->()
    let mut types = Vec::new();
    leb(3, &mut types);
    func_type(&[I32, I32], &[I32], &mut types);
    func_type(&[I32, F32, F32, F32], &[], &mut types);
    func_type(&[I32, F32], &[], &mut types);
    section(1, &types, &mut out);

    let mut imports = Vec::new();
    leb(2, &mut imports);
    import("noctis", "key_down", 0, &mut imports);
    import("noctis", "set_position", 1, &mut imports);
    section(2, &imports, &mut out);

    let mut functions = Vec::new();
    leb(1, &mut functions);
    leb(2, &mut functions);
    section(3, &functions, &mut out);

    let mut memory = Vec::new();
    leb(1, &mut memory);
    memory.push(0x00);
    leb(1, &mut memory);
    section(5, &memory, &mut out);

    let mut exports = Vec::new();
    leb(2, &mut exports);
    name("memory", &mut exports);
    exports.push(0x02);
    leb(0, &mut exports);
    name("on_update", &mut exports);
    exports.push(0x00);
    leb(2, &mut exports);
    section(7, &exports, &mut out);

    let mut body = Vec::new();
    leb(0, &mut body); // no locals
    body.extend_from_slice(&[
        0x41, 0x00, 0x41, 0x04, 0x10, 0x00, // i32.const 0; i32.const 4; call key_down("KeyW")
        0x04, 0x40, // if (no result)
        0x20, 0x00, // local.get 0
        0x43, 0x00, 0x00, 0x00, 0x00, // f32.const 0
        0x43, 0x00, 0x00, 0xa0, 0x40, // f32.const 5
        0x43, 0x00, 0x00, 0x00, 0x00, // f32.const 0
        0x10, 0x01, // call set_position
        0x0b, // end if
        0x0b, // end
    ]);
    let mut code = Vec::new();
    leb(1, &mut code);
    leb(body.len() as u32, &mut code);
    code.extend_from_slice(&body);
    section(10, &code, &mut out);

    let mut data = Vec::new();
    leb(1, &mut data);
    data.push(0x00);
    data.extend_from_slice(&[0x41, 0x00, 0x0b]); // i32.const 0
    leb(4, &mut data);
    data.extend_from_slice(b"KeyW");
    section(11, &data, &mut out);

    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// A module that instantiates a prefab by JSON held in its data segment, then
/// moves its root through the returned handle.
///
/// The embedded document's first entity ("WasmRoot", id 0) is the prefab root.
fn prefab_module() -> Vec<u8> {
    const JSON: &str = concat!(
        "{\"name\":\"Blob\",\"scene\":{\"name\":\"Blob\",\"entities\":[",
        "{\"id\":0,\"name\":\"WasmRoot\",\"transform\":{\"translation\":[0,0,0],\"rotation\":[0,0,0,1],\"scale\":[1,1,1]}},",
        "{\"id\":1,\"name\":\"WasmChild\",\"parent\":0,\"transform\":{\"translation\":[1,1,1],\"rotation\":[0,0,0,1],\"scale\":[1,1,1]}}",
        "]}}"
    );
    let json = JSON.as_bytes();
    let mut out = header();

    // types: 0 (i32,i32,i32)->i32, 1 (i32,f32,f32,f32)->(), 2 (i32,f32)->()
    let mut types = Vec::new();
    leb(3, &mut types);
    func_type(&[I32, I32, I32], &[I32], &mut types);
    func_type(&[I32, F32, F32, F32], &[], &mut types);
    func_type(&[I32, F32], &[], &mut types);
    section(1, &types, &mut out);

    let mut imports = Vec::new();
    leb(2, &mut imports);
    import("noctis", "instantiate_prefab", 0, &mut imports);
    import("noctis", "set_position", 1, &mut imports);
    section(2, &imports, &mut out);

    let mut functions = Vec::new();
    leb(1, &mut functions);
    leb(2, &mut functions);
    section(3, &functions, &mut out);

    let mut memory = Vec::new();
    leb(1, &mut memory);
    memory.push(0x00);
    leb(1, &mut memory);
    section(5, &memory, &mut out);

    let mut exports = Vec::new();
    leb(2, &mut exports);
    name("memory", &mut exports);
    exports.push(0x02);
    leb(0, &mut exports);
    name("on_update", &mut exports);
    exports.push(0x00);
    leb(2, &mut exports);
    section(7, &exports, &mut out);

    // body: root = instantiate_prefab(json, len, -1); set_position(root, 7, 8, 9)
    let mut body = Vec::new();
    leb(0, &mut body); // no locals
    body.extend_from_slice(&[0x41, 0x00]); // i32.const 0 (json ptr)
    body.push(0x41);
    leb(json.len() as u32, &mut body); // i32.const len
    body.extend_from_slice(&[0x41, 0x7f]); // i32.const -1 (no parent)
    body.extend_from_slice(&[0x10, 0x00]); // call instantiate_prefab -> root
    body.extend_from_slice(&[0x43, 0x00, 0x00, 0xe0, 0x40]); // f32.const 7
    body.extend_from_slice(&[0x43, 0x00, 0x00, 0x00, 0x41]); // f32.const 8
    body.extend_from_slice(&[0x43, 0x00, 0x00, 0x10, 0x41]); // f32.const 9
    body.extend_from_slice(&[0x10, 0x01]); // call set_position
    body.push(0x0b); // end
    let mut code = Vec::new();
    leb(1, &mut code);
    leb(body.len() as u32, &mut code);
    code.extend_from_slice(&body);
    section(10, &code, &mut out);

    let mut data = Vec::new();
    leb(1, &mut data);
    data.push(0x00);
    data.extend_from_slice(&[0x41, 0x00, 0x0b]); // i32.const 0
    leb(json.len() as u32, &mut data);
    data.extend_from_slice(json);
    section(11, &data, &mut out);

    out
}

fn world_with(source: String) -> (World, Entity) {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, Script::from_source(source));
    (world, entity)
}

fn named(world: &World, name: &str) -> Entity {
    world
        .query::<Name>()
        .find(|(_, n)| n.0 == name)
        .map(|(e, _)| e)
        .unwrap_or_else(|| panic!("no entity named {name:?}"))
}

fn translation(world: &World, entity: Entity) -> math::Vec3 {
    world
        .get::<LocalTransform>(entity)
        .expect("entity has a transform")
        .translation
}

#[test]
fn a_wasm_script_drives_the_world() {
    let (mut world, entity) = world_with(wasm_host::encode(&demo_module()));
    let assets = AssetRegistry::new();
    let mut runtime = ScriptRuntime::with_host(WasmHost::new());

    runtime.run_update(&mut world, &assets, 0.016);
    assert!(
        runtime.host().errors().is_empty(),
        "module errors: {:?}",
        runtime.host().errors()
    );

    // The module moved its own entity, its spawned child, and logged a string.
    assert!(vec3_approx_eq(
        translation(&world, entity),
        vec3(1.0, 2.0, 3.0),
        1e-4
    ));
    let child = named(&world, "Child");
    assert!(vec3_approx_eq(
        translation(&world, child),
        vec3(4.0, 5.0, 6.0),
        1e-4
    ));
    assert!(
        runtime
            .command_log()
            .iter()
            .any(|c| matches!(c, ScriptCommand::Log { message, .. } if message == "hi")),
        "expected the module's log"
    );
}

#[test]
fn a_module_with_a_wrong_import_signature_is_rejected() {
    let (mut world, _) = world_with(wasm_host::encode(&wrong_signature_module()));
    let assets = AssetRegistry::new();
    let mut runtime = ScriptRuntime::with_host(WasmHost::new());

    runtime.run_update(&mut world, &assets, 0.016);

    assert!(
        runtime
            .host()
            .errors()
            .iter()
            .any(|error| error.contains("signature")),
        "expected a signature error, got {:?}",
        runtime.host().errors()
    );
}

#[test]
fn one_runtime_runs_javascript_and_wasm_side_by_side() {
    let mut runtime = ScriptRuntime::with_host(EngineHost::new().expect("hosts start"));
    let assets = AssetRegistry::new();
    let mut world = World::new();

    let wasm = world.spawn();
    world.insert(wasm, Script::from_source(wasm_host::encode(&demo_module())));
    let js = world.spawn();
    world.insert(
        js,
        Script::from_source("function on_update(entity, dt) { translate(entity, 0, 7, 0); }"),
    );

    runtime.run_update(&mut world, &assets, 0.016);
    assert!(
        runtime.host().errors().is_empty(),
        "errors: {:?}",
        runtime.host().errors()
    );

    assert!(vec3_approx_eq(
        translation(&world, wasm),
        vec3(1.0, 2.0, 3.0),
        1e-4
    ));
    assert!(vec3_approx_eq(
        translation(&world, js),
        vec3(0.0, 7.0, 0.0),
        1e-4
    ));
}

#[test]
fn a_wasm_script_reads_and_writes_transforms() {
    let (mut world, entity) = world_with(wasm_host::encode(&getter_module()));
    let assets = AssetRegistry::new();
    let mut runtime = ScriptRuntime::with_host(WasmHost::new());
    world.insert(
        entity,
        LocalTransform::from_translation(vec3(10.0, 20.0, 30.0)),
    );

    runtime.run_update(&mut world, &assets, 0.016);

    assert!(
        runtime.host().errors().is_empty(),
        "module errors: {:?}",
        runtime.host().errors()
    );
    // The multi-value getter returned the seeded transform, and the module added
    // one to each component.
    assert!(vec3_approx_eq(
        translation(&world, entity),
        vec3(11.0, 21.0, 31.0),
        1e-4
    ));
}

#[test]
fn a_wasm_script_reads_input() {
    use ecs::resources::Key;

    let (mut world, entity) = world_with(wasm_host::encode(&input_module()));
    world.insert(
        entity,
        LocalTransform::from_translation(vec3(0.0, 0.0, 0.0)),
    );
    let assets = AssetRegistry::new();
    let mut runtime = ScriptRuntime::with_host(WasmHost::new());

    // With the key up, the guarded move does not happen.
    runtime.run_update(&mut world, &assets, 0.016);
    assert!(vec3_approx_eq(
        translation(&world, entity),
        vec3(0.0, 0.0, 0.0),
        1e-4
    ));

    // With it held, it does.
    runtime
        .input_mut()
        .press_key(Key::letter('W').expect("the letter key"));
    runtime.run_update(&mut world, &assets, 0.016);
    assert!(
        runtime.host().errors().is_empty(),
        "module errors: {:?}",
        runtime.host().errors()
    );
    assert!(vec3_approx_eq(
        translation(&world, entity),
        vec3(0.0, 5.0, 0.0),
        1e-4
    ));
}

#[test]
fn a_wasm_script_instantiates_a_prefab_by_root_handle() {
    let (mut world, _) = world_with(wasm_host::encode(&prefab_module()));
    let assets = AssetRegistry::new();
    let mut runtime = ScriptRuntime::with_host(WasmHost::new());

    runtime.run_update(&mut world, &assets, 0.016);
    assert!(
        runtime.host().errors().is_empty(),
        "module errors: {:?}",
        runtime.host().errors()
    );

    // `instantiate_prefab` returned the root's handle; `set_position` through it
    // must land on the real prefab root, while the child is left in place.
    let root = named(&world, "WasmRoot");
    assert!(vec3_approx_eq(
        translation(&world, root),
        vec3(7.0, 8.0, 9.0),
        1e-4
    ));
    let child = named(&world, "WasmChild");
    assert!(vec3_approx_eq(
        translation(&world, child),
        vec3(1.0, 1.0, 1.0),
        1e-4
    ));
}

#[test]
fn a_wasm_file_registers_as_a_script_asset() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("noctis-wasm-{}-{unique}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("demo.wasm");
    std::fs::write(&path, demo_module()).expect("write module");

    // Register the file under a name, then reference it by path from a scene.
    let mut assets = AssetRegistry::new();
    wasm_host::register(&mut assets, "scripts/demo.wasm", &path).expect("register");

    let (mut world, entity) = world_with(String::new());
    world.insert(entity, Script::from_path("scripts/demo.wasm"));
    let mut runtime = ScriptRuntime::with_host(WasmHost::new());
    runtime.run_update(&mut world, &assets, 0.016);

    assert!(
        runtime.host().errors().is_empty(),
        "module errors: {:?}",
        runtime.host().errors()
    );
    assert!(vec3_approx_eq(
        translation(&world, entity),
        vec3(1.0, 2.0, 3.0),
        1e-4
    ));
    assert_eq!(std::fs::read(&path).expect("read back"), demo_module());

    std::fs::remove_dir_all(&dir).ok();
}
