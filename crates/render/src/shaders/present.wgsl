// Present the tonemapped LDR target to a window surface.
//
// The tonemap pass already applied the sRGB transfer function, so the LDR
// texture holds sRGB-encoded bytes in a *linear* `rgba8unorm` texture. Preserving
// that exactly once means:
//
//   * a non-sRGB surface format -> pass the encoded values straight through
//     (`fs_passthrough`), letting the compositor treat them as sRGB, and
//   * an sRGB surface format -> decode back to linear here (`fs_srgb_decode`),
//     because the hardware will re-apply the sRGB transfer on write.
//
// The caller selects the fragment entry point to match the surface format.

@group(0) @binding(0) var ldr_tex: texture_2d<f32>;
@group(0) @binding(1) var ldr_sampler: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VsOut {
    // Fullscreen triangle covering clip space [-1, 1], matching tonemap.wgsl.
    var uv = vec2<f32>(f32((vertex_index << 1u) & 2u), f32(vertex_index & 2u));
    var out: VsOut;
    out.uv = uv;
    out.pos = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    return out;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((max(c, vec3<f32>(0.0)) + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fs_passthrough(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSample(ldr_tex, ldr_sampler, in.uv).rgb, 1.0);
}

@fragment
fn fs_srgb_decode(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(ldr_tex, ldr_sampler, in.uv);
    return vec4<f32>(srgb_to_linear(c.rgb), 1.0);
}
