// Camera-facing sprite quads (`.notes/billboards.md`), premultiplied-alpha and
// additive blending. The vertex arrives in world space; the fragment samples the
// sprite and returns premultiplied linear colour, so the pipeline's blend state
// (premultiplied alpha or additive) does the rest.

struct ViewUniform {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> view: ViewUniform;
@group(1) @binding(0) var sprite: texture_2d<f32>;
@group(1) @binding(1) var sprite_sampler: sampler;

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
    let texel = textureSample(sprite, sprite_sampler, in.uv) * in.color;
    if texel.a <= 0.001 {
        discard;
    }
    // Premultiply, so an additive pass adds nothing where the sprite is clear and
    // the premultiplied-alpha blend composites correctly.
    return vec4<f32>(texel.rgb * texel.a, texel.a);
}
