//! The herd (M4): the bot goats, ported from the original's `bots.js`.
//!
//! Bots roam on a small state machine — graze, stroll, trot, occasionally sprint,
//! rarely doze — with a per-bot size and temperament, steered home once they drift
//! too far and pushed apart when they overlap. Each owns its own node hierarchy and
//! animator (so two goats can hold different poses) but shares the goat's meshes,
//! skin and clips.

use crate::terrain;
use crate::{Clips, Mode, RUN_SPEED, TROT_SPEED, WALK_SPEED};
use crate::{grass, water, weather};
use noctis::prelude::*;

/// How far a bot looks for grass to walk to when it decides to graze.
const GRAZE_SEEK_RANGE: f32 = 12.0;
/// How far a bot may drift from the goat before it is steered home (metres).
const STRAY: f32 = 42.0;
/// The hard backstop: a bot this far out is pulled back in (metres).
const BACKSTOP: f32 = 80.0;
/// Radians a bot turns per second (`movement.turnRate`).
const TURN: f32 = 1.8;
/// A goat's collision radius at scale 1 (`movement.goatRadius`).
const RADIUS: f32 = 0.45;
/// A meal lasts this long when the clip's own duration is not known (`botStartEat`).
const EAT_FALLBACK_TIME: f32 = 1.0;

/// One bot's coat (sRGB bytes), body scale and temperament (`TUNING.herd.spec`).
#[derive(Clone, Copy, Debug)]
struct Spec {
    coat: [f32; 3],
    scale: f32,
    /// Scales the cruising speed.
    bold: f32,
    /// Biases it toward standing still.
    lazy: f32,
}

const SPECS: [Spec; 6] = [
    Spec {
        coat: [196.0, 168.0, 128.0],
        scale: 0.80,
        bold: 0.95,
        lazy: 0.55,
    },
    Spec {
        coat: [222.0, 216.0, 206.0],
        scale: 1.06,
        bold: 1.00,
        lazy: 0.50,
    },
    Spec {
        coat: [116.0, 92.0, 70.0],
        scale: 1.22,
        bold: 0.70,
        lazy: 0.72,
    },
    Spec {
        coat: [156.0, 126.0, 92.0],
        scale: 0.94,
        bold: 1.18,
        lazy: 0.32,
    },
    Spec {
        coat: [88.0, 90.0, 98.0],
        scale: 1.12,
        bold: 0.85,
        lazy: 0.62,
    },
    Spec {
        coat: [208.0, 180.0, 142.0],
        scale: 0.74,
        bold: 1.05,
        lazy: 0.45,
    },
];

/// The herd's own PRNG, kept apart from the weather's so its sequence is untouched.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        let mut s = self.0;
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        self.0 = s;
        s as f32 / 4_294_967_296.0
    }
}

/// One bot goat.
struct Bot {
    root: Entity,
    skinned: Entity,
    spec: Spec,
    /// The lift the model needs to rest on the ground (the goat's own).
    lift: f32,
    x: f32,
    z: f32,
    yaw: f32,
    mode: Mode,
    timer: f32,
    tx: f32,
    tz: f32,
    graze: bool,
    /// Seconds of "zoomies" left while running.
    zoom: f32,
    jump_time: f32,
    jump_dur: f32,
    jump_speed: f32,
    jump_cool: f32,
    eat_time: f32,
    eat_dur: f32,
    eat_cool: f32,
    satiety: f32,
}

impl Bot {
    /// The clip the current mode plays (`botRole`, on the player's own picker).
    fn clip(&self, clips: &Clips) -> AssetHandle<AnimationClip> {
        clips.pick(self.mode, 0)
    }

    /// Begin a new idea: graze, doze, or set off to a wander target
    /// (`botNewAction`).
    fn new_action(
        &mut self,
        rng: &mut Rng,
        goat: (f32, f32),
        meadow: &mut grass::Meadow,
        craters: &terrain::Craters,
        water: &water::Water,
    ) {
        // Bots graze too: eat what is under the nose, else walk to the nearest tuft.
        if self.eat_cool <= 0.0 && rng.next() < 0.6 {
            if let Some(tuft) = grass::nearest(self.x, self.z, grass::EAT_RANGE, meadow, craters) {
                self.start_eat(&tuft, meadow, None);
                return;
            }
            if let Some(tuft) = grass::nearest(self.x, self.z, GRAZE_SEEK_RANGE, meadow, craters) {
                self.graze = true;
                self.tx = tuft.x;
                self.tz = tuft.z;
                self.mode = Mode::Walk;
                self.timer = 14.0;
                self.zoom = 0.0;
                return;
            }
        }
        self.graze = false;
        if rng.next() < 0.30 + 0.35 * self.spec.lazy {
            let sleep = rng.next() < 0.10 && !water.in_water(self.x, self.z);
            self.mode = if sleep { Mode::Sleep } else { Mode::Idle };
            self.timer = if sleep {
                5.0 + rng.next() * 6.0
            } else {
                2.0 + rng.next() * 5.0
            };
            self.zoom = 0.0;
            return;
        }
        let s = rng.next();
        self.mode = if s < 0.60 {
            Mode::Walk
        } else if s < 0.90 {
            Mode::Trot
        } else {
            Mode::Run
        };
        self.timer = 3.0 + rng.next() * 9.0;
        self.zoom = if self.mode == Mode::Run {
            2.5 + rng.next() * 4.0
        } else {
            0.0
        };
        self.jump_cool = 0.4 + rng.next() * 1.0;
        let reach = match self.mode {
            Mode::Run => 22.0,
            Mode::Trot => 14.0,
            _ => 7.0,
        };
        let dist = reach + rng.next() * 14.0;
        let (dx, dz) = (goat.0 - self.x, goat.1 - self.z);
        let far = (dx * dx + dz * dz).sqrt();
        let a = if far > STRAY {
            (-dz).atan2(dx) + (rng.next() - 0.5) * 0.9
        } else {
            rng.next() * std::f32::consts::TAU
        };
        self.tx = self.x + a.cos() * dist;
        self.tz = self.z - a.sin() * dist;
    }

    /// Consume `tuft`: face it, fill the belly, play an eat clip, and take the clip's
    /// own duration when given (`botStartEat`).
    fn start_eat(&mut self, tuft: &grass::Tuft, meadow: &mut grass::Meadow, dur: Option<f32>) {
        meadow.consume(tuft);
        self.yaw = (-(tuft.z - self.z)).atan2(tuft.x - self.x);
        self.satiety = (self.satiety + crate::EAT_SATIETY).min(1.0);
        self.mode = Mode::Eat;
        self.eat_time = 0.0;
        self.eat_dur = dur.filter(|d| *d > 0.0).unwrap_or(EAT_FALLBACK_TIME);
        self.timer = self.eat_dur;
        self.eat_cool = 20.0 + self.spec.lazy * 40.0;
        self.graze = false;
        self.zoom = 0.0;
    }

    /// The ground speed of the current gait, scaled by the bot's boldness and the
    /// weather (`updateBots`), on the player's own stride speeds.
    fn speed(&self, weather: &weather::Weather) -> f32 {
        let base = match self.mode {
            Mode::Walk => WALK_SPEED,
            Mode::Trot => TROT_SPEED,
            Mode::Run => RUN_SPEED,
            _ => 0.0,
        };
        base * (0.75 + 0.5 * self.spec.bold) * weather.speed(self.satiety)
    }
}

/// The herd.
pub struct Herd {
    bots: Vec<Bot>,
    rng: Rng,
}

impl Herd {
    /// Spawn `count` bot goats around `goat`, sharing the loaded GLB's assets.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        ctx: &mut EngineContext,
        loaded: &LoadedGltf,
        handles: &RegisteredGltf,
        clips: &Clips,
        lift: f32,
        count: usize,
        goat: (f32, f32),
    ) -> Herd {
        let mut rng = Rng(0x2545_f491);
        let bots = (0..count)
            .filter_map(|i| {
                let spec = bot_spec(i);
                spawn_bot(ctx, loaded, handles, clips, lift, i, spec, goat, &mut rng)
            })
            .collect();
        Herd { bots, rng }
    }

    /// How many bots are alive.
    #[cfg(test)]
    pub fn count(&self) -> usize {
        self.bots.len()
    }

    /// How many bots are on the move (a stride, not standing or dozing).
    #[cfg(test)]
    pub fn moving(&self) -> usize {
        self.bots
            .iter()
            .filter(|b| matches!(b.mode, Mode::Walk | Mode::Trot | Mode::Run | Mode::Jump))
            .count()
    }

    /// The smallest gap between any two goats (the player and the bots); negative is
    /// an overlap (`goatMinGap`). The harness's collision check.
    #[cfg(test)]
    pub fn min_gap(&self, goat: (f32, f32)) -> f32 {
        let mut best = f32::INFINITY;
        for (i, b) in self.bots.iter().enumerate() {
            let dx = b.x - goat.0;
            let dz = b.z - goat.1;
            best = best.min((dx * dx + dz * dz).sqrt() - RADIUS * (1.0 + b.spec.scale));
            for c in &self.bots[i + 1..] {
                let dx = c.x - b.x;
                let dz = c.z - b.z;
                best =
                    best.min((dx * dx + dz * dz).sqrt() - RADIUS * (b.spec.scale + c.spec.scale));
            }
        }
        best
    }

    /// Advance every bot (`updateBots`), push overlapping goats apart, and write the
    /// transforms and clips.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        ctx: &mut EngineContext,
        clips: &Clips,
        goat: (f32, f32),
        dt: f32,
        meadow: &mut grass::Meadow,
        craters: &terrain::Craters,
        weather: &weather::Weather,
        water: &water::Water,
    ) {
        for i in 0..self.bots.len() {
            self.step(i, goat, dt, meadow, craters, weather, water);
        }
        // The hard backstop.
        for bot in &mut self.bots {
            let (px, pz) = (bot.x - goat.0, bot.z - goat.1);
            let far = (px * px + pz * pz).sqrt();
            if far > BACKSTOP {
                bot.x = goat.0 + (px / far) * BACKSTOP;
                bot.z = goat.1 + (pz / far) * BACKSTOP;
            }
        }
        self.collide(goat);

        // Write the transforms and the clip for this frame.
        for bot in &self.bots {
            let y = craters.surface(bot.x, bot.z) + bot.lift;
            if let Some(transform) = ctx.scene.world.get_mut::<LocalTransform>(bot.root) {
                transform.translation = math::vec3(bot.x, y, bot.z);
                transform.rotation = Quat::from_rotation_y(bot.yaw);
            }
            let clip = bot.clip(clips);
            if let Some(animator) = ctx.scene.world.get_mut::<Animator>(bot.skinned)
                && animator.clip != clip
            {
                animator.clip = clip;
                animator.time = 0.0;
                animator.playing = true;
                animator.looped = bot.mode.loops();
            }
        }
    }

    /// One bot's frame, borrowing `self.rng` and `self.bots[i]` disjointly.
    #[allow(clippy::too_many_arguments)]
    fn step(
        &mut self,
        i: usize,
        goat: (f32, f32),
        dt: f32,
        meadow: &mut grass::Meadow,
        craters: &terrain::Craters,
        weather: &weather::Weather,
        water: &water::Water,
    ) {
        // Split the borrow: `rng` and the slot are separate fields of the vector/self.
        let (rng, bots) = (&mut self.rng, &mut self.bots);
        let bot = &mut bots[i];
        bot.timer -= dt;
        if bot.jump_cool > 0.0 {
            bot.jump_cool -= dt;
        }
        if bot.eat_cool > 0.0 {
            bot.eat_cool -= dt;
        }
        bot.satiety = (bot.satiety - crate::SATIETY_DECAY * dt).max(0.0);
        // The rain gets a dozing bot up, as it does the player's goat.
        if bot.mode == Mode::Sleep && water.in_water(bot.x, bot.z) {
            bot.mode = Mode::Idle;
            bot.timer = 0.0;
        }

        match bot.mode {
            Mode::Jump => {
                bot.jump_time += dt;
                bot.x += bot.yaw.cos() * bot.jump_speed * dt;
                bot.z += -bot.yaw.sin() * bot.jump_speed * dt;
                if bot.jump_time >= bot.jump_dur {
                    if bot.zoom > 0.0 {
                        bot.mode = Mode::Run;
                        bot.jump_cool = 0.4 + rng.next() * 1.2;
                    } else {
                        bot.mode = Mode::Idle;
                        bot.timer = 1.5 + rng.next() * 3.0;
                    }
                }
            }
            Mode::Eat => {
                bot.eat_time += dt;
                if bot.eat_time >= bot.eat_dur {
                    bot.mode = Mode::Idle;
                    bot.timer = 1.5 + rng.next() * 3.0;
                }
            }
            Mode::Idle | Mode::Sleep => {
                if bot.timer <= 0.0 {
                    bot.new_action(rng, goat, meadow, craters, water);
                }
            }
            _ => {
                let spd = bot.speed(weather);
                if spd > 0.0 {
                    let (dx, dz) = (bot.tx - bot.x, bot.tz - bot.z);
                    let d = (dx * dx + dz * dz).sqrt();
                    let arrive = if bot.graze { 0.7 } else { 1.6 };
                    if d < arrive || bot.timer <= 0.0 {
                        if bot.graze {
                            bot.graze = false;
                            match grass::nearest(bot.x, bot.z, grass::EAT_RANGE, meadow, craters) {
                                Some(tuft) => bot.start_eat(&tuft, meadow, None),
                                None => {
                                    bot.mode = Mode::Idle;
                                    bot.timer = 1.5 + rng.next() * 3.0;
                                }
                            }
                        } else if bot.mode == Mode::Run && bot.zoom > 0.0 {
                            let a = rng.next() * std::f32::consts::TAU;
                            let r = 9.0 + rng.next() * 12.0;
                            bot.tx = bot.x + a.cos() * r;
                            bot.tz = bot.z - a.sin() * r;
                            bot.timer = 2.0 + rng.next() * 3.0;
                        } else {
                            bot.mode = Mode::Idle;
                            bot.timer = 1.5 + rng.next() * 4.0;
                        }
                    } else {
                        let want = (-dz).atan2(dx);
                        let mut diff = want - bot.yaw;
                        while diff > std::f32::consts::PI {
                            diff -= std::f32::consts::TAU;
                        }
                        while diff < -std::f32::consts::PI {
                            diff += std::f32::consts::TAU;
                        }
                        bot.yaw += diff.clamp(-TURN * dt, TURN * dt);
                        bot.x += bot.yaw.cos() * spd * dt;
                        bot.z += -bot.yaw.sin() * spd * dt;
                    }
                } else if bot.timer <= 0.0 {
                    bot.new_action(rng, goat, meadow, craters, water);
                }

                // Zoomies: while a run lasts, hop every second or so.
                if bot.mode == Mode::Run {
                    bot.zoom -= dt;
                    if bot.zoom > 0.0 && bot.jump_cool <= 0.0 && spd > 0.0 {
                        bot.mode = Mode::Jump;
                        bot.jump_time = 0.0;
                        bot.jump_dur = 0.6;
                        bot.jump_speed = spd * 1.15;
                        bot.jump_cool = 0.5 + rng.next() * 1.3;
                    }
                }
            }
        }
    }

    /// Push overlapping goats apart (`resolveGoatCollisions`): bots yield fully to the
    /// player and split the push evenly with each other.
    fn collide(&mut self, goat: (f32, f32)) {
        for _pass in 0..2 {
            let mut moved = false;
            for bot in &mut self.bots {
                let rr = RADIUS + RADIUS * bot.spec.scale;
                let (dx, dz) = (bot.x - goat.0, bot.z - goat.1);
                let d2 = dx * dx + dz * dz;
                if d2 < rr * rr {
                    if d2 <= 1e-4 {
                        bot.x = goat.0 + rr;
                    } else {
                        let d = d2.sqrt();
                        let push = (rr - d) / d;
                        bot.x += dx * push;
                        bot.z += dz * push;
                    }
                    if bot.timer > 0.6 {
                        bot.timer = 0.6;
                    }
                    moved = true;
                }
            }
            for i in 0..self.bots.len() {
                for j in (i + 1)..self.bots.len() {
                    let (left, right) = self.bots.split_at_mut(j);
                    let (a, b) = (&mut left[i], &mut right[0]);
                    let rr = RADIUS * a.spec.scale + RADIUS * b.spec.scale;
                    let (dx, dz) = (b.x - a.x, b.z - a.z);
                    let d2 = dx * dx + dz * dz;
                    if d2 < rr * rr && d2 > 1e-4 {
                        let d = d2.sqrt();
                        let push = (rr - d) / (2.0 * d);
                        a.x -= dx * push;
                        a.z -= dz * push;
                        b.x += dx * push;
                        b.z += dz * push;
                        moved = true;
                    }
                }
            }
            if !moved {
                break;
            }
        }
    }
}

/// A bot `i`'s spec: the table, with the coats recycling past the sixth (`botSpec`).
fn bot_spec(i: usize) -> Spec {
    let base = SPECS[i % SPECS.len()];
    let cycle = i / SPECS.len();
    Spec {
        scale: base.scale * (1.0 + 0.08 * cycle as f32),
        ..base
    }
}

/// Spawn one bot: its own node hierarchy under a root, its own animator, and its
/// materials tinted from its coat.
#[allow(clippy::too_many_arguments)]
fn spawn_bot(
    ctx: &mut EngineContext,
    loaded: &LoadedGltf,
    handles: &RegisteredGltf,
    clips: &Clips,
    lift: f32,
    index: usize,
    spec: Spec,
    goat: (f32, f32),
    rng: &mut Rng,
) -> Option<Bot> {
    // A golden-angle spread rings the player evenly for any herd size (`botAdd`).
    let a = index as f32 * 2.399_963 + 0.3;
    let r = 6.0 + rng.next() * 14.0;
    let x = goat.0 + a.cos() * r;
    let z = goat.1 + a.sin() * r;

    let nodes = loaded.instantiate(&mut ctx.scene.world, handles);
    let root = ctx.scene.world.spawn();
    ctx.scene
        .world
        .insert(root, Name::new(format!("Bot {index}")));
    ctx.scene.world.insert(
        root,
        LocalTransform::from_translation(math::vec3(x, terrain::height(x, z) + lift, z))
            .with_scale(math::vec3(spec.scale, spec.scale, spec.scale)),
    );
    for &node in &loaded.roots {
        if let Some(&entity) = nodes.get(node) {
            set_parent(&mut ctx.scene.world, entity, Some(root));
        }
    }
    let skinned = crate::skinned_in(&ctx.scene.world, &nodes)?;
    ctx.scene
        .world
        .insert(skinned, Animator::new(clips.pick(Mode::Idle, 0)));

    // Tint the bot's own materials from its coat, so the herd reads as individuals.
    tint_materials(ctx, &nodes, index, spec.coat);

    Some(Bot {
        root,
        skinned,
        spec,
        x,
        z,
        yaw: a + std::f32::consts::PI,
        lift,
        mode: Mode::Idle,
        timer: 0.5 + rng.next() * 3.0,
        tx: 0.0,
        tz: 0.0,
        graze: false,
        zoom: 0.0,
        jump_time: 0.0,
        jump_dur: 0.6,
        jump_speed: 0.0,
        jump_cool: 0.0,
        eat_time: 0.0,
        eat_dur: 1.0,
        eat_cool: 0.0,
        satiety: 0.0,
    })
}

/// Give a bot its own copy of each material, tinted from `coat` (the original gives
/// each bot its own fleece texture).
fn tint_materials(ctx: &mut EngineContext, nodes: &[Entity], index: usize, coat: [f32; 3]) {
    let tint = [coat[0] / 255.0, coat[1] / 255.0, coat[2] / 255.0, 1.0];
    for &node in nodes {
        for child in children_of(&ctx.scene.world, node) {
            let Some(material) = ctx
                .scene
                .world
                .get::<SkinnedMeshRenderer>(child)
                .map(|renderer| renderer.material)
            else {
                continue;
            };
            let Some(original) = ctx.assets.materials.get(material) else {
                continue;
            };
            let name = format!("bot{index}/{}", original.name);
            let mut tinted = original.clone();
            tinted.name = name.clone();
            tinted.base_color = tint;
            let handle = ctx.assets.materials.insert_named(name, tinted);
            if let Some(renderer) = ctx.scene.world.get_mut::<SkinnedMeshRenderer>(child) {
                renderer.material = handle;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_specs_recycle_past_the_table() {
        for i in 0..6 {
            assert_eq!(bot_spec(i).scale, SPECS[i % 6].scale);
        }
        // The seventh recycles the first coat with a bigger body (`botSpec`).
        assert!(bot_spec(6).scale > SPECS[0].scale);
    }

    #[test]
    fn the_rng_stays_in_range() {
        let mut rng = Rng(0x2545_f491);
        let mut seen_low = false;
        let mut seen_high = false;
        for _ in 0..1000 {
            let v = rng.next();
            assert!((0.0..1.0).contains(&v), "in [0, 1): {v}");
            seen_low |= v < 0.5;
            seen_high |= v >= 0.5;
        }
        assert!(seen_low && seen_high, "the stream varies");
    }
}
