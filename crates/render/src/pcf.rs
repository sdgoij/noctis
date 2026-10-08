//! Percentage-closer filtering (PCF) for shadow maps.
//!
//! Soft shadows come from averaging several depth comparisons around the
//! receiver's shadow-map coordinate. The kernel below is a deterministic,
//! separable grid: for a requested sample count `n` it builds the smallest
//! `k x k` grid (`k = ceil(sqrt(n))`) whose texel centres span `[-0.5, 0.5]` in
//! shadow-map UV space. Multiplying by the shadow texel size converts these to
//! world-independent UV offsets.
//!
//! The slope-scaled bias grows the depth comparison epsilon as the surface turns
//! away from the light, which removes most acne without introducing too much
//! peter-panning. The formula is
//!
//! ```text
//! bias = base + normal_offset * tan(acos(clamp(dot(N, L), 0, 1)))
//! ```

/// The default PCF kernel size used by the renderer.
pub const DEFAULT_PCF_KERNEL: u32 = 16;

/// Generate kernel offsets in shadow-map UV space, spanning `[-0.5, 0.5]`.
///
/// `kernel == 1` yields the single centre tap. Otherwise the result is
/// `ceil(sqrt(kernel))^2` taps laid out on a regular grid.
pub fn pcf_kernel_offsets(kernel: u32) -> Vec<[f32; 2]> {
    if kernel <= 1 {
        return vec![[0.0, 0.0]];
    }
    let k = (kernel as f32).sqrt().ceil() as u32;
    let mut offsets = Vec::with_capacity((k * k) as usize);
    for y in 0..k {
        for x in 0..k {
            let fx = (x as f32 + 0.5) / k as f32 - 0.5;
            let fy = (y as f32 + 0.5) / k as f32 - 0.5;
            offsets.push([fx, fy]);
        }
    }
    offsets
}

/// The number of taps [`pcf_kernel_offsets`] produces for a requested `kernel`.
pub fn pcf_tap_count(kernel: u32) -> u32 {
    if kernel <= 1 {
        1
    } else {
        let k = (kernel as f32).sqrt().ceil() as u32;
        k * k
    }
}

/// Slope-scaled depth bias for shadow lookups.
///
/// `n_dot_l` is clamped to `[0, 1]`; the `normal_offset` term dominates as the
/// surface becomes grazing, and `base` covers front-facing acne.
pub fn slope_scaled_bias(n_dot_l: f32, base: f32, normal_offset: f32) -> f32 {
    let ndotl = n_dot_l.clamp(0.0, 1.0);
    let tan_theta = (1.0 - ndotl * ndotl).max(0.0).sqrt() / ndotl.max(1e-3);
    base + normal_offset * tan_theta.min(8.0)
}

/// Compute the shadow factor from a set of pre-computed bias-corrected depths.
///
/// `occluder` values are the light-space depths stored in the shadow map for
/// each tap and `receiver` is the fragment's light-space depth. A tap
/// contributes light (`1.0`) when the receiver is in front of, or within `bias`
/// of, the stored depth; those contributions are averaged.
pub fn shadow_factor(occluder: &[f32], receiver: f32, bias: f32) -> f32 {
    if occluder.is_empty() {
        return 1.0;
    }
    let lit = occluder
        .iter()
        .filter(|&&depth| receiver - bias <= depth)
        .count();
    lit as f32 / occluder.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_tap_is_centre() {
        assert_eq!(pcf_kernel_offsets(1), vec![[0.0, 0.0]]);
        assert_eq!(pcf_tap_count(1), 1);
    }

    #[test]
    fn offsets_span_unit_square() {
        for kernel in [4u32, 9, 16, 25, 64] {
            let offsets = pcf_kernel_offsets(kernel);
            assert!(offsets.len() as u32 >= kernel);
            for [x, y] in &offsets {
                assert!((-0.5..=0.5).contains(x), "x = {x}");
                assert!((-0.5..=0.5).contains(y), "y = {y}");
            }
        }
    }

    #[test]
    fn grid_is_square_and_centred() {
        let offsets = pcf_kernel_offsets(16);
        assert_eq!(offsets.len(), 16);
        let sum: f32 = offsets.iter().map(|o| o[0]).sum();
        assert!(sum.abs() < 1e-5, "kernel is not centred: {sum}");
    }

    #[test]
    fn tap_count_matches_offsets() {
        for kernel in [1u32, 2, 5, 16, 100] {
            assert_eq!(
                pcf_kernel_offsets(kernel).len() as u32,
                pcf_tap_count(kernel)
            );
        }
    }

    #[test]
    fn bias_grows_at_grazing_angles() {
        let front = slope_scaled_bias(1.0, 0.001, 0.005);
        let grazing = slope_scaled_bias(0.05, 0.001, 0.005);
        assert!(
            front < grazing,
            "front {front} should be < grazing {grazing}"
        );
        assert!(front >= 0.001);
    }

    #[test]
    fn bias_handles_zero_n_dot_l() {
        let b = slope_scaled_bias(0.0, 0.001, 0.005);
        assert!(b.is_finite());
        assert!(b > 0.0);
    }

    #[test]
    fn fully_lit_when_receiver_in_front() {
        let occluders = [0.5, 0.6, 0.7, 0.8];
        let factor = shadow_factor(&occluders, 0.1, 0.0);
        assert_eq!(factor, 1.0);
    }

    #[test]
    fn fully_shadowed_when_receiver_behind() {
        let occluders = [0.5, 0.6, 0.7, 0.8];
        let factor = shadow_factor(&occluders, 0.9, 0.0);
        assert_eq!(factor, 0.0);
    }

    #[test]
    fn partial_occlusion_is_penumbra() {
        let occluders = [0.5, 0.5, 0.9, 0.9];
        let factor = shadow_factor(&occluders, 0.7, 0.0);
        assert!((factor - 0.5).abs() < 1e-6, "factor = {factor}");
    }

    #[test]
    fn empty_occluders_is_lit() {
        assert_eq!(shadow_factor(&[], 0.5, 0.0), 1.0);
    }
}
