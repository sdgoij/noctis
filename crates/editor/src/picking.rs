//! Viewport ray picking: turn a click in the 3D view into a selected entity.
//!
//! The math here is pure and unit-tested: unproject a normalized-device-
//! coordinate into a world-space [`Ray`], then intersect it against the world
//! bounds of every [`MeshRenderer`] in the scene.

use ecs::assets::AssetRegistry;
use ecs::components::{GlobalTransform, MeshRenderer};
use ecs::world::{Entity, World};
use math::{Aabb, Mat4, Vec3, vec3};

/// A world-space ray.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    /// The ray origin.
    pub origin: Vec3,
    /// The unit-length ray direction.
    pub direction: Vec3,
}

impl Ray {
    /// A ray from an origin and a direction (normalized here).
    pub fn new(origin: Vec3, direction: Vec3) -> Self {
        let len =
            (direction.x * direction.x + direction.y * direction.y + direction.z * direction.z)
                .sqrt();
        let direction = if len > 0.0 {
            vec3(direction.x / len, direction.y / len, direction.z / len)
        } else {
            vec3(0.0, 0.0, -1.0)
        };
        Self { origin, direction }
    }

    /// The point at distance `t` along the ray.
    pub fn at(&self, t: f32) -> Vec3 {
        vec3(
            self.origin.x + self.direction.x * t,
            self.origin.y + self.direction.y * t,
            self.origin.z + self.direction.z * t,
        )
    }
}

fn unproject(inverse_view_projection: &Mat4, x: f32, y: f32, z: f32) -> Vec3 {
    let v = *inverse_view_projection * math::vec4(x, y, z, 1.0);
    if v.w.abs() > f32::EPSILON {
        vec3(v.x / v.w, v.y / v.w, v.z / v.w)
    } else {
        vec3(v.x, v.y, v.z)
    }
}

/// Build a world-space ray for normalized device coordinates in `[-1, 1]`
/// (`y` pointing up), given an inverse view-projection matrix.
pub fn ray_from_ndc(inverse_view_projection: &Mat4, ndc_x: f32, ndc_y: f32) -> Ray {
    let near = unproject(inverse_view_projection, ndc_x, ndc_y, 0.0);
    let far = unproject(inverse_view_projection, ndc_x, ndc_y, 1.0);
    Ray::new(near, far - near)
}

/// Slab ray/AABB intersection. Returns the entry distance (`>= 0`) if the ray
/// hits within `max_t`.
pub fn ray_aabb(ray: &Ray, aabb: &Aabb, max_t: f32) -> Option<f32> {
    let o = [ray.origin.x, ray.origin.y, ray.origin.z];
    let d = [ray.direction.x, ray.direction.y, ray.direction.z];
    let mn = [aabb.min.x, aabb.min.y, aabb.min.z];
    let mx = [aabb.max.x, aabb.max.y, aabb.max.z];

    let mut t_min = 0.0_f32;
    let mut t_max = max_t;
    for i in 0..3 {
        if d[i].abs() < 1e-8 {
            if o[i] < mn[i] || o[i] > mx[i] {
                return None;
            }
        } else {
            let inv = 1.0 / d[i];
            let t1 = (mn[i] - o[i]) * inv;
            let t2 = (mx[i] - o[i]) * inv;
            let (lo, hi) = if t1 <= t2 { (t1, t2) } else { (t2, t1) };
            t_min = t_min.max(lo);
            t_max = t_max.min(hi);
            if t_min > t_max {
                return None;
            }
        }
    }
    Some(t_min)
}

/// The nearest entity whose world-space mesh bounds the ray hits, within
/// `max_distance`.
pub fn pick_entity(
    world: &World,
    assets: &AssetRegistry,
    ray: &Ray,
    max_distance: f32,
) -> Option<Entity> {
    let mut best: Option<(f32, Entity)> = None;
    for (entity, renderer) in world.query::<MeshRenderer>() {
        let Some(global) = world.get::<GlobalTransform>(entity) else {
            continue;
        };
        let Some(mesh) = assets.meshes.get(renderer.mesh) else {
            continue;
        };
        let Some(local) = mesh.bounds.or_else(|| mesh.compute_bounds()) else {
            continue;
        };
        let world_aabb = render::transform_aabb(&global.matrix, &local);
        if let Some(t) = ray_aabb(ray, &world_aabb, max_distance) {
            if best.is_none_or(|(best_t, _)| t < best_t) {
                best = Some((t, entity));
            }
        }
    }
    best.map(|(_, entity)| entity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ecs::assets::{AssetHandle, Mesh};
    use math::vec3;

    fn unit_cube_aabb() -> Aabb {
        Aabb::new(vec3(-0.5, -0.5, -0.5), vec3(0.5, 0.5, 0.5))
    }

    #[test]
    fn ray_from_ndc_identity_points_forward() {
        let identity = math::identity();
        let ray = ray_from_ndc(&identity, 0.0, 0.0);
        assert!((ray.origin.x).abs() < 1e-5 && (ray.origin.y).abs() < 1e-5);
        assert!(
            (ray.direction.z - 1.0).abs() < 1e-5,
            "dir {:?}",
            ray.direction
        );
    }

    #[test]
    fn ray_aabb_hits_front_face() {
        let ray = Ray::new(vec3(0.0, 0.0, 5.0), vec3(0.0, 0.0, -1.0));
        let t = ray_aabb(&ray, &unit_cube_aabb(), 100.0).expect("hit");
        assert!((t - 4.5).abs() < 1e-4, "t = {t}");
    }

    #[test]
    fn ray_aabb_misses_when_offset_or_behind() {
        let box_ = unit_cube_aabb();
        let offset = Ray::new(vec3(5.0, 0.0, 5.0), vec3(0.0, 0.0, -1.0));
        assert_eq!(ray_aabb(&offset, &box_, 100.0), None);
        let behind = Ray::new(vec3(0.0, 0.0, 5.0), vec3(0.0, 0.0, 1.0));
        assert_eq!(ray_aabb(&behind, &box_, 100.0), None);
        let too_far = Ray::new(vec3(0.0, 0.0, 5.0), vec3(0.0, 0.0, -1.0));
        assert_eq!(ray_aabb(&too_far, &box_, 1.0), None);
    }

    #[test]
    fn ray_at_advances_along_direction() {
        let ray = Ray::new(vec3(0.0, 0.0, 0.0), vec3(0.0, 0.0, -2.0));
        assert!(ray.direction.z < 0.0);
        let p = ray.at(3.0);
        assert!((p.z + 3.0).abs() < 1e-5);
    }

    #[test]
    fn pick_entity_selects_the_mesh_under_the_ray() {
        let mut world = World::new();
        let mut assets = AssetRegistry::with_defaults();
        let mesh = assets.meshes.insert_named(
            "cube",
            Mesh {
                name: "cube".into(),
                vertices: Vec::new(),
                indices: Vec::new(),
                bounds: Some(unit_cube_aabb()),
            },
        );

        let entity = world.spawn();
        world.insert(entity, GlobalTransform::identity());
        world.insert(
            entity,
            MeshRenderer {
                mesh,
                ..Default::default()
            },
        );

        let hit = Ray::new(vec3(0.0, 0.0, 5.0), vec3(0.0, 0.0, -1.0));
        assert_eq!(pick_entity(&world, &assets, &hit, 100.0), Some(entity));

        let miss = Ray::new(vec3(3.0, 3.0, 5.0), vec3(0.0, 0.0, -1.0));
        assert_eq!(pick_entity(&world, &assets, &miss, 100.0), None);
    }

    #[test]
    fn pick_entity_respects_a_translated_mesh() {
        let mut world = World::new();
        let mut assets = AssetRegistry::with_defaults();
        let mesh: AssetHandle<Mesh> = assets.meshes.insert_named(
            "cube",
            Mesh {
                name: "cube".into(),
                vertices: Vec::new(),
                indices: Vec::new(),
                bounds: Some(unit_cube_aabb()),
            },
        );

        let entity = world.spawn();
        world.insert(
            entity,
            GlobalTransform {
                matrix: math::translation(vec3(10.0, 0.0, 0.0)),
            },
        );
        world.insert(
            entity,
            MeshRenderer {
                mesh,
                ..Default::default()
            },
        );

        let at_origin = Ray::new(vec3(0.0, 0.0, 5.0), vec3(0.0, 0.0, -1.0));
        assert_eq!(pick_entity(&world, &assets, &at_origin, 100.0), None);

        let at_mesh = Ray::new(vec3(10.0, 0.0, 5.0), vec3(0.0, 0.0, -1.0));
        assert_eq!(pick_entity(&world, &assets, &at_mesh, 100.0), Some(entity));
    }
}
