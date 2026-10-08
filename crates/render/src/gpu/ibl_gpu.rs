//! Upload of CPU-precomputed IBL textures.
//!
//! [`crate::ibl::IblData`] is produced on the CPU (see [`crate::ibl`]); this
//! module converts it to `rgba16float` textures, uploads the cube faces and the
//! BRDF LUT, and builds the shared samplers used by the PBR pass.

use crate::ibl::IblData;
use half::f16;

/// The GPU IBL textures and samplers.
#[derive(Debug)]
pub struct IblGpu {
    /// Diffuse irradiance cubemap.
    pub irradiance_view: wgpu::TextureView,
    /// Prefiltered specular cubemap (mip chain).
    pub prefiltered_view: wgpu::TextureView,
    /// The number of mip levels in the prefiltered cubemap.
    pub prefiltered_mips: u32,
    /// The split-sum BRDF LUT.
    pub brdf_view: wgpu::TextureView,
    /// Trilinear, clamping sampler for environment lookups.
    pub sampler: wgpu::Sampler,
    /// Comparison sampler for the shadow map.
    pub comparison_sampler: wgpu::Sampler,
}

/// Convert a slice of RGBA `f32` texels to tightly packed `f16` bytes.
fn f16_bytes(texels: &[[f32; 4]]) -> Vec<u8> {
    let converted: Vec<[f16; 4]> = texels
        .iter()
        .map(|t| {
            [
                f16::from_f32(t[0]),
                f16::from_f32(t[1]),
                f16::from_f32(t[2]),
                f16::from_f32(t[3]),
            ]
        })
        .collect();
    bytemuck::cast_slice(&converted).to_vec()
}

fn create_cube(device: &wgpu::Device, label: &str, size: u32, mips: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 6,
        },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn upload_face(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    mip: u32,
    face: u32,
    size: u32,
    texels: &[[f32; 4]],
) {
    let size3 = wgpu::Extent3d {
        width: size,
        height: size,
        depth_or_array_layers: 1,
    };
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: mip,
            origin: wgpu::Origin3d {
                x: 0,
                y: 0,
                z: face,
            },
            aspect: wgpu::TextureAspect::All,
        },
        &f16_bytes(texels),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size * 8),
            rows_per_image: Some(size),
        },
        size3,
    );
}

impl IblGpu {
    /// Upload `data` and build the samplers.
    pub fn create(device: &wgpu::Device, queue: &wgpu::Queue, data: &IblData) -> IblGpu {
        // Diffuse irradiance.
        let irradiance = create_cube(device, "ibl.irradiance", data.irradiance.size, 1);
        for face in 0..6u32 {
            let texels: Vec<[f32; 4]> = (0..data.irradiance.size * data.irradiance.size)
                .map(|i| {
                    let x = i % data.irradiance.size;
                    let y = i / data.irradiance.size;
                    data.irradiance.get(face, x, y)
                })
                .collect();
            upload_face(queue, &irradiance, 0, face, data.irradiance.size, &texels);
        }

        // Prefiltered specular mip chain.
        let base_size = data.prefiltered.first().map(|c| c.size).unwrap_or(1);
        let mips = data.prefiltered.len().max(1) as u32;
        let prefiltered = create_cube(device, "ibl.prefiltered", base_size, mips);
        for (mip, cube) in data.prefiltered.iter().enumerate() {
            for face in 0..6u32 {
                let texels: Vec<[f32; 4]> = (0..cube.size * cube.size)
                    .map(|i| {
                        let x = i % cube.size;
                        let y = i / cube.size;
                        cube.get(face, x, y)
                    })
                    .collect();
                upload_face(queue, &prefiltered, mip as u32, face, cube.size, &texels);
            }
        }

        // BRDF LUT.
        let lut_size = data.brdf_lut_size.max(1);
        let brdf = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ibl.brdf_lut"),
            size: wgpu::Extent3d {
                width: lut_size,
                height: lut_size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let lut_texels: Vec<[f32; 4]> = (0..(lut_size * lut_size) as usize)
            .map(|i| {
                let a = data.brdf_lut.get(i * 2).copied().unwrap_or(1.0);
                let b = data.brdf_lut.get(i * 2 + 1).copied().unwrap_or(0.0);
                [a, b, 0.0, 1.0]
            })
            .collect();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &brdf,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &f16_bytes(&lut_texels),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(lut_size * 8),
                rows_per_image: Some(lut_size),
            },
            wgpu::Extent3d {
                width: lut_size,
                height: lut_size,
                depth_or_array_layers: 1,
            },
        );

        let cube_view = |texture: &wgpu::Texture| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::Cube),
                ..Default::default()
            })
        };

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ibl.sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            lod_min_clamp: 0.0,
            lod_max_clamp: 16.0,
            ..Default::default()
        });

        let comparison_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow.comparison_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });

        IblGpu {
            irradiance_view: cube_view(&irradiance),
            prefiltered_view: cube_view(&prefiltered),
            prefiltered_mips: mips,
            brdf_view: brdf.create_view(&wgpu::TextureViewDescriptor::default()),
            sampler,
            comparison_sampler,
        }
    }
}
