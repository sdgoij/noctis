//! Procedural primitive meshes.
//!
//! Each generator returns a ready-to-upload [`Mesh`](crate::assets::Mesh)
//! carrying positions, unit normals, texture coordinates, tangents (with
//! handedness in `w`), triangle indices, and computed object-space bounds.
//! Triangles are wound counter-clockwise when viewed from outside the surface,
//! matching the renderer's front-face convention.
//!
//! The generators are deterministic and allocation-light, so the tests can
//! assert exact triangle counts and bounds. [`AssetRegistry::with_builtins`]
//! (see [`crate::assets`]) registers them under `primitives/<name>`, so a scene
//! that references one always resolves.
//!
//! [`AssetRegistry::with_builtins`]: crate::assets::AssetRegistry::with_builtins

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use crate::assets::{Mesh, Vertex};
use math::{Vec3, vec3};

/// Assemble a vertex from its parts.
fn vertex(position: Vec3, normal: Vec3, uv: [f32; 2], tangent: [f32; 4]) -> Vertex {
    Vertex {
        position: [position.x, position.y, position.z],
        normal: [normal.x, normal.y, normal.z],
        uv,
        tangent,
    }
}

/// Normalize an `(x, y, z)` triple, falling back to `+Y` for a zero vector.
fn normalized(x: f32, y: f32, z: f32) -> Vec3 {
    let len_sq = x * x + y * y + z * z;
    if len_sq > 1e-20 {
        let inv = len_sq.sqrt().recip();
        vec3(x * inv, y * inv, z * inv)
    } else {
        vec3(0.0, 1.0, 0.0)
    }
}

/// Name a mesh and compute its object-space bounds.
fn finalize(name: &str, vertices: Vec<Vertex>, indices: Vec<u32>) -> Mesh {
    let mut mesh = Mesh {
        name: name.to_string(),
        vertices,
        indices,
        bounds: None,
    };
    mesh.bounds = mesh.compute_bounds();
    mesh
}

/// An axis-aligned cube centred on the origin with edge length `size`.
///
/// 24 vertices (four per face) so each face gets its own normal and UVs; 12
/// triangles.
pub fn cube(size: f32) -> Mesh {
    let h = size * 0.5;
    // Each face: outward normal, tangent (U), bitangent (V).
    let faces = [
        (
            vec3(1.0, 0.0, 0.0),
            vec3(0.0, 0.0, -1.0),
            vec3(0.0, 1.0, 0.0),
        ),
        (
            vec3(-1.0, 0.0, 0.0),
            vec3(0.0, 0.0, 1.0),
            vec3(0.0, 1.0, 0.0),
        ),
        (
            vec3(0.0, 1.0, 0.0),
            vec3(1.0, 0.0, 0.0),
            vec3(0.0, 0.0, -1.0),
        ),
        (
            vec3(0.0, -1.0, 0.0),
            vec3(1.0, 0.0, 0.0),
            vec3(0.0, 0.0, 1.0),
        ),
        (
            vec3(0.0, 0.0, 1.0),
            vec3(1.0, 0.0, 0.0),
            vec3(0.0, 1.0, 0.0),
        ),
        (
            vec3(0.0, 0.0, -1.0),
            vec3(-1.0, 0.0, 0.0),
            vec3(0.0, 1.0, 0.0),
        ),
    ];

    let mut vertices = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    for (normal, u, v) in faces {
        let center = normal * h;
        let base = vertices.len() as u32;
        let corners = [
            center - u * h - v * h,
            center + u * h - v * h,
            center + u * h + v * h,
            center - u * h + v * h,
        ];
        let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let tangent = [u.x, u.y, u.z, 1.0];
        for (corner, uv) in corners.into_iter().zip(uvs) {
            vertices.push(vertex(corner, normal, uv, tangent));
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    finalize("cube", vertices, indices)
}

/// A subdivided plane on the `XZ` axis, centred on the origin and facing `+Y`.
///
/// `subdivisions` is clamped to at least `1`; triangle count is
/// `subdivisions² * 2`.
pub fn plane(size: f32, subdivisions: u32) -> Mesh {
    let n = subdivisions.max(1);
    let h = size * 0.5;
    let step = size / n as f32;

    let stride = n + 1;
    let mut vertices = Vec::with_capacity((stride * stride) as usize);
    for iz in 0..=n {
        for ix in 0..=n {
            let x = -h + ix as f32 * step;
            let z = -h + iz as f32 * step;
            let uv = [ix as f32 / n as f32, iz as f32 / n as f32];
            vertices.push(vertex(
                vec3(x, 0.0, z),
                vec3(0.0, 1.0, 0.0),
                uv,
                [1.0, 0.0, 0.0, 1.0],
            ));
        }
    }

    let mut indices = Vec::with_capacity((n * n * 6) as usize);
    for iz in 0..n {
        for ix in 0..n {
            let i0 = iz * stride + ix;
            let i1 = i0 + 1;
            let i2 = i0 + stride;
            let i3 = i2 + 1;
            indices.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
        }
    }
    finalize("plane", vertices, indices)
}

/// A UV sphere of `radius`. `rings` is the number of latitude bands (clamped to
/// `>= 2`) and `sectors` the number of longitude bands (clamped to `>= 3`).
///
/// Triangle count is `rings * sectors * 2`.
pub fn sphere(radius: f32, rings: u32, sectors: u32) -> Mesh {
    let rings = rings.max(2);
    let sectors = sectors.max(3);

    let stride = sectors + 1;
    let mut vertices = Vec::with_capacity((stride * (rings + 1)) as usize);
    for i in 0..=rings {
        let phi = PI * i as f32 / rings as f32; // 0 (north pole) .. PI (south pole)
        let sin_phi = phi.sin();
        let cos_phi = phi.cos();
        let v = i as f32 / rings as f32;
        for j in 0..=sectors {
            let theta = TAU * j as f32 / sectors as f32;
            let (sin_t, cos_t) = theta.sin_cos();
            let normal = vec3(sin_phi * cos_t, cos_phi, sin_phi * sin_t);
            vertices.push(vertex(
                normal * radius,
                normal,
                [j as f32 / sectors as f32, v],
                [-sin_t, 0.0, cos_t, 1.0],
            ));
        }
    }

    let mut indices = Vec::with_capacity((rings * sectors * 6) as usize);
    for i in 0..rings {
        for j in 0..sectors {
            let i0 = i * stride + j;
            let i1 = i0 + 1;
            let i2 = i0 + stride;
            let i3 = i2 + 1;
            indices.extend_from_slice(&[i0, i1, i2, i1, i3, i2]);
        }
    }
    finalize("sphere", vertices, indices)
}

/// A `Y`-axis cylinder of `radius` and total `height`, with flat caps.
///
/// Triangle count is `4 * sectors` (`2 * sectors` for the wall, `sectors` per
/// cap).
pub fn cylinder(radius: f32, height: f32, sectors: u32) -> Mesh {
    let sectors = sectors.max(3);
    let half = height * 0.5;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    // Wall: a (bottom, top) vertex pair per angular sample.
    let side_base = vertices.len() as u32;
    for j in 0..=sectors {
        let theta = TAU * j as f32 / sectors as f32;
        let (sin_t, cos_t) = theta.sin_cos();
        let normal = vec3(cos_t, 0.0, sin_t);
        let tangent = [-sin_t, 0.0, cos_t, 1.0];
        let u = j as f32 / sectors as f32;
        let (x, z) = (radius * cos_t, radius * sin_t);
        vertices.push(vertex(vec3(x, -half, z), normal, [u, 0.0], tangent));
        vertices.push(vertex(vec3(x, half, z), normal, [u, 1.0], tangent));
    }
    for j in 0..sectors {
        let b0 = side_base + j * 2;
        let t0 = b0 + 1;
        let b1 = b0 + 2;
        let t1 = b0 + 3;
        indices.extend_from_slice(&[b0, t0, b1, t0, t1, b1]);
    }

    // Top cap (+Y).
    let top_center = vertices.len() as u32;
    vertices.push(vertex(
        vec3(0.0, half, 0.0),
        vec3(0.0, 1.0, 0.0),
        [0.5, 0.5],
        [1.0, 0.0, 0.0, 1.0],
    ));
    let top_ring = vertices.len() as u32;
    for j in 0..=sectors {
        let theta = TAU * j as f32 / sectors as f32;
        let (sin_t, cos_t) = theta.sin_cos();
        vertices.push(vertex(
            vec3(radius * cos_t, half, radius * sin_t),
            vec3(0.0, 1.0, 0.0),
            [0.5 + 0.5 * cos_t, 0.5 + 0.5 * sin_t],
            [1.0, 0.0, 0.0, 1.0],
        ));
    }
    for j in 0..sectors {
        indices.extend_from_slice(&[top_center, top_ring + j + 1, top_ring + j]);
    }

    // Bottom cap (-Y).
    let bottom_center = vertices.len() as u32;
    vertices.push(vertex(
        vec3(0.0, -half, 0.0),
        vec3(0.0, -1.0, 0.0),
        [0.5, 0.5],
        [1.0, 0.0, 0.0, 1.0],
    ));
    let bottom_ring = vertices.len() as u32;
    for j in 0..=sectors {
        let theta = TAU * j as f32 / sectors as f32;
        let (sin_t, cos_t) = theta.sin_cos();
        vertices.push(vertex(
            vec3(radius * cos_t, -half, radius * sin_t),
            vec3(0.0, -1.0, 0.0),
            [0.5 + 0.5 * cos_t, 0.5 - 0.5 * sin_t],
            [1.0, 0.0, 0.0, 1.0],
        ));
    }
    for j in 0..sectors {
        indices.extend_from_slice(&[bottom_center, bottom_ring + j, bottom_ring + j + 1]);
    }

    finalize("cylinder", vertices, indices)
}

/// A `Y`-axis capsule: a `height`-tall cylindrical section capped by two
/// hemispheres of `radius`.
///
/// Total height is `height + 2 * radius`. The number of hemisphere rings is
/// derived from `sectors`, so triangle count is `(2 * cap_rings + 1) * sectors *
/// 2` with `cap_rings = max(2, sectors / 2)`.
pub fn capsule(radius: f32, height: f32, sectors: u32) -> Mesh {
    let sectors = sectors.max(3);
    let cap_rings = (sectors / 2).max(2);
    let half = height * 0.5;

    // Ring descriptors top-to-bottom: (y, radial normal factor, Y normal
    // factor). The vertex radius is `radius * nr`, so the wall rings (`nr = 1`)
    // sit at the cylinder radius and the hemisphere rings taper to the poles.
    let mut rings: Vec<(f32, f32, f32)> = Vec::new();
    for i in 0..=cap_rings {
        let phi = FRAC_PI_2 * i as f32 / cap_rings as f32;
        rings.push((half + radius * phi.cos(), phi.sin(), phi.cos()));
    }
    rings.push((-half, 1.0, 0.0));
    for i in 1..=cap_rings {
        let phi = FRAC_PI_2 + FRAC_PI_2 * i as f32 / cap_rings as f32;
        rings.push((-half + radius * phi.cos(), phi.sin(), phi.cos()));
    }

    let ring_count = rings.len();
    let stride = sectors + 1;
    let mut vertices = Vec::with_capacity(ring_count * stride as usize);
    for (ri, &(y, nr, ny)) in rings.iter().enumerate() {
        let v = ri as f32 / (ring_count - 1) as f32;
        for j in 0..=sectors {
            let theta = TAU * j as f32 / sectors as f32;
            let (sin_t, cos_t) = theta.sin_cos();
            let position = vec3(radius * nr * cos_t, y, radius * nr * sin_t);
            let normal = normalized(nr * cos_t, ny, nr * sin_t);
            vertices.push(vertex(
                position,
                normal,
                [j as f32 / sectors as f32, v],
                [-sin_t, 0.0, cos_t, 1.0],
            ));
        }
    }

    let mut indices = Vec::new();
    for ri in 0..ring_count - 1 {
        for j in 0..sectors {
            let i0 = ri as u32 * stride + j;
            let i1 = i0 + 1;
            let i2 = i0 + stride;
            let i3 = i2 + 1;
            indices.extend_from_slice(&[i0, i1, i2, i1, i3, i2]);
        }
    }
    finalize("capsule", vertices, indices)
}

/// A torus on the `XZ` plane: tube radius `minor` swept around a ring of radius
/// `major`.
///
/// `sectors` samples the ring and `sides` samples the tube; triangle count is
/// `sectors * sides * 2`.
pub fn torus(major: f32, minor: f32, sectors: u32, sides: u32) -> Mesh {
    let sectors = sectors.max(3);
    let sides = sides.max(3);

    let stride = sides + 1;
    let mut vertices = Vec::with_capacity((stride * (sectors + 1)) as usize);
    for i in 0..=sectors {
        let u = TAU * i as f32 / sectors as f32;
        let (sin_u, cos_u) = u.sin_cos();
        for j in 0..=sides {
            let v = TAU * j as f32 / sides as f32;
            let (sin_v, cos_v) = v.sin_cos();
            let ring = major + minor * cos_v;
            let position = vec3(ring * cos_u, minor * sin_v, ring * sin_u);
            let normal = vec3(cos_v * cos_u, sin_v, cos_v * sin_u);
            vertices.push(vertex(
                position,
                normal,
                [i as f32 / sectors as f32, j as f32 / sides as f32],
                [-sin_u, 0.0, cos_u, 1.0],
            ));
        }
    }

    let mut indices = Vec::with_capacity((sectors * sides * 6) as usize);
    for i in 0..sectors {
        for j in 0..sides {
            let i0 = i * stride + j;
            let i1 = i0 + 1;
            let i2 = i0 + stride;
            let i3 = i2 + 1;
            indices.extend_from_slice(&[i0, i1, i2, i1, i3, i2]);
        }
    }
    finalize("torus", vertices, indices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use math::{approx_eq, vec3_approx_eq};

    /// Structural invariants every generator must satisfy.
    fn assert_well_formed(mesh: &Mesh) {
        assert!(!mesh.vertices.is_empty(), "{}: no vertices", mesh.name);
        assert!(!mesh.indices.is_empty(), "{}: no indices", mesh.name);
        assert_eq!(
            mesh.indices.len() % 3,
            0,
            "{}: index count {} is not a multiple of 3",
            mesh.name,
            mesh.indices.len()
        );
        let count = mesh.vertices.len() as u32;
        for &index in &mesh.indices {
            assert!(
                index < count,
                "{}: index {index} out of range (0..{count})",
                mesh.name
            );
        }
        for (i, v) in mesh.vertices.iter().enumerate() {
            let len =
                (v.normal[0] * v.normal[0] + v.normal[1] * v.normal[1] + v.normal[2] * v.normal[2])
                    .sqrt();
            assert!(
                (len - 1.0).abs() < 1e-3,
                "{}: vertex {i} normal length {len} not unit",
                mesh.name
            );
        }
        assert!(mesh.bounds.is_some(), "{}: missing bounds", mesh.name);
        assert_eq!(mesh.triangle_count(), mesh.indices.len() / 3);
    }

    fn assert_bounds(mesh: &Mesh, min: Vec3, max: Vec3) {
        let bounds = mesh.bounds.expect("bounds");
        assert!(
            vec3_approx_eq(bounds.min, min, 1e-4),
            "{}: min {:?} != {:?}",
            mesh.name,
            bounds.min,
            min
        );
        assert!(
            vec3_approx_eq(bounds.max, max, 1e-4),
            "{}: max {:?} != {:?}",
            mesh.name,
            bounds.max,
            max
        );
    }

    #[test]
    fn cube_is_twelve_triangles_with_unit_bounds() {
        let mesh = cube(2.0);
        assert_well_formed(&mesh);
        assert_eq!(mesh.vertices.len(), 24);
        assert_eq!(mesh.triangle_count(), 12);
        assert_bounds(&mesh, vec3(-1.0, -1.0, -1.0), vec3(1.0, 1.0, 1.0));
    }

    #[test]
    fn plane_subdivides_into_quads() {
        let mesh = plane(4.0, 3);
        assert_well_formed(&mesh);
        assert_eq!(mesh.vertices.len(), 16);
        assert_eq!(mesh.triangle_count(), 3 * 3 * 2);
        assert_bounds(&mesh, vec3(-2.0, 0.0, -2.0), vec3(2.0, 0.0, 2.0));
    }

    #[test]
    fn plane_clamps_subdivisions_to_one() {
        let mesh = plane(1.0, 0);
        assert_well_formed(&mesh);
        assert_eq!(mesh.triangle_count(), 2);
    }

    #[test]
    fn sphere_triangle_count_and_radius_bounds() {
        let mesh = sphere(1.5, 16, 24);
        assert_well_formed(&mesh);
        assert_eq!(mesh.triangle_count(), 16 * 24 * 2);
        assert_bounds(&mesh, vec3(-1.5, -1.5, -1.5), vec3(1.5, 1.5, 1.5));
    }

    #[test]
    fn cylinder_has_wall_and_two_caps() {
        let mesh = cylinder(1.0, 2.0, 24);
        assert_well_formed(&mesh);
        assert_eq!(mesh.triangle_count(), 4 * 24);
        assert_bounds(&mesh, vec3(-1.0, -1.0, -1.0), vec3(1.0, 1.0, 1.0));
    }

    #[test]
    fn capsule_bounds_include_hemispheres() {
        let radius = 0.5;
        let height = 2.0;
        let sectors = 24;
        let mesh = capsule(radius, height, sectors);
        assert_well_formed(&mesh);
        let cap_rings = (sectors / 2).max(2);
        assert_eq!(
            mesh.triangle_count(),
            ((2 * cap_rings + 1) * sectors * 2) as usize
        );
        assert_bounds(
            &mesh,
            vec3(-radius, -(height * 0.5 + radius), -radius),
            vec3(radius, height * 0.5 + radius, radius),
        );
    }

    #[test]
    fn torus_bounds_reflect_major_and_minor() {
        let mesh = torus(1.0, 0.25, 24, 16);
        assert_well_formed(&mesh);
        assert_eq!(mesh.triangle_count(), 24 * 16 * 2);
        assert_bounds(&mesh, vec3(-1.25, -0.25, -1.25), vec3(1.25, 0.25, 1.25));
    }

    #[test]
    fn all_primitives_are_well_formed_at_edge_arguments() {
        assert_well_formed(&cube(0.5));
        assert_well_formed(&plane(1.0, 1));
        assert_well_formed(&sphere(2.0, 2, 3));
        assert_well_formed(&cylinder(0.25, 1.0, 3));
        assert_well_formed(&capsule(0.25, 1.0, 3));
        assert_well_formed(&torus(0.75, 0.2, 3, 3));
    }

    #[test]
    fn triangle_counts_are_exact() {
        assert_eq!(cube(1.0).triangle_count(), 12);
        assert_eq!(plane(1.0, 4).triangle_count(), 32);
        assert_eq!(sphere(1.0, 8, 12).triangle_count(), 8 * 12 * 2);
        assert_eq!(cylinder(1.0, 1.0, 12).triangle_count(), 48);
        assert_eq!(torus(1.0, 0.2, 12, 8).triangle_count(), 12 * 8 * 2);
        assert!(approx_eq(1.0, 1.0, 1e-6));
    }
}
