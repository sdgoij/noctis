//! A small built-in scene so `noctis-runtime --demo` does something useful with
//! no input files: a ground slab, a falling ball, a camera, and a sun.

use ecs::assets::{AssetRegistry, Material, Mesh, Vertex};
use ecs::components::{
    BodyType, Camera, Collider, ColliderShape, Light, LocalTransform, MeshRenderer, Name, RigidBody,
};
use ecs::scene::Scene;
use ecs::world::World;
use math::{Quat, normalize, vec3};

/// Build the demo scene, registering its mesh and material in `assets`.
///
/// The scene is:
///
/// * a fixed ground slab (top surface at `y = 0`),
/// * a dynamic ball dropped from `y = 5`,
/// * a perspective camera at `(0, 4, 10)` looking at the origin,
/// * a directional sun.
pub fn demo_scene(assets: &mut AssetRegistry) -> Scene {
    let mesh = assets.meshes.insert_named("demo/cube", cube_mesh());
    let material = assets.materials.insert_named(
        "demo/material",
        Material {
            name: "demo".into(),
            base_color: [0.8, 0.32, 0.28, 1.0],
            roughness: 0.4,
            double_sided: true,
            ..Default::default()
        },
    );

    let mut world = World::new();

    // Ground: a wide, thin slab whose top face sits on y = 0.
    let ground = world.spawn();
    world.insert(ground, Name::new("Ground"));
    world.insert(
        ground,
        LocalTransform::from_translation(vec3(0.0, -0.5, 0.0)).with_scale(vec3(1.0, 1.0, 1.0)),
    );
    world.insert(
        ground,
        RigidBody {
            body_type: BodyType::Fixed,
            ..Default::default()
        },
    );
    world.insert(
        ground,
        Collider {
            shape: ColliderShape::Cuboid {
                half_extents: vec3(10.0, 0.5, 10.0),
            },
            ..Default::default()
        },
    );
    world.insert(
        ground,
        MeshRenderer {
            mesh,
            material,
            ..Default::default()
        },
    );

    // Ball: a dynamic sphere that falls onto the ground.
    let ball = world.spawn();
    world.insert(ball, Name::new("Ball"));
    world.insert(ball, LocalTransform::from_translation(vec3(0.0, 5.0, 0.0)));
    world.insert(ball, RigidBody::default());
    world.insert(
        ball,
        Collider {
            shape: ColliderShape::Ball { radius: 0.5 },
            ..Default::default()
        },
    );
    world.insert(
        ball,
        MeshRenderer {
            mesh,
            material,
            ..Default::default()
        },
    );

    // Camera, looking back at the origin.
    let camera = world.spawn();
    world.insert(camera, Name::new("Camera"));
    let eye = vec3(0.0, 4.0, 10.0);
    let forward = normalize(vec3(-eye.x, -eye.y, -eye.z));
    let rotation = Quat::look_rotation(forward, vec3(0.0, 1.0, 0.0));
    world.insert(
        camera,
        LocalTransform::from_translation(eye).with_rotation(rotation),
    );
    world.insert(camera, Camera::default());

    // Sun.
    let sun = world.spawn();
    world.insert(sun, Name::new("Sun"));
    world.insert(sun, LocalTransform::default());
    world.insert(sun, Light::directional(vec3(-0.3, -1.0, -0.4)));

    let mut scene = Scene::new("Demo");
    scene.world = world;
    scene
}

/// A unit cube centred at the origin: 24 vertices (4 per face, for flat
/// normals) and 36 indices.
fn cube_mesh() -> Mesh {
    // Normal, then the four corners of the face (counter-clockwise seen from
    // outside).
    let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
        (
            [1.0, 0.0, 0.0],
            [
                [0.5, -0.5, 0.5],
                [0.5, -0.5, -0.5],
                [0.5, 0.5, -0.5],
                [0.5, 0.5, 0.5],
            ],
        ),
        (
            [-1.0, 0.0, 0.0],
            [
                [-0.5, -0.5, -0.5],
                [-0.5, -0.5, 0.5],
                [-0.5, 0.5, 0.5],
                [-0.5, 0.5, -0.5],
            ],
        ),
        (
            [0.0, 1.0, 0.0],
            [
                [-0.5, 0.5, 0.5],
                [0.5, 0.5, 0.5],
                [0.5, 0.5, -0.5],
                [-0.5, 0.5, -0.5],
            ],
        ),
        (
            [0.0, -1.0, 0.0],
            [
                [-0.5, -0.5, -0.5],
                [0.5, -0.5, -0.5],
                [0.5, -0.5, 0.5],
                [-0.5, -0.5, 0.5],
            ],
        ),
        (
            [0.0, 0.0, 1.0],
            [
                [-0.5, -0.5, 0.5],
                [0.5, -0.5, 0.5],
                [0.5, 0.5, 0.5],
                [-0.5, 0.5, 0.5],
            ],
        ),
        (
            [0.0, 0.0, -1.0],
            [
                [0.5, -0.5, -0.5],
                [-0.5, -0.5, -0.5],
                [-0.5, 0.5, -0.5],
                [0.5, 0.5, -0.5],
            ],
        ),
    ];
    let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

    let mut vertices = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    for (normal, corners) in faces {
        let base = vertices.len() as u32;
        for (corner, uv) in corners.iter().zip(uvs) {
            vertices.push(Vertex {
                position: *corner,
                normal,
                uv,
                tangent: [1.0, 0.0, 0.0, 1.0],
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    let mut mesh = Mesh {
        name: "cube".into(),
        vertices,
        indices,
        bounds: None,
    };
    mesh.bounds = mesh.compute_bounds();
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use ecs::components::GlobalTransform;
    use ecs::scene::propagate_transforms;

    #[test]
    fn demo_scene_has_the_expected_entities() {
        let mut assets = AssetRegistry::with_defaults();
        let mut scene = demo_scene(&mut assets);
        propagate_transforms(&mut scene.world);

        let world = &scene.world;
        assert_eq!(world.entities().len(), 4);
        for name in ["Ground", "Ball", "Camera", "Sun"] {
            assert!(
                world
                    .entities_with::<Name>()
                    .into_iter()
                    .any(|e| world.get::<Name>(e).is_some_and(|n| n.as_str() == name)),
                "missing entity {name}"
            );
        }
        assert!(world.count::<Camera>() == 1);
        assert!(world.count::<Light>() == 1);
        assert_eq!(world.count::<RigidBody>(), 2);
        assert_eq!(world.count::<Collider>(), 2);
        assert!(world.count::<GlobalTransform>() >= 4);
    }

    #[test]
    fn cube_mesh_is_well_formed() {
        let mesh = cube_mesh();
        assert_eq!(mesh.vertices.len(), 24);
        assert_eq!(mesh.indices.len(), 36);
        assert_eq!(mesh.triangle_count(), 12);
        assert!(mesh.bounds.is_some());
    }
}
