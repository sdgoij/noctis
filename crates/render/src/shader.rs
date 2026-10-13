//! The shader seam's WGSL prelude (`.notes/shaders.md`).
//!
//! A plugin's [`Shader`] supplies a body with `vs_main` / `fs_main`; the engine
//! **prepends** this prelude, which declares every binding, the vertex input and
//! the constants, so the plugin never writes a `@group` / `@binding` and cannot
//! reshuffle a layout. This module is pure string generation — no GPU — so it is
//! unit-tested everywhere.
//!
//! The prelude mirrors the engine's real uniforms: `FrameUniform` and
//! `ObjectUniform` are the very structs the PBR pass binds ([`crate::gpu::uniforms`]),
//! and the layer counts come from [`ecs::assets`] so the WGSL and the Rust can
//! never disagree on a capacity.

use ecs::assets::{Shader, TEXTURE_SLOTS, USER_SLOTS};

/// Which pipeline a shader body is built for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadeForm {
    /// A mesh draw: the body sees the standard vertex input `VsIn` and the object
    /// uniform, and provides `vs_main` and `fs_main`.
    Mesh,
    /// A full-screen fragment pass (the sky): the engine supplies the vertex stage
    /// `vs_fullscreen`, and the body provides only `fs_main`, reading `ScreenOut`.
    Screen,
}

/// The engine-owned WGSL prelude for `form`.
pub fn prelude(form: ShadeForm) -> String {
    let mut out = String::with_capacity(4096);
    out.push_str(
        "// Engine shader prelude (Noctis) -- do not edit. Every binding below is the\n\
         // engine's; a shader body declares none of its own (`.notes/shaders.md`).\n\n",
    );

    // The capacities are the Rust constants, so the array bounds and the binding
    // count cannot drift from the values the renderer uploads.
    out.push_str(&format!("const USER_SLOTS: u32 = {USER_SLOTS}u;\n"));
    out.push_str(&format!("const TEXTURE_SLOTS: u32 = {TEXTURE_SLOTS}u;\n\n"));

    out.push_str(
        "struct FrameUniform {\n\
         \x20   view_proj: mat4x4<f32>,\n\
         \x20   inv_view_proj: mat4x4<f32>,\n\
         \x20   camera_pos: vec4<f32>,\n\
         \x20   ambient: vec4<f32>,\n\
         \x20   resolution: vec4<f32>,\n\
         \x20   params: vec4<f32>,\n\
         };\n\n\
         struct ObjectUniform {\n\
         \x20   model: mat4x4<f32>,\n\
         \x20   normal_matrix: mat4x4<f32>,\n\
         \x20   base_color: vec4<f32>,\n\
         \x20   params: vec4<f32>,\n\
         \x20   emissive: vec4<f32>,\n\
         \x20   flags: vec4<f32>,\n\
         \x20   flags2: vec4<f32>,\n\
         };\n\n\
         @group(0) @binding(0) var<uniform> frame: FrameUniform;\n\
         @group(1) @binding(0) var<uniform> object: ObjectUniform;\n\n",
    );

    // Group 2: the material's textures, one binding each (portable across backends,
    // which do not all support binding arrays), then a single shared sampler.
    for i in 0..TEXTURE_SLOTS {
        out.push_str(&format!(
            "@group(2) @binding({i}) var texture_{i}: texture_2d<f32>;\n"
        ));
    }
    out.push_str(&format!(
        "@group(2) @binding({TEXTURE_SLOTS}) var texture_sampler: sampler;\n\n"
    ));

    // Group 3: the plugin's own floats, a fixed vec4 block so the layout never moves.
    out.push_str("@group(3) @binding(0) var<uniform> user: array<vec4<f32>, USER_SLOTS>;\n\n");

    match form {
        ShadeForm::Mesh => {
            out.push_str(
                "struct VsIn {\n\
                 \x20   @location(0) position: vec3<f32>,\n\
                 \x20   @location(1) normal: vec3<f32>,\n\
                 \x20   @location(2) uv: vec2<f32>,\n\
                 \x20   @location(3) tangent: vec4<f32>,\n\
                 \x20   @location(6) color: vec4<f32>,\n\
                 };\n\n",
            );
        }
        ShadeForm::Screen => {
            out.push_str(
                "struct ScreenOut {\n\
                 \x20   @builtin(position) clip: vec4<f32>,\n\
                 \x20   @location(0) uv: vec2<f32>,\n\
                 };\n\n\
                 // A full-screen triangle. `uv` runs 0..1 over the visible area.\n\
                 @vertex\n\
                 fn vs_fullscreen(@builtin(vertex_index) index: u32) -> ScreenOut {\n\
                 \x20   var positions = array<vec2<f32>, 3>(\n\
                 \x20       vec2<f32>(-1.0, -1.0),\n\
                 \x20       vec2<f32>(3.0, -1.0),\n\
                 \x20       vec2<f32>(-1.0, 3.0),\n\
                 \x20   );\n\
                 \x20   let p = positions[index];\n\
                 \x20   var out: ScreenOut;\n\
                 \x20   out.clip = vec4<f32>(p, 0.0, 1.0);\n\
                 \x20   out.uv = p * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);\n\
                 \x20   return out;\n\
                 }\n\n",
            );
        }
    }

    out
}

/// The prelude followed by the shader body — the WGSL source to compile.
pub fn compose(shader: &Shader, form: ShadeForm) -> String {
    let mut out = prelude(form);
    out.push('\n');
    out.push_str(shader.wgsl.trim());
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mesh_prelude_declares_the_engine_bindings() {
        let p = prelude(ShadeForm::Mesh);
        assert!(p.contains("@group(0) @binding(0) var<uniform> frame: FrameUniform;"));
        assert!(p.contains("@group(1) @binding(0) var<uniform> object: ObjectUniform;"));
        assert!(p.contains(&format!(
            "@group(2) @binding({TEXTURE_SLOTS}) var texture_sampler: sampler;"
        )));
        assert!(
            p.contains("@group(3) @binding(0) var<uniform> user: array<vec4<f32>, USER_SLOTS>;")
        );
        // The mesh form sees the standard vertex input.
        assert!(p.contains("struct VsIn"));
        assert!(!p.contains("fn vs_fullscreen"));
    }

    #[test]
    fn the_screen_prelude_supplies_a_fullscreen_vertex_stage() {
        let p = prelude(ShadeForm::Screen);
        assert!(p.contains("fn vs_fullscreen"));
        assert!(p.contains("struct ScreenOut"));
        // ...and no vertex input, because a full-screen pass reads no buffer.
        assert!(!p.contains("struct VsIn"));
    }

    #[test]
    fn the_layer_counts_match_the_rust_constants() {
        let p = prelude(ShadeForm::Mesh);
        assert!(p.contains(&format!("const TEXTURE_SLOTS: u32 = {TEXTURE_SLOTS}u;")));
        assert!(p.contains(&format!("const USER_SLOTS: u32 = {USER_SLOTS}u;")));
        // One binding per texture slot.
        for i in 0..TEXTURE_SLOTS {
            assert!(
                p.contains(&format!("@group(2) @binding({i}) var texture_{i}")),
                "missing texture binding {i}"
            );
        }
    }

    #[test]
    fn compose_puts_the_body_after_the_prelude() {
        let shader = Shader::new(
            "test",
            "fn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }",
        );
        let source = compose(&shader, ShadeForm::Screen);
        let prelude_at = source.find("@group(0)").expect("the prelude");
        let body_at = source.find("fn fs_main").expect("the body");
        assert!(prelude_at < body_at, "the prelude comes first");
        // The body's text appears exactly once (nothing is duplicated).
        assert_eq!(source.matches("fn fs_main").count(), 1);
    }
}
