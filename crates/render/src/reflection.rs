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

    let direction = |v: Vec3| {
        let r = mirror * vec4(v.x, v.y, v.z, 0.0);
        math::vec3(r.x, r.y, r.z)
    };
    let eye = {
        let r = mirror * vec4(view.eye.x, view.eye.y, view.eye.z, 1.0);
        math::vec3(r.x, r.y, r.z)
    };

    CameraView {
        eye,
        forward: direction(view.forward),
        up: direction(view.up),
        right: direction(view.right),
        view: view_matrix,
        projection,
        view_projection,
        inverse_view_projection: math::inverse(&view_projection).unwrap_or_else(math::identity),
        near: view.near,
        far: view.far,
        aspect: view.aspect,
    }
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
        let r = *m * vec4(p.x, p.y, p.z, 1.0);
        math::vec3(r.x, r.y, r.z)
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
}
