//! Loading standalone image files into [`Texture`] and [`HdrTexture`] assets.
//!
//! The renderer's glTF import decodes images embedded in a document; this module
//! is the equivalent for a loose `.png`/`.jpg`/`.bmp`/`.tga`/`.hdr` file the user
//! picks in the editor, or an environment map a project names in its manifest.

use std::path::Path;

use ecs::assets::{HdrTexture, Texture, TextureFormat};

use crate::error::RenderError;

/// Load an image file into an RGBA8 texture.
///
/// `srgb` selects the sRGB color format for color maps (albedo, emissive); pass
/// `false` for data maps (normal, metallic/roughness, occlusion) so the GPU reads
/// their texels linearly.
pub fn load_texture(path: impl AsRef<Path>, srgb: bool) -> Result<Texture, RenderError> {
    let path = path.as_ref();
    let decoded = image::open(path).map_err(|e| RenderError::Image(e.to_string()))?;
    Ok(texture_from_image(
        file_stem(path, "texture"),
        decoded,
        srgb,
    ))
}

/// Decode in-memory image `bytes` (format guessed from the content) into an RGBA8
/// texture named `name`.
///
/// The byte-based counterpart of [`load_texture`], for a caller that reads the
/// file through a store (a project bundle, browser storage) rather than a path.
pub fn load_texture_bytes(name: &str, bytes: &[u8], srgb: bool) -> Result<Texture, RenderError> {
    Ok(texture_from_image(
        name.to_string(),
        decode_image(bytes)?,
        srgb,
    ))
}

/// Build an RGBA8 [`Texture`] from a decoded image.
fn texture_from_image(name: String, decoded: image::DynamicImage, srgb: bool) -> Texture {
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    Texture {
        name,
        width,
        height,
        format: if srgb {
            TextureFormat::Rgba8Srgb
        } else {
            TextureFormat::Rgba8Unorm
        },
        data: rgba.into_raw(),
    }
}

/// Load an equirectangular Radiance `.hdr` file into a floating-point
/// [`HdrTexture`], ready for [`IblData::generate`](crate::ibl::IblData::generate).
///
/// The samples are linear `f32` RGB, `width * height * 3` of them, in the layout
/// the IBL precomputation expects: row 0 is the **zenith** (see
/// `ibl::dir_to_equirect_uv`), which is exactly how Radiance HDRIs — and the
/// `.hdr` panoramas Poly Haven publishes — are stored.
///
/// Non-finite and negative samples are clamped to zero: an environment is
/// multiplied into every shaded pixel, so one bad texel would otherwise poison
/// the whole frame.
pub fn load_hdr(path: impl AsRef<Path>) -> Result<HdrTexture, RenderError> {
    let path = path.as_ref();
    let name = file_stem(path, "environment");
    let decoded = image::open(path).map_err(|e| RenderError::Image(e.to_string()))?;
    hdr_from_image(name, decoded)
}

/// Decode in-memory Radiance `.hdr` `bytes` (format guessed from the content)
/// into a floating-point [`HdrTexture`] named `name`.
///
/// The byte-based counterpart of [`load_hdr`].
pub fn load_hdr_bytes(name: &str, bytes: &[u8]) -> Result<HdrTexture, RenderError> {
    hdr_from_image(name.to_string(), decode_image(bytes)?)
}

/// Build a linear [`HdrTexture`] from a decoded image, clamping bad samples.
fn hdr_from_image(name: String, decoded: image::DynamicImage) -> Result<HdrTexture, RenderError> {
    let (width, height) = (decoded.width(), decoded.height());
    if width == 0 || height == 0 {
        return Err(RenderError::Image(format!(
            "environment {name:?} has no pixels"
        )));
    }
    if height * 2 != width {
        // A non-2:1 image is usually a mistake (a square light-probe render, a
        // cropped photo); the precompute still runs, so make it a warning.
        log::warn!("environment {name:?} is {width}x{height}: an equirectangular map is 2:1");
    }
    let mut data = decoded.to_rgb32f().into_raw();
    for sample in &mut data {
        if !sample.is_finite() || *sample < 0.0 {
            *sample = 0.0;
        }
    }
    Ok(HdrTexture {
        name,
        width,
        height,
        data,
    })
}

/// Decode in-memory image bytes, guessing the format from the content.
fn decode_image(bytes: &[u8]) -> Result<image::DynamicImage, RenderError> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| RenderError::Image(e.to_string()))?
        .decode()
        .map_err(|e| RenderError::Image(e.to_string()))
}

/// The display name for an image path: its file stem, or `fallback`.
fn file_stem(path: &Path, fallback: &str) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(fallback)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty temp directory unique to a single test invocation.
    ///
    /// Tests run in parallel and each removes its own directory when done, so
    /// sharing one pid-based directory would let one test delete another's
    /// files mid-read.
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::AtomicUsize;
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "noctis-texture-{}-{}-{}",
            std::process::id(),
            tag,
            n
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// A tiny RGBA PNG written to a unique temp path.
    fn write_png(name: &str) -> std::path::PathBuf {
        let path = temp_dir("png").join(name);
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
    fn decodes_png_bytes_with_an_explicit_name() {
        let path = write_png("bytes.png");
        let bytes = std::fs::read(&path).expect("read png");
        let texture = load_texture_bytes("wood", &bytes, true).expect("load from bytes");
        assert_eq!((texture.width, texture.height), (3, 2));
        assert_eq!(texture.format, TextureFormat::Rgba8Srgb);
        assert_eq!(texture.name, "wood", "the caller names the texture");
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn decoding_garbage_bytes_is_an_image_error() {
        let err = load_texture_bytes("junk", b"not an image", true).unwrap_err();
        assert!(matches!(err, RenderError::Image(_)), "{err:?}");
    }

    #[test]
    fn a_missing_file_is_an_image_error() {
        let err = load_texture("does-not-exist.png", true).unwrap_err();
        assert!(matches!(err, RenderError::Image(_)), "{err:?}");
    }

    /// Encode `width x height` RGBe samples as a Radiance HDR file.
    ///
    /// The decoder in `image` implements only the two run-length encodings (there
    /// is no uncompressed path), so each scanline is written in the new-style
    /// per-component RLE form: a `2, 2, width-hi, width-lo` header followed by
    /// four channels of `(count, bytes...)` literal runs.
    fn hdr_bytes(width: u32, height: u32, rgbe: &[[u8; 4]]) -> Vec<u8> {
        assert_eq!(rgbe.len() as u32, width * height);
        let mut out = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n".to_vec();
        out.extend_from_slice(format!("-Y {height} +X {width}\n").as_bytes());
        for row in 0..height {
            let scanline = &rgbe[(row * width) as usize..((row + 1) * width) as usize];
            out.extend_from_slice(&[2, 2, (width >> 8) as u8, (width & 0xff) as u8]);
            for channel in 0..4 {
                let mut offset = 0usize;
                while offset < scanline.len() {
                    let end = (offset + 128).min(scanline.len());
                    let chunk = &scanline[offset..end];
                    out.push(chunk.len() as u8);
                    out.extend(chunk.iter().map(|pixel| pixel[channel]));
                    offset = end;
                }
            }
        }
        out
    }

    fn write_hdr(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = temp_dir("hdr").join(name);
        std::fs::write(&path, bytes).expect("write hdr");
        path
    }

    #[test]
    fn loads_an_hdr_environment_into_linear_floats() {
        // A 4x2 image whose top row is bright red and bottom row dark.
        let rgbe = [
            [128, 0, 0, 129],
            [128, 0, 0, 129],
            [0, 0, 0, 0],
            [0, 0, 0, 0],
            [64, 64, 64, 128],
            [64, 64, 64, 128],
            [0, 0, 0, 0],
            [0, 0, 0, 0],
        ];
        let path = write_hdr("sky.hdr", &hdr_bytes(4, 2, &rgbe));
        let hdr = load_hdr(&path).expect("load hdr");

        assert_eq!((hdr.width, hdr.height), (4, 2));
        assert_eq!(hdr.name, "sky");
        assert_eq!(hdr.data.len(), 4 * 2 * 3);
        // RGBe (128, 0, 0) at exponent 129 is exactly 1.0 in red, 0 elsewhere.
        assert!((hdr.data[0] - 1.0).abs() < 1e-6, "red = {}", hdr.data[0]);
        assert_eq!(hdr.data[1], 0.0);
        assert_eq!(hdr.data[2], 0.0);
        // The dark pixels decode to exactly zero.
        assert_eq!(&hdr.data[6..9], &[0.0, 0.0, 0.0]);
        // The second row's (64, 64, 64) at exponent 128 decodes to 0.25.
        assert!(
            (hdr.data[12] - 0.25).abs() < 1e-6,
            "grey = {}",
            hdr.data[12]
        );
        // Values are linear floats, not an 8-bit texture.
        assert!(hdr.data.iter().all(|s| s.is_finite() && *s >= 0.0));

        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn a_broken_hdr_is_an_image_error() {
        let path = write_hdr("broken.hdr", b"not a radiance file");
        let err = load_hdr(&path).unwrap_err();
        assert!(matches!(err, RenderError::Image(_)), "{err:?}");
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn decodes_hdr_bytes_with_an_explicit_name() {
        let bytes = hdr_bytes(
            4,
            2,
            &[
                [128, 0, 0, 129],
                [128, 0, 0, 129],
                [0, 0, 0, 0],
                [0, 0, 0, 0],
                [64, 64, 64, 128],
                [64, 64, 64, 128],
                [0, 0, 0, 0],
                [0, 0, 0, 0],
            ],
        );
        let hdr = load_hdr_bytes("sky", &bytes).expect("load from bytes");
        assert_eq!((hdr.width, hdr.height), (4, 2));
        assert_eq!(hdr.name, "sky");
        assert!((hdr.data[0] - 1.0).abs() < 1e-6, "red = {}", hdr.data[0]);
    }
}
