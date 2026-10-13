//! Goats — a Noctis port of the Slag Goat sandbox (`goats/`).
//!
//! The goat model loads as a skinned mesh, the ground is a mesh built from the
//! original's own noise function ([`terrain`]), and a procedural meadow
//! ([`grass`]) is grazed. This is M1 of the port (see `.notes/goats.md`): the
//! goat's locomotion state machine — idle, its three gaits, a jump and sleep —
//! driven from input, where each state plays a baked clip through the engine's
//! `Animator` and the root moves at the state's authored speed. On top sits M2's
//! health and energy system, and its meadow: energy drains with effort, eating
//! grass tops it up, exhaustion caps the gait, and running out of health kills
//! the goat until it is restarted.

#![forbid(unsafe_code)]

mod grass;
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

// The goat's health and energy, from the original's `TUNING.stats`. Both run
// `0..=MAX_STATS`; energy drains with effort, and running it to zero exhausts
// the goat, which then burns health until it dies (or sleeps it back).
/// The ceiling for both stats.
const MAX_STATS: f32 = 100.0;
/// Energy spent per second at rest, walking, trotting and running.
const ENERGY_DRAIN_IDLE: f32 = 0.4;
const ENERGY_DRAIN_WALK: f32 = 1.0;
const ENERGY_DRAIN_TROT: f32 = 2.0;
const ENERGY_DRAIN_RUN: f32 = 4.0;
/// Energy a jump costs at take-off.
const JUMP_ENERGY_COST: f32 = 2.0;
/// Energy and health recovered per second asleep.
const SLEEP_ENERGY_RECOVER: f32 = 12.0;
const SLEEP_HEALTH_RECOVER: f32 = 2.0;
/// Health recovered per second while rested (energy above [`RESTED_ENERGY`]).
const IDLE_HEALTH_RECOVER: f32 = 0.1;
/// Health lost per second once exhausted.
const EXHAUST_HEALTH_DRAIN: f32 = 3.0;
/// Health only regenerates above this energy.
const RESTED_ENERGY: f32 = 20.0;
/// Seconds spent idle while exhausted before the goat drops off asleep.
const AUTO_SLEEP_DELAY: f32 = 2.0;

/// The tuft colour: the original's `TUFT` (`78,128,66` sRGB) in linear.
const TUFT: [f32; 4] = [0.076, 0.216, 0.054, 1.0];

// The meal, from the original's `TUNING.food`. A bite tops up energy and fills
// the belly; the belly eases the rain slowdown once weather lands, and decays, so
// the goat has to keep grazing.
/// Energy restored per tuft.
const EAT_ENERGY: f32 = 8.0;
/// Belly filled per tuft, `0..=1`.
const EAT_SATIETY: f32 = 0.55;
/// A meal taken with the belly already this full has nothing left to *fill*, so it
/// heals instead (the original's `fullBelly`).
const FULL_BELLY: f32 = 0.85;
/// Health restored by a meal taken on a full belly.
const FULL_BELLY_HEAL: f32 = 3.0;
/// Belly lost per second.
const SATIETY_DECAY: f32 = 0.02;

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
/// The original also has `flung`; it arrives with the milestone that causes it —
/// the blasts (`.notes/goats.md` M3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Idle,
    Walk,
    Trot,
    Run,
    Jump,
    Sleep,
    Eat,
    Dead,
}

impl Mode {
    /// Whether the state's clip is a cycle, or a one-shot the engine holds on
    /// its last frame (a jump lands, a meal ends, a death stays down).
    fn loops(self) -> bool {
        !matches!(self, Mode::Jump | Mode::Eat | Mode::Dead)
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
            Mode::Eat => "eating",
            Mode::Dead => "dead (R to restart)",
        }
    }
}

/// The goat's clips by role. The gaits are one clip each; idle, jump, sleep and
/// eat have several takes the goat cycles through, exactly as the original's
/// `VARIANTS` do.
#[derive(Default)]
struct Clips {
    idle: Vec<AssetHandle<AnimationClip>>,
    walk: AssetHandle<AnimationClip>,
    trot: AssetHandle<AnimationClip>,
    run: AssetHandle<AnimationClip>,
    jump: Vec<AssetHandle<AnimationClip>>,
    sleep: Vec<AssetHandle<AnimationClip>>,
    eat: Vec<AssetHandle<AnimationClip>>,
    death: AssetHandle<AnimationClip>,
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
            Mode::Eat => at(&self.eat, variant),
            Mode::Dead => Some(self.death),
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
/// the first jump, sleep and eat are the first — the original's `playerVariant`.
#[derive(Clone, Copy)]
struct Variants {
    idle: i32,
    jump: i32,
    sleep: i32,
    eat: i32,
}

impl Default for Variants {
    fn default() -> Self {
        Self {
            idle: 0,
            jump: -1,
            sleep: -1,
            eat: -1,
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
    /// Health, `0..=MAX_STATS`; at zero the goat dies.
    health: f32,
    /// Energy, `0..=MAX_STATS`; at zero the goat is exhausted.
    energy: f32,
    /// Whether energy has run out — health is draining and the gait is capped.
    exhausted: bool,
    /// Seconds spent idle while exhausted, towards the auto-sleep.
    idle_timer: f32,
    /// The belly, `0..=1`: filled by grazing, emptied over time.
    satiety: f32,
    /// Seconds into the current meal.
    eat_time: f32,
    /// The eat clip's length, so the meal ends when its clip does.
    eat_duration: f32,
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
            Mode::Eat => self.variant.eat = advance(self.variant.eat, self.clips.eat.len()),
            _ => {}
        }
    }

    /// The clip playing now, and whether it loops.
    fn animation(&self) -> (AssetHandle<AnimationClip>, bool) {
        let variant = match self.mode {
            Mode::Idle => self.variant.idle,
            Mode::Jump => self.variant.jump,
            Mode::Sleep => self.variant.sleep,
            Mode::Eat => self.variant.eat,
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
        self.energy = (self.energy - JUMP_ENERGY_COST).max(0.0);
        let (clip, _) = self.animation();
        self.jump_duration = assets
            .animations
            .get(clip)
            .map(|clip| clip.duration)
            .unwrap_or(0.0);
    }

    /// Eat `tuft`: remove it from the meadow, face it, and top up energy and belly
    /// — the original's `startEat`. The caller ensures a tuft is in reach and the
    /// goat is free.
    fn start_eat(
        &mut self,
        assets: &AssetRegistry,
        meadow: &mut grass::Meadow,
        tuft: &grass::Tuft,
    ) {
        meadow.consume(tuft);
        // Turn to face the tuft, so the head comes down where the grass was: the
        // model's local `+X` is forward, so this is `atan2(-dz, dx)`.
        self.yaw = (-(tuft.z - self.z)).atan2(tuft.x - self.x);
        // A meal on an already-full belly has nothing to fill, so it heals.
        let heal = if self.satiety >= FULL_BELLY {
            FULL_BELLY_HEAL
        } else {
            0.0
        };
        self.energy = (self.energy + EAT_ENERGY).min(MAX_STATS);
        self.satiety = (self.satiety + EAT_SATIETY).min(1.0);
        self.health = (self.health + heal).min(MAX_STATS);
        self.enter(Mode::Eat);
        self.eat_time = 0.0;
        let (clip, _) = self.animation();
        self.eat_duration = assets
            .animations
            .get(clip)
            .map(|clip| clip.duration)
            .unwrap_or(0.0);
    }

    /// Fall dead (the original's `die`).
    fn die(&mut self) {
        self.enter(Mode::Dead);
    }

    /// Start a new life: full stats, back at the origin (the original's
    /// `restart`).
    fn restart(&mut self) {
        self.health = MAX_STATS;
        self.energy = MAX_STATS;
        self.exhausted = false;
        self.idle_timer = 0.0;
        self.x = 0.0;
        self.z = 0.0;
        self.yaw = 0.0;
        self.enter(Mode::Idle);
    }

    /// Advance health and energy for the state, once a frame; returns `true` once
    /// health has run out, so the caller switches to [`Mode::Dead`]. The
    /// original's `updateStats`, minus the night and weather multipliers that
    /// arrive with their systems.
    fn update_stats(&mut self, dt: f32) -> bool {
        if self.mode == Mode::Dead {
            return false;
        }
        if self.mode == Mode::Sleep {
            self.energy = (self.energy + SLEEP_ENERGY_RECOVER * dt).min(MAX_STATS);
            self.health = (self.health + SLEEP_HEALTH_RECOVER * dt).min(MAX_STATS);
            return false;
        }

        let drain = match self.mode {
            Mode::Run => ENERGY_DRAIN_RUN,
            Mode::Trot => ENERGY_DRAIN_TROT,
            Mode::Walk => ENERGY_DRAIN_WALK,
            _ => ENERGY_DRAIN_IDLE,
        };
        self.energy = (self.energy - drain * dt).max(0.0);
        if self.energy <= 0.0 {
            self.exhausted = true;
            self.health = (self.health - EXHAUST_HEALTH_DRAIN * dt).max(0.0);
        } else {
            self.exhausted = false;
            if self.mode == Mode::Idle && self.energy > RESTED_ENERGY {
                self.health = (self.health + IDLE_HEALTH_RECOVER * dt).min(MAX_STATS);
            }
        }
        self.health <= 0.0
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

/// The running world: the goat, its meadow, and the grass field's rebuild state.
struct World {
    goat: Goat,
    meadow: grass::Meadow,
    /// The cell the grass field was last built around, so a move rebuilds it.
    grass_cell: (i32, i32),
    /// The sway clock, advanced each frame (the wind is quiet until weather).
    sway: f32,
}

impl Plugin for Goats {
    fn build(&self, app: &mut App) {
        let mut world: Option<World> = None;
        let mut orbit = Orbit::default();
        let mut seeded = false;

        app.on_update("goats.frame", move |ctx| {
            // The world is built on the first tick, so the closure owns it.
            if world.is_none() {
                world = spawn_world(ctx);
            }
            if let Some(world) = world.as_mut() {
                let ate = drive_goat(ctx, &mut world.goat, &mut world.meadow);
                update_meadow(ctx, world, ate);
            }

            let target = world
                .as_ref()
                .map(|world| {
                    let goat = &world.goat;
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
            let (state, health, energy, belly, hint) =
                world
                    .as_ref()
                    .map_or(("standing", MAX_STATS, MAX_STATS, 0.0, ""), |world| {
                        let goat = &world.goat;
                        let in_reach =
                            !matches!(goat.mode, Mode::Dead | Mode::Sleep | Mode::Jump | Mode::Eat)
                                && grass::nearest(goat.x, goat.z, grass::EAT_RANGE, &world.meadow)
                                    .is_some();
                        (
                            goat.mode.label(),
                            goat.health,
                            goat.energy,
                            goat.satiety,
                            if in_reach { "  |  eat (E)" } else { "" },
                        )
                    });
            ctx.screen_text = vec![format!(
                "Goats  |  {fps:.0} fps  |  {:.2} ms  |  {state}  |  \
                 health {health:.0}  energy {energy:.0}  belly {:.0}%{hint}",
                dt * 1000.0,
                belly * 100.0,
            )];
        });
    }
}

/// Advance the goat one frame: turn, run the locomotion state machine, step the
/// root and play the state's clip.
fn drive_goat(ctx: &mut EngineContext, goat: &mut Goat, meadow: &mut grass::Meadow) -> bool {
    let dt = ctx.time.delta();
    let input = &ctx.input;
    let down = |c: char| Key::letter(c).is_some_and(|key| input.key_down(key));
    let pressed = |c: char| Key::letter(c).is_some_and(|key| input.key_pressed(key));

    // Turning is locked while asleep, dead or eating; every other state may steer,
    // even a jump. `A` turns left (it increases `yaw`, the original's `turn += 1`).
    if !matches!(goat.mode, Mode::Sleep | Mode::Dead | Mode::Eat) {
        goat.yaw += axis(down('A'), down('D')) * TURN_RATE * dt;
    }

    // The belly empties over time.
    goat.satiety = (goat.satiety - SATIETY_DECAY * dt).max(0.0);

    // The movement intent, and the gait the modifiers ask for. An exhausted goat
    // cannot run or trot.
    let move_axis = axis(down('W'), down('S'));
    let gait = if goat.exhausted {
        Mode::Walk
    } else if input.key_down(Key::ShiftLeft) || input.key_down(Key::ShiftRight) {
        Mode::Run
    } else if input.key_down(Key::ControlLeft) || input.key_down(Key::ControlRight) {
        Mode::Trot
    } else {
        Mode::Walk
    };

    // The nearest tuft in reach; a free state lets the goat eat it.
    let food = grass::nearest(goat.x, goat.z, grass::EAT_RANGE, meadow);

    // The state machine (the original's `sceneFrame`): a locked state runs to its
    // end, a free one follows input. Eat, jump, sleep and restart take their key
    // on the press edge, so one tap is one action.
    let mut ate = false;
    match goat.mode {
        Mode::Dead => {
            if pressed('R') {
                goat.restart();
            }
        }
        Mode::Sleep => {
            if pressed('Z') || move_axis != 0.0 || goat.energy >= MAX_STATS {
                goat.enter(Mode::Idle);
            }
        }
        Mode::Jump => {
            goat.jump_time += dt;
            if goat.jump_time >= goat.jump_duration {
                goat.enter(if move_axis != 0.0 { gait } else { Mode::Idle });
            }
        }
        Mode::Eat => {
            goat.eat_time += dt;
            if goat.eat_time >= goat.eat_duration {
                goat.enter(if move_axis != 0.0 { gait } else { Mode::Idle });
            }
        }
        _ => {
            if pressed('E')
                && let Some(tuft) = food
            {
                goat.start_eat(&ctx.assets, meadow, &tuft);
                ate = true;
            } else if pressed('Z') {
                goat.enter(Mode::Sleep);
            } else if input.key_pressed(Key::Space) {
                goat.start_jump(&ctx.assets, gait, move_axis);
            } else {
                goat.enter(if move_axis != 0.0 { gait } else { Mode::Idle });
                // A goat exhausted and stood still drops off where it stands.
                if goat.exhausted && move_axis == 0.0 {
                    goat.idle_timer += dt;
                    if goat.idle_timer >= AUTO_SLEEP_DELAY {
                        goat.enter(Mode::Sleep);
                    }
                } else {
                    goat.idle_timer = 0.0;
                }
            }
        }
    }

    // Health and energy for the frame; at zero health the goat dies.
    if goat.update_stats(dt) {
        goat.die();
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
    let reversing = move_axis < 0.0 && matches!(goat.mode, Mode::Walk | Mode::Trot | Mode::Run);
    let (clip, looped) = goat.animation();
    if let Some(animator) = ctx.scene.world.get_mut::<Animator>(goat.skinned) {
        if animator.clip != clip {
            animator.clip = clip;
            animator.time = 0.0;
            animator.playing = true;
        }
        animator.looped = looped;
        animator.speed = if reversing { BACK_SPEED } else { 1.0 };
    }

    ate
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

/// Build the whole world: the ground, the meadow, then the goat.
fn spawn_world(ctx: &mut EngineContext) -> Option<World> {
    spawn_terrain(ctx);
    let meadow = grass::Meadow::default();
    spawn_grass(ctx, &meadow);
    let goat = spawn_goat(ctx)?;
    Some(World {
        goat,
        meadow,
        grass_cell: (0, 0),
        sway: 0.0,
    })
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

/// Build the meadow: its mesh and material, under one entity. The mesh is rebuilt
/// in place by [`update_meadow`] as the goat moves or grazes.
fn spawn_grass(ctx: &mut EngineContext, meadow: &grass::Meadow) {
    let mesh = ctx.assets.meshes.insert_named(
        "grass/meadow",
        grass::build_mesh(meadow, 0.0, 0.0, 0.0, 0.0),
    );
    let material = ctx.assets.materials.insert_named(
        "grass/tuft",
        Material {
            name: "grass tuft".to_string(),
            base_color: TUFT,
            metallic: 0.0,
            roughness: 0.9,
            ..Material::default()
        },
    );

    let field = ctx.scene.world.spawn();
    ctx.scene.world.insert(field, Name::new("Meadow"));
    ctx.scene.world.insert(field, LocalTransform::default());
    ctx.scene.world.insert(
        field,
        MeshRenderer {
            mesh,
            material,
            cast_shadows: false,
            receive_shadows: true,
        },
    );
}

/// Empty the belly and regrow eaten tufts each frame, and rebuild the grass field
/// around the goat when it has walked into a new cell or the meadow changed.
///
/// The rebuild replaces the mesh asset in place (same handle, new revision), so
/// the renderer re-uploads it rather than drawing the field the goat has left.
fn update_meadow(ctx: &mut EngineContext, world: &mut World, ate: bool) {
    let dt = ctx.time.delta();
    let regrew = world.meadow.update(dt);
    world.sway += dt;

    let cell = (
        (world.goat.x / grass::CELL).floor() as i32,
        (world.goat.z / grass::CELL).floor() as i32,
    );
    if ate || regrew || cell != world.grass_cell {
        world.grass_cell = cell;
        ctx.assets.meshes.insert_named(
            "grass/meadow",
            grass::build_mesh(&world.meadow, world.goat.x, world.goat.z, world.sway, 0.0),
        );
    }
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
        eat: role("eat"),
        death: first(role("death")),
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
        health: MAX_STATS,
        energy: MAX_STATS,
        exhausted: false,
        idle_timer: 0.0,
        satiety: 0.0,
        eat_time: 0.0,
        eat_duration: 0.0,
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

    #[test]
    fn running_to_empty_exhausts_the_goat_to_a_walk() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        app.tick(1.0 / 60.0);

        app.context.input.press_key(key('W'));
        app.context.input.press_key(Key::ShiftLeft);
        for _ in 0..1600 {
            app.tick(1.0 / 60.0);
        }
        // Energy ran out, so the gait is capped to a walk even with Shift held.
        assert!(
            is_role(&app, playing(&app), "walk"),
            "the exhausted goat walks: {:?}",
            playing(&app)
        );
    }

    #[test]
    fn exhaustion_kills_the_goat_and_r_starts_a_new_life() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        app.tick(1.0 / 60.0);
        let root = goat_root(&app);

        app.context.input.press_key(key('W'));
        app.context.input.press_key(Key::ShiftLeft);
        for _ in 0..3600 {
            app.tick(1.0 / 60.0);
        }
        assert!(
            is_role(&app, playing(&app), "death"),
            "the goat died: {:?}",
            playing(&app)
        );

        // `R` starts a new life back at the origin.
        app.context.input.release_key(key('W'));
        app.context.input.release_key(Key::ShiftLeft);
        app.context.input.press_key(key('R'));
        app.tick(1.0 / 60.0);
        assert!(
            is_role(&app, playing(&app), "idle"),
            "restarted: {:?}",
            playing(&app)
        );
        let at = app
            .context
            .scene
            .world
            .get::<LocalTransform>(root)
            .expect("a transform")
            .translation;
        assert!(
            at.x.abs() < 1e-3 && at.z.abs() < 1e-3,
            "back at the origin: {at:?}"
        );
    }

    #[test]
    fn an_exhausted_idle_goat_drops_off_to_sleep() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        app.tick(1.0 / 60.0);

        app.context.input.press_key(key('W'));
        app.context.input.press_key(Key::ShiftLeft);
        for _ in 0..1600 {
            app.tick(1.0 / 60.0);
        }
        // Stop; exhausted and still, it falls asleep after the delay.
        app.context.input.release_key(key('W'));
        app.context.input.release_key(Key::ShiftLeft);
        for _ in 0..140 {
            app.tick(1.0 / 60.0);
        }
        assert!(
            is_role(&app, playing(&app), "sleep"),
            "dropped off: {:?}",
            playing(&app)
        );
    }

    /// A bare goat for exercising the eat logic without a world.
    fn lone_goat() -> Goat {
        Goat {
            root: Entity::from_raw(0, 0),
            skinned: Entity::from_raw(1, 0),
            lift: 0.0,
            x: 0.0,
            z: 0.0,
            yaw: 0.0,
            clips: Clips::default(),
            variant: Variants::default(),
            mode: Mode::Idle,
            jump_time: 0.0,
            jump_speed: 0.0,
            jump_dir: 0.0,
            jump_duration: 0.0,
            health: MAX_STATS,
            energy: MAX_STATS,
            exhausted: false,
            idle_timer: 0.0,
            satiety: 0.0,
            eat_time: 0.0,
            eat_duration: 0.0,
        }
    }

    #[test]
    fn eating_a_tuft_tops_up_energy_and_belly() {
        let mut goat = lone_goat();
        goat.energy = 50.0;
        let mut meadow = grass::Meadow::default();
        let tuft = grass::nearest(0.0, 0.0, 6.0, &meadow).expect("a tuft in reach");

        goat.start_eat(&AssetRegistry::new(), &mut meadow, &tuft);

        assert_eq!(goat.mode, Mode::Eat);
        assert!(meadow.is_eaten(tuft.cx, tuft.cz), "the tuft was eaten");
        assert!(
            (goat.energy - 58.0).abs() < 1e-3,
            "energy +8: {}",
            goat.energy
        );
        assert!(
            (goat.satiety - EAT_SATIETY).abs() < 1e-3,
            "belly filled: {}",
            goat.satiety
        );

        // It turns to face the tuft (facing is the model's local `+X`).
        let forward = math::vec3(goat.yaw.cos(), 0.0, -goat.yaw.sin());
        let to_tuft = math::normalize(math::vec3(tuft.x, 0.0, tuft.z));
        let dot = forward.x * to_tuft.x + forward.y * to_tuft.y + forward.z * to_tuft.z;
        assert!(dot > 0.99, "faces the tuft: {forward:?}");
    }

    #[test]
    fn a_meal_on_a_full_belly_heals() {
        let mut goat = lone_goat();
        goat.health = 50.0;
        goat.satiety = FULL_BELLY;
        let mut meadow = grass::Meadow::default();
        let tuft = grass::nearest(0.0, 0.0, 6.0, &meadow).expect("a tuft in reach");

        goat.start_eat(&AssetRegistry::new(), &mut meadow, &tuft);
        assert!(
            goat.health > 50.0,
            "a full belly turns grass into health: {}",
            goat.health
        );
    }

    #[test]
    fn the_plugin_spawns_the_meadow() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        app.tick(1.0 / 60.0);

        assert!(
            app.context
                .scene
                .world
                .query::<Name>()
                .any(|(_, name)| name.as_str() == "Meadow"),
            "the meadow entity was spawned"
        );
        assert!(app.context.assets.materials.handle("grass/tuft").is_some());
        let mesh = app
            .context
            .assets
            .meshes
            .handle("grass/meadow")
            .and_then(|handle| app.context.assets.meshes.get(handle))
            .expect("the meadow mesh");
        assert!(!mesh.vertices.is_empty(), "the meadow has tufts at spawn");
    }

    #[test]
    fn the_goat_materials_carry_distinct_textures() {
        let loaded = load_gltf_bytes(GOAT_GLB).expect("import");
        let mut assets = AssetRegistry::with_defaults();
        let handles = loaded.register(&mut assets);

        // Four textures, all unnamed in the file. Registering must not collapse
        // them into one asset (which pointed every map at a single image and left
        // the goat a flat, textureless colour).
        assert_eq!(handles.textures.len(), 4, "the goat has four textures");

        // The fur's base colour and normal are different images, not one collapse.
        let fur = assets
            .materials
            .handle("GoatFur")
            .and_then(|handle| assets.materials.get(handle))
            .expect("the fur material");
        assert!(!fur.base_color_texture.is_none(), "the fur is textured");
        assert!(!fur.normal_texture.is_none());
        assert_ne!(
            fur.base_color_texture, fur.normal_texture,
            "the base colour and normal are separate images"
        );
    }
}
