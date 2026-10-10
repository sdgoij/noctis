//! Prefabs as project assets: `prefabs/<name>.prefab.json`.

use std::path::{Path, PathBuf};

use ecs::scene::Prefab;
use project::{AssetPath, Project};

fn temp_project(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "noctis-prefab-{tag}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent dir");
    }
    std::fs::write(path, contents).expect("write");
}

const MANIFEST: &str = "[project]\nname = \"Prefabs\"\n";

fn prefab(name: &str) -> Prefab {
    Prefab::from_json(&format!(
        r#"{{ "name": "{name}", "scene": {{ "name": "{name}", "entities": [] }} }}"#
    ))
    .expect("prefab parses")
}

#[test]
fn a_prefab_round_trips_through_the_project() {
    let dir = temp_project("roundtrip");
    write(&dir.join("noctis.toml"), MANIFEST);
    let mut project = Project::open(&dir).expect("open");

    assert_eq!(
        project.prefab_path("Pair").unwrap(),
        AssetPath::parse("prefabs/Pair.prefab.json").unwrap()
    );
    let path = project.save_prefab(&prefab("Pair")).expect("save");
    assert_eq!(path, AssetPath::parse("prefabs/Pair.prefab.json").unwrap());
    assert!(dir.join("prefabs/Pair.prefab.json").is_file());

    project.save_prefab(&prefab("Turret")).expect("save");
    let loaded = project.load_prefabs().expect("load");
    let names: Vec<&str> = loaded.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["Pair", "Turret"]);
    assert_eq!(project.discover_prefabs().unwrap().len(), 2);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn prefab_names_must_be_plain() {
    let dir = temp_project("names");
    write(&dir.join("noctis.toml"), MANIFEST);
    let project = Project::open(&dir).expect("open");

    for bad in ["", "..", "a/b", "a\\b"] {
        assert!(
            project.prefab_path(bad).is_err(),
            "{bad:?} must be rejected"
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_malformed_prefab_document_is_an_error() {
    let dir = temp_project("malformed");
    write(&dir.join("noctis.toml"), MANIFEST);
    write(&dir.join("prefabs/broken.prefab.json"), "{ not json");

    let project = Project::open(&dir).expect("open");
    assert!(project.load_prefabs().is_err());

    std::fs::remove_dir_all(&dir).ok();
}
