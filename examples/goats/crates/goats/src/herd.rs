//! The herd (M4): the bot goats, ported from the original's `bots.js`.
//!
//! Bots roam on a small state machine — graze, stroll, trot, occasionally sprint,
//! rarely doze — with a per-bot size and temperament, steered home once they drift
//! too far and pushed apart when they overlap. Each owns its own node hierarchy and
//! animator (so two goats can hold different poses) but shares the goat's meshes,
//! skin and clips.
//!
//! The herd is mortal (M19a): a bang it walks into — a mine it steps on or a tuft
//! it grazes that was trapped — damages it, and unlike the player a bot has no
//! health floor, so a bang can kill it. A dead bot lies where it fell, plays the
//! death clip, and gets up somewhere else seconds later. The bots also wear their
//! own generated fleeces (`makeBotTextures`), one mottled coat each.

use crate::hazards::{self, Hazards};
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
/// Seconds a killed bot lies there before it gets up (`herd.deathLinger`).
const DEATH_LINGER: f32 = 8.0;
/// Metres from the goat a respawning bot lands, at least and at most
/// (`botRespawn`).
const RESPAWN_MIN: f32 = 10.0;
const RESPAWN_MAX: f32 = 16.0;
/// Candidate spawns before a bot gives up and takes the last one — a dense field
/// may leave no clean ground (`botRespawn`).
const RESPAWN_TRIES: u32 = 8;
/// The most goats the settings let the herd hold (`TUNING_CLAMP["herd.count"]`).
pub const HERD_MAX: usize = 100;

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
    /// Blast damage (M19a): `0..=MAX_STATS`, and unlike the player a bot has no
    /// floor — a bang can kill it.
    health: f32,
    /// Seconds since it died, and the death clip's length the pose stretches over.
    death_time: f32,
    death_dur: f32,
    /// The blast's arc (M19c): the height above the ground it is drawn at, the
    /// absolute height the physics integrates, the world velocity, and the
    /// ballistic time the pose is stretched over.
    py: f32,
    fly_time: f32,
    fly_y: f32,
    fly_flight: f32,
    fly_vx: f32,
    fly_vy: f32,
    fly_vz: f32,
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
        hazards: &mut Hazards,
    ) {
        // Bots graze too: eat what is under the nose, else walk to the nearest tuft.
        if self.eat_cool <= 0.0 && rng.next() < 0.6 {
            if let Some(tuft) = grass::nearest(self.x, self.z, grass::EAT_RANGE, meadow, craters) {
                self.start_eat(&tuft, meadow, hazards, None);
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
    /// own duration when given (`botStartEat`). A trapped tuft is a bang for a bot
    /// too: the tuft is gone whichever way this goes, but a trapped one trades the
    /// meal for a device on its fuse.
    fn start_eat(
        &mut self,
        tuft: &grass::Tuft,
        meadow: &mut grass::Meadow,
        hazards: &mut Hazards,
        dur: Option<f32>,
    ) {
        meadow.consume(tuft);
        self.yaw = (-(tuft.z - self.z)).atan2(tuft.x - self.x);
        if !hazards.trip_trap(tuft, meadow) {
            // The herd's meals get the second helping too (`satedHeal`): a full belly
            // heals a little, the only way back from a minefield's bruises.
            let heal = if self.satiety >= crate::FULL_BELLY {
                crate::FULL_BELLY_HEAL
            } else {
                0.0
            };
            self.satiety = (self.satiety + crate::EAT_SATIETY).min(1.0);
            self.health = (self.health + heal).min(crate::MAX_STATS);
        }
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

    /// Whether the bot is over the ground rather than on it (`botAirborne`): a jump
    /// is airborne by mode (the clip's root motion does the hop) and a flung bot
    /// owns its own height. A landing is when this stops being true, which is what
    /// lets a bot set off a mine it comes down on.
    fn airborne(&self) -> bool {
        matches!(self.mode, Mode::Jump | Mode::Flung)
    }

    /// Take a blast's impulse (`startBotFling`), the player's [`crate::Goat`]
    /// `start_fling` in a bot's fields. A second bang mid-arc adds to the arc rather
    /// than restarting it, so a chain reads as one continuous throw.
    fn start_fling(&mut self, vx: f32, vz: f32, vy: f32, base_y: f32) {
        if self.mode != Mode::Flung {
            self.fly_y = base_y; // the arc starts where the bot is
            self.fly_time = 0.0;
            self.fly_vx = 0.0;
            self.fly_vz = 0.0;
            self.fly_vy = 0.0;
            self.mode = Mode::Flung;
            self.timer = 0.0;
            self.graze = false;
            self.zoom = 0.0;
        }
        self.fly_vx += vx;
        self.fly_vz += vz;
        self.fly_vy += vy;
        let air = if self.fly_vy > 0.0 && crate::FLING_GRAVITY < 0.0 {
            -2.0 * self.fly_vy / crate::FLING_GRAVITY
        } else {
            0.4
        };
        self.fly_flight = (self.fly_time + air).clamp(0.3, crate::FLING_MAX_FLIGHT);
    }

    /// The fraction through the arc the pose is stretched over (`botFlingProgress`).
    fn fling_progress(&self) -> f32 {
        (self.fly_time / self.fly_flight).min(1.0)
    }

    /// One frame of the arc, integrated in absolute height so a slope the bot
    /// crosses mid-air cannot drag it. Returns whether the feet touched the ground,
    /// which is the ground it actually meets (`advance_fling`).
    fn fly(&mut self, dt: f32, craters: &terrain::Craters) -> bool {
        self.x += self.fly_vx * dt;
        self.z += self.fly_vz * dt;
        self.fly_vy += crate::FLING_GRAVITY * dt;
        self.fly_y += self.fly_vy * dt;
        let ground = craters.surface(self.x, self.z) + self.lift;
        if self.fly_vy <= 0.0 && self.fly_y - ground <= 0.0 {
            self.fly_y = ground;
            self.py = 0.0;
            self.fly_vx = 0.0;
            self.fly_vz = 0.0;
            self.fly_vy = 0.0;
            true
        } else {
            self.py = self.fly_y - ground;
            false
        }
    }

    /// Fall dead (`botDie`): the death clip plays and the body lies where it fell. A
    /// lethal bang kills instead of throwing, so a corpse keeps the arc it already
    /// had — the mode change does not clear `fly_*`.
    fn die(&mut self, death_dur: f32) {
        self.health = 0.0;
        self.mode = Mode::Dead;
        self.death_time = 0.0;
        self.death_dur = death_dur;
        self.graze = false;
        self.zoom = 0.0;
    }

    /// Bring a dead bot back (`botRespawn`): somewhere else, facing the goat, whole.
    /// Ground with no armed mine under it is preferred, so a bot does not respawn
    /// onto one and blow up before it takes a step, but the search is bounded — a
    /// dense field has no clean ground left to offer.
    fn respawn(&mut self, rng: &mut Rng, goat: (f32, f32), hazards: &Hazards) {
        let (mut x, mut z) = (self.x, self.z);
        for _ in 0..RESPAWN_TRIES {
            let a = rng.next() * std::f32::consts::TAU;
            let r = RESPAWN_MIN + rng.next() * (RESPAWN_MAX - RESPAWN_MIN);
            x = goat.0 + a.cos() * r;
            z = goat.1 + a.sin() * r;
            if !hazards.mine_armed((x / 2.0).floor() as i32, (z / 2.0).floor() as i32) {
                break;
            }
        }
        self.x = x;
        self.z = z;
        self.yaw = (z - goat.1).atan2(goat.0 - x);
        self.health = crate::MAX_STATS;
        self.death_time = 0.0;
        self.py = 0.0;
        self.fly_time = 0.0;
        self.fly_flight = 1.0;
        self.fly_vx = 0.0;
        self.fly_vy = 0.0;
        self.fly_vz = 0.0;
        self.satiety = 0.0;
        self.graze = false;
        self.zoom = 0.0;
        self.mode = Mode::Idle;
        self.timer = 1.5 + rng.next() * 3.0;
    }
}

/// The herd.
pub struct Herd {
    bots: Vec<Bot>,
    rng: Rng,
    /// The death clip's length, which a dying bot's pose is stretched over.
    death_dur: f32,
    /// The generated fleeces, so a bot that spawns later (a herd resize) wears
    /// one too (`BOT_TEX`).
    fleeces: Vec<AssetHandle<Texture>>,
}

impl Herd {
    /// Spawn `count` bot goats around `goat`, sharing the loaded GLB's assets. The
    /// fleeces are generated here too, one mottled texture per coat
    /// (`makeBotTextures`), and registered once for the whole herd.
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
        let fleeces: Vec<AssetHandle<Texture>> = SPECS
            .iter()
            .enumerate()
            .map(|(i, spec)| {
                ctx.assets
                    .textures
                    .insert_named(format!("bot/fleece{i}"), fleece_texture(i, spec.coat))
            })
            .collect();
        // The death clip drives the corpse's pose; with no clip the model rests at
        // `1` (the player's own fallback), and the pose divides by it.
        let death_dur = ctx
            .assets
            .animations
            .get(clips.death)
            .map(|clip| clip.duration)
            .filter(|duration| *duration > 0.0)
            .unwrap_or(1.0);
        let mut rng = Rng(0x2545_f491);
        let bots = (0..count)
            .filter_map(|i| {
                let spec = bot_spec(i);
                let fleece = fleeces[i % fleeces.len()];
                spawn_bot(
                    ctx, loaded, handles, clips, lift, i, spec, fleece, goat, &mut rng,
                )
            })
            .collect();
        Herd {
            bots,
            rng,
            death_dur,
            fleeces,
        }
    }

    /// Grow or shrink the herd to `count` (`setHerdSize`): unload the goats past
    /// the count, and load the ones short of it — each at its own index, so the
    /// coats and sizes are the ones a fresh herd would have had.
    #[allow(clippy::too_many_arguments)]
    pub fn resize(
        &mut self,
        ctx: &mut EngineContext,
        loaded: &LoadedGltf,
        handles: &RegisteredGltf,
        clips: &Clips,
        lift: f32,
        count: usize,
        goat: (f32, f32),
    ) {
        let count = count.min(HERD_MAX);
        while self.bots.len() > count {
            if let Some(bot) = self.bots.pop() {
                despawn_tree(&mut ctx.scene.world, bot.root);
            }
        }
        while self.bots.len() < count {
            let index = self.bots.len();
            let spec = bot_spec(index);
            let fleece = self.fleeces[index % self.fleeces.len()];
            // A bot that cannot be built stops the growth, as `botAdd`'s `false` does.
            let Some(bot) = spawn_bot(
                ctx,
                loaded,
                handles,
                clips,
                lift,
                index,
                spec,
                fleece,
                goat,
                &mut self.rng,
            ) else {
                break;
            };
            self.bots.push(bot);
        }
    }

    /// Apply a bang to the herd (`blast`): the player's own curve off the player's
    /// radius and damage, but without the player's floor — a bot is not the promise
    /// that a mine cannot end a run, so three bangs on the centre kill it and one at
    /// the rim is a bruise. A lethal bang kills instead of throwing.
    pub fn blast(&mut self, x: f32, z: f32, seed: f32, craters: &terrain::Craters) {
        let death_dur = self.death_dur;
        let r = hazards::BLAST_RADIUS;
        for bot in &mut self.bots {
            if bot.mode == Mode::Dead {
                continue; // a corpse takes no second bang
            }
            let (bx, bz) = (bot.x - x, bot.z - z);
            let bd2 = bx * bx + bz * bz;
            if bd2 > r * r {
                continue;
            }
            let bd = bd2.sqrt();
            let falloff = 1.0 - bd / r;
            bot.health = (bot.health - hazards::BLAST_DAMAGE * falloff * falloff).max(0.0);
            if bot.health <= 0.0 {
                bot.die(death_dur);
                continue;
            }
            let (ux, uz) = hazards::fling_direction(bx, bz, bd, seed);
            let base = craters.surface(bot.x, bot.z) + bot.lift;
            bot.start_fling(
                ux * hazards::BLAST_PUSH * falloff,
                uz * hazards::BLAST_PUSH * falloff,
                hazards::BLAST_LIFT * falloff,
                base,
            );
        }
    }

    /// How many bots are in the herd.
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
        hazards: &mut Hazards,
    ) {
        for i in 0..self.bots.len() {
            self.step(i, goat, dt, meadow, craters, weather, water, hazards);
        }
        // The hard backstop: a corpse lies where it fell (`updateBots` skips this for
        // a dead bot), everyone else is pulled back in if it strays.
        for bot in &mut self.bots {
            if bot.mode == Mode::Dead {
                continue;
            }
            let (px, pz) = (bot.x - goat.0, bot.z - goat.1);
            let far = (px * px + pz * pz).sqrt();
            if far > BACKSTOP {
                bot.x = goat.0 + (px / far) * BACKSTOP;
                bot.z = goat.1 + (pz / far) * BACKSTOP;
            }
        }
        self.collide(goat);

        // Write the transforms and the clip for this frame. A thrown bot is drawn at
        // its arc's height and *posed* from the fraction (speed 0), and a corpse is
        // posed through the death clip the same way (the original's `pose`).
        for bot in &self.bots {
            let y = craters.surface(bot.x, bot.z) + bot.lift + bot.py;
            if let Some(transform) = ctx.scene.world.get_mut::<LocalTransform>(bot.root) {
                transform.translation = math::vec3(bot.x, y, bot.z);
                transform.rotation = Quat::from_rotation_y(bot.yaw);
            }
            let clip = bot.clip(clips);
            let duration = ctx
                .assets
                .animations
                .get(clip)
                .map(|clip| clip.duration)
                .unwrap_or(0.0);
            if let Some(animator) = ctx.scene.world.get_mut::<Animator>(bot.skinned) {
                if animator.clip != clip {
                    animator.clip = clip;
                    animator.time = 0.0;
                    animator.playing = true;
                }
                animator.looped = bot.mode.loops();
                match bot.mode {
                    Mode::Flung => {
                        animator.speed = 0.0;
                        animator.time = bot.fling_progress() * duration;
                    }
                    Mode::Dead => {
                        animator.speed = 0.0;
                        animator.time = (bot.death_time / bot.death_dur).min(1.0) * duration;
                    }
                    _ => animator.speed = 1.0,
                }
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
        hazards: &mut Hazards,
    ) {
        // Split the borrow: `rng` and the slot are separate fields of the vector/self.
        let (rng, bots) = (&mut self.rng, &mut self.bots);
        let bot = &mut bots[i];

        // Dead (M19a): the body lies where it fell, playing the death clip, and
        // nothing else happens to it — no AI, no grazing, and no devices, since a
        // corpse does not set off a mine. The one thing a corpse still does is
        // *fall* (a bot killed mid-arc keeps the arc it had), and `DEATH_LINGER`
        // seconds later it gets up somewhere else.
        if bot.mode == Mode::Dead {
            bot.death_time += dt;
            if bot.py > 0.0 {
                bot.fly(dt, craters);
            }
            if bot.death_time >= DEATH_LINGER {
                bot.respawn(rng, goat, hazards);
            }
            return;
        }

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
            Mode::Flung => {
                // The arc (M19c), the player's exactly: an absolute height with `py`
                // derived from it, so a slope a bot crosses mid-air cannot drag it,
                // and the ground it actually meets that ends the flight. There is no
                // AI while it flies; a bot that lands takes up its next idea.
                bot.fly_time += dt;
                if bot.fly(dt, craters) {
                    bot.mode = Mode::Idle;
                    bot.timer = 1.5 + rng.next() * 3.0;
                }
            }
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
                    bot.new_action(rng, goat, meadow, craters, water, hazards);
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
                                Some(tuft) => bot.start_eat(&tuft, meadow, hazards, None),
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
                    bot.new_action(rng, goat, meadow, craters, water, hazards);
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

        // A bot walking over a mine or a booby-trapped tuft trips it
        // (`updateExplosions`). A sleeping bot is not moving, so it does not; a
        // sleeping-or-airborne test is `check_triggers`'s own.
        if bot.mode != Mode::Sleep {
            hazards.check_triggers(bot.x, bot.z, bot.airborne(), meadow);
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

/// One bot's observable state, for the harness (`.notes/harness.md`).
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub struct BotState {
    pub x: f32,
    pub z: f32,
    pub health: f32,
    pub dead: bool,
    pub flung: bool,
    pub py: f32,
}

#[cfg(test)]
impl Herd {
    /// Every bot's state, in spawn order.
    pub fn states(&self) -> Vec<BotState> {
        self.bots
            .iter()
            .map(|bot| BotState {
                x: bot.x,
                z: bot.z,
                health: bot.health,
                dead: bot.mode == Mode::Dead,
                flung: bot.mode == Mode::Flung,
                py: bot.py,
            })
            .collect()
    }

    /// Stand bot `index` at `(x, z)`, awake and rooted for a while, so a case can
    /// put a bot where it needs one.
    pub fn place(&mut self, index: usize, x: f32, z: f32) {
        if let Some(bot) = self.bots.get_mut(index) {
            bot.x = x;
            bot.z = z;
            bot.mode = Mode::Idle;
            bot.timer = 100.0;
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

/// Despawn an entity and its descendants (the editor's `destroy`). A bot's root
/// owns its whole instantiated node hierarchy, so unloading a goat is one pass.
fn despawn_tree(world: &mut World, entity: Entity) {
    for child in children_of(world, entity) {
        despawn_tree(world, child);
    }
    world.despawn(entity);
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
    fleece: AssetHandle<Texture>,
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

    // Dress the bot in its own fleece, so the herd reads as individuals.
    coat_materials(ctx, &nodes, index, fleece);

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
        health: crate::MAX_STATS,
        death_time: 0.0,
        death_dur: 1.0,
        py: 0.0,
        fly_time: 0.0,
        fly_y: terrain::height(x, z) + lift,
        fly_flight: 1.0,
        fly_vx: 0.0,
        fly_vy: 0.0,
        fly_vz: 0.0,
    })
}

/// Give a bot its own copy of each material, wearing its fleece texture (`botAdd`
/// sets the model's texture 0, its diffuse): the model's own maps are kept for
/// everything else, and only the base colour becomes the coat.
fn coat_materials(
    ctx: &mut EngineContext,
    nodes: &[Entity],
    index: usize,
    fleece: AssetHandle<Texture>,
) {
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
            let mut coated = original.clone();
            coated.name = name.clone();
            coated.base_color_texture = fleece;
            // The fleece carries the coat colour, so the base is white (the original
            // draws the bot with an untinted `rl.WHITE`).
            coated.base_color = [1.0, 1.0, 1.0, 1.0];
            let handle = ctx.assets.materials.insert_named(name, coated);
            if let Some(renderer) = ctx.scene.world.get_mut::<SkinnedMeshRenderer>(child) {
                renderer.material = handle;
            }
        }
    }
}

/// One bot's mottled fleece texture, tinted from its coat (`makeBotTextures`): two
/// octaves of value noise over the coat, in sRGB like the model's own maps.
fn fleece_texture(index: usize, coat: [f32; 3]) -> Texture {
    const N: u32 = 64;
    let i = index as f32;
    let mut data = vec![0u8; (N * N * 4) as usize];
    for y in 0..N {
        for x in 0..N {
            let (xf, yf) = (x as f32, y as f32);
            let n = 0.55 * vnoise2(xf * 0.17 + i * 13.7, yf * 0.17 + i * 5.1)
                + 0.30 * vnoise2(xf * 0.39 + i * 3.3, yf * 0.39 + i * 9.9)
                + 0.15 * vnoise2(xf * 0.86 + i, yf * 0.86 + i * 2.0);
            let shade = 0.62 + 0.70 * (n - 0.5);
            let px = ((y * N + x) * 4) as usize;
            for c in 0..3 {
                data[px + c] = (coat[c] * shade).clamp(0.0, 255.0).round() as u8;
            }
            data[px + 3] = 255;
        }
    }
    Texture {
        name: format!("bot/fleece{index}"),
        width: N,
        height: N,
        format: TextureFormat::Rgba8Srgb,
        data,
    }
}

/// Value noise at `(x, y)` in `0..1` (the original's `vnoise2`).
fn vnoise2(x: f32, y: f32) -> f32 {
    let xi = x.floor();
    let yi = y.floor();
    let xf = x - xi;
    let yf = y - yi;
    let u = xf * xf * (3.0 - 2.0 * xf);
    let v = yf * yf * (3.0 - 2.0 * yf);
    let a = hash(xi * 57.0 + yi * 131.0);
    let b = hash((xi + 1.0) * 57.0 + yi * 131.0);
    let c = hash(xi * 57.0 + (yi + 1.0) * 131.0);
    let d = hash((xi + 1.0) * 57.0 + (yi + 1.0) * 131.0);
    (a * (1.0 - u) + b * u) * (1.0 - v) + (c * (1.0 - u) + d * u) * v
}

/// The original's cheap sin-hash, a value in `0..1` (`hash`).
fn hash(n: f32) -> f32 {
    let s = (n * 127.1).sin() * 43_758.547;
    s - s.floor()
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

    #[test]
    fn the_fleece_is_mottled_and_wears_the_coat() {
        let coat = SPECS[0].coat;
        let fleece = fleece_texture(0, coat);
        assert_eq!((fleece.width, fleece.height), (64, 64));
        assert_eq!(fleece.data.len(), 64 * 64 * 4);
        assert!(
            fleece.data.chunks(4).all(|px| px[3] == 255),
            "the fleece is opaque"
        );
        // The noise mottles the coat rather than painting one flat colour...
        let r_min = fleece.data.chunks(4).map(|px| px[0]).min().unwrap();
        let r_max = fleece.data.chunks(4).map(|px| px[0]).max().unwrap();
        assert!(r_max - r_min > 10, "mottled red: {r_min}..{r_max}");
        // ...and the coat's own ordering survives (a tan rounder than it is blue).
        let blue = fleece.data.chunks(4).map(|px| px[2]).max().unwrap();
        assert!(r_max > blue, "the coat reads tan: r {r_max} > b {blue}");
    }
}
