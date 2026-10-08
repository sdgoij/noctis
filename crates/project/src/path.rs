//! Canonical, project-relative asset paths and asset-type detection.
//!
//! An *asset path* is a logical, project-relative path — the string that appears
//! in scene and prefab JSON. It is not an OS path: the project maps it to a file
//! under its root. See `docs/project.md`.

use std::fmt;

use crate::error::ProjectError;

/// A canonical, project-relative asset path, optionally with a sub-asset
/// fragment (the part after `#`).
///
/// Canonicalization collapses `.` and repeated separators, accepts `\` or `/`
/// as the separator, and rejects empty paths, absolute paths (including Windows
/// drive prefixes), and any `..` segment.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssetPath(String);

impl AssetPath {
    /// Parse and canonicalize an asset path.
    pub fn parse(raw: impl AsRef<str>) -> Result<Self, ProjectError> {
        let raw = raw.as_ref();
        let (path, fragment) = match raw.split_once('#') {
            Some((path, fragment)) => (path, Some(fragment)),
            None => (raw, None),
        };
        let mut canonical = canonicalize(path)?;
        if let Some(fragment) = fragment {
            if fragment.is_empty() {
                return Err(ProjectError::Path(format!(
                    "sub-asset fragment is empty in {raw:?}"
                )));
            }
            canonical.push('#');
            canonical.push_str(fragment);
        }
        Ok(Self(canonical))
    }

    /// The canonical path, including any fragment.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The path without any sub-asset fragment.
    pub fn file_path(&self) -> &str {
        match self.0.split_once('#') {
            Some((path, _)) => path,
            None => &self.0,
        }
    }

    /// The sub-asset fragment, if any (the part after `#`).
    pub fn fragment(&self) -> Option<&str> {
        self.0.split_once('#').map(|(_, fragment)| fragment)
    }

    /// The final path segment (the file name).
    pub fn file_name(&self) -> &str {
        self.file_path()
            .rsplit('/')
            .next()
            .unwrap_or(self.file_path())
    }

    /// The parent directory, as a canonical path, or `None` at the root.
    pub fn parent(&self) -> Option<&str> {
        self.file_path().rsplit_once('/').map(|(parent, _)| parent)
    }
}

impl fmt::Display for AssetPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for AssetPath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Canonicalize the path part of an asset reference.
fn canonicalize(path: &str) -> Result<String, ProjectError> {
    let path = path.trim().replace('\\', "/");
    if path.is_empty() {
        return Err(ProjectError::Path("asset path is empty".to_string()));
    }
    if path.starts_with('/') || has_drive_prefix(&path) {
        return Err(ProjectError::Path(format!(
            "asset path must be project-relative: {path:?}"
        )));
    }

    let mut segments = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => continue,
            ".." => {
                return Err(ProjectError::Path(format!(
                    "asset path may not contain '..': {path:?}"
                )));
            }
            other => segments.push(other),
        }
    }
    if segments.is_empty() {
        return Err(ProjectError::Path(format!("asset path is empty: {path:?}")));
    }
    Ok(segments.join("/"))
}

/// Whether `path` begins with a Windows drive prefix such as `C:`.
fn has_drive_prefix(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// The kind of asset a file is, from its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetKind {
    /// A model (`.glb`, `.gltf`, `.fbx`).
    Model,
    /// A 2D image (`.png`, `.jpg`, `.bmp`, `.tga`).
    Texture,
    /// An equirectangular HDR environment map (`.hdr`) for image-based lighting.
    Environment,
    /// An audio clip (`.wav`, `.ogg`, `.mp3`).
    Audio,
    /// A font (`.ttf`, `.otf`).
    Font,
    /// A script (`.js`, `.mjs`, `.wasm`).
    Script,
    /// A scene (`*.scene.json`).
    Scene,
    /// A prefab (`*.prefab.json`).
    Prefab,
    /// An unrecognised file.
    Unknown,
}

impl AssetKind {
    /// Whether this kind is an asset (as opposed to a scene, a prefab, or an
    /// unrecognised file).
    pub fn is_asset(self) -> bool {
        matches!(
            self,
            AssetKind::Model
                | AssetKind::Texture
                | AssetKind::Environment
                | AssetKind::Audio
                | AssetKind::Font
                | AssetKind::Script
        )
    }

    /// The conventional project-relative directory for this kind, per
    /// `docs/project.md`. `None` for [`AssetKind::Unknown`].
    ///
    /// A project may relocate these with `[paths]`; this is only the default that
    /// drag & drop suggests.
    pub fn default_dir(self) -> Option<&'static str> {
        Some(match self {
            AssetKind::Model => "assets/models",
            AssetKind::Texture => "assets/textures",
            AssetKind::Environment => "assets/hdris",
            AssetKind::Audio => "assets/audio",
            AssetKind::Font => "assets/fonts",
            AssetKind::Script => "scripts",
            AssetKind::Scene => "scenes",
            AssetKind::Prefab => "prefabs",
            AssetKind::Unknown => return None,
        })
    }
}

/// Detect an asset's kind from its path's extension (case-insensitive).
///
/// The compound suffixes `.scene.json` and `.prefab.json` are recognised before
/// the single extension.
pub fn detect(path: &str) -> AssetKind {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".scene.json") {
        return AssetKind::Scene;
    }
    if lower.ends_with(".prefab.json") {
        return AssetKind::Prefab;
    }
    let extension = match lower.rsplit_once('.') {
        Some((_, extension)) if !extension.is_empty() => extension,
        _ => return AssetKind::Unknown,
    };
    match extension {
        "glb" | "gltf" | "fbx" => AssetKind::Model,
        "png" | "jpg" | "jpeg" | "bmp" | "tga" => AssetKind::Texture,
        // A `.hdr` is an *environment*, not a texture: it decodes to linear f32
        // radiance and is consumed by the IBL precompute, not sampled as colour.
        "hdr" => AssetKind::Environment,
        "wav" | "ogg" | "mp3" => AssetKind::Audio,
        "ttf" | "otf" => AssetKind::Font,
        "js" | "mjs" | "wasm" => AssetKind::Script,
        _ => AssetKind::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_separators_and_dot_segments() {
        assert_eq!(
            AssetPath::parse("./assets//textures/wood.png")
                .unwrap()
                .as_str(),
            "assets/textures/wood.png"
        );
        assert_eq!(
            AssetPath::parse(r"assets\models\robot.glb")
                .unwrap()
                .as_str(),
            "assets/models/robot.glb"
        );
    }

    #[test]
    fn rejects_absolute_parent_and_empty_paths() {
        assert!(matches!(
            AssetPath::parse("/etc/passwd"),
            Err(ProjectError::Path(_))
        ));
        assert!(matches!(
            AssetPath::parse("C:/windows"),
            Err(ProjectError::Path(_))
        ));
        assert!(matches!(
            AssetPath::parse("../secret"),
            Err(ProjectError::Path(_))
        ));
        assert!(matches!(
            AssetPath::parse("assets/../../x"),
            Err(ProjectError::Path(_))
        ));
        assert!(matches!(
            AssetPath::parse("   "),
            Err(ProjectError::Path(_))
        ));
        assert!(matches!(AssetPath::parse("."), Err(ProjectError::Path(_))));
    }

    #[test]
    fn parses_fragments_and_keeps_the_file_path() {
        let path = AssetPath::parse("assets/models/robot.glb#mesh/Torso").unwrap();
        assert_eq!(path.file_path(), "assets/models/robot.glb");
        assert_eq!(path.fragment(), Some("mesh/Torso"));
        assert_eq!(path.file_name(), "robot.glb");
        assert_eq!(path.parent(), Some("assets/models"));
        assert!(
            AssetPath::parse("a.png#").is_err(),
            "empty fragment rejected"
        );
    }

    #[test]
    fn a_bare_file_has_no_parent() {
        let path = AssetPath::parse("scene.json").unwrap();
        assert_eq!(path.parent(), None);
        assert_eq!(path.file_name(), "scene.json");
    }

    #[test]
    fn detects_kinds_by_extension() {
        assert_eq!(detect("assets/models/robot.glb"), AssetKind::Model);
        assert_eq!(detect("assets/textures/WOOD.PNG"), AssetKind::Texture);
        assert_eq!(detect("assets/audio/theme.ogg"), AssetKind::Audio);
        assert_eq!(detect("assets/fonts/Inter.otf"), AssetKind::Font);
        assert_eq!(detect("scripts/player.js"), AssetKind::Script);
        assert_eq!(detect("scripts/brain.wasm"), AssetKind::Script);
        assert_eq!(detect("scenes/main.scene.json"), AssetKind::Scene);
        assert_eq!(detect("prefabs/robot.prefab.json"), AssetKind::Prefab);
        assert_eq!(detect("README"), AssetKind::Unknown);
        assert_eq!(detect("archive.tar.gz"), AssetKind::Unknown);
    }

    #[test]
    fn an_hdr_is_an_environment_not_a_texture() {
        assert_eq!(detect("assets/hdris/sky.hdr"), AssetKind::Environment);
        assert_eq!(detect("assets/hdris/SKY.HDR"), AssetKind::Environment);
        assert!(AssetKind::Environment.is_asset());
        assert_eq!(AssetKind::Environment.default_dir(), Some("assets/hdris"));
    }

    #[test]
    fn default_directories_match_the_doc() {
        assert_eq!(AssetKind::Model.default_dir(), Some("assets/models"));
        assert_eq!(AssetKind::Texture.default_dir(), Some("assets/textures"));
        assert_eq!(AssetKind::Font.default_dir(), Some("assets/fonts"));
        assert_eq!(AssetKind::Script.default_dir(), Some("scripts"));
        assert_eq!(AssetKind::Scene.default_dir(), Some("scenes"));
        assert_eq!(AssetKind::Prefab.default_dir(), Some("prefabs"));
        assert_eq!(AssetKind::Unknown.default_dir(), None);
    }

    #[test]
    fn only_asset_kinds_are_assets() {
        assert!(AssetKind::Model.is_asset());
        assert!(AssetKind::Texture.is_asset());
        assert!(AssetKind::Audio.is_asset());
        assert!(AssetKind::Font.is_asset());
        assert!(AssetKind::Script.is_asset());
        assert!(!AssetKind::Scene.is_asset());
        assert!(!AssetKind::Prefab.is_asset());
        assert!(!AssetKind::Unknown.is_asset());
    }
}
