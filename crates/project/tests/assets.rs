//! The project asset map: scanning, importing, and resolving scene references.

use std::path::{Path, PathBuf};

use ecs::assets::TextureFormat;
use ecs::components::MeshRenderer;
use ecs::scene::Scene;
use project::{AssetKind, Project};

fn temp_project(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "noctis-assets-{tag}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn write(path: &Path, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent dir");
    }
    std::fs::write(path, contents).expect("write");
}

fn manifest() -> &'static str {
    "[project]\nname = \"Assets\"\n[project.settings]\ndefault_scene = \"scenes/main.scene.json\"\n"
}

#[test]
fn scans_textures_and_scripts_and_imports_them_by_path() {
    let dir = temp_project("scan");
    write(&dir.join("noctis.toml"), manifest().as_bytes());
    write(&dir.join("assets/textures/wood.png"), &png_bytes(2, 3));
    write(&dir.join("scripts/player.js"), b"function on_update() {}\n");
    // A sidecar that is not an asset must be ignored.
    write(
        &dir.join("assets/textures/wood.png.import.toml"),
        b"srgb = true\n",
    );

    let project = Project::open(&dir).expect("open");

    let map = project.asset_map().expect("map");
    let kinds: Vec<(String, AssetKind)> = map
        .entries()
        .iter()
        .map(|e| (e.path.as_str().to_string(), e.kind))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("assets/textures/wood.png".to_string(), AssetKind::Texture),
            ("scripts/player.js".to_string(), AssetKind::Script),
        ]
    );

    let imported = project.import_assets().expect("import");
    assert!(imported.errors.is_empty(), "{:?}", imported.errors);
    assert!(
        imported
            .registry
            .textures
            .handle("assets/textures/wood.png")
            .is_some(),
        "the texture resolves by its asset path"
    );
    let texture = imported
        .registry
        .textures
        .get_by_path("assets/textures/wood.png")
        .expect("texture");
    assert_eq!((texture.width, texture.height), (2, 3));
    assert!(
        imported
            .registry
            .scripts
            .handle("scripts/player.js")
            .is_some()
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn importing_a_texture_populates_the_generated_cache() {
    let dir = temp_project("cache");
    write(&dir.join("noctis.toml"), manifest().as_bytes());
    write(&dir.join("assets/textures/wood.png"), &png_bytes(2, 2));

    let project = Project::open(&dir).expect("open");
    let imported = project.import_assets().expect("import");
    assert!(imported.errors.is_empty(), "{:?}", imported.errors);

    // The decoded texture is cached under `.noctis/textures/`.
    let cache = dir.join(project::CACHE_DIR).join("textures");
    assert!(cache.is_dir(), "the cache directory was created");
    assert_eq!(std::fs::read_dir(&cache).expect("read cache").count(), 1);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_sidecar_import_setting_controls_srgb() {
    let dir = temp_project("sidecar");
    write(&dir.join("noctis.toml"), manifest().as_bytes());
    write(&dir.join("assets/textures/data.png"), &png_bytes(2, 2));
    // A data texture (normal map, roughness, ...) is linear, not sRGB.
    write(
        &dir.join("assets/textures/data.png.import.toml"),
        b"srgb = false\n",
    );

    let project = Project::open(&dir).expect("open");
    let imported = project.import_assets().expect("import");
    assert!(imported.errors.is_empty(), "{:?}", imported.errors);

    let texture = imported
        .registry
        .textures
        .get_by_path("assets/textures/data.png")
        .expect("texture imported");
    assert_eq!(texture.format, TextureFormat::Rgba8Unorm);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn imports_a_model_and_resolves_a_scene_mesh_reference() {
    let dir = temp_project("model");
    write(&dir.join("noctis.toml"), manifest().as_bytes());
    write(&dir.join("assets/models/tri.glb"), &triangle_glb());

    let scene_json = r#"{
        "name": "Main",
        "entities": [{
            "id": 0,
            "name": "Tri",
            "transform": { "translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] },
            "components": [{
                "type": "mesh_renderer",
                "mesh": "TriMesh_primitive0",
                "material": "Red",
                "cast_shadows": true,
                "receive_shadows": true
            }]
        }]
    }"#;
    write(&dir.join("scenes/main.scene.json"), scene_json.as_bytes());

    let project = Project::open(&dir).expect("open");
    let imported = project.import_assets().expect("import");
    assert!(imported.errors.is_empty(), "{:?}", imported.errors);

    // Registered under the glTF name (what an editor-saved scene references) and
    // under the fragment path.
    let by_name = imported.registry.meshes.handle("TriMesh_primitive0");
    assert!(by_name.is_some(), "mesh registered under its glTF name");
    assert_eq!(
        imported
            .registry
            .meshes
            .handle("assets/models/tri.glb#mesh/TriMesh_primitive0"),
        by_name,
        "the fragment path aliases the same mesh"
    );
    assert!(imported.registry.materials.handle("Red").is_some());

    // A scene built against the imported registry resolves its mesh reference.
    let scene = Scene::from_json(scene_json, &imported.registry).expect("scene");
    let (_, renderer) = scene
        .world
        .query::<MeshRenderer>()
        .next()
        .expect("the scene has a mesh renderer");
    assert_eq!(
        renderer.mesh,
        by_name.unwrap(),
        "the mesh reference resolved"
    );
    assert!(scene.world.query::<MeshRenderer>().count() >= 1);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_corrupt_model_is_recorded_without_failing_the_import() {
    let dir = temp_project("corrupt");
    write(&dir.join("noctis.toml"), manifest().as_bytes());
    write(&dir.join("assets/models/broken.glb"), b"not a real glb");

    let project = Project::open(&dir).expect("open");
    let imported = project.import_assets().expect("import still succeeds");

    assert_eq!(imported.errors.len(), 1);
    assert_eq!(imported.errors[0].path.as_str(), "assets/models/broken.glb");
    // The registry is still usable (it has the engine defaults).
    assert!(!imported.registry.is_empty());

    std::fs::remove_dir_all(&dir).ok();
}

// -- fixtures ---------------------------------------------------------------

/// A tiny RGBA PNG, via the `image` crate (a dev-dependency).
fn png_bytes(width: u32, height: u32) -> Vec<u8> {
    let mut image = image::RgbaImage::new(width, height);
    image.put_pixel(0, 0, image::Rgba([10, 20, 30, 255]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("encode png");
    bytes.into_inner()
}

/// A minimal binary glTF (GLB) with one triangle, mirroring the renderer's own
/// test fixture.
fn triangle_glb() -> Vec<u8> {
    let positions: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    let normals: [[f32; 3]; 3] = [[0.0, 0.0, 1.0]; 3];
    let uvs: [[f32; 2]; 3] = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
    let indices: [u16; 3] = [0, 1, 2];

    let mut bin = Vec::new();
    for p in positions {
        for c in p {
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    let normal_offset = bin.len();
    for n in normals {
        for c in n {
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    let uv_offset = bin.len();
    for uv in uvs {
        for c in uv {
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    let index_offset = bin.len();
    for i in indices {
        bin.extend_from_slice(&i.to_le_bytes());
    }
    let total = bin.len();

    let json = format!(
        r#"{{
  "asset": {{ "version": "2.0" }},
  "scene": 0,
  "scenes": [ {{ "name": "Main", "nodes": [0] }} ],
  "nodes": [ {{ "name": "Tri", "mesh": 0 }} ],
  "meshes": [ {{ "name": "TriMesh", "primitives": [
     {{ "attributes": {{ "POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2 }}, "indices": 3, "material": 0 }}
  ] }} ],
  "materials": [ {{ "name": "Red", "pbrMetallicRoughness": {{
     "baseColorFactor": [1.0, 0.0, 0.0, 1.0], "metallicFactor": 0.1, "roughnessFactor": 0.7
  }} }} ],
  "buffers": [ {{ "byteLength": {total} }} ],
  "bufferViews": [
    {{ "buffer": 0, "byteOffset": 0, "byteLength": 36 }},
    {{ "buffer": 0, "byteOffset": {normal_offset}, "byteLength": 36 }},
    {{ "buffer": 0, "byteOffset": {uv_offset}, "byteLength": 24 }},
    {{ "buffer": 0, "byteOffset": {index_offset}, "byteLength": 6 }}
  ],
  "accessors": [
    {{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0,0,0], "max": [1,1,0] }},
    {{ "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3" }},
    {{ "bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC2" }},
    {{ "bufferView": 3, "componentType": 5123, "count": 3, "type": "SCALAR" }}
  ]
}}"#
    );
    make_glb(&json, &bin)
}

fn make_glb(json: &str, bin: &[u8]) -> Vec<u8> {
    let mut json_bytes = json.as_bytes().to_vec();
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    let mut bin_bytes = bin.to_vec();
    while !bin_bytes.len().is_multiple_of(4) {
        bin_bytes.push(0);
    }
    let total = 12 + 8 + json_bytes.len() + 8 + bin_bytes.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes()); // "JSON"
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(bin_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004E_4942u32.to_le_bytes()); // "BIN\0"
    out.extend_from_slice(&bin_bytes);
    out
}
