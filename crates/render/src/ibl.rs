//! Image-based lighting (IBL) from an equirectangular HDR environment.
//!
//! This module implements the classic **split-sum approximation** (Karis, 2013)
//! on the CPU, producing the three textures a PBR shader needs:
//!
//! 1. A **diffuse irradiance cubemap** — the cosine-weighted convolution of the
//!    environment, stored as a small cube (default 16x16 per face).
//! 2. A **prefiltered specular cubemap** — a mip chain where each mip level `l`
//!    convolves the environment with a GGX lobe of roughness `l / (mips - 1)`.
//! 3. A **BRDF integration LUT** — a 2D texture indexed by `(N·V, roughness)`
//!    whose two channels are the scale (`A`) and bias (`B`) applied to the
//!    Fresnel term.
//!
//! The reason this is a good simplification for an engine: the expensive
//! convolution happens once per HDR, on the CPU, at low resolution. The GPU then
//! only samples three ordinary textures. The math is identical to the shader
//! version used by most engines, so results match what you would get from GPU
//! precomputation, just at lower resolution.
//!
//! References: Karis, *Real Shading in Unreal Engine 4* (2013);
//! Lagarde & de Rousiers, *Moving Frostbite to PBR* (2014).

use ecs::assets::HdrTexture;
use math::{Vec3, vec3};

/// Resolution and sample-count configuration for IBL precomputation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IblConfig {
    /// Per-face size of the diffuse irradiance cube.
    pub irradiance_size: u32,
    /// Per-face size of mip 0 of the prefiltered specular cube.
    pub prefilter_base_size: u32,
    /// Number of mip levels in the prefiltered specular cube.
    pub prefilter_mip_count: u32,
    /// Size of the square BRDF LUT.
    pub brdf_lut_size: u32,
    /// Samples per texel for the irradiance convolution.
    pub irradiance_samples: u32,
    /// Samples per texel for the specular prefilter.
    pub prefilter_samples: u32,
    /// Samples per texel for the BRDF LUT.
    pub brdf_samples: u32,
}

impl Default for IblConfig {
    fn default() -> Self {
        IblConfig {
            irradiance_size: 16,
            prefilter_base_size: 32,
            prefilter_mip_count: 5,
            brdf_lut_size: 64,
            irradiance_samples: 256,
            prefilter_samples: 128,
            brdf_samples: 128,
        }
    }
}

impl IblConfig {
    /// A very cheap configuration, suitable for unit tests.
    pub fn tiny() -> Self {
        IblConfig {
            irradiance_size: 4,
            prefilter_base_size: 4,
            prefilter_mip_count: 3,
            brdf_lut_size: 8,
            irradiance_samples: 16,
            prefilter_samples: 8,
            brdf_samples: 16,
        }
    }
}

/// A cubemap stored as six RGBA `f32` faces laid out face-major.
#[derive(Clone, Debug, PartialEq)]
pub struct CubeMap {
    /// Per-face edge length.
    pub size: u32,
    /// `6 * size * size` RGBA texels; face `f`, row `y`, column `x` is at
    /// `(f * size * size + y * size + x)`.
    pub faces: Vec<[f32; 4]>,
}

impl CubeMap {
    /// An all-zero cubemap.
    pub fn new(size: u32) -> CubeMap {
        CubeMap {
            size,
            faces: vec![[0.0; 4]; (size * size * 6) as usize],
        }
    }

    /// A cubemap filled with a constant color.
    pub fn solid(size: u32, color: [f32; 4]) -> CubeMap {
        CubeMap {
            size,
            faces: vec![color; (size * size * 6) as usize],
        }
    }

    /// Read a texel.
    pub fn get(&self, face: u32, x: u32, y: u32) -> [f32; 4] {
        self.faces[(face * self.size * self.size + y * self.size + x) as usize]
    }

    /// Write a texel.
    pub fn set(&mut self, face: u32, x: u32, y: u32, value: [f32; 4]) {
        self.faces[(face * self.size * self.size + y * self.size + x) as usize] = value;
    }
}

/// The direction for cube face `face` at local coordinates `(u, v)` in
/// `[-1, 1]`. This is the standard OpenGL/Vulkan cubemap orientation.
pub fn cube_face_direction(face: u32, u: f32, v: f32) -> Vec3 {
    let d = match face {
        0 => vec3(1.0, -v, -u),  // +X
        1 => vec3(-1.0, -v, u),  // -X
        2 => vec3(u, 1.0, v),    // +Y
        3 => vec3(u, -1.0, -v),  // -Y
        4 => vec3(u, -v, 1.0),   // +Z
        _ => vec3(-u, -v, -1.0), // -Z
    };
    normalize(d)
}

/// The direction for the centre of texel `(x, y)` on `face` of a `size` cube.
pub fn cube_texel_direction(face: u32, x: u32, y: u32, size: u32) -> Vec3 {
    let u = 2.0 * (x as f32 + 0.5) / size as f32 - 1.0;
    let v = 2.0 * (y as f32 + 0.5) / size as f32 - 1.0;
    cube_face_direction(face, u, v)
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

fn dot(a: Vec3, b: Vec3) -> f32 {
    a.x * b.x + a.y * b.y + a.z * b.z
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    vec3(
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    )
}

// ---------------------------------------------------------------------------
// Equirectangular mapping
// ---------------------------------------------------------------------------

/// Map a unit direction to equirectangular `(u, v)` with `u` the longitude in
/// `[0, 1]` (`0.5` at `+X` and wrapping) and `v` the latitude in `[0, 1]`
/// (`0` at `+Y`, `1` at `-Y`).
pub fn dir_to_equirect_uv(dir: Vec3) -> [f32; 2] {
    let theta = dir.z.atan2(dir.x);
    let u = theta / std::f32::consts::TAU + 0.5;
    let phi = dir.y.clamp(-1.0, 1.0).asin();
    let v = 0.5 - phi / std::f32::consts::PI;
    [u, v]
}

/// The inverse of [`dir_to_equirect_uv`].
pub fn equirect_uv_to_dir(uv: [f32; 2]) -> Vec3 {
    let theta = (uv[0] - 0.5) * std::f32::consts::TAU;
    let phi = (0.5 - uv[1]) * std::f32::consts::PI;
    let (sin_phi, cos_phi) = phi.sin_cos();
    let (sin_t, cos_t) = theta.sin_cos();
    vec3(cos_phi * cos_t, sin_phi, cos_phi * sin_t)
}

/// Bilinearly sample an equirectangular HDR texture along `dir`.
///
/// `u` wraps; `v` clamps to the poles.
pub fn sample_equirect(hdr: &HdrTexture, dir: Vec3) -> [f32; 3] {
    sample_equirect_level(&hdr.data, hdr.width, hdr.height, dir)
}

fn sample_equirect_level(data: &[f32], width: u32, height: u32, dir: Vec3) -> [f32; 3] {
    if width == 0 || height == 0 || data.is_empty() {
        return [0.0; 3];
    }
    let [u, v] = dir_to_equirect_uv(dir);
    let fx = u * width as f32 - 0.5;
    let fy = v * height as f32 - 0.5;
    let x0 = fx.floor();
    let y0 = fy.floor();
    let tx = fx - x0;
    let ty = fy - y0;
    let x0 = x0 as i64;
    let y0 = y0 as i64;

    let fetch = |xi: i64, yi: i64| -> [f32; 3] {
        let w = width as i64;
        let h = height as i64;
        let x = xi.rem_euclid(w);
        let y = yi.clamp(0, h - 1);
        let idx = ((y * w + x) * 3) as usize;
        if idx + 2 < data.len() {
            [data[idx], data[idx + 1], data[idx + 2]]
        } else {
            [0.0; 3]
        }
    };

    let c00 = fetch(x0, y0);
    let c10 = fetch(x0 + 1, y0);
    let c01 = fetch(x0, y0 + 1);
    let c11 = fetch(x0 + 1, y0 + 1);

    let mut out = [0.0f32; 3];
    for i in 0..3 {
        let top = c00[i] * (1.0 - tx) + c10[i] * tx;
        let bottom = c01[i] * (1.0 - tx) + c11[i] * tx;
        out[i] = top * (1.0 - ty) + bottom * ty;
    }
    out
}

// ---------------------------------------------------------------------------
// Quasi-random sampling
// ---------------------------------------------------------------------------

/// The Van der Corput radical inverse of `bits` in base 2.
pub fn radical_inverse_vdc(mut bits: u32) -> f32 {
    bits = bits.reverse_bits();
    bits as f32 * 2.328_306_4e-10 // 2^-32
}

/// The `i`-th 2D Hammersley point out of `n`.
pub fn hammersley(i: u32, n: u32) -> [f32; 2] {
    let n = n.max(1);
    [i as f32 / n as f32, radical_inverse_vdc(i)]
}

/// A cosine-weighted hemisphere sample (in tangent space, `+Z` up) from two
/// uniform `xi` values.
pub fn cosine_sample_hemisphere(xi: [f32; 2]) -> Vec3 {
    let r = xi[0].sqrt();
    let phi = std::f32::consts::TAU * xi[1];
    let (sin_phi, cos_phi) = phi.sin_cos();
    let z = (1.0 - xi[0]).max(0.0).sqrt();
    vec3(r * cos_phi, r * sin_phi, z)
}

/// Build a tangent frame around `n` and rotate a tangent-space direction into
/// world space.
pub fn tangent_to_world(v: Vec3, n: Vec3) -> Vec3 {
    let up = if n.y.abs() > 0.999 {
        vec3(0.0, 0.0, 1.0)
    } else {
        vec3(0.0, 1.0, 0.0)
    };
    let t = normalize(cross(up, n));
    let b = cross(n, t);
    vec3(
        t.x * v.x + b.x * v.y + n.x * v.z,
        t.y * v.x + b.y * v.y + n.y * v.z,
        t.z * v.x + b.z * v.y + n.z * v.z,
    )
}

/// Importance-sample the GGX normal distribution.
///
/// Returns a half-vector in world space around normal `n`. `roughness` is the
/// perceptual roughness; internally `alpha = roughness^2`.
pub fn importance_sample_ggx(xi: [f32; 2], n: Vec3, roughness: f32) -> Vec3 {
    let a = roughness * roughness;
    let phi = std::f32::consts::TAU * xi[0];
    let cos_theta = ((1.0 - xi[1]) / (1.0 + (a * a - 1.0) * xi[1]))
        .max(0.0)
        .sqrt();
    let sin_theta = (1.0 - cos_theta * cos_theta).max(0.0).sqrt();
    let (sin_phi, cos_phi) = phi.sin_cos();
    let h_tangent = vec3(sin_theta * cos_phi, sin_theta * sin_phi, cos_theta);
    normalize(tangent_to_world(h_tangent, n))
}

/// Schlick-GGX geometry term for a single direction.
pub fn geometry_schlick_ggx(n_dot_v: f32, roughness: f32) -> f32 {
    let r = roughness + 1.0;
    let k = (r * r) / 8.0;
    n_dot_v / (n_dot_v * (1.0 - k) + k).max(1e-5)
}

/// Smith's separable geometry function.
pub fn geometry_smith(n_dot_v: f32, n_dot_l: f32, roughness: f32) -> f32 {
    geometry_schlick_ggx(n_dot_v, roughness) * geometry_schlick_ggx(n_dot_l, roughness)
}

/// Integrate the BRDF for a given `(N·V, roughness)` using importance sampling.
///
/// Returns `[A, B]` — the scale and bias for the split-sum Fresnel term. The
/// integral is
///
/// ```text
/// A = integral (1 - (1 - V·H)^5) * G_vis * (N·L) dω / p(H)
/// B = integral         (1 - V·H)^5  * G_vis * (N·L) dω / p(H)
/// ```
///
/// which, with the GGX importance PDF, reduces to the weighted average used
/// below.
pub fn integrate_brdf(n_dot_v: f32, roughness: f32, samples: u32) -> [f32; 2] {
    let n_dot_v = n_dot_v.clamp(1e-4, 1.0);
    let v = vec3((1.0 - n_dot_v * n_dot_v).max(0.0).sqrt(), 0.0, n_dot_v);
    let n = vec3(0.0, 0.0, 1.0);
    let mut a = 0.0f32;
    let mut b = 0.0f32;
    let samples = samples.max(1);
    for i in 0..samples {
        let xi = hammersley(i, samples);
        let h = importance_sample_ggx(xi, n, roughness);
        let l = normalize(vec3(
            2.0 * dot(v, h) * h.x - v.x,
            2.0 * dot(v, h) * h.y - v.y,
            2.0 * dot(v, h) * h.z - v.z,
        ));
        let n_dot_l = l.z;
        if n_dot_l > 0.0 {
            let n_dot_h = h.z.max(0.0);
            let v_dot_h = dot(v, h).max(0.0);
            let g = geometry_smith(n_dot_v, n_dot_l, roughness);
            let g_vis = (g * v_dot_h) / (n_dot_h * n_dot_v).max(1e-5);
            let fc = (1.0 - v_dot_h).clamp(0.0, 1.0).powi(5);
            a += (1.0 - fc) * g_vis;
            b += fc * g_vis;
        }
    }
    [a / samples as f32, b / samples as f32]
}

// ---------------------------------------------------------------------------
// Equirectangular mip chain
// ---------------------------------------------------------------------------

/// A box-filtered mip chain of an equirectangular HDR texture, used as the
/// pre-blurred source for specular prefiltering.
#[derive(Clone, Debug)]
pub struct EquirectMips {
    levels: Vec<(u32, u32, Vec<f32>)>,
}

impl EquirectMips {
    /// Build the chain from an HDR texture. Level 0 is the source; each
    /// subsequent level is a 2x2 box downsample until 1x1.
    pub fn build(hdr: &HdrTexture) -> EquirectMips {
        let mut levels = Vec::new();
        let mut w = hdr.width.max(1);
        let mut h = hdr.height.max(1);
        levels.push((w, h, hdr.data.clone()));
        while w > 1 || h > 1 {
            let nw = (w / 2).max(1);
            let nh = (h / 2).max(1);
            let prev = levels.last().expect("at least one level");
            let mut data = vec![0.0f32; (nw * nh * 3) as usize];
            for y in 0..nh {
                for x in 0..nw {
                    let mut acc = [0.0f32; 3];
                    let mut count = 0.0f32;
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let sx = (x * 2 + dx).min(w - 1);
                            let sy = (y * 2 + dy).min(h - 1);
                            let idx = ((sy * w + sx) * 3) as usize;
                            if idx + 2 < prev.2.len() {
                                acc[0] += prev.2[idx];
                                acc[1] += prev.2[idx + 1];
                                acc[2] += prev.2[idx + 2];
                                count += 1.0;
                            }
                        }
                    }
                    let out = ((y * nw + x) * 3) as usize;
                    if count > 0.0 {
                        data[out] = acc[0] / count;
                        data[out + 1] = acc[1] / count;
                        data[out + 2] = acc[2] / count;
                    }
                }
            }
            levels.push((nw, nh, data));
            w = nw;
            h = nh;
        }
        EquirectMips { levels }
    }

    /// The number of mip levels.
    pub fn level_count(&self) -> usize {
        self.levels.len()
    }

    /// Sample a specific level along `dir`.
    pub fn sample_level(&self, level: u32, dir: Vec3) -> [f32; 3] {
        let level = (level as usize).min(self.levels.len().saturating_sub(1));
        let (w, h, data) = &self.levels[level];
        sample_equirect_level(data, *w, *h, dir)
    }

    /// Sample the chain with trilinear interpolation at a fractional `lod`.
    pub fn sample_lod(&self, dir: Vec3, lod: f32) -> [f32; 3] {
        let max_level = self.levels.len().saturating_sub(1) as f32;
        let lod = lod.clamp(0.0, max_level);
        let l0 = lod.floor();
        let l1 = (l0 + 1.0).min(max_level);
        let t = lod - l0;
        let a = self.sample_level(l0 as u32, dir);
        let b = self.sample_level(l1 as u32, dir);
        [
            a[0] * (1.0 - t) + b[0] * t,
            a[1] * (1.0 - t) + b[1] * t,
            a[2] * (1.0 - t) + b[2] * t,
        ]
    }
}

// ---------------------------------------------------------------------------
// Precomputation entry points
// ---------------------------------------------------------------------------

/// Cosine-weighted convolution of the environment into a small cubemap.
pub fn generate_irradiance(hdr: &HdrTexture, size: u32, samples: u32) -> CubeMap {
    let size = size.max(1);
    let samples = samples.max(1);
    let mut cube = CubeMap::new(size);
    for face in 0..6u32 {
        for y in 0..size {
            for x in 0..size {
                let n = cube_texel_direction(face, x, y, size);
                let mut acc = [0.0f32; 3];
                for i in 0..samples {
                    let xi = hammersley(i, samples);
                    let local = cosine_sample_hemisphere(xi);
                    let l = normalize(tangent_to_world(local, n));
                    let s = sample_equirect(hdr, l);
                    acc[0] += s[0];
                    acc[1] += s[1];
                    acc[2] += s[2];
                }
                let inv = 1.0 / samples as f32;
                cube.set(face, x, y, [acc[0] * inv, acc[1] * inv, acc[2] * inv, 1.0]);
            }
        }
    }
    cube
}

/// GGX-prefiltered specular cubemap mip chain.
///
/// Mip `l` has size `max(1, base >> l)` and roughness `l / (mips - 1)`.
pub fn generate_prefiltered(
    hdr: &HdrTexture,
    base_size: u32,
    mip_count: u32,
    samples: u32,
) -> Vec<CubeMap> {
    let mips = mip_count.max(1);
    let samples = samples.max(1);
    let source = EquirectMips::build(hdr);
    let mut out = Vec::with_capacity(mips as usize);
    for m in 0..mips {
        let size = (base_size >> m).max(1);
        let roughness = if mips <= 1 {
            0.0
        } else {
            m as f32 / (mips - 1) as f32
        };
        let mut cube = CubeMap::new(size);
        for face in 0..6u32 {
            for y in 0..size {
                for x in 0..size {
                    let n = cube_texel_direction(face, x, y, size);
                    // The split-sum approximation assumes V == N == R.
                    let v = n;
                    let mut acc = [0.0f32; 3];
                    let mut weight = 0.0f32;
                    for i in 0..samples {
                        let xi = hammersley(i, samples);
                        let h = importance_sample_ggx(xi, n, roughness);
                        let l = normalize(vec3(
                            2.0 * dot(v, h) * h.x - v.x,
                            2.0 * dot(v, h) * h.y - v.y,
                            2.0 * dot(v, h) * h.z - v.z,
                        ));
                        let n_dot_l = dot(n, l).max(0.0);
                        if n_dot_l > 0.0 {
                            // Sample the pre-blurred source at the current mip;
                            // rougher mips read blurrier data, which removes
                            // the fireflies that raw importance sampling gives.
                            let s = source.sample_lod(l, m as f32);
                            acc[0] += s[0] * n_dot_l;
                            acc[1] += s[1] * n_dot_l;
                            acc[2] += s[2] * n_dot_l;
                            weight += n_dot_l;
                        }
                    }
                    let inv = if weight > 0.0 { 1.0 / weight } else { 0.0 };
                    cube.set(face, x, y, [acc[0] * inv, acc[1] * inv, acc[2] * inv, 1.0]);
                }
            }
        }
        out.push(cube);
    }
    out
}

/// The split-sum BRDF integration LUT as interleaved `(A, B)` pairs.
pub fn generate_brdf_lut(size: u32, samples: u32) -> Vec<f32> {
    let size = size.max(1);
    let mut lut = vec![0.0f32; (size * size * 2) as usize];
    for y in 0..size {
        let roughness = (y as f32 + 0.5) / size as f32;
        for x in 0..size {
            let n_dot_v = ((x as f32 + 0.5) / size as f32).max(1e-3);
            let [a, b] = integrate_brdf(n_dot_v, roughness, samples);
            let idx = ((y * size + x) * 2) as usize;
            lut[idx] = a;
            lut[idx + 1] = b;
        }
    }
    lut
}

/// All three IBL textures, ready to be uploaded.
#[derive(Clone, Debug)]
pub struct IblData {
    /// Diffuse irradiance cube.
    pub irradiance: CubeMap,
    /// Specular prefiltered cube mips, index 0 = base level.
    pub prefiltered: Vec<CubeMap>,
    /// BRDF LUT, `size * size * 2` interleaved `(A, B)` floats.
    pub brdf_lut: Vec<f32>,
    /// Edge length of the BRDF LUT.
    pub brdf_lut_size: u32,
}

impl IblData {
    /// A neutral, fully-lit environment: white irradiance, white specular and a
    /// constant LUT. Used when no HDR is provided.
    pub fn neutral() -> IblData {
        let mut lut = vec![0.0f32; 4 * 4 * 2];
        for texel in lut.as_chunks_mut::<2>().0 {
            texel[0] = 1.0;
            texel[1] = 0.0;
        }
        IblData {
            irradiance: CubeMap::solid(1, [1.0, 1.0, 1.0, 1.0]),
            prefiltered: vec![CubeMap::solid(1, [1.0, 1.0, 1.0, 1.0])],
            brdf_lut: lut,
            brdf_lut_size: 4,
        }
    }

    /// Precompute all IBL textures from an equirectangular HDR.
    pub fn generate(hdr: &HdrTexture, config: IblConfig) -> IblData {
        let irradiance =
            generate_irradiance(hdr, config.irradiance_size, config.irradiance_samples);
        let prefiltered = generate_prefiltered(
            hdr,
            config.prefilter_base_size,
            config.prefilter_mip_count,
            config.prefilter_samples,
        );
        let brdf_lut = generate_brdf_lut(config.brdf_lut_size, config.brdf_samples);
        IblData {
            irradiance,
            prefiltered,
            brdf_lut,
            brdf_lut_size: config.brdf_lut_size.max(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient_hdr() -> HdrTexture {
        // 8x4 equirect: brightness peaks at the +X longitude and dips at -X, so
        // samples along those axes are unambiguous (no wrap-around ambiguity).
        let (w, h) = (8u32, 4u32);
        let mut data = vec![0.0f32; (w * h * 3) as usize];
        for y in 0..h {
            for x in 0..w {
                let idx = ((y * w + x) * 3) as usize;
                let theta = std::f32::consts::TAU * (x as f32 / w as f32 - 0.5);
                let v = 0.5 + 0.5 * theta.cos();
                data[idx] = v;
                data[idx + 1] = v * 0.5;
                data[idx + 2] = 1.0 - v;
            }
        }
        HdrTexture {
            name: "test".into(),
            width: w,
            height: h,
            data,
        }
    }

    #[test]
    fn equirect_round_trip_axes() {
        for dir in [
            vec3(1.0, 0.0, 0.0),
            vec3(-1.0, 0.0, 0.0),
            vec3(0.0, 0.0, 1.0),
            vec3(0.0, 0.0, -1.0),
            vec3(0.0, 1.0, 0.0),
            vec3(0.0, -1.0, 0.0),
        ] {
            let uv = dir_to_equirect_uv(dir);
            let back = equirect_uv_to_dir(uv);
            assert!(
                math::vec3_approx_eq(back, dir, 1e-4),
                "dir {dir:?} -> uv {uv:?} -> {back:?}"
            );
        }
    }

    #[test]
    fn equirect_pole_v_is_zero_at_plus_y() {
        let uv = dir_to_equirect_uv(vec3(0.0, 1.0, 0.0));
        assert!(uv[1].abs() < 1e-5, "v = {}", uv[1]);
        let uv = dir_to_equirect_uv(vec3(0.0, -1.0, 0.0));
        assert!((uv[1] - 1.0).abs() < 1e-5, "v = {}", uv[1]);
    }

    #[test]
    fn hammersley_bounds() {
        for i in 0..32 {
            let [x, y] = hammersley(i, 32);
            assert!((0.0..=1.0).contains(&x));
            assert!((0.0..=1.0).contains(&y));
        }
        assert_eq!(hammersley(0, 32), [0.0, 0.0]);
    }

    #[test]
    fn cosine_sample_is_in_hemisphere() {
        for i in 0..64 {
            let v = cosine_sample_hemisphere(hammersley(i, 64));
            assert!(v.z >= 0.0);
            let len = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
            assert!((len - 1.0).abs() < 1e-4, "len = {len}");
        }
    }

    #[test]
    fn importance_sample_is_unit_and_hemisphere() {
        let n = vec3(0.0, 0.0, 1.0);
        for i in 0..32 {
            let h = importance_sample_ggx(hammersley(i, 32), n, 0.4);
            let len = (h.x * h.x + h.y * h.y + h.z * h.z).sqrt();
            assert!((len - 1.0).abs() < 1e-3, "len = {len}");
            assert!(h.z > -1e-4, "below hemisphere: {}", h.z);
        }
    }

    #[test]
    fn importance_sample_rotates_with_normal() {
        let n = vec3(1.0, 0.0, 0.0);
        let h = importance_sample_ggx([0.3, 0.7], n, 0.5);
        assert!(dot(h, n) > 0.0, "sample not around normal");
    }

    #[test]
    fn brdf_integration_is_well_behaved() {
        for roughness in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
            let [a, b] = integrate_brdf(0.7, roughness, 64);
            assert!(a.is_finite() && b.is_finite());
            assert!((0.0..=1.01).contains(&a), "A = {a}");
            assert!((0.0..=1.01).contains(&b), "B = {b}");
        }
    }

    #[test]
    fn brdf_at_grazing_angle_bounded() {
        let [a, b] = integrate_brdf(0.01, 0.5, 64);
        assert!(a.is_finite() && b.is_finite());
        assert!((0.0..=1.01).contains(&a));
        assert!((0.0..=1.01).contains(&b));
    }

    #[test]
    fn geometry_terms_are_bounded() {
        for nv in [0.01f32, 0.5, 1.0] {
            for r in [0.0f32, 0.5, 1.0] {
                let g = geometry_schlick_ggx(nv, r);
                assert!((0.0..=1.0001).contains(&g), "G = {g}");
            }
        }
    }

    #[test]
    fn equirect_samples_bright_longitude() {
        let hdr = gradient_hdr();
        // +X sits at the peak of the gradient, -X at the trough.
        let bright = sample_equirect(&hdr, vec3(1.0, 0.0, 0.0));
        let dark = sample_equirect(&hdr, vec3(-1.0, 0.0, 0.0));
        assert!(bright[0] > 0.8, "red = {}", bright[0]);
        assert!(dark[0] < 0.2, "red = {}", dark[0]);
        assert!(bright[0] > dark[0]);
    }

    #[test]
    fn irradiance_has_expected_shape_and_values() {
        let hdr = gradient_hdr();
        let cube = generate_irradiance(&hdr, 4, 32);
        assert_eq!(cube.size, 4);
        assert_eq!(cube.faces.len(), 4 * 4 * 6);
        for texel in &cube.faces {
            for &c in texel.iter().take(3) {
                assert!(c.is_finite() && c >= 0.0, "c = {c}");
            }
        }
    }

    #[test]
    fn prefiltered_mip_chain_shrinks() {
        let hdr = gradient_hdr();
        let mips = generate_prefiltered(&hdr, 8, 4, 16);
        assert_eq!(mips.len(), 4);
        assert_eq!(mips[0].size, 8);
        assert_eq!(mips[1].size, 4);
        assert_eq!(mips[2].size, 2);
        assert_eq!(mips[3].size, 1);
        for mip in &mips {
            for texel in &mip.faces {
                assert!(texel[0].is_finite() && texel[0] >= 0.0);
            }
        }
    }

    #[test]
    fn brdf_lut_has_expected_size() {
        let lut = generate_brdf_lut(8, 16);
        assert_eq!(lut.len(), 8 * 8 * 2);
        for v in &lut {
            assert!(v.is_finite() && (0.0..=1.01).contains(v), "v = {v}");
        }
    }

    #[test]
    fn equirect_mips_halve() {
        let hdr = gradient_hdr();
        let mips = EquirectMips::build(&hdr);
        assert!(mips.level_count() >= 3);
        let s0 = mips.sample_level(0, vec3(1.0, 0.0, 0.0));
        let s1 = mips.sample_level(1, vec3(1.0, 0.0, 0.0));
        assert!(s0[0].is_finite() && s1[0].is_finite());
    }

    #[test]
    fn generate_full_pipeline_tiny() {
        let hdr = gradient_hdr();
        let ibl = IblData::generate(&hdr, IblConfig::tiny());
        assert_eq!(ibl.brdf_lut_size, 8);
        assert!(!ibl.prefiltered.is_empty());
        assert_eq!(ibl.irradiance.size, 4);
    }

    #[test]
    fn neutral_ibl_is_finite() {
        let ibl = IblData::neutral();
        assert!(!ibl.brdf_lut.is_empty());
        assert_eq!(ibl.prefiltered.len(), 1);
    }

    #[test]
    fn cube_texel_direction_is_unit() {
        for face in 0..6 {
            let d = cube_texel_direction(face, 0, 0, 4);
            let len = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt();
            assert!((len - 1.0).abs() < 1e-5);
        }
    }
}
