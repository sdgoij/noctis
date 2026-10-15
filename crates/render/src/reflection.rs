//! The planar-reflection maths: mirroring a view about a plane, which is what the
//! engine's mirror pass renders from (`.notes/reflections.md`).
//!
//! Pure functions — no GPU state — so the mirror matrix and the mirrored camera
//! are unit-tested with no adapter. The pass itself (an offscreen target, the
//! oblique clip and the seam's fourth bind group) is built on these.

use math::{Mat4, Vec3, vec4};

use crate::camera::CameraView;

/// The 4×4 reflection about the plane through `point` with unit `normal`: a point
/// `p` maps to `p - 2 * dot(normal, p - point) * normal`.
///
/// The plane's own points are fixed; directions (and a camera's basis vectors)
/// transform by its linear part. `point` is projected onto the normal's line, so
/// any point on the plane serves.
pub fn mirror_matrix(point: Vec3, normal: Vec3) -> Mat4 {
    let n = normal;
    let d = math::dot(n, point);
    Mat4::new(
        vec4(
            1.0 - 2.0 * n.x * n.x,
            -2.0 * n.x * n.y,
            -2.0 * n.x * n.z,
            0.0,
        ),
        vec4(
            -2.0 * n.x * n.y,
            1.0 - 2.0 * n.y * n.y,
            -2.0 * n.y * n.z,
            0.0,
        ),
        vec4(
            -2.0 * n.x * n.z,
            -2.0 * n.y * n.z,
            1.0 - 2.0 * n.z * n.z,
            0.0,
        ),
        vec4(2.0 * d * n.x, 2.0 * d * n.y, 2.0 * d * n.z, 1.0),
    )
}

/// Transform a point by `m` (homogeneous `w = 1`).
fn transform_point(m: &Mat4, p: Vec3) -> Vec3 {
    let r = *m * vec4(p.x, p.y, p.z, 1.0);
    math::vec3(r.x, r.y, r.z)
}

/// Transform a direction by `m`'s linear part (homogeneous `w = 0`), so the
/// translation is ignored.
fn transform_direction(m: &Mat4, v: Vec3) -> Vec3 {
    let r = *m * vec4(v.x, v.y, v.z, 0.0);
    math::vec3(r.x, r.y, r.z)
}

/// The view the surface would see: `view` mirrored about the plane.
///
/// The mirrored camera sits at the reflection of the original's eye with its
/// basis reflected, so its handedness flips — a mirrored view reverses winding,
/// which the pass accounts for by flipping the front face.
pub fn reflect_view(view: &CameraView, point: Vec3, normal: Vec3) -> CameraView {
    let mirror = mirror_matrix(point, normal);
    // The mirrored camera sees the reflected world: `view ∘ mirror`.
    let view_matrix = view.view * mirror;
    let projection = view.projection;
    let view_projection = projection * view_matrix;

    CameraView {
        eye: transform_point(&mirror, view.eye),
        forward: transform_direction(&mirror, view.forward),
        up: transform_direction(&mirror, view.up),
        right: transform_direction(&mirror, view.right),
        view: view_matrix,
        projection,
        view_projection,
        inverse_view_projection: math::inverse(&view_projection).unwrap_or_else(math::identity),
        near: view.near,
        far: view.far,
        aspect: view.aspect,
    }
}

/// A plane in **view space**: the set of points `p` with
/// `dot(normal, p) + offset == 0`.
///
/// `normal` points toward the *kept* half-space (`dot(normal, p) + offset > 0`),
/// which for the mirror pass is the side of the surface the reflection is drawn
/// from — the world above the water, seen by a camera that has been mirrored
/// below it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewPlane {
    /// Unit normal, toward the kept side.
    pub normal: Vec3,
    /// The plane's offset along the normal.
    pub offset: f32,
}

impl ViewPlane {
    /// The plane through `point` with normal `normal` (normalized).
    pub fn through(point: Vec3, normal: Vec3) -> Self {
        let len = math::length(normal);
        let normal = if len > 1e-9 {
            normal * (1.0 / len)
        } else {
            math::unit_y()
        };
        Self {
            normal,
            offset: -math::dot(normal, point),
        }
    }

    /// How far `p` is along the normal: positive on the kept side, negative on
    /// the clipped side, zero on the plane.
    pub fn signed_distance(&self, p: Vec3) -> f32 {
        math::dot(self.normal, p) + self.offset
    }
}

/// A world-space plane (through `point` with unit `normal`) expressed in `view`'s
/// view space, ready to be a near plane for the mirrored view.
///
/// A camera's view matrix is rigid (rotation and translation, no scale), so the
/// normal rotates by its linear part and needs no inverse-transpose.
pub fn plane_in_view(view: &CameraView, point: Vec3, normal: Vec3) -> ViewPlane {
    let n = transform_direction(&view.view, normal);
    ViewPlane {
        normal: n,
        offset: -math::dot(n, transform_point(&view.view, point)),
    }
}

/// The projection with its near plane tilted onto `plane` (view space), so the
/// mirrored pass clips the half-space behind the surface instead of a plane
/// parallel to the view.
///
/// This is Lengyel's oblique near-plane trick for a zero-to-one depth range: the
/// row that produces clip `z` is replaced by the plane itself, scaled so the far
/// plane's tilted corner still lands at `z == w`. The near plane becomes exactly
/// `plane`; the far plane's depth warps, which a reflection pass does not mind.
///
/// The returned projection divides depth by `w` as before, so a fragment with
/// `z < 0` is behind the surface and must be clipped.
pub fn oblique_near_plane(projection: &Mat4, plane: ViewPlane) -> Mat4 {
    let Some(inverse) = math::inverse(projection) else {
        return *projection;
    };

    // The plane as a row `(n, d)`: `(n, d) · (p, 1) == 0` on the plane.
    let c = vec4(plane.normal.x, plane.normal.y, plane.normal.z, plane.offset);

    // The far plane's corner the plane leans toward: the zero-to-one far plane is
    // `z == w`, so its corners are `(±1, ±1, 1, 1)` in clip space. Take the one whose
    // signs match the plane's tilt and map it back to an affine view point.
    let corner_clip = vec4(
        if c.x >= 0.0 { 1.0 } else { -1.0 },
        if c.y >= 0.0 { 1.0 } else { -1.0 },
        1.0,
        1.0,
    );
    let corner_h = inverse * corner_clip;
    let w = if corner_h.w.abs() > f32::EPSILON {
        corner_h.w
    } else {
        1.0
    };
    let corner = math::vec3(corner_h.x / w, corner_h.y / w, corner_h.z / w);

    // Scale the plane so it passes through that corner: clip `z == w` there (depth
    // one) and zero on the plane. A `distance` of about zero means the plane *is*
    // the far plane, so nothing divides cleanly and the projection is left alone.
    let distance = plane.signed_distance(corner);
    let near = -corner.z;
    if distance.abs() <= 1e-4 * near.abs().max(1.0) {
        return *projection;
    }
    let row = c * (near / distance);

    // Write the row over the one that produced clip `z` (row 2).
    let mut cols = math::to_cols_array(projection);
    cols[0][2] = row.x;
    cols[1][2] = row.y;
    cols[2][2] = row.z;
    cols[3][2] = row.w;
    math::from_cols_array(cols)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::camera_view;
    use ecs::components::Camera;
    use math::{approx_eq, unit_y, vec3, vec3_approx_eq, vec4};

    /// A flat water plane at the origin, its normal up.
    fn water() -> (Vec3, Vec3) {
        (vec3(0.0, 0.0, 0.0), unit_y())
    }

    /// A camera above the water, tilted down so its basis is not axis-aligned.
    fn camera_above() -> CameraView {
        let world = math::translation(vec3(0.0, 3.0, 5.0)) * math::rotation_x(-0.5);
        camera_view(&Camera::default(), &world, 1280, 720)
    }

    /// Transform a point by `m`.
    fn transform(m: &Mat4, p: Vec3) -> Vec3 {
        transform_point(m, p)
    }

    #[test]
    fn the_mirror_fixes_its_plane_and_flips_the_rest() {
        let (point, normal) = water();
        let mirror = mirror_matrix(point, normal);

        // A point on the plane is left where it is.
        let on = vec3(5.0, 0.0, -2.0);
        assert!(vec3_approx_eq(transform(&mirror, on), on, 1e-5));

        // A point above it lands below it, mirrored through the plane.
        assert!(vec3_approx_eq(
            transform(&mirror, vec3(1.0, 3.0, 4.0)),
            vec3(1.0, -3.0, 4.0),
            1e-5
        ));
    }

    #[test]
    fn the_mirror_works_for_a_raised_plane() {
        // A table at y = 2: a point at y = 5 mirrors to y = -1, and one on the plane
        // is fixed.
        let mirror = mirror_matrix(vec3(0.0, 2.0, 0.0), unit_y());
        assert!(approx_eq(
            transform(&mirror, vec3(0.0, 5.0, 0.0)).y,
            -1.0,
            1e-5
        ));
        assert!(approx_eq(
            transform(&mirror, vec3(3.0, 2.0, 7.0)).y,
            2.0,
            1e-5
        ));
    }

    #[test]
    fn the_mirrored_camera_reflects_its_eye() {
        let view = camera_above();
        let (point, normal) = water();
        let mirrored = reflect_view(&view, point, normal);
        assert!(
            vec3_approx_eq(
                mirrored.eye,
                vec3(view.eye.x, -view.eye.y, view.eye.z),
                1e-4
            ),
            "eye {:?} -> {:?}",
            view.eye,
            mirrored.eye
        );
        // The tilted forward points up-ish; its mirror points down the same amount.
        assert!(approx_eq(mirrored.forward.y, -view.forward.y, 1e-4));
        assert!(
            approx_eq(mirrored.forward.x, view.forward.x, 1e-4)
                && approx_eq(mirrored.forward.z, view.forward.z, 1e-4)
        );
    }

    #[test]
    fn a_point_on_the_mirror_projects_to_the_same_pixel_from_either_view() {
        // The plane is fixed and the camera is its own reflection, so a point *on*
        // the plane is seen at the same screen position from both views — the
        // property the mirror pass's own-pixel sample depends on.
        let view = camera_above();
        let (point, normal) = water();
        let mirrored = reflect_view(&view, point, normal);

        let p = vec3(2.0, 0.0, -1.0);
        let a = view.view_projection * vec4(p.x, p.y, p.z, 1.0);
        let b = mirrored.view_projection * vec4(p.x, p.y, p.z, 1.0);
        assert!(
            approx_eq(a.x / a.w, b.x / b.w, 1e-4),
            "{} vs {}",
            a.x / a.w,
            b.x / b.w
        );
        assert!(
            approx_eq(a.y / a.w, b.y / b.w, 1e-4),
            "{} vs {}",
            a.y / a.w,
            b.y / b.w
        );
        // ...and it is behind both near planes, so it is visible at all.
        assert!(a.w > 0.0 && b.w > 0.0);
    }

    /// The depth of a view-space point under `m`: clip `z / w`.
    fn depth(m: &Mat4, p: Vec3) -> f32 {
        let c = *m * vec4(p.x, p.y, p.z, 1.0);
        c.z / c.w
    }

    /// A 60-degree 16:9 projection, a `0.1..100` range.
    fn projection() -> Mat4 {
        math::perspective(60f32.to_radians(), 16.0 / 9.0, 0.1, 100.0)
    }

    /// Assert two matrices agree element-wise.
    fn assert_matrix_approx(a: &Mat4, b: &Mat4, eps: f32) {
        let (x, y) = (math::to_cols_array(a), math::to_cols_array(b));
        for (col, (xc, yc)) in x.iter().zip(y.iter()).enumerate() {
            for (row, (xv, yv)) in xc.iter().zip(yc.iter()).enumerate() {
                assert!(approx_eq(*xv, *yv, eps), "[{col}][{row}]: {xv} vs {yv}");
            }
        }
    }

    #[test]
    fn a_plane_parallel_to_the_view_is_just_a_new_near_plane() {
        // The plane through `z = -10` facing the camera's way is an ordinary near
        // plane at 10, so the oblique result must equal `perspective(.., 10, 100)`.
        let expected = math::perspective(60f32.to_radians(), 16.0 / 9.0, 10.0, 100.0);
        let plane = ViewPlane::through(vec3(0.0, 0.0, -10.0), vec3(0.0, 0.0, -1.0));
        let actual = oblique_near_plane(&projection(), plane);
        assert_matrix_approx(&actual, &expected, 1e-3);
    }

    #[test]
    fn the_oblique_near_plane_is_the_plane_and_clips_behind_it() {
        // A tilted plane: through `(0, 0, -10)`, its normal out along `-z` and up.
        let plane = ViewPlane::through(vec3(0.0, 0.0, -10.0), vec3(0.0, 0.4, -1.0));
        let m = oblique_near_plane(&projection(), plane);

        // Points on the plane land exactly on the new near plane.
        let on_plane = [
            vec3(0.0, 0.0, -10.0),
            vec3(1.0, 0.0, -10.0),
            vec3(0.0, 1.0, -9.6), // (0, 0.4, -1) · (0, 1, 0.4) == 0
        ];
        for p in on_plane {
            assert!(approx_eq(plane.signed_distance(p), 0.0, 1e-4), "{p:?}");
            assert!(approx_eq(depth(&m, p), 0.0, 1e-4), "{p:?}");
        }

        // The kept side of the plane is in front; nearer the camera is clipped.
        for p in [vec3(0.0, 0.0, -20.0), vec3(0.0, 4.0, -30.0)] {
            assert!(plane.signed_distance(p) > 0.0);
            let d = depth(&m, p);
            assert!(d > 0.0 && d <= 1.0, "{p:?} -> {d}");
        }
        for p in [vec3(0.0, 0.0, -5.0), vec3(0.0, -2.0, -8.0)] {
            assert!(plane.signed_distance(p) < 0.0, "{p:?}");
            assert!(depth(&m, p) < 0.0, "{p:?}");
        }
    }

    #[test]
    fn depth_grows_with_distance_along_the_view() {
        let plane = ViewPlane::through(vec3(0.0, 0.0, -10.0), vec3(0.0, 0.0, -1.0));
        let m = oblique_near_plane(&projection(), plane);
        let near = depth(&m, vec3(0.0, 0.0, -11.0));
        let mid = depth(&m, vec3(0.0, 0.0, -20.0));
        let far = depth(&m, vec3(0.0, 0.0, -100.0));
        assert!(near < mid && mid < far, "{near} {mid} {far}");
        assert!(approx_eq(far, 1.0, 1e-4), "far {far}");
    }

    #[test]
    fn a_degenerate_plane_or_projection_leaves_the_projection_alone() {
        // A plane at the far distance has its own corner on it, so there is no
        // receding corner to tilt toward; the projection comes back untouched.
        let far_plane = ViewPlane {
            normal: math::unit_z(),
            offset: 100.0,
        };
        assert_eq!(
            math::to_cols_array(&oblique_near_plane(&projection(), far_plane)),
            math::to_cols_array(&projection())
        );

        // A singular projection cannot be skewed at all.
        let singular = math::from_cols_array([[0.0; 4]; 4]);
        let plane = ViewPlane::through(vec3(0.0, 0.0, -10.0), math::unit_y());
        assert_eq!(
            math::to_cols_array(&oblique_near_plane(&singular, plane)),
            math::to_cols_array(&singular)
        );
    }

    #[test]
    fn a_plane_is_carried_into_view_space() {
        // The water plane, `y = 0` in world space, seen from the camera above.
        let view = camera_above();
        let plane = plane_in_view(&view, vec3(0.0, 0.0, 0.0), unit_y());
        // The camera is 3 above the water, so the water is 3 away on the kept side.
        assert!(approx_eq(
            plane.signed_distance(vec3(0.0, 0.0, 0.0)),
            3.0,
            1e-4
        ));
        // A world point on the plane still reads zero after the transform.
        let on = transform_point(&view.view, vec3(5.0, 0.0, -2.0));
        assert!(approx_eq(plane.signed_distance(on), 0.0, 1e-4));
        // And a world point above reads positive, one below negative.
        let above = transform_point(&view.view, vec3(0.0, 1.0, 0.0));
        let below = transform_point(&view.view, vec3(0.0, -1.0, 0.0));
        assert!(plane.signed_distance(above) > 0.0);
        assert!(plane.signed_distance(below) < 0.0);
    }
}
