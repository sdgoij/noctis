//! Goats — a Noctis port of the Slag Goat sandbox (`goats/`).
//!
//! The goat model loads as a skinned mesh, the ground is a mesh built from the
//! original's own noise function ([`terrain`]), and the goat walks the meadow
//! under an orbit camera. This is M1 of the port (see `.notes/goats.md`): the
//! goat's locomotion state machine — idle, its three gaits, a jump and sleep —
//! driven from input, where each state plays a baked clip through the engine's
//! `Animator` and the root moves at the state's authored speed.

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

/// The goat's locomotion state — the original's `mode`.
///
/// The original also has `flung`, `eat` and `dead`; those arrive with the
/// milestones that cause them — the blasts, the food and the energy system
/// (`.notes/goats.md` M2/M3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Idle,
    Walk,
    Trot,
    Run,
    Jump,
    Sleep,
}

impl Mode {
    /// Whether the state's clip is a cycle, or a one-shot the engine holds on
    /// its last frame (a jump has to land).
    fn loops(self) -> bool {
        self != Mode::Jump
    }

    /// A short label for the HUD.
    fn label(self) -> &'static str {
        match self {
            Mode::Idle => "standing",
            Mode::Walk => "walking",
            Mode::Trot => "trotting",
            Mode::Run => "running",
            Mode::Jump => "jumping",
            Mode::Sleep => "sleeping (Z to wake)",
        }
    }
}

/// The goat's clips by role. The gaits are one clip each; idle, jump and sleep
/// have several takes the goat cycles through, exactly as the original's
/// `VARIANTS` do.
#[derive(Default)]
struct Clips {
    idle: Vec<AssetHandle<AnimationClip>>,
    walk: AssetHandle<AnimationClip>,
    trot: AssetHandle<AnimationClip>,
    run: AssetHandle<AnimationClip>,
    jump: Vec<AssetHandle<AnimationClip>>,
    sleep: Vec<AssetHandle<AnimationClip>>,
}

impl Clips {
    /// The clip for `mode`, choosing `variant` among a role's takes and falling
    /// back to the walk for any role the model lacks (the original's `clipRole`
    /// falls back the same way).
    fn pick(&self, mode: Mode, variant: i32) -> AssetHandle<AnimationClip> {
        match mode {
            Mode::Idle => at(&self.idle, variant),
            Mode::Walk => Some(self.walk),
            Mode::Trot => Some(self.trot),
            Mode::Run => Some(self.run),
            Mode::Jump => at(&self.jump, variant),
            Mode::Sleep => at(&self.sleep, variant),
        }
        .unwrap_or(self.walk)
    }
}

/// The `variant`-th handle in `list`, wrapping negatives like the original's
/// `clipAt` (`((v % n) + n) % n`), or `None` when the list is empty.
fn at(list: &[AssetHandle<AnimationClip>], variant: i32) -> Option<AssetHandle<AnimationClip>> {
    let n = list.len() as i32;
    (n > 0).then(|| list[((variant % n + n) % n) as usize])
}

/// The next take after `current` among `count`, or `current` when there is
/// nothing to cycle (the original only advances a role with more than one clip).
fn advance(current: i32, count: usize) -> i32 {
    if count > 1 {
        (current + 1).rem_euclid(count as i32)
    } else {
        current
    }
}

/// Which take of each multi-variant role plays next. `idle` starts at `0` and
/// the others at `-1`, so the first *re-*entered idle is the second take while
/// the first jump and sleep are the first — the original's `playerVariant`.
#[derive(Clone, Copy)]
struct Variants {
    idle: i32,
    jump: i32,
    sleep: i32,
}

impl Default for Variants {
    fn default() -> Self {
        Self {
            idle: 0,
            jump: -1,
            sleep: -1,
        }
    }
}

/// The spawned goat: its root, its resting height offset, and its locomotion.
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
    /// The clips, by role.
    clips: Clips,
    /// Which take of a multi-variant role plays next.
    variant: Variants,
    /// The locomotion state.
    mode: Mode,
    /// Seconds into the current jump.
    jump_time: f32,
    /// Ground speed frozen at take-off.
    jump_speed: f32,
    /// Travel direction (`-1`/`0`/`1`) frozen at take-off.
    jump_dir: f32,
    /// The jump clip's length, so the state ends when its clip does.
    jump_duration: f32,
}

impl Goat {
    /// Enter `mode`, cycling the take of a multi-variant role — the original's
    /// `cyclePlayerVariant`, called as it enters idle, jump or sleep.
    fn enter(&mut self, mode: Mode) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        match mode {
            Mode::Idle => self.variant.idle = advance(self.variant.idle, self.clips.idle.len()),
            Mode::Jump => self.variant.jump = advance(self.variant.jump, self.clips.jump.len()),
            Mode::Sleep => self.variant.sleep = advance(self.variant.sleep, self.clips.sleep.len()),
            _ => {}
        }
    }

    /// The clip playing now, and whether it loops.
    fn animation(&self) -> (AssetHandle<AnimationClip>, bool) {
        let variant = match self.mode {
            Mode::Idle => self.variant.idle,
            Mode::Jump => self.variant.jump,
            Mode::Sleep => self.variant.sleep,
            _ => 0,
        };
        (self.clips.pick(self.mode, variant), self.mode.loops())
    }

    /// The ground speed of the state — the gaits only; a jump carries its own
    /// take-off speed and idle and sleep stand still.
    fn speed(&self) -> f32 {
        match self.mode {
            Mode::Walk => WALK_SPEED,
            Mode::Trot => TROT_SPEED,
            Mode::Run => RUN_SPEED,
            _ => 0.0,
        }
    }

    /// Take off into a jump at the gait's speed and the given travel direction,
    /// and note how long the jump clip runs (the original's `startJump`).
    fn start_jump(&mut self, assets: &AssetRegistry, gait: Mode, direction: f32) {
        self.enter(Mode::Jump);
        self.jump_time = 0.0;
        self.jump_dir = direction;
        self.jump_speed = match gait {
            Mode::Run => RUN_SPEED,
            Mode::Trot => TROT_SPEED,
            _ => WALK_SPEED,
        };
        let (clip, _) = self.animation();
        self.jump_duration = assets
            .animations
            .get(clip)
            .map(|clip| clip.duration)
            .unwrap_or(0.0);
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
            let state = goats.as_ref().map_or("standing", |goat| goat.mode.label());
            ctx.screen_text = vec![format!(
                "Goats  |  {fps:.0} fps  |  {:.2} ms  |  {state}  |  \
                 W walk, Ctrl trot, Shift run, Space jump, Z sleep",
                dt * 1000.0
            )];
        });
    }
}

/// Advance the goat one frame: turn, run the locomotion state machine, step the
/// root and play the state's clip.
fn drive_goat(ctx: &mut EngineContext, goat: &mut Goat) {
    let dt = ctx.time.delta();
    let input = &ctx.input;
    let down = |c: char| Key::letter(c).is_some_and(|key| input.key_down(key));
    let pressed = |c: char| Key::letter(c).is_some_and(|key| input.key_pressed(key));

    // Turning is locked while asleep; every other state may steer, even a jump.
    // `A` turns left (it increases `yaw`, the original's `turn += 1` for `A`).
    if goat.mode != Mode::Sleep {
        goat.yaw += axis(down('A'), down('D')) * TURN_RATE * dt;
    }

    // The movement intent, and the gait the modifiers ask for.
    let move_axis = axis(down('W'), down('S'));
    let gait = if input.key_down(Key::ShiftLeft) || input.key_down(Key::ShiftRight) {
        Mode::Run
    } else if input.key_down(Key::ControlLeft) || input.key_down(Key::ControlRight) {
        Mode::Trot
    } else {
        Mode::Walk
    };

    // The state machine (the original's `sceneFrame`): a locked state runs to its
    // end, a free one follows input. Jump and sleep take their key on the press
    // edge, so one tap is one action.
    match goat.mode {
        Mode::Sleep => {
            if pressed('Z') || move_axis != 0.0 {
                goat.enter(Mode::Idle);
            }
        }
        Mode::Jump => {
            goat.jump_time += dt;
            if goat.jump_time >= goat.jump_duration {
                goat.enter(if move_axis != 0.0 { gait } else { Mode::Idle });
            }
        }
        _ => {
            if pressed('Z') {
                goat.enter(Mode::Sleep);
            } else if input.key_pressed(Key::Space) {
                goat.start_jump(&ctx.assets, gait, move_axis);
            } else {
                goat.enter(if move_axis != 0.0 { gait } else { Mode::Idle });
            }
        }
    }

    // Step the root. A jump travels at the speed and direction frozen at
    // take-off — its vertical arc is the clip's own root motion — a gait travels
    // at its ground speed along the goat's heading, and idle and sleep stand
    // still. The heading is the model's local `+X` rotated about `+Y` by yaw.
    let forward_x = goat.yaw.cos();
    let forward_z = -goat.yaw.sin();
    let (speed, direction) = match goat.mode {
        Mode::Jump => (goat.jump_speed, goat.jump_dir),
        Mode::Sleep => (0.0, 0.0),
        _ => (goat.speed(), move_axis),
    };
    goat.x = (goat.x + forward_x * speed * direction * dt).clamp(-FIELD, FIELD);
    goat.z = (goat.z + forward_z * speed * direction * dt).clamp(-FIELD, FIELD);
    let y = terrain::height(goat.x, goat.z) + goat.lift;

    if let Some(transform) = ctx.scene.world.get_mut::<LocalTransform>(goat.root) {
        transform.translation = math::vec3(goat.x, y, goat.z);
        transform.rotation = Quat::from_rotation_y(goat.yaw);
    }

    // Play the state's clip; a reversing gait runs its clip backwards.
    let (clip, looped) = goat.animation();
    if let Some(animator) = ctx.scene.world.get_mut::<Animator>(goat.skinned) {
        if animator.clip != clip {
            animator.clip = clip;
            animator.time = 0.0;
            animator.playing = true;
        }
        animator.looped = looped;
        animator.speed = if goat.mode == Mode::Jump || move_axis >= 0.0 {
            1.0
        } else {
            BACK_SPEED
        };
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
    // `Left` increases the camera's yaw, as the original's `KEY_LEFT` does.
    let turn = axis(
        ctx.input.key_down(Key::ArrowLeft),
        ctx.input.key_down(Key::ArrowRight),
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

    // The clips, by role: the gaits are single takes, and idle, jump and sleep
    // carry every take the model gives them (the original's `VARIANTS`).
    let role = |needle: &str| variants(&loaded.animations, &handles.animations, needle);
    let clips = Clips {
        idle: role("idle"),
        walk: first(role("walk")),
        trot: first(role("trot")),
        run: first(role("run")),
        jump: role("jump"),
        sleep: role("sleep"),
    };
    ctx.scene
        .world
        .insert(skinned, Animator::new(clips.pick(Mode::Idle, 0)));

    log::info!(
        "goat: {} mesh(es), {} node(s), {} triangle(s), {} clip(s), lifted {lift:.3}",
        loaded.meshes.len(),
        loaded.nodes.len(),
        loaded.triangle_count(),
        loaded.animations.len(),
    );
    Some(Goat {
        root,
        skinned,
        lift,
        x: 0.0,
        z: 0.0,
        yaw: 0.0,
        clips,
        variant: Variants::default(),
        mode: Mode::Idle,
        jump_time: 0.0,
        jump_speed: 0.0,
        jump_dir: 0.0,
        jump_duration: 0.0,
    })
}

/// The handles of every clip whose name contains `needle` (case-insensitively),
/// in model order — the original's `findClips` over a role's takes (`GoatIdle`,
/// `GoatIdle2`, `GoatIdle3`, …). `animations` and `handles` are parallel.
fn variants(
    animations: &[AnimationClip],
    handles: &[AssetHandle<AnimationClip>],
    needle: &str,
) -> Vec<AssetHandle<AnimationClip>> {
    animations
        .iter()
        .zip(handles)
        .filter(|(clip, _)| clip.name.to_ascii_lowercase().contains(needle))
        .map(|(_, &handle)| handle)
        .collect()
}

/// The first of a role's clips, or the empty handle.
fn first(clips: Vec<AssetHandle<AnimationClip>>) -> AssetHandle<AnimationClip> {
    clips.first().copied().unwrap_or_default()
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

    /// The goat's root entity.
    fn goat_root(app: &App) -> Entity {
        app.context
            .scene
            .world
            .query::<Name>()
            .find(|(_, name)| name.as_str() == "Goat Root")
            .map(|(entity, _)| entity)
            .expect("the goat root")
    }

    /// The clip the goat's animator is playing.
    fn playing(app: &App) -> AssetHandle<AnimationClip> {
        app.context
            .scene
            .world
            .query::<Animator>()
            .map(|(_, animator)| animator.clip)
            .next()
            .expect("an animator")
    }

    /// `true` when `clip` is one of the clips whose name contains `needle`.
    fn is_role(app: &App, clip: AssetHandle<AnimationClip>, needle: &str) -> bool {
        app.context
            .assets
            .animations
            .get(clip)
            .is_some_and(|clip| clip.name.to_ascii_lowercase().contains(needle))
    }

    fn key(c: char) -> Key {
        Key::letter(c).expect("a letter key")
    }

    #[test]
    fn pressing_space_jumps_then_lands_back_in_the_gait() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        app.tick(1.0 / 60.0);
        let root = goat_root(&app);

        app.context.input.press_key(key('W'));
        app.tick(1.0 / 60.0);
        let before = app
            .context
            .scene
            .world
            .get::<LocalTransform>(root)
            .expect("a transform")
            .translation;

        app.context.input.press_key(Key::Space);
        app.tick(1.0 / 60.0);
        assert!(
            is_role(&app, playing(&app), "jump"),
            "the jump clip plays: {:?}",
            playing(&app)
        );
        let duration = app
            .context
            .assets
            .animations
            .get(playing(&app))
            .expect("the jump clip")
            .duration;

        // The goat keeps travelling while it is airborne.
        for _ in 0..8 {
            app.tick(1.0 / 60.0);
        }
        let mid = app
            .context
            .scene
            .world
            .get::<LocalTransform>(root)
            .expect("a transform")
            .translation;
        assert!(
            mid.x > before.x + 0.05,
            "airborne travel: {before:?} -> {mid:?}"
        );

        // It lands, and with `W` still held it takes up the walk again.
        for _ in 0..(duration * 60.0).ceil() as u32 + 2 {
            app.tick(1.0 / 60.0);
        }
        assert!(
            is_role(&app, playing(&app), "walk"),
            "the walk resumed: {:?}",
            playing(&app)
        );
    }

    #[test]
    fn z_sleeps_and_wakes_the_goat() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        app.tick(1.0 / 60.0);

        app.context.input.press_key(key('Z'));
        app.tick(1.0 / 60.0);
        assert!(
            is_role(&app, playing(&app), "sleep"),
            "asleep: {:?}",
            playing(&app)
        );

        // A second, fresh press wakes it.
        app.context.input.release_key(key('Z'));
        app.context.input.press_key(key('Z'));
        app.tick(1.0 / 60.0);
        assert!(
            is_role(&app, playing(&app), "idle"),
            "awake: {:?}",
            playing(&app)
        );
    }

    #[test]
    fn moving_wakes_a_sleeping_goat() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        app.tick(1.0 / 60.0);

        app.context.input.press_key(key('Z'));
        app.tick(1.0 / 60.0);
        assert!(is_role(&app, playing(&app), "sleep"));

        app.context.input.press_key(key('W'));
        app.tick(1.0 / 60.0);
        // Waking returns to idle for the one frame (the original's `wakeUp`),
        // then the free state follows the held key into a walk.
        assert!(
            is_role(&app, playing(&app), "idle"),
            "woke to idle: {:?}",
            playing(&app)
        );
        app.tick(1.0 / 60.0);
        assert!(
            is_role(&app, playing(&app), "walk"),
            "then walked: {:?}",
            playing(&app)
        );
    }

    #[test]
    fn the_idle_take_cycles_on_each_entry() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        app.tick(1.0 / 60.0);

        // The model's first idle take, on the frame the world is built.
        let first = playing(&app);
        assert!(is_role(&app, first, "idle"), "{first:?}");

        // Walk a step, then stop: re-entering idle takes the next take.
        app.context.input.press_key(key('W'));
        app.tick(1.0 / 60.0);
        app.context.input.release_key(key('W'));
        app.tick(1.0 / 60.0);
        let second = playing(&app);
        assert!(is_role(&app, second, "idle"), "{second:?}");
        assert_ne!(
            first, second,
            "the idle take advanced: {first:?} then {second:?}"
        );
    }

    #[test]
    fn a_turns_left_and_d_turns_right() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        app.tick(1.0 / 60.0);
        let root = goat_root(&app);

        // The goat's world forward (it faces its local `+X`). Turning left
        // swings it toward `-Z`, turning right toward `+Z`.
        let forward_z = |app: &App| {
            app.context
                .scene
                .world
                .get::<LocalTransform>(root)
                .expect("a transform")
                .rotation
                .rotate_vec3(math::vec3(1.0, 0.0, 0.0))
                .z
        };

        app.context.input.press_key(key('A'));
        for _ in 0..10 {
            app.tick(1.0 / 60.0);
        }
        assert!(forward_z(&app) < -0.1, "A turned left: {}", forward_z(&app));

        app.context.input.release_key(key('A'));
        app.context.input.press_key(key('D'));
        for _ in 0..20 {
            app.tick(1.0 / 60.0);
        }
        assert!(forward_z(&app) > 0.1, "D turned right: {}", forward_z(&app));
    }
}
