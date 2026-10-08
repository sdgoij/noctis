//! Conversions between the engine's `math` types and rapier's glam-based math.
//!
//! rapier 3d 0.36 exposes its public math surface through `glamx` (a `glam`
//! extension crate), re-exported as `rapier3d::math`. Its `Vector` is a
//! `glam::Vec3` and its `Rotation` a `glam::Quat`, both `f32`. These helpers
//! keep the conversion logic in one place so the rest of the crate only deals
//! in `math` types.

use math::{Mat4, Quat, Vec3, vec3};
use rapier3d::math::{Pose, Rotation, Vector};

/// `math::Vec3` -> rapier `Vector`.
#[inline]
pub(crate) fn to_vector(v: Vec3) -> Vector {
    Vector::new(v.x, v.y, v.z)
}

/// rapier `Vector` -> `math::Vec3`.
#[inline]
pub(crate) fn from_vector(v: Vector) -> Vec3 {
    vec3(v.x, v.y, v.z)
}

/// `math::Quat` -> rapier `Rotation`.
#[inline]
pub(crate) fn to_rotation(q: Quat) -> Rotation {
    Rotation::from_xyzw(q.x, q.y, q.z, q.w)
}

/// rapier `Rotation` -> `math::Quat`.
#[inline]
pub(crate) fn from_rotation(q: Rotation) -> Quat {
    Quat::new(q.x, q.y, q.z, q.w)
}

/// Build a rapier pose from a translation and rotation.
#[inline]
pub(crate) fn to_pose(translation: Vec3, rotation: Quat) -> Pose {
    Pose::from_parts(to_vector(translation), to_rotation(rotation))
}

/// Recover the rotation from a world matrix.
///
/// The upper-left 3x3 block is treated as a (possibly scaled) rotation basis:
/// each column is normalized before the quaternion is extracted, so a uniform
/// or non-uniform scale on the `GlobalTransform` does not leak into the
/// rotation.
pub(crate) fn rotation_from_matrix(m: &Mat4) -> Quat {
    let c0 = m[0];
    let c1 = m[1];
    let c2 = m[2];
    let x = normalize_or(c0.x, c0.y, c0.z, vec3(1.0, 0.0, 0.0));
    let y = normalize_or(c1.x, c1.y, c1.z, vec3(0.0, 1.0, 0.0));
    let z = normalize_or(c2.x, c2.y, c2.z, vec3(0.0, 0.0, 1.0));

    // `m[row][col]`, matching `math::Quat::look_rotation`'s basis convention:
    // column `col` is the image of the `col`-th basis vector.
    let (m00, m10, m20) = (x.x, x.y, x.z);
    let (m01, m11, m21) = (y.x, y.y, y.z);
    let (m02, m12, m22) = (z.x, z.y, z.z);

    let trace = m00 + m11 + m22;
    if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        Quat::new((m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, 0.25 * s)
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        Quat::new(0.25 * s, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s)
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        Quat::new((m01 + m10) / s, 0.25 * s, (m12 + m21) / s, (m02 - m20) / s)
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        Quat::new((m02 + m20) / s, (m12 + m21) / s, 0.25 * s, (m10 - m01) / s)
    }
    .normalized()
}

/// Normalize `(x, y, z)`, falling back to `fallback` for a degenerate vector.
fn normalize_or(x: f32, y: f32, z: f32, fallback: Vec3) -> Vec3 {
    let len_sq = x * x + y * y + z * z;
    if len_sq > 0.0 {
        let inv = len_sq.sqrt().recip();
        vec3(x * inv, y * inv, z * inv)
    } else {
        fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use math::{Transform, vec3_approx_eq};

    #[test]
    fn vector_round_trip() {
        let v = math::vec3(1.5, -2.25, 3.75);
        assert!(vec3_approx_eq(from_vector(to_vector(v)), v, 1e-6));
    }

    #[test]
    fn rotation_round_trip() {
        let q = Quat::from_axis_angle(math::vec3(1.0, 2.0, -1.0), 0.7).normalized();
        let back = from_rotation(to_rotation(q));
        assert!(vec3_approx_eq(back.vector(), q.vector(), 1e-6));
        assert!((back.w - q.w).abs() < 1e-6);
    }

    #[test]
    fn rotation_from_matrix_recovers_rotation() {
        let q = Quat::from_euler_xyz(0.3, -0.6, 1.1);
        let m = Transform::from_rotation(q).to_matrix();
        let recovered = rotation_from_matrix(&m);
        // Compare by rotating several probes rather than components, so the
        // sign-ambiguous quaternion representation doesn't matter.
        for probe in [math::unit_x(), math::unit_y(), math::unit_z()] {
            let a = q.rotate_vec3(probe);
            let b = recovered.rotate_vec3(probe);
            assert!(
                vec3_approx_eq(a, b, 1e-4),
                "probe {probe:?}: {a:?} vs {b:?}"
            );
        }
    }

    #[test]
    fn rotation_from_matrix_ignores_uniform_scale() {
        let q = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let m = Transform::from_rotation(q)
            .with_scale(math::vec3(3.0, 3.0, 3.0))
            .to_matrix();
        let recovered = rotation_from_matrix(&m);
        let rotated = recovered.rotate_vec3(math::unit_z());
        assert!(vec3_approx_eq(rotated, q.rotate_vec3(math::unit_z()), 1e-4));
    }

    #[test]
    fn rotation_from_identity_matrix_is_identity() {
        let recovered = rotation_from_matrix(&math::identity());
        assert!(vec3_approx_eq(recovered.vector(), math::zero3(), 1e-6));
        assert!((recovered.w - 1.0).abs() < 1e-6);
    }
}
