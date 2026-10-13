//! Goats — a Noctis port of the Slag Goat sandbox (`goats/`).
//!
//! The goat model loads as a skinned mesh, the ground is a mesh built from the
//! original's own noise function ([`terrain`]), and the goat walks the meadow
//! under an orbit camera. Locomotion is the first slice of M1 (see
//! `.notes/goats.md`): a gait chosen from input drives a baked clip through the
//! engine's `Animator`, and the root moves at that gait's authored speed.

#![forbid(unsafe_code)]

mod terrain;

use noctis::prelude::*;

/// The goat model, compiled in — the original embeds it with `include_bytes!`
/// too, so the game needs no files on disk.
const GOAT_GLB: &[u8] = include_bytes!("../content/goat_animated.glb");

/// The ground's colour: the original's `T_GRASS` (`98,150,84` sRGB) in linear.
const GRASS: [f32; 4] = [0.122, 0.311, 0.087, 1.0];

/// How far the goat may walk from the origin before the mesh ends, in units.
const FIELD: f32 = 44.0;
/// The gaits' ground speeds, in metres per second (the original's stride maths).
const WALK_SPEED: f32 = 0.87;
const TROT_SPEED: f32 = 1.58;
const RUN_SPEED: f32 = 2.94;
/// The rate a reversing walk plays at (the clip runs backwards).
const BACK_SPEED: f32 = -0.9;
/// Radians the goat turns per second on `A`/`D`.
const TURN_RATE: f32 = 2.6;

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
/// The pitch limits, just shy of vertical so the up vector stays defined.
const MAX_PITCH: f32 = 1.35;
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

/// The spawned goat: its root, its resting height offset, and its gait clips.
struct Goat {
    /// The entity that carries the whole imported model.
    root: Entity,
    /// The entity the [`Animator`] plays on (the skinned mesh child).
    skinned: Entity,
    /// `-model_min_y`: added to the terrain height so the feet rest on the ground.
    lift: f32,
    /// Ground position.
    x: f32,
    z: f32,
    /// Facing, radians about `+Y`.
    yaw: f32,
    /// The gait clips, by intent.
    idle: AssetHandle<AnimationClip>,
    walk: AssetHandle<AnimationClip>,
    trot: AssetHandle<AnimationClip>,
    run: AssetHandle<AnimationClip>,
}

impl Goat {
    /// The clip to play, the ground speed, and whether the goat is reversing.
    fn gait(&self, input: &InputState) -> (AssetHandle<AnimationClip>, f32, bool) {
        let down = |c: char| Key::letter(c).is_some_and(|key| input.key_down(key));
        let reversing = down('S');
        if !down('W') && !reversing {
            return (self.idle, 0.0, false);
        }
        // `Shift` runs, `Ctrl` trots, otherwise a walk (the original's modifiers).
        let (clip, speed) = if input.key_down(Key::ShiftLeft) || input.key_down(Key::ShiftRight) {
            (self.run, RUN_SPEED)
        } else if input.key_down(Key::ControlLeft) || input.key_down(Key::ControlRight) {
            (self.trot, TROT_SPEED)
        } else {
            (self.walk, WALK_SPEED)
        };
        (clip, speed, reversing)
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
        let mut goats: Option<Goat> = None;
        let mut orbit = Orbit::default();
        let mut seeded = false;

        app.on_update("goats.frame", move |ctx| {
            // The world is built on the first tick, so the closure owns the goat.
            if goats.is_none() {
                goats = spawn_world(ctx);
            }
            if let Some(goat) = goats.as_mut() {
                drive_goat(ctx, goat);
            }

            let target = goats
                .as_ref()
                .map(|goat| {
                    math::vec3(
                        goat.x,
                        terrain::height(goat.x, goat.z) + goat.lift + EYE_HEIGHT,
                        goat.z,
                    )
                })
                .unwrap_or_else(|| math::vec3(0.0, EYE_HEIGHT, 0.0));
            drive_camera(ctx, &mut orbit, &mut seeded, target);

            let dt = ctx.time.delta();
            let fps = if dt > 0.0 { 1.0 / dt } else { 0.0 };
            ctx.screen_text = vec![format!(
                "Goats  |  {fps:.0} fps  |  {:.2} ms  |  W walk, Ctrl trot, Shift run, A/D turn",
                dt * 1000.0
            )];
        });
    }
}

/// Move the goat one frame: turn, choose a gait, step the root and play the clip.
fn drive_goat(ctx: &mut EngineContext, goat: &mut Goat) {
    let dt = ctx.time.delta();
    let turn = axis(
        ctx.input
            .key_down(Key::letter('D').expect("D is a letter key")),
        ctx.input
            .key_down(Key::letter('A').expect("A is a letter key")),
    );
    goat.yaw += turn * TURN_RATE * dt;

    let (clip, speed, reversing) = goat.gait(&ctx.input);
    let direction = if reversing { -1.0 } else { 1.0 };
    // The model faces its local `+X`, rotated about `+Y` by yaw.
    let forward_x = goat.yaw.cos();
    let forward_z = -goat.yaw.sin();
    goat.x = (goat.x + forward_x * speed * direction * dt).clamp(-FIELD, FIELD);
    goat.z = (goat.z + forward_z * speed * direction * dt).clamp(-FIELD, FIELD);
    let y = terrain::height(goat.x, goat.z) + goat.lift;

    if let Some(transform) = ctx.scene.world.get_mut::<LocalTransform>(goat.root) {
        transform.translation = math::vec3(goat.x, y, goat.z);
        transform.rotation = Quat::from_rotation_y(goat.yaw);
    }
    if let Some(animator) = ctx.scene.world.get_mut::<Animator>(goat.skinned) {
        if animator.clip != clip {
            animator.clip = clip;
            animator.time = 0.0;
        }
        animator.speed = if reversing { BACK_SPEED } else { 1.0 };
    }
}

/// Advance the orbit camera about `target` from player input.
fn drive_camera(ctx: &mut EngineContext, orbit: &mut Orbit, seeded: &mut bool, target: Vec3) {
    let Some(camera) = active_camera(ctx) else {
        return;
    };
    if !*seeded && let Some(transform) = ctx.scene.world.get::<LocalTransform>(camera) {
        *orbit = Orbit::from_view(transform.translation, target);
        *seeded = true;
    }

    let dt = ctx.time.delta();
    if ctx.input.mouse.is_down(MouseButton::Left) || ctx.input.mouse.is_down(MouseButton::Middle) {
        let [dx, dy] = ctx.input.mouse.delta;
        orbit.yaw -= dx * LOOK_SPEED;
        orbit.pitch = (orbit.pitch - dy * LOOK_SPEED).clamp(MIN_PITCH, MAX_PITCH);
    }
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
    orbit.dist = (orbit.dist - ctx.input.mouse.scroll * ZOOM_SPEED).clamp(MIN_DIST, MAX_DIST);

    let (position, rotation) = orbit.view(target);
    if let Some(transform) = ctx.scene.world.get_mut::<LocalTransform>(camera) {
        transform.translation = position;
        transform.rotation = rotation;
    }
}

/// Build the whole world: the ground, then the goat.
fn spawn_world(ctx: &mut EngineContext) -> Option<Goat> {
    spawn_terrain(ctx);
    spawn_goat(ctx)
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

/// Import the goat model, stand it on the ground, and give it its gait clips.
fn spawn_goat(ctx: &mut EngineContext) -> Option<Goat> {
    let loaded = match load_gltf_bytes(GOAT_GLB) {
        Ok(loaded) => loaded,
        Err(error) => {
            log::error!("could not import the goat model: {error}");
            return None;
        }
    };
    let handles = loaded.register(&mut ctx.assets);
    let nodes = loaded.instantiate(&mut ctx.scene.world, &handles);

    // The model's lowest vertex, so the root can be lifted to rest on the ground.
    let min_y = loaded
        .meshes
        .iter()
        .filter_map(|mesh| mesh.bounds.as_ref())
        .map(|bounds| bounds.min.y)
        .fold(f32::INFINITY, f32::min);
    let lift = if min_y.is_finite() { -min_y } else { 0.0 };

    // A root entity carries the whole imported hierarchy, so the goat moves as one.
    let root = ctx.scene.world.spawn();
    ctx.scene.world.insert(root, Name::new("Goat Root"));
    ctx.scene.world.insert(
        root,
        LocalTransform::from_translation(math::vec3(0.0, terrain::height(0.0, 0.0) + lift, 0.0)),
    );
    for &node in &loaded.roots {
        if let Some(&entity) = nodes.get(node) {
            set_parent(&mut ctx.scene.world, entity, Some(root));
        }
    }

    let skinned = ctx
        .scene
        .world
        .query::<SkinnedMeshRenderer>()
        .map(|(entity, _)| entity)
        .next();
    let Some(skinned) = skinned else {
        log::warn!("the goat model has no skinned mesh; it will not animate");
        return None;
    };

    let idle = clip(&ctx.assets, "GoatIdle");
    let walk = clip(&ctx.assets, "GoatWalk");
    let trot = clip(&ctx.assets, "GoatTrot");
    let run = clip(&ctx.assets, "GoatRun");
    ctx.scene.world.insert(skinned, Animator::new(idle));

    log::info!(
        "goat: {} mesh(es), {} node(s), {} triangle(s), lifted {lift:.3}",
        loaded.meshes.len(),
        loaded.nodes.len(),
        loaded.triangle_count(),
    );
    Some(Goat {
        root,
        skinned,
        lift,
        x: 0.0,
        z: 0.0,
        yaw: 0.0,
        idle,
        walk,
        trot,
        run,
    })
}

/// The clip registered under `name`, or the empty handle.
fn clip(assets: &AssetRegistry, name: &str) -> AssetHandle<AnimationClip> {
    assets.animations.handle(name).unwrap_or_default()
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
        // The terrain is a plain mesh; the goat is skinned and plays a clip.
        assert!(world.count::<MeshRenderer>() >= 1);
        assert!(world.count::<SkinnedMeshRenderer>() >= 1);
        assert!(world.count::<Animator>() >= 1, "the goat plays a clip");
        assert!(app.context.assets.meshes.handle("terrain/meadow").is_some());
    }

    #[test]
    fn holding_w_walks_the_goat_forward() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        app.tick(1.0 / 60.0); // build the world

        let root = app
            .context
            .scene
            .world
            .query::<Name>()
            .find(|(_, name)| name.as_str() == "Goat Root")
            .map(|(entity, _)| entity)
            .expect("the goat root");
        let before = app
            .context
            .scene
            .world
            .get::<LocalTransform>(root)
            .expect("a transform")
            .translation;
        let idle = app
            .context
            .scene
            .world
            .query::<Animator>()
            .map(|(_, animator)| animator.clip)
            .next()
            .expect("an animator");

        app.context
            .input
            .press_key(Key::letter('W').expect("W is a letter key"));
        for _ in 0..30 {
            app.tick(1.0 / 60.0);
        }

        let after = app
            .context
            .scene
            .world
            .get::<LocalTransform>(root)
            .expect("a transform")
            .translation;
        assert!(
            after.x > before.x + 0.1,
            "the goat walked forward: {before:?} -> {after:?}"
        );
        // The gait switched off idle and is playing forwards. (The engine's
        // sampler, which advances `time`, is the runtime's, not this bare `App`.)
        let animator = app
            .context
            .scene
            .world
            .query::<Animator>()
            .map(|(_, animator)| *animator)
            .next()
            .expect("an animator");
        assert!(animator.clip != idle, "the gait left idle: {animator:?}");
        assert!(
            animator.speed.abs() > 0.0,
            "a gait is playing: {animator:?}"
        );
    }
}
