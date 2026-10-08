//! Font glyph atlases and text layout.
//!
//! [`FontAtlas::bake`] rasterizes a font file's glyphs into a single coverage
//! [`Texture`] at a fixed pixel size, recording each glyph's metrics (advance,
//! bitmap offset, size, and atlas UVs). [`FontAtlas::layout`] then turns a string
//! into per-glyph placements at that size; the caller scales them to the wanted
//! size and builds quads.
//!
//! Coverage is stored in the atlas texture's alpha channel (RGB is white), so the
//! shader samples `.a` and multiplies by the text colour.

use std::collections::BTreeMap;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use ecs::assets::{Texture, TextureFormat};
use ecs::components::TextAnchor;
use math::Vec3;

/// The printable ASCII set a default atlas covers (space through `~`).
pub const DEFAULT_CHARS: &str = " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~";

/// The default pixel size an atlas is baked at; text of another size is scaled
/// from it.
pub const DEFAULT_PX_SIZE: f32 = 48.0;

/// Anything that can go wrong baking a font.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextError {
    /// The bytes are not a font the rasterizer understands.
    InvalidFont(String),
    /// Nothing could be rasterized (an empty charset, or a font with no outlines).
    Empty,
}

impl std::fmt::Display for TextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TextError::InvalidFont(message) => write!(f, "invalid font: {message}"),
            TextError::Empty => write!(f, "font produced no glyphs"),
        }
    }
}

impl std::error::Error for TextError {}

/// A GPU text vertex: a world-space glyph-quad corner with its atlas UV and
/// colour. Byte-identical to the vertex layout in `text.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable, PartialEq)]
pub struct TextVertex {
    /// World-space position.
    pub position: [f32; 3],
    /// Atlas UV.
    pub uv: [f32; 2],
    /// Linear RGBA colour.
    pub color: [f32; 4],
}

impl TextVertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x4];

    /// The vertex buffer layout for text vertices.
    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<TextVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// The pixel offset from a text block's top-left corner to its anchor point,
/// given the block size (from [`FontAtlas::measure`]).
pub fn anchor_offset(anchor: TextAnchor, size: [f32; 2]) -> [f32; 2] {
    let [w, h] = size;
    match anchor {
        TextAnchor::Center => [w * 0.5, h * 0.5],
        TextAnchor::TopLeft => [0.0, 0.0],
        TextAnchor::TopCenter => [w * 0.5, 0.0],
        TextAnchor::TopRight => [w, 0.0],
        TextAnchor::MiddleLeft => [0.0, h * 0.5],
        TextAnchor::MiddleRight => [w, h * 0.5],
        TextAnchor::BottomLeft => [0.0, h],
        TextAnchor::BottomCenter => [w * 0.5, h],
        TextAnchor::BottomRight => [w, h],
    }
}

/// Build the world-space glyph quads for `text`, billboarded by `right`/`up`.
///
/// `world_height` is the height of one line in world units; the atlas is scaled
/// from its baked pixel size. The anchor point is placed at `origin`.
#[allow(clippy::too_many_arguments)]
pub fn build_vertices(
    atlas: &FontAtlas,
    text: &str,
    anchor: TextAnchor,
    origin: Vec3,
    right: Vec3,
    up: Vec3,
    world_height: f32,
    color: [f32; 4],
) -> Vec<TextVertex> {
    let scale = world_height / atlas.line_height.max(1e-6);
    let [block_w, block_h] = atlas.measure(text);
    let [ax, ay] = anchor_offset(anchor, [block_w, block_h]);

    // Map a pixel-space point measured from the block's top-left, y down, to
    // world space: right along +x, and up along -y.
    let to_world = |px: f32, py: f32| -> [f32; 3] {
        let local = right * ((px - ax) * scale) - up * ((py - ay) * scale);
        let p = origin + local;
        [p.x, p.y, p.z]
    };

    let mut out = Vec::new();
    for place in atlas.layout(text) {
        // The layout origin is the baseline; the block's top is `ascent` above it.
        let x0 = place.offset[0];
        let y0 = place.offset[1] + atlas.ascent;
        let x1 = x0 + place.size[0];
        let y1 = y0 + place.size[1];
        let [u0, v0, u1, v1] = place.uv;

        let tl = to_world(x0, y0);
        let tr = to_world(x1, y0);
        let bl = to_world(x0, y1);
        let br = to_world(x1, y1);
        let v = |position: [f32; 3], uv: [f32; 2]| TextVertex {
            position,
            uv,
            color,
        };
        out.push(v(tl, [u0, v0]));
        out.push(v(bl, [u0, v1]));
        out.push(v(tr, [u1, v0]));
        out.push(v(tr, [u1, v0]));
        out.push(v(bl, [u0, v1]));
        out.push(v(br, [u1, v1]));
    }
    out
}

/// One glyph in an atlas: where its coverage lives and how it is placed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    /// The atlas UV rect `[u0, v0, u1, v1]` of the glyph's bitmap.
    pub uv: [f32; 4],
    /// The bitmap's offset from the pen (baseline) position, in pixels, y down.
    pub offset: [f32; 2],
    /// The bitmap's size in pixels (zero for a blank glyph such as a space).
    pub size: [f32; 2],
    /// The horizontal advance to the next glyph, in pixels.
    pub advance: f32,
}

/// A glyph placed by [`FontAtlas::layout`], relative to the text origin (the
/// baseline start), in pixels at [`FontAtlas::px_size`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// The glyph.
    pub ch: char,
    /// The bitmap's top-left corner, relative to the origin.
    pub offset: [f32; 2],
    /// The bitmap's size in pixels.
    pub size: [f32; 2],
    /// The atlas UVs.
    pub uv: [f32; 4],
}

/// A single-channel glyph atlas baked from a font.
#[derive(Clone, Debug)]
pub struct FontAtlas {
    /// The coverage atlas (white RGB, coverage in alpha; `Rgba8Unorm`).
    pub texture: Texture,
    /// The pixel size the glyphs were rasterized at.
    pub px_size: f32,
    /// Distance from the baseline to the top of a line, in pixels.
    pub ascent: f32,
    /// Distance from the baseline to the bottom of a line, in pixels (positive).
    pub descent: f32,
    /// Baseline-to-baseline distance, in pixels.
    pub line_height: f32,
    /// The glyphs by character.
    pub glyphs: BTreeMap<char, Glyph>,
}

impl FontAtlas {
    /// Rasterize `chars` from `font_data` at `px_size` into a coverage atlas.
    pub fn bake(font_data: &[u8], px_size: f32, chars: &[char]) -> Result<FontAtlas, TextError> {
        let font = FontRef::try_from_slice(font_data)
            .map_err(|error| TextError::InvalidFont(error.to_string()))?;
        let px_size = px_size.max(1.0);
        let scale = PxScale::from(px_size);
        let scaled = font.as_scaled(scale);

        let ascent = scaled.ascent().max(0.0);
        let descent = (-scaled.descent()).max(0.0);
        let line_height = scaled.height().max(ascent + descent).max(1.0);
        // A cell tall enough for any glyph, with one pixel of padding all round.
        let cell_height = (ascent + descent).ceil() as u32 + 2;
        let atlas_width = 512u32;

        // Rasterize every glyph's coverage into a tight bitmap.
        struct Raster {
            ch: char,
            width: u32,
            height: u32,
            pixels: Vec<u8>,
            advance: f32,
            offset: [f32; 2],
        }
        let mut rasters = Vec::with_capacity(chars.len());
        for &ch in chars {
            let id = font.glyph_id(ch);
            let advance = scaled.h_advance(id);
            let glyph = id.with_scale_and_position(scale, point(0.0, 0.0));
            let mut width = 0u32;
            let mut height = 0u32;
            let mut pixels = Vec::new();
            let mut offset = [0.0, 0.0];
            if let Some(outlined) = font.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                offset = [bounds.min.x, bounds.min.y];
                width = (bounds.max.x - bounds.min.x).ceil().max(0.0) as u32;
                height = (bounds.max.y - bounds.min.y).ceil().max(0.0) as u32;
                if width > 0 && height > 0 {
                    pixels = vec![0u8; (width * height) as usize];
                    outlined.draw(|x, y, coverage| {
                        let index = (y * width + x) as usize;
                        if index < pixels.len() {
                            pixels[index] = (coverage.clamp(0.0, 1.0) * 255.0).round() as u8;
                        }
                    });
                }
            }
            rasters.push(Raster {
                ch,
                width,
                height,
                pixels,
                advance,
                offset,
            });
        }
        if rasters.is_empty() {
            return Err(TextError::Empty);
        }

        // Shelf-pack the bitmaps into rows of `atlas_width`.
        let mut x = 0u32;
        let mut rows = 1u32;
        let mut placements: Vec<(u32, u32)> = Vec::with_capacity(rasters.len());
        for raster in &rasters {
            if x + raster.width + 2 > atlas_width {
                x = 0;
                rows += 1;
            }
            placements.push((x + 1, (rows - 1) * cell_height + 1));
            x += raster.width + 2;
        }
        let atlas_height = rows * cell_height;
        let mut data = vec![0u8; (atlas_width * atlas_height * 4) as usize];
        // Fill RGB with white so the shader can multiply the coverage by a colour.
        for pixel in data.as_chunks_mut::<4>().0 {
            pixel[0] = 255;
            pixel[1] = 255;
            pixel[2] = 255;
            pixel[3] = 0;
        }

        let mut glyphs = BTreeMap::new();
        for (raster, (px, py)) in rasters.iter().zip(&placements) {
            for row in 0..raster.height {
                for col in 0..raster.width {
                    let coverage = raster.pixels[(row * raster.width + col) as usize];
                    if coverage == 0 {
                        continue;
                    }
                    let ax = px + col;
                    let ay = py + row;
                    let index = ((ay * atlas_width + ax) * 4 + 3) as usize;
                    data[index] = coverage;
                }
            }
            let u0 = *px as f32 / atlas_width as f32;
            let v0 = *py as f32 / atlas_height as f32;
            let u1 = (*px + raster.width) as f32 / atlas_width as f32;
            let v1 = (*py + raster.height) as f32 / atlas_height as f32;
            glyphs.insert(
                raster.ch,
                Glyph {
                    uv: [u0, v0, u1, v1],
                    offset: raster.offset,
                    size: [raster.width as f32, raster.height as f32],
                    advance: raster.advance,
                },
            );
        }

        Ok(FontAtlas {
            texture: Texture {
                name: "font-atlas".to_string(),
                width: atlas_width,
                height: atlas_height,
                format: TextureFormat::Rgba8Unorm,
                data,
            },
            px_size,
            ascent,
            descent,
            line_height,
            glyphs,
        })
    }

    /// The glyph for `ch`, falling back to a space's advance when absent.
    pub fn glyph(&self, ch: char) -> Glyph {
        self.glyphs.get(&ch).copied().unwrap_or(Glyph {
            uv: [0.0; 4],
            offset: [0.0, 0.0],
            size: [0.0, 0.0],
            advance: self.glyphs.get(&' ').map_or(0.0, |g| g.advance),
        })
    }

    /// The size of `text` laid out at [`px_size`](Self::px_size), as
    /// `[width, height]` in pixels (height is the line count times the line
    /// height).
    pub fn measure(&self, text: &str) -> [f32; 2] {
        let mut widest = 0.0f32;
        let mut pen = 0.0f32;
        let mut lines = 1u32;
        for ch in text.chars() {
            if ch == '\n' {
                widest = widest.max(pen);
                pen = 0.0;
                lines += 1;
            } else {
                pen += self.glyph(ch).advance;
            }
        }
        widest = widest.max(pen);
        [widest, lines as f32 * self.line_height]
    }

    /// Lay `text` out from the baseline origin `(0, 0)`, y down, returning one
    /// [`Placement`] per non-blank glyph.
    pub fn layout(&self, text: &str) -> Vec<Placement> {
        let mut out = Vec::new();
        let mut pen = 0.0f32;
        let mut baseline = 0.0f32;
        for ch in text.chars() {
            if ch == '\n' {
                pen = 0.0;
                baseline += self.line_height;
                continue;
            }
            let glyph = self.glyph(ch);
            if glyph.size[0] > 0.0 && glyph.size[1] > 0.0 {
                out.push(Placement {
                    ch,
                    offset: [pen + glyph.offset[0], baseline + glyph.offset[1]],
                    size: glyph.size,
                    uv: glyph.uv,
                });
            }
            pen += glyph.advance;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny embedded font is not practical, so the tests that need real
    /// outlines are ignored when no font file is available. The layout logic is
    /// tested with a hand-built atlas.
    fn synthetic_atlas() -> FontAtlas {
        // Two 4x4 glyph cells ('A' at u 0..0.25, 'B' at u 0.25..0.5) in an 8x4
        // white/alpha texture, with simple metrics.
        let mut glyphs = BTreeMap::new();
        glyphs.insert(
            'A',
            Glyph {
                uv: [0.0, 0.0, 0.25, 1.0],
                offset: [0.0, 0.0],
                size: [4.0, 4.0],
                advance: 5.0,
            },
        );
        glyphs.insert(
            'B',
            Glyph {
                uv: [0.25, 0.0, 0.5, 1.0],
                offset: [1.0, 0.0],
                size: [4.0, 4.0],
                advance: 6.0,
            },
        );
        glyphs.insert(
            ' ',
            Glyph {
                uv: [0.0; 4],
                offset: [0.0, 0.0],
                size: [0.0, 0.0],
                advance: 2.0,
            },
        );
        FontAtlas {
            texture: Texture {
                name: "synthetic".to_string(),
                width: 8,
                height: 4,
                format: TextureFormat::Rgba8Unorm,
                data: vec![255; 8 * 4 * 4],
            },
            px_size: 4.0,
            ascent: 4.0,
            descent: 0.0,
            line_height: 6.0,
            glyphs,
        }
    }

    #[test]
    fn layout_advances_and_places_glyphs() {
        let atlas = synthetic_atlas();
        let places = atlas.layout("AB");
        assert_eq!(places.len(), 2);
        assert_eq!(places[0].ch, 'A');
        assert_eq!(places[0].offset, [0.0, 0.0]);
        assert_eq!(places[0].size, [4.0, 4.0]);
        // 'B' starts after 'A's advance, plus its bearing.
        assert_eq!(places[1].ch, 'B');
        assert_eq!(places[1].offset, [5.0 + 1.0, 0.0]);
    }

    #[test]
    fn spaces_advance_without_a_quad() {
        let atlas = synthetic_atlas();
        let places = atlas.layout("A B");
        assert_eq!(places.len(), 2);
        assert_eq!(places[1].offset[0], 5.0 + 2.0 + 1.0);
    }

    #[test]
    fn newlines_start_a_new_baseline() {
        let atlas = synthetic_atlas();
        let places = atlas.layout("A\nB");
        assert_eq!(places.len(), 2);
        assert_eq!(places[1].offset, [1.0, 6.0]);
    }

    #[test]
    fn measure_reports_the_widest_line_and_the_line_count() {
        let atlas = synthetic_atlas();
        assert_eq!(atlas.measure("AB"), [11.0, 6.0]);
        assert_eq!(atlas.measure("A\nABB"), [5.0 + 6.0 + 6.0, 12.0]);
        assert_eq!(atlas.measure(""), [0.0, 6.0]);
    }

    #[test]
    fn an_unknown_character_uses_a_space_advance() {
        let atlas = synthetic_atlas();
        let glyph = atlas.glyph('Z');
        assert_eq!(glyph.size, [0.0, 0.0]);
        assert_eq!(glyph.advance, 2.0);
    }

    #[test]
    fn an_invalid_font_is_an_error() {
        let error = FontAtlas::bake(b"not a font", 16.0, &['A']).unwrap_err();
        assert!(matches!(error, TextError::InvalidFont(_)));
    }

    #[test]
    fn anchor_offsets_pick_the_right_corner() {
        let size = [10.0, 4.0];
        assert_eq!(anchor_offset(TextAnchor::TopLeft, size), [0.0, 0.0]);
        assert_eq!(anchor_offset(TextAnchor::Center, size), [5.0, 2.0]);
        assert_eq!(anchor_offset(TextAnchor::TopRight, size), [10.0, 0.0]);
        assert_eq!(anchor_offset(TextAnchor::BottomLeft, size), [0.0, 4.0]);
        assert_eq!(anchor_offset(TextAnchor::BottomRight, size), [10.0, 4.0]);
    }

    #[test]
    fn build_vertices_emits_a_quad_per_glyph() {
        let atlas = synthetic_atlas();
        let right = math::vec3(1.0, 0.0, 0.0);
        let up = math::vec3(0.0, 1.0, 0.0);
        let origin = math::vec3(0.0, 0.0, 0.0);
        // One line-height of world height means one world unit per pixel.
        let verts = build_vertices(
            &atlas,
            "A B",
            TextAnchor::Center,
            origin,
            right,
            up,
            atlas.line_height,
            [1.0, 1.0, 1.0, 1.0],
        );
        // Two glyphs ('A' and 'B'), six vertices each.
        assert_eq!(verts.len(), 12);
        // Billboarded in the XY plane.
        assert!(verts.iter().all(|v| v.position[2] == 0.0));
        // The quad carries the glyph's atlas UVs.
        assert!(verts.iter().any(|v| v.uv == [0.0, 0.0]));
        assert!(verts.iter().any(|v| v.uv == [0.25, 1.0]));

        // Centring shifts the block by half its measured size.
        let measure = atlas.measure("A B");
        let top_left = build_vertices(
            &atlas,
            "A B",
            TextAnchor::TopLeft,
            origin,
            right,
            up,
            atlas.line_height,
            [1.0; 4],
        );
        let min_x = |v: &[TextVertex]| {
            v.iter()
                .map(|t| t.position[0])
                .fold(f32::INFINITY, f32::min)
        };
        let min_y = |v: &[TextVertex]| {
            v.iter()
                .map(|t| t.position[1])
                .fold(f32::INFINITY, f32::min)
        };
        assert!((min_x(&verts) - (min_x(&top_left) - measure[0] * 0.5)).abs() < 1e-4);
        // World y is flipped relative to the block's y-down pixels, so the
        // vertical shift has the opposite sign.
        assert!((min_y(&verts) - (min_y(&top_left) + measure[1] * 0.5)).abs() < 1e-4);
    }

    #[test]
    fn build_vertices_scales_with_world_height() {
        let atlas = synthetic_atlas();
        let right = math::vec3(1.0, 0.0, 0.0);
        let up = math::vec3(0.0, 1.0, 0.0);
        let extent = |height: f32| {
            let verts = build_vertices(
                &atlas,
                "A",
                TextAnchor::TopLeft,
                math::vec3(0.0, 0.0, 0.0),
                right,
                up,
                height,
                [1.0; 4],
            );
            let xs: Vec<f32> = verts.iter().map(|v| v.position[0]).collect();
            let ys: Vec<f32> = verts.iter().map(|v| v.position[1]).collect();
            let w = xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
                - xs.iter().cloned().fold(f32::INFINITY, f32::min);
            let h = ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
                - ys.iter().cloned().fold(f32::INFINITY, f32::min);
            [w, h]
        };
        let one = extent(atlas.line_height);
        let two = extent(atlas.line_height * 2.0);
        assert!((two[0] - one[0] * 2.0).abs() < 1e-4);
        assert!((two[1] - one[1] * 2.0).abs() < 1e-4);
    }

    /// A real font to bake, if the host has one. Skips (returns `None`) when no
    /// common font file is present, so the test is portable.
    fn system_font() -> Option<Vec<u8>> {
        const CANDIDATES: &[&str] = &[
            "C:\\Windows\\Fonts\\arial.ttf",
            "C:\\Windows\\Fonts\\segoeui.ttf",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        ];
        CANDIDATES.iter().find_map(|path| std::fs::read(path).ok())
    }

    #[test]
    fn baking_a_real_font_produces_glyphs_and_a_usable_atlas() {
        let Some(data) = system_font() else {
            return; // no font on this host; skip
        };
        let atlas = FontAtlas::bake(&data, 32.0, &['A', 'g', ' ', 'W']).expect("bake");
        assert!(atlas.line_height > 0.0);
        assert!(atlas.ascent > 0.0);
        // A, g, and W have coverage; the space does not.
        assert!(atlas.glyph('A').size[0] > 0.0 && atlas.glyph('A').size[1] > 0.0);
        assert!(atlas.glyph('W').advance > atlas.glyph('A').advance * 0.5);
        assert_eq!(atlas.glyph(' ').size, [0.0, 0.0]);
        // The atlas has opaque coverage somewhere.
        assert!(
            atlas
                .texture
                .data
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[3] > 0)
        );
        // Laying out real text yields quads inside the measured box.
        let places = atlas.layout("Ag");
        assert_eq!(places.len(), 2);
        let [width, height] = atlas.measure("Ag");
        assert!(width > 0.0 && height > 0.0);
        for place in &places {
            assert!(place.offset[0] >= -atlas.px_size);
            assert!(place.offset[1] - place.size[1] <= height);
        }
    }
}
