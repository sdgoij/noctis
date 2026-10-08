// World-space text: glyph quads sampled from a font atlas, alpha-blended.

struct ViewUniform {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> view: ViewUniform;
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = view.view_proj * vec4<f32>(in.position, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // The atlas stores glyph coverage in alpha (RGB is white).
    let coverage = textureSample(atlas, atlas_sampler, in.uv).a;
    let alpha = in.color.a * coverage;
    if alpha <= 0.001 {
        discard;
    }
    return vec4<f32>(in.color.rgb, alpha);
}
