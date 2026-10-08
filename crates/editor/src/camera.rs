//! Viewport camera navigation: remappable bindings and the orbit/pan/dolly math.
//!
//! The [`CameraControls`] bindings read the engine [`InputState`], so the fly
//! keys, the orbit button and the pan modifier can all be remapped without
//! touching the driver. The transforms themselves are pure functions of a camera
//! [`LocalTransform`] and a gesture delta, which keeps them unit-testable.

use ecs::components::LocalTransform;
use ecs::resources::{ActionMap, InputState, Key, MouseButton};
use math::{Quat, Vec3, vec3};

/// Fly forward along the view direction.
pub const CAMERA_FORWARD: &str = "camera.forward";
/// Fly backward along the view direction.
pub const CAMERA_BACK: &str = "camera.back";
/// Fly left, perpendicular to the view direction.
pub const CAMERA_LEFT: &str = "camera.left";
/// Fly right, perpendicular to the view direction.
pub const CAMERA_RIGHT: &str = "camera.right";

/// Orbit sensitivity: radians of camera rotation per logical point of drag.
const ORBIT_SPEED: f32 = 0.008;
/// Dolly sensitivity: world units moved along the view direction per scroll point.
const ZOOM_SPEED: f32 = 0.05;
/// Pan sensitivity: world units moved per point, scaled by the distance to the
/// pivot so panning feels the same at any zoom.
pub const PAN_SPEED: f32 = 0.0015;

/// A letter key, which always exists for `A`–`Z`.
fn letter(c: char) -> Key {
    Key::letter(c).expect("A-Z is a letter key")
}

/// Remappable bindings for viewport camera navigation.
///
/// Defaults follow the common editor idiom: `W`/`A`/`S`/`D` fly, the middle
/// mouse button orbits, and holding Shift with it pans. Use [`CameraControls::bind`]
/// to rebind the fly keys.
#[derive(Clone, Debug)]
pub struct CameraControls {
    actions: ActionMap,
    /// The mouse button that orbits; held with [`CameraControls::pan_modifier`]
    /// it pans instead.
    pub orbit_button: MouseButton,
    /// Held with the orbit button to pan the view rather than orbit it.
    pub pan_modifier: Key,
}

impl Default for CameraControls {
    fn default() -> Self {
        let mut actions = ActionMap::new();
        actions.bind(CAMERA_FORWARD, [letter('W')]);
        actions.bind(CAMERA_BACK, [letter('S')]);
        actions.bind(CAMERA_LEFT, [letter('A')]);
        actions.bind(CAMERA_RIGHT, [letter('D')]);
        Self {
            actions,
            orbit_button: MouseButton::Middle,
            pan_modifier: Key::ShiftLeft,
        }
    }
}

impl CameraControls {
    /// Rebind the keys of a camera action. `action` is one of the `CAMERA_*`
    /// names; any keys already bound are replaced.
    pub fn bind(&mut self, action: &str, keys: impl IntoIterator<Item = Key>) {
        self.actions.bind(action, keys);
    }

    /// The keys bound to a camera action.
    pub fn keys(&self, action: &str) -> &[Key] {
        self.actions.keys(action)
    }

    /// Fly forward/backward as `-1..=1` from the current input.
    pub(crate) fn forward_axis(&self, input: &InputState) -> f32 {
        self.actions.axis(CAMERA_BACK, CAMERA_FORWARD, input)
    }

    /// Fly left/right as `-1..=1` from the current input.
    pub(crate) fn right_axis(&self, input: &InputState) -> f32 {
        self.actions.axis(CAMERA_LEFT, CAMERA_RIGHT, input)
    }

    /// Whether the pan modifier is held.
    pub(crate) fn panning(&self, input: &InputState) -> bool {
        input.key_down(self.pan_modifier)
    }
}

/// Rotate a camera transform rigidly about `pivot` by an orbit drag of `(dx, dy)`
/// logical points.
///
/// Rotating both the camera's position and orientation by the same rotation
/// leaves any point's camera-space coordinates unchanged, so the pivot keeps its
/// screen position for the whole gesture.
pub fn orbit_transform(transform: LocalTransform, pivot: Vec3, dx: f32, dy: f32) -> LocalTransform {
    // Pitch about the camera's own right axis, then yaw about the world up axis.
    let right = transform.rotation.rotate_vec3(vec3(1.0, 0.0, 0.0));
    let pitch = Quat::from_axis_angle(right, dy * ORBIT_SPEED);
    let yaw = Quat::from_rotation_y(dx * ORBIT_SPEED);
    let turn = (yaw * pitch).normalized();

    let offset = transform.translation - pivot;
    LocalTransform {
        translation: pivot + turn.rotate_vec3(offset),
        rotation: (turn * transform.rotation).normalized(),
        scale: transform.scale,
    }
}

/// Slide a camera transform in its own right/up plane by a pan drag of `(dx, dy)`
/// logical points, at `units_per_point` world units per point.
///
/// The view moves opposite the drag, so the scene follows the cursor.
pub fn pan_transform(
    transform: LocalTransform,
    dx: f32,
    dy: f32,
    units_per_point: f32,
) -> LocalTransform {
    let right = transform.rotation.rotate_vec3(vec3(1.0, 0.0, 0.0));
    let up = transform.rotation.rotate_vec3(vec3(0.0, 1.0, 0.0));
    let delta = right * (-dx * units_per_point) + up * (dy * units_per_point);
    LocalTransform {
        translation: transform.translation + delta,
        ..transform
    }
}

/// Move a camera transform along its view direction by `scroll` points; positive
/// scroll means zoom in (dolly forward).
pub fn dolly_transform(transform: LocalTransform, scroll: f32) -> LocalTransform {
    let forward = transform.rotation.rotate_vec3(vec3(0.0, 0.0, -1.0));
    LocalTransform {
        translation: transform.translation + forward * (scroll * ZOOM_SPEED),
        ..transform
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A camera on `+Z` at distance 6, looking down its local `-Z` toward the
    /// world origin.
    fn orbiting_camera() -> LocalTransform {
        LocalTransform::from_translation(vec3(0.0, 0.0, 6.0))
    }

    /// The camera-space coordinates of a world point (what the camera sees).
    fn camera_space(t: &LocalTransform, world_point: Vec3) -> Vec3 {
        t.rotation
            .inverse()
            .rotate_vec3(world_point - t.translation)
    }

    #[test]
    fn default_bindings_are_wasd() {
        let controls = CameraControls::default();
        assert_eq!(controls.keys(CAMERA_FORWARD), [letter('W')]);
        assert_eq!(controls.keys(CAMERA_BACK), [letter('S')]);
        assert_eq!(controls.keys(CAMERA_LEFT), [letter('A')]);
        assert_eq!(controls.keys(CAMERA_RIGHT), [letter('D')]);
    }

    #[test]
    fn bindings_can_be_remapped() {
        let mut controls = CameraControls::default();
        controls.bind(CAMERA_FORWARD, [Key::ArrowUp]);

        let mut input = InputState::new();
        input.press_key(Key::ArrowUp);
        assert_eq!(controls.forward_axis(&input), 1.0);

        input.press_key(letter('W'));
        assert_eq!(controls.forward_axis(&input), 1.0, "W is no longer forward");
    }

    #[test]
    fn axes_read_opposing_keys() {
        let controls = CameraControls::default();
        let mut input = InputState::new();
        assert_eq!(controls.forward_axis(&input), 0.0);

        input.press_key(letter('W'));
        assert_eq!(controls.forward_axis(&input), 1.0);
        input.press_key(letter('S'));
        assert_eq!(controls.forward_axis(&input), 0.0, "both keys cancel");

        input.release_key(letter('W'));
        assert_eq!(controls.forward_axis(&input), -1.0);

        input.press_key(letter('D'));
        assert_eq!(controls.right_axis(&input), 1.0);
    }

    #[test]
    fn orbit_preserves_distance_and_the_pivot_on_screen() {
        let pivot = vec3(0.0, 0.0, 0.0);
        let before = orbiting_camera();
        let after = orbit_transform(before, pivot, 30.0, -20.0);

        // A rigid rotation keeps the camera at the same distance from the pivot.
        let d0 = math::length(before.translation - pivot);
        let d1 = math::length(after.translation - pivot);
        assert!((d0 - d1).abs() < 1e-3, "distance changed: {d0} -> {d1}");

        // And it keeps the pivot at the same spot in the camera's view.
        let p0 = camera_space(&before, pivot);
        let p1 = camera_space(&after, pivot);
        assert!(
            math::length(p0 - p1) < 1e-3,
            "pivot moved on screen: {p0:?} -> {p1:?}"
        );

        // The camera really moved, and the rotation stayed unit length.
        assert!(math::length(after.translation - before.translation) > 0.1);
        assert!((after.rotation.length() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn orbit_direction_follows_the_drag() {
        let pivot = vec3(0.0, 0.0, 0.0);
        let base = orbiting_camera();

        // Dragging right swings the camera to `+X`.
        let right = orbit_transform(base, pivot, 20.0, 0.0);
        assert!(
            right.translation.x > 0.1,
            "right drag: {:?}",
            right.translation
        );

        // Dragging down drops the camera below the pivot.
        let down = orbit_transform(base, pivot, 0.0, 20.0);
        assert!(
            down.translation.y < -0.1,
            "down drag: {:?}",
            down.translation
        );
    }

    #[test]
    fn pan_slides_the_view_opposite_the_drag() {
        let base = orbiting_camera(); // right = +X, up = +Y

        // Dragging right moves the scene right, so the camera moves to `-X`.
        let right = pan_transform(base, 10.0, 0.0, 0.1);
        assert!(
            right.translation.x < -0.9,
            "right pan: {:?}",
            right.translation
        );
        assert!((right.translation.y).abs() < 1e-6);

        // Dragging down moves the scene down, so the camera moves to `+Y`.
        let down = pan_transform(base, 0.0, 10.0, 0.1);
        assert!(down.translation.y > 0.9, "down pan: {:?}", down.translation);

        // A pan never rotates the camera.
        let forward = base.rotation.rotate_vec3(vec3(0.0, 0.0, -1.0));
        let forward_after = right.rotation.rotate_vec3(vec3(0.0, 0.0, -1.0));
        assert!(math::length(forward - forward_after) < 1e-6);
    }

    #[test]
    fn dolly_moves_along_the_view_direction() {
        let base = orbiting_camera(); // forward is `-Z`

        let closer = dolly_transform(base, 10.0);
        assert!(
            closer.translation.z < base.translation.z,
            "scroll up should zoom in: {:?}",
            closer.translation
        );

        let farther = dolly_transform(base, -10.0);
        assert!(
            farther.translation.z > base.translation.z,
            "scroll down should zoom out: {:?}",
            farther.translation
        );

        // A dolly never rotates the camera.
        let forward = base.rotation.rotate_vec3(vec3(0.0, 0.0, -1.0));
        let forward_after = closer.rotation.rotate_vec3(vec3(0.0, 0.0, -1.0));
        assert!(math::length(forward - forward_after) < 1e-6);
    }
}
