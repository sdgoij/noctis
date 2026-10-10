//! Custom components: the [`Component`] trait and the [`ComponentRegistry`].
//!
//! The engine's built-in components ([`crate::components`]) are a closed set that
//! serializes through [`ComponentDesc`](crate::scene::ComponentDesc). A *custom*
//! component is defined outside the engine — in an embedding application, or a
//! project's Rust — and registered so the scene loader/saver, the editor, the
//! bundler, and the script bridge can all see it.
//!
//! Storage is already open: [`World`] holds any `Send + Sync + 'static` type. The
//! registry only adds *reflection* — a stable name, a default, and JSON
//! (de)serialization — so a type the engine has never seen can round-trip through
//! a scene. See `docs/embedding.md`.

use std::collections::HashMap;

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::world::{Entity, World};

/// A field's editor/reflection schema.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    /// The field name, as it appears in the component's JSON.
    pub name: &'static str,
    /// How to present (and interpret) the field.
    pub kind: FieldKind,
}

impl Field {
    /// A floating-point field.
    pub const fn number(name: &'static str) -> Self {
        Self {
            name,
            kind: FieldKind::Number,
        }
    }

    /// A whole-number field.
    pub const fn integer(name: &'static str) -> Self {
        Self {
            name,
            kind: FieldKind::Integer,
        }
    }

    /// A boolean field.
    pub const fn boolean(name: &'static str) -> Self {
        Self {
            name,
            kind: FieldKind::Boolean,
        }
    }

    /// A text field.
    pub const fn text(name: &'static str) -> Self {
        Self {
            name,
            kind: FieldKind::Text,
        }
    }

    /// A three-float field (`[x, y, z]`).
    pub const fn vec3(name: &'static str) -> Self {
        Self {
            name,
            kind: FieldKind::Vec3,
        }
    }

    /// An RGBA colour field (`[r, g, b, a]`).
    pub const fn color(name: &'static str) -> Self {
        Self {
            name,
            kind: FieldKind::Color,
        }
    }

    /// An asset-path field; the bundler treats it as a build reference.
    pub const fn asset(name: &'static str) -> Self {
        Self {
            name,
            kind: FieldKind::Asset,
        }
    }

    /// A field chosen from a fixed set of strings.
    pub const fn enumeration(name: &'static str, options: &'static [&'static str]) -> Self {
        Self {
            name,
            kind: FieldKind::Enum(options),
        }
    }
}

/// How a [`Field`] is presented and interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    /// A floating-point number.
    Number,
    /// A whole number.
    Integer,
    /// A boolean.
    Boolean,
    /// A string.
    Text,
    /// Three floats (`[x, y, z]`).
    Vec3,
    /// Four floats (`[r, g, b, a]`).
    Color,
    /// An asset path; the bundler treats it as a reference.
    Asset,
    /// One of a fixed set of strings.
    Enum(&'static [&'static str]),
}

/// A component defined outside the engine.
///
/// Implement this and register the type with [`ComponentRegistry::register`] so it
/// can be saved, loaded, and edited by name — like a built-in component, but with
/// the type living in the consumer's crate.
///
/// ```rust
/// use ecs::reflect::{Component, Field};
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Debug, Default, Serialize, Deserialize)]
/// struct Health {
///     current: f32,
///     max: f32,
/// }
///
/// impl Component for Health {
///     const NAME: &'static str = "Health";
///     fn fields() -> Vec<Field> {
///         vec![Field::number("current"), Field::number("max")]
///     }
/// }
/// ```
pub trait Component: Send + Sync + 'static + Serialize + DeserializeOwned + Default {
    /// The stable name used in scene files, the editor, and scripts.
    const NAME: &'static str;
    /// The editor/reflection schema. Empty means "no editable fields".
    fn fields() -> Vec<Field> {
        Vec::new()
    }
    /// Asset references this component contributes to a build.
    fn references(&self) -> Vec<String> {
        Vec::new()
    }
}

/// The type-erased operations for one registered component.
#[derive(Clone, Copy, Debug)]
struct Registration {
    default: fn(&mut World, Entity),
    has: fn(&World, Entity) -> bool,
    serialize: fn(&World, Entity) -> Value,
    deserialize: fn(&mut World, Entity, &Value) -> Result<(), String>,
    remove: fn(&mut World, Entity) -> bool,
    fields: fn() -> Vec<Field>,
    references_json: fn(&Value) -> Vec<String>,
}

/// The component types an application knows about beyond the built-ins.
///
/// A scene carries one so the loader and saver can (de)serialize components the
/// engine does not know by name; the editor and the script bridge use the
/// application's registry directly.
#[derive(Clone, Default, Debug)]
pub struct ComponentRegistry {
    entries: HashMap<&'static str, Registration>,
}

impl ComponentRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `C`, replacing any previous registration under the same name.
    pub fn register<C: Component>(&mut self) -> &mut Self {
        self.entries.insert(
            C::NAME,
            Registration {
                default: |world, entity| {
                    world.insert(entity, C::default());
                },
                has: |world, entity| world.has::<C>(entity),
                serialize: |world, entity| {
                    world
                        .get::<C>(entity)
                        .map(|component| serde_json::to_value(component).unwrap_or(Value::Null))
                        .unwrap_or(Value::Null)
                },
                deserialize: |world, entity, data| {
                    let component = serde_json::from_value::<C>(data.clone())
                        .map_err(|error| error.to_string())?;
                    world.insert(entity, component);
                    Ok(())
                },
                remove: |world, entity| world.remove::<C>(entity).is_some(),
                fields: C::fields,
                references_json: |data| {
                    serde_json::from_value::<C>(data.clone())
                        .map(|component| component.references())
                        .unwrap_or_default()
                },
            },
        );
        self
    }

    /// `true` if a component named `name` is registered.
    pub fn has(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// Every registered name, sorted.
    pub fn names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self.entries.keys().copied().collect();
        names.sort_unstable();
        names
    }

    /// The reflection schema for `name`.
    pub fn fields(&self, name: &str) -> Option<Vec<Field>> {
        self.entries.get(name).map(|entry| (entry.fields)())
    }

    /// Insert a default `name` component on `entity`. An unknown name is an error.
    pub fn insert_default(
        &self,
        world: &mut World,
        entity: Entity,
        name: &str,
    ) -> Result<(), String> {
        match self.entries.get(name) {
            Some(entry) => {
                (entry.default)(world, entity);
                Ok(())
            }
            None => Err(format!("no component registered as {name:?}")),
        }
    }

    /// `true` if `entity` carries the `name` component.
    pub fn has_on(&self, world: &World, entity: Entity, name: &str) -> bool {
        self.entries
            .get(name)
            .is_some_and(|entry| (entry.has)(world, entity))
    }

    /// Serialize the `name` component on `entity`, or `None` when it is absent or
    /// the name is unknown.
    pub fn serialize(&self, world: &World, entity: Entity, name: &str) -> Option<Value> {
        let entry = self.entries.get(name)?;
        (entry.has)(world, entity).then(|| (entry.serialize)(world, entity))
    }

    /// Deserialize `data` into the `name` component on `entity`.
    pub fn deserialize(
        &self,
        world: &mut World,
        entity: Entity,
        name: &str,
        data: &Value,
    ) -> Result<(), String> {
        match self.entries.get(name) {
            Some(entry) => (entry.deserialize)(world, entity, data),
            None => Err(format!("no component registered as {name:?}")),
        }
    }

    /// Remove the `name` component from `entity`, returning whether one was removed.
    pub fn remove(&self, world: &mut World, entity: Entity, name: &str) -> bool {
        self.entries
            .get(name)
            .is_some_and(|entry| (entry.remove)(world, entity))
    }

    /// Every registered component present on `entity`, as `(name, data)` sorted by
    /// name — the scene save path.
    pub fn serialize_entity(&self, world: &World, entity: Entity) -> Vec<(&'static str, Value)> {
        let mut out = Vec::new();
        for name in self.names() {
            if let Some(entry) = self.entries.get(name)
                && (entry.has)(world, entity)
            {
                out.push((name, (entry.serialize)(world, entity)));
            }
        }
        out
    }

    /// Asset references the `name` component's `data` contributes. An unknown name
    /// contributes nothing.
    pub fn references_of(&self, name: &str, data: &Value) -> Vec<String> {
        self.entries
            .get(name)
            .map(|entry| (entry.references_json)(data))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    struct Health {
        current: f32,
        max: f32,
    }

    impl Component for Health {
        const NAME: &'static str = "Health";
        fn fields() -> Vec<Field> {
            vec![Field::number("current"), Field::number("max")]
        }
    }

    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    struct Tag {
        label: String,
    }

    impl Component for Tag {
        const NAME: &'static str = "Tag";
    }

    fn registry() -> ComponentRegistry {
        let mut registry = ComponentRegistry::new();
        registry.register::<Health>().register::<Tag>();
        registry
    }

    #[test]
    fn registering_records_names_sorted() {
        let registry = registry();
        assert!(registry.has("Health"));
        assert!(registry.has("Tag"));
        assert!(!registry.has("Nope"));
        assert_eq!(registry.names(), vec!["Health", "Tag"]);
        assert_eq!(
            registry.fields("Health").map(|fields| fields.len()),
            Some(2)
        );
        assert!(registry.fields("Tag").is_some());
        assert!(registry.fields("Nope").is_none());
    }

    #[test]
    fn a_component_round_trips_through_the_registry() {
        let registry = registry();
        let mut world = World::new();
        let entity = world.spawn();

        registry
            .insert_default(&mut world, entity, "Health")
            .expect("registered");
        assert!(registry.has_on(&world, entity, "Health"));
        assert_eq!(
            registry.serialize(&world, entity, "Health"),
            Some(serde_json::json!({"current": 0.0, "max": 0.0}))
        );

        let data = serde_json::json!({ "current": 3.0, "max": 10.0 });
        registry
            .deserialize(&mut world, entity, "Health", &data)
            .expect("valid data");
        assert_eq!(
            registry.serialize(&world, entity, "Health"),
            Some(data.clone())
        );
        assert_eq!(
            registry.serialize_entity(&world, entity),
            vec![("Health", data)]
        );

        assert!(registry.remove(&mut world, entity, "Health"));
        assert!(!registry.has_on(&world, entity, "Health"));
        assert_eq!(registry.serialize(&world, entity, "Health"), None);
    }

    #[test]
    fn unknown_names_are_errors_not_panics() {
        let registry = registry();
        let mut world = World::new();
        let entity = world.spawn();
        assert!(registry.insert_default(&mut world, entity, "Nope").is_err());
        assert!(
            registry
                .deserialize(&mut world, entity, "Nope", &Value::Null)
                .is_err()
        );
        assert!(!registry.remove(&mut world, entity, "Nope"));
        assert!(!registry.has_on(&world, entity, "Nope"));
        assert_eq!(registry.serialize(&world, entity, "Nope"), None);
    }

    #[test]
    fn a_bad_payload_is_an_error() {
        let registry = registry();
        let mut world = World::new();
        let entity = world.spawn();
        let bad = serde_json::json!({ "current": "much", "max": 1.0 });
        assert!(
            registry
                .deserialize(&mut world, entity, "Health", &bad)
                .is_err()
        );
    }

    #[derive(Debug, Default, Serialize, Deserialize)]
    struct Spawned {
        #[serde(default)]
        prefab: Option<String>,
    }

    impl Component for Spawned {
        const NAME: &'static str = "Spawned";
        fn references(&self) -> Vec<String> {
            self.prefab.iter().cloned().collect()
        }
    }

    #[test]
    fn references_come_from_the_data() {
        let mut registry = ComponentRegistry::new();
        registry.register::<Spawned>();
        assert_eq!(
            registry.references_of(
                "Spawned",
                &serde_json::json!({ "prefab": "prefabs/a.json" })
            ),
            vec!["prefabs/a.json".to_string()]
        );
        assert!(registry.references_of("Spawned", &Value::Null).is_empty());
        assert!(registry.references_of("Nope", &Value::Null).is_empty());
    }
}
