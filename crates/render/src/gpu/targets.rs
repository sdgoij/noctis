//! Offscreen render targets for headless rendering.
//!
//! The frame is drawn into an HDR `rgba16float` target, tonemapped into a
//! `rgba8unorm` LDR target, and then copied into a mappable readback buffer.
//! A `depth32float` depth buffer and an `r8unorm` SSAO target complete the set.
//!
//! With MSAA (`sample_count > 1`) the forward passes render into multisampled
//! colour and depth textures that are **resolved into the single-sampled HDR
//! target** at the end of each pass, so the tonemap (and everything after it)
//! works on one sample per pixel as usual. Resolving *before* tonemapping is what
//! makes the average land in the right place: the resolve mixes linear radiance,
//! not tonemapped colour.
//!
//! The camera depth target stays single-sampled either way, because a
//! multisampled depth texture cannot be sampled by a screen-space pass and SSAO
//! reads it. When MSAA is on the forward passes write their own multisampled
//! depth, leaving this target as an SSAO source only — so at that point it is
//! allocated at **half resolution** (see [`camera_depth_size`]), quartering the
//! depth prepass's fragment work.

/// A texture plus its default view.
#[derive(Debug)]
pub struct GpuTarget {
    /// The backing texture.
    pub texture: wgpu::Texture,
    /// The default view.
    pub view: wgpu::TextureView,
}

impl GpuTarget {
    pub(crate) fn new(
        device: &wgpu::Device,
        label: &str,
        desc: &wgpu::TextureDescriptor<'_>,
    ) -> GpuTarget {
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
    /// The MSAA sample count in use (`1` = no MSAA).
    pub sample_count: u32,
    /// Multisampled HDR colour, when [`sample_count`](Self::sample_count) > 1.
    pub hdr_msaa: Option<GpuTarget>,
    /// Multisampled depth, when [`sample_count`](Self::sample_count) > 1.
    pub depth_msaa: Option<GpuTarget>,
    /// HDR colour: the resolve target when multisampled, else the render target.
    /// Bound by the tonemap pass.
    pub hdr: GpuTarget,
    /// Single-sampled camera depth. It is the SSAO source, and (at 1x) the forward
    /// and text depth target. Half resolution when MSAA is on, where SSAO is its
    /// only reader.
    pub depth: GpuTarget,
    /// LDR sRGB-encoded color target (`rgba8unorm`).
    pub ldr: GpuTarget,
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

/// The pixel size of the single-sampled camera depth target.
///
/// It is full resolution when single-sampled, because the forward and text passes
/// test against it. Under MSAA those passes write their own multisampled depth,
/// so this target is read only by SSAO — a low-frequency effect — and half
/// resolution suffices, quartering the depth prepass's fragment work.
pub fn camera_depth_size(width: u32, height: u32, sample_count: u32) -> (u32, u32) {
    if sample_count > 1 {
        (width.div_ceil(2), height.div_ceil(2))
    } else {
        (width, height)
    }
}

impl RenderTargets {
    /// Create the targets for a `width` x `height` frame (both clamped to >= 1),
    /// with MSAA at `sample_count` samples (`1` disables it).
    pub fn create(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        sample_count: u32,
    ) -> RenderTargets {
        let width = width.max(1);
        let height = height.max(1);
        let samples = sample_count.max(1);
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let target = |label: &str,
                      extent: wgpu::Extent3d,
                      format: wgpu::TextureFormat,
                      sample_count: u32,
                      usage: wgpu::TextureUsages| {
            GpuTarget::new(
                device,
                label,
                &wgpu::TextureDescriptor {
                    label: Some(label),
                    size: extent,
                    mip_level_count: 1,
                    sample_count,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                },
            )
        };

        // The HDR attachment (multisampled when MSAA is on) and the single
        // sampled texture it resolves into.
        let hdr_msaa = (samples > 1).then(|| {
            target(
                "hdr.msaa",
                size,
                wgpu::TextureFormat::Rgba16Float,
                samples,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
        });
        let hdr = target(
            "hdr",
            size,
            wgpu::TextureFormat::Rgba16Float,
            1,
            // A resolve target must be a render attachment too.
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        // The single-sampled camera depth SSAO reads, plus the multisampled depth
        // the forward passes write. At 1x one target fills both roles; at 4x it is
        // SSAO's alone, so it shrinks to half resolution.
        let (depth_width, depth_height) = camera_depth_size(width, height, samples);
        let depth = target(
            "depth",
            wgpu::Extent3d {
                width: depth_width,
                height: depth_height,
                depth_or_array_layers: 1,
            },
            wgpu::TextureFormat::Depth32Float,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let depth_msaa = (samples > 1).then(|| {
            target(
                "depth.msaa",
                size,
                wgpu::TextureFormat::Depth32Float,
                samples,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
        });
        let ldr = target(
            "ldr",
            size,
            wgpu::TextureFormat::Rgba8Unorm,
            // RENDER_ATTACHMENT: the tonemap pass renders into it. COPY_SRC: the
            // headless readback copies from it. TEXTURE_BINDING: the present
            // pass samples it to blit onto the surface.
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let ssao = target(
            "ssao",
            size,
            wgpu::TextureFormat::R8Unorm,
            1,
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
            sample_count: samples,
            hdr_msaa,
            depth_msaa,
            hdr,
            depth,
            ldr,
            ssao,
            readback,
            padded_bytes_per_row,
        }
    }

    /// `true` when the frame is rendered with MSAA.
    pub fn is_multisampled(&self) -> bool {
        self.hdr_msaa.is_some()
    }

    /// The colour attachment the forward passes render into (multisampled when
    /// MSAA is on).
    pub fn color_target(&self) -> &wgpu::TextureView {
        self.hdr_msaa
            .as_ref()
            .map_or(&self.hdr.view, |msaa| &msaa.view)
    }

    /// The texture `color_target` resolves into, or `None` when single-sampled.
    pub fn color_resolve(&self) -> Option<&wgpu::TextureView> {
        self.hdr_msaa.as_ref().map(|_| &self.hdr.view)
    }

    /// The depth attachment the forward passes test and write.
    pub fn depth_target(&self) -> &wgpu::TextureView {
        self.depth_msaa
            .as_ref()
            .map_or(&self.depth.view, |msaa| &msaa.view)
    }

    /// The number of bytes needed for the unpadded RGBA8 frame.
    pub fn frame_byte_len(&self) -> usize {
        (self.width * self.height * 4) as usize
    }
}

/// The colour and depth attachments for a second view of the scene — the planar
/// reflection pass (`.notes/reflections.md`, tier 2).
///
/// It is the frame's colour/depth pairing with one difference: the depth is one
/// texture at the pass's own sample count, because nothing samples a reflected
/// depth (the frame's single-sampled depth exists for SSAO; a reflection has no
/// such reader). The colour keeps the MSAA-plus-resolve pair, because the
/// receiver's shader samples it.
#[derive(Debug)]
pub struct ViewTargets {
    /// View width in pixels.
    pub width: u32,
    /// View height in pixels.
    pub height: u32,
    /// The MSAA sample count in use (`1` = no MSAA).
    pub sample_count: u32,
    /// Multisampled colour, when [`sample_count`](Self::sample_count) > 1.
    pub color_msaa: Option<GpuTarget>,
    /// The colour a shader samples: the resolve target under MSAA, else the
    /// render target.
    pub color: GpuTarget,
    /// The depth attachment, at the pass's sample count.
    pub depth: GpuTarget,
}

impl ViewTargets {
    /// Create the attachments for a `width` x `height` view, with MSAA at
    /// `sample_count` samples (`1` disables it).
    pub fn create(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        sample_count: u32,
    ) -> ViewTargets {
        let width = width.max(1);
        let height = height.max(1);
        let samples = sample_count.max(1);
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let target =
            |label: &str, format: wgpu::TextureFormat, samples: u32, usage: wgpu::TextureUsages| {
                GpuTarget::new(
                    device,
                    label,
                    &wgpu::TextureDescriptor {
                        label: Some(label),
                        size,
                        mip_level_count: 1,
                        sample_count: samples,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage,
                        view_formats: &[],
                    },
                )
            };

        let color_msaa = (samples > 1).then(|| {
            target(
                "reflection.color.msaa",
                wgpu::TextureFormat::Rgba16Float,
                samples,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
        });
        let color = target(
            "reflection.color",
            wgpu::TextureFormat::Rgba16Float,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let depth = target(
            "reflection.depth",
            wgpu::TextureFormat::Depth32Float,
            samples,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );

        ViewTargets {
            width,
            height,
            sample_count: samples,
            color_msaa,
            color,
            depth,
        }
    }

    /// `true` when the view is rendered with MSAA.
    pub fn is_multisampled(&self) -> bool {
        self.color_msaa.is_some()
    }

    /// The colour attachment the pass renders into (multisampled when MSAA is
    /// on).
    pub fn color_target(&self) -> &wgpu::TextureView {
        self.color_msaa
            .as_ref()
            .map_or(&self.color.view, |msaa| &msaa.view)
    }

    /// The texture `color_target` resolves into, or `None` when single-sampled.
    pub fn color_resolve(&self) -> Option<&wgpu::TextureView> {
        self.color_msaa.as_ref().map(|_| &self.color.view)
    }

    /// The depth attachment the pass tests and writes.
    pub fn depth_target(&self) -> &wgpu::TextureView {
        &self.depth.view
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

    #[test]
    fn the_camera_depth_shrinks_only_under_msaa() {
        // Single-sampled: the forward and text passes test against it, so full.
        assert_eq!(camera_depth_size(1280, 720, 1), (1280, 720));
        // MSAA: SSAO's alone, so half — with odd sizes rounding up, never 0.
        assert_eq!(camera_depth_size(1280, 720, 4), (640, 360));
        assert_eq!(camera_depth_size(1, 1, 4), (1, 1));
        assert_eq!(camera_depth_size(3, 5, 2), (2, 3));
    }
}
