//! Pure math for the viewport transform gizmo.
//!
//! The renderer draws the gizmo from geometry centred on the origin; this module
//! turns a viewport [`Ray`] into gizmo interactions: which axis handle the cursor
//! is over, and where an axis drag has moved the entity. Everything here is a
//! pure function operating on vectors, so it is fully unit-tested without a
//! window or a GPU.
//!
//! The drag model is the standard one: the entity is moved along the axis so
//! that the point that was grabbed stays under the cursor. That only needs the
//! ray/axis closest-point parameter at grab time and now, plus the entity's
//! starting position — see [`dragged_position`].

use crate::picking::Ray;
use ecs::components::Projection;
use math::{Mat4, Vec3, vec3};
pub use render::gizmo::axis_vector;
use render::{CameraView, GizmoAxis};

/// The three gizmo axes, in picking-priority order.
pub const AXES: [GizmoAxis; 3] = [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z];

/// The signed distance along the axis line through `origin` of the point
/// closest to `ray`.
///
/// This is the closest-point parameter `s` in `origin + s * normalize(axis)`.
/// Returns `None` when the ray is (nearly) parallel to the axis, where the
/// closest point is ill-defined.
pub fn axis_parameter(ray: &Ray, origin: Vec3, axis: Vec3) -> Option<f32> {
    let a = math::normalize(axis);
    let b = ray.direction;
    let w0 = origin - ray.origin;
    let ab = math::dot(a, b);
    let denom = 1.0 - ab * ab;
    if denom < 1e-6 {
        return None;
    }
    Some((math::dot(w0, b) * ab - math::dot(w0, a)) / denom)
}

/// The shortest distance from `ray` to the axis segment
/// `origin..origin + normalize(axis) * length`. `None` when the ray is parallel
/// to the axis.
pub fn axis_segment_distance(ray: &Ray, origin: Vec3, axis: Vec3, length: f32) -> Option<f32> {
    let parameter = axis_parameter(ray, origin, axis)?;
    let on_segment = origin + math::normalize(axis) * parameter.clamp(0.0, length);
    let offset = on_segment - ray.origin;
    let on_ray = ray.origin + ray.direction * math::dot(offset, ray.direction);
    Some(math::length(on_segment - on_ray))
}

/// The axis handle the ray passes within `threshold` world units of, choosing
/// the nearest. `None` if the ray misses every handle.
pub fn pick_axis(ray: &Ray, origin: Vec3, length: f32, threshold: f32) -> Option<GizmoAxis> {
    let mut best: Option<(f32, GizmoAxis)> = None;
    for axis in AXES {
        let Some(distance) = axis_segment_distance(ray, origin, axis_vector(axis), length) else {
            continue;
        };
        if distance <= threshold && best.is_none_or(|(closest, _)| distance < closest) {
            best = Some((distance, axis));
        }
    }
    best.map(|(_, axis)| axis)
}

/// Where an axis drag has moved `start`: `start` shifted along `axis` by the
/// change in closest-point parameter since the grab.
pub fn dragged_position(start: Vec3, axis: Vec3, grab: f32, current: f32) -> Vec3 {
    start + math::normalize(axis) * (current - grab)
}

/// Convert a world-space position to an entity's local translation.
///
/// `parent` is the parent's world matrix, or `None` for a root entity (whose
/// local space *is* world space).
pub fn local_from_world(parent: Option<&Mat4>, world: Vec3) -> Vec3 {
    let Some(parent) = parent else {
        return world;
    };
    let inverse = math::inverse(parent).unwrap_or_else(math::identity);
    let p = inverse * math::vec4(world.x, world.y, world.z, 1.0);
    vec3(p.x, p.y, p.z)
}

/// A gizmo shaft length that keeps the handles a roughly constant `fraction` of
/// the viewport height, so the gizmo is equally grabbable near and far.
pub fn gizmo_length(
    camera: &CameraView,
    projection: &Projection,
    origin: Vec3,
    fraction: f32,
) -> f32 {
    let distance = math::length(origin - camera.eye).max(1e-3);
    let span = match *projection {
        Projection::Perspective { fov_y, .. } => 2.0 * distance * (fov_y * 0.5).tan(),
        Projection::Orthographic { height, .. } => height,
    };
    (span * fraction).max(1e-3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ecs::components::Camera;
    use math::vec3;
    use render::camera::camera_view;

    fn ray(origin: Vec3, direction: Vec3) -> Ray {
        Ray::new(origin, direction)
    }

    #[test]
    fn axis_parameter_locates_the_closest_point_on_the_axis() {
        // A ray parallel to -Z at x = 3 crosses the X axis at parameter 3.
        let r = ray(vec3(3.0, 0.0, 5.0), vec3(0.0, 0.0, -1.0));
        let s = axis_parameter(&r, vec3(0.0, 0.0, 0.0), axis_vector(GizmoAxis::X)).unwrap();
        assert!((s - 3.0).abs() < 1e-4, "s = {s}");
    }

    #[test]
    fn axis_parameter_is_zero_for_a_ray_through_the_origin() {
        let r = ray(vec3(0.0, 0.0, 5.0), vec3(0.0, 0.0, -1.0));
        let s = axis_parameter(&r, vec3(0.0, 0.0, 0.0), axis_vector(GizmoAxis::X)).unwrap();
        assert!(s.abs() < 1e-4, "s = {s}");
    }

    #[test]
    fn axis_parameter_rejects_parallel_rays() {
        // Looking straight down the Z axis: the Z handle is un-grabbable.
        let r = ray(vec3(0.0, 0.0, 5.0), vec3(0.0, 0.0, -1.0));
        assert_eq!(
            axis_parameter(&r, vec3(0.0, 0.0, 0.0), axis_vector(GizmoAxis::Z)),
            None
        );
    }

    #[test]
    fn axis_distance_clamps_to_the_segment() {
        // The closest point on the infinite X axis is at s = 5, beyond the
        // length-1 segment, so the distance is measured to the tip instead.
        let r = ray(vec3(5.0, 0.0, 5.0), vec3(0.0, 0.0, -1.0));
        let d =
            axis_segment_distance(&r, vec3(0.0, 0.0, 0.0), axis_vector(GizmoAxis::X), 1.0).unwrap();
        assert!((d - 4.0).abs() < 1e-4, "d = {d}");
    }

    #[test]
    fn pick_axis_selects_the_handle_under_the_cursor() {
        // A ray aimed straight at the +X tip.
        let r = ray(vec3(1.0, 0.0, 5.0), vec3(0.0, 0.0, -1.0));
        assert_eq!(
            pick_axis(&r, vec3(0.0, 0.0, 0.0), 1.0, 0.1),
            Some(GizmoAxis::X)
        );

        // A ray far from every handle misses.
        let miss = ray(vec3(6.0, 6.0, 5.0), vec3(0.0, 0.0, -1.0));
        assert_eq!(pick_axis(&miss, vec3(0.0, 0.0, 0.0), 1.0, 0.1), None);
    }

    #[test]
    fn pick_axis_prefers_the_nearer_handle() {
        // Between the X and Y handles but closer to Y.
        let r = ray(vec3(0.1, 0.9, 5.0), vec3(0.0, 0.0, -1.0));
        assert_eq!(
            pick_axis(&r, vec3(0.0, 0.0, 0.0), 1.0, 0.5),
            Some(GizmoAxis::Y)
        );
    }

    #[test]
    fn dragged_position_moves_along_the_axis_by_the_parameter_delta() {
        let moved = dragged_position(vec3(1.0, 2.0, 3.0), axis_vector(GizmoAxis::X), 0.5, 2.5);
        assert!(math::vec3_approx_eq(moved, vec3(3.0, 2.0, 3.0), 1e-5));
    }

    #[test]
    fn drag_is_a_no_op_when_the_parameter_does_not_change() {
        let start = vec3(4.0, 0.0, -1.0);
        let moved = dragged_position(start, axis_vector(GizmoAxis::Z), 2.0, 2.0);
        assert!(math::vec3_approx_eq(moved, start, 1e-5));
    }

    #[test]
    fn local_from_world_inverts_the_parent_matrix() {
        // A parent shifted +10 on X: world (12, 0, 0) is local (2, 0, 0).
        let parent = math::translation(vec3(10.0, 0.0, 0.0));
        let local = local_from_world(Some(&parent), vec3(12.0, 0.0, 0.0));
        assert!(math::vec3_approx_eq(local, vec3(2.0, 0.0, 0.0), 1e-4));

        // A root entity keeps world coordinates unchanged.
        assert_eq!(
            local_from_world(None, vec3(7.0, 8.0, 9.0)),
            vec3(7.0, 8.0, 9.0)
        );
    }

    #[test]
    fn gizmo_length_grows_with_distance() {
        let camera = Camera::default();
        let view = camera_view(&camera, &math::translation(vec3(0.0, 0.0, 0.0)), 1280, 720);
        let near = gizmo_length(&view, &camera.projection, vec3(0.0, 0.0, -10.0), 0.15);
        let far = gizmo_length(&view, &camera.projection, vec3(0.0, 0.0, -40.0), 0.15);
        assert!(far > near, "far {far} should exceed near {near}");
        // 2 * 10 * tan(30 deg) * 0.15 ~= 1.732 at distance 10.
        assert!((near - 1.732).abs() < 0.05, "near = {near}");
    }

    #[test]
    fn orthographic_gizmo_length_ignores_distance() {
        let camera = Camera {
            projection: Projection::Orthographic {
                height: 20.0,
                near: 0.1,
                far: 100.0,
            },
            ..Camera::default()
        };
        let view = camera_view(&camera, &math::translation(vec3(0.0, 0.0, 0.0)), 1280, 720);
        let length = gizmo_length(&view, &camera.projection, vec3(0.0, 0.0, -5.0), 0.1);
        assert!((length - 2.0).abs() < 1e-4, "length = {length}");
    }
}
