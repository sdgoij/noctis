//! 4x4 transform, view, and projection matrices.
//!
//! All matrices are [`glm`] [`Mat4`] values, which are column-major
//! (`c0..c3`) and `#[repr(C)]`. The projection matrices target a **zero-to-one**
//! clip depth range (the wgpu/Vulkan/D3D convention) and right-handed view
//! space unless the name says otherwise.
//!
//! Builders are implemented directly against the column vectors rather than
//! [`glm::ext`] so the conventions are explicit and documented here.

use crate::quat::Quat;
use crate::{Vec3, Vec4, vec3};
use glm::{Mat4, vec4 as gvec4};

/// The 4x4 identity matrix.
#[inline]
pub fn identity() -> Mat4 {
    Mat4::new(
        gvec4(1.0, 0.0, 0.0, 0.0),
        gvec4(0.0, 1.0, 0.0, 0.0),
        gvec4(0.0, 0.0, 1.0, 0.0),
        gvec4(0.0, 0.0, 0.0, 1.0),
    )
}

/// Build a matrix from four column vectors.
#[inline]
pub fn from_cols(c0: Vec4, c1: Vec4, c2: Vec4, c3: Vec4) -> Mat4 {
    Mat4::new(c0, c1, c2, c3)
}

/// A translation matrix.
#[inline]
pub fn translation(t: Vec3) -> Mat4 {
    Mat4::new(
        gvec4(1.0, 0.0, 0.0, 0.0),
        gvec4(0.0, 1.0, 0.0, 0.0),
        gvec4(0.0, 0.0, 1.0, 0.0),
        gvec4(t.x, t.y, t.z, 1.0),
    )
}

/// A scaling matrix.
#[inline]
pub fn scaling(s: Vec3) -> Mat4 {
    Mat4::new(
        gvec4(s.x, 0.0, 0.0, 0.0),
        gvec4(0.0, s.y, 0.0, 0.0),
        gvec4(0.0, 0.0, s.z, 0.0),
        gvec4(0.0, 0.0, 0.0, 1.0),
    )
}

/// A rotation about the X axis.
pub fn rotation_x(angle: f32) -> Mat4 {
    let (s, c) = angle.sin_cos();
    Mat4::new(
        gvec4(1.0, 0.0, 0.0, 0.0),
        gvec4(0.0, c, s, 0.0),
        gvec4(0.0, -s, c, 0.0),
        gvec4(0.0, 0.0, 0.0, 1.0),
    )
}

/// A rotation about the Y axis.
pub fn rotation_y(angle: f32) -> Mat4 {
    let (s, c) = angle.sin_cos();
    Mat4::new(
        gvec4(c, 0.0, -s, 0.0),
        gvec4(0.0, 1.0, 0.0, 0.0),
        gvec4(s, 0.0, c, 0.0),
        gvec4(0.0, 0.0, 0.0, 1.0),
    )
}

/// A rotation about the Z axis.
pub fn rotation_z(angle: f32) -> Mat4 {
    let (s, c) = angle.sin_cos();
    Mat4::new(
        gvec4(c, s, 0.0, 0.0),
        gvec4(-s, c, 0.0, 0.0),
        gvec4(0.0, 0.0, 1.0, 0.0),
        gvec4(0.0, 0.0, 0.0, 1.0),
    )
}

/// A rotation matrix from a unit quaternion.
pub fn from_quat(q: Quat) -> Mat4 {
    let q = q.normalized();
    let (x, y, z, w) = (q.x, q.y, q.z, q.w);
    let (xx, yy, zz) = (x * x, y * y, z * z);
    let (xy, xz, yz) = (x * y, x * z, y * z);
    let (wx, wy, wz) = (w * x, w * y, w * z);

    Mat4::new(
        gvec4(1.0 - 2.0 * (yy + zz), 2.0 * (xy + wz), 2.0 * (xz - wy), 0.0),
        gvec4(2.0 * (xy - wz), 1.0 - 2.0 * (xx + zz), 2.0 * (yz + wx), 0.0),
        gvec4(2.0 * (xz + wy), 2.0 * (yz - wx), 1.0 - 2.0 * (xx + yy), 0.0),
        gvec4(0.0, 0.0, 0.0, 1.0),
    )
}

/// Compose translation, rotation, and scale into a `T * R * S` matrix.
#[inline]
pub fn trs(t: Vec3, r: Quat, s: Vec3) -> Mat4 {
    translation(t) * from_quat(r) * scaling(s)
}

/// A right-handed perspective projection mapping depth to `[0, 1]`.
///
/// `fov_y` is the vertical field of view in radians.
pub fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    assert!(aspect > 0.0, "aspect ratio must be positive");
    assert!(near > 0.0, "near plane must be positive");
    assert!(far > near, "far plane must be greater than near plane");
    let f = 1.0 / (fov_y * 0.5).tan();
    Mat4::new(
        gvec4(f / aspect, 0.0, 0.0, 0.0),
        gvec4(0.0, f, 0.0, 0.0),
        gvec4(0.0, 0.0, far / (near - far), -1.0),
        gvec4(0.0, 0.0, (far * near) / (near - far), 0.0),
    )
}

/// A right-handed orthographic projection mapping depth to `[0, 1]`.
pub fn orthographic(left: f32, right: f32, bottom: f32, top: f32, near: f32, far: f32) -> Mat4 {
    Mat4::new(
        gvec4(2.0 / (right - left), 0.0, 0.0, 0.0),
        gvec4(0.0, 2.0 / (top - bottom), 0.0, 0.0),
        gvec4(0.0, 0.0, 1.0 / (near - far), 0.0),
        gvec4(
            -(right + left) / (right - left),
            -(top + bottom) / (top - bottom),
            near / (near - far),
            1.0,
        ),
    )
}

/// A right-handed look-at view matrix, looking down `-Z`.
pub fn look_at_rh(eye: Vec3, center: Vec3, up: Vec3) -> Mat4 {
    let f = normalize_or(center - eye, vec3(0.0, 0.0, -1.0));
    let up = normalize_or(up, vec3(0.0, 1.0, 0.0));
    let mut s = cross3(f, up);
    if s.x * s.x + s.y * s.y + s.z * s.z < 1e-12 {
        // Degenerate: view direction parallel to up.
        s = cross3(f, vec3(0.0, 0.0, 1.0));
    }
    let s = normalize_or(s, vec3(1.0, 0.0, 0.0));
    let u = cross3(s, f);

    Mat4::new(
        gvec4(s.x, u.x, -f.x, 0.0),
        gvec4(s.y, u.y, -f.y, 0.0),
        gvec4(s.z, u.z, -f.z, 0.0),
        gvec4(-dot3(s, eye), -dot3(u, eye), dot3(f, eye), 1.0),
    )
}

/// A left-handed look-at view matrix, looking down `+Z`.
pub fn look_at_lh(eye: Vec3, center: Vec3, up: Vec3) -> Mat4 {
    let f = normalize_or(center - eye, vec3(0.0, 0.0, 1.0));
    let up = normalize_or(up, vec3(0.0, 1.0, 0.0));
    let mut s = cross3(up, f);
    if s.x * s.x + s.y * s.y + s.z * s.z < 1e-12 {
        s = cross3(vec3(0.0, 0.0, 1.0), f);
    }
    let s = normalize_or(s, vec3(1.0, 0.0, 0.0));
    let u = cross3(f, s);

    Mat4::new(
        gvec4(s.x, u.x, f.x, 0.0),
        gvec4(s.y, u.y, f.y, 0.0),
        gvec4(s.z, u.z, f.z, 0.0),
        gvec4(-dot3(s, eye), -dot3(u, eye), -dot3(f, eye), 1.0),
    )
}

/// Transpose a matrix.
#[inline]
pub fn transpose(m: &Mat4) -> Mat4 {
    glm::transpose(m)
}

/// Invert a matrix, or `None` if it is singular.
#[inline]
pub fn inverse(m: &Mat4) -> Option<Mat4> {
    glm::GenSquareMat::inverse(m)
}

/// Extract the four columns of a matrix as `[[f32; 4]; 4]`, ready for a GPU
/// uniform buffer.
pub fn to_cols_array(m: &Mat4) -> [[f32; 4]; 4] {
    let a = m.as_array();
    [
        [a[0].x, a[0].y, a[0].z, a[0].w],
        [a[1].x, a[1].y, a[1].z, a[1].w],
        [a[2].x, a[2].y, a[2].z, a[2].w],
        [a[3].x, a[3].y, a[3].z, a[3].w],
    ]
}

/// Build a matrix from `[[f32; 4]; 4]` columns.
pub fn from_cols_array(c: [[f32; 4]; 4]) -> Mat4 {
    Mat4::new(
        gvec4(c[0][0], c[0][1], c[0][2], c[0][3]),
        gvec4(c[1][0], c[1][1], c[1][2], c[1][3]),
        gvec4(c[2][0], c[2][1], c[2][2], c[2][3]),
        gvec4(c[3][0], c[3][1], c[3][2], c[3][3]),
    )
}

/// Compose a TRS matrix and return it directly as columns.
#[inline]
pub fn trs_to_cols_array(t: Vec3, r: Quat, s: Vec3) -> [[f32; 4]; 4] {
    to_cols_array(&trs(t, r, s))
}

fn dot3(a: Vec3, b: Vec3) -> f32 {
    a.x * b.x + a.y * b.y + a.z * b.z
}

fn cross3(a: Vec3, b: Vec3) -> Vec3 {
    vec3(
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Vec4, vec3_approx_eq};

    const EPS: f32 = 1e-4;

    fn transform_point(m: &Mat4, p: Vec3) -> Vec3 {
        let v: Vec4 = *m * crate::vec4(p.x, p.y, p.z, 1.0);
        vec3(v.x / v.w, v.y / v.w, v.z / v.w)
    }

    #[test]
    fn identity_is_multiplicative_unit() {
        let i = identity();
        let m = trs(vec3(1.0, 2.0, 3.0), Quat::identity(), vec3(2.0, 2.0, 2.0));
        let a = i * m;
        assert_eq!(to_cols_array(&a), to_cols_array(&m));
    }

    #[test]
    fn translation_moves_point() {
        let m = translation(vec3(5.0, -2.0, 1.0));
        let p = transform_point(&m, vec3(0.0, 0.0, 0.0));
        assert!(vec3_approx_eq(p, vec3(5.0, -2.0, 1.0), EPS));
    }

    #[test]
    fn scaling_scales_point() {
        let m = scaling(vec3(2.0, 3.0, 4.0));
        let p = transform_point(&m, vec3(1.0, 1.0, 1.0));
        assert!(vec3_approx_eq(p, vec3(2.0, 3.0, 4.0), EPS));
    }

    #[test]
    fn rotation_90_about_z_maps_x_to_y() {
        let m = rotation_z(std::f32::consts::FRAC_PI_2);
        let p = transform_point(&m, vec3(1.0, 0.0, 0.0));
        assert!(vec3_approx_eq(p, vec3(0.0, 1.0, 0.0), EPS), "got {p:?}");
    }

    #[test]
    fn quat_and_matrix_rotation_agree() {
        let q = Quat::from_euler_xyz(0.4, -0.7, 1.3);
        let m = from_quat(q);
        let v = vec3(1.0, 2.0, -3.0);
        let by_matrix = transform_point(&m, v);
        let by_quat = q.rotate_vec3(v);
        assert!(
            vec3_approx_eq(by_matrix, by_quat, 1e-4),
            "{by_matrix:?} vs {by_quat:?}"
        );
    }

    #[test]
    fn trs_applies_scale_then_rotate_then_translate() {
        let m = trs(
            vec3(10.0, 0.0, 0.0),
            Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            vec3(2.0, 2.0, 2.0),
        );
        // Local (1,0,0) -> scale (2,0,0) -> rotate 90z (0,2,0) -> translate (10,2,0)
        let p = transform_point(&m, vec3(1.0, 0.0, 0.0));
        assert!(vec3_approx_eq(p, vec3(10.0, 2.0, 0.0), EPS), "got {p:?}");
    }

    #[test]
    fn perspective_maps_near_to_zero_and_far_to_one() {
        let proj = perspective(std::f32::consts::FRAC_PI_2, 1.0, 0.1, 100.0);
        let near_pt = transform_point(&proj, vec3(0.0, 0.0, -0.1));
        let far_pt = transform_point(&proj, vec3(0.0, 0.0, -100.0));
        assert!((near_pt.z - 0.0).abs() < 1e-3, "near z = {}", near_pt.z);
        assert!((far_pt.z - 1.0).abs() < 1e-3, "far z = {}", far_pt.z);
    }

    #[test]
    fn look_at_places_eye_at_origin() {
        let view = look_at_rh(
            vec3(0.0, 0.0, 5.0),
            vec3(0.0, 0.0, 0.0),
            vec3(0.0, 1.0, 0.0),
        );
        let p = transform_point(&view, vec3(0.0, 0.0, 5.0));
        assert!(vec3_approx_eq(p, vec3(0.0, 0.0, 0.0), EPS), "got {p:?}");
        // A point in front of the camera is at negative z in RH view space.
        let front = transform_point(&view, vec3(0.0, 0.0, 0.0));
        assert!(front.z < 0.0, "front z = {}", front.z);
    }

    #[test]
    fn inverse_round_trips() {
        let m = trs(
            vec3(1.0, 2.0, 3.0),
            Quat::from_rotation_y(0.8),
            vec3(2.0, 0.5, 1.5),
        );
        let inv = inverse(&m).expect("invertible");
        let prod = m * inv;
        let cols = to_cols_array(&prod);
        let id = to_cols_array(&identity());
        for i in 0..4 {
            for j in 0..4 {
                assert!((cols[i][j] - id[i][j]).abs() < 1e-3, "cell {i},{j}");
            }
        }
    }

    #[test]
    fn cols_array_round_trip() {
        let m = trs(
            vec3(-1.0, 4.0, 0.5),
            Quat::from_euler_xyz(0.1, 0.2, 0.3),
            vec3(1.0, 2.0, 3.0),
        );
        let cols = to_cols_array(&m);
        let back = from_cols_array(cols);
        assert_eq!(to_cols_array(&back), cols);
    }
}
