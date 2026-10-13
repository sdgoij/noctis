//! Goats — the simulation of a Noctis port of the Slag Goat sandbox (`goats/`).
//!
//! The goat model loads as a skinned mesh, the ground is a mesh built from the
//! original's own noise function ([`terrain`]), and a procedural meadow
//! ([`grass`]) is grazed. M1 is the goat's locomotion state machine — idle, its
//! three gaits, a jump and sleep — driven from input, where each state plays a
//! baked clip through the engine's `Animator` and the root moves at the state's
//! authored speed. On top sits M2's health and energy system, its meadow and its
//! weather: energy drains with effort, eating grass tops it up, exhaustion caps the
//! gait, running out of health kills the goat until it is restarted, and a weather
//! state machine (clear / cloudy / rain / clearing) slows the goat and sharpens its
//! drain in rain and wind. The water table (M2) is the rain's other consequence: it
//! pools in the hollows, slows a wading goat and burns its energy, hides the tufts
//! it stands over, and refuses to let the goat sleep in a pool. M3 adds the world's
//! hazards ([`hazards`]): mines and trapped tufts, derived from a per-cell hash,
//! that trip when the goat reaches them, blow it on a ballistic arc that dishes a
//! crater into the ground, and chain into their neighbours. The weather's clouds,
//! rain streaks and sky tint, the water's surface, and the bang's fire and light,
//! are the presentation layer's (M5).
//!
//! This crate is the whole simulation. The `#[noctis::plugin]` that drives it one
//! frame at a time is the `goats-plugin` crate in `crates/plugin`, which calls
//! [`apply_frame`]; the design is `.notes/goats.md`.

#![forbid(unsafe_code)]

mod fx;
mod grass;
mod hazards;
pub mod terrain;
mod water;
mod weather;
mod weather_fx;

#[cfg(test)]
mod harness;

use noctis::prelude::*;

/// The goat model, compiled in — the original embeds it with `include_bytes!`
/// too, so the game needs no files on disk.
const GOAT_GLB: &[u8] = include_bytes!("../content/goat_animated.glb");

/// The sky, as a full-screen shader over the engine's prelude (`.notes/shaders.md`):
/// a view ray from the engine's inverse view-projection, a vertical gradient, and a
/// sun disc and glow. Its colours and sun are the material's `user` block, so it is
/// retuned without any engine change. The prelude supplies `ScreenOut` and
/// `vs_fullscreen`; the body provides only `fs_main`.
const SKY_WGSL: &str = r#"
fn sky_ray(uv: vec2<f32>) -> vec3<f32> {
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let far = frame.inv_view_proj * vec4<f32>(ndc, 1.0, 1.0);
    let near = frame.inv_view_proj * vec4<f32>(ndc, 0.0, 1.0);
    return normalize(far.xyz / far.w - near.xyz / near.w);
}

@fragment
fn fs_main(in: ScreenOut) -> @location(0) vec4<f32> {
    let dir = sky_ray(in.uv);
    let zenith = user[0].rgb;
    let horizon = user[1].rgb;
    let sun_dir = normalize(user[2].xyz);
    let sun_col = user[3].rgb;
    let sun_power = user[3].a;

    let up = clamp(dir.y, 0.0, 1.0);
    var col = mix(horizon, zenith, pow(up, 0.30));
    // Below the horizon, darken toward the ground so the terrain seam is soft.
    col = mix(col, horizon * 0.35, clamp(-dir.y * 3.0, 0.0, 1.0));

    let sun = max(dot(dir, sun_dir), 0.0);
    col = col + sun_col * pow(sun, 8.0) * 0.08 * sun_power;
    col = col + sun_col * smoothstep(0.9995, 0.9999, sun) * 2.5 * sun_power;
    return vec4<f32>(col, 1.0);
}
"#;

/// The water surface (`.notes/shaders.md`, mesh form): a flat quad at the table's
/// level, translucent, deeper toward the far edge. The terrain's depth test keeps it
/// to the pools — over higher ground it fails and is not drawn. Its `user` block is
/// the colours, the view distance and the alpha.
const WATER_WGSL: &str = r#"
struct WaterOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> WaterOut {
    let world = object.model * vec4<f32>(in.position, 1.0);
    var out: WaterOut;
    out.clip = frame.view_proj * world;
    out.world = world.xyz;
    return out;
}

@fragment
fn fs_main(in: WaterOut) -> @location(0) vec4<f32> {
    let deep = user[0].rgb;
    let shallow = user[1].rgb;
    let d = clamp(length(in.world - frame.camera_pos.xyz) / user[2].x, 0.0, 1.0);
    let col = mix(shallow, deep, d);
    return vec4<f32>(col, user[2].y);
}
"#;

/// The water's `user` block: deep colour, shallow colour, then view distance and alpha.
const WATER_UNIFORMS: [f32; 12] = [
    0.03, 0.12, 0.20, 1.0, // deep
    0.10, 0.28, 0.34, 1.0, // shallow
    40.0, 0.62, 0.0, 0.0, // view distance, alpha
];

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

/// The water level must move at least this much before the meadow is rebuilt, so a
/// rising table hides the tufts it reaches without rebuilding the field every frame
/// of a slow drain.
const WATER_MARK: f32 = 0.02;

// The flung goat's arc (the original's `TUNING.explosions.fling`): the blast is the
// impulse ([`hazards`]), and this is the ballistic flight the goat then owns.
/// Gravity of the arc, in m/s² — several times life, so the goat lands hard.
const FLING_GRAVITY: f32 = -72.0;
/// A ceiling on the arc's stretch, in seconds.
const FLING_MAX_FLIGHT: f32 = 2.5;

/// Height above the goat's feet that the camera orbits and looks at.
pub const EYE_HEIGHT: f32 = 0.75;
/// Radians the camera turns per logical point of drag.
const LOOK_SPEED: f32 = 0.006;
/// Radians per second the arrow keys turn the camera.
const KEY_LOOK_SPEED: f32 = 1.8;
/// World units the wheel dollies per point of scroll. A line-wheel notch is 40
/// points (see the platform's `LINE_SCROLL_POINTS`), so this is ~0.4 units a
/// notch — the original's `wheel * 0.4`, where a raylib notch is 1.
const ZOOM_SPEED: f32 = 0.01;
/// The nearest the camera may orbit.
const MIN_DIST: f32 = 2.5;
/// The farthest the camera may orbit.
const MAX_DIST: f32 = 40.0;
/// The pitch limits. The camera stays above the goat — the original's
/// `clamp(camPitch, 0.08, 1.35)`, negative here because this orbit's pitch grows
/// downward — so it never drops to the horizon or below, and cannot go into the
/// ground.
const MIN_PITCH: f32 = -1.35;
const MAX_PITCH: f32 = -0.08;
/// The least the camera may rise above the ground directly beneath it, so a slope
/// it backs into cannot put it underground.
const CAMERA_CLEARANCE: f32 = 0.35;

// The bang's look (M5), the cheap part: a point light that flares and a camera
// knock. The fireball, smoke and debris (the billboard/particle path) are the
// follow-up.
/// Seconds a bang's flash burns.
const FLASH_TIME: f32 = 0.22;
/// The flash's candela at its peak. The sun is 2.6 lux and the scene sits at
/// exposure 0.5; with the falloff this is roughly ten times the sun a few metres
/// out, so a bang reads as a fireball without washing the meadow out. The first
/// knob to turn if the flash is too hot or too weak.
const FLASH_INTENSITY: f32 = 30.0;
/// Metres the flash reaches; beyond this its falloff is zero.
const FLASH_RANGE: f32 = 16.0;
/// Metres above the blast centre the flash sits (the fireball's height).
const FLASH_HEIGHT: f32 = 1.0;

/// Seconds a camera knock rings.
const SHAKE_TIME: f32 = 0.32;
/// Metres the camera is shoved at full strength.
const SHAKE_POS: f32 = 0.28;
/// Radians the camera is rocked at full strength.
const SHAKE_ANGLE: f32 = 0.05;
/// The shove's upward share, so a knock rocks the view as well as shifting it.
const SHAKE_LIFT: f32 = 0.45;

/// Billboard entities reserved for the bang's sprites (a bang uses four, up to
/// twenty with the grit).
const FX_ENTITIES: usize = 64;
/// Billboard entities reserved for the mine "tell" (a handful at most).
const TELL_ENTITIES: usize = 8;

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

/// The bang's flash (M5): one shadowless point light that flares at a blast and
/// decays. A single light, so two bangs in one frame share the last site — the
/// cheap pass; a pool of lights is the follow-up. Nothing is drawn: a light at
/// zero intensity is invisible.
struct Flash {
    /// The point-light entity whose intensity this drives.
    light: Entity,
    /// Seconds of flare left (`0` = dark).
    left: f32,
}

impl Flash {
    /// The envelope now, `0..=1` — a sharp attack falling away.
    fn envelope(&self) -> f32 {
        let t = (self.left / FLASH_TIME).clamp(0.0, 1.0);
        t * t
    }
}

/// The camera's knock from a nearby bang (M5): a decaying shove, in world space so
/// [`drive_camera`] can project it onto the camera's own axes.
#[derive(Clone, Copy, Debug)]
struct Shake {
    /// Seconds of knock left (`0` = steady).
    left: f32,
    /// The world-space shove direction (unit, with a small lift).
    dir: Vec3,
    /// The peak strength (`0..=1`: a bang's impact at the goat).
    amp: f32,
}

impl Default for Shake {
    fn default() -> Self {
        Self {
            left: 0.0,
            dir: math::vec3(0.0, 1.0, 0.0),
            amp: 0.0,
        }
    }
}

impl Shake {
    /// The envelope now, `0..=1` — a fast attack easing out.
    fn envelope(&self) -> f32 {
        let t = (self.left / SHAKE_TIME).clamp(0.0, 1.0);
        t * t
    }

    /// The knock's strength now (peak times envelope).
    fn strength(&self) -> f32 {
        self.amp * self.envelope()
    }

    /// Take a bang: `impact` at the goat and the unit ground direction `shove` it
    /// throws in. A stronger knock overrides a weaker one still ringing.
    fn hit(&mut self, impact: f32, shove: (f32, f32)) {
        if impact <= 0.0 || impact < self.strength() {
            return;
        }
        self.left = SHAKE_TIME;
        self.amp = impact;
        self.dir = math::normalize(math::vec3(shove.0, SHAKE_LIFT, shove.1));
    }

    /// Let the knock ring out one frame.
    fn update(&mut self, dt: f32) {
        self.left = (self.left - dt).max(0.0);
    }
}

/// The goat's locomotion state — the original's `mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Idle,
    Walk,
    Trot,
    Run,
    Jump,
    Flung,
    Sleep,
    Eat,
    Dead,
}

impl Mode {
    /// Whether the state's clip is a cycle, or a one-shot the engine holds on
    /// its last frame (a jump lands, an arc touches down, a meal ends, a death
    /// stays down).
    fn loops(self) -> bool {
        !matches!(self, Mode::Jump | Mode::Flung | Mode::Eat | Mode::Dead)
    }

    /// A short label for the HUD.
    fn label(self) -> &'static str {
        match self {
            Mode::Idle => "standing",
            Mode::Walk => "walking",
            Mode::Trot => "trotting",
            Mode::Run => "running",
            Mode::Jump => "jumping",
            Mode::Flung => "flung",
            Mode::Sleep => "sleeping (Z to wake)",
            Mode::Eat => "eating",
            Mode::Dead => "dead (R to restart)",
        }
    }
}

/// The goat's clips by role. The gaits are one clip each; idle, jump, sleep and
/// eat have several takes the goat cycles through, exactly as the original's
/// `VARIANTS` do. `flung` is one clip; the model that lacks it poses its jump.
#[derive(Default)]
struct Clips {
    idle: Vec<AssetHandle<AnimationClip>>,
    walk: AssetHandle<AnimationClip>,
    trot: AssetHandle<AnimationClip>,
    run: AssetHandle<AnimationClip>,
    jump: Vec<AssetHandle<AnimationClip>>,
    flung: AssetHandle<AnimationClip>,
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
            // The arc: the flung clip when the model has one, the first jump when it
            // does not — the placeholder the clip contract was written around.
            Mode::Flung => Some(self.flung)
                .filter(|h| *h != AssetHandle::default())
                .or_else(|| at(&self.jump, 0)),
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
    /// The blast's arc (M3): seconds into the flight, the world-space velocity, the
    /// absolute height, and the ballistic time the pose is stretched over.
    fling_time: f32,
    fling_vx: f32,
    fling_vz: f32,
    fling_vy: f32,
    fling_y: f32,
    fling_flight: f32,
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
    /// `speed_factor` is the weather's slowdown — a wet goat does not jump as far.
    fn start_jump(
        &mut self,
        assets: &AssetRegistry,
        gait: Mode,
        direction: f32,
        speed_factor: f32,
    ) {
        self.enter(Mode::Jump);
        self.jump_time = 0.0;
        self.jump_dir = direction;
        self.jump_speed = match gait {
            Mode::Run => RUN_SPEED,
            Mode::Trot => TROT_SPEED,
            _ => WALK_SPEED,
        };
        self.jump_speed *= speed_factor;
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
        // A death mid-arc must not leave the next life hovering.
        self.fling_time = 0.0;
        self.fling_vx = 0.0;
        self.fling_vz = 0.0;
        self.fling_vy = 0.0;
        self.fling_y = 0.0;
        self.fling_flight = 0.0;
        self.enter(Mode::Idle);
    }

    /// Take a blast's impulse (the original's `startFling`). The first hit starts
    /// the arc where the goat stands; every hit adds to the velocity already there.
    fn start_fling(&mut self, vx: f32, vz: f32, vy: f32, base_y: f32) {
        if self.mode != Mode::Flung {
            self.fling_y = base_y;
            self.fling_time = 0.0;
            self.fling_vx = 0.0;
            self.fling_vz = 0.0;
            self.fling_vy = 0.0;
            self.enter(Mode::Flung);
        }
        self.fling_vx += vx;
        self.fling_vz += vz;
        self.fling_vy += vy;
        // The pose is stretched over the ballistic time the remaining upward impulse
        // implies, counted from the start of the arc, so the phase keeps rising.
        let air = if self.fling_vy > 0.0 && FLING_GRAVITY < 0.0 {
            -2.0 * self.fling_vy / FLING_GRAVITY
        } else {
            0.4
        };
        self.fling_flight = (self.fling_time + air).clamp(0.3, FLING_MAX_FLIGHT);
    }

    /// One frame of the arc, integrated in absolute height so a slope the goat
    /// crosses mid-air cannot drag it along. `ground` reports the surface under a
    /// point (the cratered terrain, plus the goat's own lift). Returns whether the
    /// feet touched the ground (`landed`).
    fn advance_fling(&mut self, dt: f32, ground: impl Fn(f32, f32) -> f32) -> bool {
        self.fling_time += dt;
        self.x = (self.x + self.fling_vx * dt).clamp(-FIELD, FIELD);
        self.z = (self.z + self.fling_vz * dt).clamp(-FIELD, FIELD);
        self.fling_vy += FLING_GRAVITY * dt;
        self.fling_y += self.fling_vy * dt;
        let g = ground(self.x, self.z);
        if self.fling_vy <= 0.0 && self.fling_y - g <= 0.0 {
            self.fling_y = g;
            self.fling_vx = 0.0;
            self.fling_vz = 0.0;
            self.fling_vy = 0.0;
            true
        } else {
            false
        }
    }

    /// How far through the arc the pose is, `0..=1` (the original's `flingProgress`).
    fn fling_progress(&self) -> f32 {
        (self.fling_time / self.fling_flight).min(1.0)
    }

    /// Advance health and energy for the state, once a frame; returns `true` once
    /// health has run out, so the caller switches to [`Mode::Dead`]. The
    /// original's `updateStats`, with the weather's and the water's drain multipliers
    /// folded into `drain_multiplier` (the night multiplier arrives with the day/night
    /// system, M5).
    fn update_stats(&mut self, dt: f32, drain_multiplier: f32) -> bool {
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
        self.energy = (self.energy - drain * drain_multiplier * dt).max(0.0);
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

/// The running world: the goat, its meadow, the ground's own state, and the orbit
/// camera that follows it.
pub struct World {
    goat: Goat,
    meadow: grass::Meadow,
    weather: weather::Weather,
    water: water::Water,
    /// The mines, traps and fuses ([`hazards`]).
    hazards: hazards::Hazards,
    /// The craters bangs have dished into the ground.
    craters: terrain::Craters,
    /// The crater revision the ground was last built from.
    crater_rev: u64,
    /// The cell the grass field was last built around, so a move rebuilds it.
    grass_cell: (i32, i32),
    /// The water level the meadow was last built against, so a rise rebuilds it.
    water_level: f32,
    /// The orbit camera about the goat.
    orbit: Orbit,
    /// Whether the orbit has been seeded from the scene's own camera yet.
    seeded: bool,
    /// The sky's material, whose `user` block is animated each frame.
    sky: AssetHandle<ShaderMaterial>,
    /// Seconds since the world was built, for the sky's slow sun arc.
    sky_time: f32,
    /// The water surface entity, moved to the table's level each frame.
    water_surface: Entity,
    /// The bang's fire, smoke and grit (M5).
    fx: fx::Fx,
    /// The generated fx atlases (a texture handle for the "tell").
    fx_textures: fx::FxTextures,
    /// Pooled billboard entities the fx sprites are written onto.
    fx_entities: Vec<Entity>,
    /// Pooled billboard entities for the mine "tell".
    tell_entities: Vec<Entity>,
    /// The cloud ring (M5).
    clouds: weather_fx::Clouds,
    /// The rain streaks (M5).
    rain: weather_fx::Rain,
    /// Pooled billboard entities for the clouds.
    cloud_entities: Vec<Entity>,
    /// Pooled billboard entities for the rain.
    rain_entities: Vec<Entity>,
    /// The bang's flash light (M5).
    flash: Flash,
    /// The camera's knock from nearby bangs (M5).
    shake: Shake,
}

/// Advance the whole world one frame: the weather and the water, the goat, the
/// hazards and the ground, the camera, and the HUD. This is the single entry point
/// the plugin drives — and, once it exists, the harness (`.notes/harness.md`).
pub fn apply_frame(ctx: &mut EngineContext, world: &mut World) {
    let dt = ctx.time.delta();
    world.weather.update(dt);
    world.water.update(dt, world.weather.rain());
    // The sky follows the clock and the rain: the sun arcs and the colour drains
    // toward grey. The renderer writes it into the material's user buffer, no rebuild.
    world.sky_time += dt;
    if let Some(material) = ctx.assets.shader_materials.get_mut(world.sky) {
        material.uniforms = sky_uniforms(world.sky_time, world.weather.rain());
    }
    // The water surface follows the table.
    if let Some(transform) = ctx
        .scene
        .world
        .get_mut::<LocalTransform>(world.water_surface)
    {
        transform.translation.y = world.water.level();
    }
    let ate = drive_goat(
        ctx,
        &mut world.goat,
        &mut world.meadow,
        &mut world.weather,
        &mut world.water,
        &world.craters,
    );
    // The hazards own the ground too: a bang dishes a crater, and the ground heals
    // on its own clock whatever else is happening. Each bang that lands this frame
    // is reported for the presentation layer.
    let bangs = world
        .hazards
        .update(dt, &mut world.goat, &world.meadow, &mut world.craters);
    // The bang's look (M5): a flash light and a camera knock.
    present_bangs(
        ctx,
        &mut world.flash,
        &mut world.shake,
        &bangs,
        &world.craters,
        dt,
    );
    // ...and its fire, smoke and grit: a burst per bang, then the pool advanced and
    // the sprites written onto the pooled billboard entities.
    for bang in &bangs {
        let ground = world.craters.surface(bang.x, bang.z);
        world.fx.blast(bang.x, bang.z, ground, bang.seed);
    }
    world.fx.update(dt);
    sync_sprites(ctx, world, dt);
    // A crater — or its heal — moves the ground: rebuild the mesh, resample the
    // water table, and re-derive the meadow from it.
    let ground_changed = world.craters.revision() != world.crater_rev;
    if ground_changed {
        world.crater_rev = world.craters.revision();
        rebuild_terrain(ctx, &world.craters);
        world.water.resample(&world.craters);
    }
    update_meadow(ctx, world, ate, ground_changed);

    let target = math::vec3(
        world.goat.x,
        world.craters.surface(world.goat.x, world.goat.z) + world.goat.lift + EYE_HEIGHT,
        world.goat.z,
    );
    drive_camera(
        ctx,
        &mut world.orbit,
        &mut world.seeded,
        target,
        &world.craters,
        &mut world.shake,
    );

    let fps = if dt > 0.0 { 1.0 / dt } else { 0.0 };
    let goat = &world.goat;
    let in_reach = !matches!(goat.mode, Mode::Dead | Mode::Sleep | Mode::Jump | Mode::Eat)
        && grass::nearest(
            goat.x,
            goat.z,
            grass::EAT_RANGE,
            &world.meadow,
            &world.craters,
        )
        .is_some();
    // The water the goat is standing in joins the weather line, and is empty when
    // it is dry (the original's `updateWeatherEffects`).
    let mut weather = world.weather.hud(goat.satiety);
    let water = world.water.hud(goat.x, goat.z);
    if !water.is_empty() {
        weather.push_str("   ");
        weather.push_str(&water);
    }
    ctx.screen_text = vec![
        format!(
            "Goats  |  {fps:.0} fps  |  {:.2} ms  |  {}  |  \
             health {:.0}  energy {:.0}  belly {:.0}%{}",
            dt * 1000.0,
            goat.mode.label(),
            goat.health,
            goat.energy,
            goat.satiety * 100.0,
            if in_reach { "  |  eat (E)" } else { "" },
        ),
        weather,
    ];
}

/// Advance the goat one frame: turn, run the locomotion state machine, step the
/// root and play the state's clip. The `water` table it stands in slows it, burns its
/// energy and gates its sleep; `craters` is the ground it stands on and, after a
/// blast, the arc it flies.
fn drive_goat(
    ctx: &mut EngineContext,
    goat: &mut Goat,
    meadow: &mut grass::Meadow,
    weather: &mut weather::Weather,
    water: &mut water::Water,
    craters: &terrain::Craters,
) -> bool {
    let dt = ctx.time.delta();
    let input = &ctx.input;
    let down = |c: char| Key::letter(c).is_some_and(|key| input.key_down(key));
    let pressed = |c: char| Key::letter(c).is_some_and(|key| input.key_pressed(key));

    // Turning is locked while asleep, dead, eating — or, for about a second, flung,
    // which is the blast's and not the player's. Every other state may steer, even a
    // jump. `A` turns left (it increases `yaw`, the original's `turn += 1`).
    if !matches!(
        goat.mode,
        Mode::Sleep | Mode::Dead | Mode::Eat | Mode::Flung
    ) {
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

    // `C` forces the next weather state, for previewing the cycle.
    if pressed('C') {
        weather.force_next();
    }
    // `J` toggles the whole water system, the original's own bisect.
    if pressed('J') {
        water.set_enabled(!water.enabled());
    }
    // Rain and wind slow the goat and make it work harder; so does wading, which is
    // exactly neutral out of the water.
    let speed_factor = weather.speed(goat.satiety) * water.speed_factor(goat.x, goat.z);

    // The nearest tuft in reach that the ground has not swallowed; a free state lets
    // the goat eat it.
    let food = grass::nearest(goat.x, goat.z, grass::EAT_RANGE, meadow, craters);

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
            // The water can arrive under a sleeping goat, so the same test that refuses
            // to *start* a sleep ends one (the original's `sceneFrame`).
            if pressed('Z')
                || move_axis != 0.0
                || goat.energy >= MAX_STATS
                || water.in_water(goat.x, goat.z)
            {
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
        Mode::Flung => {
            // Not in charge until the feet touch (M19c): no steering, no jumping
            // out of it, no eating. The arc is advanced below.
        }
        _ => {
            if pressed('E')
                && let Some(tuft) = food
            {
                goat.start_eat(&ctx.assets, meadow, &tuft);
                ate = true;
            } else if pressed('Z') {
                // A goat does not sleep in the water (the original's `startSleep`).
                if !water.in_water(goat.x, goat.z) {
                    goat.enter(Mode::Sleep);
                }
            } else if input.key_pressed(Key::Space) {
                goat.start_jump(&ctx.assets, gait, move_axis, speed_factor);
            } else {
                goat.enter(if move_axis != 0.0 { gait } else { Mode::Idle });
                // A goat exhausted and stood still drops off where it stands.
                if goat.exhausted && move_axis == 0.0 {
                    goat.idle_timer += dt;
                    if goat.idle_timer >= AUTO_SLEEP_DELAY && !water.in_water(goat.x, goat.z) {
                        goat.enter(Mode::Sleep);
                    }
                } else {
                    goat.idle_timer = 0.0;
                }
            }
        }
    }

    // Health and energy for the frame; at zero health the goat dies. Wading burns
    // extra energy (the original's `waterDrainFactor`).
    let drain = weather.drain() * water.drain_factor(goat.x, goat.z);
    if goat.update_stats(dt, drain) {
        goat.die();
    }

    // Step the root. A jump travels at the speed and direction frozen at
    // take-off — its vertical arc is the clip's own root motion — a gait travels
    // at its ground speed along the goat's heading, and idle and sleep stand
    // still. The heading is the model's local `+X` rotated about `+Y` by yaw. A
    // flung goat owns its own arc instead (M19c).
    let forward_x = goat.yaw.cos();
    let forward_z = -goat.yaw.sin();
    let y = if goat.mode == Mode::Flung {
        let lift = goat.lift;
        if goat.advance_fling(dt, |x, z| craters.surface(x, z) + lift) {
            goat.enter(if move_axis != 0.0 { gait } else { Mode::Idle });
        }
        goat.fling_y
    } else {
        let (speed, direction) = match goat.mode {
            Mode::Jump => (goat.jump_speed, goat.jump_dir),
            Mode::Sleep => (0.0, 0.0),
            _ => (goat.speed() * speed_factor, move_axis),
        };
        goat.x = (goat.x + forward_x * speed * direction * dt).clamp(-FIELD, FIELD);
        goat.z = (goat.z + forward_z * speed * direction * dt).clamp(-FIELD, FIELD);
        craters.surface(goat.x, goat.z) + goat.lift
    };

    if let Some(transform) = ctx.scene.world.get_mut::<LocalTransform>(goat.root) {
        transform.translation = math::vec3(goat.x, y, goat.z);
        transform.rotation = Quat::from_rotation_y(goat.yaw);
    }

    // Play the state's clip; a reversing gait runs its clip backwards. A flung goat
    // is *posed*, phase 0 at the blast and phase 1 at contact, stretched over the
    // flight (the original's `poseModel`), rather than played forward.
    let (clip, looped) = goat.animation();
    if goat.mode == Mode::Flung {
        let duration = ctx
            .assets
            .animations
            .get(clip)
            .map(|clip| clip.duration)
            .unwrap_or(0.0);
        if let Some(animator) = ctx.scene.world.get_mut::<Animator>(goat.skinned) {
            if animator.clip != clip {
                animator.clip = clip;
                animator.playing = true;
            }
            animator.looped = false;
            animator.speed = 0.0;
            animator.time = goat.fling_progress() * duration;
        }
    } else if let Some(animator) = ctx.scene.world.get_mut::<Animator>(goat.skinned) {
        let reversing = move_axis < 0.0 && matches!(goat.mode, Mode::Walk | Mode::Trot | Mode::Run);
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

/// The bang's look this frame (M5): flare the flash light at the last bang of the
/// frame, and hand each bang to the camera's knock. The light flares wherever a
/// bang landed — a blast out in the field is visible — while the knock takes only
/// the bangs that reached the goat (their `impact` is zero out of reach).
fn present_bangs(
    ctx: &mut EngineContext,
    flash: &mut Flash,
    shake: &mut Shake,
    bangs: &[hazards::Bang],
    craters: &terrain::Craters,
    dt: f32,
) {
    if let Some(at) = bangs.last() {
        let y = craters.surface(at.x, at.z) + FLASH_HEIGHT;
        if let Some(transform) = ctx.scene.world.get_mut::<LocalTransform>(flash.light) {
            transform.translation = math::vec3(at.x, y, at.z);
        }
        flash.left = FLASH_TIME;
    }
    flash.left = (flash.left - dt).max(0.0);
    if let Some(light) = ctx.scene.world.get_mut::<Light>(flash.light) {
        light.intensity = FLASH_INTENSITY * flash.envelope();
    }

    shake.update(dt);
    for bang in bangs {
        shake.hit(bang.impact, bang.shove);
    }
}

/// Advance and write every billboard sprite the world draws — the bang's fire,
/// smoke and grit, the mine "tell", the clouds and the rain — onto its pooled
/// entities. A pool entity with no sprite is hidden, so a quiet frame draws
/// nothing.
fn sync_sprites(ctx: &mut EngineContext, world: &mut World, dt: f32) {
    write_sprites(ctx, &world.fx_entities, &world.fx.visuals());

    // The weather's look: the clouds drift on the wind and the rain falls, both in
    // a box that follows the goat.
    let wind = world.weather.wind();
    world.clouds.update(dt, wind);
    world.rain.update(dt, wind);
    let (goat_x, goat_z) = (world.goat.x, world.goat.z);
    let cloud_visuals = world
        .clouds
        .visuals(goat_x, goat_z, world.weather.cloudiness());
    let ground = world.craters.surface(goat_x, goat_z);
    let rain_visuals = world
        .rain
        .visuals(goat_x, goat_z, world.weather.rain(), ground);
    write_sprites(ctx, &world.cloud_entities, &cloud_visuals);
    write_sprites(ctx, &world.rain_entities, &rain_visuals);

    // The mine "tell": a faint patch over each armed mine in reach.
    let tell_visuals: Vec<fx::Visual> =
        tell_positions(goat_x, goat_z, &world.hazards, &world.craters)
            .into_iter()
            .map(|position| fx::Visual {
                billboard: Billboard {
                    texture: world.fx_textures.dust,
                    size: math::vec2(fx::TELL_SIZE, fx::TELL_SIZE),
                    color: fx::TELL_TINT,
                    blend: BillboardBlend::Alpha,
                    depth_test: true,
                    ..Billboard::default()
                },
                position,
            })
            .collect();
    write_sprites(ctx, &world.tell_entities, &tell_visuals);
}

/// Write `visuals` onto `entities`, hiding any pool entity with no sprite.
fn write_sprites(ctx: &mut EngineContext, entities: &[Entity], visuals: &[fx::Visual]) {
    for (i, entity) in entities.iter().copied().enumerate() {
        match visuals.get(i) {
            Some(visual) => {
                ctx.scene.world.insert(entity, visual.billboard);
                if let Some(transform) = ctx.scene.world.get_mut::<LocalTransform>(entity) {
                    transform.translation = visual.position;
                }
                ctx.scene.world.insert(entity, Visible(true));
            }
            None => {
                ctx.scene.world.insert(entity, Visible(false));
            }
        }
    }
}

/// The world points of the armed mines within [`fx::TELL`] of the goat — a faint
/// disturbed-earth patch so a minefield is not wholly invisible (`scanMineTells`).
fn tell_positions(
    goat_x: f32,
    goat_z: f32,
    hazards: &hazards::Hazards,
    craters: &terrain::Craters,
) -> Vec<Vec3> {
    let mut out = Vec::new();
    let limit = fx::TELL * fx::TELL;
    let cx0 = ((goat_x - fx::TELL) * 0.5).floor() as i32;
    let cx1 = ((goat_x + fx::TELL) * 0.5).floor() as i32;
    let cz0 = ((goat_z - fx::TELL) * 0.5).floor() as i32;
    let cz1 = ((goat_z + fx::TELL) * 0.5).floor() as i32;
    for cx in cx0..=cx1 {
        for cz in cz0..=cz1 {
            if !hazards.mine_armed(cx, cz) {
                continue;
            }
            let mx = (cx * 2 + 1) as f32;
            let mz = (cz * 2 + 1) as f32;
            let dx = mx - goat_x;
            let dz = mz - goat_z;
            if dx * dx + dz * dz > limit {
                continue;
            }
            out.push(math::vec3(mx, craters.surface(mx, mz) + 0.04, mz));
        }
    }
    out
}

/// Advance the orbit camera about `target` from player input.
fn drive_camera(
    ctx: &mut EngineContext,
    orbit: &mut Orbit,
    seeded: &mut bool,
    target: Vec3,
    craters: &terrain::Craters,
    shake: &mut Shake,
) {
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

    let (mut position, mut rotation) = orbit.view(target);
    // A nearby bang shoves the view (M5): project the world shove onto the
    // camera's own axes, rock it about them, and nudge the eye along it.
    let strength = shake.strength();
    if strength > 0.0 {
        let right = rotation.rotate_vec3(math::vec3(1.0, 0.0, 0.0));
        let up = rotation.rotate_vec3(math::vec3(0.0, 1.0, 0.0));
        let lateral = math::dot(shake.dir, right);
        let vertical = math::dot(shake.dir, up);
        let rock = Quat::from_axis_angle(up, -lateral * SHAKE_ANGLE * strength).mul(
            Quat::from_axis_angle(right, vertical * SHAKE_ANGLE * strength),
        );
        rotation = rock.mul(rotation).normalized();
        position = position + shake.dir * (SHAKE_POS * strength);
    }
    // Never let the camera sit in the ground it is looking across.
    let floor = craters.surface(position.x, position.z) + CAMERA_CLEARANCE;
    let position = math::vec3(position.x, position.y.max(floor), position.z);
    if let Some(transform) = ctx.scene.world.get_mut::<LocalTransform>(camera) {
        transform.translation = position;
        transform.rotation = rotation;
    }
}

/// Spawn the sky: a full-screen shader pass over the engine's prelude
/// (`.notes/shaders.md`), the seam's first consumer.
fn spawn_sky(ctx: &mut EngineContext) -> AssetHandle<ShaderMaterial> {
    let shader = ctx
        .assets
        .shaders
        .insert_named("sky/meadow", Shader::new("goats sky", SKY_WGSL));
    let material = ctx.assets.shader_materials.insert_named(
        "sky/meadow",
        ShaderMaterial {
            name: "goats sky".into(),
            shader,
            uniforms: sky_uniforms(0.0, 0.0),
            ..ShaderMaterial::default()
        },
    );
    let sky = ctx.scene.world.spawn();
    ctx.scene.world.insert(sky, Name::new("Sky"));
    ctx.scene
        .world
        .insert(sky, ScreenShaderMaterial { material });
    material
}

/// The sky's animated `user` block: a slowly arcing sun, and colours drained toward
/// grey by the rain. Recomputed each frame it is cheap — the renderer writes it into
/// the material's user buffer without rebuilding anything.
fn sky_uniforms(time: f32, rain: f32) -> Vec<f32> {
    let angle = time * 0.08;
    let sun = math::normalize(math::vec3(
        angle.cos() * 0.7,
        0.30 + 0.45 * (0.5 + 0.5 * (angle * 0.5).sin()),
        angle.sin() * 0.7,
    ));
    let mix = |a: Vec3, b: Vec3, t: f32| a + (b - a) * t;
    let zenith = mix(
        math::vec3(0.015, 0.05, 0.15),
        math::vec3(0.10, 0.11, 0.14),
        rain,
    );
    let horizon = mix(
        math::vec3(0.18, 0.25, 0.36),
        math::vec3(0.17, 0.18, 0.21),
        rain,
    );
    let sun_color = math::vec3(1.0, 0.95, 0.82) * (1.0 - 0.8 * rain);
    vec![
        zenith.x,
        zenith.y,
        zenith.z,
        1.0, //
        horizon.x,
        horizon.y,
        horizon.z,
        1.0, //
        sun.x,
        sun.y,
        sun.z,
        0.0, //
        sun_color.x,
        sun_color.y,
        sun_color.z,
        1.0 - 0.7 * rain,
    ]
}

/// The water surface mesh: a unit quad in the XZ plane, scaled to the field by the
/// entity's transform.
fn water_quad() -> Mesh {
    let corner = |x: f32, z: f32| Vertex {
        position: [x, 0.0, z],
        normal: [0.0, 1.0, 0.0],
        uv: [x + 0.5, z + 0.5],
        tangent: [1.0, 0.0, 0.0, 1.0],
        color: [1.0, 1.0, 1.0, 1.0],
    };
    Mesh {
        name: "water/surface".into(),
        vertices: vec![
            corner(-0.5, -0.5),
            corner(0.5, -0.5),
            corner(0.5, 0.5),
            corner(-0.5, 0.5),
        ],
        indices: vec![0, 1, 2, 0, 2, 3],
        ..Default::default()
    }
}

/// Spawn the water surface: a flat, translucent quad at the table's level, drawn by a
/// custom shader through the seam (its mesh form — the seam's first mesh draw). The
/// level is moved each frame in [`apply_frame`].
fn spawn_water(ctx: &mut EngineContext) -> Entity {
    let shader = ctx
        .assets
        .shaders
        .insert_named("water/surface", Shader::new("goats water", WATER_WGSL));
    let material = ctx.assets.shader_materials.insert_named(
        "water/surface",
        ShaderMaterial {
            name: "goats water".into(),
            shader,
            uniforms: WATER_UNIFORMS.to_vec(),
            alpha_mode: AlphaMode::Blend,
            double_sided: true,
            ..ShaderMaterial::default()
        },
    );
    let mesh = ctx
        .assets
        .meshes
        .insert_named("water/surface", water_quad());
    let water = ctx.scene.world.spawn();
    ctx.scene.world.insert(water, Name::new("Water"));
    ctx.scene.world.insert(
        water,
        LocalTransform::from_translation(math::vec3(0.0, 0.0, 0.0)).with_scale(math::vec3(
            2.0 * FIELD,
            1.0,
            2.0 * FIELD,
        )),
    );
    ctx.scene.world.insert(
        water,
        ShaderMeshRenderer {
            mesh,
            material,
            cast_shadows: false,
            receive_shadows: false,
        },
    );
    water
}

/// Spawn the bang's flash: one shadowless point light at zero intensity, which
/// [`apply_frame`] flares when a bang lands. It lights the goat and the ground
/// around the blast — the cheap half of the bang's look (M5).
fn spawn_flash(ctx: &mut EngineContext) -> Entity {
    let light = ctx.scene.world.spawn();
    ctx.scene.world.insert(light, Name::new("Flash"));
    ctx.scene.world.insert(light, LocalTransform::default());
    ctx.scene.world.insert(
        light,
        Light {
            kind: LightKind::Point { range: FLASH_RANGE },
            color: math::vec3(1.0, 0.62, 0.28),
            intensity: 0.0,
            cast_shadows: false,
            ..Light::default()
        },
    );
    light
}

/// Build the whole world: the ground, the meadow, the sky, then the goat. Called
/// by the plugin's frame system on the first tick, and by the harness directly.
pub fn spawn_world(ctx: &mut EngineContext) -> Option<World> {
    let craters = terrain::Craters::default();
    spawn_terrain(ctx, &craters);
    // The water table samples the ground, so it is built once the terrain is up and
    // before the meadow, which culls the tufts the table stands over.
    let water = water::Water::new(&craters);
    let water_level = water.level();
    let meadow = grass::Meadow::default();
    spawn_grass(ctx, &meadow, &water, &craters);
    let sky = spawn_sky(ctx);
    let water_surface = spawn_water(ctx);
    let flash = spawn_flash(ctx);
    // The bang's look (M5): the generated atlases and the sprite pools.
    let fx_textures = fx::FxTextures {
        fire: ctx
            .assets
            .textures
            .insert_named("fx/blast", fx::fire_atlas()),
        smoke: ctx
            .assets
            .textures
            .insert_named("fx/smoke", fx::smoke_atlas()),
        dust: ctx
            .assets
            .textures
            .insert_named("fx/dust", fx::dust_atlas()),
    };
    let fx_entities = spawn_billboard_pool(ctx, "Fx", FX_ENTITIES);
    let tell_entities = spawn_billboard_pool(ctx, "Tell", TELL_ENTITIES);
    let cloud_entities = spawn_billboard_pool(ctx, "Cloud", weather_fx::CLOUD_COUNT);
    let rain_entities = spawn_billboard_pool(ctx, "Rain", weather_fx::RAIN_COUNT);
    let goat = spawn_goat(ctx)?;
    Some(World {
        goat,
        meadow,
        weather: weather::Weather::default(),
        water,
        hazards: hazards::Hazards::default(),
        crater_rev: craters.revision(),
        craters,
        grass_cell: (0, 0),
        water_level,
        orbit: Orbit::default(),
        seeded: false,
        sky,
        sky_time: 0.0,
        water_surface,
        fx: fx::Fx::new(fx_textures),
        fx_textures,
        fx_entities,
        tell_entities,
        clouds: weather_fx::Clouds::ring(fx_textures.dust),
        rain: weather_fx::Rain::field(fx_textures.dust),
        cloud_entities,
        rain_entities,
        flash: Flash {
            light: flash,
            left: 0.0,
        },
        shake: Shake::default(),
    })
}

/// A pool of hidden billboard entities the simulation writes sprites onto each
/// frame (`sync_fx`). Spawning them once avoids per-burst entity churn; a pool
/// entity with no sprite is `Visible(false)`, so it draws nothing.
fn spawn_billboard_pool(ctx: &mut EngineContext, name: &str, count: usize) -> Vec<Entity> {
    (0..count)
        .map(|i| {
            let entity = ctx.scene.world.spawn();
            ctx.scene
                .world
                .insert(entity, Name::new(format!("{name} {i}")));
            ctx.scene.world.insert(entity, LocalTransform::default());
            ctx.scene.world.insert(entity, Visible(false));
            ctx.scene.world.insert(entity, Billboard::default());
            entity
        })
        .collect()
}

/// Build the ground mesh and put it in the scene under one entity.
fn spawn_terrain(ctx: &mut EngineContext, craters: &terrain::Craters) {
    let mesh = ctx
        .assets
        .meshes
        .insert_named("terrain/meadow", terrain::build_mesh(craters));
    let material = ctx.assets.materials.insert_named(
        "terrain/grass",
        Material {
            name: "terrain grass".to_string(),
            // The biome colour is per vertex (grass / sand / rock in `terrain`), so
            // the base is white and the vertex colours carry it.
            base_color: [1.0, 1.0, 1.0, 1.0],
            metallic: 0.0,
            roughness: 0.95,
            vertex_colors: true,
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

/// Rebuild the ground mesh in place after a crater (or its heal) moved it. The
/// path is unchanged, so the renderer re-uploads the same mesh with a new revision
/// rather than drawing the ground the blast left.
fn rebuild_terrain(ctx: &mut EngineContext, craters: &terrain::Craters) {
    ctx.assets
        .meshes
        .insert_named("terrain/meadow", terrain::build_mesh(craters));
}

/// Build the meadow: its mesh and material, under one entity. The mesh is rebuilt
/// in place by [`update_meadow`] as the goat moves or grazes.
fn spawn_grass(
    ctx: &mut EngineContext,
    meadow: &grass::Meadow,
    water: &water::Water,
    craters: &terrain::Craters,
) {
    let mesh = ctx.assets.meshes.insert_named(
        "grass/meadow",
        grass::build_mesh(meadow, 0.0, 0.0, 0.0, 0.0, Some(water), craters),
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
fn update_meadow(ctx: &mut EngineContext, world: &mut World, ate: bool, ground_changed: bool) {
    let dt = ctx.time.delta();
    let regrew = world.meadow.update(dt);

    let cell = (
        (world.goat.x / grass::CELL).floor() as i32,
        (world.goat.z / grass::CELL).floor() as i32,
    );
    // A rising table hides the tufts it reaches, so the field is rebuilt when it
    // moves enough to matter — the original culls live, per frame.
    let rose = (world.water.level() - world.water_level).abs() > WATER_MARK;
    if ate || regrew || rose || ground_changed || cell != world.grass_cell {
        world.grass_cell = cell;
        world.water_level = world.water.level();
        ctx.assets.meshes.insert_named(
            "grass/meadow",
            grass::build_mesh(
                &world.meadow,
                world.goat.x,
                world.goat.z,
                world.weather.sway_time(),
                // The grass's own wind sway waits on the shader seam (M5); the
                // field is a rebuilt mesh, so it would only lean on a rebuild.
                0.0,
                Some(&world.water),
                &world.craters,
            ),
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
        flung: first(role("flung")),
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
        fling_time: 0.0,
        fling_vx: 0.0,
        fling_vz: 0.0,
        fling_vy: 0.0,
        fling_y: 0.0,
        fling_flight: 0.0,
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
    fn a_flinged_goat_arcs_up_and_touches_down() {
        let mut goat = lone_goat();
        goat.start_fling(0.0, 0.0, 34.0, 0.0);
        assert_eq!(goat.mode, Mode::Flung);
        assert!(goat.fling_flight > 0.0, "the pose has a stretch to ride");

        let ground = |_: f32, _: f32| 0.0;
        let mut peak: f32 = 0.0;
        let mut landed = false;
        for _ in 0..600 {
            if goat.advance_fling(1.0 / 60.0, ground) {
                landed = true;
                break;
            }
            peak = peak.max(goat.fling_y);
        }
        assert!(landed, "the arc ends on the ground");
        assert!(peak > 5.0, "it goes up: {peak}");
        assert_eq!(goat.fling_y, 0.0, "and lands back on it");

        // A second blast mid-air adds to the velocity already there.
        let mut goat = lone_goat();
        goat.start_fling(10.0, 0.0, 20.0, 0.0);
        let vx = goat.fling_vx;
        goat.start_fling(10.0, 0.0, 0.0, 0.0);
        assert!(goat.mode == Mode::Flung);
        assert!(goat.fling_vx > vx, "the impulses stack");
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
            fling_time: 0.0,
            fling_vx: 0.0,
            fling_vz: 0.0,
            fling_vy: 0.0,
            fling_y: 0.0,
            fling_flight: 0.0,
        }
    }

    #[test]
    fn eating_a_tuft_tops_up_energy_and_belly() {
        let mut goat = lone_goat();
        goat.energy = 50.0;
        let mut meadow = grass::Meadow::default();
        let tuft = grass::nearest(0.0, 0.0, 6.0, &meadow, &terrain::Craters::default())
            .expect("a tuft in reach");

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
        let tuft = grass::nearest(0.0, 0.0, 6.0, &meadow, &terrain::Craters::default())
            .expect("a tuft in reach");

        goat.start_eat(&AssetRegistry::new(), &mut meadow, &tuft);
        assert!(
            goat.health > 50.0,
            "a full belly turns grass into health: {}",
            goat.health
        );
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
        // ...and each resolves to a real texture in the registry (a remapped
        // handle that names nothing would silently bind the white fallback).
        assert!(assets.textures.get(fur.base_color_texture).is_some());
        assert!(assets.textures.get(fur.normal_texture).is_some());
    }
}
