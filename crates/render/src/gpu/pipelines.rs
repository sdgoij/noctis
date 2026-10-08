//! Pipeline layouts, render pipelines, and the persistent per-frame bind groups.
//!
//! Eight pipelines cover the frame: a depth-only pipeline (shared by the shadow
//! pass and the camera depth prepass), SSAO, opaque and transparent PBR, gizmo
//! lines and triangles, and the tonemap resolve. Bind groups are split into
//! group 0 (frame: camera, lights, IBL, shadow, SSAO), group 1 (per-object,
//! dynamic offset) and group 2 (per-material textures).

use crate::gpu::ibl_gpu::IblGpu;
use crate::gpu::resources::GpuVertex;
use crate::gpu::targets::align_up;
use crate::gpu::uniforms::{FrameUniform, ObjectUniform, ShadowUniform};
use crate::lights::LightsUniform;
use bytemuck::Pod;
use std::num::NonZeroU64;

/// The bind group layouts shared by the pipelines.
#[derive(Debug)]
pub struct BindGroupLayouts {
    /// Group 0 of the PBR pass.
    pub frame: wgpu::BindGroupLayout,
    /// Group 1 of the PBR and depth passes (per-object, dynamic offset).
    pub object: wgpu::BindGroupLayout,
    /// Group 2 of the PBR pass (per-material textures).
    pub material: wgpu::BindGroupLayout,
    /// A single-matrix layout used by the depth and gizmo passes.
    pub view: wgpu::BindGroupLayout,
    /// Group 0 of the SSAO pass.
    pub ssao: wgpu::BindGroupLayout,
    /// Group 0 of the tonemap pass.
    pub tonemap: wgpu::BindGroupLayout,
    /// Group 0 of the present (surface blit) pass.
    pub present: wgpu::BindGroupLayout,
}

fn buffer_entry(
    ty: wgpu::BufferBindingType,
    visibility: wgpu::ShaderStages,
    size: Option<u64>,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: size.and_then(NonZeroU64::new),
        },
        count: None,
    }
}

/// Create all bind group layouts.
pub fn create_layouts(device: &wgpu::Device) -> BindGroupLayouts {
    let uniform = |binding: u32, visibility: wgpu::ShaderStages| wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let texture = |binding: u32| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let texture_cube = |binding: u32| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::Cube,
            multisampled: false,
        },
        count: None,
    };
    let sampler = |binding: u32, comparison: bool| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(if comparison {
            wgpu::SamplerBindingType::Comparison
        } else {
            wgpu::SamplerBindingType::Filtering
        }),
        count: None,
    };
    let depth_texture = |binding: u32| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Depth,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };

    let frame = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("pbr.frame_layout"),
        entries: &[
            uniform(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
            uniform(1, wgpu::ShaderStages::FRAGMENT),
            sampler(2, false),
            texture_cube(3),
            texture_cube(4),
            texture(5),
            sampler(6, true),
            depth_texture(7),
            uniform(8, wgpu::ShaderStages::FRAGMENT),
            texture(9),
        ],
    });

    let object = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("object.layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: NonZeroU64::new(std::mem::size_of::<ObjectUniform>() as u64),
            },
            count: None,
        }],
    });

    let material = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("material.layout"),
        entries: &[
            texture(0),
            texture(1),
            texture(2),
            texture(3),
            texture(4),
            sampler(5, false),
        ],
    });

    let view = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("view.layout"),
        entries: &[buffer_entry(
            wgpu::BufferBindingType::Uniform,
            wgpu::ShaderStages::VERTEX,
            None,
        )],
    });

    let ssao = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ssao.layout"),
        entries: &[uniform(0, wgpu::ShaderStages::FRAGMENT), depth_texture(1)],
    });

    let tonemap = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("tonemap.layout"),
        entries: &[
            uniform(0, wgpu::ShaderStages::FRAGMENT),
            texture(1),
            sampler(2, false),
        ],
    });

    let present = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("present.layout"),
        entries: &[texture(0), sampler(1, false)],
    });

    BindGroupLayouts {
        frame,
        object,
        material,
        view,
        ssao,
        tonemap,
        present,
    }
}

/// The render pipelines.
#[derive(Debug)]
pub struct Pipelines {
    /// Depth-only (shadow map and camera depth prepass).
    pub depth: wgpu::RenderPipeline,
    /// Ambient occlusion resolve.
    pub ssao: wgpu::RenderPipeline,
    /// Opaque PBR.
    pub pbr_opaque: wgpu::RenderPipeline,
    /// Alpha-blended PBR.
    pub pbr_transparent: wgpu::RenderPipeline,
    /// Gizmo line list.
    pub gizmo_line: wgpu::RenderPipeline,
    /// Gizmo triangle list.
    pub gizmo_triangle: wgpu::RenderPipeline,
    /// HDR -> LDR tonemap.
    pub tonemap: wgpu::RenderPipeline,
}

fn pipeline_layout(
    device: &wgpu::Device,
    label: &str,
    layouts: &[&wgpu::BindGroupLayout],
) -> wgpu::PipelineLayout {
    let entries: Vec<Option<&wgpu::BindGroupLayout>> = layouts.iter().map(|l| Some(*l)).collect();
    device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &entries,
        immediate_size: 0,
    })
}

/// Create all render pipelines.
pub fn create_pipelines(device: &wgpu::Device, layouts: &BindGroupLayouts) -> Pipelines {
    let depth_shader =
        device.create_shader_module(wgpu::include_wgsl!("../shaders/depth_only.wgsl"));
    let ssao_shader = device.create_shader_module(wgpu::include_wgsl!("../shaders/ssao.wgsl"));
    let pbr_shader = device.create_shader_module(wgpu::include_wgsl!("../shaders/pbr.wgsl"));
    let gizmo_shader = device.create_shader_module(wgpu::include_wgsl!("../shaders/gizmo.wgsl"));
    let tonemap_shader =
        device.create_shader_module(wgpu::include_wgsl!("../shaders/tonemap.wgsl"));

    let depth = {
        let layout = pipeline_layout(device, "depth.layout", &[&layouts.view, &layouts.object]);
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("depth.pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &depth_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(GpuVertex::layout())],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: None,
            multiview_mask: None,
            cache: None,
        })
    };

    let ssao = {
        let layout = pipeline_layout(device, "ssao.layout", &[&layouts.ssao]);
        let fragment = wgpu::FragmentState {
            module: &ssao_shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::R8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        };
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ssao.pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &ssao_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(fragment),
            multiview_mask: None,
            cache: None,
        })
    };

    let make_pbr = |label: &str, blend: Option<wgpu::BlendState>| {
        let layout = pipeline_layout(
            device,
            &format!("{label}.layout"),
            &[&layouts.frame, &layouts.object, &layouts.material],
        );
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &pbr_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(GpuVertex::layout())],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                // Depth was already written by the prepass.
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &pbr_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba16Float,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    };
    let pbr_opaque = make_pbr("pbr.opaque", None);
    let pbr_transparent = make_pbr("pbr.transparent", Some(wgpu::BlendState::ALPHA_BLENDING));

    let gizmo = |label: &str, topology: wgpu::PrimitiveTopology| {
        let layout = pipeline_layout(device, &format!("{label}.layout"), &[&layouts.view]);
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &gizmo_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<crate::gizmo::GizmoVertex>()
                        as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 12,
                            shader_location: 1,
                        },
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &gizmo_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba16Float,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    };
    let gizmo_line = gizmo("gizmo.line", wgpu::PrimitiveTopology::LineList);
    let gizmo_triangle = gizmo("gizmo.triangle", wgpu::PrimitiveTopology::TriangleList);

    let tonemap = {
        let layout = pipeline_layout(device, "tonemap.layout", &[&layouts.tonemap]);
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("tonemap.pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &tonemap_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &tonemap_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    };

    Pipelines {
        depth,
        ssao,
        pbr_opaque,
        pbr_transparent,
        gizmo_line,
        gizmo_triangle,
        tonemap,
    }
}

/// Create the present (surface blit) pipeline for a specific surface `format`.
///
/// `fragment_entry` is `"fs_passthrough"` for a non-sRGB surface format, or
/// `"fs_srgb_decode"` for an sRGB one (see `shaders/present.wgsl`).
pub fn create_present_pipeline(
    device: &wgpu::Device,
    layouts: &BindGroupLayouts,
    format: wgpu::TextureFormat,
    fragment_entry: &str,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::include_wgsl!("../shaders/present.wgsl"));
    let layout = pipeline_layout(device, "present.pipeline_layout", &[&layouts.present]);
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("present.pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some(fragment_entry),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// Choose a surface format: prefer an 8-bit sRGB format (the present shader
/// decodes for those), otherwise the first advertised format.
pub fn pick_surface_format(formats: &[wgpu::TextureFormat]) -> Option<wgpu::TextureFormat> {
    let eight_bit_srgb = formats.iter().copied().find(|f| {
        f.is_srgb()
            && matches!(
                f,
                wgpu::TextureFormat::Bgra8UnormSrgb | wgpu::TextureFormat::Rgba8UnormSrgb
            )
    });
    eight_bit_srgb
        .or_else(|| formats.iter().copied().find(|f| f.is_srgb()))
        .or_else(|| formats.first().copied())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::TextureFormat;

    #[test]
    fn prefers_eight_bit_srgb_formats() {
        let formats = [TextureFormat::Bgra8Unorm, TextureFormat::Rgba8UnormSrgb];
        assert_eq!(
            pick_surface_format(&formats),
            Some(TextureFormat::Rgba8UnormSrgb)
        );
        let formats = [TextureFormat::Rgba16Float, TextureFormat::Bgra8UnormSrgb];
        assert_eq!(
            pick_surface_format(&formats),
            Some(TextureFormat::Bgra8UnormSrgb)
        );
    }

    #[test]
    fn falls_back_to_any_srgb_then_first_format() {
        // No sRGB format at all: the first advertised format is used.
        let formats = [TextureFormat::Rgba8Unorm, TextureFormat::Rgba16Float];
        assert_eq!(
            pick_surface_format(&formats),
            Some(TextureFormat::Rgba8Unorm)
        );
        // No sRGB format at all: the first advertised format is used.
        let formats = [TextureFormat::Bgra8Unorm, TextureFormat::Rgba8Unorm];
        assert_eq!(
            pick_surface_format(&formats),
            Some(TextureFormat::Bgra8Unorm)
        );
        // Empty list has no valid choice.
        assert_eq!(pick_surface_format(&[]), None);
    }
}

/// The uniform buffers and bind group of group 0 of the PBR pass.
#[derive(Debug)]
pub struct FrameBindings {
    /// Camera uniform buffer.
    pub camera_buffer: wgpu::Buffer,
    /// Lights uniform buffer.
    pub lights_buffer: wgpu::Buffer,
    /// Shadow uniform buffer.
    pub shadow_buffer: wgpu::Buffer,
    /// The assembled bind group.
    pub bind_group: wgpu::BindGroup,
}

impl FrameBindings {
    /// Create the buffers and bind group for the given IBL, shadow and SSAO views.
    pub fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        ibl: &IblGpu,
        shadow_view: &wgpu::TextureView,
        ssao_view: &wgpu::TextureView,
    ) -> FrameBindings {
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame.camera"),
            size: std::mem::size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let lights_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame.lights"),
            size: std::mem::size_of::<LightsUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame.shadow"),
            size: std::mem::size_of::<ShadowUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame.bind_group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: lights_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&ibl.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&ibl.irradiance_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&ibl.prefiltered_view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&ibl.brdf_view),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::Sampler(&ibl.comparison_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: shadow_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::TextureView(ssao_view),
                },
            ],
        });

        FrameBindings {
            camera_buffer,
            lights_buffer,
            shadow_buffer,
            bind_group,
        }
    }

    /// Upload the frame, light, and shadow uniforms.
    pub fn write(
        &self,
        queue: &wgpu::Queue,
        frame: &FrameUniform,
        lights: &LightsUniform,
        shadow: &ShadowUniform,
    ) {
        queue.write_buffer(&self.camera_buffer, 0, as_bytes(frame));
        queue.write_buffer(&self.lights_buffer, 0, as_bytes(lights));
        queue.write_buffer(&self.shadow_buffer, 0, as_bytes(shadow));
    }
}

fn as_bytes<T: Pod>(value: &T) -> &[u8] {
    bytemuck::bytes_of(value)
}

/// A growable uniform buffer holding one [`ObjectUniform`] per draw, bound with
/// a dynamic offset.
#[derive(Debug)]
pub struct ObjectBindings {
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    capacity: u32,
    stride: u32,
    layout: wgpu::BindGroupLayout,
}

impl ObjectBindings {
    /// Create with room for `capacity` objects.
    pub fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        capacity: u32,
    ) -> ObjectBindings {
        let alignment = device.limits().min_uniform_buffer_offset_alignment.max(1);
        let stride = align_up(std::mem::size_of::<ObjectUniform>() as u32, alignment);
        let capacity = capacity.max(1);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("object.buffer"),
            size: (stride as u64) * (capacity as u64),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = make_object_bind_group(device, layout, &buffer);
        ObjectBindings {
            buffer,
            bind_group,
            capacity,
            stride,
            layout: layout.clone(),
        }
    }

    /// The buffer stride between objects (the dynamic-offset alignment).
    pub fn stride(&self) -> u32 {
        self.stride
    }

    /// The current capacity in objects.
    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// Ensure there is room for at least `needed` objects, growing if required.
    pub fn ensure(&mut self, device: &wgpu::Device, needed: u32) {
        if needed <= self.capacity {
            return;
        }
        let new_capacity = needed.next_power_of_two().max(self.capacity * 2);
        self.buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("object.buffer"),
            size: (self.stride as u64) * (new_capacity as u64),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.bind_group = make_object_bind_group(device, &self.layout, &self.buffer);
        self.capacity = new_capacity;
    }

    /// Upload the object uniform at `index`.
    pub fn write(&self, queue: &wgpu::Queue, index: u32, data: &ObjectUniform) {
        queue.write_buffer(&self.buffer, (index * self.stride) as u64, as_bytes(data));
    }

    /// The bind group to bind with a dynamic offset.
    pub fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }
}

fn make_object_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("object.bind_group"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset: 0,
                size: NonZeroU64::new(std::mem::size_of::<ObjectUniform>() as u64),
            }),
        }],
    })
}
