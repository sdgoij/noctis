//! Importing a loose file into a project (the drag & drop import step).

use std::path::{Path, PathBuf};

use project::{AssetPath, Project};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "noctis-import-{tag}-{}-{:?}",
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

const MANIFEST: &str = "[project]\nname = \"Import\"\n";

#[test]
fn import_places_a_file_by_kind_and_never_clobbers() {
    let dir = temp_dir("project");
    write(&dir.join("noctis.toml"), MANIFEST.as_bytes());
    let source = temp_dir("source");
    write(&source.join("wood.png"), b"PNG");

    let project = Project::open(&dir).expect("open");

    let first = project
        .import_file(&source.join("wood.png"))
        .expect("import");
    assert_eq!(first, AssetPath::parse("assets/textures/wood.png").unwrap());
    assert!(dir.join("assets/textures/wood.png").is_file());

    // A second import of the same name does not overwrite.
    let second = project
        .import_file(&source.join("wood.png"))
        .expect("import again");
    assert_eq!(
        second,
        AssetPath::parse("assets/textures/wood-1.png").unwrap()
    );
    assert_eq!(
        std::fs::read(dir.join("assets/textures/wood.png")).unwrap(),
        b"PNG"
    );

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&source).ok();
}

#[test]
fn import_routes_by_extension() {
    let dir = temp_dir("kinds");
    write(&dir.join("noctis.toml"), MANIFEST.as_bytes());
    let source = temp_dir("kinds-src");
    write(&source.join("hero.glb"), b"glTF");
    write(&source.join("theme.js"), b"// js");
    write(&source.join("hit.wav"), b"RIFF");

    let project = Project::open(&dir).expect("open");
    assert_eq!(
        project.import_file(&source.join("hero.glb")).unwrap(),
        AssetPath::parse("assets/models/hero.glb").unwrap()
    );
    assert_eq!(
        project.import_file(&source.join("theme.js")).unwrap(),
        AssetPath::parse("scripts/theme.js").unwrap()
    );
    assert_eq!(
        project.import_file(&source.join("hit.wav")).unwrap(),
        AssetPath::parse("assets/audio/hit.wav").unwrap()
    );

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&source).ok();
}

#[test]
fn import_rejects_an_unknown_type() {
    let dir = temp_dir("unknown");
    write(&dir.join("noctis.toml"), MANIFEST.as_bytes());
    let source = temp_dir("unknown-src");
    write(&source.join("mystery.xyz"), b"?");

    let project = Project::open(&dir).expect("open");
    let error = project.import_file(&source.join("mystery.xyz"));
    assert!(error.is_err());

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&source).ok();
}
