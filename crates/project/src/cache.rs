//! The generated import cache (`.noctis/`).
//!
//! Decoding a texture is comparatively expensive, so the result is cached under
//! `<project>/.noctis/textures/` keyed by the source file's content (a 64-bit
//! FNV-1a hash plus its byte length) and its import settings. The cache is
//! **generated and meant to be gitignored**: a fresh clone regenerates it by
//! importing, and a changed source (or setting) misses and re-decodes.
//!
//! The cache only ever holds *derived* data; the source file remains the truth,
//! so deleting `.noctis/` is always safe.

use std::io::Read;
use std::path::{Path, PathBuf};

use ecs::assets::{Texture, TextureFormat};

/// The generated cache directory name (gitignored).
pub const CACHE_DIR: &str = ".noctis";

/// The texture-cache subdirectory under [`CACHE_DIR`].
const TEXTURE_DIR: &str = "textures";

/// The on-disk cache magic and version.
const MAGIC: &[u8; 4] = b"NTEX";
const VERSION: u8 = 1;

/// A view onto a project's generated cache.
#[derive(Clone, Debug)]
pub struct ImportCache {
    root: PathBuf,
}

impl ImportCache {
    /// The cache for a project rooted at `project_root`.
    pub fn new(project_root: &Path) -> Self {
        Self {
            root: project_root.join(CACHE_DIR),
        }
    }

    /// The cache directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn texture_path(&self, key: &str) -> PathBuf {
        self.root.join(TEXTURE_DIR).join(format!("{key}.bin"))
    }

    /// A cached texture for `key`, if present and well-formed. A missing or
    /// corrupt entry is a miss (never an error).
    pub fn load_texture(&self, key: &str) -> Option<Texture> {
        let bytes = std::fs::read(self.texture_path(key)).ok()?;
        decode_texture(&bytes)
    }

    /// Store `texture` under `key`. A failure to write the cache is not fatal —
    /// the caller already has the decoded value — so this only returns an error
    /// for the caller to log.
    pub fn store_texture(&self, key: &str, texture: &Texture) -> Result<(), String> {
        let path = self.texture_path(key);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let bytes = encode_texture(texture);
        std::fs::write(&path, bytes).map_err(|e| e.to_string())
    }
}

/// The cache key for a texture: its content hash, byte length, and sRGB-ness.
///
/// Including the length (and the setting) makes an accidental collision between
/// two distinct files vanishingly unlikely for a 64-bit hash, and the setting
/// change correctly misses.
pub fn texture_key(bytes: &[u8], srgb: bool) -> String {
    format!("{:016x}-{:x}-{}", fnv1a(bytes), bytes.len(), u8::from(srgb))
}

/// A deterministic 64-bit FNV-1a hash. Deterministic across runs and platforms
/// (unlike `std`'s `DefaultHasher`, whose seed is not a stability guarantee).
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn format_byte(format: TextureFormat) -> u8 {
    match format {
        TextureFormat::Rgba8Srgb => 0,
        TextureFormat::Rgba8Unorm => 1,
    }
}

fn byte_format(byte: u8) -> Option<TextureFormat> {
    match byte {
        0 => Some(TextureFormat::Rgba8Srgb),
        1 => Some(TextureFormat::Rgba8Unorm),
        _ => None,
    }
}

/// Serialize a texture: magic, version, format, dimensions, name, then the data.
fn encode_texture(texture: &Texture) -> Vec<u8> {
    let name = texture.name.as_bytes();
    let mut out = Vec::with_capacity(4 + 1 + 1 + 8 + 2 + name.len() + texture.data.len());
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.push(format_byte(texture.format));
    out.extend_from_slice(&texture.width.to_le_bytes());
    out.extend_from_slice(&texture.height.to_le_bytes());
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(&texture.data);
    out
}

/// Parse a cached texture, returning `None` on any inconsistency.
fn decode_texture(bytes: &[u8]) -> Option<Texture> {
    let mut cursor = std::io::Cursor::new(bytes);
    let mut magic = [0u8; 4];
    cursor.read_exact(&mut magic).ok()?;
    if &magic != MAGIC {
        return None;
    }
    let mut version = [0u8; 1];
    cursor.read_exact(&mut version).ok()?;
    if version[0] != VERSION {
        return None;
    }
    let mut format = [0u8; 1];
    cursor.read_exact(&mut format).ok()?;
    let format = byte_format(format[0])?;
    let width = u32::from_le_bytes(read_array::<4>(&mut cursor)?);
    let height = u32::from_le_bytes(read_array::<4>(&mut cursor)?);
    let name_len = u16::from_le_bytes(read_array::<2>(&mut cursor)?) as usize;
    let mut name_bytes = vec![0u8; name_len];
    cursor.read_exact(&mut name_bytes).ok()?;
    let name = String::from_utf8(name_bytes).ok()?;
    let mut data = Vec::new();
    cursor.read_to_end(&mut data).ok()?;

    Some(Texture {
        name,
        width,
        height,
        format,
        data,
    })
}

fn read_array<const N: usize>(reader: &mut impl Read) -> Option<[u8; N]> {
    let mut buffer = [0u8; N];
    reader.read_exact(&mut buffer).ok()?;
    Some(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texture() -> Texture {
        Texture {
            name: "wood".to_string(),
            width: 2,
            height: 1,
            format: TextureFormat::Rgba8Srgb,
            data: vec![1, 2, 3, 4, 5, 6, 7, 8],
        }
    }

    #[test]
    fn texture_round_trips_through_the_cache() {
        let dir = std::env::temp_dir().join(format!(
            "noctis-cache-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let cache = ImportCache::new(&dir);
        let key = texture_key(b"source-bytes", true);
        assert!(cache.load_texture(&key).is_none(), "a cold cache misses");

        cache.store_texture(&key, &texture()).expect("store");
        let loaded = cache.load_texture(&key).expect("hit");
        assert_eq!(loaded.name, "wood");
        assert_eq!((loaded.width, loaded.height), (2, 1));
        assert_eq!(loaded.format, TextureFormat::Rgba8Srgb);
        assert_eq!(loaded.data, vec![1, 2, 3, 4, 5, 6, 7, 8]);

        // The cache lives under `.noctis/`.
        assert!(cache.root().ends_with(CACHE_DIR));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_key_changes_with_the_source_and_the_setting() {
        assert_ne!(texture_key(b"a", true), texture_key(b"b", true));
        assert_ne!(texture_key(b"a", true), texture_key(b"a", false));
        // Length is part of the key, so a padded file differs.
        assert_ne!(texture_key(b"a", true), texture_key(b"aa", true));
    }

    #[test]
    fn a_corrupt_entry_is_a_miss() {
        let dir = std::env::temp_dir().join(format!(
            "noctis-cache-corrupt-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let cache = ImportCache::new(&dir);
        let key = texture_key(b"x", true);
        let path = dir
            .join(CACHE_DIR)
            .join(TEXTURE_DIR)
            .join(format!("{key}.bin"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"not a texture").unwrap();
        assert!(cache.load_texture(&key).is_none());

        std::fs::remove_dir_all(&dir).ok();
    }
}
