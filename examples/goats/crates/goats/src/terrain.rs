//! The heightfield ground, ported from the original `world.js`.
//!
//! The same three-octave value noise, the same flat spawn bowl and the same
//! central-difference normals, so the ground the port draws is the ground the
//! original drew. Everything that reads the ground in the original reads one
//! `terrainHeight` function; [`height`] is that function here too.
//!
//! Not in M0: the per-vertex material colours (the engine's `Vertex` has no
//! colour yet — see `.notes/goats.md`) and the craters a blast stamps in.

use noctis::prelude::*;

/// World units per grid cell (the original's `TERRAIN_CELL`, matching the grass).
pub const CELL: f32 = 2.0;
/// Quads per side: a field of ±48 units around the anchor (the original's 48).
pub const QUADS: usize = 48;

/// Peak displacement of the relief, in metres (`TUNING.terrain.relief`).
const RELIEF: f32 = 2.1;
/// The spawn-bowl radius that stays level, in units (`TUNING.terrain.flat`).
const FLAT: f32 = 6.0;
/// The units over which the bowl reaches full relief (`TUNING.terrain.ramp`).
const RAMP: f32 = 16.0;
/// Texture tiles per world unit (`TUNING.terrain.uv`).
const UV: f32 = 0.06;

/// The ground height at `(x, z)`: the flat bowl plus the relief noise.
///
/// The one source of truth for the ground — the goat, the camera and the terrain
/// mesh all read it, so nothing floats or sinks as the field rises.
pub fn height(x: f32, z: f32) -> f32 {
    let d = (x * x + z * z).sqrt();
    // The relief eases in over `RAMP` past the bowl, so there is no cliff at its
    // edge (the original's smoothstep).
    let t = ((d - FLAT) / RAMP).clamp(0.0, 1.0);
    let t = t * t * (3.0 - 2.0 * t);
    (shape(x, z) - 0.5) * 2.0 * RELIEF * t
}

/// Three octaves of value noise: broad hills with smaller bumps on top.
fn shape(x: f32, z: f32) -> f32 {
    0.62 * vnoise2(x * 0.033 + 13.7, z * 0.033 + 5.1)
        + 0.26 * vnoise2(x * 0.081 + 41.3, z * 0.081 + 27.9)
        + 0.12 * vnoise2(x * 0.191 + 77.1, z * 0.191 + 61.3)
}

/// Smooth `0..1` value noise: bilinear over four hashed corners, smoothstepped
/// (the original's `vnoise2`).
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

/// The original's `hash`: a sine scrambler folded into `0..1`. (The multiplier
/// is the original's `43758.5453`, rounded to `f32`.)
pub(crate) fn hash(n: f32) -> f32 {
    let s = (n * 127.1).sin() * 43_758.547;
    s - s.floor()
}

// ---- craters (M19d) -------------------------------------------------------
//
// A bang dishes the ground. The dish is a *term in the ground's height* rather
// than geometry of its own, so everything that reads the ground — the goat, the
// camera, the grass and the water — stands in the hole for free. The list is the
// only state, capped at `CRATER_MAX` with the oldest retired first, and it heals
// by scaling the dish down over `CRATER_HEAL`.

/// Metres of the dish's radius (the original's `crater.radius`).
const CRATER_RADIUS: f32 = 1.6;
/// Metres deep at the centre (`crater.depth`).
const CRATER_DEPTH: f32 = 0.45;
/// Metres of the raised rim just outside the dish (`crater.lip`).
const CRATER_LIP: f32 = 0.12;
/// Seconds until the ground is flat again (`crater.heal`).
const CRATER_HEAL: f32 = 240.0;
/// Live craters; the oldest retires first (`crater.max`).
const CRATER_MAX: usize = 24;
/// Metres outside the dish's radius where the raised lip has fallen to nothing.
const CRATER_LIP_OUT: f32 = 1.45;
/// How far the dish eases before the mesh is told to catch up (`CRATER_STEP`):
/// the quantization is what keeps the ground the goat stands on exactly what the
/// mesh shows, and costs a rebuild roughly every minute of a crater's four.
const CRATER_STEP: f32 = 0.1;

/// A crater a blast left: a bowl with a raised rim that eases back to flat.
#[derive(Clone, Copy, Debug)]
struct Crater {
    x: f32,
    z: f32,
    /// The dish's radius, jittered a little per bang.
    r: f32,
    /// `(r * CRATER_LIP_OUT)^2`: the squared reach, so a query is one compare.
    reach2: f32,
    /// The depth at its deepest.
    depth: f32,
    /// The depth now, quantized — what the mesh was built from.
    dip: f32,
    /// The raised rim's height.
    lip: f32,
    /// Seconds since it was dug.
    age: f32,
    /// Seconds until the ground is flat again.
    heal: f32,
}

/// The craters on the ground, and the ground's own deformation from them: the
/// single source of truth for the surface, so the goat, the camera, the grass and
/// the water all stand in a hole for free.
#[derive(Debug, Default)]
pub struct Craters {
    list: Vec<Crater>,
    /// Bumped whenever the ground's shape moves, so a caller knows to rebuild.
    revision: u64,
}

impl Craters {
    /// The revision of the ground's shape; a change means the mesh, the water and
    /// the grass must be rebuilt.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The craters' contribution to the ground at a point (`craterDipAt`): a bowl
    /// with a raised rim. `0` away from every crater.
    pub fn dip_at(&self, x: f32, z: f32) -> f32 {
        let mut dip = 0.0;
        for c in &self.list {
            let dx = x - c.x;
            let dz = z - c.z;
            let d2 = dx * dx + dz * dz;
            if d2 > c.reach2 {
                continue;
            }
            dip += dip_one(c, d2);
        }
        dip
    }

    /// The ground's surface height at a point: the base bowl and noise plus the
    /// craters' dish (the original's `terrainHeight`).
    pub fn surface(&self, x: f32, z: f32) -> f32 {
        height(x, z) + self.dip_at(x, z)
    }

    /// Whether a point is inside a live crater's dish — the grass it swallowed.
    pub fn covers(&self, x: f32, z: f32) -> bool {
        self.list.iter().any(|c| {
            let dx = x - c.x;
            let dz = z - c.z;
            dx * dx + dz * dz <= c.r * c.r
        })
    }

    /// Dig a crater at `(x, z)`. The radius varies a little per bang; the oldest
    /// retires first when at the cap (`addCrater`).
    pub fn add(&mut self, x: f32, z: f32, seed: f32) {
        if CRATER_RADIUS <= 0.0 || CRATER_DEPTH <= 0.0 || CRATER_MAX == 0 {
            return;
        }
        while self.list.len() >= CRATER_MAX {
            self.list.remove(0);
        }
        let r = CRATER_RADIUS * (0.85 + hash(seed) * 0.3);
        let reach = r * CRATER_LIP_OUT;
        self.list.push(Crater {
            x,
            z,
            r,
            reach2: reach * reach,
            depth: CRATER_DEPTH,
            dip: CRATER_DEPTH,
            lip: CRATER_LIP,
            age: 0.0,
            heal: CRATER_HEAL,
        });
        self.revision += 1;
    }

    /// Age the craters (`updateCraters`); returns whether the ground's shape moved.
    pub fn update(&mut self, dt: f32) -> bool {
        let mut moved = false;
        let mut i = 0;
        while i < self.list.len() {
            let c = self.list[i];
            let age = c.age + dt;
            if age >= c.heal {
                self.list.remove(i);
                moved = true;
                continue;
            }
            let next = c.depth * (1.0 - age / c.heal);
            if c.dip - next >= CRATER_STEP {
                let c = &mut self.list[i];
                c.age = age;
                c.dip = (next / CRATER_STEP).floor() * CRATER_STEP;
                moved = true;
            } else {
                self.list[i].age = age;
            }
            i += 1;
        }
        if moved {
            self.revision += 1;
        }
        moved
    }
}

/// One crater's contribution at a point, from its squared distance (`dip_one` is
/// `craterDipOne`): `-dip` at the centre, a raised `lip` just outside the rim.
fn dip_one(c: &Crater, d2: f32) -> f32 {
    let t = d2.sqrt() / c.r;
    if t < 1.0 {
        let s = 1.0 - t;
        -c.dip * s * s * (3.0 - 2.0 * s)
    } else {
        let mut s = (t - 1.0) / (CRATER_LIP_OUT - 1.0);
        if s > 1.0 {
            return 0.0;
        }
        s = 1.0 - s;
        c.lip * s * s * (3.0 - 2.0 * s)
    }
}

/// Build the ground mesh: a `QUADS × QUADS` grid centred on the origin, with
/// per-vertex normals from central differences (the original's `terrainVertex`).
/// `craters` dishes the ground, so a rebuild after a bang shows the hole.
///
/// The vertex layout is the engine's standard one; the per-vertex colours the
/// original carries have no slot yet, so the mesh is one material for now.
pub fn build_mesh(craters: &Craters) -> Mesh {
    let n = QUADS + 1;
    let half = QUADS as f32 * CELL * 0.5;

    // Heights first: the normals are central differences across the grid.
    let mut heights = vec![0.0f32; n * n];
    for j in 0..n {
        for i in 0..n {
            let x = -half + i as f32 * CELL;
            let z = -half + j as f32 * CELL;
            heights[j * n + i] = craters.surface(x, z);
        }
    }
    // The grid height at `(i, j)`. The original clamps the difference step at an
    // edge to a one-sided difference, which is this index clamp.
    let grid = |i: usize, j: usize| heights[j * n + i];

    let mut vertices = Vec::with_capacity(n * n);
    for j in 0..n {
        for i in 0..n {
            let x = -half + i as f32 * CELL;
            let z = -half + j as f32 * CELL;
            let y = heights[j * n + i];

            let dx = (grid(i.saturating_add(1).min(n - 1), j) - grid(i.saturating_sub(1), j))
                / (2.0 * CELL);
            let dz = (grid(i, j.saturating_add(1).min(n - 1)) - grid(i, j.saturating_sub(1)))
                / (2.0 * CELL);
            let inv = 1.0 / (dx * dx + dz * dz + 1.0).sqrt();

            vertices.push(Vertex {
                position: [x, y, z],
                normal: [-dx * inv, inv, -dz * inv],
                uv: [x * UV, z * UV],
                tangent: [1.0, 0.0, 0.0, 1.0],
            });
        }
    }

    // Two triangles per quad, wound as the original's `terrainIndices`.
    let mut indices = Vec::with_capacity(QUADS * QUADS * 6);
    for j in 0..QUADS {
        for i in 0..QUADS {
            let a = (j * n + i) as u32;
            let b = a + 1;
            let c = a + n as u32;
            let d = c + 1;
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }

    let mut mesh = Mesh {
        name: "terrain/meadow".to_string(),
        vertices,
        indices,
        ..Default::default()
    };
    mesh.bounds = mesh.compute_bounds();
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bowl_centre_is_level() {
        // The goat starts on flat grass: the bowl at the origin is exactly zero.
        assert_eq!(height(0.0, 0.0), 0.0);
        // And the relief only reaches full strength past the bowl's ramp.
        assert!(height(FLAT + RAMP, 0.0).abs() > 0.0);
    }

    #[test]
    fn the_mesh_is_a_full_grid() {
        let mesh = build_mesh(&Craters::default());
        let n = QUADS + 1;

        assert_eq!(mesh.vertices.len(), n * n);
        assert_eq!(mesh.indices.len(), QUADS * QUADS * 6);
        assert_eq!(mesh.triangle_count(), QUADS * QUADS * 2);

        let bounds = mesh.bounds.expect("the mesh has bounds");
        let span = QUADS as f32 * CELL;
        assert!((bounds.max.x - bounds.min.x - span).abs() < 1e-3);
        assert!((bounds.max.z - bounds.min.z - span).abs() < 1e-3);
        assert!((bounds.min.x + span * 0.5).abs() < 1e-3, "centred on x");

        // Normals point up (no overhang), so the ground is lit from above.
        assert!(mesh.vertices.iter().all(|v| v.normal[1] > 0.0));
        // The first vertex sits on the sampled height at its own position.
        let corner = mesh.vertices[0];
        assert!((corner.position[1] - height(corner.position[0], corner.position[2])).abs() < 1e-4);
    }

    #[test]
    fn indices_stay_in_range() {
        let mesh = build_mesh(&Craters::default());
        let count = mesh.vertices.len() as u32;
        assert!(mesh.indices.iter().all(|&index| index < count));
    }

    #[test]
    fn a_crater_dishes_the_ground_and_heals() {
        let mut craters = Craters::default();
        let before = craters.surface(0.0, 0.0);
        assert_eq!(craters.dip_at(0.0, 0.0), 0.0, "flat until a bang");
        assert!(craters.list.is_empty());

        craters.add(0.0, 0.0, 1.5);
        assert_eq!(craters.list.len(), 1);
        assert!(craters.dip_at(0.0, 0.0) < 0.0, "the centre is a hole");
        // The centre is the deepest point: a faintly shallower dip just off it.
        assert!(craters.dip_at(0.0, 0.0) < craters.dip_at(0.06, 0.0));
        assert_eq!(craters.surface(0.0, 0.0), before + craters.dip_at(0.0, 0.0));
        // The rim is raised just outside the dish.
        let rim = craters.dip_at(CRATER_RADIUS * 1.2, 0.0);
        assert!(rim > 0.0, "the lip stands proud: {rim}");
        // Well outside, the ground is untouched.
        assert_eq!(craters.dip_at(CRATER_RADIUS * 3.0, 0.0), 0.0);

        // The dish eases back to flat over its heal, then the ground is whole.
        let revision = craters.revision();
        craters.update(CRATER_HEAL * 0.5);
        assert!(craters.revision() > revision, "the ground moved");
        assert!(
            (craters.dip_at(0.0, 0.0) + CRATER_DEPTH * 0.5).abs() < CRATER_STEP,
            "half the heal, half the hole: {}",
            craters.dip_at(0.0, 0.0)
        );
        craters.update(CRATER_HEAL);
        assert!(craters.list.is_empty(), "the crater closed over");
        assert_eq!(craters.dip_at(0.0, 0.0), 0.0);
    }

    #[test]
    fn the_oldest_crater_retires_at_the_cap() {
        let mut craters = Craters::default();
        for i in 0..(CRATER_MAX + 3) {
            craters.add(i as f32 * 20.0, 0.0, i as f32);
        }
        assert_eq!(craters.list.len(), CRATER_MAX, "the list is capped");
        // The first few bangs' holes are gone; the last is there.
        assert!(!craters.covers(0.0, 0.0), "the oldest closed over");
        assert!(craters.covers((CRATER_MAX + 2) as f32 * 20.0, 0.0));
    }

    #[test]
    fn a_crater_covers_what_it_swallowed() {
        let mut craters = Craters::default();
        craters.add(0.0, 0.0, 1.0);
        assert!(craters.covers(0.0, 0.0), "inside the dish");
        assert!(craters.covers(CRATER_RADIUS * 0.9, 0.0));
        assert!(!craters.covers(CRATER_RADIUS * 1.5, 0.0), "outside it");
    }
}
