//! The `examples/showcase` project, imported and loaded the way the engine does.
//!
//! The example project is a first-class deliverable, so it is the integration
//! spine for the project model: this test opens it, imports every asset, loads
//! every scene and prefab, and requires the showcase scene to exercise *every*
//! built-in component kind. It also bakes the project's generated font, so a
//! broken font or a dangling asset reference fails `cargo test --workspace`
//! rather than only showing up when someone opens the project.

use std::path::PathBuf;

use ecs::components::component_index::ComponentKind;
use ecs::prelude::*;
use project::{Project, referenced_files, scene_references};

/// The example project directory, from the crate root.
fn showcase_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join("showcase")
}

fn open() -> (Project, project::ImportedAssets) {
    let project = Project::open(showcase_dir()).expect("the showcase project opens");
    let imported = project.import_assets().expect("assets import");
    assert!(
        imported.errors.is_empty(),
        "every showcase asset must import: {:#?}",
        imported.errors
    );
    (project, imported)
}

/// The component kinds present in a loaded world, mirroring
/// `testscene::coverage_of`.
fn coverage_of(world: &World) -> Vec<&'static str> {
    let mut present = Vec::new();
    for kind in ComponentKind::ALL {
        let has = match kind {
            ComponentKind::LocalTransform => world.count::<LocalTransform>() > 0,
            ComponentKind::GlobalTransform => world.count::<GlobalTransform>() > 0,
            ComponentKind::Name => world.count::<Name>() > 0,
            ComponentKind::Active => world.count::<Active>() > 0,
            ComponentKind::Visible => world.count::<Visible>() > 0,
            ComponentKind::Camera => world.count::<Camera>() > 0,
            ComponentKind::Light => world.count::<Light>() > 0,
            ComponentKind::MeshRenderer => world.count::<MeshRenderer>() > 0,
            ComponentKind::RigidBody => world.count::<RigidBody>() > 0,
            ComponentKind::Collider => world.count::<Collider>() > 0,
            ComponentKind::AudioSource => world.count::<AudioSource>() > 0,
            ComponentKind::Script => world.count::<Script>() > 0,
            ComponentKind::Text => world.count::<Text>() > 0,
        };
        if has {
            present.push(kind.name());
        }
    }
    present
}

#[test]
fn every_scene_loads_and_bundles() {
    let (project, imported) = open();
    let scenes = project.scenes().expect("the scene list resolves");
    assert_eq!(scenes.len(), 3, "three scenes are listed: {scenes:#?}");

    for scene in &scenes {
        let file = project.file_for(&scene.path);
        let json = std::fs::read_to_string(&file)
            .unwrap_or_else(|e| panic!("read {}: {e}", file.display()));
        let loaded = Scene::from_json(&json, &imported.registry)
            .unwrap_or_else(|e| panic!("load {}: {e}", scene.path.as_str()));
        assert!(
            !loaded.world.entities().is_empty(),
            "{} is empty",
            scene.name
        );

        // `noctis build` bundles a scene's references through the import index;
        // a scene that cannot resolve every reference cannot be shipped.
        let desc = serde_json::from_str(&json).expect("scene parses for the bundler");
        referenced_files(&desc, &imported.lookup)
            .unwrap_or_else(|e| panic!("{} cannot be bundled: {e}", scene.path.as_str()));
    }
}

#[test]
fn the_showcase_scene_covers_every_component_kind() {
    let (project, imported) = open();
    let file = project
        .file_for(&project::AssetPath::parse("scenes/showcase.scene.json").expect("valid path"));
    let json = std::fs::read_to_string(&file).expect("read the showcase scene");
    let scene = Scene::from_json(&json, &imported.registry).expect("load the showcase scene");

    let present = coverage_of(&scene.world);
    let missing: Vec<&str> = ComponentKind::ALL
        .iter()
        .map(|kind| kind.name())
        .filter(|name| !present.contains(name))
        .collect();
    assert!(
        missing.is_empty(),
        "the showcase scene is missing: {missing:?}"
    );
}

#[test]
fn the_menu_scene_is_two_dimensional() {
    let (project, imported) = open();
    let file =
        project.file_for(&project::AssetPath::parse("scenes/menu.scene.json").expect("valid path"));
    let json = std::fs::read_to_string(&file).expect("read the menu scene");
    let scene = Scene::from_json(&json, &imported.registry).expect("load the menu scene");
    assert!(scene.is_2d(), "the menu scene is a 2D scene");
}

#[test]
fn every_prefab_loads_and_resolves_its_references() {
    let (project, imported) = open();
    let prefabs = project.load_prefabs().expect("prefabs load");
    assert_eq!(prefabs.len(), 2, "two prefabs ship with the project");
    for prefab in &prefabs {
        assert!(
            !prefab.scene.entities.is_empty(),
            "{} is empty",
            prefab.name
        );
        let references = scene_references(&prefab.scene);
        referenced_files(&prefab.scene, &imported.lookup).unwrap_or_else(|e| {
            panic!("prefab {} has an unresolvable reference: {e}", prefab.name)
        });
        // A script path is the only non-built-in reference the prefabs carry.
        for reference in references {
            assert!(
                reference.starts_with('$')
                    || reference.starts_with("primitives/")
                    || imported.lookup.contains_key(reference),
                "prefab {} references {reference:?}",
                prefab.name
            );
        }
    }
}

#[test]
fn the_generated_font_bakes() {
    let (_project, imported) = open();
    let font = imported
        .registry
        .fonts
        .get_by_path("assets/fonts/NoctisSans.ttf")
        .expect("the showcase font imported");
    assert!(!font.data.is_empty());

    // Baking exercises the whole glyph pipeline (the font parser, outlines, and
    // metrics), not just the file signature `is_font_data` checks.
    let atlas = render::text::FontAtlas::bake(
        &font.data,
        render::text::DEFAULT_PX_SIZE,
        &render::text::DEFAULT_CHARS.chars().collect::<Vec<_>>(),
    )
    .expect("the generated font bakes");
    assert!(atlas.ascent > 0.0 && atlas.line_height > 0.0);
    for ch in ['A', 'Z', '0', '9'] {
        let glyph = atlas.glyph(ch);
        assert!(
            glyph.size[0] > 0.0 && glyph.size[1] > 0.0,
            "glyph {ch:?} has no coverage"
        );
    }
    assert_eq!(atlas.glyph(' ').size, [0.0, 0.0], "a space draws nothing");
}
