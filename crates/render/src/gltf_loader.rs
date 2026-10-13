//! glTF 2.0 import.
//!
//! [`load_gltf`] / [`load_gltf_bytes`] turn a `.gltf` or `.glb` document into
//! plain engine values: [`Mesh`] assets (one per primitive), [`Material`] assets,
//! decoded [`Texture`] assets, and a flat list of [`NodeDesc`] hierarchy entries.
//!
//! ## Texture handle convention
//!
//! glTF materials reference textures by index, but [`Material`] stores
//! [`AssetHandle`]s. While a [`LoadedGltf`] is *unregistered* the handles are
//! encoded as `AssetHandle::from_raw(texture_index + 1)` so the data is
//! self-describing. [`LoadedGltf::register`] then inserts every texture into an
//! [`AssetRegistry`] and rewrites those placeholders to the real handles, so it
//! works correctly even when the registry already contains assets.
//!
//! [`LoadedGltf::instantiate`] spawns one entity per node, wires up the parent
//! hierarchy with [`ecs::scene::set_parent`], and attaches a child render entity
//! for each mesh primitive.

use crate::error::RenderError;
use ecs::assets::{
    AnimationChannel, AnimationClip, AnimationProperty, AssetHandle, AssetRegistry, Material, Mesh,
    Skin, Texture, TextureFilter, TextureFormat, Vertex, VertexInfluence,
};
use ecs::components::{LocalTransform, MeshRenderer, Name, Skeleton, SkinnedMeshRenderer};
use ecs::scene::set_parent;
use ecs::world::{Entity, World};
use math::{Mat4, Quat, Transform, from_cols_array, vec3};

/// A `KHR_texture_transform` applied to a texture's UVs.
///
/// glTF lets a texture carry an affine transform (`offset`, `rotation`, `scale`),
/// which is how a tiled detail map is packed into an atlas. The engine samples one
/// UV set per vertex, so the transform is baked into the primitive's UVs as the
/// mesh is built (see [`base_color_uv_transform`]).
#[derive(Clone, Copy, Debug, PartialEq)]
struct UvTransform {
    offset: [f32; 2],
    rotation: f32,
    scale: [f32; 2],
}

impl UvTransform {
    /// Apply the transform to a UV. The extension's order is scale, then rotation
    /// (counter-clockwise), then the offset.
    fn apply(&self, uv: [f32; 2]) -> [f32; 2] {
        let (sin, cos) = self.rotation.sin_cos();
        let u = self.scale[0] * uv[0];
        let v = self.scale[1] * uv[1];
        [
            cos * u - sin * v + self.offset[0],
            sin * u + cos * v + self.offset[1],
        ]
    }
}

/// A material's base-color texture transform, if it carries one.
///
/// Only the base-color map drives the engine's single UV set; a transform that
/// differs per map (the same material's normal or metallic-roughness texture) is
/// not representable with one UV attribute and is ignored.
fn base_color_uv_transform(material: gltf::Material<'_>) -> Option<UvTransform> {
    let transform = material
        .pbr_metallic_roughness()
        .base_color_texture()?
        .texture_transform()?;
    Some(UvTransform {
        offset: transform.offset(),
        rotation: transform.rotation(),
        scale: transform.scale(),
    })
}

/// A node in the imported glTF hierarchy.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeDesc {
    /// The node name (empty when the file omits it).
    pub name: String,
    /// The index of the parent node, if any.
    pub parent: Option<usize>,
    /// The node's local transform.
    pub transform: Transform,
    /// The first mesh primitive attached to this node, if any.
    pub mesh: Option<usize>,
    /// Every mesh primitive attached to this node (usually zero or one entry).
    pub mesh_primitives: Vec<usize>,
    /// The index of the skin this node deforms with, if any (its `skin`).
    pub skin: Option<usize>,
}

/// The full result of importing a glTF document.
#[derive(Clone, Debug, Default)]
pub struct LoadedGltf {
    /// One [`Mesh`] per glTF primitive.
    pub meshes: Vec<Mesh>,
    /// One [`Material`] per glTF material.
    pub materials: Vec<Material>,
    /// One [`Texture`] per glTF texture.
    pub textures: Vec<Texture>,
    /// The node hierarchy, in glTF node-index order.
    pub nodes: Vec<NodeDesc>,
    /// The root node indices of the default scene.
    pub roots: Vec<usize>,
    /// The default scene name, if the file names it.
    pub scene_name: Option<String>,
    /// Material index for each entry in [`LoadedGltf::meshes`].
    pub mesh_materials: Vec<Option<usize>>,
    /// The mesh primitives belonging to each glTF mesh, in mesh-index order.
    pub mesh_primitives: Vec<Vec<usize>>,
    /// One [`Skin`] per glTF skin, in skin-index order.
    pub skins: Vec<Skin>,
    /// One [`AnimationClip`] per glTF animation, in animation-index order.
    pub animations: Vec<AnimationClip>,
}

/// Handles produced by registering a [`LoadedGltf`] into an [`AssetRegistry`].
#[derive(Clone, Debug, Default)]
pub struct RegisteredGltf {
    /// Mesh handles, parallel to [`LoadedGltf::meshes`].
    pub meshes: Vec<AssetHandle<Mesh>>,
    /// Material handles, parallel to [`LoadedGltf::materials`].
    pub materials: Vec<AssetHandle<Material>>,
    /// Texture handles, parallel to [`LoadedGltf::textures`].
    pub textures: Vec<AssetHandle<Texture>>,
    /// The material used by primitives without one.
    pub default_material: AssetHandle<Material>,
    /// Skin handles, parallel to [`LoadedGltf::skins`].
    pub skins: Vec<AssetHandle<Skin>>,
    /// Animation handles, parallel to [`LoadedGltf::animations`].
    pub animations: Vec<AssetHandle<AnimationClip>>,
}

/// Load a glTF document from a filesystem path.
pub fn load_gltf(path: impl AsRef<std::path::Path>) -> Result<LoadedGltf, RenderError> {
    let bytes = std::fs::read(path)?;
    load_gltf_bytes(&bytes)
}

/// Load a glTF document from an in-memory `.gltf` or `.glb` buffer.
pub fn load_gltf_bytes(bytes: &[u8]) -> Result<LoadedGltf, RenderError> {
    let (document, buffers, images) = gltf::import_slice(bytes)?;
    Ok(build_loaded(&document, &buffers, &images))
}

fn build_loaded(
    document: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    images: &[gltf::image::Data],
) -> LoadedGltf {
    // Which textures are color (sRGB) rather than data (linear).
    let texture_count = document.textures().count();
    let mut srgb = vec![false; texture_count];
    for material in document.materials() {
        let pbr = material.pbr_metallic_roughness();
        if let Some(info) = pbr.base_color_texture() {
            mark(&mut srgb, info.texture().index());
        }
        if let Some(info) = material.emissive_texture() {
            mark(&mut srgb, info.texture().index());
        }
    }

    let textures = document
        .textures()
        .map(|tex| {
            let idx = tex.index();
            let image_idx = tex.source().index();
            let name = tex.name().unwrap_or("texture").to_string();
            match images.get(image_idx) {
                Some(image) => {
                    image_to_texture(&name, image, srgb.get(idx).copied().unwrap_or(false))
                }
                None => Texture::solid(
                    name,
                    [255, 255, 255, 255],
                    if srgb.get(idx).copied().unwrap_or(false) {
                        TextureFormat::Rgba8Srgb
                    } else {
                        TextureFormat::Rgba8Unorm
                    },
                ),
            }
        })
        .collect::<Vec<_>>();

    let materials = document.materials().map(build_material).collect::<Vec<_>>();
    // The UV transform each material's base-color map asks for, precomputed so
    // the primitive loop can bake it in without re-walking the materials.
    let material_uv = document
        .materials()
        .map(base_color_uv_transform)
        .collect::<Vec<_>>();

    // Skins: the joint nodes and their bind-time inverse transforms.
    let skins = document
        .skins()
        .map(|skin| {
            let joints: Vec<usize> = skin.joints().map(|node| node.index()).collect();
            let inverse_bind: Vec<Mat4> = skin
                .reader(|buffer| buffers.get(buffer.index()).map(|b| b.0.as_slice()))
                .read_inverse_bind_matrices()
                .map(|matrices| matrices.map(from_cols_array).collect())
                .unwrap_or_default();
            Skin {
                name: skin.name().unwrap_or("skin").to_string(),
                joints,
                inverse_bind,
            }
        })
        .collect::<Vec<_>>();

    // Animations: one clip per glTF animation, named for lookup.
    let animations = document
        .animations()
        .map(|animation| build_clip(animation, buffers))
        .collect::<Vec<_>>();

    let mut meshes = Vec::new();
    let mut mesh_materials = Vec::new();
    let mut mesh_primitives = Vec::new();
    for mesh in document.meshes() {
        let mut primitives = Vec::new();
        for primitive in mesh.primitives() {
            let reader =
                primitive.reader(|buffer| buffers.get(buffer.index()).map(|b| b.0.as_slice()));
            let positions: Vec<[f32; 3]> = reader
                .read_positions()
                .map(|p| p.collect())
                .unwrap_or_default();
            if positions.is_empty() {
                continue;
            }
            let normals: Vec<[f32; 3]> = reader
                .read_normals()
                .map(|n| n.collect())
                .unwrap_or_default();
            let uvs: Vec<[f32; 2]> = reader
                .read_tex_coords(0)
                .map(|t| t.into_f32().collect())
                .unwrap_or_default();
            let uv_transform = primitive
                .material()
                .index()
                .and_then(|index| material_uv.get(index))
                .copied()
                .flatten();
            let tangents: Vec<[f32; 4]> = reader
                .read_tangents()
                .map(|t| t.collect())
                .unwrap_or_default();
            let indices: Vec<u32> = reader
                .read_indices()
                .map(|i| i.into_u32().collect())
                .unwrap_or_else(|| (0..positions.len() as u32).collect());

            // glTF calls TANGENT optional, and many exporters (Blender among them)
            // omit it — but a normal map needs the tangent basis, without which the
            // map contributes nothing and the surface reads flat. Derive one from the
            // UVs when the file does not carry it.
            let tangents = if tangents.is_empty() {
                compute_tangents(&positions, &normals, &uvs, &indices)
            } else {
                tangents
            };

            // The per-vertex skin influence, when the primitive carries one.
            let influences: Vec<VertexInfluence> = {
                let joints: Vec<[u16; 4]> = reader
                    .read_joints(0)
                    .map(|j| j.into_u16().collect())
                    .unwrap_or_default();
                if joints.is_empty() {
                    Vec::new()
                } else {
                    let weights: Vec<[f32; 4]> = reader
                        .read_weights(0)
                        .map(|w| w.into_f32().collect())
                        .unwrap_or_default();
                    joints
                        .iter()
                        .enumerate()
                        .map(|(i, &joints)| VertexInfluence {
                            joints,
                            weights: weights.get(i).copied().unwrap_or([0.0; 4]),
                        })
                        .collect()
                }
            };

            let vertices = positions
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let uv = uvs.get(i).copied().unwrap_or([0.0, 0.0]);
                    Vertex {
                        position: *p,
                        normal: normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]),
                        uv: uv_transform.map_or(uv, |transform| transform.apply(uv)),
                        tangent: tangents.get(i).copied().unwrap_or([0.0, 0.0, 0.0, 1.0]),
                        color: [1.0, 1.0, 1.0, 1.0],
                    }
                })
                .collect::<Vec<_>>();

            let mut asset = Mesh {
                name: format!(
                    "{}_primitive{}",
                    mesh.name().unwrap_or("mesh"),
                    primitive.index()
                ),
                vertices,
                indices,
                bounds: None,
                influences,
            };
            asset.bounds = asset.compute_bounds();

            primitives.push(meshes.len());
            meshes.push(asset);
            mesh_materials.push(primitive.material().index());
        }
        mesh_primitives.push(primitives);
    }

    // Derive parent links by walking every scene's node tree.
    let mut parents: Vec<Option<usize>> = vec![None; document.nodes().count()];
    for scene in document.scenes() {
        for root in scene.nodes() {
            walk_parents(&root, &mut parents);
        }
    }

    let nodes = document
        .nodes()
        .map(|node| {
            let (translation, rotation, scale) = node.transform().decomposed();
            let mesh = node.mesh().and_then(|m| {
                mesh_primitives
                    .get(m.index())
                    .and_then(|p| p.first().copied())
            });
            let primitives = node
                .mesh()
                .and_then(|m| mesh_primitives.get(m.index()).cloned())
                .unwrap_or_default();
            NodeDesc {
                name: node.name().unwrap_or("node").to_string(),
                parent: parents[node.index()],
                transform: Transform {
                    translation: vec3(translation[0], translation[1], translation[2]),
                    rotation: Quat::new(rotation[0], rotation[1], rotation[2], rotation[3]),
                    scale: vec3(scale[0], scale[1], scale[2]),
                },
                mesh,
                mesh_primitives: primitives,
                skin: node.skin().map(|skin| skin.index()),
            }
        })
        .collect::<Vec<_>>();

    let (roots, scene_name) = match document.default_scene() {
        Some(scene) => (
            scene.nodes().map(|n| n.index()).collect(),
            scene.name().map(|s| s.to_string()),
        ),
        None => (
            (0..nodes.len())
                .filter(|i| parents.get(*i).copied().flatten().is_none())
                .collect(),
            None,
        ),
    };

    LoadedGltf {
        meshes,
        materials,
        textures,
        nodes,
        roots,
        scene_name,
        mesh_materials,
        mesh_primitives,
        skins,
        animations,
    }
}

/// Turn one glTF animation into an [`AnimationClip`].
///
/// Channels for a property the engine cannot drive (morph targets) are dropped;
/// the clip's duration is the latest key time across its channels.
fn build_clip(animation: gltf::Animation<'_>, buffers: &[gltf::buffer::Data]) -> AnimationClip {
    use gltf::animation::util::ReadOutputs;

    let get = |buffer: gltf::Buffer<'_>| buffers.get(buffer.index()).map(|b| b.0.as_slice());
    let mut channels = Vec::new();
    let mut duration = 0.0f32;
    for channel in animation.channels() {
        let target = channel.target();
        let property = match target.property() {
            gltf::animation::Property::Translation => AnimationProperty::Translation,
            gltf::animation::Property::Rotation => AnimationProperty::Rotation,
            gltf::animation::Property::Scale => AnimationProperty::Scale,
            gltf::animation::Property::MorphTargetWeights => continue,
        };
        let reader = channel.reader(get);
        let times: Vec<f32> = reader
            .read_inputs()
            .map(|inputs| inputs.collect())
            .unwrap_or_default();
        let values: Vec<[f32; 4]> = match reader.read_outputs() {
            Some(ReadOutputs::Translations(t)) => t.map(|v| [v[0], v[1], v[2], 0.0]).collect(),
            Some(ReadOutputs::Scales(s)) => s.map(|v| [v[0], v[1], v[2], 0.0]).collect(),
            Some(ReadOutputs::Rotations(r)) => {
                r.into_f32().map(|v| [v[0], v[1], v[2], v[3]]).collect()
            }
            Some(ReadOutputs::MorphTargetWeights(_)) | None => continue,
        };
        duration = duration.max(times.last().copied().unwrap_or(0.0));
        channels.push(AnimationChannel {
            node: target.node().index(),
            property,
            times,
            values,
        });
    }
    AnimationClip {
        name: animation.name().unwrap_or("animation").to_string(),
        duration,
        channels,
    }
}

fn mark(flags: &mut [bool], index: usize) {
    if let Some(slot) = flags.get_mut(index) {
        *slot = true;
    }
}

/// A registration key for an asset with `name`: the name itself, or
/// `fallback#index` for an empty one, with a `#index` suffix when the name is
/// already taken (so two never alias to the same id).
fn unique_name(name: &str, fallback: &str, index: usize, taken: impl Fn(&str) -> bool) -> String {
    let base = if name.is_empty() { fallback } else { name };
    if name.is_empty() || taken(base) {
        format!("{base}#{index}")
    } else {
        name.to_string()
    }
}

fn walk_parents(node: &gltf::Node<'_>, parents: &mut [Option<usize>]) {
    for child in node.children() {
        if let Some(slot) = parents.get_mut(child.index()) {
            *slot = Some(node.index());
        }
        walk_parents(&child, parents);
    }
}

/// Derive per-vertex tangents from positions, normals, UVs and indices (the usual
/// per-triangle accumulation). `w` is the handedness. A vertex whose UVs span no
/// area falls back to an arbitrary tangent orthogonal to its normal, so the normal
/// map's basis is never degenerate.
fn compute_tangents(
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    uvs: &[[f32; 2]],
    indices: &[u32],
) -> Vec<[f32; 4]> {
    let count = positions.len();
    let mut tangent = vec![[0.0f32; 3]; count];
    let mut bitangent = vec![[0.0f32; 3]; count];
    let uv = |i: usize| uvs.get(i).copied().unwrap_or([0.0, 0.0]);
    for tri in indices.as_chunks::<3>().0 {
        let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        if i0 >= count || i1 >= count || i2 >= count {
            continue;
        }
        let (p0, p1, p2) = (positions[i0], positions[i1], positions[i2]);
        let (uv0, uv1, uv2) = (uv(i0), uv(i1), uv(i2));
        let e1 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
        let e2 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
        let duv1 = [uv1[0] - uv0[0], uv1[1] - uv0[1]];
        let duv2 = [uv2[0] - uv0[0], uv2[1] - uv0[1]];
        let det = duv1[0] * duv2[1] - duv2[0] * duv1[1];
        if det.abs() < 1e-12 {
            continue;
        }
        let r = 1.0 / det;
        let t = [
            (e1[0] * duv2[1] - e2[0] * duv1[1]) * r,
            (e1[1] * duv2[1] - e2[1] * duv1[1]) * r,
            (e1[2] * duv2[1] - e2[2] * duv1[1]) * r,
        ];
        let b = [
            (e2[0] * duv1[0] - e1[0] * duv2[0]) * r,
            (e2[1] * duv1[0] - e1[1] * duv2[0]) * r,
            (e2[2] * duv1[0] - e1[2] * duv2[0]) * r,
        ];
        for i in [i0, i1, i2] {
            for k in 0..3 {
                tangent[i][k] += t[k];
                bitangent[i][k] += b[k];
            }
        }
    }

    (0..count)
        .map(|i| {
            let n = normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
            let mut t = tangent[i];
            let d = n[0] * t[0] + n[1] * t[1] + n[2] * t[2];
            t = [t[0] - n[0] * d, t[1] - n[1] * d, t[2] - n[2] * d];
            if (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt() < 1e-8 {
                // Degenerate UVs: any tangent orthogonal to the normal will do.
                t = if n[0].abs() > 0.9 {
                    [0.0, 0.0, 1.0]
                } else {
                    [1.0, 0.0, 0.0]
                };
                let d = n[0] * t[0] + n[1] * t[1] + n[2] * t[2];
                t = [t[0] - n[0] * d, t[1] - n[1] * d, t[2] - n[2] * d];
            }
            let len = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt().max(1e-8);
            t = [t[0] / len, t[1] / len, t[2] / len];
            let cross = [
                n[1] * t[2] - n[2] * t[1],
                n[2] * t[0] - n[0] * t[2],
                n[0] * t[1] - n[1] * t[0],
            ];
            let b = bitangent[i];
            let w = if cross[0] * b[0] + cross[1] * b[1] + cross[2] * b[2] < 0.0 {
                -1.0
            } else {
                1.0
            };
            [t[0], t[1], t[2], w]
        })
        .collect()
}

fn build_material(material: gltf::Material<'_>) -> Material {
    let pbr = material.pbr_metallic_roughness();
    let base = pbr.base_color_factor();
    let emissive = material.emissive_factor();
    let tex =
        |info: gltf::texture::Info<'_>| AssetHandle::from_raw(info.texture().index() as u32 + 1);

    Material {
        name: material.name().unwrap_or("material").to_string(),
        base_color: base,
        metallic: pbr.metallic_factor(),
        roughness: pbr.roughness_factor(),
        emissive,
        emissive_strength: 1.0,
        base_color_texture: pbr
            .base_color_texture()
            .map(tex)
            .unwrap_or(AssetHandle::NONE),
        normal_texture: material
            .normal_texture()
            .map(|n| AssetHandle::from_raw(n.texture().index() as u32 + 1))
            .unwrap_or(AssetHandle::NONE),
        metallic_roughness_texture: pbr
            .metallic_roughness_texture()
            .map(tex)
            .unwrap_or(AssetHandle::NONE),
        emissive_texture: material
            .emissive_texture()
            .map(tex)
            .unwrap_or(AssetHandle::NONE),
        occlusion_texture: material
            .occlusion_texture()
            .map(|o| AssetHandle::from_raw(o.texture().index() as u32 + 1))
            .unwrap_or(AssetHandle::NONE),
        alpha_mode: match material.alpha_mode() {
            gltf::material::AlphaMode::Opaque => ecs::assets::AlphaMode::Opaque,
            gltf::material::AlphaMode::Mask => ecs::assets::AlphaMode::Mask,
            gltf::material::AlphaMode::Blend => ecs::assets::AlphaMode::Blend,
        },
        double_sided: material.double_sided(),
        unlit: material.unlit(),
        vertex_colors: false,
        filter: filter_from_mag_filter(
            pbr.base_color_texture()
                .and_then(|info| info.texture().sampler().mag_filter()),
        ),
    }
}

/// Map a glTF mag-filter to a material texture filter. `NEAREST` asks for crisp
/// pixel art; anything else (or unset) is smooth.
fn filter_from_mag_filter(mag: Option<gltf::texture::MagFilter>) -> TextureFilter {
    match mag {
        Some(gltf::texture::MagFilter::Nearest) => TextureFilter::Nearest,
        _ => TextureFilter::Linear,
    }
}

fn image_to_texture(name: &str, image: &gltf::image::Data, srgb: bool) -> Texture {
    let format = if srgb {
        TextureFormat::Rgba8Srgb
    } else {
        TextureFormat::Rgba8Unorm
    };
    let data = image_to_rgba8(image)
        .unwrap_or_else(|| vec![255u8; (image.width * image.height * 4) as usize]);
    Texture {
        name: name.to_string(),
        width: image.width,
        height: image.height,
        format,
        data,
    }
}

/// Convert a decoded glTF image to tightly packed RGBA8. Returns `None` for
/// formats we do not understand.
fn image_to_rgba8(image: &gltf::image::Data) -> Option<Vec<u8>> {
    use gltf::image::Format;
    let pixels = image.pixels.as_slice();
    let count = (image.width as usize) * (image.height as usize);
    let mut out = Vec::with_capacity(count * 4);

    let u16_at = |i: usize| -> u8 {
        let lo = *pixels.get(i * 2).unwrap_or(&0) as u16;
        let hi = *pixels.get(i * 2 + 1).unwrap_or(&0) as u16;
        ((lo | (hi << 8)) >> 8) as u8
    };
    let f32_at = |i: usize| -> u8 {
        let mut bytes = [0u8; 4];
        for (k, b) in bytes.iter_mut().enumerate() {
            *b = *pixels.get(i * 4 + k).unwrap_or(&0);
        }
        let v = f32::from_le_bytes(bytes);
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    };

    match image.format {
        Format::R8 => {
            for i in 0..count {
                let r = *pixels.get(i).unwrap_or(&0);
                out.extend_from_slice(&[r, r, r, 255]);
            }
        }
        Format::R8G8 => {
            for i in 0..count {
                let r = *pixels.get(i * 2).unwrap_or(&0);
                let g = *pixels.get(i * 2 + 1).unwrap_or(&0);
                out.extend_from_slice(&[r, g, 0, 255]);
            }
        }
        Format::R8G8B8 => {
            for i in 0..count {
                let r = *pixels.get(i * 3).unwrap_or(&0);
                let g = *pixels.get(i * 3 + 1).unwrap_or(&0);
                let b = *pixels.get(i * 3 + 2).unwrap_or(&0);
                out.extend_from_slice(&[r, g, b, 255]);
            }
        }
        Format::R8G8B8A8 => {
            for i in 0..count {
                out.extend_from_slice(&[
                    *pixels.get(i * 4).unwrap_or(&0),
                    *pixels.get(i * 4 + 1).unwrap_or(&0),
                    *pixels.get(i * 4 + 2).unwrap_or(&0),
                    *pixels.get(i * 4 + 3).unwrap_or(&0),
                ]);
            }
        }
        Format::R16 => {
            for i in 0..count {
                let r = u16_at(i);
                out.extend_from_slice(&[r, r, r, 255]);
            }
        }
        Format::R16G16 => {
            for i in 0..count {
                out.extend_from_slice(&[u16_at(i * 2), u16_at(i * 2 + 1), 0, 255]);
            }
        }
        Format::R16G16B16 => {
            for i in 0..count {
                out.extend_from_slice(&[u16_at(i * 3), u16_at(i * 3 + 1), u16_at(i * 3 + 2), 255]);
            }
        }
        Format::R16G16B16A16 => {
            for i in 0..count {
                out.extend_from_slice(&[
                    u16_at(i * 4),
                    u16_at(i * 4 + 1),
                    u16_at(i * 4 + 2),
                    u16_at(i * 4 + 3),
                ]);
            }
        }
        Format::R32G32B32FLOAT => {
            for i in 0..count {
                out.extend_from_slice(&[f32_at(i * 3), f32_at(i * 3 + 1), f32_at(i * 3 + 2), 255]);
            }
        }
        Format::R32G32B32A32FLOAT => {
            for i in 0..count {
                out.extend_from_slice(&[
                    f32_at(i * 4),
                    f32_at(i * 4 + 1),
                    f32_at(i * 4 + 2),
                    f32_at(i * 4 + 3),
                ]);
            }
        }
    }
    Some(out)
}

fn remap_texture(
    handle: AssetHandle<Texture>,
    textures: &[AssetHandle<Texture>],
) -> AssetHandle<Texture> {
    if handle.is_none() {
        return AssetHandle::NONE;
    }
    let index = handle.id().0.saturating_sub(1) as usize;
    textures.get(index).copied().unwrap_or(AssetHandle::NONE)
}

impl LoadedGltf {
    /// Insert every texture, material, and mesh into `registry`, returning the
    /// real handles. Material texture references are rewritten in the process.
    pub fn register(&self, registry: &mut AssetRegistry) -> RegisteredGltf {
        // Textures are frequently unnamed, so several share the name "texture";
        // `insert_named` replaces by path, which would collapse them into one
        // asset and point every map at it. Uniquify so each stays its own asset
        // (glTF references them by index, resolved below).
        let mut texture_handles = Vec::with_capacity(self.textures.len());
        for (index, texture) in self.textures.iter().enumerate() {
            let key = unique_name(&texture.name, "texture", index, |path| {
                registry.textures.handle(path).is_some()
            });
            texture_handles.push(registry.textures.insert_named(key, texture.clone()));
        }

        let mut material_handles = Vec::with_capacity(self.materials.len());
        for (index, material) in self.materials.iter().enumerate() {
            let mut fixed = material.clone();
            fixed.base_color_texture = remap_texture(material.base_color_texture, &texture_handles);
            fixed.normal_texture = remap_texture(material.normal_texture, &texture_handles);
            fixed.metallic_roughness_texture =
                remap_texture(material.metallic_roughness_texture, &texture_handles);
            fixed.emissive_texture = remap_texture(material.emissive_texture, &texture_handles);
            fixed.occlusion_texture = remap_texture(material.occlusion_texture, &texture_handles);
            let key = unique_name(&material.name, "material", index, |path| {
                registry.materials.handle(path).is_some()
            });
            material_handles.push(registry.materials.insert_named(key, fixed));
        }

        let mut mesh_handles = Vec::with_capacity(self.meshes.len());
        for (index, mesh) in self.meshes.iter().enumerate() {
            let key = unique_name(&mesh.name, "mesh", index, |path| {
                registry.meshes.handle(path).is_some()
            });
            mesh_handles.push(registry.meshes.insert_named(key, mesh.clone()));
        }

        let default_material = registry
            .materials
            .handle("$default")
            .unwrap_or_else(|| registry.materials.insert(Material::default()));

        // Skins and clips register under their names, so a script plays a clip by
        // name (its asset path). An unnamed or duplicate name gets a `#index`
        // suffix, so two never alias to one id (`insert_named` replaces by path).
        let mut skin_handles = Vec::with_capacity(self.skins.len());
        for (index, skin) in self.skins.iter().enumerate() {
            let key = unique_name(&skin.name, "skin", index, |path| {
                registry.skins.handle(path).is_some()
            });
            skin_handles.push(registry.skins.insert_named(key, skin.clone()));
        }
        let mut animation_handles = Vec::with_capacity(self.animations.len());
        for (index, clip) in self.animations.iter().enumerate() {
            let key = unique_name(&clip.name, "animation", index, |path| {
                registry.animations.handle(path).is_some()
            });
            animation_handles.push(registry.animations.insert_named(key, clip.clone()));
        }

        RegisteredGltf {
            meshes: mesh_handles,
            materials: material_handles,
            textures: texture_handles,
            default_material,
            skins: skin_handles,
            animations: animation_handles,
        }
    }

    /// Spawn one entity per node and attach mesh-renderer children to nodes that
    /// reference geometry. Returns the node entities in node-index order.
    pub fn instantiate(&self, world: &mut World, handles: &RegisteredGltf) -> Vec<Entity> {
        let node_entities: Vec<Entity> = self
            .nodes
            .iter()
            .map(|node| {
                let entity = world.spawn();
                world.insert(entity, Name::new(node.name.clone()));
                world.insert(entity, LocalTransform { ..node.transform });
                entity
            })
            .collect();

        for (index, node) in self.nodes.iter().enumerate() {
            if let Some(parent) = node.parent
                && let Some(&parent_entity) = node_entities.get(parent)
            {
                set_parent(world, node_entities[index], Some(parent_entity));
            }
        }

        for (index, node) in self.nodes.iter().enumerate() {
            for &primitive in &node.mesh_primitives {
                let Some(&mesh) = handles.meshes.get(primitive) else {
                    continue;
                };
                let material = self
                    .mesh_materials
                    .get(primitive)
                    .copied()
                    .flatten()
                    .and_then(|m| handles.materials.get(m).copied())
                    .unwrap_or(handles.default_material);
                let child = world.spawn();
                world.insert(child, Name::new(format!("{}_mesh{}", node.name, primitive)));
                world.insert(child, LocalTransform::default());
                // A skinned node draws through the skinning pipeline, its joints
                // resolved to the node entities just spawned; a plain node draws
                // with the standard mesh renderer.
                let skin = node
                    .skin
                    .and_then(|index| handles.skins.get(index).copied())
                    .filter(|handle| !handle.is_none());
                match skin {
                    Some(skin) => {
                        world.insert(
                            child,
                            SkinnedMeshRenderer {
                                mesh,
                                skin,
                                material,
                                ..SkinnedMeshRenderer::default()
                            },
                        );
                        // The whole node list, because both the skin's joints and
                        // an animation channel's targets index it by node index.
                        world.insert(child, Skeleton(node_entities.clone()));
                    }
                    None => {
                        world.insert(
                            child,
                            MeshRenderer {
                                mesh,
                                material,
                                ..MeshRenderer::default()
                            },
                        );
                    }
                }
                set_parent(world, child, Some(node_entities[index]));
            }
        }

        node_entities
    }

    /// The number of triangles across every imported mesh.
    pub fn triangle_count(&self) -> usize {
        self.meshes.iter().map(|m| m.triangle_count()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tangents_are_generated_from_uvs() {
        // `triangle_glb` carries no TANGENT attribute (glTF leaves it optional).
        let loaded = load_gltf_bytes(&triangle_glb()).expect("import");
        let mesh = loaded.meshes.first().expect("a mesh");
        assert!(
            mesh.vertices
                .iter()
                .any(|v| v.tangent[..3] != [0.0, 0.0, 0.0]),
            "a tangent is derived when the file omits one"
        );
        for v in &mesh.vertices {
            let len = (v.tangent[0].powi(2) + v.tangent[1].powi(2) + v.tangent[2].powi(2)).sqrt();
            assert!((len - 1.0).abs() < 1e-3, "the tangent is unit: {len}");
            assert_eq!(v.tangent[3].abs(), 1.0, "the handedness is +-1");
        }
    }

    /// Build a minimal binary glTF (GLB) containing one triangle with normals,
    /// UVs, indices, and a red material.
    fn triangle_glb() -> Vec<u8> {
        let positions: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let normals: [[f32; 3]; 3] = [[0.0, 0.0, 1.0]; 3];
        let uvs: [[f32; 2]; 3] = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        let indices: [u16; 3] = [0, 1, 2];

        let mut bin = Vec::new();
        for p in positions {
            for c in p {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        let normal_offset = bin.len();
        for n in normals {
            for c in n {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        let uv_offset = bin.len();
        for uv in uvs {
            for c in uv {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        let index_offset = bin.len();
        for i in indices {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        let total = bin.len();

        let json = format!(
            r#"{{
  "asset": {{ "version": "2.0" }},
  "scene": 0,
  "scenes": [ {{ "name": "Main", "nodes": [0] }} ],
  "nodes": [ {{ "name": "Tri", "mesh": 0 }} ],
  "meshes": [ {{ "name": "TriMesh", "primitives": [
     {{ "attributes": {{ "POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2 }}, "indices": 3, "material": 0 }}
  ] }} ],
  "materials": [ {{ "name": "Red", "pbrMetallicRoughness": {{
     "baseColorFactor": [1.0, 0.0, 0.0, 1.0], "metallicFactor": 0.1, "roughnessFactor": 0.7
  }} }} ],
  "buffers": [ {{ "byteLength": {total} }} ],
  "bufferViews": [
    {{ "buffer": 0, "byteOffset": 0, "byteLength": 36 }},
    {{ "buffer": 0, "byteOffset": {normal_offset}, "byteLength": 36 }},
    {{ "buffer": 0, "byteOffset": {uv_offset}, "byteLength": 24 }},
    {{ "buffer": 0, "byteOffset": {index_offset}, "byteLength": 6 }}
  ],
  "accessors": [
    {{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0,0,0], "max": [1,1,0] }},
    {{ "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3" }},
    {{ "bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC2" }},
    {{ "bufferView": 3, "componentType": 5123, "count": 3, "type": "SCALAR" }}
  ]
}}"#
        );
        make_glb(&json, &bin)
    }

    fn make_glb(json: &str, bin: &[u8]) -> Vec<u8> {
        let mut json_bytes = json.as_bytes().to_vec();
        while !json_bytes.len().is_multiple_of(4) {
            json_bytes.push(b' ');
        }
        let mut bin_bytes = bin.to_vec();
        while !bin_bytes.len().is_multiple_of(4) {
            bin_bytes.push(0);
        }
        let total = 12 + 8 + json_bytes.len() + 8 + bin_bytes.len();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes()); // "JSON"
        out.extend_from_slice(&json_bytes);
        out.extend_from_slice(&(bin_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&0x004E_4942u32.to_le_bytes()); // "BIN\0"
        out.extend_from_slice(&bin_bytes);
        out
    }

    #[test]
    fn loads_triangle_glb() {
        let loaded = load_gltf_bytes(&triangle_glb()).expect("import");
        assert_eq!(loaded.meshes.len(), 1);
        assert_eq!(loaded.materials.len(), 1);
        assert_eq!(loaded.nodes.len(), 1);
        assert_eq!(loaded.triangle_count(), 1);
    }

    #[test]
    fn mesh_geometry_is_correct() {
        let loaded = load_gltf_bytes(&triangle_glb()).expect("import");
        let mesh = &loaded.meshes[0];
        assert_eq!(mesh.vertices.len(), 3);
        assert_eq!(mesh.indices, vec![0, 1, 2]);
        assert_eq!(mesh.triangle_count(), 1);
        let bounds = mesh.bounds.expect("bounds");
        assert!(math::vec3_approx_eq(bounds.min, vec3(0.0, 0.0, 0.0), 1e-6));
        assert!(math::vec3_approx_eq(bounds.max, vec3(1.0, 1.0, 0.0), 1e-6));
        assert_eq!(mesh.vertices[1].position, [1.0, 0.0, 0.0]);
        assert_eq!(mesh.vertices[1].uv, [1.0, 0.0]);
    }

    #[test]
    fn material_matches_gltf() {
        let loaded = load_gltf_bytes(&triangle_glb()).expect("import");
        let mat = &loaded.materials[0];
        assert_eq!(mat.name, "Red");
        assert!((mat.base_color[0] - 1.0).abs() < 1e-6);
        assert!(mat.base_color[1].abs() < 1e-6);
        assert!((mat.metallic - 0.1).abs() < 1e-6);
        assert!((mat.roughness - 0.7).abs() < 1e-6);
        assert!(mat.base_color_texture.is_none());
    }

    /// A 1x1 PNG, for a primitive whose material references a texture.
    fn one_pixel_png() -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([10, 20, 30, 255]));
        let mut bytes = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .expect("encode the test texture");
        bytes
    }

    /// A textured triangle whose base-color map carries `KHR_texture_transform`
    /// (required), as an atlas-packed model — the goat's fleece — does.
    fn textured_triangle_glb() -> Vec<u8> {
        let positions: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let normals: [[f32; 3]; 3] = [[0.0, 0.0, 1.0]; 3];
        let uvs: [[f32; 2]; 3] = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        let indices: [u16; 3] = [0, 1, 2];

        let mut bin = Vec::new();
        for p in positions {
            for c in p {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        let normal_offset = bin.len();
        for n in normals {
            for c in n {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        let uv_offset = bin.len();
        for uv in uvs {
            for c in uv {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        let index_offset = bin.len();
        for i in indices {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        let png = one_pixel_png();
        let image_len = png.len();
        let image_offset = bin.len();
        bin.extend_from_slice(&png);
        let total = bin.len();

        let json = format!(
            r#"{{
  "asset": {{ "version": "2.0" }},
  "extensionsUsed": ["KHR_texture_transform"],
  "extensionsRequired": ["KHR_texture_transform"],
  "scene": 0,
  "scenes": [ {{ "name": "Main", "nodes": [0] }} ],
  "nodes": [ {{ "name": "Tri", "mesh": 0 }} ],
  "meshes": [ {{ "name": "TriMesh", "primitives": [
     {{ "attributes": {{ "POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2 }}, "indices": 3, "material": 0 }}
  ] }} ],
  "materials": [ {{ "name": "Fur", "pbrMetallicRoughness": {{
     "baseColorTexture": {{ "index": 0, "extensions": {{ "KHR_texture_transform": {{ "scale": [2.0, 3.0], "offset": [0.5, 0.25] }} }} }}
  }} }} ],
  "textures": [ {{ "source": 0, "sampler": 0 }} ],
  "samplers": [ {{ "magFilter": 9729, "minFilter": 9729, "wrapS": 10497, "wrapT": 10497 }} ],
  "images": [ {{ "bufferView": 4, "mimeType": "image/png" }} ],
  "buffers": [ {{ "byteLength": {total} }} ],
  "bufferViews": [
    {{ "buffer": 0, "byteOffset": 0, "byteLength": 36 }},
    {{ "buffer": 0, "byteOffset": {normal_offset}, "byteLength": 36 }},
    {{ "buffer": 0, "byteOffset": {uv_offset}, "byteLength": 24 }},
    {{ "buffer": 0, "byteOffset": {index_offset}, "byteLength": 6 }},
    {{ "buffer": 0, "byteOffset": {image_offset}, "byteLength": {image_len} }}
  ],
  "accessors": [
    {{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0,0,0], "max": [1,1,0] }},
    {{ "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3" }},
    {{ "bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC2" }},
    {{ "bufferView": 3, "componentType": 5123, "count": 3, "type": "SCALAR" }}
  ]
}}"#
        );
        make_glb(&json, &bin)
    }

    #[test]
    fn texture_transform_is_applied_to_uvs() {
        let loaded =
            load_gltf_bytes(&textured_triangle_glb()).expect("a texture-transform model imports");
        let mesh = &loaded.meshes[0];
        // scale [2, 3], offset [0.5, 0.25], no rotation.
        assert_eq!(mesh.vertices[0].uv, [0.5, 0.25]);
        assert_eq!(mesh.vertices[1].uv, [2.5, 0.25]);
        assert_eq!(mesh.vertices[2].uv, [0.5, 3.25]);
        // The base-color map resolved to a real texture handle.
        assert!(!loaded.materials[0].base_color_texture.is_none());
    }

    /// A two-bone skinned triangle with one clip: `Body` (the mesh node, also
    /// joint 0) with `Bone` (joint 1) as its child, and a `Wiggle` clip that moves
    /// `Bone` up.
    fn skinned_triangle_glb() -> Vec<u8> {
        fn section(bin: &mut Vec<u8>, bytes: &[u8]) -> usize {
            while !bin.len().is_multiple_of(4) {
                bin.push(0);
            }
            let offset = bin.len();
            bin.extend_from_slice(bytes);
            offset
        }
        let f =
            |values: &[f32]| -> Vec<u8> { values.iter().flat_map(|x| x.to_le_bytes()).collect() };
        let u =
            |values: &[u16]| -> Vec<u8> { values.iter().flat_map(|x| x.to_le_bytes()).collect() };

        let positions = f(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        let normals = f(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]);
        let uvs = f(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0]);
        // Joint 1 for the third vertex, joint 0 for the rest; weight 1 on slot 0.
        let joints = u(&[0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]);
        let weights = f(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
        let indices = u(&[0, 1, 2]);
        let bind = f(&[
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0,
            0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ]);
        let times = f(&[0.0, 1.0]);
        let moves = f(&[0.0, 1.0, 0.0, 0.0, 2.0, 0.0]);

        let mut bin = Vec::new();
        let len = |bin: &mut Vec<u8>, bytes: &[u8]| -> (usize, usize) {
            (section(bin, bytes), bytes.len())
        };
        let views = [
            len(&mut bin, &positions),
            len(&mut bin, &normals),
            len(&mut bin, &uvs),
            len(&mut bin, &joints),
            len(&mut bin, &weights),
            len(&mut bin, &indices),
            len(&mut bin, &bind),
            len(&mut bin, &times),
            len(&mut bin, &moves),
        ];
        let total = bin.len();

        let json = format!(
            r#"{{
  "asset": {{ "version": "2.0" }},
  "scene": 0,
  "scenes": [ {{ "name": "Main", "nodes": [0] }} ],
  "nodes": [
    {{ "name": "Body", "mesh": 0, "skin": 0, "children": [1] }},
    {{ "name": "Bone", "translation": [0.0, 1.0, 0.0] }}
  ],
  "skins": [ {{ "name": "Rig", "joints": [0, 1], "inverseBindMatrices": 6 }} ],
  "meshes": [ {{ "name": "TriMesh", "primitives": [
     {{ "attributes": {{ "POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2, "JOINTS_0": 3, "WEIGHTS_0": 4 }}, "indices": 5, "material": 0 }}
  ] }} ],
  "materials": [ {{ "name": "Hide", "pbrMetallicRoughness": {{ "baseColorFactor": [0.6, 0.4, 0.2, 1.0] }} }} ],
  "animations": [ {{ "name": "Wiggle",
     "channels": [ {{ "sampler": 0, "target": {{ "node": 1, "path": "translation" }} }} ],
     "samplers": [ {{ "input": 7, "output": 8, "interpolation": "LINEAR" }} ]
  }} ],
  "buffers": [ {{ "byteLength": {total} }} ],
  "bufferViews": [
    {{ "buffer": 0, "byteOffset": {o0}, "byteLength": {l0} }},
    {{ "buffer": 0, "byteOffset": {o1}, "byteLength": {l1} }},
    {{ "buffer": 0, "byteOffset": {o2}, "byteLength": {l2} }},
    {{ "buffer": 0, "byteOffset": {o3}, "byteLength": {l3} }},
    {{ "buffer": 0, "byteOffset": {o4}, "byteLength": {l4} }},
    {{ "buffer": 0, "byteOffset": {o5}, "byteLength": {l5} }},
    {{ "buffer": 0, "byteOffset": {o6}, "byteLength": {l6} }},
    {{ "buffer": 0, "byteOffset": {o7}, "byteLength": {l7} }},
    {{ "buffer": 0, "byteOffset": {o8}, "byteLength": {l8} }}
  ],
  "accessors": [
    {{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0,0,0], "max": [1,1,0] }},
    {{ "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3" }},
    {{ "bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC2" }},
    {{ "bufferView": 3, "componentType": 5123, "count": 3, "type": "VEC4" }},
    {{ "bufferView": 4, "componentType": 5126, "count": 3, "type": "VEC4" }},
    {{ "bufferView": 5, "componentType": 5123, "count": 3, "type": "SCALAR" }},
    {{ "bufferView": 6, "componentType": 5126, "count": 2, "type": "MAT4" }},
    {{ "bufferView": 7, "componentType": 5126, "count": 2, "type": "SCALAR", "min": [0.0], "max": [1.0] }},
    {{ "bufferView": 8, "componentType": 5126, "count": 2, "type": "VEC3" }}
  ]
}}"#,
            o0 = views[0].0,
            l0 = views[0].1,
            o1 = views[1].0,
            l1 = views[1].1,
            o2 = views[2].0,
            l2 = views[2].1,
            o3 = views[3].0,
            l3 = views[3].1,
            o4 = views[4].0,
            l4 = views[4].1,
            o5 = views[5].0,
            l5 = views[5].1,
            o6 = views[6].0,
            l6 = views[6].1,
            o7 = views[7].0,
            l7 = views[7].1,
            o8 = views[8].0,
            l8 = views[8].1,
        );
        make_glb(&json, &bin)
    }

    #[test]
    fn the_skin_and_clip_import() {
        let loaded = load_gltf_bytes(&skinned_triangle_glb()).expect("a skinned model imports");

        // The skin: two joints and their inverse bind transforms.
        assert_eq!(loaded.skins.len(), 1);
        assert_eq!(loaded.skins[0].name, "Rig");
        assert_eq!(loaded.skins[0].joints, vec![0, 1]);
        assert_eq!(loaded.skins[0].inverse_bind.len(), 2);
        assert_eq!(loaded.nodes[0].skin, Some(0));

        // The per-vertex influences, parallel to the vertices.
        let influences = &loaded.meshes[0].influences;
        assert_eq!(influences.len(), 3);
        assert_eq!(influences[2].joints[0], 1, "the third vertex is on bone 1");
        assert_eq!(influences[0].weights[0], 1.0);

        // The clip: one translation channel on bone 1, two keys over one second.
        assert_eq!(loaded.animations.len(), 1);
        let clip = &loaded.animations[0];
        assert_eq!(clip.name, "Wiggle");
        assert_eq!(clip.duration, 1.0);
        assert_eq!(clip.channels.len(), 1);
        let channel = &clip.channels[0];
        assert_eq!(channel.node, 1);
        assert_eq!(channel.property, AnimationProperty::Translation);
        assert_eq!(channel.times, vec![0.0, 1.0]);
        assert_eq!(
            channel.values,
            vec![[0.0, 1.0, 0.0, 0.0], [0.0, 2.0, 0.0, 0.0]]
        );

        // Registering returns handles, and the clip resolves by name.
        let mut registry = AssetRegistry::new();
        let handles = loaded.register(&mut registry);
        assert_eq!(handles.skins.len(), 1);
        assert_eq!(handles.animations.len(), 1);
        assert!(registry.animations.handle("Wiggle").is_some());

        // Instantiating attaches a skinned renderer with its joints linked.
        let mut world = World::new();
        let nodes = loaded.instantiate(&mut world, &handles);
        assert!(nodes.len() >= 2);
        let (child, _) = world
            .query::<SkinnedMeshRenderer>()
            .next()
            .expect("a skinned renderer was attached");
        assert_eq!(world.get::<Skeleton>(child).map(|s| s.0.len()), Some(2));
    }

    #[test]
    fn unnamed_textures_register_as_distinct_assets() {
        // A glTF whose textures are unnamed (all called "texture"), each in a
        // different material slot. They must not collapse into one asset, or every
        // map would sample whichever image last won the name.
        let loaded = LoadedGltf {
            textures: vec![
                Texture::solid("texture", [255, 0, 0, 255], TextureFormat::Rgba8Unorm),
                Texture::solid("texture", [0, 255, 0, 255], TextureFormat::Rgba8Unorm),
            ],
            materials: vec![
                Material {
                    name: "A".into(),
                    base_color_texture: AssetHandle::from_raw(1),
                    ..Material::default()
                },
                Material {
                    name: "B".into(),
                    normal_texture: AssetHandle::from_raw(2),
                    ..Material::default()
                },
            ],
            ..LoadedGltf::default()
        };

        let mut registry = AssetRegistry::new();
        let handles = loaded.register(&mut registry);

        assert_eq!(handles.textures.len(), 2);
        assert_ne!(handles.textures[0].id(), handles.textures[1].id());
        assert_eq!(registry.textures.len(), 2);

        // Each material's map resolves to the texture it named, not the other's.
        let a = registry.materials.get(handles.materials[0]).expect("A");
        let b = registry.materials.get(handles.materials[1]).expect("B");
        assert_eq!(a.base_color_texture.id(), handles.textures[0].id());
        assert_eq!(b.normal_texture.id(), handles.textures[1].id());
    }

    /// A triangle whose material carries `KHR_materials_unlit`.
    fn unlit_triangle_glb() -> Vec<u8> {
        let positions: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let normals: [[f32; 3]; 3] = [[0.0, 0.0, 1.0]; 3];
        let uvs: [[f32; 2]; 3] = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        let indices: [u16; 3] = [0, 1, 2];

        let mut bin = Vec::new();
        for p in positions {
            for c in p {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        let normal_offset = bin.len();
        for n in normals {
            for c in n {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        let uv_offset = bin.len();
        for uv in uvs {
            for c in uv {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        let index_offset = bin.len();
        for i in indices {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        let total = bin.len();

        let json = format!(
            r#"{{
  "asset": {{ "version": "2.0" }},
  "extensionsUsed": ["KHR_materials_unlit"],
  "scene": 0,
  "scenes": [ {{ "name": "Main", "nodes": [0] }} ],
  "nodes": [ {{ "name": "Tri", "mesh": 0 }} ],
  "meshes": [ {{ "name": "TriMesh", "primitives": [
     {{ "attributes": {{ "POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2 }}, "indices": 3, "material": 0 }}
  ] }} ],
  "materials": [ {{ "name": "Flat", "pbrMetallicRoughness": {{
     "baseColorFactor": [0.2, 0.4, 0.8, 1.0]
  }}, "extensions": {{ "KHR_materials_unlit": {{}} }} }} ],
  "buffers": [ {{ "byteLength": {total} }} ],
  "bufferViews": [
    {{ "buffer": 0, "byteOffset": 0, "byteLength": 36 }},
    {{ "buffer": 0, "byteOffset": {normal_offset}, "byteLength": 36 }},
    {{ "buffer": 0, "byteOffset": {uv_offset}, "byteLength": 24 }},
    {{ "buffer": 0, "byteOffset": {index_offset}, "byteLength": 6 }}
  ],
  "accessors": [
    {{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0,0,0], "max": [1,1,0] }},
    {{ "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3" }},
    {{ "bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC2" }},
    {{ "bufferView": 3, "componentType": 5123, "count": 3, "type": "SCALAR" }}
  ]
}}"#
        );
        make_glb(&json, &bin)
    }

    #[test]
    fn unlit_materials_import() {
        let loaded = load_gltf_bytes(&unlit_triangle_glb()).expect("import");
        assert!(loaded.materials[0].unlit, "KHR_materials_unlit sets unlit");

        // A material without the extension stays lit.
        let lit = load_gltf_bytes(&triangle_glb()).expect("import");
        assert!(!lit.materials[0].unlit);
    }

    #[test]
    fn mag_filter_maps_to_texture_filter() {
        use gltf::texture::MagFilter;
        assert_eq!(
            filter_from_mag_filter(Some(MagFilter::Nearest)),
            TextureFilter::Nearest
        );
        assert_eq!(
            filter_from_mag_filter(Some(MagFilter::Linear)),
            TextureFilter::Linear
        );
        assert_eq!(filter_from_mag_filter(None), TextureFilter::Linear);
    }

    #[test]
    fn nodes_and_roots() {
        let loaded = load_gltf_bytes(&triangle_glb()).expect("import");
        assert_eq!(loaded.roots, vec![0]);
        assert_eq!(loaded.scene_name.as_deref(), Some("Main"));
        let node = &loaded.nodes[0];
        assert_eq!(node.name, "Tri");
        assert_eq!(node.parent, None);
        assert_eq!(node.mesh, Some(0));
        assert_eq!(node.mesh_primitives, vec![0]);
        assert_eq!(node.transform, Transform::identity());
    }

    #[test]
    fn register_produces_handles() {
        let loaded = load_gltf_bytes(&triangle_glb()).expect("import");
        let mut registry = AssetRegistry::new();
        let handles = loaded.register(&mut registry);
        assert_eq!(handles.meshes.len(), 1);
        assert_eq!(handles.materials.len(), 1);
        assert!(!handles.default_material.is_none());
        assert!(registry.meshes.get(handles.meshes[0]).is_some());
        assert_eq!(
            registry.materials.get(handles.materials[0]).unwrap().name,
            "Red"
        );
    }

    #[test]
    fn register_works_on_non_empty_registry() {
        let loaded = load_gltf_bytes(&triangle_glb()).expect("import");
        let mut registry = AssetRegistry::with_defaults();
        // A pre-existing texture shifts every later texture id.
        registry.textures.insert(Texture::solid(
            "existing",
            [1, 2, 3, 255],
            TextureFormat::Rgba8Unorm,
        ));
        let handles = loaded.register(&mut registry);
        assert_eq!(handles.meshes.len(), 1);
        assert!(registry.meshes.get(handles.meshes[0]).is_some());
        // Default material resolves to the registry's `$default`.
        assert_eq!(
            handles.default_material,
            registry.materials.handle("$default").unwrap()
        );
    }

    #[test]
    fn instantiate_spawns_node_and_mesh_entities() {
        let loaded = load_gltf_bytes(&triangle_glb()).expect("import");
        let mut registry = AssetRegistry::new();
        let handles = loaded.register(&mut registry);
        let mut world = World::new();
        let nodes = loaded.instantiate(&mut world, &handles);
        assert_eq!(nodes.len(), 1);
        assert!(world.has::<Name>(nodes[0]));
        assert!(world.has::<LocalTransform>(nodes[0]));
        // The renderer child exists and references the imported mesh.
        let renderers: Vec<_> = world.query::<MeshRenderer>().collect();
        assert_eq!(renderers.len(), 1);
        assert_eq!(renderers[0].1.mesh, handles.meshes[0]);
        assert_eq!(renderers[0].1.material, handles.materials[0]);
        assert_eq!(world.count::<Name>(), 2);
    }

    #[test]
    fn invalid_bytes_report_gltf_error() {
        let err = load_gltf_bytes(b"not a gltf").unwrap_err();
        assert!(matches!(err, RenderError::Gltf(_)));
    }

    #[test]
    fn image_conversion_rgba() {
        let image = gltf::image::Data {
            pixels: vec![10, 20, 30, 40],
            format: gltf::image::Format::R8G8B8A8,
            width: 1,
            height: 1,
        };
        assert_eq!(image_to_rgba8(&image).unwrap(), vec![10, 20, 30, 40]);
    }

    #[test]
    fn image_conversion_rgb_expands_alpha() {
        let image = gltf::image::Data {
            pixels: vec![1, 2, 3],
            format: gltf::image::Format::R8G8B8,
            width: 1,
            height: 1,
        };
        assert_eq!(image_to_rgba8(&image).unwrap(), vec![1, 2, 3, 255]);
    }
}
