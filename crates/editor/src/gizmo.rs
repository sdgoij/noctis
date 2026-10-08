//! Pure math for the viewport transform gizmo.
//!
//! The renderer draws the gizmo from geometry centred on the origin; this module
//! turns a viewport [`Ray`] into gizmo interactions: which handle the cursor is
//! over, and where a drag has moved the entity. Everything here is a pure
//! function operating on vectors and matrices, so it is fully unit-tested
//! without a window or a GPU.
//!
//! Three handle families, three drag models:
//!
//! * **Translate** — the entity moves along an axis so the grabbed point stays
//!   under the cursor. See [`dragged_position`].
//! * **Rotate** — the entity spins about an axis by the change in the cursor's
//!   angle around that axis. See [`ring_angle`].
//! * **Scale** — the entity's local scale on an axis is multiplied by the change
//!   in the cursor's distance along that axis.
//!
//! Translate and rotate handles are world-oriented; scale handles follow the
//! entity's own frame (see [`rotation_of`]), because scaling along an arbitrary
//! world axis of a rotated object is not representable without shear.

use crate::picking::Ray;
use ecs::components::Projection;
use math::{Mat4, Vec3, vec3};
pub use render::gizmo::{axis_vector, basis_for_axis};
use render::{CameraView, GizmoAxis};

/// The three gizmo axes, in picking-priority order.
pub const AXES: [GizmoAxis; 3] = [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z];

/// Whether the gizmo's handles follow the world axes or the entity's own frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GizmoOrientation {
    /// Handles are aligned to the world axes.
    #[default]
    World,
    /// Handles follow the entity's own frame.
    Local,
}

impl GizmoOrientation {
    /// The human-readable label.
    pub fn label(self) -> &'static str {
        match self {
            GizmoOrientation::World => "World",
            GizmoOrientation::Local => "Local",
        }
    }
}

/// The index of `axis` within [`AXES`].
pub fn axis_index(axis: GizmoAxis) -> usize {
    match axis {
        GizmoAxis::X => 0,
        GizmoAxis::Y => 1,
        GizmoAxis::Z => 2,
    }
}

/// The three world-space axis directions, in [`AXES`] order.
pub fn world_axes() -> [Vec3; 3] {
    [
        axis_vector(GizmoAxis::X),
        axis_vector(GizmoAxis::Y),
        axis_vector(GizmoAxis::Z),
    ]
}

fn normalize_or(v: Vec3, fallback: Vec3) -> Vec3 {
    let len_sq = v.x * v.x + v.y * v.y + v.z * v.z;
    if len_sq > 1e-20 {
        let inv = len_sq.sqrt().recip();
        vec3(v.x * inv, v.y * inv, v.z * inv)
    } else {
        fallback
    }
}

/// The rotation part of a world matrix: its first three columns normalized to
/// unit length.
///
/// A TRS composition has no shear, so normalizing the columns yields an
/// orthonormal rotation even when the source has non-uniform scale. The result
/// is a pure rotation (zero translation), suitable for orienting the scale
/// gizmo and for mapping axes between world and the entity's frame.
pub fn rotation_of(world: &Mat4) -> Mat4 {
    let column = |index: usize| {
        let c = world[index];
        normalize_or(vec3(c.x, c.y, c.z), axis_vector(AXES[index.min(2)]))
    };
    let (x, y, z) = (column(0), column(1), column(2));
    Mat4::new(
        math::vec4(x.x, x.y, x.z, 0.0),
        math::vec4(y.x, y.y, y.z, 0.0),
        math::vec4(z.x, z.y, z.z, 0.0),
        math::vec4(0.0, 0.0, 0.0, 1.0),
    )
}

/// Express a world-space axis in a frame's local coordinates, i.e. apply the
/// inverse of an orthonormal rotation (see [`rotation_of`]).
pub fn axis_into_frame(frame: &Mat4, axis: Vec3) -> Vec3 {
    let v = math::transpose(frame) * math::vec4(axis.x, axis.y, axis.z, 0.0);
    vec3(v.x, v.y, v.z)
}

/// Rotate a vector by a frame's rotation (see [`rotation_of`]), the inverse of
/// [`axis_into_frame`].
pub fn rotate_axis(frame: &Mat4, axis: Vec3) -> Vec3 {
    let v = *frame * math::vec4(axis.x, axis.y, axis.z, 0.0);
    vec3(v.x, v.y, v.z)
}

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
/// the nearest. `dirs[i]` is the world direction of [`AXES`]`[i]`. `None` if the
/// ray misses every handle.
pub fn pick_axis(
    ray: &Ray,
    origin: Vec3,
    dirs: [Vec3; 3],
    length: f32,
    threshold: f32,
) -> Option<GizmoAxis> {
    let mut best: Option<(f32, GizmoAxis)> = None;
    for (index, dir) in dirs.iter().enumerate() {
        let Some(distance) = axis_segment_distance(ray, origin, *dir, length) else {
            continue;
        };
        if distance <= threshold && best.is_none_or(|(closest, _)| distance < closest) {
            best = Some((distance, AXES[index]));
        }
    }
    best.map(|(_, axis)| axis)
}

/// Where an axis drag has moved `start`: `start` shifted along `axis` by the
/// change in closest-point parameter since the grab.
pub fn dragged_position(start: Vec3, axis: Vec3, grab: f32, current: f32) -> Vec3 {
    start + math::normalize(axis) * (current - grab)
}

/// Intersect `ray` with the plane through `origin` with normal `normal`,
/// returning the distance along the ray. `None` when the ray is parallel to the
/// plane.
pub fn plane_parameter(ray: &Ray, origin: Vec3, normal: Vec3) -> Option<f32> {
    let denom = math::dot(ray.direction, normal);
    if denom.abs() < 1e-6 {
        return None;
    }
    Some(math::dot(origin - ray.origin, normal) / denom)
}

/// How far the ray's intersection with the ring plane lands from the ring of
/// `radius`, or `None` when the ray misses the plane or crosses it behind its
/// origin.
pub fn ring_distance(ray: &Ray, origin: Vec3, normal: Vec3, radius: f32) -> Option<f32> {
    let t = plane_parameter(ray, origin, normal)?;
    if t <= 0.0 {
        return None;
    }
    let p = ray.at(t) - origin;
    Some((math::length(p) - radius).abs())
}

/// The rotation handle the ray passes within `threshold` world units of,
/// choosing the nearest. `normals[i]` is the axis of the ring drawn for
/// [`AXES`]`[i]`.
pub fn pick_ring(
    ray: &Ray,
    origin: Vec3,
    normals: [Vec3; 3],
    radius: f32,
    threshold: f32,
) -> Option<GizmoAxis> {
    let mut best: Option<(f32, GizmoAxis)> = None;
    for (index, normal) in normals.iter().enumerate() {
        let Some(distance) = ring_distance(ray, origin, *normal, radius) else {
            continue;
        };
        if distance <= threshold && best.is_none_or(|(closest, _)| distance < closest) {
            best = Some((distance, AXES[index]));
        }
    }
    best.map(|(_, axis)| axis)
}

/// The angle (radians) of the ray's intersection with the ring plane around
/// `origin`, measured in the plane basis `(u, v)` as `atan2(p·v, p·u)`. `None`
/// when the ray is parallel to the plane or crosses it behind its origin.
pub fn ring_angle(ray: &Ray, origin: Vec3, normal: Vec3, u: Vec3, v: Vec3) -> Option<f32> {
    let t = plane_parameter(ray, origin, normal)?;
    if t <= 0.0 {
        return None;
    }
    let p = ray.at(t) - origin;
    Some(f32::atan2(math::dot(p, v), math::dot(p, u)))
}

/// Wrap an angle to `(-pi, pi]`.
pub fn wrap_angle(angle: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let mut wrapped = angle % TAU;
    if wrapped > PI {
        wrapped -= TAU;
    } else if wrapped <= -PI {
        wrapped += TAU;
    }
    wrapped
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
    use math::{Quat, vec3};
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
            pick_axis(&r, vec3(0.0, 0.0, 0.0), world_axes(), 1.0, 0.1),
            Some(GizmoAxis::X)
        );

        // A ray far from every handle misses.
        let miss = ray(vec3(6.0, 6.0, 5.0), vec3(0.0, 0.0, -1.0));
        assert_eq!(
            pick_axis(&miss, vec3(0.0, 0.0, 0.0), world_axes(), 1.0, 0.1),
            None
        );
    }

    #[test]
    fn pick_axis_prefers_the_nearer_handle() {
        // Between the X and Y handles but closer to Y.
        let r = ray(vec3(0.1, 0.9, 5.0), vec3(0.0, 0.0, -1.0));
        assert_eq!(
            pick_axis(&r, vec3(0.0, 0.0, 0.0), world_axes(), 1.0, 0.5),
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
    fn rotation_of_strips_scale_and_matches_the_quaternion() {
        let q = Quat::from_rotation_y(0.9);
        let world = math::trs(vec3(3.0, -2.0, 5.0), q, vec3(2.0, 3.0, 4.0));
        let rotation = rotation_of(&world);
        let v = vec3(1.0, 2.0, -3.0);
        let by_matrix = rotation * math::vec4(v.x, v.y, v.z, 0.0);
        let by_quat = q.rotate_vec3(v);
        assert!(
            math::vec3_approx_eq(vec3(by_matrix.x, by_matrix.y, by_matrix.z), by_quat, 1e-4),
            "got {by_matrix:?} want {by_quat:?}"
        );
    }

    #[test]
    fn axis_into_frame_undoes_the_frame_rotation() {
        let q = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let frame = math::from_quat(q);
        let world_axis = q.rotate_vec3(vec3(1.0, 0.0, 0.0));
        let local = axis_into_frame(&frame, world_axis);
        assert!(math::vec3_approx_eq(local, vec3(1.0, 0.0, 0.0), 1e-4));
    }

    #[test]
    fn rotate_axis_inverts_axis_into_frame() {
        let q = Quat::from_rotation_y(0.8);
        let frame = math::from_quat(q);
        let axis = vec3(0.2, 0.9, -0.3);
        let local = axis_into_frame(&frame, axis);
        let round = rotate_axis(&frame, local);
        assert!(math::vec3_approx_eq(round, axis, 1e-4), "got {round:?}");
    }

    #[test]
    fn a_parented_rotation_about_a_world_axis_composes_correctly() {
        // A child under a rotated parent: rotating the child about a world axis
        // means building the delta about that axis expressed in the parent's
        // frame, then pre-multiplying the child's local rotation. Composing the
        // result with the parent must equal rotating the original world rotation
        // about the world axis directly.
        let parent_rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let parent = math::from_quat(parent_rotation);
        let world_axis = vec3(1.0, 0.0, 0.0);
        let theta = 0.6;

        let start_local = Quat::from_rotation_z(0.3);
        let local_axis = axis_into_frame(&parent, world_axis);
        let new_local = Quat::from_axis_angle(local_axis, theta) * start_local;

        let world_before = parent_rotation * start_local;
        let world_after = parent_rotation * new_local;
        let expected = Quat::from_axis_angle(world_axis, theta) * world_before;

        let probe = vec3(0.3, -0.7, 0.5);
        assert!(math::vec3_approx_eq(
            world_after.rotate_vec3(probe),
            expected.rotate_vec3(probe),
            1e-4
        ));
    }

    #[test]
    fn ring_distance_is_zero_on_the_ring() {
        // Ring of radius 1 in the XY plane; a ray down -Z through (1, 0, *) hits
        // it exactly.
        let r = ray(vec3(1.0, 0.0, 5.0), vec3(0.0, 0.0, -1.0));
        let d = ring_distance(&r, vec3(0.0, 0.0, 0.0), vec3(0.0, 0.0, 1.0), 1.0).unwrap();
        assert!(d < 1e-4, "d = {d}");

        // A ray through (2, 0, *) is 1 unit outside the ring.
        let outside = ray(vec3(2.0, 0.0, 5.0), vec3(0.0, 0.0, -1.0));
        let d = ring_distance(&outside, vec3(0.0, 0.0, 0.0), vec3(0.0, 0.0, 1.0), 1.0).unwrap();
        assert!((d - 1.0).abs() < 1e-4, "d = {d}");
    }

    #[test]
    fn pick_ring_selects_the_ring_under_the_cursor() {
        // Only the Z ring lies in the XY plane the ray crosses.
        let r = ray(vec3(1.0, 0.0, 5.0), vec3(0.0, 0.0, -1.0));
        assert_eq!(
            pick_ring(&r, vec3(0.0, 0.0, 0.0), world_axes(), 1.0, 0.1),
            Some(GizmoAxis::Z)
        );

        // The same ray, one unit out, misses every ring.
        let miss = ray(vec3(3.0, 0.0, 5.0), vec3(0.0, 0.0, -1.0));
        assert_eq!(
            pick_ring(&miss, vec3(0.0, 0.0, 0.0), world_axes(), 1.0, 0.1),
            None
        );
    }

    #[test]
    fn ring_angle_measures_around_the_plane_basis() {
        let (u, v) = basis_for_axis(GizmoAxis::Z);
        let angle = 0.7_f32;
        let r = ray(vec3(angle.cos(), angle.sin(), 5.0), vec3(0.0, 0.0, -1.0));
        let measured =
            ring_angle(&r, vec3(0.0, 0.0, 0.0), axis_vector(GizmoAxis::Z), u, v).unwrap();
        assert!((measured - angle).abs() < 1e-4, "measured = {measured}");
    }

    #[test]
    fn wrap_angle_folds_into_pi() {
        use std::f32::consts::PI;
        assert!((wrap_angle(0.5) - 0.5).abs() < 1e-6);
        assert!((wrap_angle(1.5 * PI) + 0.5 * PI).abs() < 1e-6);
        assert!((wrap_angle(-1.5 * PI) - 0.5 * PI).abs() < 1e-6);
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
