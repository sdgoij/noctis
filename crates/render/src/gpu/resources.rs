//! GPU-side asset resources and their lazy cache.
//!
//! CPU assets (from [`ecs::assets`]) are uploaded to the GPU on first use and
//! kept alive in a [`ResourceCache`] keyed by [`AssetId`]. Meshes become vertex
//! and index buffers, textures become sampled 2D textures, and materials become
//! a bind group over five texture slots plus a sampler. Missing textures fall
//! back to a 1x1 white / flat-normal texture so the shader can always bind
//! something.

use ecs::assets::{AssetId, AssetRegistry, Material, Mesh, Texture, TextureFormat};
use std::collections::HashMap;
use wgpu::util::DeviceExt;

/// The GPU vertex layout, byte-identical to [`ecs::assets::Vertex`].
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable, PartialEq, Default)]
pub struct GpuVertex {
    /// Object-space position.
    pub position: [f32; 3],
    /// Object-space normal.
    pub normal: [f32; 3],
    /// Texture coordinate 0.
    pub uv: [f32; 2],
    /// Tangent with handedness in `w`.
    pub tangent: [f32; 4],
}

impl GpuVertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
        0 => Float32x3,
        1 => Float32x3,
        2 => Float32x2,
        3 => Float32x4,
    ];

    /// The vertex buffer layout for the standard mesh format.
    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<GpuVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// A vertex's skin influence, a second vertex buffer beside [`GpuVertex`].
///
/// Parallel to a skinned mesh's vertices; absent for a static mesh. Joint indices
/// are `f32` here (the shader truncates them) so the attribute is a plain float
/// vector like the rest of the layout.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable, PartialEq, Default)]
pub struct SkinVertex {
    /// Joint indices into the draw's joint matrices.
    pub joints: [f32; 4],
    /// Blend weights.
    pub weights: [f32; 4],
}

impl SkinVertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![4 => Float32x4, 5 => Float32x4];

    /// The skin buffer's vertex layout (shader locations 4 and 5).
    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<SkinVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// An uploaded mesh.
#[derive(Debug)]
pub struct GpuMesh {
    /// Vertex buffer.
    pub vertex_buffer: wgpu::Buffer,
    /// Joint indices and weights, one per vertex; `None` for a static mesh.
    pub skin_buffer: Option<wgpu::Buffer>,
    /// Index buffer.
    pub index_buffer: wgpu::Buffer,
    /// Number of indices.
    pub index_count: u32,
}

impl GpuMesh {
    fn upload(device: &wgpu::Device, name: &str, mesh: &Mesh) -> GpuMesh {
        let vertices: Vec<GpuVertex> = mesh
            .vertices
            .iter()
            .map(|v| GpuVertex {
                position: v.position,
                normal: v.normal,
                uv: v.uv,
                tangent: v.tangent,
            })
            .collect();
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{name}.vertices")),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        // A skinned mesh carries a second vertex buffer with the joint indices and
        // weights; a static mesh has none.
        let skin_buffer = (!mesh.influences.is_empty()).then(|| {
            let skin: Vec<SkinVertex> = mesh
                .influences
                .iter()
                .map(|influence| SkinVertex {
                    joints: [
                        f32::from(influence.joints[0]),
                        f32::from(influence.joints[1]),
                        f32::from(influence.joints[2]),
                        f32::from(influence.joints[3]),
                    ],
                    weights: influence.weights,
                })
                .collect();
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("{name}.skin")),
                contents: bytemuck::cast_slice(&skin),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{name}.indices")),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        GpuMesh {
            vertex_buffer,
            skin_buffer,
            index_buffer,
            index_count: mesh.indices.len() as u32,
        }
    }
}

/// An uploaded 2D texture.
#[derive(Debug)]
pub struct GpuTexture {
    /// The texture.
    pub texture: wgpu::Texture,
    /// The default view.
    pub view: wgpu::TextureView,
}

impl GpuTexture {
    fn upload(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        texture: &Texture,
    ) -> GpuTexture {
        let format = match texture.format {
            TextureFormat::Rgba8Srgb => wgpu::TextureFormat::Rgba8UnormSrgb,
            TextureFormat::Rgba8Unorm => wgpu::TextureFormat::Rgba8Unorm,
        };
        let width = texture.width.max(1);
        let height = texture.height.max(1);
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let gpu = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(name),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let bytes = if texture.data.len() >= (width * height * 4) as usize {
            texture.data.clone()
        } else {
            // Pad malformed data rather than panic.
            let mut padded = texture.data.clone();
            padded.resize((width * height * 4) as usize, 255);
            padded
        };
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &gpu,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            size,
        );
        let view = gpu.create_view(&wgpu::TextureViewDescriptor::default());
        GpuTexture { texture: gpu, view }
    }
}

/// An uploaded material: a bind group over its textures.
#[derive(Debug)]
pub struct GpuMaterial {
    /// The material bind group (group 2 of the PBR pipeline).
    pub bind_group: wgpu::BindGroup,
}

/// A lazily-populated cache of GPU resources keyed by asset id.
#[derive(Debug)]
pub struct ResourceCache {
    meshes: HashMap<AssetId, CachedMesh>,
    textures: HashMap<AssetId, GpuTexture>,
    materials: HashMap<AssetId, GpuMaterial>,
    white: GpuTexture,
    flat_normal: GpuTexture,
    black: GpuTexture,
}

/// An uploaded mesh together with the asset revision it was uploaded at, so a
/// mesh whose asset was rebuilt in place is re-uploaded rather than drawn stale.
#[derive(Debug)]
struct CachedMesh {
    gpu: GpuMesh,
    revision: u64,
}

impl ResourceCache {
    /// Create the cache and its 1x1 fallback textures.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> ResourceCache {
        let white = GpuTexture::upload(
            device,
            queue,
            "$white",
            &Texture::solid("$white", [255, 255, 255, 255], TextureFormat::Rgba8Srgb),
        );
        let flat_normal = GpuTexture::upload(
            device,
            queue,
            "$normal",
            &Texture::solid("$normal", [128, 128, 255, 255], TextureFormat::Rgba8Unorm),
        );
        let black = GpuTexture::upload(
            device,
            queue,
            "$black",
            &Texture::solid("$black", [0, 0, 0, 255], TextureFormat::Rgba8Unorm),
        );
        ResourceCache {
            meshes: HashMap::new(),
            textures: HashMap::new(),
            materials: HashMap::new(),
            white,
            flat_normal,
            black,
        }
    }

    /// Number of cached GPU meshes.
    pub fn mesh_count(&self) -> usize {
        self.meshes.len()
    }

    /// Number of cached GPU textures.
    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }

    /// Number of cached GPU materials.
    pub fn material_count(&self) -> usize {
        self.materials.len()
    }

    /// Get or upload a mesh.
    ///
    /// `revision` is the owning asset's revision (see `Assets::revision`); when it
    /// differs from the revision the cached mesh was uploaded at, the mesh is
    /// re-uploaded, so a caller that rebuilds a mesh in place (a following
    /// terrain, a meadow) is drawn from its new data.
    pub fn mesh(
        &mut self,
        device: &wgpu::Device,
        id: AssetId,
        revision: u64,
        mesh: &Mesh,
    ) -> &GpuMesh {
        let cached = self.meshes.entry(id).or_insert_with(|| CachedMesh {
            gpu: GpuMesh::upload(device, &mesh.name, mesh),
            revision,
        });
        if cached.revision != revision {
            cached.gpu = GpuMesh::upload(device, &mesh.name, mesh);
            cached.revision = revision;
        }
        &cached.gpu
    }

    /// Get or upload a texture.
    pub fn texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        id: AssetId,
        texture: &Texture,
    ) -> &GpuTexture {
        self.textures
            .entry(id)
            .or_insert_with(|| GpuTexture::upload(device, queue, &texture.name, texture))
    }

    fn view_for(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        assets: &AssetRegistry,
        handle: ecs::assets::AssetHandle<Texture>,
        fallback: &wgpu::TextureView,
        fallback_id: u8,
    ) -> wgpu::TextureView {
        if handle.is_none() {
            return fallback.clone();
        }
        match assets.textures.get(handle) {
            Some(texture) => {
                let gpu = self.texture(device, queue, handle.id(), texture);
                gpu.view.clone()
            }
            None => {
                let _ = fallback_id;
                fallback.clone()
            }
        }
    }

    /// The GPU view for a texture handle, or the shared white fallback when the
    /// handle is empty or missing. The shader seam's texture array (`.notes/shaders.md`)
    /// needs one view per slot, so every slot has something to bind.
    pub fn texture_view(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        assets: &AssetRegistry,
        handle: ecs::assets::AssetHandle<Texture>,
    ) -> wgpu::TextureView {
        let white = self.white.view.clone();
        self.view_for(device, queue, assets, handle, &white, 0)
    }

    /// Get or create a material bind group, resolving its textures (with
    /// fallbacks) against `assets`.
    // The parameters are the device/queue plus the material and its texture
    // resolution inputs; a one-use struct would only add indirection here.
    #[allow(clippy::too_many_arguments)]
    pub fn material(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        id: AssetId,
        material: &Material,
        assets: &AssetRegistry,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
    ) -> &GpuMaterial {
        if !self.materials.contains_key(&id) {
            let white = self.white.view.clone();
            let flat = self.flat_normal.view.clone();
            let black = self.black.view.clone();

            let base = self.view_for(
                device,
                queue,
                assets,
                material.base_color_texture,
                &white,
                0,
            );
            let normal = self.view_for(device, queue, assets, material.normal_texture, &flat, 1);
            let mr = self.view_for(
                device,
                queue,
                assets,
                material.metallic_roughness_texture,
                &white,
                2,
            );
            let emissive =
                self.view_for(device, queue, assets, material.emissive_texture, &black, 3);
            let occlusion =
                self.view_for(device, queue, assets, material.occlusion_texture, &white, 4);

            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("material{}", id.0)),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&base),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&normal),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&mr),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&emissive),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&occlusion),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            });
            self.materials.insert(id, GpuMaterial { bind_group });
        }
        &self.materials[&id]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_vertex_matches_cpu_vertex_layout() {
        assert_eq!(
            std::mem::size_of::<GpuVertex>(),
            std::mem::size_of::<ecs::assets::Vertex>()
        );
        assert_eq!(std::mem::size_of::<GpuVertex>(), 48);
    }

    #[test]
    fn vertex_layout_attributes_are_contiguous() {
        let layout = GpuVertex::layout();
        assert_eq!(layout.array_stride, 48);
        assert_eq!(layout.attributes.len(), 4);
        assert_eq!(layout.attributes[0].offset, 0);
        assert_eq!(layout.attributes[1].offset, 12);
        assert_eq!(layout.attributes[2].offset, 24);
        assert_eq!(layout.attributes[3].offset, 32);
    }
}
