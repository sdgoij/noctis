//! Open a project from a real directory on disk.

use std::path::PathBuf;

use project::{MANIFEST_FILE, Project};

/// A unique temp directory for one test.
fn temp_project(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "noctis-project-{tag}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn write(path: &std::path::Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent dir");
    }
    std::fs::write(path, contents).expect("write");
}

#[test]
fn opens_a_project_and_discovers_its_scenes_and_prefabs() {
    let dir = temp_project("open");
    write(
        &dir.join(MANIFEST_FILE),
        r#"
[project]
name = "MyGame"

[project.settings]
default_scene = "scenes/main.scene.json"
"#,
    );
    write(&dir.join("scenes/main.scene.json"), "{}");
    write(&dir.join("scenes/sub/level.scene.json"), "{}");
    write(&dir.join("scenes/notes.txt"), "not a scene");
    write(&dir.join("prefabs/robot.prefab.json"), "{}");

    let project = Project::open(&dir).expect("open");
    assert_eq!(project.name(), "MyGame");
    assert_eq!(project.default_scene(), Some("scenes/main.scene.json"));
    assert_eq!(project.scenes_dir(), dir.join("scenes"));

    let scenes: Vec<String> = project
        .discover_scenes()
        .expect("scenes")
        .into_iter()
        .map(|p| p.as_str().to_string())
        .collect();
    assert_eq!(
        scenes,
        vec!["scenes/main.scene.json", "scenes/sub/level.scene.json"],
        "sorted, recursive, and only *.scene.json"
    );

    let prefabs: Vec<String> = project
        .discover_prefabs()
        .expect("prefabs")
        .into_iter()
        .map(|p| p.as_str().to_string())
        .collect();
    assert_eq!(prefabs, vec!["prefabs/robot.prefab.json"]);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_project_with_no_asset_folders_discovers_nothing() {
    let dir = temp_project("empty");
    write(&dir.join(MANIFEST_FILE), "[project]\nname = \"Bare\"\n");

    let project = Project::open(&dir).expect("open");
    assert!(project.discover_scenes().expect("scenes").is_empty());
    assert!(project.discover_prefabs().expect("prefabs").is_empty());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_invalid_manifest_fails_to_open() {
    let dir = temp_project("bad");
    write(&dir.join(MANIFEST_FILE), "[project]\nname = \"\"\n");

    let error = Project::open(&dir).unwrap_err();
    assert!(
        matches!(error, project::ProjectError::Manifest(_)),
        "{error:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
