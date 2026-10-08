//! The headless wgpu renderer.
//!
//! [`Renderer::headless`] builds a GPU device without a window or surface and
//! renders into offscreen textures. A frame runs the following passes, in order:
//!
//! 1. **Shadow pass** — the primary shadow-casting directional/spot light
//!    renders opaque geometry into a depth32float shadow map.
//! 2. **Depth prepass** — opaque geometry into the camera depth buffer, giving
//!    SSAO a full-depth source and the forward pass free early-z.
//! 3. **SSAO** — reconstructs view-space positions from depth and writes the
//!    ambient occlusion term.
//! 4. **Forward PBR** — opaque then transparent draws into the HDR target using
//!    the Cook–Torrance GGX shader with IBL, PCF shadows and SSAO.
//! 5. **Gizmo** — optional editor gizmo lines and triangles.
//! 6. **Tonemap** — the HDR target is resolved to an sRGB-encoded LDR target.
//!
//! The LDR result is copied into a mappable buffer and returned as a [`Frame`].
//!
//! If the machine has no usable adapter, [`Renderer::headless`] returns
//! [`RenderError::NoAdapter`]; callers (and tests) should skip rendering rather
//! than fail.

use crate::camera::{CameraView, extract_camera};
use crate::draw_list::{DrawItem, extract_draw_list};
use crate::error::RenderError;
use crate::gizmo::{GizmoGeometry, GizmoVertex};
use crate::gpu::ibl_gpu::IblGpu;
use crate::gpu::pipelines::{
    BindGroupLayouts, FrameBindings, ObjectBindings, Pipelines, create_layouts, create_pipelines,
    create_present_pipeline, pick_surface_format,
};
use crate::gpu::resources::ResourceCache;
use crate::gpu::shadow::ShadowResources;
use crate::gpu::targets::RenderTargets;
use crate::gpu::uniforms::{
    FrameUniform, ObjectUniform, ShadowUniform, SsaoUniform, TonemapUniform, ViewUniform,
    ssao_kernel,
};
use crate::ibl::{IblConfig, IblData};
use crate::lights::{LightsUniform, MAX_LIGHTS, extract_lights, primary_shadow_light};
use crate::tonemap::Tonemap;
use ecs::assets::{AssetId, AssetRegistry, HdrTexture, Material};
use ecs::components::LightKind;
use ecs::world::{Entity, World};
use math::{Aabb, Mat4, vec3};
use wgpu::util::DeviceExt;

/// Configuration for a headless [`Renderer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RendererConfig {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Whether lighting is computed in the HDR (`rgba16float`) pipeline before
    /// tonemapping. Only `true` is implemented; `false` falls back to the same
    /// pipeline with a lower exposure.
    pub hdr: bool,
    /// Requested MSAA sample count. Only `1` is implemented today; other values
    /// are clamped and a warning is logged.
    pub sample_count: u32,
}

impl Default for RendererConfig {
    fn default() -> Self {
        RendererConfig {
            width: 1280,
            height: 720,
            hdr: true,
            sample_count: 1,
        }
    }
}

/// A rendered frame as tightly packed RGBA8 sRGB bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// `width * height * 4` RGBA bytes.
    pub rgba: Vec<u8>,
}

impl Frame {
    /// The pixels as a byte slice.
    pub fn to_rgba8(&self) -> Vec<u8> {
        self.rgba.clone()
    }

    /// Read a single pixel, or `None` if out of bounds.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = ((y * self.width + x) * 4) as usize;
        Some([
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ])
    }
}

/// A prepared draw ready to be issued inside a render pass.
struct PreparedDraw {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    offset: u32,
    material: wgpu::BindGroup,
}

/// The offscreen PBR renderer.
pub struct Renderer {
    config: RendererConfig,
    device: wgpu::Device,
    queue: wgpu::Queue,
    targets: RenderTargets,
    shadow: ShadowResources,
    layouts: BindGroupLayouts,
    pipelines: Pipelines,
    ibl: IblGpu,
    frame: FrameBindings,
    objects: ObjectBindings,
    cache: ResourceCache,
    material_sampler: wgpu::Sampler,
    camera_view_buffer: wgpu::Buffer,
    camera_view_bind_group: wgpu::BindGroup,
    light_view_buffer: wgpu::Buffer,
    light_view_bind_group: wgpu::BindGroup,
    ssao_buffer: wgpu::Buffer,
    ssao_bind_group: wgpu::BindGroup,
    tonemap_buffer: wgpu::Buffer,
    tonemap_bind_group: wgpu::BindGroup,
    gizmo_view_buffer: wgpu::Buffer,
    gizmo_view_bind_group: wgpu::BindGroup,
    gizmo_lines: Option<(wgpu::Buffer, u32)>,
    gizmo_triangles: Option<(wgpu::Buffer, u32)>,
    gizmo_triangle_indices: Option<wgpu::Buffer>,
    ambient: [f32; 4],
    exposure: f32,
    tonemap_mode: Tonemap,
    ssao_strength: f32,
    clear_color: [f32; 4],
    surface: Option<wgpu::Surface<'static>>,
    surface_config: Option<wgpu::SurfaceConfiguration>,
    present_pipeline: Option<wgpu::RenderPipeline>,
    present_bind_group: Option<wgpu::BindGroup>,
    present_format: Option<wgpu::TextureFormat>,
}

impl std::fmt::Debug for Renderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Renderer")
            .field("config", &self.config)
            .field("shadow_size", &self.shadow.size)
            .finish_non_exhaustive()
    }
}

fn uniform_buffer(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

impl Renderer {
    /// Create a headless renderer rendering into offscreen textures.
    ///
    /// Returns [`RenderError::NoAdapter`] when no graphics adapter is available.
    pub fn headless(config: RendererConfig) -> Result<Renderer, RenderError> {
        let instance = wgpu::Instance::default();
        let options = wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        };
        let adapter = pollster::block_on(instance.request_adapter(&options))
            .map_err(|_| RenderError::NoAdapter)?;

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("noctis.render.device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .map_err(|e| RenderError::RequestDevice(e.to_string()))?;

        Ok(Self::build_core(device, queue, config))
    }

    /// Shared GPU-resource construction, used by both the headless and the
    /// surface renderer. A surface is attached afterwards, if any.
    fn build_core(device: wgpu::Device, queue: wgpu::Queue, config: RendererConfig) -> Renderer {
        let mut config = config;
        if config.sample_count != 1 {
            log::warn!(
                "render: sample_count {} requested but only 1 is implemented; using 1",
                config.sample_count
            );
            config.sample_count = 1;
        }

        let targets = RenderTargets::create(&device, config.width, config.height);
        let shadow = ShadowResources::new(&device, 2048);
        let layouts = create_layouts(&device);
        let pipelines = create_pipelines(&device, &layouts);
        let ibl = IblGpu::create(&device, &queue, &IblData::neutral());
        let frame = FrameBindings::new(
            &device,
            &layouts.frame,
            &ibl,
            &shadow.view,
            &targets.ssao.view,
        );
        let objects = ObjectBindings::new(&device, &layouts.object, 256);
        let cache = ResourceCache::new(&device, &queue);

        let material_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("material.sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        let camera_view_buffer = uniform_buffer(
            &device,
            "view.camera",
            std::mem::size_of::<ViewUniform>() as u64,
        );
        let light_view_buffer = uniform_buffer(
            &device,
            "view.light",
            std::mem::size_of::<ViewUniform>() as u64,
        );
        let camera_view_bind_group =
            make_view_bind_group(&device, &layouts.view, &camera_view_buffer);
        let light_view_bind_group =
            make_view_bind_group(&device, &layouts.view, &light_view_buffer);
        let gizmo_view_buffer = uniform_buffer(
            &device,
            "view.gizmo",
            std::mem::size_of::<ViewUniform>() as u64,
        );
        let gizmo_view_bind_group =
            make_view_bind_group(&device, &layouts.view, &gizmo_view_buffer);

        let ssao_buffer = uniform_buffer(
            &device,
            "ssao.uniform",
            std::mem::size_of::<SsaoUniform>() as u64,
        );
        let ssao_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ssao.bind_group"),
            layout: &layouts.ssao,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ssao_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&targets.depth.view),
                },
            ],
        });

        let tonemap_buffer = uniform_buffer(
            &device,
            "tonemap.uniform",
            std::mem::size_of::<TonemapUniform>() as u64,
        );
        let tonemap_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tonemap.bind_group"),
            layout: &layouts.tonemap,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: tonemap_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&targets.hdr.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&ibl.sampler),
                },
            ],
        });

        Renderer {
            config,
            device,
            queue,
            targets,
            shadow,
            layouts,
            pipelines,
            ibl,
            frame,
            objects,
            cache,
            material_sampler,
            camera_view_buffer,
            camera_view_bind_group,
            light_view_buffer,
            light_view_bind_group,
            ssao_buffer,
            ssao_bind_group,
            tonemap_buffer,
            tonemap_bind_group,
            gizmo_view_buffer,
            gizmo_view_bind_group,
            gizmo_lines: None,
            gizmo_triangles: None,
            gizmo_triangle_indices: None,
            ambient: [1.0, 1.0, 1.0, 1.0],
            exposure: 1.0,
            tonemap_mode: Tonemap::Aces,
            ssao_strength: 1.0,
            clear_color: [0.02, 0.02, 0.03, 1.0],
            surface: None,
            surface_config: None,
            present_pipeline: None,
            present_bind_group: None,
            present_format: None,
        }
    }

    /// The active configuration.
    pub fn config(&self) -> RendererConfig {
        self.config
    }

    /// The frame dimensions.
    pub fn size(&self) -> (u32, u32) {
        (self.targets.width, self.targets.height)
    }

    /// The number of GPU meshes currently cached.
    pub fn cached_meshes(&self) -> usize {
        self.cache.mesh_count()
    }

    /// The number of GPU materials currently cached.
    pub fn cached_materials(&self) -> usize {
        self.cache.material_count()
    }

    /// Set the ambient (indirect) light color and intensity.
    pub fn set_ambient(&mut self, color: [f32; 3], intensity: f32) {
        self.ambient = [color[0], color[1], color[2], intensity];
    }

    /// Set the tonemap exposure and operator.
    pub fn set_tonemap(&mut self, exposure: f32, mode: Tonemap) {
        self.exposure = exposure.max(0.0);
        self.tonemap_mode = mode;
    }

    /// Set the SSAO strength in `[0, 1]`.
    pub fn set_ssao_strength(&mut self, strength: f32) {
        self.ssao_strength = strength.clamp(0.0, 1.0);
    }

    /// Set the color the HDR target is cleared to (linear RGBA).
    pub fn set_clear_color(&mut self, color: [f32; 4]) {
        self.clear_color = color;
    }

    /// Precompute and upload image-based lighting from an equirectangular HDR.
    pub fn set_environment(&mut self, hdr: &HdrTexture, config: IblConfig) {
        let data = IblData::generate(hdr, config);
        self.ibl = IblGpu::create(&self.device, &self.queue, &data);
        // The frame bind group references the IBL views, so rebuild it.
        self.frame = FrameBindings::new(
            &self.device,
            &self.layouts.frame,
            &self.ibl,
            &self.shadow.view,
            &self.targets.ssao.view,
        );
    }

    /// Replace the current editor gizmo with `geometry`, placed by `transform`.
    pub fn set_gizmo(&mut self, geometry: &GizmoGeometry, transform: &Mat4) {
        let place = |v: &GizmoVertex| -> crate::gizmo::GizmoVertex {
            let p = *transform * math::vec4(v.position[0], v.position[1], v.position[2], 1.0);
            let w = if p.w.abs() > 1e-6 { p.w } else { 1.0 };
            crate::gizmo::GizmoVertex {
                position: [p.x / w, p.y / w, p.z / w],
                color: v.color,
            }
        };
        let lines: Vec<GizmoVertex> = geometry.lines.iter().map(place).collect();
        let triangles: Vec<GizmoVertex> = geometry.triangles.iter().map(place).collect();

        self.gizmo_lines = if lines.is_empty() {
            None
        } else {
            let buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("gizmo.lines"),
                    contents: bytemuck::cast_slice(&lines),
                    usage: wgpu::BufferUsages::VERTEX,
                });
            Some((buffer, lines.len() as u32))
        };
        self.gizmo_triangles = if triangles.is_empty() || geometry.triangle_indices.is_empty() {
            None
        } else {
            let vertices = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("gizmo.triangles"),
                    contents: bytemuck::cast_slice(&triangles),
                    usage: wgpu::BufferUsages::VERTEX,
                });
            let indices = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("gizmo.indices"),
                    contents: bytemuck::cast_slice(&geometry.triangle_indices),
                    usage: wgpu::BufferUsages::INDEX,
                });
            // `triangles` references vertices by index, so the stored count is
            // the number of *indices* the draw call must consume.
            self.gizmo_triangle_indices = Some(indices);
            Some((vertices, geometry.triangle_indices.len() as u32))
        };
    }

    /// Remove the current gizmo.
    pub fn clear_gizmo(&mut self) {
        self.gizmo_lines = None;
        self.gizmo_triangles = None;
        self.gizmo_triangle_indices = None;
    }

    /// Render one frame offscreen and read the pixels back as RGBA8.
    pub fn render_frame(
        &mut self,
        world: &World,
        assets: &AssetRegistry,
        camera: Entity,
    ) -> Result<Frame, RenderError> {
        self.render_scene(world, assets, camera, None)?;
        self.read_back()
    }

    /// Record one frame. When `present_view` is `Some`, the tonemapped LDR
    /// result is blitted into it (a surface texture view); otherwise it is
    /// copied into the readback buffer for [`Renderer::render_frame`].
    fn render_scene(
        &mut self,
        world: &World,
        assets: &AssetRegistry,
        camera: Entity,
        present_view: Option<&wgpu::TextureView>,
    ) -> Result<(), RenderError> {
        let view = extract_camera(world, camera, self.targets.width, self.targets.height)
            .ok_or(RenderError::NoCamera)?;
        let lights = extract_lights(world);
        let lights_uniform = LightsUniform::from_lights(&lights);
        let draw_list = extract_draw_list(world, assets, &view);

        // ---- shadow matrix -------------------------------------------------
        let (shadow_uniform, shadow_matrix, has_shadow) = self.build_shadow(&lights, &draw_list);

        // ---- frame + view + ssao + tonemap uniforms ------------------------
        let width = self.targets.width as f32;
        let height = self.targets.height as f32;
        let frame_uniform = FrameUniform {
            view_proj: math::to_cols_array(&view.view_projection),
            inv_view_proj: math::to_cols_array(&view.inverse_view_projection),
            camera_pos: view.eye4(),
            ambient: self.ambient,
            resolution: [width, height, 1.0 / width, 1.0 / height],
            params: [self.ssao_strength, 0.0, 0.0, 0.0],
        };
        self.frame.write(
            &self.queue,
            &frame_uniform,
            &lights_uniform,
            &shadow_uniform,
        );

        let camera_view = ViewUniform {
            view_proj: math::to_cols_array(&view.view_projection),
        };
        self.queue.write_buffer(
            &self.camera_view_buffer,
            0,
            bytemuck::bytes_of(&camera_view),
        );
        let light_view = ViewUniform {
            view_proj: math::to_cols_array(&shadow_matrix),
        };
        self.queue
            .write_buffer(&self.light_view_buffer, 0, bytemuck::bytes_of(&light_view));
        self.queue
            .write_buffer(&self.gizmo_view_buffer, 0, bytemuck::bytes_of(&camera_view));

        let ssao_uniform = SsaoUniform {
            projection: math::to_cols_array(&view.projection),
            inv_projection: math::to_cols_array(
                &math::inverse(&view.projection).unwrap_or_else(math::identity),
            ),
            samples: ssao_kernel(),
            params: [0.5, 0.025, 1.5, 16.0],
            resolution: [width, height, 1.0 / width, 1.0 / height],
        };
        self.queue
            .write_buffer(&self.ssao_buffer, 0, bytemuck::bytes_of(&ssao_uniform));

        let tonemap_uniform = TonemapUniform {
            exposure: self.exposure,
            mode: match self.tonemap_mode {
                Tonemap::Aces => 0,
                Tonemap::Reinhard => 1,
            },
            pad0: 0.0,
            pad1: 0.0,
        };
        self.queue.write_buffer(
            &self.tonemap_buffer,
            0,
            bytemuck::bytes_of(&tonemap_uniform),
        );

        // ---- prepare draws -------------------------------------------------
        let total = draw_list.len() as u32;
        self.objects.ensure(&self.device, total.max(1));
        let default_material = Material::default();
        let mut prepared_opaque = Vec::with_capacity(draw_list.opaque.len());
        let mut prepared_transparent = Vec::with_capacity(draw_list.transparent.len());
        let mut index = 0u32;
        for item in &draw_list.opaque {
            if let Some(draw) = self.prepare_draw(item, assets, &default_material, index) {
                prepared_opaque.push(draw);
            }
            index += 1;
        }
        for item in &draw_list.transparent {
            if let Some(draw) = self.prepare_draw(item, assets, &default_material, index) {
                prepared_transparent.push(draw);
            }
            index += 1;
        }

        // ---- record --------------------------------------------------------
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame.encoder"),
            });

        // 1. Shadow pass.
        if has_shadow && !prepared_opaque.is_empty() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow.pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow.view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipelines.depth);
            pass.set_bind_group(0, &self.light_view_bind_group, &[]);
            for draw in &prepared_opaque {
                pass.set_bind_group(1, self.objects.bind_group(), &[draw.offset]);
                pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                pass.set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..draw.index_count, 0, 0..1);
            }
        }

        // 2. Depth prepass.
        if !prepared_opaque.is_empty() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("depth.pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.depth.view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipelines.depth);
            pass.set_bind_group(0, &self.camera_view_bind_group, &[]);
            for draw in &prepared_opaque {
                pass.set_bind_group(1, self.objects.bind_group(), &[draw.offset]);
                pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                pass.set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..draw.index_count, 0, 0..1);
            }
        }

        // 3. SSAO.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ssao.pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.ssao.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipelines.ssao);
            pass.set_bind_group(0, &self.ssao_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        // 4. Forward PBR.
        {
            let clear = wgpu::Color {
                r: self.clear_color[0] as f64,
                g: self.clear_color[1] as f64,
                b: self.clear_color[2] as f64,
                a: self.clear_color[3] as f64,
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pbr.pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.hdr.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.depth.view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.frame.bind_group, &[]);
            if !prepared_opaque.is_empty() {
                pass.set_pipeline(&self.pipelines.pbr_opaque);
                for draw in &prepared_opaque {
                    pass.set_bind_group(1, self.objects.bind_group(), &[draw.offset]);
                    pass.set_bind_group(2, &draw.material, &[]);
                    pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                    pass.set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..draw.index_count, 0, 0..1);
                }
            }
            if !prepared_transparent.is_empty() {
                pass.set_pipeline(&self.pipelines.pbr_transparent);
                for draw in &prepared_transparent {
                    pass.set_bind_group(1, self.objects.bind_group(), &[draw.offset]);
                    pass.set_bind_group(2, &draw.material, &[]);
                    pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                    pass.set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..draw.index_count, 0, 0..1);
                }
            }
        }

        // 5. Gizmo.
        if self.gizmo_lines.is_some() || self.gizmo_triangles.is_some() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gizmo.pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.hdr.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.depth.view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.gizmo_view_bind_group, &[]);
            if let Some((buffer, count)) = &self.gizmo_lines {
                pass.set_pipeline(&self.pipelines.gizmo_line);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..*count, 0..1);
            }
            if let (Some((buffer, count)), Some(indices)) =
                (&self.gizmo_triangles, &self.gizmo_triangle_indices)
            {
                pass.set_pipeline(&self.pipelines.gizmo_triangle);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..*count, 0, 0..1);
            }
        }

        // 6. Tonemap.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("tonemap.pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.ldr.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipelines.tonemap);
            pass.set_bind_group(0, &self.tonemap_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        // 7. Present to the surface, or copy into the readback buffer.
        match present_view {
            Some(view) => {
                let pipeline = self.present_pipeline.as_ref().ok_or_else(|| {
                    RenderError::Unsupported("renderer has no present pipeline".into())
                })?;
                let bind_group = self.present_bind_group.as_ref().ok_or_else(|| {
                    RenderError::Unsupported("renderer has no present bind group".into())
                })?;
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("present.pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
            None => {
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.targets.ldr.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &self.targets.readback,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(self.targets.padded_bytes_per_row),
                            rows_per_image: Some(self.targets.height),
                        },
                    },
                    wgpu::Extent3d {
                        width: self.targets.width,
                        height: self.targets.height,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }

        self.queue.submit(Some(encoder.finish()));
        Ok(())
    }

    /// Create a wgpu instance to build a surface from, for [`Renderer::for_surface`].
    pub fn create_instance() -> wgpu::Instance {
        wgpu::Instance::default()
    }

    /// Create a renderer that presents to a window `surface`.
    ///
    /// The `surface` must have been created from the same `instance`
    /// (`instance.create_surface(window)`). Returns [`RenderError::NoAdapter`]
    /// when no adapter can drive the surface.
    pub fn for_surface(
        config: RendererConfig,
        instance: wgpu::Instance,
        surface: wgpu::Surface<'static>,
    ) -> Result<Renderer, RenderError> {
        let options = wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
            apply_limit_buckets: false,
        };
        let adapter = pollster::block_on(instance.request_adapter(&options))
            .map_err(|_| RenderError::NoAdapter)?;
        let caps = surface.get_capabilities(&adapter);
        let format = pick_surface_format(&caps.formats)
            .ok_or_else(|| RenderError::Unsupported("surface advertises no formats".into()))?;

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("noctis.render.device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .map_err(|e| RenderError::RequestDevice(e.to_string()))?;

        let mut renderer = Self::build_core(device, queue, config);
        renderer.attach_surface(surface, &caps, format)?;
        Ok(renderer)
    }

    fn attach_surface(
        &mut self,
        surface: wgpu::Surface<'static>,
        caps: &wgpu::SurfaceCapabilities,
        format: wgpu::TextureFormat,
    ) -> Result<(), RenderError> {
        let entry = if format.is_srgb() {
            "fs_srgb_decode"
        } else {
            "fs_passthrough"
        };
        let pipeline = create_present_pipeline(&self.device, &self.layouts, format, entry);
        let bind_group = self.present_bind_group_for(&self.device);
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: self.targets.width,
            height: self.targets.height,
            present_mode: caps
                .present_modes
                .first()
                .copied()
                .unwrap_or(wgpu::PresentMode::Fifo),
            alpha_mode: caps
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto),
            color_space: wgpu::SurfaceColorSpace::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&self.device, &surface_config);
        self.present_pipeline = Some(pipeline);
        self.present_bind_group = Some(bind_group);
        self.present_format = Some(format);
        self.surface_config = Some(surface_config);
        self.surface = Some(surface);
        Ok(())
    }

    fn present_bind_group_for(&self, device: &wgpu::Device) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("present.bind_group"),
            layout: &self.layouts.present,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.targets.ldr.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.ibl.sampler),
                },
            ],
        })
    }

    /// `true` if this renderer presents to a window surface.
    pub fn has_surface(&self) -> bool {
        self.surface.is_some()
    }

    /// The surface format, if this renderer has a surface.
    pub fn present_format(&self) -> Option<wgpu::TextureFormat> {
        self.present_format
    }

    /// The underlying device (for integrations that must share it).
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// The underlying queue.
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Resize the offscreen targets and, if present, the window surface.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), RenderError> {
        let width = width.max(1);
        let height = height.max(1);
        if width == self.targets.width && height == self.targets.height {
            return Ok(());
        }
        self.config.width = width;
        self.config.height = height;
        self.targets = RenderTargets::create(&self.device, width, height);
        self.frame = FrameBindings::new(
            &self.device,
            &self.layouts.frame,
            &self.ibl,
            &self.shadow.view,
            &self.targets.ssao.view,
        );
        let ssao = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ssao.bind_group"),
            layout: &self.layouts.ssao,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.ssao_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.targets.depth.view),
                },
            ],
        });
        self.ssao_bind_group = ssao;
        let tonemap = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tonemap.bind_group"),
            layout: &self.layouts.tonemap,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.tonemap_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.targets.hdr.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.ibl.sampler),
                },
            ],
        });
        self.tonemap_bind_group = tonemap;
        if self.present_pipeline.is_some() {
            let present = self.present_bind_group_for(&self.device);
            self.present_bind_group = Some(present);
        }
        if let (Some(surface), Some(cfg)) = (self.surface.as_ref(), self.surface_config.as_mut()) {
            cfg.width = width;
            cfg.height = height;
            surface.configure(&self.device, cfg);
        }
        Ok(())
    }

    /// Render one frame and present it to the window surface.
    pub fn present(
        &mut self,
        world: &World,
        assets: &AssetRegistry,
        camera: Entity,
    ) -> Result<(), RenderError> {
        let acquired = {
            let surface = self
                .surface
                .as_ref()
                .ok_or_else(|| RenderError::Unsupported("renderer has no surface".into()))?;
            surface.get_current_texture()
        };
        let (frame, suboptimal) = match acquired {
            wgpu::CurrentSurfaceTexture::Success(frame) => (frame, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => (frame, true),
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                // Skip this frame; try again next tick.
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.reconfigure_surface();
                match self
                    .surface
                    .as_ref()
                    .expect("surface present")
                    .get_current_texture()
                {
                    wgpu::CurrentSurfaceTexture::Success(frame)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => (frame, false),
                    other => {
                        return Err(RenderError::Surface(format!(
                            "surface reacquire failed: {other:?}"
                        )));
                    }
                }
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                return Err(RenderError::Surface("surface lost".into()));
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(RenderError::Surface("surface validation error".into()));
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.render_scene(world, assets, camera, Some(&view))?;
        self.queue.present(frame);
        if suboptimal {
            self.reconfigure_surface();
        }
        Ok(())
    }

    fn reconfigure_surface(&mut self) {
        if let (Some(surface), Some(cfg)) = (self.surface.as_ref(), self.surface_config.as_ref()) {
            surface.configure(&self.device, cfg);
        }
    }

    fn build_shadow(
        &self,
        lights: &[crate::lights::ExtractedLight],
        draw_list: &crate::draw_list::DrawList,
    ) -> (ShadowUniform, Mat4, bool) {
        let disabled = || {
            (
                ShadowUniform {
                    view_proj: math::to_cols_array(&math::identity()),
                    params: [0.0, 0.0, self.shadow.texel_size(), 0.0],
                    direction: [0.0, -1.0, 0.0, 0.0],
                },
                math::identity(),
                false,
            )
        };
        let Some(index) = primary_shadow_light(lights) else {
            return disabled();
        };
        let light = &lights[index];
        let (center, radius) = scene_bounds(draw_list);
        let matrix = match light.kind {
            LightKind::Directional { .. } => crate::lights::directional_shadow_matrix(
                light.direction,
                center,
                radius,
                0.1,
                radius * 4.0 + 10.0,
            ),
            LightKind::Spot {
                direction,
                outer_angle,
                range,
                ..
            } => crate::lights::spot_shadow_matrix(
                light.position,
                direction,
                outer_angle,
                0.1,
                range.max(1.0),
            ),
            LightKind::Point { .. } => return disabled(),
        };
        let uniform = ShadowUniform {
            view_proj: math::to_cols_array(&matrix),
            params: [0.0015, 0.006, self.shadow.texel_size(), 1.0],
            direction: [light.direction.x, light.direction.y, light.direction.z, 0.0],
        };
        (uniform, matrix, true)
    }

    fn prepare_draw(
        &mut self,
        item: &DrawItem,
        assets: &AssetRegistry,
        default_material: &Material,
        index: u32,
    ) -> Option<PreparedDraw> {
        let mesh = assets.meshes.get(item.mesh)?;
        let material = assets
            .materials
            .get(item.material)
            .unwrap_or(default_material);
        let uniform = object_uniform(item, material);
        self.objects.write(&self.queue, index, &uniform);

        let gpu_mesh = self.cache.mesh(&self.device, item.mesh.id(), mesh);
        let vertex_buffer = gpu_mesh.vertex_buffer.clone();
        let index_buffer = gpu_mesh.index_buffer.clone();
        let index_count = gpu_mesh.index_count;

        let material_id = if item.material.is_none() {
            AssetId::NONE
        } else {
            item.material.id()
        };
        let material_bind_group = self
            .cache
            .material(
                &self.device,
                &self.queue,
                material_id,
                material,
                assets,
                &self.layouts.material,
                &self.material_sampler,
            )
            .bind_group
            .clone();

        Some(PreparedDraw {
            vertex_buffer,
            index_buffer,
            index_count,
            offset: index * self.objects.stride(),
            material: material_bind_group,
        })
    }

    fn read_back(&self) -> Result<Frame, RenderError> {
        let slice = self.targets.readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let data = slice
            .get_mapped_range()
            .map_err(|e| RenderError::Readback(e.to_string()))?;

        let width = self.targets.width as usize;
        let height = self.targets.height as usize;
        let padded = self.targets.padded_bytes_per_row as usize;
        let row_bytes = width * 4;
        let mut rgba = Vec::with_capacity(row_bytes * height);
        for row in 0..height {
            let start = row * padded;
            rgba.extend_from_slice(&data[start..start + row_bytes]);
        }
        drop(data);
        self.targets.readback.unmap();

        Ok(Frame {
            width: self.targets.width,
            height: self.targets.height,
            rgba,
        })
    }
}

fn make_view_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("view.bind_group"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buffer.as_entire_binding(),
        }],
    })
}

fn object_uniform(item: &DrawItem, material: &Material) -> ObjectUniform {
    let model = math::to_cols_array(&item.world_matrix);
    let normal_matrix = math::inverse(&item.world_matrix)
        .map(|inv| math::to_cols_array(&math::transpose(&inv)))
        .unwrap_or_else(|| math::to_cols_array(&math::identity()));

    let has = |handle: ecs::assets::AssetHandle<ecs::assets::Texture>| {
        if handle.is_none() { 0.0 } else { 1.0 }
    };
    let cutoff = match material.alpha_mode {
        ecs::assets::AlphaMode::Mask => 0.5,
        _ => 0.0,
    };

    ObjectUniform {
        model,
        normal_matrix,
        base_color: material.base_color,
        params: [
            material.metallic,
            material.roughness,
            material.emissive_strength,
            cutoff,
        ],
        emissive: [
            material.emissive[0],
            material.emissive[1],
            material.emissive[2],
            0.0,
        ],
        flags: [
            has(material.base_color_texture),
            has(material.normal_texture),
            has(material.metallic_roughness_texture),
            has(material.emissive_texture),
        ],
        flags2: [
            has(material.occlusion_texture),
            if material.double_sided { 1.0 } else { 0.0 },
            0.0,
            0.0,
        ],
    }
}

/// The world-space bounds of everything in the draw list, plus a padding radius.
fn scene_bounds(draw_list: &crate::draw_list::DrawList) -> (math::Vec3, f32) {
    let mut bounds: Option<Aabb> = None;
    for item in draw_list.iter() {
        bounds = Some(match bounds {
            Some(b) => b.union(&item.world_bounds),
            None => item.world_bounds,
        });
    }
    match bounds {
        Some(b) if !b.is_empty() => {
            let size = b.size();
            let radius = (size.x.max(size.y).max(size.z) * 0.75).max(1.0);
            (b.center(), radius)
        }
        _ => (vec3(0.0, 0.0, 0.0), 10.0),
    }
}

/// Re-export of the camera view type for callers that want to inspect it.
pub type ExtractedCamera = CameraView;

/// The maximum number of lights the renderer uploads.
pub const MAX_RENDER_LIGHTS: usize = MAX_LIGHTS;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_reports_pixels() {
        let frame = Frame {
            width: 2,
            height: 1,
            rgba: vec![1, 2, 3, 4, 5, 6, 7, 8],
        };
        assert_eq!(frame.pixel(0, 0), Some([1, 2, 3, 4]));
        assert_eq!(frame.pixel(1, 0), Some([5, 6, 7, 8]));
        assert_eq!(frame.pixel(2, 0), None);
        assert_eq!(frame.to_rgba8().len(), 8);
    }

    #[test]
    fn config_defaults_are_sensible() {
        let config = RendererConfig::default();
        assert!(config.width > 0 && config.height > 0);
        assert!(config.hdr);
        assert_eq!(config.sample_count, 1);
    }

    #[test]
    fn scene_bounds_defaults_without_geometry() {
        let list = crate::draw_list::DrawList::default();
        let (center, radius) = scene_bounds(&list);
        assert!(radius > 0.0);
        assert!(math::vec3_approx_eq(center, vec3(0.0, 0.0, 0.0), 1e-6));
    }

    /// The GPU path is exercised only when an adapter exists; otherwise the
    /// renderer reports [`RenderError::NoAdapter`] and the test is a no-op.
    #[test]
    fn headless_renders_or_reports_no_adapter() {
        use ecs::assets::{Mesh, Vertex};
        use ecs::components::{Light, MeshRenderer};
        use math::vec3;

        let config = RendererConfig {
            width: 64,
            height: 64,
            ..Default::default()
        };
        let mut renderer = match Renderer::headless(config) {
            Ok(renderer) => renderer,
            Err(RenderError::NoAdapter) => {
                eprintln!("skipping GPU test: no adapter");
                return;
            }
            Err(other) => panic!("unexpected renderer error: {other}"),
        };

        // A cube-ish triangle with a directional light, a shadow caster and a
        // gizmo so every pass records real draw calls.
        let mut assets = AssetRegistry::with_defaults();
        let mesh = assets.meshes.insert(Mesh {
            name: "triangle".into(),
            vertices: vec![
                Vertex {
                    position: [0.0, 0.5, 0.0],
                    normal: [0.0, 0.0, 1.0],
                    uv: [0.5, 0.0],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                },
                Vertex {
                    position: [-0.5, -0.5, 0.0],
                    normal: [0.0, 0.0, 1.0],
                    uv: [0.0, 1.0],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                },
                Vertex {
                    position: [0.5, -0.5, 0.0],
                    normal: [0.0, 0.0, 1.0],
                    uv: [1.0, 1.0],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                },
            ],
            indices: vec![0, 1, 2],
            bounds: None,
        });
        let material = assets.materials.handle("$default").unwrap();

        let mut world = World::new();
        let camera = world.spawn_with(ecs::components::Camera::default());
        world.spawn_with(Light::directional(vec3(0.0, -1.0, -0.5)));
        let e = world.spawn_with(MeshRenderer {
            mesh,
            material,
            ..MeshRenderer::default()
        });
        world.insert(e, math::Transform::from_translation(vec3(0.0, 0.0, -3.0)));

        let gizmo = crate::gizmo::build_gizmo(crate::gizmo::GizmoMode::Translate, 1.0, None);
        renderer.set_gizmo(&gizmo, &math::identity());

        let frame = renderer
            .render_frame(&world, &assets, camera)
            .expect("render frame");
        assert_eq!(frame.width, 64);
        assert_eq!(frame.height, 64);
        assert_eq!(frame.rgba.len(), 64 * 64 * 4);
        assert!(renderer.cached_meshes() >= 1);

        renderer.clear_gizmo();
        renderer
            .render_frame(&world, &assets, camera)
            .expect("render frame without gizmo");
    }
}
