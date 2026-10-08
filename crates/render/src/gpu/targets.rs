//! Offscreen render targets for headless rendering.
//!
//! The frame is drawn into an HDR `rgba16float` target, tonemapped into a
//! `rgba8unorm` LDR target, and then copied into a mappable readback buffer.
//! A `depth32float` depth buffer and an `r8unorm` SSAO target complete the set.

/// A texture plus its default view.
#[derive(Debug)]
pub struct GpuTarget {
    /// The backing texture.
    pub texture: wgpu::Texture,
    /// The default view.
    pub view: wgpu::TextureView,
}

impl GpuTarget {
    fn new(device: &wgpu::Device, label: &str, desc: &wgpu::TextureDescriptor<'_>) -> GpuTarget {
        let texture = device.create_texture(desc);
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some(label),
            ..Default::default()
        });
        GpuTarget { texture, view }
    }
}

/// The texture set for one offscreen frame plus its readback buffer.
#[derive(Debug)]
pub struct RenderTargets {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// HDR color target (`rgba16float`).
    pub hdr: GpuTarget,
    /// LDR sRGB-encoded color target (`rgba8unorm`).
    pub ldr: GpuTarget,
    /// Depth buffer (`depth32float`).
    pub depth: GpuTarget,
    /// Ambient-occlusion target (`r8unorm`).
    pub ssao: GpuTarget,
    /// Mappable buffer that receives the LDR pixels.
    pub readback: wgpu::Buffer,
    /// Row stride of the readback buffer (256-byte aligned).
    pub padded_bytes_per_row: u32,
}

/// The 256-byte row alignment required by texture-to-buffer copies.
pub const COPY_BYTES_PER_ROW_ALIGNMENT: u32 = 256;

/// Round `value` up to the next multiple of `alignment`.
pub fn align_up(value: u32, alignment: u32) -> u32 {
    if alignment == 0 {
        return value;
    }
    value.div_ceil(alignment) * alignment
}

impl RenderTargets {
    /// Create the targets for a `width` x `height` frame (both clamped to >= 1).
    pub fn create(device: &wgpu::Device, width: u32, height: u32) -> RenderTargets {
        let width = width.max(1);
        let height = height.max(1);
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let color = |label: &str, format: wgpu::TextureFormat, usage: wgpu::TextureUsages| {
            GpuTarget::new(
                device,
                label,
                &wgpu::TextureDescriptor {
                    label: Some(label),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                },
            )
        };

        let hdr = color(
            "hdr",
            wgpu::TextureFormat::Rgba16Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let ldr = color(
            "ldr",
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let depth = color(
            "depth",
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let ssao = color(
            "ssao",
            wgpu::TextureFormat::R8Unorm,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );

        let padded_bytes_per_row = align_up(width * 4, COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (padded_bytes_per_row as u64) * (height as u64),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        RenderTargets {
            width,
            height,
            hdr,
            ldr,
            depth,
            ssao,
            readback,
            padded_bytes_per_row,
        }
    }

    /// The number of bytes needed for the unpadded RGBA8 frame.
    pub fn frame_byte_len(&self) -> usize {
        (self.width * self.height * 4) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn align_up_rounds_to_256() {
        assert_eq!(align_up(0, 256), 0);
        assert_eq!(align_up(1, 256), 256);
        assert_eq!(align_up(256, 256), 256);
        assert_eq!(align_up(257, 256), 512);
        // e.g. a 300-pixel-wide rgba8 row is 1200 bytes -> 1280.
        assert_eq!(align_up(300 * 4, 256), 1280);
    }
}
