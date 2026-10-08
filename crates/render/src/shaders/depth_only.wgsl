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

@vertex
fn vs_main(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return view.view_proj * object.model * vec4<f32>(position, 1.0);
}
