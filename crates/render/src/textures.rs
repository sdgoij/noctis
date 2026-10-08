//! Loading standalone image files into [`Texture`] assets.
//!
//! The renderer's glTF import decodes images embedded in a document; this module
//! is the equivalent for a loose `.png`/`.jpg`/`.bmp`/`.tga`/`.hdr` file the user
//! picks in the editor.

use std::path::Path;

use ecs::assets::{Texture, TextureFormat};

use crate::error::RenderError;

/// Load an image file into an RGBA8 texture.
///
/// `srgb` selects the sRGB color format for color maps (albedo, emissive); pass
/// `false` for data maps (normal, metallic/roughness, occlusion) so the GPU reads
/// their texels linearly.
pub fn load_texture(path: impl AsRef<Path>, srgb: bool) -> Result<Texture, RenderError> {
    let path = path.as_ref();
    let decoded = image::open(path).map_err(|e| RenderError::Image(e.to_string()))?;
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    let name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("texture")
        .to_string();
    Ok(Texture {
        name,
        width,
        height,
        format: if srgb {
            TextureFormat::Rgba8Srgb
        } else {
            TextureFormat::Rgba8Unorm
        },
        data: rgba.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny RGBA PNG written to a unique temp path.
    fn write_png(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("noctis-texture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(name);
        let mut image = image::RgbaImage::new(3, 2);
        image.put_pixel(0, 0, image::Rgba([10, 20, 30, 255]));
        image.save(&path).expect("save png");
        path
    }

    #[test]
    fn loads_a_png_into_an_rgba_texture() {
        let path = write_png("color.png");
        let texture = load_texture(&path, true).expect("load");
        assert_eq!((texture.width, texture.height), (3, 2));
        assert_eq!(texture.format, TextureFormat::Rgba8Srgb);
        assert_eq!(texture.data.len(), 3 * 2 * 4);
        assert_eq!(texture.name, "color");
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn a_data_texture_is_linear() {
        let path = write_png("normal.png");
        let texture = load_texture(&path, false).expect("load");
        assert_eq!(texture.format, TextureFormat::Rgba8Unorm);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn a_missing_file_is_an_image_error() {
        let err = load_texture("does-not-exist.png", true).unwrap_err();
        assert!(matches!(err, RenderError::Image(_)), "{err:?}");
    }
}
