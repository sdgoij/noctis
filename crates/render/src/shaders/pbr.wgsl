// Forward metallic/roughness PBR with Cook-Torrance GGX, image-based lighting,
// PCF shadow maps and SSAO.
//
// Lighting model: for each dynamic light,
//
//     Lo = (kd * albedo / PI + D * G * F / (4 * NdotV * NdotL)) * radiance * NdotL
//
// where D is the GGX normal distribution, G the Smith geometry term and F the
// Schlick Fresnel. Indirect light uses the split-sum IBL approximation:
//
//     ambient = kd * irradiance * albedo + prefiltered * (F * brdf.x + brdf.y)
//
// and is scaled by the SSAO term and the SSAO strength. All 2D texture lookups
// use explicit LOD 0 (`textureSampleLevel`) so the shader has no implicit
// derivative requirements and may freely discard and branch.

const PI: f32 = 3.14159265359;
const MAX_REFLECTION_LOD: f32 = 4.0;

struct FrameUniform {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    ambient: vec4<f32>,
    resolution: vec4<f32>,
    params: vec4<f32>,
};

struct LightData {
    position: vec4<f32>,
    direction: vec4<f32>,
    color_intensity: vec4<f32>,
    params: vec4<f32>,   // type, range, cos(inner), cos(outer)
};

struct LightsUniform {
    count: vec4<u32>,
    lights: array<LightData, 16>,
};

struct ShadowUniform {
    view_proj: mat4x4<f32>,
    params: vec4<f32>,
    direction: vec4<f32>,
};

struct ObjectUniform {
    model: mat4x4<f32>,
    normal_matrix: mat4x4<f32>,
    base_color: vec4<f32>,
    params: vec4<f32>,
    emissive: vec4<f32>,
    flags: vec4<f32>,
    flags2: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: FrameUniform;
@group(0) @binding(1) var<uniform> lights: LightsUniform;
@group(0) @binding(2) var env_sampler: sampler;
@group(0) @binding(3) var irradiance_map: texture_cube<f32>;
@group(0) @binding(4) var prefilter_map: texture_cube<f32>;
@group(0) @binding(5) var brdf_lut: texture_2d<f32>;
@group(0) @binding(6) var shadow_sampler: sampler_comparison;
@group(0) @binding(7) var shadow_map: texture_depth_2d;
@group(0) @binding(8) var<uniform> shadow: ShadowUniform;
@group(0) @binding(9) var ssao_map: texture_2d<f32>;

@group(1) @binding(0) var<uniform> object: ObjectUniform;

// The joint matrices for a skinned draw, one per joint of its skin. Bound with a
// dynamic offset so several skinned meshes share the buffer; only `vs_skinned`
// reads it, so an unskinned pipeline need not bind the group.
@group(3) @binding(0) var<storage, read> skin: array<mat4x4<f32>>;

@group(2) @binding(0) var base_color_tex: texture_2d<f32>;
@group(2) @binding(1) var normal_tex: texture_2d<f32>;
@group(2) @binding(2) var mr_tex: texture_2d<f32>;
@group(2) @binding(3) var emissive_tex: texture_2d<f32>;
@group(2) @binding(4) var occlusion_tex: texture_2d<f32>;
@group(2) @binding(5) var material_sampler: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
    @location(6) color: vec4<f32>,
};

// The skinned input: the standard vertex attributes plus four joint indices and
// their weights, from a second vertex buffer.
struct VsInSkinned {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
    @location(6) color: vec4<f32>,
    @location(4) joints: vec4<f32>,
    @location(5) weights: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
    @location(4) color: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let world = object.model * vec4<f32>(in.position, 1.0);
    var out: VsOut;
    out.clip = frame.view_proj * world;
    out.world_pos = world.xyz;
    out.normal = normalize((object.normal_matrix * vec4<f32>(in.normal, 0.0)).xyz);
    out.uv = in.uv;
    out.tangent = in.tangent;
    out.color = in.color;
    return out;
}

/// Blend the four weighted joint matrices for a vertex.
fn skin_matrix(joints: vec4<f32>, weights: vec4<f32>) -> mat4x4<f32> {
    return skin[u32(joints.x)] * weights.x + skin[u32(joints.y)] * weights.y +
        skin[u32(joints.z)] * weights.z + skin[u32(joints.w)] * weights.w;
}

@vertex
fn vs_skinned(in: VsInSkinned) -> VsOut {
    let skinned = skin_matrix(in.joints, in.weights);
    let world = object.model * skinned * vec4<f32>(in.position, 1.0);
    var out: VsOut;
    out.clip = frame.view_proj * world;
    out.world_pos = world.xyz;
    out.normal = normalize((object.normal_matrix * (skinned * vec4<f32>(in.normal, 0.0))).xyz);
    out.uv = in.uv;
    out.tangent = in.tangent;
    out.color = in.color;
    return out;
}

fn distribution_ggx(n_dot_h: f32, roughness: f32) -> f32 {
    let a = roughness * roughness;
    let a2 = a * a;
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / max(PI * d * d, 1e-6);
}

fn geometry_schlick_ggx(n_dot_v: f32, roughness: f32) -> f32 {
    let r = roughness + 1.0;
    let k = (r * r) / 8.0;
    return n_dot_v / (n_dot_v * (1.0 - k) + k);
}

fn geometry_smith(n_dot_v: f32, n_dot_l: f32, roughness: f32) -> f32 {
    return geometry_schlick_ggx(n_dot_v, roughness) * geometry_schlick_ggx(n_dot_l, roughness);
}

fn fresnel_schlick(cos_theta: f32, f0: vec3<f32>) -> vec3<f32> {
    return f0 + (vec3<f32>(1.0) - f0) * pow(clamp(1.0 - cos_theta, 0.0, 1.0), 5.0);
}

// 4x4 percentage-closer filtering against the directional shadow map.
fn shadow_factor(world_pos: vec3<f32>, n_dot_l: f32) -> f32 {
    if (shadow.params.w < 0.5) {
        return 1.0;
    }
    var light_clip = shadow.view_proj * vec4<f32>(world_pos, 1.0);
    let proj = light_clip.xyz / light_clip.w;
    if (proj.x < -1.0 || proj.x > 1.0 || proj.y < -1.0 || proj.y > 1.0 || proj.z > 1.0) {
        return 1.0;
    }
    let uv = proj.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    let bias = shadow.params.x + shadow.params.y * (1.0 - n_dot_l);
    let texel = shadow.params.z;
    var sum = 0.0;
    for (var y = -1; y < 3; y = y + 1) {
        for (var x = -1; x < 3; x = x + 1) {
            let offset = vec2<f32>((f32(x) + 0.5) * texel, (f32(y) + 0.5) * texel);
            sum = sum + textureSampleCompareLevel(shadow_map, shadow_sampler, uv + offset, proj.z - bias);
        }
    }
    return sum / 16.0;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var base_sample = object.base_color;
    if (object.flags.x > 0.5) {
        base_sample = base_sample * textureSampleLevel(base_color_tex, material_sampler, in.uv, 0.0);
    }
    // Per-vertex material (the terrain's grass / sand / rock), when the material opts in.
    if (object.flags2.w > 0.5) {
        base_sample = vec4<f32>(base_sample.rgb * in.color.rgb, base_sample.a * in.color.a);
    }
    if (base_sample.a < object.params.w) {
        discard;
    }

    // Unlit materials output their base color (times the base-color texture)
    // directly: no direct light, image-based lighting, or screen-space occlusion.
    // This is the flat-shaded path for 2D sprites and UI.
    if (object.flags2.z > 0.5) {
        return vec4<f32>(base_sample.rgb, base_sample.a);
    }

    let albedo = base_sample.rgb;
    var metallic = object.params.x;
    var roughness = object.params.y;
    if (object.flags.z > 0.5) {
        let mr = textureSampleLevel(mr_tex, material_sampler, in.uv, 0.0);
        roughness = roughness * mr.g;
        metallic = metallic * mr.b;
    }
    roughness = clamp(roughness, 0.04, 1.0);
    metallic = clamp(metallic, 0.0, 1.0);

    var n = normalize(in.normal);
    if (object.flags.y > 0.5) {
        let sampled = textureSampleLevel(normal_tex, material_sampler, in.uv, 0.0).xyz * 2.0 - 1.0;
        let t = normalize(in.tangent.xyz - n * dot(n, in.tangent.xyz));
        let b = cross(n, t) * in.tangent.w;
        n = normalize(t * sampled.x + b * sampled.y + n * sampled.z);
    }

    let v = normalize(frame.camera_pos.xyz - in.world_pos);
    let n_dot_v = max(dot(n, v), 1e-4);
    let f0 = mix(vec3<f32>(0.04), albedo, metallic);

    var direct = vec3<f32>(0.0);
    let count = lights.count.x;
    for (var i = 0u; i < count; i = i + 1u) {
        let light = lights.lights[i];
        let kind = u32(light.params.x);
        var l = vec3<f32>(0.0);
        var attenuation = 1.0;
        if (kind == 0u) {
            l = normalize(-light.direction.xyz);
        } else {
            let to_light = light.position.xyz - in.world_pos;
            let dist = length(to_light);
            l = to_light / max(dist, 1e-4);
            let r = max(light.params.y, 1e-3);
            let att = clamp(1.0 - (dist * dist) / (r * r), 0.0, 1.0);
            attenuation = att * att;
            if (kind == 2u) {
                let cd = dot(-l, normalize(light.direction.xyz));
                let inner = light.params.z;
                let outer = light.params.w;
                attenuation = attenuation * clamp((cd - outer) / max(inner - outer, 1e-4), 0.0, 1.0);
            }
        }
        let n_dot_l = max(dot(n, l), 0.0);
        if (n_dot_l <= 0.0) {
            continue;
        }
        let h = normalize(v + l);
        let n_dot_h = max(dot(n, h), 0.0);
        let d = distribution_ggx(n_dot_h, roughness);
        let g = geometry_smith(n_dot_v, n_dot_l, roughness);
        let f = fresnel_schlick(max(dot(h, v), 0.0), f0);
        let specular = (d * g * f) / max(4.0 * n_dot_v * n_dot_l, 1e-4);
        let kd = (vec3<f32>(1.0) - f) * (1.0 - metallic);
        var shadow_term = 1.0;
        if (kind == 0u) {
            shadow_term = shadow_factor(in.world_pos, n_dot_l);
        }
        let radiance = light.color_intensity.rgb * light.color_intensity.a;
        direct = direct + (kd * albedo / PI + specular) * radiance * n_dot_l * attenuation * shadow_term;
    }

    // Split-sum IBL.
    let f_ambient = fresnel_schlick(n_dot_v, f0);
    let kd_ambient = (vec3<f32>(1.0) - f_ambient) * (1.0 - metallic);
    let irradiance = textureSampleLevel(irradiance_map, env_sampler, n, 0.0).rgb;
    let r_dir = reflect(-v, n);
    let prefiltered = textureSampleLevel(prefilter_map, env_sampler, r_dir, roughness * MAX_REFLECTION_LOD).rgb;
    let brdf = textureSampleLevel(brdf_lut, env_sampler, vec2<f32>(n_dot_v, roughness), 0.0).rg;
    let specular_ibl = prefiltered * (f_ambient * brdf.x + brdf.y);
    var ambient = (kd_ambient * irradiance * albedo + specular_ibl) * frame.ambient.rgb * frame.ambient.a;

    // Ambient-occlusion texture (R channel) and the screen-space term.
    if (object.flags2.x > 0.5) {
        let ao_tex = textureSampleLevel(occlusion_tex, material_sampler, in.uv, 0.0).r;
        ambient = ambient * ao_tex;
    }
    let screen_uv = in.clip.xy * frame.resolution.zw;
    let ssao = textureSampleLevel(ssao_map, env_sampler, screen_uv, 0.0).r;
    ambient = ambient * mix(1.0, ssao, frame.params.x);

    var emissive = object.emissive.rgb * object.params.z;
    if (object.flags.w > 0.5) {
        emissive = emissive * textureSampleLevel(emissive_tex, material_sampler, in.uv, 0.0).rgb;
    }

    let color = direct + ambient + emissive;
    return vec4<f32>(color, base_sample.a);
}
