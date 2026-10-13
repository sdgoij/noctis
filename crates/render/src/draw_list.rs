//! Draw-list extraction, frustum culling, and sorting.
//!
//! [`extract_draw_list`] is a pure function that walks a [`World`], resolves
//! each [`MeshRenderer`]'s world transform and material, culls it against the
//! camera frustum, computes a view distance, and splits the survivors into an
//! **opaque** list (sorted front-to-back to maximise early-z rejection) and a
//! **transparent** list (sorted back-to-front for correct alpha blending).
//!
//! Nothing here touches the GPU, so the ordering and culling rules are directly
//! unit-tested.

use crate::camera::CameraView;
use crate::frustum::{Frustum, transform_aabb};
use ecs::assets::{AlphaMode, AssetHandle, AssetRegistry, Material, Mesh, Skin};
use ecs::components::{
    Active, GlobalTransform, LocalTransform, MeshRenderer, Parent, Skeleton, SkinnedMeshRenderer,
    Visible,
};
use ecs::world::{Entity, World};
use math::{Aabb, Mat4, vec3};

/// One mesh instance scheduled for drawing.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawItem {
    /// The owning entity.
    pub entity: Entity,
    /// The mesh to draw.
    pub mesh: AssetHandle<Mesh>,
    /// The material to shade with.
    pub material: AssetHandle<Material>,
    /// The world transform.
    pub world_matrix: Mat4,
    /// The world-space bounds (indexed against culling and sorting).
    pub world_bounds: Aabb,
    /// Distance from the camera eye to the bounds centre.
    pub distance: f32,
    /// Whether this item uses alpha blending.
    pub transparent: bool,
    /// Whether the item casts shadows.
    pub cast_shadows: bool,
    /// Whether the item receives shadows.
    pub receive_shadows: bool,
    /// The joint skin matrices (one per joint) for a skinned mesh, or `None`.
    pub skin: Option<Vec<Mat4>>,
}

/// The sorted output of [`extract_draw_list`].
#[derive(Clone, Debug, Default)]
pub struct DrawList {
    /// Opaque items, nearest first.
    pub opaque: Vec<DrawItem>,
    /// Transparent items, furthest first.
    pub transparent: Vec<DrawItem>,
    /// How many candidate entities were rejected by frustum culling.
    pub culled: usize,
}

impl DrawList {
    /// The total number of items (opaque + transparent).
    pub fn len(&self) -> usize {
        self.opaque.len() + self.transparent.len()
    }

    /// `true` if there is nothing to draw.
    pub fn is_empty(&self) -> bool {
        self.opaque.is_empty() && self.transparent.is_empty()
    }

    /// Iterate every item (opaque then transparent).
    pub fn iter(&self) -> impl Iterator<Item = &DrawItem> {
        self.opaque.iter().chain(self.transparent.iter())
    }
}

fn world_matrix(world: &World, entity: Entity) -> Mat4 {
    if let Some(global) = world.get::<GlobalTransform>(entity) {
        global.to_matrix()
    } else if let Some(local) = world.get::<LocalTransform>(entity) {
        local.to_matrix()
    } else {
        math::identity()
    }
}

/// Whether an entity is drawn: it must not have `Visible(false)` or
/// `Active(false)` on itself or any ancestor.
pub fn is_renderable(world: &World, entity: Entity) -> bool {
    let mut cursor = Some(entity);
    let mut guard = 0;
    while let Some(current) = cursor {
        if let Some(Visible(false)) = world.get::<Visible>(current) {
            return false;
        }
        if let Some(Active(false)) = world.get::<Active>(current) {
            return false;
        }
        cursor = world.get::<Parent>(current).map(|p| p.0);
        guard += 1;
        if guard > 4096 {
            break;
        }
    }
    true
}

/// The local-space bounds of a mesh, computing them if the asset has none.
fn mesh_bounds(mesh: &Mesh) -> Aabb {
    if let Some(bounds) = mesh.bounds
        && !bounds.is_empty()
    {
        return bounds;
    }
    mesh.compute_bounds()
        .unwrap_or(Aabb::new(vec3(-0.5, -0.5, -0.5), vec3(0.5, 0.5, 0.5)))
}

/// Whether a material is alpha-blended (and therefore drawn last).
fn is_transparent(material: Option<&Material>) -> bool {
    matches!(material.map(|m| m.alpha_mode), Some(AlphaMode::Blend))
}

/// The joint skin matrices for a skinned entity: `mesh_world⁻¹ · joint_world ·
/// inverse_bind` per joint, the pose the vertex shader blends against the four
/// influences. Empty when the skin or skeleton is missing.
fn skin_matrices(
    world: &World,
    assets: &AssetRegistry,
    entity: Entity,
    mesh_world: Mat4,
    skin: AssetHandle<Skin>,
) -> Vec<Mat4> {
    let Some(skin) = assets.skins.get(skin) else {
        return Vec::new();
    };
    let Some(skeleton) = world.get::<Skeleton>(entity) else {
        return Vec::new();
    };
    let Some(inverse_mesh) = math::inverse(&mesh_world) else {
        return Vec::new();
    };
    skin.joints
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let joint_world = skeleton
                .0
                .get(index)
                .and_then(|joint| world.get::<GlobalTransform>(*joint))
                .map(|global| global.to_matrix())
                .unwrap_or_else(math::identity);
            let inverse_bind = skin
                .inverse_bind
                .get(index)
                .copied()
                .unwrap_or_else(math::identity);
            inverse_mesh * joint_world * inverse_bind
        })
        .collect()
}

/// Extract, cull, and sort every drawable in `world`.
pub fn extract_draw_list(world: &World, assets: &AssetRegistry, view: &CameraView) -> DrawList {
    let frustum = Frustum::from_view_projection(&view.view_projection);
    let mut list = DrawList::default();

    for (entity, renderer) in world.query::<MeshRenderer>() {
        if renderer.mesh.is_none() {
            continue;
        }
        let Some(mesh) = assets.meshes.get(renderer.mesh) else {
            continue;
        };
        if !is_renderable(world, entity) {
            continue;
        }

        let matrix = world_matrix(world, entity);
        let local = mesh_bounds(mesh);
        let world_bounds = transform_aabb(&matrix, &local);

        if !frustum.intersects_aabb(&world_bounds) {
            list.culled += 1;
            continue;
        }

        let center = world_bounds.center();
        let delta = center - view.eye;
        let distance = (delta.x * delta.x + delta.y * delta.y + delta.z * delta.z).sqrt();
        let material = assets.materials.get(renderer.material);

        let item = DrawItem {
            entity,
            mesh: renderer.mesh,
            material: renderer.material,
            world_matrix: matrix,
            world_bounds,
            distance,
            transparent: is_transparent(material),
            cast_shadows: renderer.cast_shadows,
            receive_shadows: renderer.receive_shadows,
            skin: None,
        };

        if item.transparent {
            list.transparent.push(item);
        } else {
            list.opaque.push(item);
        }
    }

    // Skinned meshes: the same, plus each vertex's weighted joint matrices.
    for (entity, renderer) in world.query::<SkinnedMeshRenderer>() {
        if renderer.mesh.is_none() {
            continue;
        }
        let Some(mesh) = assets.meshes.get(renderer.mesh) else {
            continue;
        };
        if !is_renderable(world, entity) {
            continue;
        }

        let matrix = world_matrix(world, entity);
        let local = mesh_bounds(mesh);
        let world_bounds = transform_aabb(&matrix, &local);

        if !frustum.intersects_aabb(&world_bounds) {
            list.culled += 1;
            continue;
        }

        let center = world_bounds.center();
        let delta = center - view.eye;
        let distance = (delta.x * delta.x + delta.y * delta.y + delta.z * delta.z).sqrt();
        let material = assets.materials.get(renderer.material);

        let item = DrawItem {
            entity,
            mesh: renderer.mesh,
            material: renderer.material,
            world_matrix: matrix,
            world_bounds,
            distance,
            transparent: is_transparent(material),
            cast_shadows: renderer.cast_shadows,
            receive_shadows: renderer.receive_shadows,
            skin: Some(skin_matrices(world, assets, entity, matrix, renderer.skin)),
        };

        if item.transparent {
            list.transparent.push(item);
        } else {
            list.opaque.push(item);
        }
    }

    // Front-to-back for opaque (early-z friendly).
    list.opaque.sort_by(|a, b| {
        a.distance
            .partial_cmp(&b.distance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    // Back-to-front for transparent (correct blending).
    list.transparent.sort_by(|a, b| {
        b.distance
            .partial_cmp(&a.distance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    list
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::extract_camera;
    use ecs::assets::{Material, Mesh, Texture, TextureFormat, Vertex};
    use ecs::components::Camera;
    use math::vec3;

    fn triangle_mesh(name: &str) -> Mesh {
        Mesh {
            name: name.into(),
            vertices: vec![
                Vertex {
                    position: [0.0, 0.0, 0.0],
                    ..Vertex::default()
                },
                Vertex {
                    position: [1.0, 0.0, 0.0],
                    ..Vertex::default()
                },
                Vertex {
                    position: [0.0, 1.0, 0.0],
                    ..Vertex::default()
                },
            ],
            indices: vec![0, 1, 2],
            ..Default::default()
        }
    }

    fn setup() -> (World, AssetRegistry, Entity) {
        let mut registry = AssetRegistry::new();
        registry.meshes.insert_named("tri", triangle_mesh("tri"));
        registry.materials.insert_named("mat", Material::default());
        let mut world = World::new();
        let camera = world.spawn_with(Camera::default());
        (world, registry, camera)
    }

    fn spawn_renderable(world: &mut World, registry: &AssetRegistry, at: math::Vec3) -> Entity {
        let e = world.spawn();
        world.insert(e, LocalTransform::from_translation(at));
        world.insert(
            e,
            MeshRenderer {
                mesh: registry.meshes.handle("tri").unwrap(),
                material: registry.materials.handle("mat").unwrap(),
                ..MeshRenderer::default()
            },
        );
        e
    }

    fn camera_view(world: &World, camera: Entity) -> CameraView {
        extract_camera(world, camera, 800, 600).unwrap()
    }

    #[test]
    fn extracts_visible_mesh() {
        let (mut world, registry, camera) = setup();
        spawn_renderable(&mut world, &registry, vec3(0.0, 0.0, -5.0));
        let view = camera_view(&world, camera);
        let list = extract_draw_list(&world, &registry, &view);
        assert_eq!(list.opaque.len(), 1);
        assert_eq!(list.culled, 0);
    }

    #[test]
    fn culls_geometry_behind_camera() {
        let (mut world, registry, camera) = setup();
        spawn_renderable(&mut world, &registry, vec3(0.0, 0.0, 5.0));
        let view = camera_view(&world, camera);
        let list = extract_draw_list(&world, &registry, &view);
        assert!(list.is_empty());
        assert_eq!(list.culled, 1);
    }

    #[test]
    fn skips_missing_mesh_asset() {
        let (mut world, registry, camera) = setup();
        let e = world.spawn();
        world.insert(e, LocalTransform::from_translation(vec3(0.0, 0.0, -2.0)));
        world.insert(
            e,
            MeshRenderer {
                mesh: AssetHandle::from_raw(999),
                ..MeshRenderer::default()
            },
        );
        let view = camera_view(&world, camera);
        let list = extract_draw_list(&world, &registry, &view);
        assert!(list.is_empty());
    }

    #[test]
    fn hidden_entity_is_not_drawn() {
        let (mut world, registry, camera) = setup();
        let e = spawn_renderable(&mut world, &registry, vec3(0.0, 0.0, -5.0));
        world.insert(e, Visible(false));
        let view = camera_view(&world, camera);
        assert!(extract_draw_list(&world, &registry, &view).is_empty());
    }

    #[test]
    fn inactive_parent_hides_child() {
        let (mut world, registry, camera) = setup();
        let parent = world.spawn_with(Active(false));
        let child = spawn_renderable(&mut world, &registry, vec3(0.0, 0.0, -5.0));
        ecs::scene::set_parent(&mut world, child, Some(parent));
        let view = camera_view(&world, camera);
        assert!(extract_draw_list(&world, &registry, &view).is_empty());
    }

    #[test]
    fn opaque_is_sorted_front_to_back() {
        let (mut world, registry, camera) = setup();
        let near = spawn_renderable(&mut world, &registry, vec3(0.0, 0.0, -2.0));
        let mid = spawn_renderable(&mut world, &registry, vec3(0.0, 0.0, -6.0));
        let far = spawn_renderable(&mut world, &registry, vec3(0.0, 0.0, -12.0));
        let view = camera_view(&world, camera);
        let list = extract_draw_list(&world, &registry, &view);
        assert_eq!(
            list.opaque.iter().map(|i| i.entity).collect::<Vec<_>>(),
            vec![near, mid, far]
        );
    }

    #[test]
    fn transparent_is_sorted_back_to_front() {
        let (mut world, mut registry, camera) = setup();
        let alpha = registry.materials.insert(Material {
            alpha_mode: AlphaMode::Blend,
            ..Material::default()
        });
        let mut spawn = |at: math::Vec3| {
            let e = world.spawn();
            world.insert(e, LocalTransform::from_translation(at));
            world.insert(
                e,
                MeshRenderer {
                    mesh: registry.meshes.handle("tri").unwrap(),
                    material: alpha,
                    ..MeshRenderer::default()
                },
            );
            e
        };
        let near = spawn(vec3(0.0, 0.0, -2.0));
        let far = spawn(vec3(0.0, 0.0, -12.0));
        let view = camera_view(&world, camera);
        let list = extract_draw_list(&world, &registry, &view);
        assert!(list.opaque.is_empty());
        assert_eq!(
            list.transparent
                .iter()
                .map(|i| i.entity)
                .collect::<Vec<_>>(),
            vec![far, near]
        );
    }

    #[test]
    fn bounds_fall_back_to_computed() {
        let (mut world, registry, camera) = setup();
        spawn_renderable(&mut world, &registry, vec3(0.0, 0.0, -5.0));
        let view = camera_view(&world, camera);
        let list = extract_draw_list(&world, &registry, &view);
        let b = list.opaque[0].world_bounds;
        assert!(!b.is_empty());
        assert!(b.max.x >= b.min.x);
    }

    #[test]
    fn full_frame_behind_each_other_all_visible_from_above() {
        // A chain of boxes receding along -Z; from a camera looking down -Z they
        // are all within the frustum but at distinct depths.
        let (mut world, registry, camera) = setup();
        for i in 0..5 {
            spawn_renderable(&mut world, &registry, vec3(0.0, 0.0, -2.0 - i as f32 * 2.0));
        }
        let view = camera_view(&world, camera);
        let list = extract_draw_list(&world, &registry, &view);
        assert_eq!(list.len(), 5);
        assert!(
            list.opaque
                .windows(2)
                .all(|w| w[0].distance <= w[1].distance)
        );
    }

    #[test]
    fn transforms_move_world_bounds() {
        let (mut world, registry, camera) = setup();
        let e = spawn_renderable(&mut world, &registry, vec3(0.0, 0.0, -5.0));
        world.insert(
            e,
            GlobalTransform::from_local(&LocalTransform::from_translation(vec3(3.0, 1.0, -5.0))),
        );
        let view = camera_view(&world, camera);
        let list = extract_draw_list(&world, &registry, &view);
        let c = list.opaque[0].world_bounds.center();
        assert!(
            math::vec3_approx_eq(c, vec3(3.5, 1.5, -5.0), 1e-3),
            "center {c:?}"
        );
    }

    #[test]
    fn texture_registry_does_not_affect_extraction() {
        let (mut world, mut registry, camera) = setup();
        registry.textures.insert(Texture::solid(
            "white",
            [255, 255, 255, 255],
            TextureFormat::Rgba8Srgb,
        ));
        spawn_renderable(&mut world, &registry, vec3(0.0, 0.0, -5.0));
        let view = camera_view(&world, camera);
        assert_eq!(extract_draw_list(&world, &registry, &view).len(), 1);
    }
}
