//! # testscene — the engine's all-features integration scene
//!
//! This crate is the integration spine of the engine: [`build`] assembles a
//! single, self-contained [`Scene`] that exercises *every* built-in component
//! kind ([`ecs::components::component_index::ComponentKind::ALL`]) together with
//! a representative spread of assets and features — procedural meshes and
//! PBR materials, a camera pair, the three light shapes, a rigid-body/collider
//! sandbox, spatial and non-spatial audio, and JavaScript behaviours that cover
//! the whole [`script`](crate) API surface.
//!
//! The scene is meant to be driven end-to-end by the other crates:
//!
//! * `physics` syncs and steps the bodies,
//! * `script` runs the behaviours,
//! * `audio` reconciles the emitters,
//! * `render` extracts the draw list and (when a GPU exists) renders a frame.
//!
//! Everything here is deterministic and GPU/device-free, so the integration
//! tests in `tests/all_features.rs` pass without any hardware.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod primitives;

use std::f32::consts::TAU;

use ecs::assets::{
    AlphaMode, AssetRegistry, AudioClip, HdrTexture, Material, ScriptAsset, Texture, TextureFormat,
};
use ecs::components::component_index::ComponentKind;
use ecs::prelude::*;
use math::{Quat, Vec3, vec3};

// ---------------------------------------------------------------------------
// Sample scripts
// ---------------------------------------------------------------------------

/// Spawns a child entity and positions it on start.
const SCRIPT_SPAWNER: &str = r#"
// spawner.js — spawn and place a new entity.
export function on_start(api) {
  const spawned = api.spawn("spawned-by-script");
  api.set_position(spawned, 1.0, 2.0, 3.0);
}

export function on_update(api) {
  // no per-frame work
}
"#;

/// Translates its own entity every frame and reads rotation/scale.
const SCRIPT_MOVER: &str = r#"
// mover.js — drift along +X and spin, scaled by frame delta.
export function on_update(api) {
  const self = api.current_entity();
  const dt = api.time_delta();
  api.translate(self, 1.0 * dt, 0.0, 0.0);
  const euler = api.get_rotation_euler(self);
  api.set_rotation_euler(self, euler.x, euler.y + dt, euler.z);
  const scale = api.get_scale(self);
  api.set_scale(self, scale.x, scale.y, scale.z);
}
"#;

/// Exercises the whole `ScriptApi` surface in one place.
const SCRIPT_FULL_API: &str = r#"
// full_api.js — touches every ScriptApi entry point.
export function on_start(api) {
  const target = api.find_entity("Ball");
  if (target !== null) {
    api.set_position(target, 0.0, 4.0, 0.0);
    api.translate(target, 0.0, 1.0, 0.0);
    api.set_rotation_euler(target, 0.0, 1.5, 0.0);
    const euler = api.get_rotation_euler(target);
    api.set_scale(target, 1.0, 2.0, 1.0);
    const scale = api.get_scale(target);
  }
}

export function on_update(api) {
  const self = api.current_entity();
  const dt = api.time_delta();
  if (api.key_down("Space")) {
    const spawned = api.spawn("jumper");
    api.add_script(spawned, "export function on_update(api) {}");
    api.destroy(spawned);
  }
  api.destroy_by_name("obsolete");
  api.instantiate_prefab("{}", self);
  if (api.is_alive(self)) {
    api.log("tick " + dt);
  }
}
"#;

/// The script sources the scene ships with, as `(file_name, source)` pairs.
///
/// They are stored as [`ScriptAsset`]s in the registry *and* attached to
/// entities via [`Script::from_path`] / [`Script::from_source`].
pub fn sample_scripts() -> Vec<(&'static str, &'static str)> {
    vec![
        ("spawner.js", SCRIPT_SPAWNER),
        ("mover.js", SCRIPT_MOVER),
        ("full_api.js", SCRIPT_FULL_API),
    ]
}

// ---------------------------------------------------------------------------
// Generated assets
// ---------------------------------------------------------------------------

/// A mono sine-wave [`AudioClip`].
fn sine_clip(name: &str, frequency: f32, seconds: f32, sample_rate: u32) -> AudioClip {
    let samples_len = (sample_rate as f32 * seconds).round() as usize;
    let mut samples = Vec::with_capacity(samples_len);
    for i in 0..samples_len {
        let t = i as f32 / sample_rate as f32;
        samples.push((TAU * frequency * t).sin() * 0.5);
    }
    AudioClip {
        name: name.to_string(),
        sample_rate,
        channels: 1,
        samples,
    }
}

/// A small equirectangular sky gradient (bright top, warm horizon).
fn sky_gradient(width: u32, height: u32) -> HdrTexture {
    let top = vec3(0.30, 0.55, 0.95);
    let horizon = vec3(1.00, 0.82, 0.62);
    let bottom = vec3(0.05, 0.06, 0.10);
    let mut data = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        let v = y as f32 / (height - 1).max(1) as f32;
        let color = if v <= 0.5 {
            let t = v * 2.0;
            vec3(
                math::lerp(top.x, horizon.x, t),
                math::lerp(top.y, horizon.y, t),
                math::lerp(top.z, horizon.z, t),
            )
        } else {
            let t = (v - 0.5) * 2.0;
            vec3(
                math::lerp(horizon.x, bottom.x, t),
                math::lerp(horizon.y, bottom.y, t),
                math::lerp(horizon.z, bottom.z, t),
            )
        };
        for _ in 0..width {
            data.push(color.x);
            data.push(color.y);
            data.push(color.z);
        }
    }
    HdrTexture {
        name: "env/sky".to_string(),
        width,
        height,
        data,
    }
}

/// A simple two-tone checker [`Texture`].
fn checker_texture(name: &str, cells: u32) -> Texture {
    let cells = cells.max(2);
    let size = 64u32;
    let cell = size / cells;
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let on = ((x / cell) + (y / cell)).is_multiple_of(2);
            let c = if on { 230u8 } else { 40u8 };
            data.extend_from_slice(&[c, c, c, 255]);
        }
    }
    Texture {
        name: name.to_string(),
        width: size,
        height: size,
        format: TextureFormat::Rgba8Srgb,
        data,
    }
}

// ---------------------------------------------------------------------------
// Scene construction
// ---------------------------------------------------------------------------

/// Spawn an entity carrying a [`Name`] and a [`LocalTransform`].
fn named(world: &mut World, name: &str, transform: Transform) -> Entity {
    let entity = world.spawn();
    world.insert(entity, Name::new(name));
    world.insert(entity, LocalTransform::from(transform));
    entity
}

/// A translation-only transform.
fn at(x: f32, y: f32, z: f32) -> Transform {
    Transform::from_translation(vec3(x, y, z))
}

/// Build the default all-features scene.
pub fn build() -> (Scene, AssetRegistry) {
    build_named("all-features")
}

/// Build the all-features scene with a custom name.
pub fn build_named(name: impl Into<String>) -> (Scene, AssetRegistry) {
    let mut assets = AssetRegistry::with_defaults();

    // -- mesh assets -------------------------------------------------------
    let mesh_cube = assets
        .meshes
        .insert_named("primitives/cube", primitives::cube(1.0));
    let mesh_plane = assets
        .meshes
        .insert_named("primitives/plane", primitives::plane(1.0, 4));
    let mesh_sphere = assets
        .meshes
        .insert_named("primitives/sphere", primitives::sphere(0.5, 24, 32));
    let mesh_cylinder = assets
        .meshes
        .insert_named("primitives/cylinder", primitives::cylinder(0.5, 1.0, 24));
    let mesh_capsule = assets
        .meshes
        .insert_named("primitives/capsule", primitives::capsule(0.4, 1.0, 24));
    let mesh_torus = assets
        .meshes
        .insert_named("primitives/torus", primitives::torus(1.0, 0.25, 32, 16));

    // -- textures and materials -------------------------------------------
    let checker = assets
        .textures
        .insert_named("textures/checker", checker_texture("checker", 8));

    let mat_opaque = assets.materials.insert_named(
        "materials/opaque",
        Material {
            name: "opaque".to_string(),
            base_color: [0.80, 0.80, 0.85, 1.0],
            metallic: 0.0,
            roughness: 0.6,
            ..Material::default()
        },
    );
    let mat_masked = assets.materials.insert_named(
        "materials/masked",
        Material {
            name: "masked".to_string(),
            base_color: [0.90, 0.35, 0.30, 1.0],
            roughness: 0.4,
            base_color_texture: checker,
            alpha_mode: AlphaMode::Mask,
            ..Material::default()
        },
    );
    let mat_blend = assets.materials.insert_named(
        "materials/blend",
        Material {
            name: "blend".to_string(),
            base_color: [0.25, 0.60, 0.95, 0.45],
            metallic: 0.1,
            roughness: 0.05,
            emissive: [0.05, 0.10, 0.20],
            emissive_strength: 0.5,
            alpha_mode: AlphaMode::Blend,
            double_sided: true,
            ..Material::default()
        },
    );

    // -- audio and scripts -------------------------------------------------
    let clip_beep = assets
        .audio
        .insert_named("audio/beep", sine_clip("beep", 440.0, 0.5, 44_100));
    for (file, source) in sample_scripts() {
        let path = format!("scripts/{file}");
        assets.scripts.insert_named(
            path.clone(),
            ScriptAsset {
                name: path,
                source: source.to_string(),
            },
        );
    }

    // -- environment -------------------------------------------------------
    assets.hdr.insert_named("env/sky", sky_gradient(64, 32));

    // =====================================================================
    // Entities
    // =====================================================================
    let mut scene = Scene::new(name);
    let world = &mut scene.world;

    // Transform hierarchy: root -> children -> grandchild, with a non-identity
    // rotation and non-uniform scale.
    let root = named(
        world,
        "Root",
        Transform {
            translation: vec3(0.0, 0.0, 0.0),
            rotation: Quat::from_euler_xyz(0.05, 0.35, 0.0),
            scale: vec3(1.2, 0.8, 1.0),
        },
    );
    let child_a = named(
        world,
        "ChildA",
        Transform {
            translation: vec3(2.0, 0.0, 0.0),
            rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_4),
            scale: vec3(0.5, 1.5, 0.5),
        },
    );
    world.insert(
        child_a,
        MeshRenderer {
            mesh: mesh_sphere,
            material: mat_opaque,
            cast_shadows: true,
            receive_shadows: true,
        },
    );
    set_parent(world, child_a, Some(root));

    let child_b = named(world, "ChildB", at(-2.0, 1.0, 0.0));
    world.insert(
        child_b,
        MeshRenderer {
            mesh: mesh_cylinder,
            material: mat_opaque,
            cast_shadows: true,
            receive_shadows: false,
        },
    );
    set_parent(world, child_b, Some(root));

    let grandchild = named(world, "Grandchild", at(0.0, 1.0, 0.0));
    world.insert(
        grandchild,
        MeshRenderer {
            mesh: mesh_cube,
            material: mat_masked,
            cast_shadows: false,
            receive_shadows: true,
        },
    );
    set_parent(world, grandchild, Some(child_a));

    // Visibility toggles: an invisible mesh and an inactive subtree.
    let hidden = named(world, "Hidden", at(4.0, 1.0, 0.0));
    world.insert(hidden, Visible(false));
    world.insert(
        hidden,
        MeshRenderer {
            mesh: mesh_cube,
            material: mat_opaque,
            ..MeshRenderer::default()
        },
    );

    let disabled = named(world, "Disabled", at(-4.0, 1.0, 0.0));
    world.insert(disabled, Active(false));
    world.insert(
        disabled,
        MeshRenderer {
            mesh: mesh_cube,
            material: mat_opaque,
            ..MeshRenderer::default()
        },
    );

    // Cameras: a perspective main camera and an orthographic preview.
    let eye = vec3(0.0, 4.0, 12.0);
    let target = vec3(0.0, 1.0, 0.0);
    let persp_forward = math::normalize(target - eye);
    let persp_cam = named(
        world,
        "PerspectiveCamera",
        Transform {
            translation: eye,
            rotation: Quat::look_rotation(persp_forward, vec3(0.0, 1.0, 0.0)),
            scale: vec3(1.0, 1.0, 1.0),
        },
    );
    world.insert(
        persp_cam,
        Camera {
            projection: Projection::Perspective {
                fov_y: math::radians(60.0),
                near: 0.1,
                far: 500.0,
            },
            is_active: true,
            priority: 0,
            clear_color: [0.02, 0.02, 0.03, 1.0],
            hdr: true,
        },
    );

    let ortho_cam = named(
        world,
        "OrthoCamera",
        Transform {
            translation: vec3(0.0, 20.0, 0.0),
            rotation: Quat::look_rotation(vec3(0.0, -1.0, 0.0), vec3(0.0, 0.0, -1.0)),
            scale: vec3(1.0, 1.0, 1.0),
        },
    );
    world.insert(
        ortho_cam,
        Camera {
            projection: Projection::Orthographic {
                height: 16.0,
                near: 0.1,
                far: 100.0,
            },
            is_active: true,
            priority: 10,
            clear_color: [0.0, 0.0, 0.0, 1.0],
            hdr: false,
        },
    );

    // Lights: directional (shadow caster), point, and spot.
    let sun = named(world, "Sun", at(0.0, 10.0, 4.0));
    world.insert(
        sun,
        Light {
            kind: LightKind::Directional {
                direction: math::normalize(vec3(-0.4, -1.0, -0.3)),
            },
            color: vec3(1.0, 0.97, 0.9),
            intensity: 3.0,
            cast_shadows: true,
            shadow_bias: 0.001,
        },
    );

    let point_light = named(world, "PointLight", at(0.0, 6.0, 0.0));
    world.insert(point_light, Light::point(vec3(0.9, 0.5, 0.2), 12.0, 25.0));

    let spot_light = named(world, "SpotLight", at(4.0, 6.0, 4.0));
    world.insert(
        spot_light,
        Light {
            kind: LightKind::Spot {
                direction: math::normalize(vec3(-0.6, -1.0, -0.6)),
                inner_angle: math::radians(18.0),
                outer_angle: math::radians(28.0),
                range: 30.0,
            },
            color: vec3(0.6, 0.8, 1.0),
            intensity: 20.0,
            cast_shadows: true,
            shadow_bias: 0.002,
        },
    );

    // Renderable props across the primitive/material/casting matrix.
    let props: [(&str, usize, usize, bool, bool); 6] = [
        ("CubeProp", 0, 0, true, true),
        ("PlaneProp", 1, 0, false, true),
        ("MaskedProp", 2, 1, true, true),
        ("GlassProp", 5, 2, false, false),
        ("CapsuleProp", 4, 0, false, true),
        ("TorusProp", 5, 0, true, true),
    ];
    let meshes = [
        mesh_cube,
        mesh_plane,
        mesh_sphere,
        mesh_cylinder,
        mesh_capsule,
        mesh_torus,
    ];
    let materials = [mat_opaque, mat_masked, mat_blend];
    for (i, (prop_name, mesh_idx, mat_idx, cast, receive)) in props.into_iter().enumerate() {
        let x = -3.0 + i as f32 * 1.5;
        let entity = named(world, prop_name, at(x, 0.6, -2.0));
        world.insert(
            entity,
            MeshRenderer {
                mesh: meshes[mesh_idx],
                material: materials[mat_idx],
                cast_shadows: cast,
                receive_shadows: receive,
            },
        );
    }

    // -- physics sandbox ---------------------------------------------------
    let floor = named(world, "Floor", at(0.0, -0.5, 0.0));
    world.insert(
        floor,
        RigidBody {
            body_type: BodyType::Fixed,
            ..RigidBody::default()
        },
    );
    world.insert(
        floor,
        Collider {
            shape: ColliderShape::Cuboid {
                half_extents: vec3(10.0, 0.5, 10.0),
            },
            ..Collider::default()
        },
    );
    world.insert(
        floor,
        MeshRenderer {
            mesh: mesh_plane,
            material: mat_opaque,
            cast_shadows: false,
            receive_shadows: true,
        },
    );

    let spawn_dynamic =
        |world: &mut World,
         name: &str,
         position: Vec3,
         shape: ColliderShape,
         mesh: ecs::assets::AssetHandle<ecs::assets::Mesh>| {
            let entity = named(world, name, at(position.x, position.y, position.z));
            world.insert(
                entity,
                RigidBody {
                    body_type: BodyType::Dynamic,
                    mass: 1.0,
                    restitution: 0.2,
                    friction: 0.6,
                    ..RigidBody::default()
                },
            );
            world.insert(
                entity,
                Collider {
                    shape,
                    ..Collider::default()
                },
            );
            world.insert(
                entity,
                MeshRenderer {
                    mesh,
                    material: mat_opaque,
                    cast_shadows: true,
                    receive_shadows: true,
                },
            );
        };

    spawn_dynamic(
        world,
        "Ball",
        vec3(0.0, 4.0, 0.0),
        ColliderShape::Ball { radius: 0.5 },
        mesh_sphere,
    );
    spawn_dynamic(
        world,
        "BoxBody",
        vec3(1.5, 6.0, 0.0),
        ColliderShape::Cuboid {
            half_extents: vec3(0.5, 0.5, 0.5),
        },
        mesh_cube,
    );
    spawn_dynamic(
        world,
        "CapsuleBody",
        vec3(-1.5, 8.0, 0.0),
        ColliderShape::Capsule {
            half_height: 0.5,
            radius: 0.4,
        },
        mesh_capsule,
    );
    spawn_dynamic(
        world,
        "CylinderBody",
        vec3(0.0, 10.0, 1.5),
        ColliderShape::Cylinder {
            half_height: 0.5,
            radius: 0.5,
        },
        mesh_cylinder,
    );

    // A non-resolving sensor volume.
    let sensor = named(world, "Sensor", at(3.0, 2.0, 0.0));
    world.insert(
        sensor,
        RigidBody {
            body_type: BodyType::Fixed,
            ..RigidBody::default()
        },
    );
    world.insert(
        sensor,
        Collider {
            shape: ColliderShape::Ball { radius: 1.0 },
            is_sensor: true,
            ..Collider::default()
        },
    );

    // -- audio -------------------------------------------------------------
    let spatial_sound = named(world, "SpatialSound", at(3.0, 1.0, 0.0));
    world.insert(
        spatial_sound,
        AudioSource {
            clip: clip_beep,
            volume: 0.7,
            pitch: 1.0,
            looped: true,
            playing: true,
            spatial: true,
            max_distance: 30.0,
            rolloff: 1.0,
        },
    );

    let ui_sound = named(world, "UiSound", at(0.0, 0.0, 0.0));
    world.insert(
        ui_sound,
        AudioSource {
            clip: clip_beep,
            volume: 0.4,
            pitch: 1.25,
            looped: false,
            playing: true,
            spatial: false,
            max_distance: 50.0,
            rolloff: 1.0,
        },
    );

    // -- scripts -----------------------------------------------------------
    let script_inline = named(world, "ScriptInline", at(-1.0, 0.0, 3.0));
    world.insert(script_inline, Script::from_source(SCRIPT_FULL_API));

    let script_by_path = named(world, "ScriptByPath", at(0.0, 0.0, 3.0));
    world.insert(script_by_path, Script::from_path("scripts/spawner.js"));

    let script_mover = named(world, "ScriptMover", at(1.0, 0.0, 3.0));
    world.insert(script_mover, Script::from_path("scripts/mover.js"));

    // -- prefab ------------------------------------------------------------
    // Build a small prop subtree, turn it into a prefab, and re-instantiate it
    // under the hierarchy root.
    let mut prop = Scene::new("Prop");
    let prop_root = {
        let e = prop.world.spawn();
        prop.world.insert(e, Name::new("PropRoot"));
        prop.world
            .insert(e, LocalTransform::from_translation(vec3(0.0, 0.0, 0.0)));
        prop.world.insert(
            e,
            MeshRenderer {
                mesh: mesh_cube,
                material: mat_opaque,
                ..MeshRenderer::default()
            },
        );
        e
    };
    let prop_child = {
        let e = prop.world.spawn();
        prop.world.insert(e, Name::new("PropChild"));
        prop.world
            .insert(e, LocalTransform::from_translation(vec3(0.0, 1.5, 0.0)));
        prop.world.insert(
            e,
            MeshRenderer {
                mesh: mesh_torus,
                material: mat_blend,
                ..MeshRenderer::default()
            },
        );
        e
    };
    set_parent(&mut prop.world, prop_child, Some(prop_root));
    let prefab = Prefab::from_scene_desc("Prop", prop.to_desc(&assets));
    prefab.instantiate(world, &assets, Some(root));

    scene.update_transforms();
    (scene, assets)
}

/// The names of every [`ComponentKind`] present in the scene built by [`build`].
pub fn coverage() -> Vec<&'static str> {
    let (scene, _assets) = build();
    coverage_of(&scene.world)
}

/// The names of every [`ComponentKind`] present in `world`.
pub fn coverage_of(world: &World) -> Vec<&'static str> {
    let mut present = Vec::new();
    for kind in ComponentKind::ALL {
        let has = match kind {
            ComponentKind::LocalTransform => world.count::<LocalTransform>() > 0,
            ComponentKind::GlobalTransform => world.count::<GlobalTransform>() > 0,
            ComponentKind::Name => world.count::<Name>() > 0,
            ComponentKind::Active => world.count::<Active>() > 0,
            ComponentKind::Visible => world.count::<Visible>() > 0,
            ComponentKind::Camera => world.count::<Camera>() > 0,
            ComponentKind::Light => world.count::<Light>() > 0,
            ComponentKind::MeshRenderer => world.count::<MeshRenderer>() > 0,
            ComponentKind::RigidBody => world.count::<RigidBody>() > 0,
            ComponentKind::Collider => world.count::<Collider>() > 0,
            ComponentKind::AudioSource => world.count::<AudioSource>() > 0,
            ComponentKind::Script => world.count::<Script>() > 0,
        };
        if has {
            present.push(kind.name());
        }
    }
    present
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_populates_the_registry() {
        let (_scene, assets) = build();
        assert!(assets.meshes.handle("primitives/cube").is_some());
        assert!(assets.meshes.handle("primitives/torus").is_some());
        assert!(assets.materials.handle("materials/opaque").is_some());
        assert!(assets.audio.handle("audio/beep").is_some());
        assert!(assets.scripts.handle("scripts/spawner.js").is_some());
        assert!(assets.hdr.handle("env/sky").is_some());
    }

    #[test]
    fn coverage_covers_every_component_kind() {
        let present = coverage();
        for kind in ComponentKind::ALL {
            assert!(
                present.contains(&kind.name()),
                "scene is missing component kind {}",
                kind.name()
            );
        }
    }

    #[test]
    fn generated_clip_has_duration() {
        let clip = sine_clip("sine", 440.0, 0.25, 8_000);
        assert!(clip.duration() > 0.2 && clip.duration() < 0.3);
        assert!(clip.samples.iter().all(|s| s.abs() <= 0.5 + 1e-6));
    }

    #[test]
    fn sky_gradient_has_expected_size() {
        let hdr = sky_gradient(8, 4);
        assert_eq!(hdr.data.len(), (8 * 4 * 3) as usize);
        assert_eq!(hdr.width, 8);
        assert_eq!(hdr.height, 4);
    }

    #[test]
    fn sample_scripts_are_non_empty() {
        let scripts = sample_scripts();
        assert_eq!(scripts.len(), 3);
        for (file, source) in scripts {
            assert!(file.ends_with(".js"));
            assert!(source.contains("function"));
        }
    }
}
