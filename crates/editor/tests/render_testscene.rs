//! End-to-end render check for the all-features test scene.
//!
//! This is the test that proves the 3D pipeline actually rasterizes *visible*
//! geometry — not merely that a frame comes back the right size. It builds the
//! real [`testscene`] scene (every component, every primitive, all three light
//! shapes), renders it offscreen through the real GPU [`Renderer`], and asserts
//! that a meaningful portion of the frame differs from the empty-scene
//! background.
//!
//! It is self-calibrating: it first renders a camera-only scene to learn exactly
//! what the background looks like (which depends on the clear colour and the
//! tonemap curve), asserts that background is featureless, then requires the
//! test scene to add real detail on top of it. That makes the assertion robust
//! to exposure/tonemap changes without a magic brightness threshold.
//!
//! Like every GPU test in this workspace it skips cleanly when no adapter is
//! available (`RenderError::NoAdapter`) rather than failing on headless CI.

use render::{Frame, RenderError, Renderer, RendererConfig};

/// The channel-wise difference between two RGBA pixels.
fn diff(a: [u8; 4], b: [u8; 4]) -> u8 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

/// The RGBA pixels of a frame.
fn pixels(frame: &Frame) -> &[[u8; 4]] {
    frame.rgba.as_chunks::<4>().0
}

/// Count pixels differing from `background` by more than `threshold`.
fn count_differing(frame: &Frame, background: [u8; 4], threshold: u8) -> usize {
    pixels(frame)
        .iter()
        .filter(|pixel| diff(**pixel, background) > threshold)
        .count()
}

/// Create a headless renderer, or `None` when no adapter exists.
fn headless(width: u32, height: u32) -> Option<Renderer> {
    match Renderer::headless(RendererConfig {
        width,
        height,
        ..Default::default()
    }) {
        Ok(renderer) => Some(renderer),
        Err(RenderError::NoAdapter) => {
            eprintln!("skipping GPU test: no adapter");
            None
        }
        Err(other) => panic!("unexpected renderer error: {other}"),
    }
}

/// Render a camera-only scene and return its uniform background colour.
fn background_colour(renderer: &mut Renderer) -> [u8; 4] {
    let mut world = ecs::world::World::new();
    let camera = world.spawn_with(ecs::components::Camera::default());
    let assets = ecs::assets::AssetRegistry::with_defaults();
    let frame = renderer
        .render_frame(&world, &assets, camera)
        .expect("render a camera-only scene");

    let background = pixels(&frame)
        .first()
        .copied()
        .expect("a rendered frame has pixels");
    assert_eq!(
        count_differing(&frame, background, 1),
        0,
        "a scene with no geometry must render a featureless background"
    );
    background
}

#[test]
fn test_scene_renders_visible_geometry() {
    let Some(mut renderer) = headless(256, 256) else {
        return;
    };
    let background = background_colour(&mut renderer);

    let (scene, assets) = testscene::build();
    let camera = render::first_active_camera(&scene.world)
        .expect("the test scene provides an active camera");
    let frame = renderer
        .render_frame(&scene.world, &assets, camera)
        .expect("render the test scene");

    assert_eq!(frame.width, 256);
    assert_eq!(frame.height, 256);
    assert_eq!(frame.rgba.len(), 256 * 256 * 4);

    let total = (frame.width * frame.height) as usize;
    let detailed = count_differing(&frame, background, 12);
    assert!(
        detailed > total / 20,
        "expected the test scene to draw over the background in a visible portion \
         of the frame, but only {detailed}/{total} pixels differed from it"
    );
}

/// Keep the camera-only calibration honest: a `Frame` we build by hand must
/// round-trip through [`diff`] the way the GPU frames do.
#[test]
fn diff_is_per_channel_maximum() {
    let frame = Frame {
        width: 2,
        height: 1,
        rgba: vec![10, 20, 30, 255, 10, 20, 31, 255],
    };
    let px = |x| frame.pixel(x, 0).expect("in bounds");
    assert_eq!(diff(px(0), px(1)), 1);
    assert_eq!(diff(px(0), px(0)), 0);
}
