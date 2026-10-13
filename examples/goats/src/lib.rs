//! Goats — a Noctis port of the Slag Goat sandbox (`goats/`).
//!
//! **M0**, the render-path spike (see `.notes/goats.md`): the goat model loads as
//! a static mesh at bind pose, the ground is a mesh built from the original's own
//! noise function ([`terrain`]), and an orbit camera frames it. There is no
//! simulation and no animation yet — those are M1 and `.notes/animation.md`.

#![forbid(unsafe_code)]

mod terrain;

use noctis::prelude::*;

/// The goat model, compiled in — the original embeds it with `include_bytes!`
/// too, so the game needs no files on disk.
const GOAT_GLB: &[u8] = include_bytes!("../content/goat_animated.glb");

/// The ground's colour: the original's `T_GRASS` (`98,150,84` sRGB) in linear.
const GRASS: [f32; 4] = [0.122, 0.311, 0.087, 1.0];

/// Height above the goat's feet that the camera orbits and looks at.
const EYE_HEIGHT: f32 = 0.75;
/// Radians the camera turns per logical point of drag.
const LOOK_SPEED: f32 = 0.006;
/// Radians per second the arrow keys turn the camera.
const KEY_LOOK_SPEED: f32 = 1.8;
/// World units the wheel dollies per point of scroll.
const ZOOM_SPEED: f32 = 0.6;
/// The nearest the camera may orbit.
const MIN_DIST: f32 = 2.5;
/// The farthest the camera may orbit.
const MAX_DIST: f32 = 40.0;
/// The pitch limit, just shy of straight up/down so the up vector stays defined.
const MAX_PITCH: f32 = 1.35;
/// The other end of the pitch limit.
const MIN_PITCH: f32 = -1.35;

/// The orbit camera's state, about the goat.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Orbit {
    /// Rotation about the world up axis, radians.
    yaw: f32,
    /// Elevation, radians (negative looks down).
    pitch: f32,
    /// Distance from the target.
    dist: f32,
}

impl Default for Orbit {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: -0.32,
            dist: 9.0,
        }
    }
}

impl Orbit {
    /// The camera position and orientation for this orbit about `target`. The
    /// camera looks at the target (its `-Z` points from the camera to it).
    fn view(&self, target: Vec3) -> (Vec3, Quat) {
        let offset = math::vec3(
            self.pitch.cos() * self.yaw.sin(),
            -self.pitch.sin(),
            self.pitch.cos() * self.yaw.cos(),
        ) * self.dist;
        let position = target + offset;
        let rotation = (Quat::from_rotation_y(self.yaw)
            * Quat::from_axis_angle(math::vec3(1.0, 0.0, 0.0), self.pitch))
        .normalized();
        (position, rotation)
    }

    /// Recover an orbit from a camera placed at `position` looking at `target`,
    /// so a scene's authored view is honoured on the first frame.
    fn from_view(position: Vec3, target: Vec3) -> Self {
        let offset = position - target;
        let dist = math::length(offset).clamp(MIN_DIST, MAX_DIST);
        let pitch = (-offset.y / dist).clamp(-1.0, 1.0).asin();
        Self {
            yaw: offset.x.atan2(offset.z),
            pitch: pitch.clamp(MIN_PITCH, MAX_PITCH),
            dist,
        }
    }
}

/// The `-1..=1` reading of a key pair (`positive` minus `negative`).
fn axis(positive: bool, negative: bool) -> f32 {
    (i32::from(positive) - i32::from(negative)) as f32
}

/// The world's active camera, if any.
fn active_camera(ctx: &EngineContext) -> Option<Entity> {
    ctx.scene
        .world
        .query::<Camera>()
        .find(|(_, camera)| camera.is_active)
        .map(|(entity, _)| entity)
}

/// The project's plugin. `#[noctis::plugin]` exports the `noctis_plugin` symbol
/// the tool links — a `Cargo.toml` at the project root is all `noctis` needs to
/// find it.
#[noctis::plugin]
#[derive(Default)]
pub struct Goats;

impl Plugin for Goats {
    fn build(&self, app: &mut App) {
        app.on_startup("goats.spawn", spawn_world);

        let mut orbit = Orbit::default();
        let mut seeded = false;
        app.on_update("goats.frame", move |ctx| {
            let target = math::vec3(0.0, terrain::height(0.0, 0.0) + EYE_HEIGHT, 0.0);
            let Some(camera) = active_camera(ctx) else {
                return;
            };

            // Seed from the scene's view once, so the camera starts where the
            // scene placed it instead of snapping to the default orbit.
            if !seeded && let Some(transform) = ctx.scene.world.get::<LocalTransform>(camera) {
                orbit = Orbit::from_view(transform.translation, target);
                seeded = true;
            }

            let dt = ctx.time.delta();

            // Drag (left or middle button) orbits.
            if ctx.input.mouse.is_down(MouseButton::Left)
                || ctx.input.mouse.is_down(MouseButton::Middle)
            {
                let [dx, dy] = ctx.input.mouse.delta;
                orbit.yaw -= dx * LOOK_SPEED;
                orbit.pitch = (orbit.pitch - dy * LOOK_SPEED).clamp(MIN_PITCH, MAX_PITCH);
            }

            // The arrow keys orbit too, as a keyboard fallback.
            let turn = axis(
                ctx.input.key_down(Key::ArrowRight),
                ctx.input.key_down(Key::ArrowLeft),
            );
            let tilt = axis(
                ctx.input.key_down(Key::ArrowDown),
                ctx.input.key_down(Key::ArrowUp),
            );
            orbit.yaw += turn * KEY_LOOK_SPEED * dt;
            orbit.pitch = (orbit.pitch + tilt * KEY_LOOK_SPEED * dt).clamp(MIN_PITCH, MAX_PITCH);

            // The wheel dollies.
            orbit.dist =
                (orbit.dist - ctx.input.mouse.scroll * ZOOM_SPEED).clamp(MIN_DIST, MAX_DIST);

            let (position, rotation) = orbit.view(target);
            if let Some(transform) = ctx.scene.world.get_mut::<LocalTransform>(camera) {
                transform.translation = position;
                transform.rotation = rotation;
            }

            let fps = if dt > 0.0 { 1.0 / dt } else { 0.0 };
            ctx.screen_text = vec![format!(
                "Goats M0  |  {fps:.0} fps  |  {:.2} ms  |  camera dist {:.1}",
                dt * 1000.0,
                orbit.dist,
            )];
        });
    }
}

/// Spawn the world's contents: the ground, then the goat.
fn spawn_world(ctx: &mut EngineContext) {
    spawn_terrain(ctx);
    spawn_goat(ctx);
}

/// Build the ground mesh and put it in the scene under one entity.
fn spawn_terrain(ctx: &mut EngineContext) {
    let mesh = ctx
        .assets
        .meshes
        .insert_named("terrain/meadow", terrain::build_mesh());
    let material = ctx.assets.materials.insert_named(
        "terrain/grass",
        Material {
            name: "terrain grass".to_string(),
            base_color: GRASS,
            metallic: 0.0,
            roughness: 0.95,
            ..Material::default()
        },
    );

    let ground = ctx.scene.world.spawn();
    ctx.scene.world.insert(ground, Name::new("Terrain"));
    ctx.scene.world.insert(ground, LocalTransform::default());
    ctx.scene.world.insert(
        ground,
        MeshRenderer {
            mesh,
            material,
            cast_shadows: false,
            receive_shadows: true,
        },
    );
}

/// Import the goat model and stand it on the ground at the origin.
///
/// The model is drawn at its bind pose (no skinning yet — `.notes/animation.md`),
/// so its mesh bounds are the drawn bounds; the roots are lifted so the lowest
/// vertex rests at the terrain height under the origin.
fn spawn_goat(ctx: &mut EngineContext) {
    let loaded = match load_gltf_bytes(GOAT_GLB) {
        Ok(loaded) => loaded,
        Err(error) => {
            log::error!("could not import the goat model: {error}");
            return;
        }
    };
    let handles = loaded.register(&mut ctx.assets);
    let nodes = loaded.instantiate(&mut ctx.scene.world, &handles);

    let min_y = loaded
        .meshes
        .iter()
        .filter_map(|mesh| mesh.bounds.as_ref())
        .map(|bounds| bounds.min.y)
        .fold(f32::INFINITY, f32::min);
    let lift = if min_y.is_finite() {
        terrain::height(0.0, 0.0) - min_y
    } else {
        0.0
    };
    for &root in &loaded.roots {
        if let Some(&entity) = nodes.get(root)
            && let Some(transform) = ctx.scene.world.get_mut::<LocalTransform>(entity)
        {
            transform.translation = transform.translation + math::vec3(0.0, lift, 0.0);
        }
    }

    log::info!(
        "goat: {} mesh(es), {} node(s), {} triangle(s), lifted {lift:.3}",
        loaded.meshes.len(),
        loaded.nodes.len(),
        loaded.triangle_count(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_orbit_round_trips_through_a_view() {
        let target = math::vec3(0.0, EYE_HEIGHT, 0.0);
        let orbit = Orbit {
            yaw: 0.6,
            pitch: -0.4,
            dist: 8.0,
        };
        let (position, _) = orbit.view(target);
        let seeded = Orbit::from_view(position, target);
        assert!((seeded.yaw - orbit.yaw).abs() < 1e-3);
        assert!((seeded.pitch - orbit.pitch).abs() < 1e-3);
        assert!((seeded.dist - orbit.dist).abs() < 1e-3);
    }

    #[test]
    fn the_camera_looks_at_its_target() {
        let target = math::vec3(0.0, EYE_HEIGHT, 0.0);
        let (position, rotation) = Orbit::default().view(target);
        // The camera's forward (`-Z`) points from it toward the target.
        let forward = rotation.rotate_vec3(math::vec3(0.0, 0.0, -1.0));
        let to_target = math::normalize(target - position);
        assert!(math::length(forward - to_target) < 1e-3);
    }

    #[test]
    fn the_goat_model_imports() {
        let loaded = load_gltf_bytes(GOAT_GLB).expect("the embedded goat imports");
        assert!(!loaded.meshes.is_empty(), "the goat has geometry");
        assert!(loaded.triangle_count() > 0);
        assert!(!loaded.materials.is_empty(), "the goat has materials");
        assert!(loaded.meshes.iter().any(|mesh| mesh.bounds.is_some()));
    }

    #[test]
    fn the_goat_model_carries_its_skin_and_clips() {
        let loaded = load_gltf_bytes(GOAT_GLB).expect("the embedded goat imports");

        // One rig, with a bind transform per joint and one influence per vertex.
        assert_eq!(loaded.skins.len(), 1, "the goat has one rig");
        let skin = &loaded.skins[0];
        assert!(skin.joints.len() >= 10, "the rig has real bones");
        assert_eq!(skin.inverse_bind.len(), skin.joints.len());
        assert!(
            loaded
                .meshes
                .iter()
                .any(|mesh| !mesh.influences.is_empty()
                    && mesh.influences.len() == mesh.vertices.len()),
            "a skinned primitive carries one influence per vertex",
        );

        // The fourteen clips the original's `model.js` looks up by name.
        let names: Vec<&str> = loaded.animations.iter().map(|c| c.name.as_str()).collect();
        for wanted in [
            "GoatIdle",
            "GoatIdle2",
            "GoatIdle3",
            "GoatWalk",
            "GoatTrot",
            "GoatRun",
            "GoatJump",
            "GoatJump2",
            "GoatFlung",
            "GoatSleep",
            "GoatSleep2",
            "GoatDeath",
            "GoatEat",
            "GoatEat2",
        ] {
            assert!(names.contains(&wanted), "missing clip {wanted}: {names:?}");
        }
        assert!(loaded.animations.iter().all(|clip| clip.duration > 0.0));
    }

    #[test]
    fn the_plugin_spawns_the_goat_and_the_terrain() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        app.tick(1.0 / 60.0);

        let world = &app.context.scene.world;
        assert!(
            world
                .query::<Name>()
                .any(|(_, name)| name.as_str() == "Terrain"),
            "the terrain entity was spawned"
        );
        // The terrain is a plain mesh; the goat's primitives are skinned.
        assert!(world.count::<MeshRenderer>() >= 1);
        assert!(world.count::<SkinnedMeshRenderer>() >= 1);
        assert!(app.context.assets.meshes.handle("terrain/meadow").is_some());
    }
}
