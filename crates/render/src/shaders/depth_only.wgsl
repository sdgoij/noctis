// Depth-only vertex pass shared by the shadow map and the camera depth prepass.
//
// The only difference between the two uses is which matrix is bound to `view`:
// the light's view-projection for shadows, the camera's for the prepass.

struct ViewUniform {
    view_proj: mat4x4<f32>,
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

@group(0) @binding(0) var<uniform> view: ViewUniform;
@group(1) @binding(0) var<uniform> object: ObjectUniform;
// The joint matrices of a skinned draw, bound with a dynamic offset (group 2 in
// this pass, where the PBR pass has it at group 3). A uniform array rather than
// a storage buffer because WebGL2, the browser fallback, has no storage buffers.
const MAX_SKIN_MATRICES: u32 = 128u;
@group(2) @binding(0) var<uniform> skin: array<mat4x4<f32>, MAX_SKIN_MATRICES>;

@vertex
fn vs_main(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return view.view_proj * object.model * vec4<f32>(position, 1.0);
}

@vertex
fn vs_skinned(
    @location(0) position: vec3<f32>,
    @location(4) joints: vec4<f32>,
    @location(5) weights: vec4<f32>,
) -> @builtin(position) vec4<f32> {
    let skinned = skin[u32(joints.x)] * weights.x + skin[u32(joints.y)] * weights.y +
        skin[u32(joints.z)] * weights.z + skin[u32(joints.w)] * weights.w;
    return view.view_proj * object.model * skinned * vec4<f32>(position, 1.0);
}
