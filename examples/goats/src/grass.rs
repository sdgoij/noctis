//! The meadow: procedural grass tufts and eating them, ported from the
//! original's `weather.js` (`tuftField`/`drawTufts`) and `food.js`.
//!
//! A tuft's existence and position come from a hash of its 2-unit cell — the
//! same `Math.imul` mixing the original uses, so the meadow is the same meadow —
//! rather than being stored. Only eaten cells are kept, in a [`Meadow`] with the
//! seconds left before each returns, so the field visibly thins as the goat
//! grazes and thickens again over the regrow window.
//!
//! The field draws as one mesh of small cubes ([`build_mesh`]), rebuilt around
//! the goat as it moves. The original walks the cells and issues a `drawCube`
//! each; a mesh is the engine's equivalent (there is no instancing), and the
//! rebuild is cheap enough to follow the goat.

use std::collections::HashMap;

use noctis::prelude::*;

use crate::terrain;
use crate::water;

/// The terrain's half-extent. The meadow is clamped to it: this island's ground
/// does not follow the goat yet (the original's does — see `.notes/goats.md`), so
/// grass beyond the edge would float.
const HALF: f32 = 48.0;

/// World units per tuft cell (the original's `2`).
pub const CELL: f32 = 2.0;
/// The cull radius around the goat, in units (the original's `drawTufts` `576`).
pub const CULL: f32 = 24.0;
/// Within this distance *squared* of the goat a tuft draws its second segment
/// (the original's `180`): near tufts read as a bent blade, far ones as a dot.
const DETAIL2: f32 = 180.0;
/// How close a tuft must be to be in reach of a bite (`TUNING.food.eatRange`).
pub const EAT_RANGE: f32 = 1.1;
/// An eaten tuft regrows after this many seconds, at least and at most
/// (`TUNING.food.regrowMin`/`regrowMax`), jittered per cell.
const REGROW_MIN: f32 = 40.0;
const REGROW_MAX: f32 = 90.0;
/// A cell carries a tuft when its existence hash clears this share
/// (`weather.js` skips `a < 0.45`).
const EXISTENCE: f32 = 0.45;

/// A tuft: its world position and the cell it was derived from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tuft {
    pub x: f32,
    pub z: f32,
    pub cx: i32,
    pub cz: i32,
}

impl Tuft {
    /// How far this tuft is from `(x, z)`, squared.
    pub fn distance2(&self, x: f32, z: f32) -> f32 {
        let dx = self.x - x;
        let dz = self.z - z;
        dx * dx + dz * dz
    }
}

/// The eaten tufts, and the seconds each has left before it returns.
#[derive(Debug)]
pub struct Meadow {
    eaten: HashMap<i64, f32>,
    rng: u32,
}

impl Default for Meadow {
    fn default() -> Self {
        // The original seeds its private food PRNG with this (`food.js`).
        Self {
            eaten: HashMap::new(),
            rng: 0x1f2e_3d4c,
        }
    }
}

impl Meadow {
    /// Whether the tuft in cell `(cx, cz)` has been eaten.
    pub fn is_eaten(&self, cx: i32, cz: i32) -> bool {
        self.eaten.contains_key(&cell_key(cx, cz))
    }

    /// Eat `tuft`, scheduling its regrow over the window.
    pub fn consume(&mut self, tuft: &Tuft) {
        let left = self.regrow();
        self.eaten.insert(cell_key(tuft.cx, tuft.cz), left);
    }

    /// Count the eaten tufts down, returning them to the field as they expire.
    /// Returns whether any tuft came back this step.
    pub fn update(&mut self, dt: f32) -> bool {
        if self.eaten.is_empty() {
            return false;
        }
        let before = self.eaten.len();
        self.eaten.retain(|_, left| {
            *left -= dt;
            *left > 0.0
        });
        self.eaten.len() != before
    }

    /// A fresh regrow time, drawn from the original's xorshift.
    fn regrow(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        REGROW_MIN + (self.rng as f32 / 4_294_967_296.0) * (REGROW_MAX - REGROW_MIN)
    }
}

/// The nearest live tuft within `range` of `(x, z)`, or `None`.
pub fn nearest(x: f32, z: f32, range: f32, meadow: &Meadow) -> Option<Tuft> {
    let r = (range / CELL).ceil() as i32;
    let cx = (x / CELL).floor() as i32;
    let cz = (z / CELL).floor() as i32;
    let mut best: Option<Tuft> = None;
    let mut best_d2 = range * range;
    for i in cx - r..=cx + r {
        for j in cz - r..=cz + r {
            if meadow.is_eaten(i, j) {
                continue;
            }
            let Some(tuft) = tuft_at(i, j) else {
                continue;
            };
            let d2 = tuft.distance2(x, z);
            if d2 <= best_d2 {
                best_d2 = d2;
                best = Some(tuft);
            }
        }
    }
    best
}

/// The tuft in cell `(cx, cz)`, or `None` where the hash says bare ground.
///
/// The existence hash is `a`; the tuft's x comes from a second hash `h2` and its
/// z jitter from `a` itself — exactly `tuftField`'s arithmetic.
pub fn tuft_at(cx: i32, cz: i32) -> Option<Tuft> {
    let a = mix(existence_seed(cx, cz), 13, 1_274_126_177, 16) as f32 / 4_294_967_296.0;
    if a < EXISTENCE {
        return None;
    }
    let h2 = mix(x_seed(cx, cz), 15, 2_246_822_519u32 as i32, 13) as f32 / 4_294_967_296.0;
    Some(Tuft {
        x: cx as f32 * CELL + (h2 - 0.5) * 1.8,
        z: cz as f32 * CELL + (a - 0.5) * 1.8,
        cx,
        cz,
    })
}

/// Build the meadow mesh: one cube per visible tuft around `(gx, gz)`, clamped to
/// the island. `sway` is the clock and `wind` the sway strength — both zero until
/// the weather system gives them values. A tuft standing under `water` is skipped,
/// as the original's `drawTufts` skips it: the surface is drawn over the ground, so
/// a tuft inside a pool would stand out of it.
pub fn build_mesh(
    meadow: &Meadow,
    gx: f32,
    gz: f32,
    sway: f32,
    wind: f32,
    water: Option<&water::Water>,
) -> Mesh {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    let reach = (CULL / CELL).ceil() as i32;
    let edge = (HALF / CELL) as i32;
    let cx = (gx / CELL).floor() as i32;
    let cz = (gz / CELL).floor() as i32;
    let cull2 = CULL * CULL;

    for i in (cx - reach).max(-edge)..=(cx + reach).min(edge) {
        for j in (cz - reach).max(-edge)..=(cz + reach).min(edge) {
            if meadow.is_eaten(i, j) {
                continue;
            }
            let Some(tuft) = tuft_at(i, j) else {
                continue;
            };
            let d2 = tuft.distance2(gx, gz);
            if d2 > cull2 {
                continue;
            }
            let phase =
                mix(sway_seed(i, j), 16, 3_266_489_917u32 as i32, 16) as f32 / 4_294_967_296.0;
            // The original uses `6.28`; `TAU` is the same phase to the eye.
            let off = (sway * 3.0 + phase * std::f32::consts::TAU).sin() * 0.11 * wind;
            // The cull is tested at the tuft's *drawn* position, as the original does.
            if let Some(water) = water
                && water.submerged_at(tuft.x + off, tuft.z + off * 0.4)
            {
                continue;
            }
            let gy = terrain::height(tuft.x, tuft.z);
            push_cube(
                &mut vertices,
                &mut indices,
                [tuft.x + off, gy + 0.06, tuft.z + off * 0.4],
                [0.07, 0.08, 0.07],
            );
            if d2 < DETAIL2 {
                push_cube(
                    &mut vertices,
                    &mut indices,
                    [tuft.x + off * 1.7, gy + 0.20, tuft.z + off * 0.7],
                    [0.055, 0.08, 0.055],
                );
            }
        }
    }

    let mut mesh = Mesh {
        name: "grass/meadow".to_string(),
        vertices,
        indices,
        ..Default::default()
    };
    mesh.bounds = mesh.compute_bounds();
    mesh
}

/// The world height of the meadow's own ground — the tufts stand on it.
///
/// (A thin re-export for the module's tests; [`terrain::height`] is the source.)
#[cfg(test)]
fn ground(x: f32, z: f32) -> f32 {
    terrain::height(x, z)
}

/// A cell's identity, packed exactly as the original's `tuftKey`.
fn cell_key(cx: i32, cz: i32) -> i64 {
    (i64::from(cx) + 4096) * 8192 + (i64::from(cz) + 4096)
}

/// One step of the original's `Math.imul` mix-and-shift chain: XOR `seed` with
/// its own `shift_a` shift, multiply by `mul` (wrapping, as `imul` does), then
/// fold back with a `shift_b` shift.
fn mix(seed: i32, shift_a: u32, mul: i32, shift_b: u32) -> u32 {
    let x = seed ^ ((seed as u32 >> shift_a) as i32);
    let m = x.wrapping_mul(mul);
    (m ^ ((m as u32 >> shift_b) as i32)) as u32
}

/// `tuftField`'s existence seed: `cx * 374761393 + cz * 668265263`.
fn existence_seed(cx: i32, cz: i32) -> i32 {
    cx.wrapping_mul(374_761_393)
        .wrapping_add(cz.wrapping_mul(668_265_263))
}

/// `tuftField`'s x-jitter seed: `cx * 1103515245 + cz * 12345`.
fn x_seed(cx: i32, cz: i32) -> i32 {
    cx.wrapping_mul(1_103_515_245)
        .wrapping_add(cz.wrapping_mul(12_345))
}

/// `tuftField`'s sway seed: `cx * 2654435761 + cz * 40503`.
fn sway_seed(cx: i32, cz: i32) -> i32 {
    cx.wrapping_mul(2_654_435_761u32 as i32)
        .wrapping_add(cz.wrapping_mul(40_503))
}

/// Append an axis-aligned box centred at `center` with half-extents `half`: four
/// vertices per face, wound counter-clockwise from outside so it is lit and culled
/// like any other solid.
fn push_cube(v: &mut Vec<Vertex>, indices: &mut Vec<u32>, center: [f32; 3], half: [f32; 3]) {
    // Each face's outward normal and an in-face tangent; the bitangent is
    // `n × t`, and the corners are `±t ± bitangent` in CCW order.
    let faces = [
        ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([-1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0]),
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0]),
    ];
    for (normal, tangent) in faces {
        let bitangent = cross(normal, tangent);
        let ht = extent(half, tangent);
        let hb = extent(half, bitangent);
        let corner = |st: f32, sb: f32| {
            [
                center[0] + tangent[0] * st * ht + bitangent[0] * sb * hb,
                center[1] + tangent[1] * st * ht + bitangent[1] * sb * hb,
                center[2] + tangent[2] * st * ht + bitangent[2] * sb * hb,
            ]
        };
        let base = v.len() as u32;
        for position in [
            corner(-1.0, -1.0),
            corner(1.0, -1.0),
            corner(1.0, 1.0),
            corner(-1.0, 1.0),
        ] {
            v.push(Vertex {
                position,
                normal,
                uv: [0.0, 0.0],
                tangent: [1.0, 0.0, 0.0, 1.0],
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

/// The half extent of a box along an axis-aligned unit direction (the direction's
/// one non-zero component selects the axis).
fn extent(half: [f32; 3], direction: [f32; 3]) -> f32 {
    half[0] * direction[0].abs() + half[1] * direction[1].abs() + half[2] * direction[2].abs()
}

/// The right-handed cross product.
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
    }

    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    #[test]
    fn the_cell_hash_is_the_original_layout() {
        // The hash is deterministic: the same cell always names the same tuft.
        for (cx, cz) in [(0, 0), (3, -7), (-12, 40), (17, 17)] {
            assert_eq!(tuft_at(cx, cz), tuft_at(cx, cz));
        }
        // A swept patch has both bare cells and tufts (~55% carry one).
        let n = 0..40;
        let cells = n.clone().count() * n.clone().count();
        let tufts = n
            .clone()
            .flat_map(|cx| n.clone().map(move |cz| (cx, cz)))
            .filter(|&(cx, cz)| tuft_at(cx, cz).is_some())
            .count();
        let share = tufts as f32 / cells as f32;
        assert!(
            (0.45..0.65).contains(&share),
            "about half the cells carry a tuft: {share}"
        );
    }

    #[test]
    fn nearest_finds_the_closest_tuft_in_reach() {
        let meadow = Meadow::default();
        let Some(tuft) = nearest(0.0, 0.0, 6.0, &meadow) else {
            panic!("a tuft within 6 units of the origin");
        };
        assert!(tuft.distance2(0.0, 0.0).sqrt() <= 6.0);
        // Nothing is nearer than what `nearest` returned.
        for i in -3..=3 {
            for j in -3..=3 {
                if let Some(other) = tuft_at(i, j) {
                    assert!(other.distance2(0.0, 0.0) >= tuft.distance2(0.0, 0.0) - 1e-6);
                }
            }
        }
    }

    #[test]
    fn an_eaten_tuft_disappears_and_regrows() {
        let mut meadow = Meadow::default();
        let tuft = nearest(0.0, 0.0, 6.0, &meadow).expect("a tuft");
        assert!(!meadow.is_eaten(tuft.cx, tuft.cz));
        meadow.consume(&tuft);
        assert!(meadow.is_eaten(tuft.cx, tuft.cz));

        // Still gone well inside the shortest regrow window...
        meadow.update(REGROW_MIN - 1.0);
        assert!(meadow.is_eaten(tuft.cx, tuft.cz));

        // ...and back once the longest has passed.
        meadow.update(REGROW_MAX);
        assert!(!meadow.is_eaten(tuft.cx, tuft.cz));
    }

    #[test]
    fn the_mesh_holds_the_visible_tufts_on_the_ground() {
        let meadow = Meadow::default();
        let mesh = build_mesh(&meadow, 0.0, 0.0, 0.0, 0.0, None);
        assert!(!mesh.vertices.is_empty(), "the spawn meadow has tufts");
        assert_eq!(mesh.indices.len() % 3, 0);
        assert!(mesh.bounds.is_some());

        // Every vertex is inside the island and stands on the ground — the cube
        // bodies reach at most ~0.28 above the tuft's own height.
        for v in &mesh.vertices {
            let (x, y, z) = (v.position[0], v.position[1], v.position[2]);
            assert!(
                x.abs() <= HALF + 1.0 && z.abs() <= HALF + 1.0,
                "inside the island"
            );
            assert!((y - ground(x, z)).abs() < 0.3, "sits on the ground: {y}");
        }
    }

    #[test]
    fn every_cube_face_winds_outward() {
        let mesh = build_mesh(&Meadow::default(), 0.0, 0.0, 0.0, 0.0, None);
        for tri in mesh.indices.as_chunks::<3>().0 {
            let normal = mesh.vertices[tri[0] as usize].normal;
            let p = |k: usize| mesh.vertices[tri[k] as usize].position;
            let geometric = cross(sub(p(1), p(0)), sub(p(2), p(0)));
            assert!(
                dot(geometric, normal) > 0.0,
                "a face is wound counter-clockwise from outside"
            );
        }
    }

    #[test]
    fn an_eaten_tuft_leaves_the_mesh() {
        let mut meadow = Meadow::default();
        let before = build_mesh(&meadow, 0.0, 0.0, 0.0, 0.0, None).vertices.len();

        // Eat every tuft inside a tight radius of the origin.
        let mut eaten = 0;
        for i in -2..=2 {
            for j in -2..=2 {
                if let Some(tuft) = tuft_at(i, j)
                    && tuft.distance2(0.0, 0.0) <= 4.0
                {
                    meadow.consume(&tuft);
                    eaten += 1;
                }
            }
        }
        assert!(eaten > 0, "there was something to eat");
        let after = build_mesh(&meadow, 0.0, 0.0, 0.0, 0.0, None).vertices.len();
        assert!(after < before, "the meadow thinned: {before} -> {after}");
    }

    #[test]
    fn a_tuft_under_the_water_leaves_the_mesh() {
        let meadow = Meadow::default();
        let dry = build_mesh(&meadow, 0.0, 0.0, 0.0, 0.0, None).vertices.len();

        // A downpour fills the table; the tufts in the hollows go under.
        let mut flood = water::Water::new();
        flood.update(1.0 / 60.0, 1.0);
        let wet = build_mesh(&meadow, 0.0, 0.0, 0.0, 0.0, Some(&flood))
            .vertices
            .len();
        assert!(wet < dry, "the flood hid some tufts: {dry} -> {wet}");
    }
}
