//! Asset handles and typed asset stores.
//!
//! Assets are referenced by a copyable [`AssetHandle`] rather than by a direct
//! pointer, so components stay `Copy`-friendly and can be serialized as paths.
//! Each asset type has its own [`Assets`] store, and an [`AssetRegistry`]
//! bundles the stores the engine cares about.

use math::Aabb;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

/// A stable numeric id for an asset. `0` is reserved to mean "no asset".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssetId(pub u32);

impl AssetId {
    /// The "no asset" id.
    pub const NONE: AssetId = AssetId(0);

    /// `true` if this is the reserved "no asset" id.
    #[inline]
    pub const fn is_none(self) -> bool {
        self.0 == 0
    }
}

/// A typed, copyable reference to an asset.
///
/// The type parameter is used only for compile-time safety; the handle itself
/// is a plain `u32`.
pub struct AssetHandle<T> {
    id: AssetId,
    _marker: PhantomData<fn() -> T>,
}

impl<T> AssetHandle<T> {
    /// A handle that refers to nothing.
    pub const NONE: AssetHandle<T> = AssetHandle {
        id: AssetId::NONE,
        _marker: PhantomData,
    };

    /// A handle for a specific asset id.
    #[inline]
    pub const fn new(id: AssetId) -> Self {
        Self {
            id,
            _marker: PhantomData,
        }
    }

    /// A handle from a raw id.
    #[inline]
    pub const fn from_raw(id: u32) -> Self {
        Self::new(AssetId(id))
    }

    /// The asset id.
    #[inline]
    pub const fn id(self) -> AssetId {
        self.id
    }

    /// `true` if this handle refers to nothing.
    #[inline]
    pub const fn is_none(self) -> bool {
        self.id.is_none()
    }
}

impl<T> Clone for AssetHandle<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for AssetHandle<T> {}
impl<T> PartialEq for AssetHandle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl<T> Eq for AssetHandle<T> {}
impl<T> std::hash::Hash for AssetHandle<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}
impl<T> Default for AssetHandle<T> {
    fn default() -> Self {
        Self::NONE
    }
}
impl<T> std::fmt::Debug for AssetHandle<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AssetHandle({})", self.id.0)
    }
}

struct Entry<T> {
    value: Arc<T>,
    path: Option<String>,
}

/// A typed store of assets, addressable by id or by path.
pub struct Assets<T> {
    entries: HashMap<AssetId, Entry<T>>,
    paths: HashMap<String, AssetId>,
    next_id: u32,
}

impl<T> Default for Assets<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Assets<T> {
    /// An empty store. Ids start at 1 (`0` is reserved).
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            paths: HashMap::new(),
            next_id: 1,
        }
    }

    /// Insert an unnamed asset, returning its handle.
    pub fn insert(&mut self, value: T) -> AssetHandle<T> {
        self.insert_impl(None, value)
    }

    /// Insert (or replace) a named asset. Replacing keeps the original id so
    /// existing handles stay valid.
    pub fn insert_named(&mut self, path: impl Into<String>, value: T) -> AssetHandle<T> {
        self.insert_impl(Some(path.into()), value)
    }

    fn insert_impl(&mut self, path: Option<String>, value: T) -> AssetHandle<T> {
        if let Some(ref p) = path
            && let Some(&id) = self.paths.get(p)
        {
            self.entries.insert(
                id,
                Entry {
                    value: Arc::new(value),
                    path,
                },
            );
            return AssetHandle::new(id);
        }
        let id = AssetId(self.next_id);
        self.next_id += 1;
        if let Some(ref p) = path {
            self.paths.insert(p.clone(), id);
        }
        self.entries.insert(
            id,
            Entry {
                value: Arc::new(value),
                path,
            },
        );
        AssetHandle::new(id)
    }

    /// Borrow an asset by handle.
    pub fn get(&self, handle: AssetHandle<T>) -> Option<&T> {
        self.entries.get(&handle.id).map(|e| e.value.as_ref())
    }

    /// The shared pointer for an asset, if present.
    pub fn get_shared(&self, handle: AssetHandle<T>) -> Option<Arc<T>> {
        self.entries.get(&handle.id).map(|e| Arc::clone(&e.value))
    }

    /// Mutably borrow an asset by handle, cloning it out of the shared store if
    /// the value is aliased elsewhere.
    pub fn get_mut(&mut self, handle: AssetHandle<T>) -> Option<&mut T>
    where
        T: Clone,
    {
        self.entries
            .get_mut(&handle.id)
            .map(|entry| Arc::make_mut(&mut entry.value))
    }

    /// The handle registered at `path`, if any.
    pub fn handle(&self, path: &str) -> Option<AssetHandle<T>> {
        self.paths.get(path).map(|&id| AssetHandle::new(id))
    }

    /// Borrow an asset by path.
    pub fn get_by_path(&self, path: &str) -> Option<&T> {
        self.paths
            .get(path)
            .and_then(|id| self.entries.get(id))
            .map(|e| e.value.as_ref())
    }

    /// The path an asset was registered under, if any.
    pub fn path_of(&self, handle: AssetHandle<T>) -> Option<&str> {
        self.entries.get(&handle.id).and_then(|e| e.path.as_deref())
    }

    /// Point an additional `path` at an existing asset, without copying it.
    ///
    /// Returns `false` if the path is already registered or the handle is
    /// unknown. This exposes one asset under several logical paths: a project
    /// registers a glTF sub-asset under both its in-file name and its
    /// `file#fragment` path.
    pub fn insert_alias(&mut self, path: impl Into<String>, handle: AssetHandle<T>) -> bool {
        let path = path.into();
        if !self.entries.contains_key(&handle.id) || self.paths.contains_key(&path) {
            return false;
        }
        self.paths.insert(path, handle.id);
        true
    }

    /// Remove an asset, returning whether it existed.
    pub fn remove(&mut self, handle: AssetHandle<T>) -> bool {
        match self.entries.remove(&handle.id) {
            Some(entry) => {
                if let Some(path) = entry.path {
                    self.paths.remove(&path);
                }
                true
            }
            None => false,
        }
    }

    /// The number of stored assets.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` if the store is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate over `(id, path, value)` triples.
    pub fn iter(&self) -> impl Iterator<Item = (AssetId, Option<&str>, &T)> {
        self.entries
            .iter()
            .map(|(id, e)| (*id, e.path.as_deref(), e.value.as_ref()))
    }
}

/// A vertex in the engine's standard mesh layout.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
#[repr(C)]
pub struct Vertex {
    /// Object-space position.
    pub position: [f32; 3],
    /// Object-space normal.
    pub normal: [f32; 3],
    /// Texture coordinate 0.
    pub uv: [f32; 2],
    /// Tangent with handedness in `w`.
    pub tangent: [f32; 4],
}

/// An indexed triangle mesh.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    /// A human-readable name (often the primitive name from a glTF file).
    pub name: String,
    /// Vertex data.
    pub vertices: Vec<Vertex>,
    /// Triangle indices.
    pub indices: Vec<u32>,
    /// Object-space bounds.
    pub bounds: Option<Aabb>,
}

impl Mesh {
    /// Compute the tight object-space bounds from the vertices.
    pub fn compute_bounds(&self) -> Option<Aabb> {
        let points: Vec<math::Vec3> = self
            .vertices
            .iter()
            .map(|v| math::vec3(v.position[0], v.position[1], v.position[2]))
            .collect();
        Aabb::from_points(&points)
    }

    /// The number of triangles.
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// How a material treats its alpha channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AlphaMode {
    /// Alpha is ignored; the surface is opaque.
    #[default]
    Opaque,
    /// Alpha below the cutoff is discarded.
    Mask,
    /// Alpha blends with what is behind.
    Blend,
}

/// A physically-based material (metallic/roughness workflow).
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    /// A human-readable name.
    pub name: String,
    /// Linear base color (RGBA).
    pub base_color: [f32; 4],
    /// Metalness in `[0, 1]`.
    pub metallic: f32,
    /// Roughness in `[0, 1]`.
    pub roughness: f32,
    /// Linear emissive color.
    pub emissive: [f32; 3],
    /// Emissive multiplier.
    pub emissive_strength: f32,
    /// Base color texture.
    pub base_color_texture: AssetHandle<Texture>,
    /// Normal map.
    pub normal_texture: AssetHandle<Texture>,
    /// Packed metallic (B) / roughness (G) texture.
    pub metallic_roughness_texture: AssetHandle<Texture>,
    /// Emissive texture.
    pub emissive_texture: AssetHandle<Texture>,
    /// Ambient occlusion texture (R channel).
    pub occlusion_texture: AssetHandle<Texture>,
    /// Alpha handling.
    pub alpha_mode: AlphaMode,
    /// Whether back faces are rendered.
    pub double_sided: bool,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            name: String::new(),
            base_color: [1.0, 1.0, 1.0, 1.0],
            metallic: 0.0,
            roughness: 0.5,
            emissive: [0.0, 0.0, 0.0],
            emissive_strength: 1.0,
            base_color_texture: AssetHandle::NONE,
            normal_texture: AssetHandle::NONE,
            metallic_roughness_texture: AssetHandle::NONE,
            emissive_texture: AssetHandle::NONE,
            occlusion_texture: AssetHandle::NONE,
            alpha_mode: AlphaMode::Opaque,
            double_sided: false,
        }
    }
}

/// The pixel format of a [`Texture`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextureFormat {
    /// 8-bit sRGB RGBA, the usual color texture format.
    #[default]
    Rgba8Srgb,
    /// 8-bit linear RGBA, for data textures (normal maps, masks).
    Rgba8Unorm,
}

/// A decoded CPU-side texture.
#[derive(Clone, Debug)]
pub struct Texture {
    /// A human-readable name.
    pub name: String,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixel format.
    pub format: TextureFormat,
    /// Raw pixel bytes, `width * height * 4` long.
    pub data: Vec<u8>,
}

impl Texture {
    /// A 1x1 texture filled with a single color (linear components in `[0,1]`).
    pub fn solid(name: impl Into<String>, color: [u8; 4], format: TextureFormat) -> Self {
        Self {
            name: name.into(),
            width: 1,
            height: 1,
            format,
            data: color.to_vec(),
        }
    }
}

/// A decoded, uncompressed HDR environment map (equirectangular).
#[derive(Clone, Debug)]
pub struct HdrTexture {
    /// A human-readable name.
    pub name: String,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Linear RGB float samples, `width * height * 3` long.
    pub data: Vec<f32>,
}

/// A decoded PCM audio clip.
#[derive(Clone, Debug)]
pub struct AudioClip {
    /// A human-readable name.
    pub name: String,
    /// Samples per second.
    pub sample_rate: u32,
    /// Channel count.
    pub channels: u16,
    /// Interleaved `f32` samples in `[-1, 1]`.
    pub samples: Vec<f32>,
}

impl AudioClip {
    /// Duration in seconds.
    pub fn duration(&self) -> f32 {
        if self.sample_rate == 0 || self.channels == 0 {
            0.0
        } else {
            self.samples.len() as f32 / (self.sample_rate as f32 * self.channels as f32)
        }
    }
}

/// A JavaScript source asset.
#[derive(Clone, Debug)]
pub struct ScriptAsset {
    /// A human-readable name (usually the file path).
    pub name: String,
    /// The JavaScript source text.
    pub source: String,
}

/// A font source asset: the font file's bytes, ready for an atlas builder.
///
/// Glyph rasterization (the atlas) is produced on demand and cached; this type
/// holds the versioned source the engine reads it from.
#[derive(Clone, Debug)]
pub struct FontAsset {
    /// A human-readable name (usually the file path).
    pub name: String,
    /// The font file's bytes (TrueType/OpenType).
    pub data: Vec<u8>,
}

impl FontAsset {
    /// `true` if the bytes begin with a recognised font signature
    /// (`0x00010000`, `OTTO`, `true`, `ttcf`, `wOFF`, or `wOF2`).
    pub fn is_font_data(data: &[u8]) -> bool {
        matches!(
            data.get(..4),
            Some([0x00, 0x01, 0x00, 0x00])
                | Some(b"OTTO")
                | Some(b"true")
                | Some(b"ttcf")
                | Some(b"wOFF")
                | Some(b"wOF2")
        )
    }
}

/// Bundles the asset stores the engine uses.
#[derive(Default)]
pub struct AssetRegistry {
    /// Mesh assets.
    pub meshes: Assets<Mesh>,
    /// Material assets.
    pub materials: Assets<Material>,
    /// Texture assets.
    pub textures: Assets<Texture>,
    /// Audio clip assets.
    pub audio: Assets<AudioClip>,
    /// Script assets.
    pub scripts: Assets<ScriptAsset>,
    /// Font assets.
    pub fonts: Assets<FontAsset>,
    /// HDR environment assets.
    pub hdr: Assets<HdrTexture>,
}

impl AssetRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Install placeholder defaults (a white texture, a unit material, and so
    /// on) under well-known paths. Useful so a scene referencing `"$white"`
    /// always resolves.
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        registry.textures.insert_named(
            "$white",
            Texture::solid("white", [255, 255, 255, 255], TextureFormat::Rgba8Srgb),
        );
        registry.textures.insert_named(
            "$normal",
            Texture::solid("normal", [128, 128, 255, 255], TextureFormat::Rgba8Unorm),
        );
        registry
            .materials
            .insert_named("$default", Material::default());
        registry
    }

    /// The placeholder defaults plus the built-in primitive meshes
    /// (`primitives/cube`, `primitives/sphere`, `primitives/plane`,
    /// `primitives/cylinder`, `primitives/capsule`, `primitives/torus`).
    ///
    /// Use this whenever a scene document is loaded, so a reference to a built-in
    /// primitive always resolves and [`Scene::from_json`](crate::scene::Scene::from_json)'s
    /// missing-reference check cannot fire on an engine-provided mesh.
    pub fn with_builtins() -> Self {
        let mut registry = Self::with_defaults();
        registry.install_primitives();
        registry
    }

    /// Register the built-in primitive meshes under `primitives/<name>`,
    /// replacing any already registered under those paths.
    pub fn install_primitives(&mut self) {
        use crate::primitives;
        self.meshes
            .insert_named("primitives/cube", primitives::cube(1.0));
        self.meshes
            .insert_named("primitives/sphere", primitives::sphere(0.5, 24, 32));
        self.meshes
            .insert_named("primitives/plane", primitives::plane(1.0, 4));
        self.meshes
            .insert_named("primitives/cylinder", primitives::cylinder(0.5, 1.0, 24));
        self.meshes
            .insert_named("primitives/capsule", primitives::capsule(0.4, 1.0, 24));
        self.meshes
            .insert_named("primitives/torus", primitives::torus(1.0, 0.25, 32, 16));
    }

    /// The total number of assets across all stores.
    pub fn len(&self) -> usize {
        self.meshes.len()
            + self.materials.len()
            + self.textures.len()
            + self.audio.len()
            + self.scripts.len()
            + self.fonts.len()
            + self.hdr.len()
    }

    /// `true` if every store is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_mesh(name: &str) -> Mesh {
        Mesh {
            name: name.to_string(),
            vertices: vec![Vertex::default(), Vertex::default(), Vertex::default()],
            indices: vec![0, 1, 2],
            bounds: None,
        }
    }

    #[test]
    fn insert_and_get() {
        let mut meshes: Assets<Mesh> = Assets::new();
        let h = meshes.insert(dummy_mesh("a"));
        assert!(!h.is_none());
        assert_eq!(meshes.get(h).unwrap().name, "a");
        assert_eq!(meshes.len(), 1);
    }

    #[test]
    fn named_insert_and_lookup() {
        let mut meshes: Assets<Mesh> = Assets::new();
        let h = meshes.insert_named("models/cube.gltf", dummy_mesh("cube"));
        assert_eq!(meshes.handle("models/cube.gltf"), Some(h));
        assert_eq!(meshes.get_by_path("models/cube.gltf").unwrap().name, "cube");
        assert_eq!(meshes.path_of(h), Some("models/cube.gltf"));
    }

    #[test]
    fn replacing_named_asset_keeps_id() {
        let mut meshes: Assets<Mesh> = Assets::new();
        let h1 = meshes.insert_named("m", dummy_mesh("first"));
        let h2 = meshes.insert_named("m", dummy_mesh("second"));
        assert_eq!(h1, h2);
        assert_eq!(meshes.get(h1).unwrap().name, "second");
        assert_eq!(meshes.len(), 1);
    }

    #[test]
    fn get_mut_edits_in_place_and_keeps_the_handle() {
        let mut materials: Assets<Material> = Assets::new();
        let h = materials.insert_named("m", Material::default());
        materials.get_mut(h).expect("present").roughness = 0.9;
        assert_eq!(materials.get(h).unwrap().roughness, 0.9);
        assert_eq!(materials.path_of(h), Some("m"));
    }

    #[test]
    fn insert_alias_points_extra_paths_at_one_asset() {
        let mut textures: Assets<Texture> = Assets::new();
        let h = textures.insert_named(
            "assets/models/robot.glb#texture/0",
            Texture::solid("t", [1, 2, 3, 4], TextureFormat::Rgba8Srgb),
        );
        assert!(textures.insert_alias("0", h));
        assert_eq!(textures.handle("0"), Some(h));
        assert_eq!(
            textures.handle("assets/models/robot.glb#texture/0"),
            Some(h)
        );
        assert_eq!(textures.len(), 1, "an alias does not copy the asset");

        // Aliasing an unknown handle, or a path already taken, is rejected.
        assert!(!textures.insert_alias("c", AssetHandle::NONE));
        assert!(!textures.insert_alias("0", h));
    }

    #[test]
    fn remove_clears_path() {
        let mut meshes: Assets<Mesh> = Assets::new();
        let h = meshes.insert_named("m", dummy_mesh("x"));
        assert!(meshes.remove(h));
        assert!(meshes.get(h).is_none());
        assert!(meshes.handle("m").is_none());
    }

    #[test]
    fn shared_handle_survives_store_mutation() {
        let mut meshes: Assets<Mesh> = Assets::new();
        let h = meshes.insert(dummy_mesh("keep"));
        let shared = meshes.get_shared(h).unwrap();
        meshes.remove(h);
        // The Arc keeps the data alive even after removal.
        assert_eq!(shared.name, "keep");
    }

    #[test]
    fn mesh_bounds_and_triangle_count() {
        let mut mesh = dummy_mesh("tri");
        mesh.vertices[0].position = [0.0, 0.0, 0.0];
        mesh.vertices[1].position = [1.0, 0.0, 0.0];
        mesh.vertices[2].position = [0.0, 2.0, 0.0];
        assert_eq!(mesh.triangle_count(), 1);
        let b = mesh.compute_bounds().unwrap();
        assert_eq!((b.min.x, b.min.y, b.min.z), (0.0, 0.0, 0.0));
        assert_eq!((b.max.x, b.max.y, b.max.z), (1.0, 2.0, 0.0));
    }

    #[test]
    fn audio_duration() {
        let clip = AudioClip {
            name: "beep".into(),
            sample_rate: 48_000,
            channels: 2,
            samples: vec![0.0; 48_000 * 2],
        };
        assert!((clip.duration() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn font_signatures_are_recognised() {
        assert!(FontAsset::is_font_data(&[0x00, 0x01, 0x00, 0x00, 0x00]));
        assert!(FontAsset::is_font_data(b"OTTO...."));
        assert!(FontAsset::is_font_data(b"true...."));
        assert!(!FontAsset::is_font_data(b"not a font"));
        assert!(!FontAsset::is_font_data(b"OT"));
    }

    #[test]
    fn registry_with_defaults_resolves_well_known_paths() {
        let registry = AssetRegistry::with_defaults();
        assert!(!registry.is_empty());
        assert!(registry.materials.handle("$default").is_some());
        assert!(registry.textures.handle("$white").is_some());
    }

    #[test]
    fn with_builtins_adds_the_primitive_meshes() {
        let registry = AssetRegistry::with_builtins();
        assert!(registry.materials.handle("$default").is_some());
        assert!(registry.meshes.handle("primitives/cube").is_some());
        assert!(registry.meshes.handle("primitives/torus").is_some());
        assert_eq!(registry.meshes.len(), 6);

        // `with_defaults` itself stays minimal (placeholders only).
        assert!(AssetRegistry::with_defaults().meshes.is_empty());
    }
}
