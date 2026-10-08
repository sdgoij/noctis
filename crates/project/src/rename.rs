//! Renaming an asset: move the file and rewrite every reference to it.
//!
//! A stale asset path is a broken scene, so a rename is a first-class operation,
//! not a manual find-and-replace: [`Project::rename_asset`] moves the file and
//! rewrites the old path — and any `old#fragment` sub-asset references — in every
//! scene and prefab document under the project.
//!
//! The rewrite is a **textual** substitution of the quoted path (and the quoted
//! `path#` prefix), not a re-serialization, so documents keep their exact
//! formatting and key order: a rename that touches no reference changes no bytes.

use std::path::PathBuf;

use crate::error::ProjectError;
use crate::path::AssetPath;
use crate::project::{Project, walk_files};

/// What [`Project::rename_asset`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenameReport {
    /// The file's old on-disk path.
    pub from: PathBuf,
    /// The file's new on-disk path.
    pub to: PathBuf,
    /// The documents whose references were rewritten, as asset paths.
    pub documents: Vec<AssetPath>,
}

/// Rewrite references to `from` (and `from#fragment`) as `to` in a document's
/// text. Returns the text unchanged when it contains no such reference.
///
/// Only whole quoted paths are replaced, so a path that merely *contains* `from`
/// (e.g. `"assets/x/old"` for `from == "old"`) is left alone.
pub fn rewrite_references(text: &str, from: &AssetPath, to: &AssetPath) -> String {
    let whole = text.replace(
        &format!("\"{}\"", from.as_str()),
        &format!("\"{}\"", to.as_str()),
    );
    whole.replace(
        &format!("\"{}#", from.as_str()),
        &format!("\"{}#", to.as_str()),
    )
}

impl Project {
    /// Move `from` to `to` on disk and rewrite every reference to it.
    ///
    /// `from` and `to` are file asset paths (a fragment is rejected). `from` must
    /// exist and `to` must not. Scenes and prefabs under the project are rewritten
    /// in place; only the documents that changed are reported.
    pub fn rename_asset(
        &self,
        from: &AssetPath,
        to: &AssetPath,
    ) -> Result<RenameReport, ProjectError> {
        if from.fragment().is_some() || to.fragment().is_some() {
            return Err(ProjectError::Rename(
                "rename operates on files, not sub-asset fragments".to_string(),
            ));
        }
        if from == to {
            return Err(ProjectError::Rename(
                "the source and destination paths are identical".to_string(),
            ));
        }

        let source = self.file_for(from);
        let destination = self.file_for(to);
        if !source.is_file() {
            return Err(ProjectError::MissingAsset(from.as_str().to_string()));
        }
        if destination.exists() {
            return Err(ProjectError::Rename(format!(
                "{} already exists",
                to.as_str()
            )));
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ProjectError::Io(e.to_string()))?;
        }
        std::fs::rename(&source, &destination).map_err(|e| ProjectError::Io(e.to_string()))?;

        // Rewrite references in every scene and prefab document.
        let mut documents = Vec::new();
        let mut files = Vec::new();
        walk_files(&self.scenes_dir(), &mut files)?;
        walk_files(&self.prefabs_dir(), &mut files)?;
        for file in files {
            if !is_document(&file) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            let rewritten = rewrite_references(&text, from, to);
            if rewritten == text {
                continue;
            }
            std::fs::write(&file, rewritten).map_err(|e| ProjectError::Io(e.to_string()))?;
            if let Some(relative) = relative(self.root(), &file) {
                documents.push(relative);
            }
        }
        documents.sort();

        Ok(RenameReport {
            from: source,
            to: destination,
            documents,
        })
    }
}

/// Whether `path` is a scene or prefab document.
fn is_document(path: &std::path::Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    name.ends_with(".scene.json") || name.ends_with(".prefab.json") || name == "scene.json"
}

/// `path` relative to `root`, as an asset path.
fn relative(root: &std::path::Path, path: &std::path::Path) -> Option<AssetPath> {
    let relative = path.strip_prefix(root).ok()?;
    AssetPath::parse(relative.to_string_lossy().replace('\\', "/")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> AssetPath {
        AssetPath::parse(text).unwrap()
    }

    #[test]
    fn rewrite_replaces_whole_paths_and_fragments() {
        let from = path("assets/models/robot.glb");
        let to = path("assets/models/mech.glb");
        let document = r#"{
            "mesh": "assets/models/robot.glb#mesh/Torso",
            "material": "assets/models/robot.glb",
            "other": "assets/models/robot.glb.backup"
        }"#;
        let rewritten = rewrite_references(document, &from, &to);
        assert!(rewritten.contains("\"assets/models/mech.glb#mesh/Torso\""));
        assert!(rewritten.contains("\"material\": \"assets/models/mech.glb\""));
        // A path merely *containing* the old one is untouched.
        assert!(rewritten.contains("\"assets/models/robot.glb.backup\""));
    }

    #[test]
    fn rewrite_of_a_substring_is_left_alone() {
        let from = path("old");
        let to = path("new");
        let document = r#"{ "keep": "assets/x/old", "change": "old" }"#;
        let rewritten = rewrite_references(document, &from, &to);
        assert!(rewritten.contains("\"assets/x/old\""));
        assert!(rewritten.contains("\"change\": \"new\""));
    }

    #[test]
    fn rewrite_is_identity_without_a_reference() {
        let document = r#"{ "mesh": "primitives/cube" }"#;
        assert_eq!(
            rewrite_references(document, &path("a"), &path("b")),
            document
        );
    }
}
