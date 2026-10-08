// Screen-space ambient occlusion.
//
// For each pixel we reconstruct the view-space position from the depth buffer,
// estimate the view-space normal from the screen-space derivatives of that
// position, and march a small hemisphere kernel of sample points. Each sample is
// projected back to screen space; if the stored depth there is nearer than the
// sample (and within the radius), it occludes. The fraction of occluded samples
// becomes the ambient occlusion term, applied later to indirect light only.

struct SsaoUniform {
    projection: mat4x4<f32>,
    inv_projection: mat4x4<f32>,
    samples: array<vec4<f32>, 16>,
    params: vec4<f32>,      // radius, bias, power, kernel size
    resolution: vec4<f32>,  // width, height, 1/width, 1/height
};

@group(0) @binding(0) var<uniform> ssao: SsaoUniform;
@group(0) @binding(1) var depth_map: texture_depth_2d;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VsOut {
    var uv = vec2<f32>(f32((vertex_index << 1u) & 2u), f32(vertex_index & 2u));
    var out: VsOut;
    out.uv = uv;
    out.pos = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    return out;
}

fn view_position_from_depth(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    // Reconstruct from the zero-to-one clip depth convention.
    let ndc = vec3<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth);
    var view = ssao.inv_projection * vec4<f32>(ndc, 1.0);
    return view.xyz / view.w;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) f32 {
    let coord = vec2<i32>(in.pos.xy);
    let center_depth = textureLoad(depth_map, coord, 0);
    if (center_depth >= 1.0) {
        return 1.0;
    }

    let center = view_position_from_depth(in.uv, center_depth);

    // The view-space normal from the gradient of the reconstructed position.
    let dx = dpdx(center);
    let dy = dpdy(center);
    var normal = normalize(cross(dx, dy));
    // View space looks down -Z, so a front-facing normal points toward +Z.
    if (normal.z < 0.0) {
        normal = -normal;
    }

    var occlusion = 0.0;
    let count = i32(ssao.params.w);
    let radius = ssao.params.x;
    for (var i = 0; i < count; i = i + 1) {
        var sample_dir = ssao.samples[i].xyz;
        if (dot(sample_dir, normal) < 0.0) {
            sample_dir = -sample_dir;
        }
        let sample_pos = center + sample_dir * radius;
        var projected = ssao.projection * vec4<f32>(sample_pos, 1.0);
        projected = projected / projected.w;
        let ouv = projected.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
        if (ouv.x < 0.0 || ouv.x > 1.0 || ouv.y < 0.0 || ouv.y > 1.0) {
            continue;
        }
        let ocoord = vec2<i32>(ouv * ssao.resolution.xy);
        let odepth = textureLoad(depth_map, ocoord, 0);
        let ov = view_position_from_depth(ouv, odepth);
        // Only count occlusion within the sample radius; this attenuates the
        // halos that happen when a nearer background surface fills the kernel.
        let range_check = smoothstep(0.0, 1.0, radius / max(abs(center.z - ov.z), 1e-4));
        if (ov.z >= sample_pos.z + ssao.params.y) {
            occlusion = occlusion + range_check;
        }
    }

    let ao = 1.0 - occlusion / max(f32(count), 1.0);
    return pow(clamp(ao, 0.0, 1.0), ssao.params.z);
}
