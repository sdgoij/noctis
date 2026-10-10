//! Resolving a scene's asset references to the project files that back them.
//!
//! A scene document references assets by path: a file path
//! (`assets/textures/wood.png`), a `file#kind/name` fragment
//! (`assets/models/robot.glb#mesh/Torso`), or — for a glTF saved from the editor
//! — a bare sub-asset name (`Torso`). [`referenced_files`] maps every such
//! reference to the source file a build must bundle, using the [`lookup`] index
//! produced by [`Project::import_assets`].
//!
//! Engine built-ins (`$…` placeholders and `primitives/…` meshes) are not files:
//! they are recreated at load time by `AssetRegistry::with_builtins`, so they are
//! skipped. A non-built-in reference that resolves to no file is an error naming
//! it — a build never silently ships a scene with a dangling reference.
//!
//! [`lookup`]: crate::ImportedAssets::lookup
//! [`Project::import_assets`]: crate::Project::import_assets

use std::collections::{BTreeMap, BTreeSet};

use ecs::reflect::ComponentRegistry;
use ecs::scene::{ComponentDesc, SceneDesc};

use crate::error::ProjectError;
use crate::path::AssetPath;

/// Whether `reference` names an engine built-in rather than a project file.
pub fn is_builtin(reference: &str) -> bool {
    reference.starts_with('$') || reference.starts_with("primitives/")
}

/// The asset references carried by one component.
fn component_references(component: &ComponentDesc) -> Vec<&str> {
    match component {
        ComponentDesc::MeshRenderer { mesh, material, .. } => {
            [mesh.as_deref(), material.as_deref()]
                .into_iter()
                .flatten()
                .collect()
        }
        ComponentDesc::AudioSource { clip, .. } => clip.as_deref().into_iter().collect(),
        ComponentDesc::Script { path, .. } => path.as_deref().into_iter().collect(),
        ComponentDesc::Text { font, .. } => font.as_deref().into_iter().collect(),
        // Cameras, lights, and physics carry no asset references. A custom
        // component's references need the registry — see
        // [`scene_references_with`].
        _ => Vec::new(),
    }
}

/// Every asset reference in `scene`, in document order (with duplicates).
///
/// Built-in components only; a custom component's references need the registry —
/// use [`scene_references_with`].
pub fn scene_references(scene: &SceneDesc) -> Vec<&str> {
    scene
        .entities
        .iter()
        .flat_map(|entity| entity.components.iter())
        .flat_map(component_references)
        .collect()
}

/// Every asset reference in `scene`, including a registered custom component's.
///
/// A `Custom` entry's references come from the registry
/// ([`ComponentRegistry::references_of`]), so the files a custom component points
/// at are bundled like a mesh or a clip. An unregistered name contributes nothing.
pub fn scene_references_with(scene: &SceneDesc, components: &ComponentRegistry) -> Vec<String> {
    let mut out = Vec::new();
    for entity in &scene.entities {
        for component in &entity.components {
            match component {
                ComponentDesc::Custom { name, data } => {
                    out.extend(components.references_of(name, data));
                }
                other => out.extend(component_references(other).into_iter().map(str::to_string)),
            }
        }
    }
    out
}

/// Map every asset reference in `scene` to the project file that backs it,
/// sorted and deduplicated.
///
/// `lookup` maps a registry key (a file path, a fragment, or a sub-asset name) to
/// the source file, as produced by [`Project::import_assets`]. Built-in
/// references are skipped; any other reference that is absent from `lookup` is a
/// [`ProjectError::MissingAsset`].
///
/// [`Project::import_assets`]: crate::Project::import_assets
pub fn referenced_files(
    scene: &SceneDesc,
    lookup: &BTreeMap<String, AssetPath>,
) -> Result<Vec<AssetPath>, ProjectError> {
    referenced_files_with(scene, lookup, &ComponentRegistry::new())
}

/// Like [`referenced_files`], but including a registered custom component's
/// asset references (from `components`).
pub fn referenced_files_with(
    scene: &SceneDesc,
    lookup: &BTreeMap<String, AssetPath>,
    components: &ComponentRegistry,
) -> Result<Vec<AssetPath>, ProjectError> {
    let mut files: BTreeSet<AssetPath> = BTreeSet::new();
    for reference in scene_references_with(scene, components) {
        if is_builtin(&reference) {
            continue;
        }
        match lookup.get(&reference) {
            Some(file) => {
                files.insert(file.clone());
            }
            None => return Err(ProjectError::MissingAsset(reference)),
        }
    }
    Ok(files.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ecs::reflect::Component;
    use ecs::scene::SceneDesc;

    #[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
    struct Portal {
        texture: String,
    }

    impl Component for Portal {
        const NAME: &'static str = "Portal";
        fn references(&self) -> Vec<String> {
            if self.texture.is_empty() {
                Vec::new()
            } else {
                vec![self.texture.clone()]
            }
        }
    }

    fn scene_with(component_json: &str) -> SceneDesc {
        let json = format!(
            r#"{{ "name": "S", "entities": [{{ "id": 0, "transform": {{ "translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] }}, "components": [{component_json}] }}] }}"#
        );
        serde_json::from_str(&json).expect("scene parses")
    }

    fn lookup(pairs: &[(&str, &str)]) -> BTreeMap<String, AssetPath> {
        pairs
            .iter()
            .map(|(key, file)| (key.to_string(), AssetPath::parse(file).unwrap()))
            .collect()
    }

    #[test]
    fn builtins_are_recognised() {
        assert!(is_builtin("$default"));
        assert!(is_builtin("primitives/cube"));
        assert!(!is_builtin("assets/models/robot.glb"));
        assert!(!is_builtin("Torso"));
    }

    #[test]
    fn references_resolve_to_the_backing_files() {
        let scene = scene_with(
            r#"{ "type": "mesh_renderer", "mesh": "Torso", "material": "assets/materials/steel.mat", "cast_shadows": true, "receive_shadows": true }"#,
        );
        let index = lookup(&[
            ("Torso", "assets/models/robot.glb"),
            ("assets/materials/steel.mat", "assets/materials/steel.mat"),
        ]);
        let files = referenced_files(&scene, &index).expect("resolve");
        assert_eq!(
            files,
            vec![
                AssetPath::parse("assets/materials/steel.mat").unwrap(),
                AssetPath::parse("assets/models/robot.glb").unwrap(),
            ]
        );
    }

    #[test]
    fn builtin_references_are_skipped_and_duplicates_collapse() {
        let scene = scene_with(
            r#"{ "type": "mesh_renderer", "mesh": "primitives/cube", "material": "$default", "cast_shadows": true, "receive_shadows": true }"#,
        );
        assert!(
            referenced_files(&scene, &BTreeMap::new())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn an_unresolvable_reference_is_an_error_naming_it() {
        let scene = scene_with(
            r#"{ "type": "audio_source", "clip": "audio/gone.wav", "volume": 1.0, "pitch": 1.0, "looped": false, "playing": true, "spatial": true, "max_distance": 20.0, "rolloff": 1.0 }"#,
        );
        let err = referenced_files(&scene, &BTreeMap::new()).unwrap_err();
        assert!(matches!(err, ProjectError::MissingAsset(ref r) if r == "audio/gone.wav"));
    }

    #[test]
    fn a_script_path_is_bundled_too() {
        let scene = scene_with(
            r#"{ "type": "script", "source": null, "path": "scripts/player.js", "enabled": true }"#,
        );
        let index = lookup(&[("scripts/player.js", "scripts/player.js")]);
        let files = referenced_files(&scene, &index).unwrap();
        assert_eq!(files, vec![AssetPath::parse("scripts/player.js").unwrap()]);
    }

    #[test]
    fn a_text_font_is_bundled_too() {
        let scene = scene_with(
            r#"{ "type": "text", "text": "Hi", "font": "assets/fonts/body.ttf", "size": 1.0, "color": [1,1,1,1], "anchor": "center", "depth_test": true }"#,
        );
        let index = lookup(&[("assets/fonts/body.ttf", "assets/fonts/body.ttf")]);
        let files = referenced_files(&scene, &index).unwrap();
        assert_eq!(
            files,
            vec![AssetPath::parse("assets/fonts/body.ttf").unwrap()]
        );

        // A built-in font (or none) is not a project file.
        let none = scene_with(
            r#"{ "type": "text", "text": "Hi", "font": null, "size": 1.0, "color": [1,1,1,1], "anchor": "center", "depth_test": true }"#,
        );
        assert!(
            referenced_files(&none, &BTreeMap::new())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_custom_component_reference_is_bundled() {
        let scene = scene_with(
            r#"{ "type": "custom", "name": "Portal", "data": { "texture": "assets/textures/portal.png" } }"#,
        );
        let index = lookup(&[("assets/textures/portal.png", "assets/textures/portal.png")]);

        // Without the registry a custom component contributes nothing.
        assert!(referenced_files(&scene, &index).unwrap().is_empty());

        // With the component registered, its reference resolves to the file.
        let mut registry = ComponentRegistry::new();
        registry.register::<Portal>();
        assert_eq!(
            referenced_files_with(&scene, &index, &registry).unwrap(),
            vec![AssetPath::parse("assets/textures/portal.png").unwrap()]
        );

        // An unregistered name is skipped, not an error.
        assert!(
            referenced_files_with(&scene, &index, &ComponentRegistry::new())
                .unwrap()
                .is_empty()
        );
    }
}
