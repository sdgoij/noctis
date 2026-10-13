//! The windowed inspector: editing every component of the selected entity.
//!
//! The inspector reads components through [`Editor::component`] and writes them
//! back with [`Editor::insert_component`] / [`Editor::edit_component`], so each
//! edit is one undo step and fields the command path would reset (a light's
//! `shadow_bias`, a collider's `rotation`, a body's damping) survive editing.
//! Material assets are edited in place through the asset registry.

use ecs::assets::{AlphaMode, AssetHandle, AssetId, Assets, Material, TextureFilter};
use ecs::components::{
    Active, Animator, AudioSource, BodyType, Camera, Collider, ColliderShape, Light, LightKind,
    MeshRenderer, Projection, RigidBody, Script, SkinnedMeshRenderer, Text, TextAnchor, Visible,
};
use ecs::reflect::{Field, FieldKind};
use ecs::world::Entity;
use math::{Quat, Vec3, degrees, radians, vec3};
use platform::egui;
use serde_json::{Value, json};

use crate::command::ComponentName;
use crate::editor::Editor;

/// Components the inspector can add.
const ADDABLE: [ComponentName; 10] = [
    ComponentName::Camera,
    ComponentName::Light,
    ComponentName::MeshRenderer,
    ComponentName::RigidBody,
    ComponentName::Collider,
    ComponentName::AudioSource,
    ComponentName::Script,
    ComponentName::Text,
    ComponentName::Active,
    ComponentName::Visible,
];

/// A starter body for a new inline script.
const NEW_SCRIPT: &str =
    "function on_start(entity, dt) {\n}\n\nfunction on_update(entity, dt) {\n}\n";

/// Inspector UI state that must persist between frames.
#[derive(Debug, Default)]
pub(crate) struct InspectorState {
    /// The kind selected in the "add component" combo.
    add_index: usize,
    /// Path typed into the glTF import field.
    gltf_path: String,
    /// Path typed into the texture import field.
    texture_path: String,
    /// Whether an imported texture is treated as an sRGB color map.
    texture_srgb: bool,
    /// The last import error, if any.
    import_error: Option<String>,
}

/// Draw the whole inspector for `entity`.
///
/// Returns an entity to select (the root of a freshly imported glTF, so the new
/// object is focused), or `None`.
pub(crate) fn show(
    editor: &mut Editor,
    entity: Entity,
    state: &mut InspectorState,
    ui: &mut egui::Ui,
) -> Option<Entity> {
    name_row(editor, entity, ui);
    flags_row(editor, entity, ui);
    transform_section(editor, entity, ui);

    if editor.has_component(entity, ComponentName::Camera) {
        camera_section(editor, entity, ui);
    }
    if editor.has_component(entity, ComponentName::Light) {
        light_section(editor, entity, ui);
    }
    if editor.has_component(entity, ComponentName::MeshRenderer) {
        mesh_renderer_section(editor, entity, ui);
    }
    if editor.has_component(entity, ComponentName::RigidBody) {
        rigid_body_section(editor, entity, ui);
    }
    if editor.has_component(entity, ComponentName::Collider) {
        collider_section(editor, entity, ui);
    }
    if editor.has_component(entity, ComponentName::AudioSource) {
        audio_source_section(editor, entity, ui);
    }
    if editor.has_component(entity, ComponentName::Script) {
        script_section(editor, entity, ui);
    }
    if editor.has_component(entity, ComponentName::Text) {
        text_section(editor, entity, ui);
    }

    custom_sections(editor, entity, ui);

    add_component_row(editor, entity, state, ui);
    import_section(editor, state, ui)
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

fn name_row(editor: &mut Editor, entity: Entity, ui: &mut egui::Ui) {
    let mut name = editor.name_of(entity).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label("Name");
        if ui.text_edit_singleline(&mut name).changed() {
            let _ = editor.rename(entity, name);
        }
    });
}

fn flags_row(editor: &mut Editor, entity: Entity, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        if editor.has_component(entity, ComponentName::Active) {
            let mut active = editor.component::<Active>(entity).is_none_or(|a| a.0);
            if ui.checkbox(&mut active, "Active").changed() {
                let _ = editor.set_active(entity, active);
            }
        }
        if editor.has_component(entity, ComponentName::Visible) {
            let mut visible = editor.component::<Visible>(entity).is_none_or(|v| v.0);
            if ui.checkbox(&mut visible, "Visible").changed() {
                let _ = editor.set_visible(entity, visible);
            }
        }
    });
}

fn transform_section(editor: &mut Editor, entity: Entity, ui: &mut egui::Ui) {
    ui.separator();
    ui.label(egui::RichText::new("Transform").strong());

    let mut translation = editor
        .translation(entity)
        .unwrap_or_else(|| vec3(0.0, 0.0, 0.0));
    if vec3_row(ui, "Position", &mut translation, 0.05) {
        let _ = editor.set_translation(entity, translation);
    }

    let mut rotation = editor
        .rotation_euler_degrees(entity)
        .unwrap_or_else(|| vec3(0.0, 0.0, 0.0));
    if vec3_row(ui, "Rotation", &mut rotation, 0.5) {
        let _ = editor.set_rotation_euler(entity, rotation);
    }

    let mut scale = editor.scale(entity).unwrap_or_else(|| vec3(1.0, 1.0, 1.0));
    if vec3_row(ui, "Scale", &mut scale, 0.02) {
        let _ = editor.set_scale(entity, scale);
    }
}

// ---------------------------------------------------------------------------
// Component sections
// ---------------------------------------------------------------------------

fn camera_section(editor: &mut Editor, entity: Entity, ui: &mut egui::Ui) {
    let Some(mut camera) = editor.component::<Camera>(entity).copied() else {
        return;
    };
    if section_header(ui, "Camera") {
        let _ = editor.remove_component(entity, ComponentName::Camera);
        return;
    }

    let mut changed = false;
    let is_perspective = matches!(camera.projection, Projection::Perspective { .. });
    let mut want_perspective = is_perspective;
    ui.horizontal(|ui| {
        ui.selectable_value(&mut want_perspective, true, "Perspective");
        ui.selectable_value(&mut want_perspective, false, "Orthographic");
    });
    if want_perspective != is_perspective {
        let near = camera.projection.near();
        let far = camera.projection.far();
        camera.projection = if want_perspective {
            Projection::Perspective {
                fov_y: radians(60.0),
                near,
                far,
            }
        } else {
            Projection::Orthographic {
                height: 10.0,
                near,
                far,
            }
        };
        changed = true;
    }
    match &mut camera.projection {
        Projection::Perspective { fov_y, near, far } => {
            let mut fov_deg = degrees(*fov_y);
            ui.horizontal(|ui| {
                ui.label("FOV°");
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut fov_deg)
                            .speed(0.5)
                            .range(1.0..=179.0),
                    )
                    .changed();
            });
            *fov_y = radians(fov_deg);
            changed |= pair_row(ui, "Near/Far", near, far, 0.05);
        }
        Projection::Orthographic { height, near, far } => {
            changed |= f32_row(ui, "Height", height, 0.1);
            changed |= pair_row(ui, "Near/Far", near, far, 0.05);
        }
    }
    ui.horizontal(|ui| {
        changed |= ui.checkbox(&mut camera.is_active, "Active").changed();
        ui.label("Priority");
        changed |= ui
            .add(egui::DragValue::new(&mut camera.priority).speed(0.1))
            .changed();
    });
    changed |= rgba_row(ui, "Clear color", &mut camera.clear_color);
    changed |= ui.checkbox(&mut camera.hdr, "HDR").changed();

    if changed {
        editor.edit_component::<Camera>(entity, |c| *c = camera);
    }
}

fn light_section(editor: &mut Editor, entity: Entity, ui: &mut egui::Ui) {
    let Some(mut light) = editor.component::<Light>(entity).copied() else {
        return;
    };
    if section_header(ui, "Light") {
        let _ = editor.remove_component(entity, ComponentName::Light);
        return;
    }

    let mut changed = false;
    let kind = match light.kind {
        LightKind::Directional { .. } => 0u8,
        LightKind::Point { .. } => 1,
        LightKind::Spot { .. } => 2,
    };
    let mut want = kind;
    ui.horizontal(|ui| {
        ui.selectable_value(&mut want, 0, "Directional");
        ui.selectable_value(&mut want, 1, "Point");
        ui.selectable_value(&mut want, 2, "Spot");
    });
    if want != kind {
        light.kind = match want {
            0 => LightKind::Directional {
                direction: vec3(0.0, -1.0, 0.0),
            },
            1 => LightKind::Point { range: 10.0 },
            _ => LightKind::Spot {
                direction: vec3(0.0, -1.0, 0.0),
                inner_angle: radians(20.0),
                outer_angle: radians(30.0),
                range: 10.0,
            },
        };
        changed = true;
    }
    match &mut light.kind {
        LightKind::Directional { direction } => {
            changed |= vec3_row(ui, "Direction", direction, 0.01);
        }
        LightKind::Point { range } => changed |= f32_row(ui, "Range", range, 0.1),
        LightKind::Spot {
            direction,
            inner_angle,
            outer_angle,
            range,
        } => {
            changed |= vec3_row(ui, "Direction", direction, 0.01);
            let mut inner = degrees(*inner_angle);
            let mut outer = degrees(*outer_angle);
            ui.horizontal(|ui| {
                ui.label("Angles°");
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut inner)
                            .speed(0.5)
                            .range(0.0..=89.0),
                    )
                    .changed();
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut outer)
                            .speed(0.5)
                            .range(0.0..=89.0),
                    )
                    .changed();
            });
            *inner_angle = radians(inner);
            *outer_angle = radians(outer);
            changed |= f32_row(ui, "Range", range, 0.1);
        }
    }

    let mut rgb = [light.color.x, light.color.y, light.color.z];
    if rgb_row(ui, "Color", &mut rgb) {
        light.color = vec3(rgb[0], rgb[1], rgb[2]);
        changed = true;
    }
    changed |= f32_row(ui, "Intensity", &mut light.intensity, 0.05);
    changed |= ui
        .checkbox(&mut light.cast_shadows, "Cast shadows")
        .changed();
    changed |= f32_row(ui, "Shadow bias", &mut light.shadow_bias, 0.0001);

    if changed {
        editor.edit_component::<Light>(entity, |l| *l = light);
    }
}

fn mesh_renderer_section(editor: &mut Editor, entity: Entity, ui: &mut egui::Ui) {
    let Some(mut renderer) = editor.component::<MeshRenderer>(entity).copied() else {
        return;
    };
    if section_header(ui, "Mesh Renderer") {
        let _ = editor.remove_component(entity, ComponentName::MeshRenderer);
        return;
    }

    let meshes = asset_list(&editor.assets().meshes);
    let materials = asset_list(&editor.assets().materials);
    let mut changed = false;
    if let Some(id) = asset_combo(ui, "mesh", "Mesh", renderer.mesh.id(), &meshes) {
        renderer.mesh = AssetHandle::new(id);
        changed = true;
    }
    if let Some(id) = asset_combo(
        ui,
        "material",
        "Material",
        renderer.material.id(),
        &materials,
    ) {
        renderer.material = AssetHandle::new(id);
        changed = true;
    }
    changed |= ui
        .checkbox(&mut renderer.cast_shadows, "Cast shadows")
        .changed();
    changed |= ui
        .checkbox(&mut renderer.receive_shadows, "Receive shadows")
        .changed();
    if changed {
        editor.edit_component::<MeshRenderer>(entity, |r| *r = renderer);
    }

    if !renderer.material.is_none() {
        material_editor(editor, renderer.material, ui);
    }
}

fn rigid_body_section(editor: &mut Editor, entity: Entity, ui: &mut egui::Ui) {
    let Some(mut body) = editor.component::<RigidBody>(entity).copied() else {
        return;
    };
    if section_header(ui, "Rigid Body") {
        let _ = editor.remove_component(entity, ComponentName::RigidBody);
        return;
    }

    let mut changed = false;
    let kind = body.body_type;
    let mut want = kind;
    ui.horizontal(|ui| {
        ui.selectable_value(&mut want, BodyType::Dynamic, "Dynamic");
        ui.selectable_value(&mut want, BodyType::Fixed, "Fixed");
        ui.selectable_value(&mut want, BodyType::Kinematic, "Kinematic");
    });
    if want != kind {
        body.body_type = want;
        changed = true;
    }
    changed |= f32_row(ui, "Mass", &mut body.mass, 0.05);
    changed |= unit_row(ui, "Restitution", &mut body.restitution);
    changed |= f32_row(ui, "Friction", &mut body.friction, 0.02);
    changed |= f32_row(ui, "Gravity scale", &mut body.gravity_scale, 0.05);
    changed |= vec3_row(ui, "Linear velocity", &mut body.linear_velocity, 0.05);
    changed |= vec3_row(ui, "Angular velocity", &mut body.angular_velocity, 0.02);
    changed |= f32_row(ui, "Linear damping", &mut body.linear_damping, 0.02);
    changed |= f32_row(ui, "Angular damping", &mut body.angular_damping, 0.02);
    changed |= ui.checkbox(&mut body.ccd, "CCD").changed();

    if changed {
        editor.edit_component::<RigidBody>(entity, |b| *b = body);
    }
}

fn collider_section(editor: &mut Editor, entity: Entity, ui: &mut egui::Ui) {
    let Some(mut collider) = editor.component::<Collider>(entity).copied() else {
        return;
    };
    if section_header(ui, "Collider") {
        let _ = editor.remove_component(entity, ComponentName::Collider);
        return;
    }

    let mut changed = false;
    let kind = match collider.shape {
        ColliderShape::Cuboid { .. } => 0u8,
        ColliderShape::Ball { .. } => 1,
        ColliderShape::Capsule { .. } => 2,
        ColliderShape::Cylinder { .. } => 3,
    };
    let mut want = kind;
    ui.horizontal(|ui| {
        ui.selectable_value(&mut want, 0, "Cuboid");
        ui.selectable_value(&mut want, 1, "Ball");
        ui.selectable_value(&mut want, 2, "Capsule");
        ui.selectable_value(&mut want, 3, "Cylinder");
    });
    if want != kind {
        collider.shape = match want {
            0 => ColliderShape::cube(),
            1 => ColliderShape::sphere(),
            2 => ColliderShape::Capsule {
                half_height: 0.5,
                radius: 0.5,
            },
            _ => ColliderShape::Cylinder {
                half_height: 0.5,
                radius: 0.5,
            },
        };
        changed = true;
    }
    match &mut collider.shape {
        ColliderShape::Cuboid { half_extents } => {
            changed |= vec3_row(ui, "Half extents", half_extents, 0.02);
        }
        ColliderShape::Ball { radius } => changed |= f32_row(ui, "Radius", radius, 0.02),
        ColliderShape::Capsule {
            half_height,
            radius,
        }
        | ColliderShape::Cylinder {
            half_height,
            radius,
        } => {
            changed |= f32_row(ui, "Half height", half_height, 0.02);
            changed |= f32_row(ui, "Radius", radius, 0.02);
        }
    }
    changed |= vec3_row(ui, "Offset", &mut collider.offset, 0.02);
    changed |= quat_row(ui, "Rotation", &mut collider.rotation, 0.5);
    changed |= ui.checkbox(&mut collider.is_sensor, "Sensor").changed();
    changed |= f32_row(ui, "Friction", &mut collider.friction, 0.02);
    changed |= unit_row(ui, "Restitution", &mut collider.restitution);

    if changed {
        editor.edit_component::<Collider>(entity, |c| *c = collider);
    }
}

fn audio_source_section(editor: &mut Editor, entity: Entity, ui: &mut egui::Ui) {
    let Some(mut source) = editor.component::<AudioSource>(entity).copied() else {
        return;
    };
    if section_header(ui, "Audio Source") {
        let _ = editor.remove_component(entity, ComponentName::AudioSource);
        return;
    }

    let clips = asset_list(&editor.assets().audio);
    let mut changed = false;
    if let Some(id) = asset_combo(ui, "clip", "Clip", source.clip.id(), &clips) {
        source.clip = AssetHandle::new(id);
        changed = true;
    }
    changed |= f32_row(ui, "Volume", &mut source.volume, 0.02);
    changed |= f32_row(ui, "Pitch", &mut source.pitch, 0.02);
    ui.horizontal(|ui| {
        changed |= ui.checkbox(&mut source.looped, "Looped").changed();
        changed |= ui.checkbox(&mut source.playing, "Playing").changed();
        changed |= ui.checkbox(&mut source.spatial, "Spatial").changed();
    });
    changed |= f32_row(ui, "Max distance", &mut source.max_distance, 0.5);
    changed |= unit_row(ui, "Rolloff", &mut source.rolloff);

    if changed {
        editor.edit_component::<AudioSource>(entity, |a| *a = source);
    }
}

/// The nine text anchors with their UI labels.
const ANCHORS: [(TextAnchor, &str); 9] = [
    (TextAnchor::Center, "Center"),
    (TextAnchor::TopLeft, "Top-left"),
    (TextAnchor::TopCenter, "Top-center"),
    (TextAnchor::TopRight, "Top-right"),
    (TextAnchor::MiddleLeft, "Middle-left"),
    (TextAnchor::MiddleRight, "Middle-right"),
    (TextAnchor::BottomLeft, "Bottom-left"),
    (TextAnchor::BottomCenter, "Bottom-center"),
    (TextAnchor::BottomRight, "Bottom-right"),
];

fn anchor_name(anchor: TextAnchor) -> &'static str {
    ANCHORS
        .iter()
        .find(|(a, _)| *a == anchor)
        .map_or("Center", |(_, name)| name)
}

fn text_section(editor: &mut Editor, entity: Entity, ui: &mut egui::Ui) {
    let Some(mut text) = editor.component::<Text>(entity).cloned() else {
        return;
    };
    if section_header(ui, "Text") {
        let _ = editor.remove_component(entity, ComponentName::Text);
        return;
    }

    let mut changed = false;
    changed |= ui
        .add(
            egui::TextEdit::multiline(&mut text.text)
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        )
        .changed();

    let fonts = asset_list(&editor.assets().fonts);
    if let Some(id) = asset_combo(ui, "text_font", "Font", text.font.id(), &fonts) {
        text.font = AssetHandle::new(id);
        changed = true;
    }
    changed |= f32_row(ui, "Size", &mut text.size, 0.02);
    changed |= rgba_row(ui, "Color", &mut text.color);
    ui.horizontal(|ui| {
        ui.label("Anchor");
        egui::ComboBox::from_id_salt("inspector_text_anchor")
            .selected_text(anchor_name(text.anchor))
            .show_ui(ui, |ui| {
                for (anchor, name) in ANCHORS {
                    changed |= ui
                        .selectable_value(&mut text.anchor, anchor, name)
                        .changed();
                }
            });
    });
    changed |= ui.checkbox(&mut text.depth_test, "Depth test").changed();

    if changed {
        editor.edit_component::<Text>(entity, |t| *t = text);
    }
}

fn script_section(editor: &mut Editor, entity: Entity, ui: &mut egui::Ui) {
    let Some(mut script) = editor.component::<Script>(entity).cloned() else {
        return;
    };
    if section_header(ui, "Script") {
        let _ = editor.remove_component(entity, ComponentName::Script);
        return;
    }

    let mut changed = false;
    changed |= ui.checkbox(&mut script.enabled, "Enabled").changed();

    let inline = script.source.is_some();
    let mut want_inline = inline;
    ui.horizontal(|ui| {
        ui.selectable_value(&mut want_inline, true, "Inline");
        ui.selectable_value(&mut want_inline, false, "File path");
    });
    if want_inline != inline {
        if want_inline {
            script.source = Some(script.source.clone().unwrap_or_default());
            script.path = None;
        } else {
            script.path = Some(script.path.clone().unwrap_or_default());
            script.source = None;
        }
        changed = true;
    }

    if let Some(source) = script.source.as_mut() {
        changed |= ui
            .add(
                egui::TextEdit::multiline(source)
                    .desired_rows(6)
                    .desired_width(f32::INFINITY)
                    .code_editor(),
            )
            .changed();
    } else if let Some(path) = script.path.as_mut() {
        ui.horizontal(|ui| {
            ui.label("Path");
            changed |= ui.text_edit_singleline(path).changed();
        });
    }

    if changed {
        editor.edit_component::<Script>(entity, |s| *s = script);
    }
}

// ---------------------------------------------------------------------------
// Custom (plugin) components
// ---------------------------------------------------------------------------

/// Draw a schema-driven form for every registered custom component on `entity`.
fn custom_sections(editor: &mut Editor, entity: Entity, ui: &mut egui::Ui) {
    for name in editor.custom_components() {
        if editor.has_custom_component(entity, name) {
            custom_section(editor, entity, name, ui);
        }
    }
}

/// One custom component: a header with a Remove button, then a widget per
/// reflected field.
fn custom_section(editor: &mut Editor, entity: Entity, name: &str, ui: &mut egui::Ui) {
    if section_header(ui, name) {
        editor.remove_custom_component(entity, name);
        return;
    }
    let Some(fields) = editor.custom_component_fields(name) else {
        return;
    };
    for field in fields {
        let Some(current) = editor
            .custom_component_json(entity, name)
            .and_then(|data| data.get(field.name).cloned())
        else {
            continue;
        };
        if let Some(value) = custom_field(ui, name, field, current) {
            editor.set_custom_component_field(entity, name, field.name, value);
        }
    }
}

/// A widget for one reflected field. Returns the new JSON value when it changed.
fn custom_field(ui: &mut egui::Ui, component: &str, field: Field, current: Value) -> Option<Value> {
    match field.kind {
        FieldKind::Number => {
            let mut value = current.as_f64().unwrap_or(0.0) as f32;
            f32_row(ui, field.name, &mut value, 0.01).then(|| json!(value))
        }
        FieldKind::Integer => {
            let mut value = current.as_i64().unwrap_or(0);
            let changed = ui
                .horizontal(|ui| {
                    ui.label(field.name);
                    ui.add(egui::DragValue::new(&mut value)).changed()
                })
                .inner;
            changed.then(|| json!(value))
        }
        FieldKind::Boolean => {
            let mut value = current.as_bool().unwrap_or(false);
            ui.checkbox(&mut value, field.name)
                .changed()
                .then(|| json!(value))
        }
        FieldKind::Text | FieldKind::Asset => {
            let mut value = current.as_str().unwrap_or_default().to_string();
            let changed = ui
                .horizontal(|ui| {
                    ui.label(field.name);
                    ui.add(egui::TextEdit::singleline(&mut value).desired_width(f32::INFINITY))
                        .changed()
                })
                .inner;
            changed.then(|| json!(value))
        }
        FieldKind::Vec3 => {
            let mut value = json_vec3(&current);
            vec3_row(ui, field.name, &mut value, 0.01).then(|| json!([value.x, value.y, value.z]))
        }
        FieldKind::Color => {
            let mut value = json_vec4(&current);
            rgba_row(ui, field.name, &mut value).then(|| json!(value))
        }
        FieldKind::Enum(options) => {
            let value = current.as_str().unwrap_or_default().to_string();
            let mut chosen = None;
            ui.horizontal(|ui| {
                ui.label(field.name);
                egui::ComboBox::from_id_salt(format!("custom_{component}_{}", field.name))
                    .selected_text(value.clone())
                    .show_ui(ui, |ui| {
                        for option in options {
                            if ui.selectable_label(value == *option, *option).clicked() {
                                chosen = Some((*option).to_string());
                            }
                        }
                    });
            });
            chosen.map(|choice| json!(choice))
        }
    }
}

fn json_vec3(value: &Value) -> Vec3 {
    let at = |index: usize| json_at(value, index);
    vec3(at(0), at(1), at(2))
}

fn json_vec4(value: &Value) -> [f32; 4] {
    let at = |index: usize| json_at(value, index);
    [at(0), at(1), at(2), at(3)]
}

fn json_at(value: &Value, index: usize) -> f32 {
    value
        .as_array()
        .and_then(|array| array.get(index))
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as f32
}

// ---------------------------------------------------------------------------
// Material editor (textures live on the material asset)
// ---------------------------------------------------------------------------

fn material_editor(editor: &mut Editor, handle: AssetHandle<Material>, ui: &mut egui::Ui) {
    let textures = asset_list(&editor.assets().textures);
    let Some(mut material) = editor.assets().materials.get(handle).cloned() else {
        return;
    };

    let mut changed = false;
    egui::CollapsingHeader::new("Material")
        .default_open(true)
        .show(ui, |ui| {
            changed |= rgba_row(ui, "Base color", &mut material.base_color);
            changed |= unit_row(ui, "Metallic", &mut material.metallic);
            changed |= unit_row(ui, "Roughness", &mut material.roughness);
            let mut emissive = material.emissive;
            if rgb_row(ui, "Emissive", &mut emissive) {
                material.emissive = emissive;
                changed = true;
            }
            changed |= f32_row(
                ui,
                "Emissive strength",
                &mut material.emissive_strength,
                0.05,
            );

            let mode = material.alpha_mode;
            let mut want = mode;
            ui.horizontal(|ui| {
                ui.selectable_value(&mut want, AlphaMode::Opaque, "Opaque");
                ui.selectable_value(&mut want, AlphaMode::Mask, "Mask");
                ui.selectable_value(&mut want, AlphaMode::Blend, "Blend");
            });
            if want != mode {
                material.alpha_mode = want;
                changed = true;
            }
            changed |= ui
                .checkbox(&mut material.double_sided, "Double sided")
                .changed();
            changed |= ui.checkbox(&mut material.unlit, "Unlit").changed();
            let filter = material.filter;
            let mut want_filter = filter;
            ui.horizontal(|ui| {
                ui.selectable_value(&mut want_filter, TextureFilter::Linear, "Linear");
                ui.selectable_value(&mut want_filter, TextureFilter::Nearest, "Nearest");
            });
            if want_filter != filter {
                material.filter = want_filter;
                changed = true;
            }

            if let Some(id) = asset_combo(
                ui,
                "tex_base",
                "Base color tex",
                material.base_color_texture.id(),
                &textures,
            ) {
                material.base_color_texture = AssetHandle::new(id);
                changed = true;
            }
            if let Some(id) = asset_combo(
                ui,
                "tex_normal",
                "Normal tex",
                material.normal_texture.id(),
                &textures,
            ) {
                material.normal_texture = AssetHandle::new(id);
                changed = true;
            }
            if let Some(id) = asset_combo(
                ui,
                "tex_mr",
                "Metallic/rough tex",
                material.metallic_roughness_texture.id(),
                &textures,
            ) {
                material.metallic_roughness_texture = AssetHandle::new(id);
                changed = true;
            }
            if let Some(id) = asset_combo(
                ui,
                "tex_emissive",
                "Emissive tex",
                material.emissive_texture.id(),
                &textures,
            ) {
                material.emissive_texture = AssetHandle::new(id);
                changed = true;
            }
            if let Some(id) = asset_combo(
                ui,
                "tex_occlusion",
                "Occlusion tex",
                material.occlusion_texture.id(),
                &textures,
            ) {
                material.occlusion_texture = AssetHandle::new(id);
                changed = true;
            }
        });

    if changed && let Some(entry) = editor.assets_mut().materials.get_mut(handle) {
        *entry = material;
    }
}

// ---------------------------------------------------------------------------
// Add / remove
// ---------------------------------------------------------------------------

fn section_header(ui: &mut egui::Ui, title: &str) -> bool {
    ui.separator();
    let mut removed = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).strong());
        if ui
            .small_button("Remove")
            .on_hover_text("Remove this component")
            .clicked()
        {
            removed = true;
        }
    });
    removed
}

fn add_component_row(
    editor: &mut Editor,
    entity: Entity,
    state: &mut InspectorState,
    ui: &mut egui::Ui,
) {
    ui.separator();
    // The combo lists the built-in kinds followed by the project's registered
    // custom components, so a plugin's components are addable like any other.
    let custom = editor.custom_components();
    let total = ADDABLE.len() + custom.len();
    if state.add_index >= total {
        state.add_index = 0;
    }
    let selected = if state.add_index < ADDABLE.len() {
        ADDABLE[state.add_index].as_str().to_string()
    } else {
        custom[state.add_index - ADDABLE.len()].to_string()
    };

    ui.horizontal(|ui| {
        ui.label("Add component");
        egui::ComboBox::from_id_salt("inspector_add_component")
            .selected_text(selected)
            .show_ui(ui, |ui| {
                for (index, name) in ADDABLE.iter().enumerate() {
                    ui.selectable_value(&mut state.add_index, index, name.as_str());
                }
                for (offset, name) in custom.iter().enumerate() {
                    ui.selectable_value(&mut state.add_index, ADDABLE.len() + offset, *name);
                }
            });
        if ui.button("Add").clicked() {
            if state.add_index < ADDABLE.len() {
                add_component(editor, entity, ADDABLE[state.add_index]);
            } else if let Some(name) = custom.get(state.add_index - ADDABLE.len()) {
                editor.insert_custom_component(entity, name);
            }
        }
    });
}

fn add_component(editor: &mut Editor, entity: Entity, name: ComponentName) {
    match name {
        ComponentName::Camera => {
            editor.insert_component(entity, Camera::default());
        }
        ComponentName::Light => {
            editor.insert_component(entity, Light::default());
        }
        ComponentName::MeshRenderer => {
            // Start from the first registered mesh/material so it renders at once.
            let mesh = editor
                .assets()
                .meshes
                .iter()
                .next()
                .map(|(id, ..)| AssetHandle::new(id))
                .unwrap_or(AssetHandle::NONE);
            let material = editor
                .assets()
                .materials
                .iter()
                .next()
                .map(|(id, ..)| AssetHandle::new(id))
                .unwrap_or(AssetHandle::NONE);
            editor.insert_component(
                entity,
                MeshRenderer {
                    mesh,
                    material,
                    ..MeshRenderer::default()
                },
            );
        }
        ComponentName::SkinnedMeshRenderer => {
            let mesh = editor
                .assets()
                .meshes
                .iter()
                .next()
                .map(|(id, ..)| AssetHandle::new(id))
                .unwrap_or(AssetHandle::NONE);
            let skin = editor
                .assets()
                .skins
                .iter()
                .next()
                .map(|(id, ..)| AssetHandle::new(id))
                .unwrap_or(AssetHandle::NONE);
            let material = editor
                .assets()
                .materials
                .iter()
                .next()
                .map(|(id, ..)| AssetHandle::new(id))
                .unwrap_or(AssetHandle::NONE);
            editor.insert_component(
                entity,
                SkinnedMeshRenderer {
                    mesh,
                    skin,
                    material,
                    ..SkinnedMeshRenderer::default()
                },
            );
        }
        ComponentName::Animator => {
            let clip = editor
                .assets()
                .animations
                .iter()
                .next()
                .map(|(id, ..)| AssetHandle::new(id))
                .unwrap_or(AssetHandle::NONE);
            editor.insert_component(entity, Animator::new(clip));
        }
        ComponentName::RigidBody => {
            editor.insert_component(entity, RigidBody::default());
        }
        ComponentName::Collider => {
            editor.insert_component(entity, Collider::default());
        }
        ComponentName::AudioSource => {
            editor.insert_component(entity, AudioSource::default());
        }
        ComponentName::Script => {
            editor.insert_component(entity, Script::from_source(NEW_SCRIPT));
        }
        ComponentName::Text => {
            editor.insert_component(entity, Text::new("Text"));
        }
        ComponentName::Active => {
            editor.insert_component(entity, Active(true));
        }
        ComponentName::Visible => {
            editor.insert_component(entity, Visible(true));
        }
        ComponentName::LocalTransform | ComponentName::GlobalTransform | ComponentName::Name => {}
    }
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

/// A path field + button to import a glTF model or a texture file. Returns the
/// root entity of an imported glTF so the caller can select it.
fn import_section(
    editor: &mut Editor,
    state: &mut InspectorState,
    ui: &mut egui::Ui,
) -> Option<Entity> {
    let mut select = None;
    egui::CollapsingHeader::new("Import asset")
        .default_open(false)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("glTF");
                ui.add(
                    egui::TextEdit::singleline(&mut state.gltf_path)
                        .desired_width(180.0)
                        .hint_text("models/model.glb"),
                );
                if ui.button("Import model").clicked() {
                    match editor.import_gltf(&state.gltf_path) {
                        Ok(entities) => {
                            state.import_error = None;
                            select = entities.first().copied();
                        }
                        Err(error) => state.import_error = Some(error.to_string()),
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Texture");
                ui.add(
                    egui::TextEdit::singleline(&mut state.texture_path)
                        .desired_width(180.0)
                        .hint_text("textures/albedo.png"),
                );
                if ui.button("Import texture").clicked() {
                    match editor.import_texture(&state.texture_path, state.texture_srgb) {
                        Ok(_) => state.import_error = None,
                        Err(error) => state.import_error = Some(error.to_string()),
                    }
                }
            });
            ui.checkbox(&mut state.texture_srgb, "Texture is sRGB (color map)");
            if let Some(error) = &state.import_error {
                ui.colored_label(egui::Color32::from_rgb(220, 80, 80), error);
            }
        });
    select
}

// ---------------------------------------------------------------------------
// Widget helpers
// ---------------------------------------------------------------------------

fn vec3_row(ui: &mut egui::Ui, label: &str, value: &mut Vec3, speed: f64) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        let x = ui
            .add(egui::DragValue::new(&mut value.x).speed(speed))
            .changed();
        let y = ui
            .add(egui::DragValue::new(&mut value.y).speed(speed))
            .changed();
        let z = ui
            .add(egui::DragValue::new(&mut value.z).speed(speed))
            .changed();
        x || y || z
    })
    .inner
}

fn f32_row(ui: &mut egui::Ui, label: &str, value: &mut f32, speed: f64) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::DragValue::new(value).speed(speed)).changed()
    })
    .inner
}

fn unit_row(ui: &mut egui::Ui, label: &str, value: &mut f32) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::DragValue::new(value).speed(0.005).range(0.0..=1.0))
            .changed()
    })
    .inner
}

fn pair_row(ui: &mut egui::Ui, label: &str, a: &mut f32, b: &mut f32, speed: f64) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        let first = ui.add(egui::DragValue::new(a).speed(speed)).changed();
        let second = ui.add(egui::DragValue::new(b).speed(speed)).changed();
        first || second
    })
    .inner
}

fn rgba_row(ui: &mut egui::Ui, label: &str, value: &mut [f32; 4]) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.color_edit_button_rgba_unmultiplied(value).changed()
    })
    .inner
}

fn rgb_row(ui: &mut egui::Ui, label: &str, value: &mut [f32; 3]) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.color_edit_button_rgb(value).changed()
    })
    .inner
}

fn quat_row(ui: &mut egui::Ui, label: &str, quaternion: &mut Quat, speed: f64) -> bool {
    let (roll, pitch, yaw) = quaternion.to_euler_xyz();
    let mut euler = vec3(degrees(roll), degrees(pitch), degrees(yaw));
    let changed = vec3_row(ui, label, &mut euler, speed);
    if changed {
        *quaternion = Quat::from_euler_xyz(radians(euler.x), radians(euler.y), radians(euler.z));
    }
    changed
}

/// `(id, display name)` for every asset in a store.
fn asset_list<T>(assets: &Assets<T>) -> Vec<(AssetId, String)> {
    assets
        .iter()
        .map(|(id, path, _)| {
            let name = path
                .map(str::to_string)
                .unwrap_or_else(|| format!("#{}", id.0));
            (id, name)
        })
        .collect()
}

/// A combo box of assets. Returns the newly chosen id when the selection changed.
fn asset_combo(
    ui: &mut egui::Ui,
    salt: &str,
    label: &str,
    current: AssetId,
    items: &[(AssetId, String)],
) -> Option<AssetId> {
    let selected = items
        .iter()
        .find(|(id, _)| *id == current)
        .map(|(_, name)| name.clone())
        .unwrap_or_else(|| "(none)".to_string());

    let mut chosen = None;
    ui.horizontal(|ui| {
        ui.label(label);
        egui::ComboBox::from_id_salt(ui.id().with(salt))
            .selected_text(selected)
            .width(200.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(current.is_none(), "(none)").clicked() {
                    chosen = Some(AssetId::NONE);
                }
                for (id, name) in items {
                    if ui.selectable_label(current == *id, name).clicked() {
                        chosen = Some(*id);
                    }
                }
            });
    });
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run one inspector frame for `entity` against a headless egui context.
    fn run_one_frame(editor: &mut Editor, entity: Entity, state: &mut InspectorState) {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            egui::Panel::right("inspector_test").show(ui, |ui| {
                show(editor, entity, state, ui);
            });
        });
        output.drop_without_applying_deltas();
    }

    #[test]
    fn adding_every_component_kind_works() {
        let mut editor = Editor::new();
        let entity = editor.spawn("E", None);
        for name in ADDABLE {
            add_component(&mut editor, entity, name);
        }
        for name in ADDABLE {
            assert!(editor.has_component(entity, name), "missing {name:?}");
        }
    }

    #[test]
    fn renders_an_entity_with_every_component() {
        let mut editor = Editor::new();
        let entity = editor.spawn("Thing", None);
        for name in ADDABLE {
            add_component(&mut editor, entity, name);
        }
        // Give the renderer a real material so the material editor runs too.
        let material = editor
            .assets()
            .materials
            .handle("$default")
            .expect("default");
        if let Some(renderer) = editor.component::<MeshRenderer>(entity).copied() {
            editor.insert_component(
                entity,
                MeshRenderer {
                    material,
                    ..renderer
                },
            );
        }

        let mut state = InspectorState::default();
        run_one_frame(&mut editor, entity, &mut state);
        assert_eq!(editor.entity_count(), 1);
    }

    #[test]
    fn renders_an_entity_with_nothing_selected() {
        let mut editor = Editor::new();
        let entity = editor.spawn("Bare", None);
        let mut state = InspectorState::default();
        run_one_frame(&mut editor, entity, &mut state);
        assert_eq!(editor.entity_count(), 1);
    }

    #[derive(Debug, Default, serde::Serialize, serde::Deserialize, ecs::Component)]
    struct Health {
        current: f32,
        #[field(boolean)]
        alive: bool,
    }

    fn registry_with_health() -> ecs::reflect::ComponentRegistry {
        let mut registry = ecs::reflect::ComponentRegistry::new();
        registry.register::<Health>();
        registry
    }

    #[test]
    fn lists_and_renders_a_custom_component() {
        let mut editor = Editor::with_component_registry(registry_with_health());
        let entity = editor.spawn("Hero", None);
        assert!(editor.insert_custom_component(entity, "Health"));

        // The Add-component menu lists registered custom names beside the built-ins.
        assert_eq!(editor.custom_components(), vec!["Health"]);

        // A frame renders the schema-driven form without panicking.
        let mut state = InspectorState::default();
        run_one_frame(&mut editor, entity, &mut state);
        assert_eq!(editor.entity_count(), 1);
    }
}
