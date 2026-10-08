//! HDR tonemapping and color-space conversion.
//!
//! The forward pass renders into a linear `rgba16float` (or `rgba32float`) HDR
//! target. The tonemap pass converts those unbounded linear values into
//! displayable `[0, 1]` sRGB using one of the curves below and then applies the
//! piecewise sRGB transfer function.
//!
//! * [`Tonemap::Aces`] — the Narkowicz fit of the ACES filmic curve. It has a
//!   gentle shoulder and gives filmic highlights.
//! * [`Tonemap::Reinhard`] — `x / (1 + x)`, the classic extended Reinhard
//!   operator. Cheaper, flatter highlights.
//!
//! All functions here are pure and unit-tested; the WGSL mirror lives in
//! `shaders/tonemap.wgsl` and must be kept numerically in sync.

/// A tonemapping operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tonemap {
    /// The ACES filmic curve (Narkowicz 2015 fit).
    #[default]
    Aces,
    /// Extended Reinhard: `x / (1 + x)`.
    Reinhard,
}

/// A single component of the ACES filmic curve.
///
/// `(x * (a*x + b)) / (x * (c*x + d) + e)` with the published fit constants.
#[inline]
pub fn aces_filmic_component(x: f32) -> f32 {
    const A: f32 = 2.51;
    const B: f32 = 0.03;
    const C: f32 = 2.43;
    const D: f32 = 0.59;
    const E: f32 = 0.14;
    ((x * (A * x + B)) / (x * (C * x + D) + E)).clamp(0.0, 1.0)
}

/// Apply the ACES filmic curve to a linear RGB triple.
pub fn aces_filmic(rgb: [f32; 3]) -> [f32; 3] {
    [
        aces_filmic_component(rgb[0]),
        aces_filmic_component(rgb[1]),
        aces_filmic_component(rgb[2]),
    ]
}

/// Apply the extended Reinhard curve to a linear RGB triple.
pub fn reinhard(rgb: [f32; 3]) -> [f32; 3] {
    let f = |x: f32| {
        let x = x.max(0.0);
        x / (1.0 + x)
    };
    [f(rgb[0]), f(rgb[1]), f(rgb[2])]
}

/// Convert one linear component to the sRGB transfer function.
#[inline]
pub fn linear_to_srgb(c: f32) -> f32 {
    let c = c.max(0.0);
    if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Convert one sRGB-encoded component back to linear.
#[inline]
pub fn srgb_to_linear(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Tonemap and gamma-encode a linear HDR color.
///
/// `exposure` is a linear multiplier applied before the curve.
pub fn tonemap(rgb: [f32; 3], exposure: f32, mode: Tonemap) -> [f32; 3] {
    let exposed = [rgb[0] * exposure, rgb[1] * exposure, rgb[2] * exposure];
    let mapped = match mode {
        Tonemap::Aces => aces_filmic(exposed),
        Tonemap::Reinhard => reinhard(exposed),
    };
    [
        linear_to_srgb(mapped[0]),
        linear_to_srgb(mapped[1]),
        linear_to_srgb(mapped[2]),
    ]
}

/// Tonemap without gamma encoding (leaves the result linear in `[0, 1]`).
pub fn tonemap_linear(rgb: [f32; 3], exposure: f32, mode: Tonemap) -> [f32; 3] {
    let exposed = [rgb[0] * exposure, rgb[1] * exposure, rgb[2] * exposure];
    match mode {
        Tonemap::Aces => aces_filmic(exposed),
        Tonemap::Reinhard => reinhard(exposed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_stays_black() {
        for mode in [Tonemap::Aces, Tonemap::Reinhard] {
            let out = tonemap([0.0, 0.0, 0.0], 1.0, mode);
            assert_eq!(out, [0.0, 0.0, 0.0]);
        }
    }

    #[test]
    fn output_is_bounded() {
        for mode in [Tonemap::Aces, Tonemap::Reinhard] {
            let out = tonemap([1000.0, 5000.0, 0.25], 4.0, mode);
            for c in out {
                assert!((0.0..=1.0).contains(&c), "component {c} out of range");
            }
        }
    }

    #[test]
    fn curve_is_monotonic() {
        let mut prev = -1.0;
        for i in 0..=100 {
            let x = i as f32 * 0.1;
            let y = aces_filmic_component(x);
            assert!(y >= prev - 1e-6, "aces not monotonic at {x}");
            prev = y;
        }
        let mut prev = -1.0;
        for i in 0..=100 {
            let x = i as f32 * 0.1;
            let y = reinhard([x, 0.0, 0.0])[0];
            assert!(y >= prev - 1e-6, "reinhard not monotonic at {x}");
            prev = y;
        }
    }

    #[test]
    fn reinhard_matches_formula() {
        let out = reinhard([1.0, 2.0, 3.0]);
        assert!((out[0] - 0.5).abs() < 1e-6);
        assert!((out[1] - 2.0 / 3.0).abs() < 1e-6);
        assert!((out[2] - 0.75).abs() < 1e-6);
    }

    #[test]
    fn srgb_round_trip() {
        for i in 0..=255 {
            let c = i as f32 / 255.0;
            let round = linear_to_srgb(srgb_to_linear(c));
            assert!((round - c).abs() < 1e-4, "round trip {c} -> {round}");
        }
    }

    #[test]
    fn srgb_endpoints() {
        assert!((linear_to_srgb(0.0)).abs() < 1e-6);
        assert!((linear_to_srgb(1.0) - 1.0).abs() < 1e-4);
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn exposure_darkens_and_brightens() {
        let dark = tonemap_linear([0.5, 0.5, 0.5], 1.0, Tonemap::Reinhard)[0];
        let bright = tonemap_linear([0.5, 0.5, 0.5], 2.0, Tonemap::Reinhard)[0];
        assert!(bright > dark);
    }

    #[test]
    fn default_is_aces() {
        assert_eq!(Tonemap::default(), Tonemap::Aces);
    }
}
