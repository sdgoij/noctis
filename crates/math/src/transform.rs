//! Translation / rotation / scale transforms.

use crate::matrices;
use crate::{Mat4, Quat, Vec3, vec3};

/// A rigid-ish transform: translation, rotation, and (non-uniform) scale.
///
/// The composed matrix is `T * R * S`, so scale is applied first in local
/// space, then rotation, then translation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    /// Position in the parent space.
    pub translation: Vec3,
    /// Rotation, always stored as a (nominally unit) quaternion.
    pub rotation: Quat,
    /// Non-uniform scale.
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Self::identity()
    }
}

impl Transform {
    /// The identity transform.
    #[inline]
    pub fn identity() -> Self {
        Self {
            translation: vec3(0.0, 0.0, 0.0),
            rotation: Quat::identity(),
            scale: vec3(1.0, 1.0, 1.0),
        }
    }

    /// A transform with only a translation.
    #[inline]
    pub fn from_translation(translation: Vec3) -> Self {
        Self {
            translation,
            ..Self::identity()
        }
    }

    /// A transform with only a rotation.
    #[inline]
    pub fn from_rotation(rotation: Quat) -> Self {
        Self {
            rotation,
            ..Self::identity()
        }
    }

    /// A transform with only a scale.
    #[inline]
    pub fn from_scale(scale: Vec3) -> Self {
        Self {
            scale,
            ..Self::identity()
        }
    }

    /// Builder: set the translation.
    #[inline]
    pub fn with_translation(mut self, translation: Vec3) -> Self {
        self.translation = translation;
        self
    }

    /// Builder: set the rotation.
    #[inline]
    pub fn with_rotation(mut self, rotation: Quat) -> Self {
        self.rotation = rotation;
        self
    }

    /// Builder: set the scale.
    #[inline]
    pub fn with_scale(mut self, scale: Vec3) -> Self {
        self.scale = scale;
        self
    }

    /// The composed `T * R * S` matrix.
    #[inline]
    pub fn to_matrix(&self) -> Mat4 {
        matrices::trs(self.translation, self.rotation, self.scale)
    }

    /// The composed matrix as GPU-ready columns.
    #[inline]
    pub fn to_cols_array(&self) -> [[f32; 4]; 4] {
        matrices::trs_to_cols_array(self.translation, self.rotation, self.scale)
    }

    /// Transform a point (applies scale, rotation, then translation).
    #[inline]
    pub fn transform_point(&self, point: Vec3) -> Vec3 {
        let scaled = vec3(
            point.x * self.scale.x,
            point.y * self.scale.y,
            point.z * self.scale.z,
        );
        self.rotation.rotate_vec3(scaled) + self.translation
    }

    /// Transform a direction (applies scale then rotation; ignores translation).
    #[inline]
    pub fn transform_vector(&self, vector: Vec3) -> Vec3 {
        let scaled = vec3(
            vector.x * self.scale.x,
            vector.y * self.scale.y,
            vector.z * self.scale.z,
        );
        self.rotation.rotate_vec3(scaled)
    }

    /// The inverse transform.
    ///
    /// This is exact when the scale is **uniform**. For non-uniform scale a
    /// `T·R·S` transform cannot represent its own inverse (the inverse needs
    /// scale applied *after* rotation), so this returns a best-effort
    /// approximation; use [`Transform::inverse_matrix`] when you need the exact
    /// inverse for arbitrary scale.
    pub fn inverse(&self) -> Option<Self> {
        if self.scale.x.abs() < f32::EPSILON
            || self.scale.y.abs() < f32::EPSILON
            || self.scale.z.abs() < f32::EPSILON
        {
            return None;
        }
        let inv_rot = self.rotation.conjugate();
        let inv_scale = vec3(1.0 / self.scale.x, 1.0 / self.scale.y, 1.0 / self.scale.z);
        let inv_translation = inv_rot.rotate_vec3(vec3(
            -self.translation.x * inv_scale.x,
            -self.translation.y * inv_scale.y,
            -self.translation.z * inv_scale.z,
        ));
        Some(Self {
            translation: inv_translation,
            rotation: inv_rot,
            scale: inv_scale,
        })
    }

    /// The exact inverse of [`Transform::to_matrix`], or `None` if singular.
    /// Works for arbitrary (including non-uniform) scale.
    #[inline]
    pub fn inverse_matrix(&self) -> Option<Mat4> {
        matrices::inverse(&self.to_matrix())
    }

    /// Compose `self` with `child`, producing the child's transform in the
    /// parent space of `self` (scale of `self` is applied to the child).
    pub fn mul(&self, child: &Transform) -> Transform {
        let matrix = self.to_matrix() * child.to_matrix();
        // Decompose back into T/R/S. Scale is taken from the column lengths,
        // which is exact for non-shearing compositions.
        let c0 = matrix[0];
        let c1 = matrix[1];
        let c2 = matrix[2];
        let c3 = matrix[3];
        let sx = vlen(vec3(c0.x, c0.y, c0.z));
        let sy = vlen(vec3(c1.x, c1.y, c1.z));
        let sz = vlen(vec3(c2.x, c2.y, c2.z));
        let scale = vec3(sx, sy, sz);
        let inv = |v: f32| if v > 0.0 { 1.0 / v } else { 0.0 };
        let rotation = Quat::look_rotation(
            -vec3(c2.x * inv(sz), c2.y * inv(sz), c2.z * inv(sz)),
            vec3(c1.x * inv(sy), c1.y * inv(sy), c1.z * inv(sy)),
        );
        Transform {
            translation: vec3(c3.x, c3.y, c3.z),
            rotation,
            scale,
        }
    }

    /// Interpolate toward `other`; rotation uses slerp, position and scale lerp.
    pub fn lerp(&self, other: &Transform, t: f32) -> Transform {
        let l = |a: f32, b: f32| a + (b - a) * t;
        Transform {
            translation: vec3(
                l(self.translation.x, other.translation.x),
                l(self.translation.y, other.translation.y),
                l(self.translation.z, other.translation.z),
            ),
            rotation: self.rotation.slerp(other.rotation, t),
            scale: vec3(
                l(self.scale.x, other.scale.x),
                l(self.scale.y, other.scale.y),
                l(self.scale.z, other.scale.z),
            ),
        }
    }
}

fn vlen(v: Vec3) -> f32 {
    (v.x * v.x + v.y * v.y + v.z * v.z).sqrt()
}

impl std::ops::Mul for Transform {
    type Output = Transform;
    #[inline]
    fn mul(self, rhs: Transform) -> Transform {
        Transform::mul(&self, &rhs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vec3_approx_eq;

    const EPS: f32 = 1e-4;

    #[test]
    fn default_is_identity() {
        assert_eq!(Transform::default(), Transform::identity());
    }

    #[test]
    fn transform_point_applies_srt_order() {
        let t = Transform::identity()
            .with_scale(vec3(2.0, 2.0, 2.0))
            .with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2))
            .with_translation(vec3(1.0, 0.0, 0.0));
        let p = t.transform_point(vec3(1.0, 0.0, 0.0));
        // scale -> (2,0,0); rotate 90z -> (0,2,0); translate -> (1,2,0)
        assert!(vec3_approx_eq(p, vec3(1.0, 2.0, 0.0), EPS), "got {p:?}");
    }

    #[test]
    fn transform_vector_ignores_translation() {
        let t = Transform::identity().with_translation(vec3(5.0, 5.0, 5.0));
        let v = t.transform_vector(vec3(1.0, 0.0, 0.0));
        assert!(vec3_approx_eq(v, vec3(1.0, 0.0, 0.0), EPS));
    }

    #[test]
    fn inverse_round_trips_point() {
        let t = Transform::identity()
            .with_scale(vec3(2.0, 2.0, 2.0))
            .with_rotation(Quat::from_euler_xyz(0.3, -0.4, 0.9))
            .with_translation(vec3(-2.0, 1.0, 7.0));
        let inv = t.inverse().expect("invertible");
        let p = vec3(1.5, -0.5, 2.0);
        let round = inv.transform_point(t.transform_point(p));
        assert!(vec3_approx_eq(round, p, 1e-3), "got {round:?}");
    }

    #[test]
    fn inverse_matrix_round_trips_nonuniform_point() {
        let t = Transform::identity()
            .with_scale(vec3(2.0, 3.0, 4.0))
            .with_rotation(Quat::from_euler_xyz(0.3, -0.4, 0.9))
            .with_translation(vec3(-2.0, 1.0, 7.0));
        let inv = t.inverse_matrix().expect("invertible");
        let p = crate::vec4(1.5, -0.5, 2.0, 1.0);
        let forward = t.to_matrix() * p;
        let back = inv * forward;
        assert!((back.x - p.x).abs() < 1e-3, "x {} vs {}", back.x, p.x);
        assert!((back.y - p.y).abs() < 1e-3, "y {} vs {}", back.y, p.y);
        assert!((back.z - p.z).abs() < 1e-3, "z {} vs {}", back.z, p.z);
    }

    #[test]
    fn inverse_of_zero_scale_is_none() {
        let t = Transform::from_scale(vec3(1.0, 0.0, 1.0));
        assert!(t.inverse().is_none());
    }

    #[test]
    fn composition_matches_matrix_product() {
        let parent = Transform::identity()
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2))
            .with_translation(vec3(5.0, 0.0, 0.0));
        let child = Transform::from_translation(vec3(1.0, 0.0, 0.0));
        let composed = parent * child;
        // child's origin in parent space: rotate (1,0,0) by 90y -> (0,0,-1),
        // then translate -> (5,0,-1).
        assert!(vec3_approx_eq(composed.translation, vec3(5.0, 0.0, -1.0), 1e-3));
    }

    #[test]
    fn lerp_midpoint() {
        let a = Transform::from_translation(vec3(0.0, 0.0, 0.0));
        let b = Transform::from_translation(vec3(10.0, 0.0, 0.0));
        let mid = a.lerp(&b, 0.5);
        assert!(vec3_approx_eq(mid.translation, vec3(5.0, 0.0, 0.0), EPS));
    }
}
