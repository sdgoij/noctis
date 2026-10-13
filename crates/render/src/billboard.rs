//! Camera-facing sprite quads (`.notes/billboards.md`).
//!
//! A [`Billboard`](ecs::components::Billboard) is drawn as one textured quad that
//! always faces the camera. [`build_vertices`] is a pure function that turns a
//! billboard and the camera basis into its six vertices (two triangles), so the
//! geometry is unit-tested without a GPU — the same shape as
//! [`crate::text::build_vertices`].

use ecs::components::Billboard;
use math::Vec3;

/// A GPU billboard vertex: a world-space sprite-quad corner with its texture UV
/// and colour. Byte-identical to the vertex layout in `billboard.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable, PartialEq)]
pub struct BillboardVertex {
    /// World-space position.
    pub position: [f32; 3],
    /// Texture UV.
    pub uv: [f32; 2],
    /// Linear RGBA colour.
    pub color: [f32; 4],
}

impl BillboardVertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x4];

    /// The vertex buffer layout for billboard vertices.
    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<BillboardVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// Build the four corners (six vertices, two triangles) of `billboard`, centred
/// at `origin` and billboarded in the camera plane spanned by `right`/`up`, with
/// the billboard's in-plane `rotation` applied.
///
/// `right` and `up` are the camera basis vectors (unit); the quad's local `+Y` is
/// `up`, so a sprite's texture `v` runs bottom-to-top and an unrotated quad reads
/// upright.
pub fn build_vertices(
    billboard: &Billboard,
    origin: Vec3,
    right: Vec3,
    up: Vec3,
) -> Vec<BillboardVertex> {
    let [u0, v0, u1, v1] = billboard.uv;
    let half = billboard.size * 0.5;
    // The quad's own axes: the camera basis rotated in-plane by `rotation`.
    let (s, c) = billboard.rotation.sin_cos();
    let ax = right * c + up * s;
    let ay = up * c - right * s;
    let corner = |sx: f32, sy: f32| -> [f32; 3] {
        let p = origin + ax * (sx * half.x) + ay * (sy * half.y);
        [p.x, p.y, p.z]
    };
    let vertex = |position: [f32; 3], uv: [f32; 2]| BillboardVertex {
        position,
        uv,
        color: billboard.color,
    };
    let tl = corner(-1.0, 1.0);
    let tr = corner(1.0, 1.0);
    let bl = corner(-1.0, -1.0);
    let br = corner(1.0, -1.0);
    vec![
        vertex(tl, [u0, v0]),
        vertex(bl, [u0, v1]),
        vertex(tr, [u1, v0]),
        vertex(tr, [u1, v0]),
        vertex(bl, [u0, v1]),
        vertex(br, [u1, v1]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use math::vec3;

    fn billboard(size: (f32, f32)) -> Billboard {
        Billboard {
            size: math::vec2(size.0, size.1),
            color: [1.0, 0.5, 0.25, 0.75],
            ..Billboard::default()
        }
    }

    #[test]
    fn emits_six_vertices_in_two_triangles() {
        let v = build_vertices(
            &billboard((1.0, 1.0)),
            vec3(0.0, 0.0, 0.0),
            vec3(1.0, 0.0, 0.0),
            vec3(0.0, 1.0, 0.0),
        );
        assert_eq!(v.len(), 6);
        // The second triangle shares its first two corners with the first.
        assert_eq!(v[2], v[3]);
        assert_eq!(v[1], v[4]);
    }

    #[test]
    fn centres_on_the_origin_and_spans_the_size() {
        let v = build_vertices(
            &billboard((2.0, 4.0)),
            vec3(1.0, 2.0, 3.0),
            vec3(1.0, 0.0, 0.0),
            vec3(0.0, 1.0, 0.0),
        );
        let xs: Vec<f32> = v.iter().map(|p| p.position[0]).collect();
        let ys: Vec<f32> = v.iter().map(|p| p.position[1]).collect();
        let min_x = xs.iter().copied().fold(f32::INFINITY, f32::min);
        let max_x = xs.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let min_y = ys.iter().copied().fold(f32::INFINITY, f32::min);
        let max_y = ys.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert!(
            (min_x - 0.0).abs() < 1e-6 && (max_x - 2.0).abs() < 1e-6,
            "x spans the size"
        );
        assert!(
            (min_y - 0.0).abs() < 1e-6 && (max_y - 4.0).abs() < 1e-6,
            "y spans the size"
        );
        // Every corner is in the camera plane (z is the origin's, here 3).
        assert!(v.iter().all(|p| (p.position[2] - 3.0).abs() < 1e-6));
    }

    #[test]
    fn carries_the_atlas_uv_and_the_tint() {
        let b = Billboard {
            uv: [0.25, 0.5, 0.75, 1.0],
            ..billboard((1.0, 1.0))
        };
        let v = build_vertices(
            &b,
            vec3(0.0, 0.0, 0.0),
            vec3(1.0, 0.0, 0.0),
            vec3(0.0, 1.0, 0.0),
        );
        assert!(
            v.iter().all(|p| p.color == b.color),
            "the tint is on every vertex"
        );
        assert!(
            v.iter().any(|p| p.uv == [0.25, 0.5]),
            "the sub-rect's min corner"
        );
        assert!(v.iter().any(|p| p.uv == [0.75, 1.0]), "and its max corner");
    }

    #[test]
    fn rotation_spins_in_the_camera_plane() {
        // A horizontal quad (2 wide, 0 tall) turned 90° becomes vertical.
        let b = Billboard {
            size: math::vec2(2.0, 0.0),
            rotation: std::f32::consts::FRAC_PI_2,
            ..Billboard::default()
        };
        let v = build_vertices(
            &b,
            vec3(0.0, 0.0, 0.0),
            vec3(1.0, 0.0, 0.0),
            vec3(0.0, 1.0, 0.0),
        );
        let max_y = v.iter().map(|p| p.position[1].abs()).fold(0.0, f32::max);
        let max_x = v.iter().map(|p| p.position[0].abs()).fold(0.0, f32::max);
        assert!(
            (max_y - 1.0).abs() < 1e-5,
            "the quad now points along up: {max_y}"
        );
        assert!(max_x < 1e-5, "and no longer along right: {max_x}");
    }

    #[test]
    fn a_zero_size_quad_is_degenerate_but_finite() {
        let v = build_vertices(
            &billboard((0.0, 0.0)),
            vec3(0.0, 0.0, 0.0),
            vec3(1.0, 0.0, 0.0),
            vec3(0.0, 1.0, 0.0),
        );
        assert!(v.iter().all(|p| p.position.iter().all(|c| c.is_finite())));
    }
}
