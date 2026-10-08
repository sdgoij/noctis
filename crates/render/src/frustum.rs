//! View-frustum extraction and AABB culling.
//!
//! The frustum is derived directly from a clip-space *view-projection* matrix
//! using the Gribb–Hartmann method (the "fast extraction of view frustum planes"
//! technique). wgpu uses a **zero-to-one** clip depth range, so the planes are:
//!
//! ```text
//! left   = row3 + row0     (x >= -w)
//! right  = row3 - row0     (x <=  w)
//! bottom = row3 + row1     (y >= -w)
//! top    = row3 - row1     (y <=  w)
//! near   = row2            (z >=  0)
//! far    = row3 - row2     (z <=  w)
//! ```
//!
//! Here `rowN` is the `N`-th *row* of the column-major matrix. Each plane is
//! normalised so its `xyz` part is unit length, which lets the AABB test use a
//! cheap "positive vertex" bound (the support point of the box along the plane
//! normal). A box is rejected as soon as its maximum signed distance to any
//! plane is negative.

use math::{Aabb, Mat4, Vec3, vec3};

/// A plane in Hessian normal form: `dot(normal, p) + d = 0`.
///
/// A point is *inside* (on the positive side of) the plane when
/// `dot(normal, p) + d >= 0`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    /// The unit-length plane normal.
    pub normal: Vec3,
    /// The plane offset.
    pub d: f32,
}

impl Plane {
    /// Build a plane from an unnormalised `(a, b, c, d)` coefficient vector.
    ///
    /// A degenerate (zero-length normal) plane is normalised to `(0, 0, 1, 0)`,
    /// which never culls anything meaningful but keeps the arithmetic finite.
    pub fn from_coefficients(a: f32, b: f32, c: f32, d: f32) -> Plane {
        let len_sq = a * a + b * b + c * c;
        // A tiny but valid normal is common when the near/far ratio is large
        // (the far plane normal scales like `1/far`), so the degeneracy cutoff
        // must be far below `f32::EPSILON`.
        if len_sq <= 1e-24 {
            return Plane {
                normal: vec3(0.0, 0.0, 1.0),
                d: 0.0,
            };
        }
        let inv = len_sq.sqrt().recip();
        Plane {
            normal: vec3(a * inv, b * inv, c * inv),
            d: d * inv,
        }
    }

    /// The signed distance from `point` to the plane; positive means inside.
    #[inline]
    pub fn signed_distance(&self, point: Vec3) -> f32 {
        self.normal.x * point.x + self.normal.y * point.y + self.normal.z * point.z + self.d
    }
}

/// The six planes of a view frustum, in the order left, right, bottom, top,
/// near, far.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frustum {
    /// The six clipping planes.
    pub planes: [Plane; 6],
}

impl Frustum {
    /// Extract the frustum from a clip-space view-projection matrix.
    pub fn from_view_projection(m: &Mat4) -> Frustum {
        let row = |r: usize| -> [f32; 4] {
            let c0 = m[0];
            let c1 = m[1];
            let c2 = m[2];
            let c3 = m[3];
            let cols = [
                [c0.x, c0.y, c0.z, c0.w],
                [c1.x, c1.y, c1.z, c1.w],
                [c2.x, c2.y, c2.z, c2.w],
                [c3.x, c3.y, c3.z, c3.w],
            ];
            [cols[0][r], cols[1][r], cols[2][r], cols[3][r]]
        };

        let r0 = row(0);
        let r1 = row(1);
        let r2 = row(2);
        let r3 = row(3);

        let add = |a: [f32; 4], b: [f32; 4]| [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]];
        let sub = |a: [f32; 4], b: [f32; 4]| [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]];

        let left = add(r3, r0);
        let right = sub(r3, r0);
        let bottom = add(r3, r1);
        let top = sub(r3, r1);
        let near = r2;
        let far = sub(r3, r2);

        let to_plane = |c: [f32; 4]| Plane::from_coefficients(c[0], c[1], c[2], c[3]);

        Frustum {
            planes: [
                to_plane(left),
                to_plane(right),
                to_plane(bottom),
                to_plane(top),
                to_plane(near),
                to_plane(far),
            ],
        }
    }

    /// `true` if the axis-aligned box is at least partially inside the frustum.
    ///
    /// The test uses the box's support point along each plane normal, so it is
    /// conservative (it never rejects a box that actually intersects the
    /// frustum) but fast.
    pub fn intersects_aabb(&self, aabb: &Aabb) -> bool {
        if aabb.is_empty() {
            return false;
        }
        let center = aabb.center();
        let half = aabb.half_extents();
        for plane in &self.planes {
            let n = plane.normal;
            let radius = n.x.abs() * half.x + n.y.abs() * half.y + n.z.abs() * half.z;
            let distance = plane.signed_distance(center);
            if distance + radius < 0.0 {
                return false;
            }
        }
        true
    }

    /// The signed distance from a point to the nearest plane (negative when the
    /// point is outside the frustum).
    pub fn distance_to_point(&self, point: Vec3) -> f32 {
        self.planes
            .iter()
            .map(|p| p.signed_distance(point))
            .fold(f32::INFINITY, f32::min)
    }
}

/// Transform an object-space [`Aabb`] into world space by transforming its eight
/// corners and taking the tightest axis-aligned box around them.
pub fn transform_aabb(matrix: &Mat4, aabb: &Aabb) -> Aabb {
    let mut min = vec3(f32::INFINITY, f32::INFINITY, f32::INFINITY);
    let mut max = vec3(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for corner in aabb.corners() {
        let v = *matrix * math::vec4(corner.x, corner.y, corner.z, 1.0);
        let w = if v.w.abs() > f32::EPSILON { v.w } else { 1.0 };
        let p = vec3(v.x / w, v.y / w, v.z / w);
        min = vec3(min.x.min(p.x), min.y.min(p.y), min.z.min(p.z));
        max = vec3(max.x.max(p.x), max.y.max(p.y), max.z.max(p.z));
    }
    Aabb { min, max }
}

#[cfg(test)]
mod tests {
    use super::*;
    use math::{perspective, radians};
    use std::f32::consts::FRAC_PI_2;

    fn camera_frustum() -> Frustum {
        // Camera at origin looking down -Z; near 0.1, far 100.
        let proj = perspective(radians(60.0), 1.0, 0.1, 100.0);
        Frustum::from_view_projection(&proj)
    }

    #[test]
    fn planes_are_unit_length() {
        let f = camera_frustum();
        for plane in &f.planes {
            let len = (plane.normal.x * plane.normal.x
                + plane.normal.y * plane.normal.y
                + plane.normal.z * plane.normal.z)
                .sqrt();
            assert!((len - 1.0).abs() < 1e-4, "plane normal length {len}");
        }
    }

    #[test]
    fn point_in_front_is_inside() {
        let f = camera_frustum();
        assert!(f.distance_to_point(vec3(0.0, 0.0, -5.0)) > 0.0);
    }

    #[test]
    fn point_behind_camera_is_outside() {
        let f = camera_frustum();
        // z = +5 is behind a camera looking down -Z.
        assert!(f.distance_to_point(vec3(0.0, 0.0, 5.0)) < 0.0);
    }

    #[test]
    fn aabb_inside_is_visible() {
        let f = camera_frustum();
        let b = Aabb::from_center_half_extents(vec3(0.0, 0.0, -5.0), vec3(0.5, 0.5, 0.5));
        assert!(f.intersects_aabb(&b));
    }

    #[test]
    fn aabb_behind_is_culled() {
        let f = camera_frustum();
        let b = Aabb::from_center_half_extents(vec3(0.0, 0.0, 20.0), vec3(1.0, 1.0, 1.0));
        assert!(!f.intersects_aabb(&b));
    }

    #[test]
    fn aabb_beyond_far_is_culled() {
        let f = camera_frustum();
        let b = Aabb::from_center_half_extents(vec3(0.0, 0.0, -500.0), vec3(1.0, 1.0, 1.0));
        assert!(!f.intersects_aabb(&b));
    }

    #[test]
    fn aabb_straddling_near_plane_is_visible() {
        let f = camera_frustum();
        // Half the box is behind the near plane, half in front; still visible.
        let b = Aabb::from_center_half_extents(vec3(0.0, 0.0, 0.0), vec3(0.2, 0.2, 0.2));
        assert!(f.intersects_aabb(&b));
    }

    #[test]
    fn aabb_far_off_to_the_side_is_culled() {
        let f = camera_frustum();
        let b = Aabb::from_center_half_extents(vec3(100.0, 0.0, -5.0), vec3(0.5, 0.5, 0.5));
        assert!(!f.intersects_aabb(&b));
    }

    #[test]
    fn empty_aabb_is_culled() {
        let f = camera_frustum();
        // Build an empty box directly: `Aabb::new` would order the corners.
        let empty = Aabb {
            min: vec3(1.0, 1.0, 1.0),
            max: vec3(-1.0, -1.0, -1.0),
        };
        assert!(empty.is_empty());
        assert!(!f.intersects_aabb(&empty));
    }

    #[test]
    fn transform_aabb_grows_for_rotation() {
        let local = Aabb::from_center_half_extents(vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0));
        let rotated = math::trs(
            vec3(5.0, 0.0, 0.0),
            math::Quat::from_rotation_z(FRAC_PI_2),
            vec3(1.0, 1.0, 1.0),
        );
        let world = transform_aabb(&rotated, &local);
        // Center moves to (5,0,0) and the extent is preserved under a 90 deg spin.
        assert!(math::vec3_approx_eq(
            world.center(),
            vec3(5.0, 0.0, 0.0),
            1e-4
        ));
        assert!(math::vec3_approx_eq(
            world.size(),
            vec3(2.0, 2.0, 2.0),
            1e-4
        ));
    }

    #[test]
    fn transform_aabb_scales_extents() {
        let local = Aabb::from_center_half_extents(vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0));
        let scaled = math::trs(
            vec3(0.0, 0.0, 0.0),
            math::Quat::identity(),
            vec3(2.0, 3.0, 4.0),
        );
        let world = transform_aabb(&scaled, &local);
        assert!(math::vec3_approx_eq(
            world.size(),
            vec3(4.0, 6.0, 8.0),
            1e-4
        ));
    }
}
