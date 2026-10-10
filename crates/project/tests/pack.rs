//! Packaging a project into a `.zip` bundle and reading it back.
//!
//! A bundle is what a browser client opens: the project at a `.zip` root, read
//! through a `ZipStore`. This test bundles `examples/showcase`, reopens it from
//! the archive's bytes, and imports every asset — the whole
//! pack → `ZipStore` → `Project::open_in` → decode path, with no directory
//! backing the reopened project.

use std::path::PathBuf;

use project::Project;
use vfs::{Store, ZipStore};

/// The example project directory, from the crate root.
fn showcase_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join("showcase")
}

/// A unique temp directory for one test.
fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("noctis-pack-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

#[test]
fn a_packed_project_reopens_from_the_archive() {
    let project = Project::open(showcase_dir()).expect("the showcase project opens");
    let bytes = project.bundle().expect("bundle");

    // The archive drops generated directories, so it is project content only.
    let store = ZipStore::new(&bytes).expect("the bundle reads back as a zip");
    assert!(store.exists("noctis.toml"), "the manifest is bundled");
    assert!(!store.is_dir("dist"), "dist/ is excluded");
    assert!(!store.is_dir(".noctis"), ".noctis/ is excluded");

    // Reopen from the archive bytes; the logical root only backs the cache.
    let root = temp_dir("reopen");
    let reopened = Project::open_in(Box::new(store), root.clone()).expect("open from the bundle");
    assert_eq!(reopened.name(), project.name());
    assert_eq!(
        reopened.scenes().expect("scenes").len(),
        project.scenes().expect("scenes").len()
    );

    let imported = reopened.import_assets().expect("import from the bundle");
    assert!(
        imported.errors.is_empty(),
        "every bundled asset must import: {:#?}",
        imported.errors
    );

    std::fs::remove_dir_all(&root).ok();
}
