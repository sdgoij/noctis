//! Renaming an asset moves the file and rewrites every reference to it.

use std::path::{Path, PathBuf};

use project::{AssetPath, Project};

fn temp_project(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "noctis-rename-{tag}-{}-{:?}",
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

const MANIFEST: &str = "[project]\nname = \"Rename\"\n\n[project.settings]\ndefault_scene = \"scenes/main.scene.json\"\n";

#[test]
fn rename_moves_the_file_and_rewrites_scene_and_prefab_references() {
    let dir = temp_project("move");
    write(&dir.join("noctis.toml"), MANIFEST);
    write(&dir.join("assets/models/robot.glb"), "glTF-bytes");
    let scene = r#"{
        "name": "Main",
        "entities": [{
            "id": 0, "name": "Robot",
            "transform": { "translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] },
            "components": [{
                "type": "mesh_renderer",
                "mesh": "assets/models/robot.glb#mesh/Torso",
                "material": "assets/models/robot.glb#material/Red",
                "cast_shadows": true, "receive_shadows": true
            }]
        }]
    }"#;
    write(&dir.join("scenes/main.scene.json"), scene);
    let prefab = r#"{ "name": "P", "scene": { "name": "P", "entities": [{
        "id": 0, "transform": { "translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] },
        "components": [{
            "type": "mesh_renderer", "mesh": "assets/models/robot.glb",
            "cast_shadows": true, "receive_shadows": true
        }]
    }] } }"#;
    write(&dir.join("prefabs/robot.prefab.json"), prefab);

    let project = Project::open(&dir).expect("open");
    let from = AssetPath::parse("assets/models/robot.glb").unwrap();
    let to = AssetPath::parse("assets/models/mech.glb").unwrap();
    let report = project.rename_asset(&from, &to).expect("rename");

    // The file moved and the old path is gone.
    assert!(dir.join("assets/models/mech.glb").is_file());
    assert!(!dir.join("assets/models/robot.glb").exists());
    assert_eq!(report.from, dir.join("assets/models/robot.glb"));
    assert_eq!(report.to, dir.join("assets/models/mech.glb"));

    // Both documents were rewritten and reported.
    assert_eq!(
        report.documents,
        vec![
            AssetPath::parse("prefabs/robot.prefab.json").unwrap(),
            AssetPath::parse("scenes/main.scene.json").unwrap(),
        ]
    );
    let scene = std::fs::read_to_string(dir.join("scenes/main.scene.json")).unwrap();
    assert!(
        scene.contains("assets/models/mech.glb#mesh/Torso"),
        "{scene}"
    );
    assert!(
        scene.contains("assets/models/mech.glb#material/Red"),
        "{scene}"
    );
    assert!(!scene.contains("robot.glb"));
    let prefab = std::fs::read_to_string(dir.join("prefabs/robot.prefab.json")).unwrap();
    assert!(prefab.contains("\"assets/models/mech.glb\""), "{prefab}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn rename_rejects_a_missing_source_and_an_existing_destination() {
    let dir = temp_project("reject");
    write(&dir.join("noctis.toml"), MANIFEST);
    write(&dir.join("assets/a.txt"), "a");
    write(&dir.join("assets/b.txt"), "b");
    let project = Project::open(&dir).expect("open");

    let missing = project.rename_asset(
        &AssetPath::parse("assets/nope.txt").unwrap(),
        &AssetPath::parse("assets/c.txt").unwrap(),
    );
    assert!(missing.is_err());

    let collision = project.rename_asset(
        &AssetPath::parse("assets/a.txt").unwrap(),
        &AssetPath::parse("assets/b.txt").unwrap(),
    );
    assert!(collision.is_err());
    // Neither file was disturbed.
    assert!(dir.join("assets/a.txt").is_file());
    assert_eq!(
        std::fs::read_to_string(dir.join("assets/b.txt")).unwrap(),
        "b"
    );

    // A fragment source is rejected.
    let fragment = project.rename_asset(
        &AssetPath::parse("assets/a.txt#x").unwrap(),
        &AssetPath::parse("assets/c.txt").unwrap(),
    );
    assert!(fragment.is_err());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn rename_creates_the_destination_directory() {
    let dir = temp_project("mkdir");
    write(&dir.join("noctis.toml"), MANIFEST);
    write(&dir.join("assets/loose.txt"), "x");
    let project = Project::open(&dir).expect("open");

    project
        .rename_asset(
            &AssetPath::parse("assets/loose.txt").unwrap(),
            &AssetPath::parse("assets/textures/nested/loose.txt").unwrap(),
        )
        .expect("rename");
    assert!(dir.join("assets/textures/nested/loose.txt").is_file());

    std::fs::remove_dir_all(&dir).ok();
}
