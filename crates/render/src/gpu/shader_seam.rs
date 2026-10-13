//! GPU resources for the shader seam (`.notes/shaders.md`).
//!
//! Per-[`Shader`] pipelines and per-[`ShaderMaterial`] bind groups, cached and
//! rebuilt when the owning asset's revision changes. A shader is authored for one
//! form — a **mesh** body (opaque + alpha-blended pipelines) or a **screen** body
//! (one full-screen pass) — so only the form it is used as is ever compiled: a
//! screen body that names `ScreenOut` could not also be a mesh vertex stage.
//!
//! The material builds the texture array (group 2, one view per slot with a white
//! fallback) and the user uniform block (group 3).

use std::collections::HashMap;

use ecs::assets::{
    AssetHandle, AssetId, AssetRegistry, Shader, ShaderMaterial, TEXTURE_SLOTS, USER_SLOTS,
};
use wgpu::util::DeviceExt;

use crate::gpu::pipelines::{BindGroupLayouts, create_shader_pipeline};
use crate::gpu::resources::ResourceCache;
use crate::shader::ShadeForm;

/// The two pipelines a mesh shader needs: opaque and alpha-blended.
#[derive(Debug)]
pub struct MeshPipelines {
    /// Opaque custom-shader mesh draws.
    pub opaque: wgpu::RenderPipeline,
    /// Alpha-blended custom-shader mesh draws.
    pub transparent: wgpu::RenderPipeline,
    revision: u64,
}

/// The pipeline a screen shader needs: one full-screen pass.
#[derive(Debug)]
pub struct ScreenPipeline {
    /// The full-screen pass (a sky).
    pub pipeline: wgpu::RenderPipeline,
    revision: u64,
}

/// A shader material's seam bind groups: the texture array (group 2) and the user
/// uniform block (group 3).
#[derive(Debug)]
pub struct SeamMaterial {
    /// Group 2: one texture binding per slot plus the shared sampler.
    pub textures: wgpu::BindGroup,
    /// Group 3: the user uniform block.
    pub user: wgpu::BindGroup,
    revision: u64,
    /// Kept alive so the user bind group's buffer outlives it.
    _uniform: wgpu::Buffer,
}

/// The seam's GPU cache, keyed by asset id.
#[derive(Debug, Default)]
pub struct ShaderSeam {
    meshes: HashMap<AssetId, MeshPipelines>,
    screens: HashMap<AssetId, ScreenPipeline>,
    materials: HashMap<AssetId, SeamMaterial>,
}

impl ShaderSeam {
    /// Get (building if new or stale) the mesh pipelines for a `Shader` used by a
    /// [`ShaderMeshRenderer`](ecs::components::ShaderMeshRenderer).
    pub fn mesh_pipelines(
        &mut self,
        device: &wgpu::Device,
        layouts: &BindGroupLayouts,
        shader: &Shader,
        id: AssetId,
        revision: u64,
        sample_count: u32,
    ) -> &MeshPipelines {
        let stale = self
            .meshes
            .get(&id)
            .is_none_or(|cached| cached.revision != revision);
        if stale {
            self.meshes.insert(
                id,
                MeshPipelines {
                    opaque: create_shader_pipeline(
                        device,
                        layouts,
                        shader,
                        ShadeForm::Mesh,
                        None,
                        true,
                        sample_count,
                    ),
                    transparent: create_shader_pipeline(
                        device,
                        layouts,
                        shader,
                        ShadeForm::Mesh,
                        Some(wgpu::BlendState::ALPHA_BLENDING),
                        false,
                        sample_count,
                    ),
                    revision,
                },
            );
        }
        &self.meshes[&id]
    }

    /// Get (building if new or stale) the full-screen pipeline for a `Shader` used
    /// by a [`ScreenShaderMaterial`](ecs::components::ScreenShaderMaterial).
    pub fn screen_pipeline(
        &mut self,
        device: &wgpu::Device,
        layouts: &BindGroupLayouts,
        shader: &Shader,
        id: AssetId,
        revision: u64,
        sample_count: u32,
    ) -> &ScreenPipeline {
        let stale = self
            .screens
            .get(&id)
            .is_none_or(|cached| cached.revision != revision);
        if stale {
            self.screens.insert(
                id,
                ScreenPipeline {
                    pipeline: create_shader_pipeline(
                        device,
                        layouts,
                        shader,
                        ShadeForm::Screen,
                        None,
                        false,
                        sample_count,
                    ),
                    revision,
                },
            );
        }
        &self.screens[&id]
    }

    /// Get (building if new or stale) the bind groups for `material`.
    #[allow(clippy::too_many_arguments)]
    pub fn material(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cache: &mut ResourceCache,
        assets: &AssetRegistry,
        id: AssetId,
        material: &ShaderMaterial,
        revision: u64,
        textures_layout: &wgpu::BindGroupLayout,
        user_layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
    ) -> &SeamMaterial {
        let stale = self
            .materials
            .get(&id)
            .is_none_or(|cached| cached.revision != revision);
        if stale {
            // The texture array: one view per slot (white where the material names
            // none), then the shared sampler. The views are held until the bind
            // group is created, because the entries borrow them.
            let views: Vec<wgpu::TextureView> = (0..TEXTURE_SLOTS)
                .map(|slot| {
                    let handle = material
                        .textures
                        .get(slot)
                        .copied()
                        .unwrap_or(AssetHandle::NONE);
                    cache.texture_view(device, queue, assets, handle)
                })
                .collect();
            let mut entries: Vec<wgpu::BindGroupEntry> = views
                .iter()
                .enumerate()
                .map(|(slot, view)| wgpu::BindGroupEntry {
                    binding: slot as u32,
                    resource: wgpu::BindingResource::TextureView(view),
                })
                .collect();
            entries.push(wgpu::BindGroupEntry {
                binding: TEXTURE_SLOTS as u32,
                resource: wgpu::BindingResource::Sampler(sampler),
            });
            let textures = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("shader.textures.bind_group"),
                layout: textures_layout,
                entries: &entries,
            });

            // The user block: the material's floats, truncated/padded to capacity.
            let mut data = vec![0.0f32; USER_SLOTS * 4];
            for (slot, value) in material.uniforms.iter().take(USER_SLOTS * 4).enumerate() {
                data[slot] = *value;
            }
            let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("shader.user.uniform"),
                contents: bytemuck::cast_slice(&data),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            let user = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("shader.user.bind_group"),
                layout: user_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                }],
            });

            self.materials.insert(
                id,
                SeamMaterial {
                    textures,
                    user,
                    revision,
                    _uniform: uniform,
                },
            );
        }
        &self.materials[&id]
    }
}
