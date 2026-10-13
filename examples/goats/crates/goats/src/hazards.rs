//! Mines, boobytraps, blasts and the flung goat, ported from the original's
//! `explosions.js`.
//!
//! Two devices, both *derived* rather than placed: a 2-unit cell either holds a
//! mine or does not, and a share of the tufts are trapped, from a hash of the cell
//! salted per session (`SALT`). Nothing is stored at boot, so the field is a pure
//! function of position. The only state is where the devices *were*: which cells
//! have gone off, and which cells a replacement moved into — so the field drifts
//! rather than thinning out.
//!
//! A device the goat comes within `MINE_TRIGGER` of leaves the field and goes on a
//! fuse; when the fuse runs out the bang deals damage with a falloff and a floor
//! (a mine cannot end the player's run), dishes a crater into the ground, and
//! throws the goat on a ballistic arc. Neighbouring mines chain, one level deep.
//!
//! The bang's *look* — the fireball and smoke atlases, the debris, the light, the
//! camera's knock and the red damage pulse — is the presentation layer
//! (`.notes/goats.md` M5); only the devices, the blast and the arc live here.

use std::collections::HashSet;

use crate::terrain::{Craters, hash};
use crate::{Goat, Mode, grass};

/// Metres around the spawn with no devices (`explosions.safe`), so `restart()`
/// always puts the goat somewhere survivable.
const SAFE: f32 = 8.0;
/// Seconds between the trigger and the bang (`explosions.fuse`).
const FUSE: f32 = 0.18;
/// Chance per 2-unit cell of a mine (`mine.density`), one per ~330 m².
const MINE_DENSITY: f32 = 0.012;
/// Metres a goat has to come within to trip a device (`mine.trigger`).
const MINE_TRIGGER: f32 = 0.6;
/// Share of the tufts that are trapped (`trap.chance`).
const TRAP_CHANCE: f32 = 0.04;
/// Metres from the cell a device left that its replacement lands (`relocate.min`).
const RELOCATE_MIN: f32 = 8.0;
/// ...at most, so a device stays in its own field (`relocate.max`).
const RELOCATE_MAX: f32 = 24.0;
/// Candidate draws before the device is simply not replaced (`relocate.tries`).
const RELOCATE_TRIES: i32 = 12;
/// Metres of the blast's reach (`blast.radius`).
const BLAST_RADIUS: f32 = 3.2;
/// Damage at the centre, falling to zero at the rim (`blast.damage`).
const BLAST_DAMAGE: f32 = 45.0;
/// A blast cannot take health below this (`blast.healthFloor`): the promise that a
/// mine cannot end the player's run.
const BLAST_HEALTH_FLOOR: f32 = 1.0;
/// m/s of outward velocity at the centre (`blast.push`).
const BLAST_PUSH: f32 = 13.0;
/// m/s of upward velocity at the centre (`blast.lift`).
const BLAST_LIFT: f32 = 34.0;
/// How far a cascade goes; `1` = neighbours only (`chainDepth`).
const CHAIN_DEPTH: u32 = 1;
/// The per-session trap salt (`trapSalt`; the port has no session seeding, so the
/// original's default).
const SALT: i32 = 0x2f6e_2b1d;

/// The tau constant, as the original writes `6.283185307179586`.
const TAU: f32 = std::f32::consts::TAU;

/// A device: a mine at a cell's centre, or a trapped tuft.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Device {
    Mine,
    Trap,
}

/// A device on its fuse: it has left the field and its bang lands in `left` seconds.
#[derive(Clone, Copy, Debug)]
struct Pending {
    x: f32,
    z: f32,
    seed: f32,
    depth: u32,
    left: f32,
}

/// A blast's site: the centre and the seed its variation is drawn from.
#[derive(Clone, Copy, Debug)]
struct At {
    x: f32,
    z: f32,
    seed: f32,
}

/// A device's cell and the world point it sits at.
#[derive(Clone, Copy, Debug)]
struct Loc {
    cx: i32,
    cz: i32,
    x: f32,
    z: f32,
}

/// A bang that landed this frame, handed to the presentation layer (M5): where it
/// was, how hard it hit the goat, and which way it throws — the flash's site and
/// the camera's knock read it. This is the only thing a bang exposes; the look
/// itself lives in [`crate::apply_frame`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bang {
    /// World X of the blast centre.
    pub x: f32,
    /// World Z of the blast centre.
    pub z: f32,
    /// The blast's own variation stream, so its look is the same for everyone who
    /// sees it (the original's `seed`).
    pub seed: f32,
    /// The blast's reach at the goat: `1.0` at its feet, falling to `0.0` at the
    /// rim and `0.0` out of reach — the same falloff the damage and the fling use.
    pub impact: f32,
    /// The unit ground direction the blast throws the goat in (away from the
    /// centre, turned by the blast's stream); zero when out of reach.
    pub shove: (f32, f32),
}

/// The hazard field: the devices that have gone off and the ones that moved, plus
/// the fuses counting down.
#[derive(Debug, Default)]
pub struct Hazards {
    /// Mine cells that have gone off: empty ground now, for the session.
    spent: HashSet<i64>,
    /// Mine cells a replacement was placed in.
    moved: HashSet<i64>,
    /// The same two, for trapped tufts.
    trap_spent: HashSet<i64>,
    trap_moved: HashSet<i64>,
    /// Devices on their fuse.
    pending: Vec<Pending>,
}

impl Hazards {
    /// Advance the hazard field one frame: heal the ground, land the bangs whose
    /// fuses have run out, then check what the goat has walked onto
    /// (`updateExplosions`). Returns the bangs that landed this frame, for the
    /// presentation layer (M5).
    pub fn update(
        &mut self,
        dt: f32,
        goat: &mut Goat,
        meadow: &grass::Meadow,
        craters: &mut Craters,
    ) -> Vec<Bang> {
        // The ground heals whether or not the hazards are armed: a crater is not an
        // effect.
        craters.update(dt);
        // Fuses first, so a bang that lands this frame can still chain.
        let mut bangs = Vec::new();
        let mut i = self.pending.len();
        while i > 0 {
            i -= 1;
            self.pending[i].left -= dt;
            if self.pending[i].left > 0.0 {
                continue;
            }
            let p = self.pending.remove(i);
            bangs.push(self.blast(
                At {
                    x: p.x,
                    z: p.z,
                    seed: p.seed,
                },
                p.depth,
                goat,
                meadow,
                craters,
            ));
        }
        // A sleeping goat is not moving, and a dead one is not a goat the world has
        // to be fair about: neither trips anything.
        if matches!(goat.mode, Mode::Sleep | Mode::Dead) {
            return bangs;
        }
        let airborne = matches!(goat.mode, Mode::Jump | Mode::Flung);
        self.check_triggers(goat.x, goat.z, airborne, meadow);
        bangs
    }

    /// A bang at `(x, z)`: leave a crater, damage and fling the goat, and set off
    /// the neighbours if there is chain budget left (`blast`). Returns the bang for
    /// the presentation layer.
    fn blast(
        &mut self,
        at: At,
        depth: u32,
        goat: &mut Goat,
        meadow: &grass::Meadow,
        craters: &mut Craters,
    ) -> Bang {
        let (x, z, seed) = (at.x, at.z, at.seed);
        // The hole is everyone's, and it is dug whatever else the bang does.
        craters.add(x, z, seed);
        let mut bang = Bang {
            x,
            z,
            seed,
            impact: 0.0,
            shove: (0.0, 0.0),
        };
        let r = BLAST_RADIUS;
        let dx = goat.x - x;
        let dz = goat.z - z;
        let d2 = dx * dx + dz * dz;
        if d2 <= r * r {
            let d = d2.sqrt();
            let falloff = 1.0 - d / r;
            let took = BLAST_DAMAGE * falloff * falloff;
            goat.health = (goat.health - took).max(BLAST_HEALTH_FLOOR);
            // Away from the blast, up, scaled by the same falloff as the damage, so
            // the rim is a shove and the centre is a launch.
            let (ux, uz) = fling_direction(dx, dz, d, seed);
            bang.impact = falloff;
            bang.shove = (ux, uz);
            let base = craters.surface(goat.x, goat.z) + goat.lift;
            goat.start_fling(
                ux * BLAST_PUSH * falloff,
                uz * BLAST_PUSH * falloff,
                BLAST_LIFT * falloff,
                base,
            );
        }
        if depth < CHAIN_DEPTH {
            self.chain_from(x, z, depth + 1, meadow);
        }
        bang
    }

    /// Fire a bang at `(x, z)` as a fuse would, with a fresh chain budget -- the
    /// harness's seed seam (the original's `goats.explosions.blast`).
    #[cfg(test)]
    pub(crate) fn detonate(
        &mut self,
        x: f32,
        z: f32,
        seed: f32,
        goat: &mut Goat,
        meadow: &grass::Meadow,
        craters: &mut Craters,
    ) {
        let _ = self.blast(At { x, z, seed }, 0, goat, meadow, craters);
    }

    /// A blast sets off the armed mines it reaches, `FUSE` seconds later
    /// (`chainFrom`).
    fn chain_from(&mut self, x: f32, z: f32, depth: u32, meadow: &grass::Meadow) {
        let r = BLAST_RADIUS;
        let cx0 = ((x - r) / 2.0).floor() as i32;
        let cx1 = ((x + r) / 2.0).floor() as i32;
        let cz0 = ((z - r) / 2.0).floor() as i32;
        let cz1 = ((z + r) / 2.0).floor() as i32;
        for cx in cx0..=cx1 {
            for cz in cz0..=cz1 {
                if !self.mine_armed(cx, cz) {
                    continue;
                }
                let mx = (cx * 2 + 1) as f32;
                let mz = (cz * 2 + 1) as f32;
                let dx = mx - x;
                let dz = mz - z;
                if dx * dx + dz * dz > r * r {
                    continue;
                }
                self.trip_device(
                    Device::Mine,
                    Loc {
                        cx,
                        cz,
                        x: mx,
                        z: mz,
                    },
                    depth,
                    meadow,
                );
            }
        }
    }

    /// What the goat has walked onto (`checkTriggers`): the mine in its own cell,
    /// then the nearest trapped tuft in reach.
    fn check_triggers(&mut self, x: f32, z: f32, airborne: bool, meadow: &grass::Meadow) {
        if airborne {
            return;
        }
        let cx = (x / 2.0).floor() as i32;
        let cz = (z / 2.0).floor() as i32;
        if let Some((mx, mz)) = self.mine_in_cell(x, z, cx, cz) {
            self.trip_device(
                Device::Mine,
                Loc {
                    cx,
                    cz,
                    x: mx,
                    z: mz,
                },
                0,
                meadow,
            );
        }
        if let Some((tcx, tcz, tx, tz)) = self.nearest_trap(x, z, meadow) {
            self.trip_device(
                Device::Trap,
                Loc {
                    cx: tcx,
                    cz: tcz,
                    x: tx,
                    z: tz,
                },
                0,
                meadow,
            );
        }
    }

    /// The nearest armed mine within `MINE_TRIGGER` of `(x, z)` in cell `(cx, cz)`,
    /// or `None` (`mineInCell`).
    fn mine_in_cell(&self, x: f32, z: f32, cx: i32, cz: i32) -> Option<(f32, f32)> {
        if !self.mine_armed(cx, cz) {
            return None;
        }
        let mx = (cx * 2 + 1) as f32;
        let mz = (cz * 2 + 1) as f32;
        let dx = mx - x;
        let dz = mz - z;
        if dx * dx + dz * dz > MINE_TRIGGER * MINE_TRIGGER {
            return None;
        }
        Some((mx, mz))
    }

    /// The nearest live trapped tuft within `MINE_TRIGGER`, or `None` — the trap's
    /// trigger is the tuft's own position, not a cell's centre.
    fn nearest_trap(&self, x: f32, z: f32, meadow: &grass::Meadow) -> Option<(i32, i32, f32, f32)> {
        let r = (MINE_TRIGGER / grass::CELL).ceil() as i32;
        let cx = (x / grass::CELL).floor() as i32;
        let cz = (z / grass::CELL).floor() as i32;
        let mut best: Option<(i32, i32, f32, f32)> = None;
        let mut best_d2 = MINE_TRIGGER * MINE_TRIGGER;
        for i in cx - r..=cx + r {
            for j in cz - r..=cz + r {
                if meadow.is_eaten(i, j) || !self.trap_at(i, j) {
                    continue;
                }
                if let Some(tuft) = grass::tuft_at(i, j) {
                    let d2 = tuft.distance2(x, z);
                    if d2 <= best_d2 {
                        best_d2 = d2;
                        best = Some((tuft.cx, tuft.cz, tuft.x, tuft.z));
                    }
                }
            }
        }
        best
    }

    /// Take a device out of the field and put it on a fuse (`tripDevice`).
    fn trip_device(&mut self, kind: Device, loc: Loc, depth: u32, meadow: &grass::Meadow) -> bool {
        if !self.spend_device(kind, loc.cx, loc.cz, meadow) {
            return false;
        }
        let key = tuft_key(loc.cx, loc.cz);
        self.pending.push(Pending {
            x: loc.x,
            z: loc.z,
            seed: hash((key as f64 * 0.0001) as f32),
            depth,
            left: FUSE,
        });
        true
    }

    /// Remove a device: it is spent for the session, and a replacement is placed
    /// elsewhere (`spendDevice`). `false` when there is nothing armed at the cell.
    fn spend_device(&mut self, kind: Device, cx: i32, cz: i32, meadow: &grass::Meadow) -> bool {
        let key = tuft_key(cx, cz);
        if kind == Device::Mine {
            if !self.mine_armed(cx, cz) {
                return false;
            }
            // A replacement that goes off is simply removed; a derived mine leaves
            // its cell spent, which keeps that cell empty for the session.
            if !self.moved.remove(&key) {
                self.spent.insert(key);
            }
        } else {
            if !self.trap_at(cx, cz) {
                return false;
            }
            if !self.trap_moved.remove(&key) {
                self.trap_spent.insert(key);
            }
        }
        self.relocate_device(kind, cx, cz, meadow);
        true
    }

    /// Where a device goes when it has gone off (`relocateDevice`): a ring
    /// `RELOCATE_MIN..RELOCATE_MAX` metres from the cell it left, derived from the
    /// cell's own key, so every peer computes the same move.
    fn relocate_device(&mut self, kind: Device, fcx: i32, fcz: i32, meadow: &grass::Meadow) {
        let lo = RELOCATE_MIN.max(BLAST_RADIUS);
        let fx = (fcx * 2 + 1) as f32;
        let fz = (fcz * 2 + 1) as f32;
        for tries in 0..RELOCATE_TRIES {
            let d = lo + (RELOCATE_MAX - lo) * trap_noise(fcx, fcz, SALT + tries);
            let a = TAU * trap_noise(fcz ^ tries, fcx + tries, SALT ^ 0x27d4_eb2f);
            let cx = ((fx + a.cos() * d) / 2.0).floor() as i32;
            let cz = ((fz + a.sin() * d) / 2.0).floor() as i32;
            let x = (cx * 2 + 1) as f32;
            let z = (cz * 2 + 1) as f32;
            // The spawn's disc, measured to the cell's nearest edge as `mine_at` does.
            let sx = (x.abs() - 1.0).max(0.0);
            let sz = (z.abs() - 1.0).max(0.0);
            if sx * sx + sz * sz < SAFE * SAFE {
                continue;
            }
            let key = tuft_key(cx, cz);
            if kind == Device::Mine {
                // A cell that already holds one *and* one whose mine has gone off
                // are both refused, so the spent set is its own test here.
                if self.spent.contains(&key) || self.mine_armed(cx, cz) {
                    continue;
                }
                self.moved.insert(key);
                return;
            }
            if self.trap_spent.contains(&key) || self.trap_at(cx, cz) {
                continue;
            }
            // A trap has to land on a tuft the meadow grows one in — and a live one,
            // or it is a device nobody can see or trip.
            if grass::tuft_at(cx, cz).is_none() || meadow.is_eaten(cx, cz) {
                continue;
            }
            self.trap_moved.insert(key);
            return;
        }
    }

    /// Whether cell `(cx, cz)` holds an armed mine now: the derived field, minus the
    /// mines that have gone off, plus the replacements (`mineArmed`). Crate-visible
    /// so the harness can stand the goat on one and prove the spawn disc is clear.
    pub(crate) fn mine_armed(&self, cx: i32, cz: i32) -> bool {
        let key = tuft_key(cx, cz);
        if self.spent.contains(&key) {
            return false;
        }
        self.moved.contains(&key) || mine_at(cx, cz)
    }

    /// Whether the tuft in cell `(cx, cz)` is trapped now (`trapAt`).
    fn trap_at(&self, cx: i32, cz: i32) -> bool {
        let key = tuft_key(cx, cz);
        if self.trap_spent.contains(&key) {
            return false;
        }
        self.trap_moved.contains(&key)
            || trap_noise(cx ^ 0x5bf0_3635, cz ^ 0x27d4_eb2f, SALT) < TRAP_CHANCE
    }
}

/// Whether cell `(cx, cz)` holds a mine (`mineAt`): `safe` clears a disc around the
/// spawn, and nothing else is excluded.
fn mine_at(cx: i32, cz: i32) -> bool {
    let x = cx * 2 + 1;
    let z = cz * 2 + 1;
    // The distance is to the cell's nearest edge, not its centre, so the clear disc
    // is a guarantee about the ground.
    let dx = (x.abs() - 1).max(0);
    let dz = (z.abs() - 1).max(0);
    if ((dx * dx + dz * dz) as f32) < SAFE * SAFE {
        return false;
    }
    trap_noise(cx, cz, SALT) < MINE_DENSITY
}

/// A cell's device value in `0..1`: the integer mixing every peer reproduces
/// (`trapNoise`), with the session salt.
fn trap_noise(cx: i32, cz: i32, salt: i32) -> f32 {
    let mut h = cx
        .wrapping_mul(374_761_393)
        .wrapping_add(cz.wrapping_mul(668_265_263))
        .wrapping_add(salt);
    h = (h ^ ((h as u32 >> 13) as i32)).wrapping_mul(1_274_126_177);
    (((h ^ ((h as u32 >> 16) as i32)) as u32) as f32) / 4_294_967_296.0
}

/// The direction a blast throws a goat in: away from the centre, turned a little
/// off that axis by the blast's own stream (`flingDirection`).
fn fling_direction(ox: f32, oz: f32, d: f32, seed: f32) -> (f32, f32) {
    let (ux, uz) = if d > 1e-3 {
        (ox / d, oz / d)
    } else {
        let a = hash(seed * 9.7) * TAU;
        (a.cos(), a.sin())
    };
    let turn = (hash(seed * 3.7 + 1.3) - 0.5) * 0.6; // ±0.3 rad off-axis
    (ux - uz * turn, uz + ux * turn)
}

/// A cell's identity, packed exactly as the original's `tuftKey`.
fn tuft_key(cx: i32, cz: i32) -> i64 {
    (i64::from(cx) + 4096) * 8192 + (i64::from(cz) + 4096)
}

#[cfg(test)]
mod tests {
    use super::*;
    use noctis::prelude::Entity;

    /// A goat at the origin with no model, for the blast tests.
    fn test_goat() -> Goat {
        Goat {
            root: Entity::from_raw(0, 0),
            skinned: Entity::from_raw(1, 0),
            lift: 0.0,
            x: 0.0,
            z: 0.0,
            yaw: 0.0,
            clips: crate::Clips::default(),
            variant: crate::Variants::default(),
            mode: Mode::Idle,
            jump_time: 0.0,
            jump_speed: 0.0,
            jump_dir: 0.0,
            jump_duration: 0.0,
            health: 100.0,
            energy: 100.0,
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
    fn the_spawn_disc_is_clear_and_the_field_is_sparse() {
        // Nothing within `SAFE` of the spawn is a mine, or `restart` would be a lie.
        for cx in -5i32..=5 {
            for cz in -5i32..=5 {
                let x = cx * 2 + 1;
                let z = cz * 2 + 1;
                let dx = (x.abs() - 1).max(0);
                let dz = (z.abs() - 1).max(0);
                if ((dx * dx + dz * dz) as f32) < SAFE * SAFE {
                    assert!(!mine_at(cx, cz), "clear at ({cx}, {cz})");
                }
            }
        }
        // Over a wide field the density is about `MINE_DENSITY`, and the traps about
        // `TRAP_CHANCE`.
        let n = 200;
        let mut mines = 0;
        let mut traps = 0;
        let total = (n * n) as f32;
        for cx in 0..n {
            for cz in 0..n {
                if mine_at(cx, cz) {
                    mines += 1;
                }
                if trap_noise(cx ^ 0x5bf0_3635, cz ^ 0x27d4_eb2f, SALT) < TRAP_CHANCE {
                    traps += 1;
                }
            }
        }
        let mine_share = mines as f32 / total;
        let trap_share = traps as f32 / total;
        assert!((0.006..0.02).contains(&mine_share), "mines: {mine_share}");
        assert!((0.02..0.07).contains(&trap_share), "traps: {trap_share}");
    }

    #[test]
    fn the_hash_is_deterministic_and_salted() {
        assert_eq!(trap_noise(3, -7, SALT), trap_noise(3, -7, SALT));
        assert_ne!(trap_noise(3, -7, SALT), trap_noise(3, -7, SALT ^ 1));
        for cx in -50..50 {
            for cz in -50..50 {
                let v = trap_noise(cx, cz, SALT);
                assert!((0.0..1.0).contains(&v));
            }
        }
    }

    #[test]
    fn a_mine_leaves_its_cell_spent_for_the_session() {
        // Find an armed mine out in the field.
        let meadow = grass::Meadow::default();
        let (mut cx, mut cz) = (0, 0);
        'find: for x in 0..400 {
            for z in 0..400 {
                if mine_at(x, z) {
                    cx = x;
                    cz = z;
                    break 'find;
                }
            }
        }
        let mut hazards = Hazards::default();
        assert!(hazards.mine_armed(cx, cz), "the cell holds a mine");
        assert!(hazards.spend_device(Device::Mine, cx, cz, &meadow));
        assert!(!hazards.mine_armed(cx, cz), "it is gone");
        assert!(
            !hazards.spend_device(Device::Mine, cx, cz, &meadow),
            "twice"
        );
        assert!(
            !hazards.mine_armed(cx, cz),
            "the cell stays empty even after the replacement goes off"
        );
    }

    #[test]
    fn a_blast_dishes_the_ground_and_flings_the_goat() {
        let meadow = grass::Meadow::default();
        let mut craters = Craters::default();
        let mut hazards = Hazards::default();
        let mut goat = test_goat();

        hazards.blast(
            At {
                x: goat.x,
                z: goat.z,
                seed: 1.5,
            },
            0,
            &mut goat,
            &meadow,
            &mut craters,
        );

        assert!(craters.revision() > 0, "the bang left a hole");
        assert!(craters.dip_at(0.0, 0.0) < 0.0);
        assert!(goat.health < 100.0, "the goat took damage");
        assert!(
            goat.health >= BLAST_HEALTH_FLOOR,
            "but never below the floor"
        );
        assert!(goat.health > 0.0, "a mine cannot end the run");
        assert_eq!(goat.mode, Mode::Flung, "and it was flung");
        assert!(goat.fling_vy > 0.0, "upward");
    }

    #[test]
    fn a_blast_out_of_reach_is_only_a_hole() {
        let meadow = grass::Meadow::default();
        let mut craters = Craters::default();
        let mut hazards = Hazards::default();
        let mut goat = test_goat();
        let health = goat.health;

        hazards.blast(
            At {
                x: goat.x + BLAST_RADIUS * 2.0,
                z: goat.z,
                seed: 1.5,
            },
            0,
            &mut goat,
            &meadow,
            &mut craters,
        );

        assert!(craters.revision() > 0, "still a hole");
        assert_eq!(goat.health, health, "but no damage");
        assert_eq!(goat.mode, Mode::Idle, "and no fling");
    }

    #[test]
    fn a_bang_reports_its_impact_and_shove() {
        let meadow = grass::Meadow::default();
        let mut craters = Craters::default();
        let mut hazards = Hazards::default();
        let mut goat = test_goat();

        // At the goat's feet: a full hit.
        let at_feet = hazards.blast(
            At {
                x: goat.x,
                z: goat.z,
                seed: 1.5,
            },
            0,
            &mut goat,
            &meadow,
            &mut craters,
        );
        assert!(
            (at_feet.impact - 1.0).abs() < 1e-5,
            "the centre is a full hit"
        );

        // Off to the side: a partial hit, and the shove points away from the blast.
        let side = hazards.blast(
            At {
                x: goat.x + BLAST_RADIUS * 0.5,
                z: goat.z,
                seed: 2.5,
            },
            0,
            &mut goat,
            &meadow,
            &mut craters,
        );
        assert!(
            side.impact > 0.0 && side.impact < 1.0,
            "the rim is a partial hit"
        );
        assert!(side.shove.0 < -0.5, "thrown away from the blast (+X)");
        assert!(side.shove.1.abs() < 0.5, "and mostly along that axis");

        // Out of reach: no impact and no shove.
        let far = hazards.blast(
            At {
                x: goat.x + BLAST_RADIUS * 2.0,
                z: goat.z,
                seed: 3.5,
            },
            0,
            &mut goat,
            &meadow,
            &mut craters,
        );
        assert_eq!(far.impact, 0.0, "out of reach is no hit");
        assert_eq!(far.shove, (0.0, 0.0), "and no shove");
    }

    #[test]
    fn a_tripped_device_lands_after_its_fuse() {
        let meadow = grass::Meadow::default();
        let mut craters = Craters::default();
        let mut hazards = Hazards::default();
        // Asleep, so the trigger pass does not fire on the grass around the origin
        // and the test is about the fuse alone.
        let mut goat = test_goat();
        goat.mode = Mode::Sleep;

        // Cell (0,0) is inside the spawn's safe disc, so there is no mine to trip.
        assert!(!hazards.trip_device(
            Device::Mine,
            Loc {
                cx: 0,
                cz: 0,
                x: goat.x,
                z: goat.z,
            },
            0,
            &meadow,
        ));
        assert_eq!(hazards.pending.len(), 0);

        // A direct fuse: push a pending bang and step it.
        hazards.pending.push(Pending {
            x: goat.x,
            z: goat.z,
            seed: 1.0,
            depth: 0,
            left: FUSE,
        });
        hazards.update(0.0, &mut goat, &meadow, &mut craters);
        assert_eq!(hazards.pending.len(), 1, "the fuse has not run out");
        assert_eq!(craters.revision(), 0, "no bang yet");
        hazards.update(FUSE, &mut goat, &meadow, &mut craters);
        assert_eq!(hazards.pending.len(), 0, "the fuse ran out");
        assert!(craters.revision() > 0, "the bang landed");
        assert_eq!(goat.mode, Mode::Flung);
    }

    #[test]
    fn the_fling_direction_is_a_unit_vector_away_from_the_blast() {
        let (ux, uz) = fling_direction(3.0, 4.0, 5.0, 0.7);
        let len = (ux * ux + uz * uz).sqrt();
        assert!((len - 1.0).abs() < 0.2, "roughly unit: {len}");
        assert!(ux > 0.0 && uz > 0.0, "away from the centre");
        // Dead on the centre, the stream still picks a direction.
        let (dx, dz) = fling_direction(0.0, 0.0, 0.0, 0.7);
        assert!((dx * dx + dz * dz - 1.0).abs() < 0.25);
    }
}
