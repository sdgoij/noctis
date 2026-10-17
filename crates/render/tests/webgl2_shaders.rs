//! Every shader the GL backend compiles must translate to GLSL ES 3.00, the
//! language a WebGL2 context compiles.
//!
//! The browser build prefers WebGPU and falls back to WebGL2 (`.notes/web.md`).
//! wgpu translates WGSL to GLSL for that backend, and the translation is only as
//! good as the WGSL: a construct the GLSL writer cannot express (a storage
//! buffer, for instance) would fail when a browser creates the pipeline. This
//! test runs that translation for every shader and every entry point on a
//! normal host build, where it is cheap and runs in CI.

#![forbid(unsafe_code)]

use naga::back::glsl;
use naga::front::wgsl;
use naga::valid::{Capabilities, ModuleInfo, ValidationFlags, Validator};

/// `(name, source)` for every shader the GL backend compiles.
///
/// `ssao` is deliberately absent: it reads raw depth with `textureLoad`, which
/// naga cannot express in GLSL, so the GL backend renders without it (see
/// [`ssao_is_the_documented_exception`]).
const SHADERS: &[(&str, &str)] = &[
    ("billboard", include_str!("../src/shaders/billboard.wgsl")),
    ("depth_only", include_str!("../src/shaders/depth_only.wgsl")),
    ("gizmo", include_str!("../src/shaders/gizmo.wgsl")),
    ("pbr", include_str!("../src/shaders/pbr.wgsl")),
    ("present", include_str!("../src/shaders/present.wgsl")),
    ("text", include_str!("../src/shaders/text.wgsl")),
    ("tonemap", include_str!("../src/shaders/tonemap.wgsl")),
];

#[test]
fn every_shader_translates_to_glsl_es_300() {
    for &(name, source) in SHADERS {
        let module = wgsl::parse_str(source)
            .unwrap_or_else(|error| panic!("{name}: WGSL parse failed: {error}"));
        let info = Validator::new(ValidationFlags::all(), Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|error| panic!("{name}: WGSL validation failed: {error}"));

        for entry_point in &module.entry_points {
            translate(&module, &info, &entry_point.name, entry_point.stage)
                .unwrap_or_else(|error| panic!("{name}::{}: {error}", entry_point.name));
        }
    }
}

/// The SSAO pass is the one the GL backend does not build, because its
/// `textureLoad` of a depth texture has no GLSL equivalent. This test pins that
/// down: if a naga upgrade starts translating it, the test fails and asks for
/// the pass to be re-enabled (`backend_supports_ssao`) rather than silently
/// leaving the feature off on the web.
#[test]
fn ssao_is_the_documented_exception() {
    let module = wgsl::parse_str(include_str!("../src/shaders/ssao.wgsl")).expect("SSAO parses");
    let info = Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .expect("SSAO validates");
    let fragment = module
        .entry_points
        .iter()
        .find(|entry_point| entry_point.stage == naga::ShaderStage::Fragment)
        .expect("SSAO has a fragment entry point");

    let result = translate(&module, &info, &fragment.name, fragment.stage);
    assert!(
        result.is_err(),
        "naga now translates the SSAO shader to GLSL; re-enable it in \
         `backend_supports_ssao` and add it to `SHADERS`"
    );
}

/// The shaders declare the joint-array size themselves (WGSL needs a constant
/// expression), and the bind group binds one slot of exactly that size. Keep the
/// two in step: a shader array shorter than the slot would silently truncate the
/// joints WebGL2's uniform block can carry.
#[test]
fn the_skin_uniform_size_matches_the_rust_slot() {
    let expected = format!(
        "const MAX_SKIN_MATRICES: u32 = {}u;",
        render::gpu::pipelines::MAX_SKIN_MATRICES
    );
    for (name, source) in [
        ("pbr", include_str!("../src/shaders/pbr.wgsl")),
        ("depth_only", include_str!("../src/shaders/depth_only.wgsl")),
    ] {
        assert!(
            source.contains(&expected),
            "{name}: expected `{expected}` to match the Rust slot size"
        );
    }
    // WebGL2 guarantees a 16 KiB uniform block; the slot must fit in one.
    assert!(u64::from(render::gpu::pipelines::MAX_SKIN_MATRICES) * 64 <= 16 * 1024);
}

/// Translate one entry point to GLSL ES 3.00, returning the generated source or
/// the writer's error.
fn translate(
    module: &naga::Module,
    info: &ModuleInfo,
    entry_point: &str,
    stage: naga::ShaderStage,
) -> Result<String, String> {
    let options = glsl::Options {
        // WebGL2 compiles GLSL ES 3.00; `is_webgl` selects the stricter
        // embedded feature set wgpu uses for a browser context.
        version: glsl::Version::Embedded {
            version: 300,
            is_webgl: true,
        },
        ..Default::default()
    };
    let pipeline = glsl::PipelineOptions {
        shader_stage: stage,
        entry_point: entry_point.to_string(),
        multiview: None,
    };
    let mut output = String::new();
    let mut writer = glsl::Writer::new(
        &mut output,
        module,
        info,
        &options,
        &pipeline,
        naga::proc::BoundsCheckPolicies::default(),
    )
    .map_err(|error| format!("GLSL ES 3.00 translation failed: {error}"))?;
    writer
        .write()
        .map_err(|error| format!("GLSL ES 3.00 output failed: {error}"))?;
    assert!(
        output.contains("#version 300 es"),
        "expected a GLSL ES 3.00 header, got:\n{output}"
    );
    Ok(output)
}
