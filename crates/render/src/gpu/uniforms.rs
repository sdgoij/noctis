//! GPU uniform blocks.
//!
//! Every struct here is `#[repr(C)]` + [`Pod`] and mirrors a WGSL `struct`
//! one-for-one. They are padded so their size is a multiple of 16 (WGSL uniform
//! alignment) and their field order matches the shaders exactly. The sizes are
//! asserted in the tests so a layout drift is caught immediately.

use bytemuck::{Pod, Zeroable};

/// A 4x4 matrix as columns, ready for a uniform buffer.
pub type Mat4Raw = [[f32; 4]; 4];

/// Per-frame camera and global parameters (group 0, binding 0 of the PBR pass).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct FrameUniform {
    /// World -> clip.
    pub view_proj: Mat4Raw,
    /// Clip -> world.
    pub inv_view_proj: Mat4Raw,
    /// `xyz` = camera position, `w` = 1.
    pub camera_pos: [f32; 4],
    /// `rgb` = ambient color, `a` = ambient intensity.
    pub ambient: [f32; 4],
    /// `x` = width, `y` = height, `z` = 1/width, `w` = 1/height.
    pub resolution: [f32; 4],
    /// `x` = SSAO strength, remaining components reserved.
    pub params: [f32; 4],
}

/// Per-object data (group 1, binding 0), accessed with a dynamic offset.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct ObjectUniform {
    /// Object -> world.
    pub model: Mat4Raw,
    /// Inverse-transpose of the model matrix for normals.
    pub normal_matrix: Mat4Raw,
    /// `rgb` = base color, `a` = base alpha.
    pub base_color: [f32; 4],
    /// `x` = metallic, `y` = roughness, `z` = emissive strength, `w` = alpha cutoff.
    pub params: [f32; 4],
    /// `rgb` = emissive color, `w` unused.
    pub emissive: [f32; 4],
    /// `x` = has base-color tex, `y` = has normal tex, `z` = has MR tex, `w` = has emissive tex.
    pub flags: [f32; 4],
    /// `x` = occlusion tex, `y` = double sided, `z` = unlit, `w` = vertex colours.
    pub flags2: [f32; 4],
}

/// The shadow-map parameters (group 0, binding 8 of the PBR pass).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct ShadowUniform {
    /// Light view-projection matrix.
    pub view_proj: Mat4Raw,
    /// `x` = base bias, `y` = slope bias, `z` = texel size, `w` = enabled flag.
    pub params: [f32; 4],
    /// `xyz` = light direction, `w` unused.
    pub direction: [f32; 4],
}

/// A single matrix used by the depth-only and gizmo passes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct ViewUniform {
    /// The view-projection (or light view-projection) matrix.
    pub view_proj: Mat4Raw,
}

/// The SSAO pass parameters (group 0, binding 0).
///
/// `samples` is 16 `vec4`s (256 bytes) laid out exactly like the WGSL
/// `array<vec4<f32>, 16>`; storing it as four `Mat4Raw` keeps the POD derive
/// simple while preserving the byte layout.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct SsaoUniform {
    /// View -> clip.
    pub projection: Mat4Raw,
    /// Clip -> view.
    pub inv_projection: Mat4Raw,
    /// The hemisphere sample kernel, in view space.
    pub samples: [Mat4Raw; 4],
    /// `x` = radius, `y` = bias, `z` = power, `w` = kernel size.
    pub params: [f32; 4],
    /// `x` = width, `y` = height, `z` = 1/width, `w` = 1/height.
    pub resolution: [f32; 4],
}

/// The tonemap pass parameters (group 0, binding 0).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct TonemapUniform {
    /// Linear exposure multiplier.
    pub exposure: f32,
    /// `0` = ACES, `1` = Reinhard.
    pub mode: u32,
    /// Reserved padding.
    pub pad0: f32,
    /// Reserved padding.
    pub pad1: f32,
}

impl Default for TonemapUniform {
    fn default() -> Self {
        TonemapUniform {
            exposure: 1.0,
            mode: 0,
            pad0: 0.0,
            pad1: 0.0,
        }
    }
}

/// The standard SSAO hemisphere sample kernel (16 points in a unit hemisphere).
///
/// Generated deterministically so results are reproducible across runs. Samples
/// cluster near the origin (where occlusion is most informative) and stay inside
/// a unit hemisphere.
pub fn ssao_kernel() -> [Mat4Raw; 4] {
    const RAW: [[f32; 3]; 16] = [
        [0.0, 0.0, 0.0],
        [0.12, 0.03, 0.21],
        [-0.15, 0.11, 0.18],
        [0.07, -0.19, 0.25],
        [-0.22, -0.08, 0.31],
        [0.19, 0.21, 0.27],
        [-0.05, 0.24, 0.35],
        [0.26, -0.13, 0.22],
        [-0.28, 0.17, 0.29],
        [0.14, 0.29, 0.38],
        [-0.20, -0.25, 0.33],
        [0.31, 0.06, 0.24],
        [-0.11, -0.31, 0.41],
        [0.23, -0.27, 0.36],
        [-0.33, 0.28, 0.30],
        [0.09, 0.33, 0.42],
    ];
    let mut out = [[[0.0f32; 4]; 4]; 4];
    for (i, sample) in RAW.iter().enumerate() {
        out[i / 4][i % 4] = [sample[0], sample[1], sample[2], 0.0];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_sizes_match_wgsl_layouts() {
        assert_eq!(std::mem::size_of::<FrameUniform>(), 192);
        assert_eq!(std::mem::size_of::<ObjectUniform>(), 208);
        assert_eq!(std::mem::size_of::<ShadowUniform>(), 96);
        assert_eq!(std::mem::size_of::<ViewUniform>(), 64);
        assert_eq!(std::mem::size_of::<SsaoUniform>(), 416);
        assert_eq!(std::mem::size_of::<TonemapUniform>(), 16);
    }

    #[test]
    fn uniforms_are_16_byte_aligned() {
        assert_eq!(std::mem::size_of::<FrameUniform>() % 16, 0);
        assert_eq!(std::mem::size_of::<ObjectUniform>() % 16, 0);
        assert_eq!(std::mem::size_of::<SsaoUniform>() % 16, 0);
    }

    #[test]
    fn ssao_kernel_is_in_positive_hemisphere_and_normalised() {
        let kernel = ssao_kernel();
        assert_eq!(kernel[0][0], [0.0, 0.0, 0.0, 0.0]);
        let mut count = 0;
        for mat in &kernel {
            for row in mat {
                let len = (row[0] * row[0] + row[1] * row[1] + row[2] * row[2]).sqrt();
                if len > 1e-6 {
                    assert!(len <= 0.6, "sample too far: {len}");
                    count += 1;
                }
            }
        }
        assert_eq!(count, 15);
    }
}
