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
fn hash(n: f32) -> f32 {
    let s = (n * 127.1).sin() * 43_758.547;
    s - s.floor()
}

/// Build the ground mesh: a `QUADS × QUADS` grid centred on the origin, with
/// per-vertex normals from central differences (the original's `terrainVertex`).
///
/// The vertex layout is the engine's standard one; the per-vertex colours the
/// original carries have no slot yet, so the mesh is one material for now.
pub fn build_mesh() -> Mesh {
    let n = QUADS + 1;
    let half = QUADS as f32 * CELL * 0.5;

    // Heights first: the normals are central differences across the grid.
    let mut heights = vec![0.0f32; n * n];
    for j in 0..n {
        for i in 0..n {
            let x = -half + i as f32 * CELL;
            let z = -half + j as f32 * CELL;
            heights[j * n + i] = height(x, z);
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
        bounds: None,
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
        let mesh = build_mesh();
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
        let mesh = build_mesh();
        let count = mesh.vertices.len() as u32;
        assert!(mesh.indices.iter().all(|&index| index < count));
    }
}
