// HDR tonemapping to an sRGB-encoded LDR target.
//
// The HDR source is linear `rgba16float`; this pass applies exposure, then an
// ACES (Narkowicz fit) or Reinhard curve, then the sRGB transfer function. The
// output target must be a *linear* `rgba8unorm` texture (no hardware sRGB) so
// the encoding is applied exactly once.

struct TonemapUniform {
    exposure: f32,
    mode: u32,
    pad0: f32,
    pad1: f32,
};

@group(0) @binding(0) var<uniform> tone: TonemapUniform;
@group(0) @binding(1) var hdr_tex: texture_2d<f32>;
@group(0) @binding(2) var tex_sampler: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VsOut {
    // Fullscreen triangle covering clip space [-1, 1].
    var uv = vec2<f32>(f32((vertex_index << 1u) & 2u), f32(vertex_index & 2u));
    var out: VsOut;
    out.uv = uv;
    out.pos = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    return out;
}

fn aces_filmic(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn reinhard(x: vec3<f32>) -> vec3<f32> {
    return x / (vec3<f32>(1.0) + max(x, vec3<f32>(0.0)));
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3<f32>(1e-5)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var color = textureSample(hdr_tex, tex_sampler, in.uv).rgb * tone.exposure;
    if (tone.mode == 0u) {
        color = aces_filmic(color);
    } else {
        color = reinhard(color);
    }
    color = linear_to_srgb(color);
    return vec4<f32>(color, 1.0);
}
