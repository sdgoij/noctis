//! The water table, ported from the original's `water.js`.
//!
//! The table's level is a *follower* of the weather's rain and nothing else: it rises
//! the instant the rain does and lets go slowly, so a shower leaves standing water
//! behind it. What the game reads from it is the goat's own cost — the depth it is
//! wading through, which slows it and burns energy — whether it may sleep, and whether
//! a tuft is standing under the surface.
//!
//! The surface mesh, the chop, the wake, the splashes and the reflections are the
//! presentation layer (`.notes/goats.md` M5); only the level and what it decides live
//! here. The whole of the level is one scalar, so two runs handed the same rain agree.

use crate::terrain;

/// The grid's vertices per side — the terrain's own: the table is a function of the
/// heights, so it is read on the same grid the ground is drawn from.
const N: usize = terrain::QUADS + 1;
/// World units per cell (the terrain's own).
const CELL: f32 = terrain::CELL;
/// Half the field's width, in units.
const HALF: f32 = terrain::QUADS as f32 * terrain::CELL * 0.5;

/// The table's floor, in metres (`TUNING.water.low`): the terrain's own relief below
/// zero, so a dry spell is exactly dry at every point of the field.
const LOW: f32 = -2.1;
/// The table's ceiling (`TUNING.water.high`).
const HIGH: f32 = 0.0;
/// The share of the band the heaviest rain reaches (`TUNING.water.fill`).
const FILL: f32 = 0.75;
/// Rain below this leaves the ground dry (`TUNING.water.seep`).
const SEEP: f32 = 0.05;
/// Seconds the table takes to drain once the rain that filled it has gone
/// (`TUNING.water.wetDown`).
const WET_DOWN: f32 = 600.0;
/// The wading cap, in metres (`TUNING.water.maxDepth`).
const MAX_DEPTH: f32 = 0.6;
/// The share of its speed the goat loses at the wading cap (`TUNING.water.drag`).
const DRAG: f32 = 0.35;
/// The extra energy it burns wading (`TUNING.water.dragEnergy`).
const DRAG_ENERGY: f32 = 0.25;

/// The wetting curve's exponent (`WATER_WET_CURVE`): *concave* in the rain, so the
/// first of a shower is most of what pools and a moderate shower leaves a puddle
/// rather than a film.
const WET_CURVE: f32 = 0.5;
/// The wetting below which a draining table is called dry (`WATER_DRY_EPS`), so a dry
/// spell is pinned to its floor rather than merely approaching it.
const DRY_EPS: f32 = 1e-4;
/// The least depth the HUD and the rules call "in the water" (`WATER_HUD_MIN`): the
/// drain leaves a film that thin behind for minutes, and a pool nobody can see should
/// not have a read-out.
const HUD_MIN: f32 = 0.01;

/// The water table: its grid of ground heights, the table's own level, and the state
/// the level is a function of.
#[derive(Debug)]
pub struct Water {
    /// The ground, `[j * N + i]`, sampled once (the terrain is fixed, so it cannot
    /// change under the table).
    heights: Vec<f32>,
    /// The field's lowest ground, the "is anything wet" test (`waterUpdate`). Nothing
    /// in the field is lower than it, so the table clears it exactly when a pool does.
    ground: f32,
    /// The water's own wetting, `0..=1`: the *only* memory the table has, so two runs
    /// handed the same rain agree to the last bit.
    wet: f32,
    /// The table's level, in metres.
    level: f32,
    /// Whether the table stands above the ground: there is water to draw.
    on: bool,
    /// Whether the system is on at all (the frame-cost bisect).
    enabled: bool,
}

impl Water {
    /// Sample the ground and start the table on its floor (dry).
    pub fn new(craters: &terrain::Craters) -> Self {
        let mut water = Self {
            heights: Vec::new(),
            ground: f32::INFINITY,
            wet: 0.0,
            level: LOW,
            on: false,
            enabled: true,
        };
        water.resample(craters);
        water
    }

    /// Re-sample the ground under the table — after a blast dished a crater, or one
    /// healed — and re-measure the field's floor (`waterRebuild`). The table's own
    /// state (its wetting) is kept, so a rise carries across the rebuild.
    pub fn resample(&mut self, craters: &terrain::Craters) {
        self.heights.clear();
        self.heights.reserve(N * N);
        for j in 0..N {
            for i in 0..N {
                self.heights.push(craters.surface(vertex_x(i), vertex_z(j)));
            }
        }
        self.ground = self.heights.iter().copied().fold(f32::INFINITY, f32::min);
        self.refresh();
    }

    /// Advance the table one frame for the weather's `rain` (`waterStep` then
    /// `waterUpdate`): the rise is immediate, the drain is slow.
    pub fn update(&mut self, dt: f32, rain: f32) {
        let target = wetness_for(rain);
        if target < self.wet {
            let k = if WET_DOWN > 1e-6 {
                (dt / WET_DOWN).min(1.0)
            } else {
                1.0
            };
            self.wet += (target - self.wet) * k;
            if self.wet < DRY_EPS {
                self.wet = 0.0;
            }
        }
        if target > self.wet {
            self.wet = target;
        }
        self.refresh();
    }

    /// Turn the whole system on or off (`waterSetEnabled`, the `J` key), returning
    /// its new state.
    pub fn set_enabled(&mut self, on: bool) -> bool {
        self.enabled = on;
        self.refresh();
        self.enabled
    }

    /// Whether the system is on.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The table's level, in metres.
    pub fn level(&self) -> f32 {
        self.level
    }

    /// The water depth at a world point, bilinear over the grid so a wading goat reads
    /// a smooth number rather than whichever vertex it stands nearest. Off the grid
    /// there is no water, and a disabled system is dry everywhere.
    pub fn depth_at(&self, x: f32, z: f32) -> f32 {
        if !self.enabled {
            return 0.0;
        }
        let fx = (x + HALF) / CELL;
        let fz = (z + HALF) / CELL;
        let i = fx.floor();
        let j = fz.floor();
        if i < 0.0 || j < 0.0 || i >= (N - 1) as f32 || j >= (N - 1) as f32 {
            return 0.0;
        }
        let (i, j) = (i as usize, j as usize);
        let u = fx - i as f32;
        let v = fz - j as f32;
        let d00 = self.vertex_depth(i, j);
        let d10 = self.vertex_depth(i + 1, j);
        let d01 = self.vertex_depth(i, j + 1);
        let d11 = self.vertex_depth(i + 1, j + 1);
        let top = d00 + (d10 - d00) * u;
        let bot = d01 + (d11 - d01) * u;
        top + (bot - top) * v
    }

    /// Whether a tuft standing at `(x, z)` is under the water, for the grass field to
    /// skip (`waterSubmergedAt`): one comparison of the nearest vertex's ground
    /// against the table.
    pub fn submerged_at(&self, x: f32, z: f32) -> bool {
        if !self.on {
            return false;
        }
        let i = ((x + HALF) / CELL).round();
        let j = ((z + HALF) / CELL).round();
        if i < 0.0 || j < 0.0 || i >= N as f32 || j >= N as f32 {
            return false;
        }
        self.heights[j as usize * N + i as usize] < self.level
    }

    /// Whether a point is *in* the water rather than on a damp film (`waterInWater`) —
    /// the HUD's own line, so the read-out and the sleep rule cannot disagree.
    pub fn in_water(&self, x: f32, z: f32) -> bool {
        self.depth_at(x, z) >= HUD_MIN
    }

    /// The goat's ground-speed multiplier from wading (`waterSpeedFactor`). Exactly
    /// `1` out of the water, so a dry gait speed is untouched.
    pub fn speed_factor(&self, x: f32, z: f32) -> f32 {
        1.0 - DRAG * self.drag_fraction(x, z)
    }

    /// The goat's energy-drain multiplier from wading (`waterDrainFactor`).
    pub fn drain_factor(&self, x: f32, z: f32) -> f32 {
        1.0 + DRAG_ENERGY * self.drag_fraction(x, z)
    }

    /// The HUD's water read-out (`waterHudText`), empty when the goat is dry so a
    /// clear frame's weather line is exactly the line it has always been.
    pub fn hud(&self, x: f32, z: f32) -> String {
        let depth = self.depth_at(x, z);
        if depth < HUD_MIN {
            return String::new();
        }
        format!(
            "water {depth:.2} m   slowed {}%",
            (DRAG * self.drag_fraction(x, z) * 100.0).round() as i32
        )
    }

    /// How much of the water's effect applies where the goat is standing: `0` dry, `1`
    /// at the wading cap (`waterDragFraction`).
    fn drag_fraction(&self, x: f32, z: f32) -> f32 {
        let depth = self.depth_at(x, z);
        if depth <= 0.0 {
            return 0.0;
        }
        if MAX_DEPTH <= 0.0 {
            return 1.0;
        }
        (depth / MAX_DEPTH).min(1.0)
    }

    /// The water's own depth at vertex `(i, j)`: `max(0, level - ground)`.
    fn vertex_depth(&self, i: usize, j: usize) -> f32 {
        (self.level - self.heights[j * N + i]).max(0.0)
    }

    /// Recompute the level and the "is there water" flag from the current wetting and
    /// the enabled leaf (`waterUpdate`).
    fn refresh(&mut self) {
        self.level = if !self.enabled || self.wet <= 0.0 {
            LOW
        } else {
            LOW + self.wet * FILL * (HIGH - LOW)
        };
        self.on = self.level > self.ground + 1e-6;
    }
}

/// The rain, as the share of the table's own band it can reach (`wetnessFor`): nothing
/// below `seep`, and above it a *concave* curve, so the first of a shower is most of
/// what pools.
fn wetness_for(rain: f32) -> f32 {
    let raw = (rain - SEEP) / (1.0 - SEEP);
    if raw <= 0.0 {
        return 0.0;
    }
    raw.min(1.0).powf(WET_CURVE)
}

/// The world x of grid vertex `i`.
fn vertex_x(i: usize) -> f32 {
    -HALF + i as f32 * CELL
}

/// The world z of grid vertex `j`.
fn vertex_z(j: usize) -> f32 {
    -HALF + j as f32 * CELL
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A water table over undished ground.
    fn flat() -> Water {
        Water::new(&terrain::Craters::default())
    }

    /// The grid's lowest and highest ground over the cells `depth_at` can read (its
    /// lower-left corners, `0..N-1`), and the vertices there.
    fn extremal(water: &Water) -> ((f32, f32), (usize, usize), (usize, usize)) {
        let mut low = (f32::INFINITY, 0, 0);
        let mut high = (f32::NEG_INFINITY, 0, 0);
        for j in 0..N - 1 {
            for i in 0..N - 1 {
                let h = water.heights[j * N + i];
                if h < low.0 {
                    low = (h, i, j);
                }
                if h > high.0 {
                    high = (h, i, j);
                }
            }
        }
        ((low.0, high.0), (low.1, low.2), (high.1, high.2))
    }

    #[test]
    fn a_dry_spell_is_exactly_dry() {
        let water = flat();
        assert_eq!(water.wet, 0.0);
        assert_eq!(water.level(), LOW, "a dry table sits on its floor");
        assert!(!water.on, "nothing is wet");
        // Everywhere on the field is dry.
        for j in [0, N / 2, N - 1] {
            for i in [0, N / 2, N - 1] {
                assert_eq!(water.depth_at(vertex_x(i), vertex_z(j)), 0.0);
            }
        }
        assert_eq!(water.speed_factor(0.0, 0.0), 1.0, "dry is neutral");
        assert_eq!(water.drain_factor(0.0, 0.0), 1.0);
        assert!(!water.in_water(0.0, 0.0));
        assert_eq!(water.hud(0.0, 0.0), "");
    }

    #[test]
    fn the_wetting_curve_is_concave_and_starts_at_the_seep() {
        assert_eq!(wetness_for(SEEP), 0.0, "the seep is exactly dry");
        assert_eq!(wetness_for(0.0), 0.0);
        // A concave curve: half the rain above the seep pools more than half the band.
        let raw = (0.5 - SEEP) / (1.0 - SEEP);
        assert!((wetness_for(0.5) - raw.powf(0.5)).abs() < 1e-6);
        assert!(wetness_for(0.5) > raw, "the first of a shower pools most");
        assert!(
            (wetness_for(1.0) - 1.0).abs() < 1e-6,
            "a downpour fills the band"
        );
    }

    #[test]
    fn heavy_rain_fills_the_table_and_it_drains_slowly() {
        let mut water = flat();
        // A downpour: the rise is immediate.
        water.update(1.0 / 60.0, 1.0);
        assert_eq!(water.wet, 1.0, "the table rises the instant the rain does");
        assert!((water.level() - (LOW + FILL * (HIGH - LOW))).abs() < 1e-6);
        assert!(water.on, "the field holds water");

        // The rain stops. A dry spell barely drains it inside a frame...
        water.update(1.0 / 60.0, 0.0);
        assert!(water.wet > 0.99, "the table lets go slowly: {}", water.wet);

        // ...and a short while later it still stands.
        water.update(60.0, 0.0);
        assert!(
            water.wet > 0.8,
            "a pool stands after the rain: {}",
            water.wet
        );

        // Given long enough it drains back to dry, exactly.
        water.update(WET_DOWN * 4.0, 0.0);
        assert_eq!(water.wet, 0.0, "the drain reaches a real zero");
        assert_eq!(water.level(), LOW);
        assert!(!water.on);
    }

    #[test]
    fn wading_slows_the_goat_and_burns_energy() {
        let mut water = flat();
        let ((_, _), (li, lj), (hi, hj)) = extremal(&water);
        // A downpour fills the table to the top of its band, so the deepest point is
        // under water and the highest stands proud of it.
        water.update(1.0 / 60.0, 1.0);
        let (lx, lz) = (vertex_x(li), vertex_z(lj));
        let (hx, hz) = (vertex_x(hi), vertex_z(hj));

        let depth = water.depth_at(lx, lz);
        assert!(depth > 0.0, "the hollow holds water: {depth}");
        assert!(water.in_water(lx, lz));
        assert!(water.speed_factor(lx, lz) < 1.0, "wading slows the goat");
        assert!(water.drain_factor(lx, lz) > 1.0, "wading burns energy");
        // The cap: a deep pool is not slower than the wading cap.
        assert!(depth >= MAX_DEPTH, "deep enough to hit the cap: {depth}");
        assert!((water.speed_factor(lx, lz) - (1.0 - DRAG)).abs() < 1e-6);

        // The high ground stands above the table and is untouched.
        assert_eq!(water.depth_at(hx, hz), 0.0);
        assert!(!water.in_water(hx, hz), "the hill is dry");
        assert_eq!(water.speed_factor(hx, hz), 1.0);
        assert_eq!(water.drain_factor(hx, hz), 1.0);
    }

    #[test]
    fn the_off_switch_dries_the_whole_field() {
        let mut water = flat();
        water.update(1.0 / 60.0, 1.0);
        assert!(water.on, "the field is wet after a downpour");

        // Off is the whole system: the field is dry everywhere...
        assert!(!water.set_enabled(false), "the system reports off");
        assert_eq!(water.level(), LOW, "off pins the table to its floor");
        assert!(!water.on);
        assert_eq!(water.depth_at(0.0, 0.0), 0.0, "a disabled table is dry");
        assert!(!water.in_water(0.0, 0.0));

        // ...and the wetting survived the toggle, so the pool is back at once.
        assert!(water.set_enabled(true), "and back on");
        assert!(water.on, "the pool returns with the system");
    }

    #[test]
    fn tufts_under_the_water_are_submerged() {
        let mut water = flat();
        let ((_, _), (li, lj), (hi, hj)) = extremal(&water);
        let (lx, lz) = (vertex_x(li), vertex_z(lj));
        let (hx, hz) = (vertex_x(hi), vertex_z(hj));

        assert!(!water.submerged_at(lx, lz), "a dry field culls nothing");
        water.update(1.0 / 60.0, 1.0);
        assert!(
            water.submerged_at(lx, lz),
            "the hollow's tufts are under water"
        );
        assert!(!water.submerged_at(hx, hz), "the hill's tufts stand proud");
    }

    #[test]
    fn the_hud_reads_the_depth_and_the_slowdown() {
        let mut water = flat();
        let ((_, _), (li, lj), _) = extremal(&water);
        let (lx, lz) = (vertex_x(li), vertex_z(lj));
        water.update(1.0 / 60.0, 1.0);
        let line = water.hud(lx, lz);
        assert!(line.starts_with("water "), "{line}");
        assert!(line.contains("slowed"), "{line}");
    }
}
