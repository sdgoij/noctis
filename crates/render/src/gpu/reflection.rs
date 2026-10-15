//! The planar reflection pass's GPU resources (`.notes/reflections.md`, tier 2).
//!
//! The engine renders the scene from the camera a surface's `PlanarReflection`
//! mirrors the primary one into, into an offscreen colour target; the receiver's
//! shader samples that target at its own screen position. The pass reuses the
//! frame's pipelines, draw list and object uniforms, so what it owns is only: the
//! target and its depth, group 0 carrying the mirrored camera, and the sampler and
//! 1x1 empty texture that keep a pass from sampling what it writes.

use crate::gpu::ibl_gpu::IblGpu;
use crate::gpu::pipelines::{BindGroupLayouts, FrameBindings};
use crate::gpu::targets::{GpuTarget, ViewTargets};
use crate::gpu::uniforms::{FrameUniform, ReflectionUniform, ShadowUniform};
use crate::lights::LightsUniform;

/// The smallest fraction of the frame a reflection may be rendered at, so a tiny
/// `resolution` still yields a texel.
const MIN_SCALE: f32 = 0.05;

/// The pixel size of a reflection target: `scale` of a `width` x `height` frame,
/// clamped to a sane range and never zero.
pub fn reflection_size(width: u32, height: u32, scale: f32) -> (u32, u32) {
    let scale = scale.clamp(MIN_SCALE, 1.0);
    let scaled = |extent: u32| ((extent as f32 * scale).round() as u32).max(1);
    (scaled(width), scaled(height))
}

/// The engine's planar reflection: one offscreen view per frame, re-created only
/// when its size changes.
#[derive(Debug)]
pub struct Reflection {
    targets: ViewTargets,
    frame: FrameBindings,
    sampler: wgpu::Sampler,
    /// A 1x1 white texture the pass's own group 0 binds as both the reflection and
    /// the SSAO source: a pass must never sample the texture it is rendering into,
    /// and a reflection wants no part of the frame's ambient occlusion.
    empty: GpuTarget,
    /// The size last requested, as a fraction of the frame.
    scale: f32,
}

impl Reflection {
    /// Create the pass's resources for a `width` x `height` frame rendered at
    /// `sample_count` samples per pixel.
    // The arguments are the device, the queue (for the empty texture) and the
    // frame's shape — the same bundle every GPU resource here needs.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layouts: &BindGroupLayouts,
        ibl: &IblGpu,
        shadow_view: &wgpu::TextureView,
        width: u32,
        height: u32,
        sample_count: u32,
    ) -> Reflection {
        let empty = GpuTarget::new(
            device,
            "reflection.empty",
            &wgpu::TextureDescriptor {
                label: Some("reflection.empty"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
        );
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &empty.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255u8, 255, 255, 255],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("reflection.sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let targets = ViewTargets::create(device, width, height, sample_count);
        let frame = FrameBindings::new(
            device,
            &layouts.frame,
            ibl,
            shadow_view,
            &empty.view,
            &empty.view,
            &sampler,
        );

        Reflection {
            targets,
            frame,
            sampler,
            empty,
            scale: 1.0,
        }
    }

    /// Rebuild the pass's group 0 after a texture it binds changes — a new IBL, or
    /// a new shadow map. The target is not one of them, so this is rare.
    pub fn set_ibl(
        &mut self,
        device: &wgpu::Device,
        layouts: &BindGroupLayouts,
        ibl: &IblGpu,
        shadow_view: &wgpu::TextureView,
    ) {
        self.frame = FrameBindings::new(
            device,
            &layouts.frame,
            ibl,
            shadow_view,
            &self.empty.view,
            &self.empty.view,
            &self.sampler,
        );
    }

    /// The colour the receiver samples.
    pub fn color_view(&self) -> &wgpu::TextureView {
        &self.targets.color.view
    }

    /// The sampler a receiver samples the colour with.
    pub fn sampler(&self) -> &wgpu::Sampler {
        &self.sampler
    }

    /// Group 0 of the reflection pass: the frame layout fed the mirrored camera.
    pub fn frame(&self) -> &FrameBindings {
        &self.frame
    }

    /// The pass's colour attachment (multisampled when MSAA is on).
    pub fn color_target(&self) -> &wgpu::TextureView {
        self.targets.color_target()
    }

    /// The texture [`color_target`](Self::color_target) resolves into, or `None`
    /// when single-sampled.
    pub fn color_resolve(&self) -> Option<&wgpu::TextureView> {
        self.targets.color_resolve()
    }

    /// The pass's depth attachment.
    pub fn depth_target(&self) -> &wgpu::TextureView {
        self.targets.depth_target()
    }

    /// The size last requested, as a fraction of the frame.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// The target's size in pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.targets.width, self.targets.height)
    }

    /// Stage the pass's uniforms: the mirrored frame, the frame's lights and
    /// shadow, and — for every receiver in the *pass* — a disabled reflection, so
    /// nothing samples the target being written.
    pub fn write(
        &self,
        queue: &wgpu::Queue,
        frame: &FrameUniform,
        lights: &LightsUniform,
        shadow: &ShadowUniform,
    ) {
        self.frame.write(queue, frame, lights, shadow);
        self.frame
            .write_reflection(queue, &ReflectionUniform::default());
    }

    /// Resize the target to `scale` of a `width` x `height` frame. Returns `true`
    /// when the target was re-created — its view changed, so every bind group that
    /// holds it must be rebuilt.
    pub fn set_size(&mut self, device: &wgpu::Device, width: u32, height: u32, scale: f32) -> bool {
        let (w, h) = reflection_size(width, height, scale);
        self.scale = scale.clamp(MIN_SCALE, 1.0);
        if w == self.targets.width && h == self.targets.height {
            return false;
        }
        self.targets = ViewTargets::create(device, w, h, self.targets.sample_count);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reflection_target_scales_with_the_frame() {
        assert_eq!(reflection_size(1280, 720, 1.0), (1280, 720));
        assert_eq!(reflection_size(1280, 720, 0.5), (640, 360));
        // An odd size rounds, and nothing ever collapses to zero pixels.
        assert_eq!(reflection_size(9, 9, 0.5), (5, 5));
        assert_eq!(reflection_size(1, 1, 1.0), (1, 1));
    }

    #[test]
    fn the_scale_is_clamped() {
        // Above one it is the whole frame; below the floor it is the floor, so a
        // nonsensical `resolution` cannot ask for a zero-pixel target.
        assert_eq!(reflection_size(100, 100, 4.0), (100, 100));
        assert_eq!(
            reflection_size(100, 100, 0.0),
            reflection_size(100, 100, MIN_SCALE)
        );
        assert_eq!(
            reflection_size(100, 100, -1.0),
            reflection_size(100, 100, 0.0)
        );
        assert_eq!(reflection_size(0, 0, 1.0), (1, 1));
    }
}
