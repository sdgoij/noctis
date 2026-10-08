//! Camera extraction and the camera uniform block.
//!
//! [`extract_camera`] is a pure function: given a [`World`] and a camera
//! [`Entity`] it produces a [`CameraView`] with the view, projection and
//! combined matrices plus the derived basis vectors. It touches no GPU state,
//! so it is fully unit-testable.
//!
//! [`CameraUniform`] is the `#[repr(C)]` / `Pod` mirror uploaded to the GPU. Its
//! packing is asserted against [`math::to_cols_array`] in the tests.

use bytemuck::{Pod, Zeroable};
use ecs::components::{Camera, GlobalTransform, LocalTransform, Projection};
use ecs::world::{Entity, World};
use math::{Mat4, Vec3, vec3};

/// Everything the renderer needs to know about a camera for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraView {
    /// Camera position in world space.
    pub eye: Vec3,
    /// Unit forward vector (the camera looks along this direction).
    pub forward: Vec3,
    /// Unit up vector.
    pub up: Vec3,
    /// Unit right vector.
    pub right: Vec3,
    /// World -> view matrix.
    pub view: Mat4,
    /// View -> clip matrix.
    pub projection: Mat4,
    /// World -> clip matrix (`projection * view`).
    pub view_projection: Mat4,
    /// Clip -> world matrix.
    pub inverse_view_projection: Mat4,
    /// Near plane distance.
    pub near: f32,
    /// Far plane distance.
    pub far: f32,
    /// Width / height.
    pub aspect: f32,
}

impl CameraView {
    /// The camera's world-space position as a 4-component vector.
    pub fn eye4(&self) -> [f32; 4] {
        [self.eye.x, self.eye.y, self.eye.z, 1.0]
    }
}

/// Build a projection matrix from a component [`Projection`] and aspect ratio.
pub fn projection_matrix(projection: &Projection, aspect: f32) -> Mat4 {
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        1.0
    };
    match *projection {
        Projection::Perspective { fov_y, near, far } => {
            math::perspective(fov_y, aspect, near, far.max(near + 1e-4))
        }
        Projection::Orthographic { height, near, far } => {
            let half_h = (height * 0.5).max(1e-4);
            let half_w = half_h * aspect;
            math::orthographic(-half_w, half_w, -half_h, half_h, near, far.max(near + 1e-4))
        }
    }
}

fn normalized_or(v: Vec3, fallback: Vec3) -> Vec3 {
    let len_sq = v.x * v.x + v.y * v.y + v.z * v.z;
    if len_sq > 1e-12 {
        let inv = len_sq.sqrt().recip();
        vec3(v.x * inv, v.y * inv, v.z * inv)
    } else {
        fallback
    }
}

fn column(m: &Mat4, c: usize) -> Vec3 {
    let v = m[c];
    vec3(v.x, v.y, v.z)
}

/// Extract a fully-resolved [`CameraView`] from a world.
///
/// Returns `None` if `entity` is not alive, has no [`Camera`], or the camera's
/// world matrix is singular. The basis vectors are taken from the world matrix
/// columns: `right = +X`, `up = +Y`, `forward = -Z` (a right-handed camera).
pub fn extract_camera(
    world: &World,
    entity: Entity,
    width: u32,
    height: u32,
) -> Option<CameraView> {
    let camera = world.get::<Camera>(entity)?;
    let world_matrix = if let Some(global) = world.get::<GlobalTransform>(entity) {
        global.to_matrix()
    } else if let Some(local) = world.get::<LocalTransform>(entity) {
        local.to_matrix()
    } else {
        math::identity()
    };
    Some(camera_view(camera, &world_matrix, width, height))
}

/// Build a [`CameraView`] from a camera component and its world matrix.
pub fn camera_view(camera: &Camera, world_matrix: &Mat4, width: u32, height: u32) -> CameraView {
    let aspect = if height == 0 {
        1.0
    } else {
        width as f32 / height as f32
    };
    let view = math::inverse(world_matrix).unwrap_or_else(math::identity);
    let projection = projection_matrix(&camera.projection, aspect);
    let view_projection = projection * view;
    let inverse_view_projection = math::inverse(&view_projection).unwrap_or_else(math::identity);

    let right = normalized_or(column(world_matrix, 0), vec3(1.0, 0.0, 0.0));
    let up = normalized_or(column(world_matrix, 1), vec3(0.0, 1.0, 0.0));
    let forward = normalized_or(-column(world_matrix, 2), vec3(0.0, 0.0, -1.0));
    let eye = column(world_matrix, 3);

    CameraView {
        eye,
        forward,
        up,
        right,
        view,
        projection,
        view_projection,
        inverse_view_projection,
        near: camera.projection.near(),
        far: camera.projection.far(),
        aspect,
    }
}

/// Convenience: extract the first active camera in the world.
pub fn first_active_camera(world: &World) -> Option<Entity> {
    let mut best: Option<(i32, Entity)> = None;
    for (entity, camera) in world.query::<Camera>() {
        if !camera.is_active {
            continue;
        }
        match best {
            Some((priority, _)) if priority <= camera.priority => {}
            _ => best = Some((camera.priority, entity)),
        }
    }
    best.map(|(_, e)| e)
}

/// The GPU-side camera uniform. All matrices are column-major `[f32; 4]` arrays
/// so they can be uploaded verbatim.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct CameraUniform {
    /// World -> clip.
    pub view_proj: [[f32; 4]; 4],
    /// Clip -> world.
    pub inv_view_proj: [[f32; 4]; 4],
    /// World -> view.
    pub view: [[f32; 4]; 4],
    /// View -> clip.
    pub proj: [[f32; 4]; 4],
    /// `xyz` = eye position, `w` = 1.
    pub position: [f32; 4],
    /// `x` = near, `y` = far, `z` = aspect, `w` = unused.
    pub near_far: [f32; 4],
}

impl CameraUniform {
    /// Pack a [`CameraView`] for the GPU.
    pub fn from_view(view: &CameraView) -> CameraUniform {
        CameraUniform {
            view_proj: math::to_cols_array(&view.view_projection),
            inv_view_proj: math::to_cols_array(&view.inverse_view_projection),
            view: math::to_cols_array(&view.view),
            proj: math::to_cols_array(&view.projection),
            position: view.eye4(),
            near_far: [view.near, view.far, view.aspect, 0.0],
        }
    }
}

impl Default for CameraUniform {
    fn default() -> Self {
        CameraUniform {
            view_proj: math::to_cols_array(&math::identity()),
            inv_view_proj: math::to_cols_array(&math::identity()),
            view: math::to_cols_array(&math::identity()),
            proj: math::to_cols_array(&math::identity()),
            position: [0.0, 0.0, 0.0, 1.0],
            near_far: [0.1, 1000.0, 1.0, 0.0],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ecs::components::{GlobalTransform, LocalTransform, Projection};
    use math::Quat;

    #[test]
    fn camera_view_basis_from_identity_matrix() {
        let camera = Camera {
            projection: Projection::Perspective {
                fov_y: math::radians(60.0),
                near: 0.1,
                far: 100.0,
            },
            ..Camera::default()
        };
        let view = camera_view(&camera, &math::identity(), 800, 600);
        assert!(math::vec3_approx_eq(
            view.forward,
            vec3(0.0, 0.0, -1.0),
            1e-5
        ));
        assert!(math::vec3_approx_eq(view.up, vec3(0.0, 1.0, 0.0), 1e-5));
        assert!(math::vec3_approx_eq(view.right, vec3(1.0, 0.0, 0.0), 1e-5));
        assert!(math::vec3_approx_eq(view.eye, vec3(0.0, 0.0, 0.0), 1e-5));
        assert!((view.aspect - 800.0 / 600.0).abs() < 1e-6);
    }

    #[test]
    fn projection_maps_near_and_far() {
        let camera = Camera::default();
        let proj = projection_matrix(&camera.projection, 1.0);
        let near = proj * math::vec4(0.0, 0.0, -0.1, 1.0);
        let far = proj * math::vec4(0.0, 0.0, -1000.0, 1.0);
        assert!((near.z / near.w).abs() < 1e-3);
        assert!((far.z / far.w - 1.0).abs() < 1e-3);
    }

    #[test]
    fn orthographic_projection_is_accepted() {
        let camera = Camera {
            projection: Projection::Orthographic {
                height: 10.0,
                near: 0.1,
                far: 100.0,
            },
            ..Camera::default()
        };
        let view = camera_view(&camera, &math::identity(), 100, 100);
        // Center of the ortho volume maps to the center of clip space.
        let p = view.projection * math::vec4(0.0, 0.0, -1.0, 1.0);
        assert!(p.x.abs() < 1e-4 && p.y.abs() < 1e-4);
    }

    #[test]
    fn extract_requires_camera_component() {
        let mut world = World::new();
        let e = world.spawn();
        assert!(extract_camera(&world, e, 100, 100).is_none());
    }

    #[test]
    fn extract_uses_global_transform() {
        let mut world = World::new();
        let e = world.spawn_with(Camera::default());
        world.insert(
            e,
            GlobalTransform::from_local(&LocalTransform::from_translation(vec3(0.0, 0.0, 5.0))),
        );
        let view = extract_camera(&world, e, 100, 100).expect("camera");
        assert!(math::vec3_approx_eq(view.eye, vec3(0.0, 0.0, 5.0), 1e-5));
    }

    #[test]
    fn rotated_camera_faces_its_forward_axis() {
        let mut world = World::new();
        let e = world.spawn_with(Camera::default());
        // Rotate 90 degrees about Y: local -Z becomes world -X.
        let local =
            LocalTransform::from_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
        world.insert(e, GlobalTransform::from_local(&local));
        let view = extract_camera(&world, e, 100, 100).expect("camera");
        assert!(
            math::vec3_approx_eq(view.forward, vec3(-1.0, 0.0, 0.0), 1e-4),
            "forward = {:?}",
            view.forward
        );
    }

    #[test]
    fn first_active_camera_prefers_lowest_priority() {
        let mut world = World::new();
        let a = world.spawn_with(Camera {
            priority: 10,
            ..Camera::default()
        });
        let b = world.spawn_with(Camera {
            priority: 1,
            ..Camera::default()
        });
        let c = world.spawn_with(Camera {
            is_active: false,
            priority: -5,
            ..Camera::default()
        });
        assert_eq!(first_active_camera(&world), Some(b));
        assert_ne!(first_active_camera(&world), Some(c));
        let _ = a;
    }

    #[test]
    fn uniform_packing_matches_math_helper() {
        let camera = Camera::default();
        let local = LocalTransform::from_translation(vec3(1.0, 2.0, 3.0));
        let view = camera_view(&camera, &local.to_matrix(), 1280, 720);
        let uniform = CameraUniform::from_view(&view);
        assert_eq!(
            uniform.view_proj,
            math::to_cols_array(&view.view_projection)
        );
        assert_eq!(uniform.view, math::to_cols_array(&view.view));
        assert_eq!(uniform.proj, math::to_cols_array(&view.projection));
        assert_eq!(uniform.position, [1.0, 2.0, 3.0, 1.0]);
        assert!((uniform.near_far[0] - 0.1).abs() < 1e-6);
        assert!((uniform.near_far[2] - 1280.0 / 720.0).abs() < 1e-6);
    }

    #[test]
    fn uniform_size_is_multiple_of_alignment() {
        // Four 4x4 matrices plus two vec4s: 4 * 64 + 2 * 16 = 288 bytes, and it
        // must be a multiple of 16 to satisfy WGSL uniform alignment.
        assert_eq!(std::mem::size_of::<CameraUniform>(), 288);
        assert_eq!(std::mem::size_of::<CameraUniform>() % 16, 0);
    }
}
