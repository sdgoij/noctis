//! Integration test for the windowed editor's gizmo plumbing.
//!
//! The drag math is unit-tested in `editor::gizmo`; this test covers the wiring
//! the renderer depends on: a selected entity must hand the host a non-empty
//! gizmo, in every tool, placed at that entity's world position — and no
//! selection must hand it none.

use ecs::components::GlobalTransform;
use editor::Editor;
use editor::window::EditorDriver;
use platform::AppDriver;
use render::GizmoMode;

fn testscene_editor() -> Editor {
    let mut editor = Editor::new();
    let (scene, assets) = testscene::build();
    *editor.scene_mut() = scene;
    *editor.assets_mut() = assets;
    editor.scene_mut().update_transforms();
    editor
}

#[test]
fn no_selection_yields_no_gizmo() {
    let driver = EditorDriver::new(Editor::new());
    assert!(driver.gizmo().is_none());
}

#[test]
fn every_tool_yields_a_gizmo_at_the_entities_world_position() {
    let editor = testscene_editor();
    let info = editor
        .list_entities()
        .into_iter()
        .find(|info| info.components.contains(&"MeshRenderer"))
        .expect("the test scene contains a mesh entity");
    let expected = editor
        .scene()
        .world
        .get::<GlobalTransform>(info.entity)
        .expect("a test-scene entity has a world transform")
        .position();

    let mut driver = EditorDriver::new(editor);
    driver.select(info.entity);

    for (mode, has_triangles) in [
        (GizmoMode::Translate, true),
        (GizmoMode::Rotate, false),
        (GizmoMode::Scale, true),
    ] {
        driver.set_gizmo_mode(mode);
        assert_eq!(driver.gizmo_mode(), mode);

        let (geometry, transform) = driver
            .gizmo()
            .unwrap_or_else(|| panic!("{mode:?} shows a gizmo for a selected entity"));
        assert!(
            geometry.line_segment_count() >= 3,
            "{mode:?} should draw at least three handles"
        );
        assert_eq!(
            !geometry.triangles.is_empty(),
            has_triangles,
            "{mode:?} triangle-handle presence"
        );

        let placed = math::vec3(transform[3].x, transform[3].y, transform[3].z);
        assert!(
            math::vec3_approx_eq(placed, expected, 1e-4),
            "{mode:?} gizmo placed at {placed:?}, expected {expected:?}"
        );
    }
}
