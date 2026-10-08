//! Quaternions.
//!
//! [`glm`] has no quaternion type, so the engine defines one here. The layout
//! is `(x, y, z, w)` with `w` the scalar part, and `Quat` is `#[repr(C)]` so a
//! `[f32; 4]` can be viewed as one (useful for uniform buffers).
//!
//! Conventions:
//!
//! * A quaternion `q` rotates a vector `v` as `q * v * q⁻¹`.
//! * [`Quat::mul`] composes rotations so that `(a * b)` applies `b` first.
//! * [`Quat::from_euler_xyz`] interprets `(roll, pitch, yaw)` as intrinsic
//!   rotations about X, then Y, then Z.
//! * Angles are radians and rotation is right-handed.

use crate::{Vec3, vec3};

/// A rotation quaternion.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    /// The `x` (i) component.
    pub x: f32,
    /// The `y` (j) component.
    pub y: f32,
    /// The `z` (k) component.
    pub z: f32,
    /// The `w` (scalar / real) component.
    pub w: f32,
}

impl Default for Quat {
    fn default() -> Self {
        Self::identity()
    }
}

impl Quat {
    /// The identity rotation.
    #[inline]
    pub const fn identity() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        }
    }

    /// Construct from raw components.
    #[inline]
    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    /// The vector (imaginary) part.
    #[inline]
    pub fn vector(self) -> Vec3 {
        vec3(self.x, self.y, self.z)
    }

    /// The scalar (real) part.
    #[inline]
    pub fn scalar(self) -> f32 {
        self.w
    }

    /// A quaternion from an axis and angle. `axis` need not be normalized.
    pub fn from_axis_angle(axis: Vec3, angle: f32) -> Self {
        let axis = normalize_or(axis, vec3(1.0, 0.0, 0.0));
        let half = angle * 0.5;
        let s = half.sin();
        Self {
            x: axis.x * s,
            y: axis.y * s,
            z: axis.z * s,
            w: half.cos(),
        }
    }

    /// A rotation about the X axis by `angle` radians.
    pub fn from_rotation_x(angle: f32) -> Self {
        Self::from_axis_angle(vec3(1.0, 0.0, 0.0), angle)
    }

    /// A rotation about the Y axis by `angle` radians.
    pub fn from_rotation_y(angle: f32) -> Self {
        Self::from_axis_angle(vec3(0.0, 1.0, 0.0), angle)
    }

    /// A rotation about the Z axis by `angle` radians.
    pub fn from_rotation_z(angle: f32) -> Self {
        Self::from_axis_angle(vec3(0.0, 0.0, 1.0), angle)
    }

    /// An intrinsic XYZ Euler rotation `(roll, pitch, yaw)` in radians.
    ///
    /// Equivalent to `from_rotation_z(yaw) * from_rotation_y(pitch) *
    /// from_rotation_x(roll)`.
    pub fn from_euler_xyz(roll: f32, pitch: f32, yaw: f32) -> Self {
        let qx = Self::from_rotation_x(roll);
        let qy = Self::from_rotation_y(pitch);
        let qz = Self::from_rotation_z(yaw);
        qz.mul(qy).mul(qx)
    }

    /// Decompose into intrinsic XYZ Euler angles `(roll, pitch, yaw)`.
    ///
    /// For pitch values outside `(-π/2, π/2)` the result may be a different
    /// but equivalent set of angles (gimbal lock behaviour).
    pub fn to_euler_xyz(self) -> (f32, f32, f32) {
        let q = self.normalized();
        // Sourced from the standard ZYX matrix extraction, adapted to our
        // qz*qy*qx convention.
        let (x, y, z, w) = (q.x, q.y, q.z, q.w);

        let sinp = clamp_unit(2.0 * (w * y - z * x));
        let pitch = sinp.asin();
        let (roll, yaw) = if sinp.abs() > 0.999_999 {
            (0.0, 2.0 * x.atan2(2.0 * w * z + 2.0 * y * x))
        } else {
            let roll = (2.0 * (w * x + y * z)).atan2(1.0 - 2.0 * (x * x + y * y));
            let yaw = (2.0 * (w * z + x * y)).atan2(1.0 - 2.0 * (y * y + z * z));
            (roll, yaw)
        };
        (roll, pitch, yaw)
    }

    /// The dot product of two quaternions.
    #[inline]
    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z + self.w * other.w
    }

    /// The squared length.
    #[inline]
    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }

    /// The length.
    #[inline]
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    /// A normalized copy. Falls back to identity for degenerate input.
    pub fn normalized(self) -> Self {
        let len = self.length();
        if len > 0.0 {
            Self::new(self.x / len, self.y / len, self.z / len, self.w / len)
        } else {
            Self::identity()
        }
    }

    /// The conjugate, which for a unit quaternion is the inverse rotation.
    #[inline]
    pub fn conjugate(self) -> Self {
        Self::new(-self.x, -self.y, -self.z, self.w)
    }

    /// The inverse. For unit quaternions this equals the conjugate.
    pub fn inverse(self) -> Self {
        let len_sq = self.length_squared();
        if len_sq > 0.0 {
            let inv = 1.0 / len_sq;
            Self::new(-self.x * inv, -self.y * inv, -self.z * inv, self.w * inv)
        } else {
            Self::identity()
        }
    }

    /// Negate all components (same rotation, opposite hemisphere).
    #[inline]
    pub fn negate(self) -> Self {
        Self::new(-self.x, -self.y, -self.z, -self.w)
    }

    /// Hamilton product; `a.mul(b)` applies `b` then `a`.
    ///
    /// This is the named form of the `*` operator: [`std::ops::Mul`] is
    /// implemented for `Quat` and forwards here. Both forms are part of the
    /// `math` contract (see `docs/core-api.md`), so the method is intentionally
    /// kept next to the operator. The lint is a false positive: the trait is
    /// already implemented, and consumers such as `editor` call `.mul(..)`
    /// without importing `std::ops::Mul`.
    #[allow(clippy::should_implement_trait)]
    pub fn mul(self, rhs: Self) -> Self {
        Self::new(
            self.w * rhs.x + self.x * rhs.w + self.y * rhs.z - self.z * rhs.y,
            self.w * rhs.y - self.x * rhs.z + self.y * rhs.w + self.z * rhs.x,
            self.w * rhs.z + self.x * rhs.y - self.y * rhs.x + self.z * rhs.w,
            self.w * rhs.w - self.x * rhs.x - self.y * rhs.y - self.z * rhs.z,
        )
    }

    /// Rotate a vector by this quaternion.
    pub fn rotate_vec3(self, v: Vec3) -> Vec3 {
        let qv = self.vector();
        let t = cross3(qv, v) * 2.0;
        v + t * self.w + cross3(qv, t)
    }

    /// Spherical linear interpolation. `t` is clamped to `[0, 1]`.
    pub fn slerp(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let mut other = other;
        let mut cos = self.dot(other);
        if cos < 0.0 {
            cos = -cos;
            other = other.negate();
        }
        if cos > 0.999_5 {
            // Too close for a stable slerp; nlerp is fine here.
            return Self::new(
                self.x + (other.x - self.x) * t,
                self.y + (other.y - self.y) * t,
                self.z + (other.z - self.z) * t,
                self.w + (other.w - self.w) * t,
            )
            .normalized();
        }
        let theta = cos.clamp(-1.0, 1.0).acos();
        let sin_theta = theta.sin();
        let a = ((1.0 - t) * theta).sin() / sin_theta;
        let b = (t * theta).sin() / sin_theta;
        Self::new(
            self.x * a + other.x * b,
            self.y * a + other.y * b,
            self.z * a + other.z * b,
            self.w * a + other.w * b,
        )
        .normalized()
    }

    /// Look-at rotation: orients `-Z` toward `forward` with `up` as the up
    /// reference. `forward` and `up` need not be normalized.
    pub fn look_rotation(forward: Vec3, up: Vec3) -> Self {
        let f = normalize_or(forward, vec3(0.0, 0.0, -1.0));
        let up_hint = normalize_or(up, vec3(0.0, 1.0, 0.0));
        // For a right-handed basis with the camera looking along `-Z`:
        //   right = forward × up,  up = right × forward,  z = -forward.
        let mut right = cross3(f, up_hint);
        if dot3(right, right) < 1e-12 {
            // `forward` is parallel to the up hint; pick another reference.
            let alt = if f.y.abs() > 0.99 {
                vec3(1.0, 0.0, 0.0)
            } else {
                vec3(0.0, 1.0, 0.0)
            };
            right = cross3(f, alt);
        }
        let right = normalize_or(right, vec3(1.0, 0.0, 0.0));
        let u = cross3(right, f);

        // Camera basis columns are (right, up, -forward).
        quat_from_basis(right.x, u.x, -f.x, right.y, u.y, -f.y, right.z, u.z, -f.z)
    }
}

fn clamp_unit(v: f32) -> f32 {
    v.clamp(-1.0, 1.0)
}

fn normalize_or(v: Vec3, fallback: Vec3) -> Vec3 {
    let len_sq = v.x * v.x + v.y * v.y + v.z * v.z;
    if len_sq > 0.0 {
        let inv = len_sq.sqrt().recip();
        vec3(v.x * inv, v.y * inv, v.z * inv)
    } else {
        fallback
    }
}

/// Dot product of two `f32` vec3s.
#[inline]
fn dot3(a: Vec3, b: Vec3) -> f32 {
    a.x * b.x + a.y * b.y + a.z * b.z
}

/// Cross product of two `f32` vec3s.
#[inline]
pub fn cross3(a: Vec3, b: Vec3) -> Vec3 {
    vec3(
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    )
}

/// Build a quaternion from a column-major 3x3 rotation basis.
#[allow(clippy::too_many_arguments)]
fn quat_from_basis(
    m00: f32,
    m01: f32,
    m02: f32,
    m10: f32,
    m11: f32,
    m12: f32,
    m20: f32,
    m21: f32,
    m22: f32,
) -> Quat {
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

impl std::ops::Mul for Quat {
    type Output = Quat;
    #[inline]
    fn mul(self, rhs: Quat) -> Quat {
        Quat::mul(self, rhs)
    }
}

impl std::ops::Mul<Vec3> for Quat {
    type Output = Vec3;
    #[inline]
    fn mul(self, rhs: Vec3) -> Vec3 {
        self.rotate_vec3(rhs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Vec3, vec3_approx_eq};

    const EPS: f32 = 1e-5;

    fn normalize_vec(v: Vec3) -> Vec3 {
        let l = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
        vec3(v.x / l, v.y / l, v.z / l)
    }

    #[test]
    fn identity_rotates_nothing() {
        let v = vec3(1.0, 2.0, 3.0);
        assert!(vec3_approx_eq(Quat::identity().rotate_vec3(v), v, EPS));
    }

    #[test]
    fn rotate_90_about_y_maps_z_to_x() {
        let q = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let r = q.rotate_vec3(vec3(0.0, 0.0, 1.0));
        assert!(vec3_approx_eq(r, vec3(1.0, 0.0, 0.0), 1e-4), "got {r:?}");
    }

    #[test]
    fn rotate_90_about_z_maps_x_to_y() {
        let q = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let r = q.rotate_vec3(vec3(1.0, 0.0, 0.0));
        assert!(vec3_approx_eq(r, vec3(0.0, 1.0, 0.0), 1e-4), "got {r:?}");
    }

    #[test]
    fn rotation_preserves_length() {
        let q = Quat::from_axis_angle(vec3(0.3, 0.5, 0.8), 1.234);
        let v = vec3(3.0, -1.0, 2.0);
        let r = q.rotate_vec3(v);
        let len_v = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
        let len_r = (r.x * r.x + r.y * r.y + r.z * r.z).sqrt();
        assert!((len_v - len_r).abs() < 1e-4);
    }

    #[test]
    fn inverse_undoes_rotation() {
        let q = Quat::from_axis_angle(vec3(1.0, 2.0, -1.0), 0.9).normalized();
        let v = vec3(0.5, -0.25, 4.0);
        let round = q.inverse().rotate_vec3(q.rotate_vec3(v));
        assert!(vec3_approx_eq(round, v, 1e-4), "got {round:?}");
    }

    #[test]
    fn composition_applies_rhs_first() {
        let a = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let b = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        let v: Vec3 = vec3(0.0, 0.0, 1.0);
        let composed = (a * b).rotate_vec3(v);
        let sequential = a.rotate_vec3(b.rotate_vec3(v));
        assert!(vec3_approx_eq(composed, sequential, 1e-4));
    }

    #[test]
    fn mul_matches_hamilton_product_on_basis() {
        let i = Quat::new(1.0, 0.0, 0.0, 0.0);
        let j = Quat::new(0.0, 1.0, 0.0, 0.0);
        let k = Quat::new(0.0, 0.0, 1.0, 0.0);
        assert_eq!(i * j, k);
        assert_eq!(j * k, i);
        assert_eq!(k * i, j);
        assert_eq!(i * i, Quat::new(0.0, 0.0, 0.0, -1.0));
    }

    #[test]
    fn euler_round_trip() {
        let (r, p, y) = (0.3, -0.6, 1.1);
        let q = Quat::from_euler_xyz(r, p, y);
        let (r2, p2, y2) = q.to_euler_xyz();
        assert!((r - r2).abs() < 1e-3, "roll {r} vs {r2}");
        assert!((p - p2).abs() < 1e-3, "pitch {p} vs {p2}");
        assert!((y - y2).abs() < 1e-3, "yaw {y} vs {y2}");
    }

    #[test]
    fn slerp_endpoints_and_midpoint() {
        let a = Quat::identity();
        let b = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        assert!(vec3_approx_eq(
            a.slerp(b, 0.0).rotate_vec3(vec3(1.0, 0.0, 0.0)),
            vec3(1.0, 0.0, 0.0),
            1e-4
        ));
        let end = a.slerp(b, 1.0).rotate_vec3(vec3(1.0, 0.0, 0.0));
        assert!(vec3_approx_eq(end, vec3(0.0, 1.0, 0.0), 1e-4));

        let mid = a.slerp(b, 0.5).rotate_vec3(vec3(1.0, 0.0, 0.0));
        let expected = vec3(0.5f32.sqrt(), 0.5f32.sqrt(), 0.0);
        assert!(vec3_approx_eq(mid, expected, 1e-3), "got {mid:?}");
    }

    #[test]
    fn look_rotation_orients_forward() {
        let q = Quat::look_rotation(vec3(0.0, 0.0, -1.0), vec3(0.0, 1.0, 0.0));
        // -Z forward should be close to identity.
        let f = q.rotate_vec3(vec3(0.0, 0.0, -1.0));
        assert!(vec3_approx_eq(f, vec3(0.0, 0.0, -1.0), 1e-4), "got {f:?}");
    }

    #[test]
    fn look_rotation_matches_forward_and_up_for_each_axis() {
        let ups = [
            vec3(0.0, 1.0, 0.0),
            vec3(0.0, 0.0, 1.0),
            vec3(1.0, 0.0, 0.0),
        ];
        let forwards = [
            vec3(1.0, 0.0, 0.0),
            vec3(-1.0, 0.0, 0.0),
            vec3(0.0, 1.0, 0.0),
            vec3(0.0, -1.0, 0.0),
            vec3(0.0, 0.0, 1.0),
            vec3(0.0, 0.0, -1.0),
            vec3(1.0, 1.0, 1.0),
        ];
        for forward in forwards {
            for up in ups {
                if (forward.x * up.x + forward.y * up.y + forward.z * up.z).abs() > 0.99 {
                    continue; // parallel: up hint is degenerate for this pair
                }
                let q = Quat::look_rotation(forward, up);
                let want_f = normalize_vec(forward);
                let got_f = q.rotate_vec3(vec3(0.0, 0.0, -1.0));
                assert!(
                    vec3_approx_eq(got_f, want_f, 1e-3),
                    "forward {forward:?} up {up:?}: got {got_f:?}"
                );
                // The rotated world-up must lie in the plane perpendicular to forward.
                let got_up = q.rotate_vec3(vec3(0.0, 1.0, 0.0));
                let dot = got_up.x * want_f.x + got_up.y * want_f.y + got_up.z * want_f.z;
                assert!(dot.abs() < 1e-3, "up not perpendicular: dot {dot}");
            }
        }
    }

    #[test]
    fn look_rotation_degenerate_up_does_not_panic() {
        let q = Quat::look_rotation(vec3(0.0, 1.0, 0.0), vec3(0.0, 1.0, 0.0));
        let f = q.rotate_vec3(vec3(0.0, 0.0, -1.0));
        assert!(vec3_approx_eq(f, vec3(0.0, 1.0, 0.0), 1e-3), "got {f:?}");
        assert!((q.length() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn normalized_handles_zero() {
        assert_eq!(Quat::new(0.0, 0.0, 0.0, 0.0).normalized(), Quat::identity());
    }
}
