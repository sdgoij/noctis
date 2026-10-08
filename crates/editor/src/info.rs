//! Introspection types for the command API.
//!
//! Agents drive the editor without a UI, so they need a compact, serializable
//! view of the scene: which entities exist, what they are named, their parent,
//! which components they carry, and where they are. [`EntityInfo`] is that view.

use ecs::world::Entity;
use math::Vec3;
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

/// A serializable `(index, generation)` pair identifying an [`Entity`].
///
/// [`Entity`] itself is not serializable, so it is projected to this small,
/// stable struct whenever it must appear in JSON (introspection output, command
/// payloads, ...).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntityDto {
    /// The entity slot index.
    pub index: u32,
    /// The entity slot generation.
    pub generation: u32,
}

impl EntityDto {
    /// Project an [`Entity`] into its serializable form.
    pub fn new(entity: Entity) -> Self {
        Self {
            index: entity.index(),
            generation: entity.generation(),
        }
    }

    /// Reconstruct the [`Entity`] this refers to.
    pub fn to_entity(self) -> Entity {
        Entity::from_raw(self.index, self.generation)
    }
}

impl From<Entity> for EntityDto {
    fn from(entity: Entity) -> Self {
        Self::new(entity)
    }
}

impl Serialize for EntityDto {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Entity", 2)?;
        state.serialize_field("index", &self.index)?;
        state.serialize_field("generation", &self.generation)?;
        state.end()
    }
}

/// A flat, human- and machine-readable description of one entity.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityInfo {
    /// The entity this describes.
    pub entity: Entity,
    /// The entity's name (`""` if it has none).
    pub name: String,
    /// The entity's parent, if any.
    pub parent: Option<Entity>,
    /// Canonical names of the "attachable" components present on the entity.
    pub components: Vec<&'static str>,
    /// The entity's local translation.
    pub translation: Vec3,
}

impl Serialize for EntityInfo {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("EntityInfo", 5)?;
        state.serialize_field("entity", &EntityDto::new(self.entity))?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("parent", &self.parent.map(EntityDto::new))?;
        state.serialize_field("components", &self.components)?;
        state.serialize_field(
            "translation",
            &[self.translation.x, self.translation.y, self.translation.z],
        )?;
        state.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use math::vec3;

    #[test]
    fn entity_dto_round_trips() {
        let e = Entity::from_raw(7, 2);
        assert_eq!(EntityDto::new(e).to_entity(), e);
    }

    #[test]
    fn entity_info_serializes_to_json() {
        let info = EntityInfo {
            entity: Entity::from_raw(3, 1),
            name: "Sun".into(),
            parent: None,
            components: vec!["Light"],
            translation: vec3(1.0, 2.0, 3.0),
        };
        let value = serde_json::to_value(&info).unwrap();
        assert_eq!(value["name"], "Sun");
        assert_eq!(value["entity"]["index"], 3);
        assert_eq!(value["translation"], serde_json::json!([1.0, 2.0, 3.0]));
        assert_eq!(value["components"], serde_json::json!(["Light"]));
        assert!(value["parent"].is_null());
    }
}
