//! `#[derive(Component)]` generates the reflection a registered component needs:
//! `NAME`, the `fields` schema (with kinds inferred from the Rust types), and
//! `references` from the `#[field(asset)]` fields.

use ecs::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, PartialEq, Serialize, Deserialize, Component)]
struct Health {
    current: f32,
    max: f32,
    #[field(skip)]
    scratch: u8,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize, Component)]
#[component(name = "Turret")]
struct Weapon {
    damage: i32,
    active: bool,
    label: String,
    #[field(asset)]
    muzzle: Option<String>,
    #[field(options = "single, burst, auto")]
    fire_mode: String,
    offset: [f32; 3],
    tint: [f32; 4],
}

#[test]
fn names_are_the_struct_name_or_the_override() {
    assert_eq!(Health::NAME, "Health");
    assert_eq!(Weapon::NAME, "Turret");
}

#[test]
fn field_kinds_are_inferred_from_the_types() {
    let kinds: Vec<(&str, FieldKind)> = Health::fields()
        .iter()
        .map(|field| (field.name, field.kind))
        .collect();
    assert_eq!(
        kinds,
        vec![("current", FieldKind::Number), ("max", FieldKind::Number)]
    );
    // `scratch` is `#[field(skip)]`, so it does not appear.

    let kinds: Vec<(&str, FieldKind)> = Weapon::fields()
        .iter()
        .map(|field| (field.name, field.kind))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("damage", FieldKind::Integer),
            ("active", FieldKind::Boolean),
            ("label", FieldKind::Text),
            ("muzzle", FieldKind::Asset),
            ("fire_mode", FieldKind::Enum(&["single", "burst", "auto"])),
            ("offset", FieldKind::Vec3),
            ("tint", FieldKind::Color),
        ]
    );
}

#[test]
fn asset_fields_become_references() {
    let weapon = Weapon {
        muzzle: Some("assets/models/muzzle.glb".to_string()),
        ..Weapon::default()
    };
    assert_eq!(
        weapon.references(),
        vec!["assets/models/muzzle.glb".to_string()]
    );

    assert!(Weapon::default().references().is_empty());
}

#[test]
fn a_derived_component_registers_and_round_trips() {
    let mut registry = ComponentRegistry::new();
    registry.register::<Weapon>();

    let mut world = World::new();
    let entity = world.spawn();
    registry
        .insert_default(&mut world, entity, "Turret")
        .expect("registered");

    let data = serde_json::json!({
        "damage": 3,
        "active": true,
        "label": "x",
        "muzzle": null,
        "fire_mode": "single",
        "offset": [0.0, 0.0, 0.0],
        "tint": [1.0, 1.0, 1.0, 1.0],
    });
    registry
        .deserialize(&mut world, entity, "Turret", &data)
        .expect("valid data");
    assert_eq!(
        registry.serialize(&world, entity, "Turret"),
        Some(data.clone())
    );

    let with_asset = serde_json::json!({
        "damage": 3,
        "active": true,
        "label": "x",
        "muzzle": "assets/models/muzzle.glb",
        "fire_mode": "single",
        "offset": [0.0, 0.0, 0.0],
        "tint": [1.0, 1.0, 1.0, 1.0],
    });
    assert_eq!(
        registry.references_of("Turret", &with_asset),
        vec!["assets/models/muzzle.glb".to_string()]
    );
}
