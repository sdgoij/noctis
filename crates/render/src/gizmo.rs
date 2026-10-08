//! Editor gizmo geometry.
//!
//! The gizmo is generated purely on the CPU as two draw lists:
//!
//! * `lines` — a `LineList` of coloured segments (axis shafts and rotation
//!   rings), and
//! * `triangles` + `triangle_indices` — an `TriangleList` of arrowheads and
//!   scale cubes.
//!
//! Both share the same [`GizmoVertex`] layout so the renderer can reuse one
//! pipeline (with two topologies) or draw them with a single pass. Axes are
//! coloured `+X` red, `+Y` green, `+Z` blue; the hovered axis turns yellow.

use bytemuck::{Pod, Zeroable};
use math::{Vec3, vec3};

/// Which transform tool the gizmo draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GizmoMode {
    /// Three arrows for translation.
    #[default]
    Translate,
    /// Three rings for rotation.
    Rotate,
    /// Three boxed shafts for scaling.
    Scale,
}

/// A gizmo axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoAxis {
    /// The X axis.
    X,
    /// The Y axis.
    Y,
    /// The Z axis.
    Z,
}

/// A colored, unlit vertex used by every gizmo shape.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct GizmoVertex {
    /// Object-space position.
    pub position: [f32; 3],
    /// Linear RGBA color.
    pub color: [f32; 4],
}

impl GizmoVertex {
    /// Construct a vertex.
    pub fn new(position: Vec3, color: [f32; 4]) -> Self {
        GizmoVertex {
            position: [position.x, position.y, position.z],
            color,
        }
    }
}

/// A generated gizmo: line segments and a triangle mesh.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GizmoGeometry {
    /// Vertices for the `LineList` pass.
    pub lines: Vec<GizmoVertex>,
    /// Vertices for the `TriangleList` pass.
    pub triangles: Vec<GizmoVertex>,
    /// Indices into `triangles`.
    pub triangle_indices: Vec<u32>,
}

impl GizmoGeometry {
    /// The number of line segments.
    pub fn line_segment_count(&self) -> usize {
        self.lines.len() / 2
    }

    /// The number of triangles.
    pub fn triangle_count(&self) -> usize {
        self.triangle_indices.len() / 3
    }
}

/// The unit direction of an axis.
pub fn axis_vector(axis: GizmoAxis) -> Vec3 {
    match axis {
        GizmoAxis::X => vec3(1.0, 0.0, 0.0),
        GizmoAxis::Y => vec3(0.0, 1.0, 0.0),
        GizmoAxis::Z => vec3(0.0, 0.0, 1.0),
    }
}

/// The base color of an axis (`X` red, `Y` green, `Z` blue).
pub fn axis_color(axis: GizmoAxis) -> [f32; 4] {
    match axis {
        GizmoAxis::X => [0.9, 0.15, 0.15, 1.0],
        GizmoAxis::Y => [0.2, 0.85, 0.2, 1.0],
        GizmoAxis::Z => [0.25, 0.4, 0.95, 1.0],
    }
}

/// The highlight color used for the hovered axis.
pub const HOVER_COLOR: [f32; 4] = [1.0, 0.9, 0.1, 1.0];

fn color_for(axis: GizmoAxis, hovered: Option<GizmoAxis>) -> [f32; 4] {
    if hovered == Some(axis) {
        HOVER_COLOR
    } else {
        axis_color(axis)
    }
}

fn normalize(v: Vec3) -> Vec3 {
    let len_sq = v.x * v.x + v.y * v.y + v.z * v.z;
    if len_sq > 1e-20 {
        let inv = len_sq.sqrt().recip();
        vec3(v.x * inv, v.y * inv, v.z * inv)
    } else {
        vec3(0.0, 0.0, 1.0)
    }
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    vec3(
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    )
}

/// Two unit vectors spanning the plane perpendicular to `axis`.
///
/// This is the ring basis used both to draw rotation rings and to measure a
/// drag angle around an axis, so the two must agree.
pub fn basis_for_axis(axis: GizmoAxis) -> (Vec3, Vec3) {
    match axis {
        GizmoAxis::X => (vec3(0.0, 1.0, 0.0), vec3(0.0, 0.0, 1.0)),
        GizmoAxis::Y => (vec3(0.0, 0.0, 1.0), vec3(1.0, 0.0, 0.0)),
        GizmoAxis::Z => (vec3(1.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0)),
    }
}

fn push_line(lines: &mut Vec<GizmoVertex>, a: Vec3, b: Vec3, color: [f32; 4]) {
    lines.push(GizmoVertex::new(a, color));
    lines.push(GizmoVertex::new(b, color));
}

fn push_circle(
    lines: &mut Vec<GizmoVertex>,
    u: Vec3,
    v: Vec3,
    radius: f32,
    segments: u32,
    color: [f32; 4],
) {
    let segments = segments.max(3);
    let mut prev = u * radius;
    for i in 1..=segments {
        let angle = std::f32::consts::TAU * i as f32 / segments as f32;
        let (s, c) = angle.sin_cos();
        let p = u * (radius * c) + v * (radius * s);
        push_line(lines, prev, p, color);
        prev = p;
    }
}

fn push_cone(
    triangles: &mut Vec<GizmoVertex>,
    indices: &mut Vec<u32>,
    base: Vec3,
    u: Vec3,
    v: Vec3,
    radius: f32,
    height: f32,
    segments: u32,
    color: [f32; 4],
) {
    let segments = segments.max(3);
    // The shaft direction is orthogonal to the base plane spanned by `u`, `v`.
    let axis = normalize(cross(u, v));
    let tip = base + axis * height;
    let base_index = triangles.len() as u32;
    triangles.push(GizmoVertex::new(tip, color));
    for i in 0..=segments {
        let angle = std::f32::consts::TAU * i as f32 / segments as f32;
        let (s, c) = angle.sin_cos();
        let p = base + u * (radius * c) + v * (radius * s);
        triangles.push(GizmoVertex::new(p, color));
    }
    for i in 0..segments {
        indices.push(base_index);
        indices.push(base_index + 1 + i);
        indices.push(base_index + 2 + i);
    }
}

fn push_cube(
    triangles: &mut Vec<GizmoVertex>,
    indices: &mut Vec<u32>,
    center: Vec3,
    half: f32,
    color: [f32; 4],
) {
    let base = triangles.len() as u32;
    let corners = [
        vec3(-1.0, -1.0, -1.0),
        vec3(1.0, -1.0, -1.0),
        vec3(1.0, 1.0, -1.0),
        vec3(-1.0, 1.0, -1.0),
        vec3(-1.0, -1.0, 1.0),
        vec3(1.0, -1.0, 1.0),
        vec3(1.0, 1.0, 1.0),
        vec3(-1.0, 1.0, 1.0),
    ];
    for c in corners {
        triangles.push(GizmoVertex::new(center + c * half, color));
    }
    const FACES: [[u32; 6]; 6] = [
        [0, 1, 2, 0, 2, 3], // -Z
        [4, 6, 5, 4, 7, 6], // +Z
        [0, 4, 5, 0, 5, 1], // -Y
        [3, 2, 6, 3, 6, 7], // +Y
        [0, 3, 7, 0, 7, 4], // -X
        [1, 5, 6, 1, 6, 2], // +X
    ];
    for face in FACES {
        for i in face {
            indices.push(base + i);
        }
    }
}

/// Build a gizmo of the given `mode` with shafts `length` long.
///
/// `hovered` highlights one axis. The returned geometry is centred on the
/// origin; the renderer places it with the selected entity's transform.
pub fn build_gizmo(mode: GizmoMode, length: f32, hovered: Option<GizmoAxis>) -> GizmoGeometry {
    let length = length.max(1e-3);
    let mut geometry = GizmoGeometry::default();
    let axes = [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z];
    for axis in axes {
        let color = color_for(axis, hovered);
        let dir = axis_vector(axis);
        let (u, v) = basis_for_axis(axis);
        match mode {
            GizmoMode::Translate => {
                let shaft = length * 0.85;
                push_line(&mut geometry.lines, vec3(0.0, 0.0, 0.0), dir * shaft, color);
                push_cone(
                    &mut geometry.triangles,
                    &mut geometry.triangle_indices,
                    dir * shaft,
                    u,
                    v,
                    length * 0.06,
                    length * 0.15,
                    12,
                    color,
                );
            }
            GizmoMode::Scale => {
                push_line(
                    &mut geometry.lines,
                    vec3(0.0, 0.0, 0.0),
                    dir * length,
                    color,
                );
                push_cube(
                    &mut geometry.triangles,
                    &mut geometry.triangle_indices,
                    dir * length,
                    length * 0.07,
                    color,
                );
            }
            GizmoMode::Rotate => {
                push_circle(&mut geometry.lines, u, v, length, 48, color);
            }
        }
    }
    geometry
}

/// Build a ground grid of `LineList` segments on the XZ plane.
///
/// `half_extent` is the distance from the origin to the edge and `spacing` the
/// distance between lines. The centre lines are tinted; the rest are grey.
pub fn build_grid(half_extent: f32, spacing: f32) -> Vec<GizmoVertex> {
    let mut lines = Vec::new();
    let spacing = spacing.max(1e-3);
    let steps = (half_extent / spacing).floor() as i32;
    let grey = [0.35, 0.35, 0.38, 1.0];
    for i in -steps..=steps {
        let offset = i as f32 * spacing;
        let edge = steps as f32 * spacing;
        let color = if i == 0 { [0.55, 0.55, 0.6, 1.0] } else { grey };
        push_line(
            &mut lines,
            vec3(offset, 0.0, -edge),
            vec3(offset, 0.0, edge),
            color,
        );
        push_line(
            &mut lines,
            vec3(-edge, 0.0, offset),
            vec3(edge, 0.0, offset),
            color,
        );
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translate_has_lines_and_arrowheads() {
        let g = build_gizmo(GizmoMode::Translate, 1.0, None);
        assert!(g.line_segment_count() >= 3);
        assert!(g.triangle_count() >= 3 * 12);
        assert_eq!(g.triangle_indices.len() % 3, 0);
    }

    #[test]
    fn scale_has_cubes() {
        let g = build_gizmo(GizmoMode::Scale, 1.0, None);
        assert_eq!(g.line_segment_count(), 3);
        assert_eq!(g.triangle_count(), 3 * 12);
    }

    #[test]
    fn rotate_has_rings() {
        let g = build_gizmo(GizmoMode::Rotate, 1.0, None);
        // 3 rings * 48 segments.
        assert_eq!(g.line_segment_count(), 3 * 48);
        assert!(g.triangles.is_empty());
    }

    #[test]
    fn all_vertices_are_finite() {
        for mode in [GizmoMode::Translate, GizmoMode::Rotate, GizmoMode::Scale] {
            let g = build_gizmo(mode, 2.5, Some(GizmoAxis::Y));
            for v in g.lines.iter().chain(g.triangles.iter()) {
                assert!(v.position.iter().all(|c| c.is_finite()));
            }
        }
    }

    #[test]
    fn hovered_axis_is_yellow() {
        let g = build_gizmo(GizmoMode::Translate, 1.0, Some(GizmoAxis::X));
        let has_hover = g.lines.iter().any(|v| v.color == HOVER_COLOR);
        assert!(has_hover, "hovered axis should be highlighted");
        // The other two axes keep their base colors.
        assert!(g.lines.iter().any(|v| v.color == axis_color(GizmoAxis::Y)));
    }

    #[test]
    fn indices_are_in_range() {
        let g = build_gizmo(GizmoMode::Translate, 1.0, None);
        for &i in &g.triangle_indices {
            assert!((i as usize) < g.triangles.len());
        }
    }

    #[test]
    fn translate_arrows_reach_along_axes() {
        let g = build_gizmo(GizmoMode::Translate, 4.0, None);
        let max_x = g
            .lines
            .iter()
            .map(|v| v.position[0])
            .fold(f32::MIN, f32::max);
        let max_y = g
            .lines
            .iter()
            .map(|v| v.position[1])
            .fold(f32::MIN, f32::max);
        assert!(max_x > 3.0 && max_y > 3.0);
    }

    #[test]
    fn grid_is_square_and_centred() {
        let lines = build_grid(10.0, 1.0);
        assert!(!lines.is_empty());
        assert_eq!(lines.len() % 2, 0);
        assert_eq!(lines.len(), (21 * 2) * 2);
    }

    #[test]
    fn vertex_layout_is_gpu_friendly() {
        assert_eq!(std::mem::size_of::<GizmoVertex>(), 28);
    }
}
