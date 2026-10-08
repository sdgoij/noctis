//! Noctis math foundation.
//!
//! This crate is the single source of truth for math across the engine. It is
//! built on [`glm`] (a GLSL-style math library) for vectors and matrices, and
//! adds the pieces `glm` deliberately omits:
//!
//! * [`Quat`] — a quaternion type (`glm` has none), with conversions to and
//!   from rotation matrices, Euler angles, and axis/angle pairs.
//! * [`Transform`] — a translation/rotation/scale transform with matrix
//!   composition.
//! * [`matrices`] — projection and view matrix builders (`perspective`,
//!   `look_at_rh`, ...) written directly against `glm`'s column-major
//!   [`Mat4`], plus column-array conversion for the GPU.
//! * [`Aabb`] — an axis-aligned bounding box.
//!
//! Everything is `f32`-based: this is a real-time 3D engine and the GPU
//! pipeline is `f32`, so keeping one scalar type avoids conversion noise.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod bounds;
pub mod matrices;
pub mod quat;
pub mod transform;

pub use bounds::Aabb;
pub use matrices::{
    from_cols_array, from_quat, identity, inverse, look_at_lh, look_at_rh, orthographic,
    perspective, rotation_x, rotation_y, rotation_z, scaling, to_cols_array, translation, transpose,
    trs, trs_to_cols_array,
};
pub use quat::Quat;
pub use transform::Transform;

// Re-exports of the `glm` building blocks so downstream crates depend on
// `noctis-math` alone and never on `glm` directly.
pub use glm::{Mat2, Mat3, Mat4, Vec2, Vec3, Vec4};
pub use glm::{cross, distance, dot, length, normalize};

/// Construct a `f32` 3-component vector.
#[inline]
pub fn vec3(x: f32, y: f32, z: f32) -> Vec3 {
    glm::vec3(x, y, z)
}

/// Construct a `f32` 4-component vector.
#[inline]
pub fn vec4(x: f32, y: f32, z: f32, w: f32) -> Vec4 {
    glm::vec4(x, y, z, w)
}

/// Construct a `f32` 2-component vector.
#[inline]
pub fn vec2(x: f32, y: f32) -> Vec2 {
    glm::vec2(x, y)
}

/// The zero vector.
#[inline]
pub fn zero3() -> Vec3 {
    glm::vec3(0.0, 0.0, 0.0)
}

/// The all-ones vector.
#[inline]
pub fn one3() -> Vec3 {
    glm::vec3(1.0, 1.0, 1.0)
}

/// The `+X` unit vector.
#[inline]
pub fn unit_x() -> Vec3 {
    glm::vec3(1.0, 0.0, 0.0)
}

/// The `+Y` unit vector.
#[inline]
pub fn unit_y() -> Vec3 {
    glm::vec3(0.0, 1.0, 0.0)
}

/// The `+Z` unit vector.
#[inline]
pub fn unit_z() -> Vec3 {
    glm::vec3(0.0, 0.0, 1.0)
}

/// Approximate equality for scalars.
#[inline]
pub fn approx_eq(a: f32, b: f32, eps: f32) -> bool {
    (a - b).abs() <= eps
}

/// Approximate equality for vectors, component-wise.
#[inline]
pub fn vec3_approx_eq(a: Vec3, b: Vec3, eps: f32) -> bool {
    approx_eq(a.x, b.x, eps) && approx_eq(a.y, b.y, eps) && approx_eq(a.z, b.z, eps)
}

/// Linear interpolation between two scalars.
#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Clamp a scalar to `[min, max]`.
#[inline]
pub fn clamp(v: f32, min: f32, max: f32) -> f32 {
    if v < min {
        min
    } else if v > max {
        max
    } else {
        v
    }
}

/// Convert degrees to radians.
#[inline]
pub fn radians(degrees: f32) -> f32 {
    degrees * std::f32::consts::PI / 180.0
}

/// Convert radians to degrees.
#[inline]
pub fn degrees(radians: f32) -> f32 {
    radians * 180.0 / std::f32::consts::PI
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_constructors_and_arithmetic() {
        let a = vec3(1.0, 2.0, 3.0);
        let b = vec3(4.0, 5.0, 6.0);
        let sum = a + b;
        assert_eq!((sum.x, sum.y, sum.z), (5.0, 7.0, 9.0));

        let scaled = a * 2.0;
        assert_eq!((scaled.x, scaled.y, scaled.z), (2.0, 4.0, 6.0));
    }

    #[test]
    fn lerp_and_clamp() {
        assert!((lerp(0.0, 10.0, 0.25) - 2.5).abs() < 1e-6);
        assert_eq!(clamp(-1.0, 0.0, 1.0), 0.0);
        assert_eq!(clamp(2.0, 0.0, 1.0), 1.0);
        assert_eq!(clamp(0.5, 0.0, 1.0), 0.5);
    }

    #[test]
    fn degree_radian_round_trip() {
        let r = radians(90.0);
        assert!((r - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!((degrees(r) - 90.0).abs() < 1e-6);
    }

    #[test]
    fn vec_approx_eq_works() {
        assert!(vec3_approx_eq(vec3(1.0, 2.0, 3.0), vec3(1.0, 2.0, 3.0 + 1e-7), 1e-5));
        assert!(!vec3_approx_eq(vec3(1.0, 2.0, 3.0), vec3(1.0, 2.0, 3.1), 1e-5));
    }
}
