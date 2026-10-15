//! Pipeline layouts, render pipelines, and the persistent per-frame bind groups.
//!
//! Nine pipelines cover the frame: a depth-only pipeline (shared by the shadow
//! pass and the camera depth prepass), SSAO, opaque and transparent PBR, gizmo
//! lines and triangles, two world-space text pipelines (depth-tested and
//! depth-ignoring), and the tonemap resolve. Bind groups are split into group 0
//! (frame: camera, lights, IBL, shadow, SSAO), group 1 (per-object, dynamic
//! offset) and group 2 (per-material textures).

use crate::gpu::ibl_gpu::IblGpu;
use crate::gpu::resources::{GpuVertex, SkinVertex};
use crate::gpu::targets::align_up;
use crate::gpu::uniforms::{FrameUniform, ObjectUniform, ReflectionUniform, ShadowUniform};
use crate::lights::LightsUniform;
use crate::shader::ShadeForm;
use bytemuck::Pod;
use ecs::assets::Shader;
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
    /// Group 1 of the text pass: the font atlas texture and its sampler.
    pub text: wgpu::BindGroupLayout,
    /// Group 0 of the SSAO pass.
    pub ssao: wgpu::BindGroupLayout,
    /// Group 0 of the tonemap pass.
    pub tonemap: wgpu::BindGroupLayout,
    /// Group 0 of the present (surface blit) pass.
    pub present: wgpu::BindGroupLayout,
    /// The joint matrices of a skinned draw (group 3 of the PBR pass, group 2 of
    /// the depth pass), bound with a dynamic offset.
    pub skin: wgpu::BindGroupLayout,
    /// The shader seam's texture array and its shared sampler (group 2).
    pub shader_textures: wgpu::BindGroupLayout,
    /// The shader seam's user uniform block (group 3).
    pub shader_user: wgpu::BindGroupLayout,
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
            // The planar reflection (`.notes/reflections.md`, tier 2). It lives in
            // group 0 because that group is already "the frame's shared resources"
            // and is per-pass, which is exactly the reflection's granularity — and
            // because a WebGPU device is only guaranteed four bind groups.
            texture(10),
            sampler(11, false),
            uniform(12, wgpu::ShaderStages::FRAGMENT),
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

    let text = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("text.atlas_layout"),
        entries: &[texture(0), sampler(1, false)],
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

    let skin = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("skin.layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: true,
                min_binding_size: None,
            },
            count: None,
        }],
    });

    // The shader seam (`.notes/shaders.md`). Group 2 is the material's texture
    // array plus one shared sampler; group 3 is the user uniform block. The
    // capacities are the `ecs` constants, so nothing can drift.
    let shader_textures = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("shader.textures_layout"),
        entries: &{
            let mut entries: Vec<wgpu::BindGroupLayoutEntry> = (0..ecs::assets::TEXTURE_SLOTS
                as u32)
                .map(texture)
                .collect();
            entries.push(sampler(ecs::assets::TEXTURE_SLOTS as u32, false));
            entries
        },
    });
    let shader_user = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("shader.user_layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new((ecs::assets::USER_SLOTS * 16) as u64),
            },
            count: None,
        }],
    });

    BindGroupLayouts {
        frame,
        object,
        material,
        view,
        text,
        ssao,
        tonemap,
        present,
        skin,
        shader_textures,
        shader_user,
    }
}

/// The render pipelines.
#[derive(Debug)]
pub struct Pipelines {
    /// Depth-only (shadow map and camera depth prepass).
    pub depth: wgpu::RenderPipeline,
    /// Depth-only for a skinned mesh.
    pub depth_skinned: wgpu::RenderPipeline,
    /// Ambient occlusion resolve.
    pub ssao: wgpu::RenderPipeline,
    /// Opaque PBR.
    pub pbr_opaque: wgpu::RenderPipeline,
    /// Alpha-blended PBR.
    pub pbr_transparent: wgpu::RenderPipeline,
    /// Opaque PBR for a skinned mesh.
    pub pbr_opaque_skinned: wgpu::RenderPipeline,
    /// Alpha-blended PBR for a skinned mesh.
    pub pbr_transparent_skinned: wgpu::RenderPipeline,
    /// Gizmo line list.
    pub gizmo_line: wgpu::RenderPipeline,
    /// Gizmo triangle list.
    pub gizmo_triangle: wgpu::RenderPipeline,
    /// World-space text glyph quads, depth-tested against the scene
    /// ([`ecs::components::Text::depth_test`]).
    pub text: wgpu::RenderPipeline,
    /// World-space text glyph quads that ignore depth, drawn over the scene
    /// ([`ecs::components::Text::depth_test`] `== false`).
    pub text_no_depth: wgpu::RenderPipeline,
    /// Billboard sprite quads, depth-tested, indexed by
    /// [`BillboardBlend`](ecs::components::BillboardBlend) (opaque, alpha,
    /// additive). `.notes/billboards.md`.
    pub billboard: [wgpu::RenderPipeline; 3],
    /// The same, ignoring depth (`Billboard::depth_test == false`).
    pub billboard_no_depth: [wgpu::RenderPipeline; 3],
    /// HDR -> LDR tonemap.
    pub tonemap: wgpu::RenderPipeline,
}

impl Pipelines {
    /// The billboard pipeline for a blend mode and depth-test setting.
    pub fn billboard(
        &self,
        blend: ecs::components::BillboardBlend,
        depth_test: bool,
    ) -> &wgpu::RenderPipeline {
        let set = if depth_test {
            &self.billboard
        } else {
            &self.billboard_no_depth
        };
        &set[blend as usize]
    }
}

/// Build the render pipeline for a custom [`Shader`] in the seam's fixed layout
/// (`.notes/shaders.md`). `form` picks the vertex stage — the body's `vs_main`,
/// or the prelude's `vs_fullscreen` for the screen form — and `blend` /
/// `depth_write` / `sample_count` mirror the PBR pass. A screen pipeline writes no
/// depth, because it runs before the scene draws.
pub fn create_shader_pipeline(
    device: &wgpu::Device,
    layouts: &BindGroupLayouts,
    shader: &Shader,
    form: ShadeForm,
    blend: Option<wgpu::BlendState>,
    depth_write: bool,
    sample_count: u32,
) -> wgpu::RenderPipeline {
    let source = crate::shader::compose(shader, form);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("shader.seam.module"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let layout = pipeline_layout(
        device,
        "shader.seam.layout",
        &[
            &layouts.frame,
            &layouts.object,
            &layouts.shader_textures,
            &layouts.shader_user,
        ],
    );

    let mesh_buffers = [Some(GpuVertex::layout())];
    let screen_buffers: [Option<wgpu::VertexBufferLayout>; 0] = [];
    let (vs_entry, buffers, depth_stencil): (
        &str,
        &[Option<wgpu::VertexBufferLayout>],
        Option<wgpu::DepthStencilState>,
    ) = match form {
        ShadeForm::Mesh => (
            "vs_main",
            &mesh_buffers,
            Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(depth_write),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
        ),
        ShadeForm::Screen => (
            "vs_fullscreen",
            &screen_buffers,
            // A depth state is required to share the scene's depth-attached pass;
            // `Always` with no write means the sky never rejects and never occludes.
            Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
        ),
    };

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("shader.seam.pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some(vs_entry),
            compilation_options: Default::default(),
            buffers,
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil,
        multisample: wgpu::MultisampleState {
            count: sample_count.max(1),
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some(shader.fs_entry.as_str()),
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

/// Create all render pipelines for a frame rendered at `sample_count` samples
/// per pixel (`1` = no MSAA).
///
/// The depth-only, SSAO and tonemap pipelines stay single-sampled whatever the
/// setting: they render into single-sampled targets (the shadow map, the SSAO
/// term, the LDR image). The pipelines that draw into the multisampled HDR target
/// — opaque, transparent, gizmo and both text pipelines — declare the same
/// sample count as that target, or the device rejects the pipeline/pass pairing.
pub fn create_pipelines(
    device: &wgpu::Device,
    layouts: &BindGroupLayouts,
    sample_count: u32,
) -> Pipelines {
    let samples = sample_count.max(1);
    let multisample = wgpu::MultisampleState {
        count: samples,
        ..Default::default()
    };
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

    let depth_skinned = {
        let layout = pipeline_layout(
            device,
            "depth.skinned.layout",
            &[&layouts.view, &layouts.object, &layouts.skin],
        );
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("depth.skinned.pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &depth_shader,
                entry_point: Some("vs_skinned"),
                compilation_options: Default::default(),
                buffers: &[Some(GpuVertex::layout()), Some(SkinVertex::layout())],
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

    let make_pbr =
        |label: &str, blend: Option<wgpu::BlendState>, depth_write: bool, skinned: bool| {
            let one = [Some(GpuVertex::layout())];
            let two = [Some(GpuVertex::layout()), Some(SkinVertex::layout())];
            let buffers: &[Option<wgpu::VertexBufferLayout>] = if skinned { &two } else { &one };
            let layout = if skinned {
                pipeline_layout(
                    device,
                    &format!("{label}.layout"),
                    &[
                        &layouts.frame,
                        &layouts.object,
                        &layouts.material,
                        &layouts.skin,
                    ],
                )
            } else {
                pipeline_layout(
                    device,
                    &format!("{label}.layout"),
                    &[&layouts.frame, &layouts.object, &layouts.material],
                )
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &pbr_shader,
                    entry_point: Some(if skinned { "vs_skinned" } else { "vs_main" }),
                    compilation_options: Default::default(),
                    buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    // Without MSAA the prepass already wrote the depth this pass
                    // shares. With MSAA the forward passes render into their own
                    // multisampled depth, which the *opaque* pass has to populate
                    // (the transparent pass must not, or it would occlude itself).
                    depth_write_enabled: Some(depth_write),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample,
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
    let pbr_opaque = make_pbr("pbr.opaque", None, samples > 1, false);
    let pbr_transparent = make_pbr(
        "pbr.transparent",
        Some(wgpu::BlendState::ALPHA_BLENDING),
        false,
        false,
    );
    let pbr_opaque_skinned = make_pbr("pbr.opaque.skinned", None, samples > 1, true);
    let pbr_transparent_skinned = make_pbr(
        "pbr.transparent.skinned",
        Some(wgpu::BlendState::ALPHA_BLENDING),
        false,
        true,
    );

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
            multisample,
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

    // Text is built twice: a label behind geometry is hidden (`depth_test`), or
    // always drawn on top of it. The two pipelines differ only in the depth
    // comparison, so a device does not pay for a second shader.
    let text_shader = device.create_shader_module(wgpu::include_wgsl!("../shaders/text.wgsl"));
    let make_text = |label: &str, depth_compare: wgpu::CompareFunction| {
        let layout = pipeline_layout(
            device,
            &format!("{label}.layout"),
            &[&layouts.view, &layouts.text],
        );
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &text_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(crate::text::TextVertex::layout())],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            // Tested against the scene but never writing depth, so a translucent
            // glyph does not occlude the next one. Only the comparison differs.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(depth_compare),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample,
            fragment: Some(wgpu::FragmentState {
                module: &text_shader,
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
    let text = make_text("text", wgpu::CompareFunction::LessEqual);
    let text_no_depth = make_text("text.no_depth", wgpu::CompareFunction::Always);

    // Billboards (`.notes/billboards.md`): the same shape as text — a world-space
    // textured quad — but sampling the sprite's own texture with a tint, in three
    // blend modes, each with a depth-tested and a depth-ignoring variant. The
    // fragment premultiplies, so alpha uses the premultiplied blend and additive
    // adds nothing where the sprite is clear.
    let billboard_shader =
        device.create_shader_module(wgpu::include_wgsl!("../shaders/billboard.wgsl"));
    let additive = wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    };
    let make_billboard = |label: &str,
                          blend: Option<wgpu::BlendState>,
                          depth_write: bool,
                          depth_compare: wgpu::CompareFunction| {
        let layout = pipeline_layout(
            device,
            &format!("{label}.layout"),
            &[&layouts.view, &layouts.text],
        );
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &billboard_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(crate::billboard::BillboardVertex::layout())],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(depth_write),
                depth_compare: Some(depth_compare),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample,
            fragment: Some(wgpu::FragmentState {
                module: &billboard_shader,
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
    let billboard = [
        make_billboard(
            "billboard.opaque",
            None,
            true,
            wgpu::CompareFunction::LessEqual,
        ),
        make_billboard(
            "billboard.alpha",
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            false,
            wgpu::CompareFunction::LessEqual,
        ),
        make_billboard(
            "billboard.additive",
            Some(additive),
            false,
            wgpu::CompareFunction::LessEqual,
        ),
    ];
    let billboard_no_depth = [
        make_billboard(
            "billboard.opaque.no_depth",
            None,
            false,
            wgpu::CompareFunction::Always,
        ),
        make_billboard(
            "billboard.alpha.no_depth",
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            false,
            wgpu::CompareFunction::Always,
        ),
        make_billboard(
            "billboard.additive.no_depth",
            Some(additive),
            false,
            wgpu::CompareFunction::Always,
        ),
    ];

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
        depth_skinned,
        ssao,
        pbr_opaque,
        pbr_transparent,
        pbr_opaque_skinned,
        pbr_transparent_skinned,
        gizmo_line,
        gizmo_triangle,
        text,
        text_no_depth,
        billboard,
        billboard_no_depth,
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

/// Choose a surface format.
///
/// A gamma-space (non-sRGB) 8-bit format is preferred: the tonemap blit and
/// egui both write sRGB-encoded values, so no hardware transfer function is
/// wanted on the surface. Falls back to an sRGB 8-bit format, then any non-sRGB
/// format, then the first advertised format.
pub fn pick_surface_format(formats: &[wgpu::TextureFormat]) -> Option<wgpu::TextureFormat> {
    use wgpu::TextureFormat as F;
    let is_8bit = |f: F| {
        matches!(
            f,
            F::Bgra8Unorm | F::Rgba8Unorm | F::Bgra8UnormSrgb | F::Rgba8UnormSrgb
        )
    };
    let find = |pred: &dyn Fn(F) -> bool| formats.iter().copied().find(|f| pred(*f));
    find(&|f| f == F::Bgra8Unorm)
        .or_else(|| find(&|f| f == F::Rgba8Unorm))
        .or_else(|| find(&|f| is_8bit(f) && !f.is_srgb()))
        .or_else(|| find(&|f| is_8bit(f) && f.is_srgb()))
        .or_else(|| find(&|f| !f.is_srgb()))
        .or_else(|| formats.first().copied())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::TextureFormat;

    #[test]
    fn prefers_gamma_space_8bit_formats() {
        assert_eq!(
            pick_surface_format(&[TextureFormat::Rgba8UnormSrgb, TextureFormat::Bgra8Unorm]),
            Some(TextureFormat::Bgra8Unorm)
        );
        assert_eq!(
            pick_surface_format(&[TextureFormat::Bgra8UnormSrgb, TextureFormat::Rgba8Unorm]),
            Some(TextureFormat::Rgba8Unorm)
        );
    }

    #[test]
    fn falls_back_through_srgb_then_first() {
        // Only an sRGB format is available.
        assert_eq!(
            pick_surface_format(&[TextureFormat::Bgra8UnormSrgb]),
            Some(TextureFormat::Bgra8UnormSrgb)
        );
        // No 8-bit format: the first non-sRGB format wins.
        assert_eq!(
            pick_surface_format(&[TextureFormat::Rgba16Float, TextureFormat::Rgba32Float]),
            Some(TextureFormat::Rgba16Float)
        );
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
    /// The planar reflection's parameters (`.notes/reflections.md`).
    pub reflection_buffer: wgpu::Buffer,
    /// The assembled bind group.
    pub bind_group: wgpu::BindGroup,
}

impl FrameBindings {
    /// Create the buffers and bind group for the given IBL, shadow and SSAO views,
    /// plus the planar reflection's texture and sampler.
    ///
    /// The reflection pass builds its *own* group 0 (its camera is the mirrored
    /// one), pointing the reflection and SSAO bindings at a 1x1 white texture: a
    /// pass must never sample the texture it is rendering into, and a reflection
    /// wants no part of the frame's ambient occlusion.
    pub fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        ibl: &IblGpu,
        shadow_view: &wgpu::TextureView,
        ssao_view: &wgpu::TextureView,
        reflection_view: &wgpu::TextureView,
        reflection_sampler: &wgpu::Sampler,
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
        let reflection_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame.reflection"),
            size: std::mem::size_of::<ReflectionUniform>() as u64,
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
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::TextureView(reflection_view),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: wgpu::BindingResource::Sampler(reflection_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 12,
                    resource: reflection_buffer.as_entire_binding(),
                },
            ],
        });

        FrameBindings {
            camera_buffer,
            lights_buffer,
            shadow_buffer,
            reflection_buffer,
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

    /// Upload the planar reflection's parameters.
    pub fn write_reflection(&self, queue: &wgpu::Queue, reflection: &ReflectionUniform) {
        queue.write_buffer(&self.reflection_buffer, 0, as_bytes(reflection));
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

/// The largest joint count one skinned draw may have (one slot of matrices).
pub const MAX_SKIN_MATRICES: u32 = 128;

/// A growable storage buffer of joint matrices, one slot per skinned draw, bound
/// with a dynamic offset.
///
/// Each skinned mesh uploads its joints' skin matrices into one slot and binds the
/// buffer at that slot's offset, so a herd of goats shares one buffer.
#[derive(Debug)]
pub struct SkinBindings {
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    capacity: u32,
    stride: u32,
    layout: wgpu::BindGroupLayout,
}

impl SkinBindings {
    /// Create with room for `capacity` skinned draws.
    pub fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        capacity: u32,
    ) -> SkinBindings {
        let alignment = device.limits().min_storage_buffer_offset_alignment.max(1);
        let stride = align_up(MAX_SKIN_MATRICES * 64, alignment);
        let capacity = capacity.max(1);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("skin.buffer"),
            size: u64::from(stride) * u64::from(capacity),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group =
            make_skin_bind_group(device, layout, &buffer, u64::from(MAX_SKIN_MATRICES) * 64);
        SkinBindings {
            buffer,
            bind_group,
            capacity,
            stride,
            layout: layout.clone(),
        }
    }

    /// Ensure there is room for at least `needed` skinned draws.
    pub fn ensure(&mut self, device: &wgpu::Device, needed: u32) {
        if needed <= self.capacity {
            return;
        }
        let new_capacity = needed.next_power_of_two().max(self.capacity * 2);
        self.buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("skin.buffer"),
            size: u64::from(self.stride) * u64::from(new_capacity),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.bind_group = make_skin_bind_group(
            device,
            &self.layout,
            &self.buffer,
            u64::from(MAX_SKIN_MATRICES) * 64,
        );
        self.capacity = new_capacity;
    }

    /// The dynamic offset (in bytes) of slot `index`.
    pub fn offset(&self, index: u32) -> u32 {
        index * self.stride
    }

    /// Upload the column-major joint matrices of slot `index`.
    pub fn write(&self, queue: &wgpu::Queue, index: u32, matrices: &[f32]) {
        queue.write_buffer(
            &self.buffer,
            u64::from(index * self.stride),
            bytemuck::cast_slice(matrices),
        );
    }

    /// The bind group to bind with a dynamic offset.
    pub fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }
}

fn make_skin_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
    slot_size: u64,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("skin.bind_group"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            // Bind one slot, so the dynamic offset can select a slot; a
            // whole-buffer binding (`size: None`) could only be offset by zero.
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset: 0,
                size: NonZeroU64::new(slot_size),
            }),
        }],
    })
}
