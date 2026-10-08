//! The project asset map: discovering a project's asset files and importing them
//! into an [`AssetRegistry`].
//!
//! Discovery walks the project's `assets/` and `scripts/` directories and
//! classifies each file by extension ([`detect`]). Import decodes each file into
//! the registry under its **project-relative asset path**, which is exactly what
//! scene and prefab documents reference, so building a scene against the imported
//! registry resolves those references. A glTF's sub-assets are additionally
//! registered under their `file#kind/name` fragment path.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ecs::assets::{AssetRegistry, ScriptAsset};

use crate::cache::{ImportCache, texture_key};
use crate::error::ProjectError;
use crate::path::{AssetKind, AssetPath, detect};
use crate::project::{Project, relative_asset_path, walk_files};
use crate::settings::TextureImportSettings;

/// One importable file in the project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetMapEntry {
    /// The file's canonical, project-relative asset path.
    pub path: AssetPath,
    /// The kind detected from its extension.
    pub kind: AssetKind,
    /// The file on disk.
    pub file: PathBuf,
}

/// The project's importable asset files, discovered by scanning.
#[derive(Clone, Debug, Default)]
pub struct AssetMap {
    entries: Vec<AssetMapEntry>,
}

impl AssetMap {
    /// The entries, sorted by asset path.
    pub fn entries(&self) -> &[AssetMapEntry] {
        &self.entries
    }

    /// The number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entry for an asset path, if present.
    pub fn get(&self, path: &AssetPath) -> Option<&AssetMapEntry> {
        self.entries.iter().find(|entry| &entry.path == path)
    }

    /// The entries of one kind.
    pub fn with_kind(&self, kind: AssetKind) -> impl Iterator<Item = &AssetMapEntry> {
        self.entries.iter().filter(move |entry| entry.kind == kind)
    }
}

/// A non-fatal failure importing one asset file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetImportError {
    /// The file that failed to import.
    pub path: AssetPath,
    /// Why it failed.
    pub message: String,
}

/// A registry populated from the project, plus per-file import failures.
///
/// A failing file is recorded in [`ImportedAssets::errors`] rather than aborting
/// the whole import, so one corrupt asset does not stop a project from opening.
pub struct ImportedAssets {
    /// The populated registry.
    pub registry: AssetRegistry,
    /// The number of asset files imported from the project.
    pub files: usize,
    /// Files that could not be imported.
    pub errors: Vec<AssetImportError>,
    /// Every registry lookup key mapped to the source file's asset path: a
    /// file's own path, a `file#kind/name` fragment, or a glTF sub-asset's
    /// in-file name. Lets a caller resolve a scene reference to a file to bundle.
    pub lookup: BTreeMap<String, AssetPath>,
}

impl std::fmt::Debug for ImportedAssets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportedAssets")
            .field("files", &self.files)
            .field("meshes", &self.registry.meshes.len())
            .field("materials", &self.registry.materials.len())
            .field("textures", &self.registry.textures.len())
            .field("scripts", &self.registry.scripts.len())
            .field("lookup", &self.lookup.len())
            .field("errors", &self.errors)
            .finish()
    }
}

impl Project {
    /// Discover every importable asset file in the project.
    pub fn asset_map(&self) -> Result<AssetMap, ProjectError> {
        let mut files = Vec::new();
        walk_files(&self.assets_dir(), &mut files)?;
        walk_files(&self.scripts_dir(), &mut files)?;

        let mut entries: Vec<AssetMapEntry> = files
            .into_iter()
            .filter_map(|file| {
                let path = relative_asset_path(self.root(), &file)?;
                let kind = detect(path.as_str());
                kind.is_asset()
                    .then_some(AssetMapEntry { path, kind, file })
            })
            .collect();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        entries.dedup_by(|a, b| a.path == b.path);
        Ok(AssetMap { entries })
    }

    /// Import every asset file into a fresh registry.
    ///
    /// The registry starts with the engine's default placeholder assets
    /// (`$default`, `$white`, `$normal`), so a scene that references a missing
    /// asset still resolves to a placeholder rather than nothing.
    pub fn import_assets(&self) -> Result<ImportedAssets, ProjectError> {
        let map = self.asset_map()?;
        let mut registry = AssetRegistry::with_builtins();
        let cache = ImportCache::new(self.root());
        let mut errors = Vec::new();
        let mut lookup = BTreeMap::new();
        for entry in map.entries() {
            // The file is always bundleable, even if decoding it fails.
            lookup.insert(entry.path.as_str().to_string(), entry.path.clone());
            if let Err(message) = import_entry(&mut registry, entry, &mut lookup, &cache) {
                errors.push(AssetImportError {
                    path: entry.path.clone(),
                    message,
                });
            }
        }
        Ok(ImportedAssets {
            registry,
            files: map.len(),
            errors,
            lookup,
        })
    }
}

/// Import one file into `registry`. A kind with no importer yet is a no-op.
fn import_entry(
    registry: &mut AssetRegistry,
    entry: &AssetMapEntry,
    lookup: &mut BTreeMap<String, AssetPath>,
    cache: &ImportCache,
) -> Result<(), String> {
    match entry.kind {
        AssetKind::Model => import_model(registry, &entry.path, &entry.file, lookup),
        AssetKind::Texture => import_texture(registry, &entry.path, &entry.file, cache),
        AssetKind::Script => import_script(registry, &entry.path, &entry.file),
        // Audio and fonts have no importer yet; scenes and prefabs are not assets.
        _ => Ok(()),
    }
}

/// Import an image file, registered under its asset path. The texture's sRGB-ness
/// comes from its sidecar import settings (`<file>.import.toml`), defaulting to
/// sRGB. Decoding is cached under `.noctis/` keyed by the file's content and the
/// setting.
fn import_texture(
    registry: &mut AssetRegistry,
    path: &AssetPath,
    file: &Path,
    cache: &ImportCache,
) -> Result<(), String> {
    let settings = TextureImportSettings::load(file)?;
    let bytes = std::fs::read(file).map_err(|error| error.to_string())?;
    let key = texture_key(&bytes, settings.srgb);

    let texture = match cache.load_texture(&key) {
        Some(texture) => texture,
        None => {
            let texture = render::load_texture(file, settings.srgb).map_err(|e| e.to_string())?;
            // A cache write failure is not fatal: we already have the texture.
            let _ = cache.store_texture(&key, &texture);
            texture
        }
    };
    registry.textures.insert_named(path.as_str(), texture);
    Ok(())
}

/// Import a `.js` script, registered under its asset path.
fn import_script(
    registry: &mut AssetRegistry,
    path: &AssetPath,
    file: &Path,
) -> Result<(), String> {
    let source = std::fs::read_to_string(file).map_err(|error| error.to_string())?;
    let asset = ScriptAsset {
        name: path.as_str().to_string(),
        source,
    };
    registry.scripts.insert_named(path.as_str(), asset);
    Ok(())
}

/// Import a glTF model, registering its sub-assets under both their in-file names
/// (what a scene saved from the editor references) and their fragment paths. Each
/// key is recorded in `lookup` so it can be resolved back to `path`.
fn import_model(
    registry: &mut AssetRegistry,
    path: &AssetPath,
    file: &Path,
    lookup: &mut BTreeMap<String, AssetPath>,
) -> Result<(), String> {
    let loaded = render::load_gltf(file).map_err(|error| error.to_string())?;
    let registered = loaded.register(registry);
    for (mesh, handle) in loaded.meshes.iter().zip(&registered.meshes) {
        let fragment = fragment_path(path.as_str(), "mesh", &mesh.name);
        registry.meshes.insert_alias(&fragment, *handle);
        lookup.insert(fragment, path.clone());
        lookup.insert(mesh.name.clone(), path.clone());
    }
    for (material, handle) in loaded.materials.iter().zip(&registered.materials) {
        let fragment = fragment_path(path.as_str(), "material", &material.name);
        registry.materials.insert_alias(&fragment, *handle);
        lookup.insert(fragment, path.clone());
        lookup.insert(material.name.clone(), path.clone());
    }
    for (texture, handle) in loaded.textures.iter().zip(&registered.textures) {
        let fragment = fragment_path(path.as_str(), "texture", &texture.name);
        registry.textures.insert_alias(&fragment, *handle);
        lookup.insert(fragment, path.clone());
        lookup.insert(texture.name.clone(), path.clone());
    }
    Ok(())
}

/// The fragment path for a sub-asset: `<asset path>#<kind>/<name>`.
pub fn fragment_path(asset_path: &str, kind: &str, name: &str) -> String {
    format!("{asset_path}#{kind}/{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_paths_use_the_hash_scheme() {
        assert_eq!(
            fragment_path("assets/models/robot.glb", "mesh", "Cube"),
            "assets/models/robot.glb#mesh/Cube"
        );
        // Fragments parse back as asset paths.
        let path = AssetPath::parse(fragment_path("m.glb", "material", "Red")).unwrap();
        assert_eq!(path.file_path(), "m.glb");
        assert_eq!(path.fragment(), Some("material/Red"));
    }
}
