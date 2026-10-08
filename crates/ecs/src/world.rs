//! Entities, components, and the [`World`] that stores them.
//!
//! The ECS here is deliberately small and explicit rather than a mega-framework:
//!
//! * [`Entity`] is a packed `(index, generation)` handle. Generations make
//!   stale handles detectable, so component access on a despawned entity
//!   returns `None` instead of silently reading a reused slot.
//! * [`World`] stores one dense `Vec<Option<C>>` per component type, keyed by
//!   `TypeId`. Component access is by type with no registration step.
//! * Queries come in single-component ([`World::query`] / [`World::query_mut`])
//!   and two-component ([`World::query_with`]) flavors. All are implemented
//!   with slice iterators, so no `unsafe` is required.
//!
//! Component types must be `Send + Sync + 'static`.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::iter::Enumerate;
use std::slice::{Iter, IterMut};

/// A lightweight, generational handle to an entity.
///
/// `Entity` is `Copy` and eight bytes, so it is cheap to store in components
/// and to hand to scripts. The generation makes a handle to a despawned entity
/// distinguishable from a handle to a spawned one that reused the slot.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Entity {
    index: u32,
    generation: u32,
}

impl Entity {
    /// The slot index, stable for the lifetime of the entity.
    #[inline]
    pub const fn index(self) -> u32 {
        self.index
    }

    /// The generation counter for the slot.
    #[inline]
    pub const fn generation(self) -> u32 {
        self.generation
    }

    /// Build a handle from raw parts. Intended for tests and deserialization.
    #[inline]
    pub const fn from_raw(index: u32, generation: u32) -> Entity {
        Entity { index, generation }
    }
}

impl fmt::Debug for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Entity({}v{})", self.index, self.generation)
    }
}

impl fmt::Display for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}v{}", self.index, self.generation)
    }
}

#[derive(Clone, Copy)]
struct Slot {
    generation: u32,
    alive: bool,
}

/// Allocates and recycles [`Entity`] handles.
#[derive(Default)]
pub struct Entities {
    slots: Vec<Slot>,
    free: Vec<u32>,
    alive: usize,
}

impl Entities {
    /// An empty allocator.
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of live entities.
    #[inline]
    pub fn len(&self) -> usize {
        self.alive
    }

    /// `true` if no entities are alive.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.alive == 0
    }

    /// Allocate a fresh entity, reusing a freed slot when possible.
    pub fn alloc(&mut self) -> Entity {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.alive = true;
            self.alive += 1;
            Entity {
                index,
                generation: slot.generation,
            }
        } else {
            let index = self.slots.len() as u32;
            self.slots.push(Slot {
                generation: 0,
                alive: true,
            });
            self.alive += 1;
            Entity {
                index,
                generation: 0,
            }
        }
    }

    /// `true` if the handle refers to a live entity.
    #[inline]
    pub fn is_alive(&self, entity: Entity) -> bool {
        self.slots
            .get(entity.index as usize)
            .is_some_and(|s| s.alive && s.generation == entity.generation)
    }

    /// Free an entity. Returns `false` if it was already dead.
    pub fn free(&mut self, entity: Entity) -> bool {
        match self.slots.get_mut(entity.index as usize) {
            Some(slot) if slot.alive && slot.generation == entity.generation => {
                slot.alive = false;
                slot.generation = slot.generation.wrapping_add(1);
                self.free.push(entity.index);
                self.alive -= 1;
                true
            }
            _ => false,
        }
    }

    /// The live entity at `index`, if any.
    pub fn entity_at(&self, index: usize) -> Option<Entity> {
        self.slots.get(index).filter(|s| s.alive).map(|s| Entity {
            index: index as u32,
            generation: s.generation,
        })
    }

    /// Iterate over every live entity.
    pub fn iter(&self) -> impl Iterator<Item = Entity> + '_ {
        self.slots.iter().enumerate().filter_map(|(i, s)| {
            s.alive.then_some(Entity {
                index: i as u32,
                generation: s.generation,
            })
        })
    }
}

/// Type-erased component column.
trait ErasedColumn: Any + Send + Sync {
    fn remove_index(&mut self, index: usize);
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn component_count(&self) -> usize;
}

impl<T: Send + Sync + 'static> ErasedColumn for Vec<Option<T>> {
    fn remove_index(&mut self, index: usize) {
        if let Some(slot) = self.get_mut(index) {
            *slot = None;
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn component_count(&self) -> usize {
        self.iter().filter(|c| c.is_some()).count()
    }
}

/// A container for entities and their components.
#[derive(Default)]
pub struct World {
    entities: Entities,
    columns: HashMap<TypeId, Box<dyn ErasedColumn>>,
    names: HashMap<TypeId, &'static str>,
}

impl World {
    /// An empty world.
    pub fn new() -> Self {
        Self::default()
    }

    /// The entity allocator.
    #[inline]
    pub fn entities(&self) -> &Entities {
        &self.entities
    }

    /// Spawn a bare entity with no components.
    pub fn spawn(&mut self) -> Entity {
        self.entities.alloc()
    }

    /// Spawn an entity that already carries `component`.
    pub fn spawn_with<C: Send + Sync + 'static>(&mut self, component: C) -> Entity {
        let entity = self.spawn();
        self.insert(entity, component);
        entity
    }

    /// `true` if the entity is alive.
    #[inline]
    pub fn is_alive(&self, entity: Entity) -> bool {
        self.entities.is_alive(entity)
    }

    /// Despawn an entity and drop all of its components.
    pub fn despawn(&mut self, entity: Entity) -> bool {
        if !self.entities.free(entity) {
            return false;
        }
        for column in self.columns.values_mut() {
            column.remove_index(entity.index as usize);
        }
        true
    }

    fn column_mut<C: Send + Sync + 'static>(&mut self) -> &mut Vec<Option<C>> {
        let type_id = TypeId::of::<C>();
        self.names.entry(type_id).or_insert_with(type_name::<C>);
        self.columns
            .entry(type_id)
            .or_insert_with(|| Box::new(Vec::<Option<C>>::new()))
            .as_any_mut()
            .downcast_mut::<Vec<Option<C>>>()
            .expect("component column type mismatch")
    }

    fn column<C: Send + Sync + 'static>(&self) -> Option<&Vec<Option<C>>> {
        self.columns
            .get(&TypeId::of::<C>())
            .and_then(|c| c.as_any().downcast_ref::<Vec<Option<C>>>())
    }

    /// Insert (or replace) a component on a live entity. Returns the previous
    /// value, if any.
    pub fn insert<C: Send + Sync + 'static>(&mut self, entity: Entity, component: C) -> Option<C> {
        if !self.entities.is_alive(entity) {
            return None;
        }
        let index = entity.index as usize;
        let column = self.column_mut::<C>();
        if column.len() <= index {
            column.resize_with(index + 1, || None);
        }
        column[index].replace(component)
    }

    /// Borrow a component.
    pub fn get<C: Send + Sync + 'static>(&self, entity: Entity) -> Option<&C> {
        if !self.entities.is_alive(entity) {
            return None;
        }
        self.column::<C>()?.get(entity.index as usize)?.as_ref()
    }

    /// Mutably borrow a component.
    pub fn get_mut<C: Send + Sync + 'static>(&mut self, entity: Entity) -> Option<&mut C> {
        if !self.entities.is_alive(entity) {
            return None;
        }
        self.columns
            .get_mut(&TypeId::of::<C>())?
            .as_any_mut()
            .downcast_mut::<Vec<Option<C>>>()?
            .get_mut(entity.index as usize)?
            .as_mut()
    }

    /// Remove a component, returning it if present.
    pub fn remove<C: Send + Sync + 'static>(&mut self, entity: Entity) -> Option<C> {
        self.columns
            .get_mut(&TypeId::of::<C>())?
            .as_any_mut()
            .downcast_mut::<Vec<Option<C>>>()?
            .get_mut(entity.index as usize)?
            .take()
    }

    /// `true` if the entity has a component of type `C`.
    #[inline]
    pub fn has<C: Send + Sync + 'static>(&self, entity: Entity) -> bool {
        self.get::<C>(entity).is_some()
    }

    /// The number of components of type `C`.
    pub fn count<C: Send + Sync + 'static>(&self) -> usize {
        self.columns
            .get(&TypeId::of::<C>())
            .map_or(0, |c| c.component_count())
    }

    /// Iterate over every `(Entity, &C)` pair.
    pub fn query<C: Send + Sync + 'static>(&self) -> Query<'_, C> {
        Query {
            entities: &self.entities,
            inner: self.column::<C>().map(|c| c.iter().enumerate()),
        }
    }

    /// Iterate over every `(Entity, &mut C)` pair.
    pub fn query_mut<C: Send + Sync + 'static>(&mut self) -> QueryMut<'_, C> {
        let World {
            entities, columns, ..
        } = self;
        let inner = columns
            .get_mut(&TypeId::of::<C>())
            .and_then(|c| c.as_any_mut().downcast_mut::<Vec<Option<C>>>())
            .map(|c| c.iter_mut().enumerate());
        QueryMut {
            entities: &*entities,
            inner,
        }
    }

    /// Iterate over every entity that has *both* `A` and `B`, yielding mutable
    /// access to `B` and the entity id. The two component types must differ.
    pub fn query_with<A: Send + Sync + 'static, B: Send + Sync + 'static>(
        &mut self,
    ) -> QueryWith<'_, A, B> {
        assert_ne!(
            TypeId::of::<A>(),
            TypeId::of::<B>(),
            "query_with requires two distinct component types"
        );
        // Collect the candidate rows first; this immutable borrow ends before
        // the mutable borrow below.
        let mut candidates: Vec<u32> = match self.column::<A>() {
            Some(column) => column
                .iter()
                .enumerate()
                .filter_map(|(i, c)| c.is_some().then_some(i as u32))
                .collect(),
            None => Vec::new(),
        };
        candidates.sort_unstable();

        let World {
            entities, columns, ..
        } = self;
        let inner = columns
            .get_mut(&TypeId::of::<B>())
            .and_then(|c| c.as_any_mut().downcast_mut::<Vec<Option<B>>>())
            .map(|c| c.iter_mut().enumerate());
        QueryWith {
            entities: &*entities,
            inner,
            candidates,
            cursor: 0,
            _marker: std::marker::PhantomData,
        }
    }

    /// Every live entity that carries a component of type `C`.
    pub fn entities_with<C: Send + Sync + 'static>(&self) -> Vec<Entity> {
        self.query::<C>().map(|(e, _)| e).collect()
    }

    /// The registered type name for a component type, if it has been used.
    pub fn component_type_name<C: Send + Sync + 'static>(&self) -> Option<&'static str> {
        self.names.get(&TypeId::of::<C>()).copied()
    }

    /// The names of every component type seen by this world, sorted.
    pub fn component_type_names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self.names.values().copied().collect();
        names.sort_unstable();
        names.dedup();
        names
    }
}

fn type_name<T: ?Sized>() -> &'static str {
    let full = std::any::type_name::<T>();
    full.rsplit("::").next().unwrap_or(full)
}

/// Iterator over `(Entity, &C)`.
pub struct Query<'a, C> {
    entities: &'a Entities,
    inner: Option<Enumerate<Iter<'a, Option<C>>>>,
}

impl<'a, C> Iterator for Query<'a, C> {
    type Item = (Entity, &'a C);
    fn next(&mut self) -> Option<Self::Item> {
        let inner = self.inner.as_mut()?;
        for (index, slot) in inner.by_ref() {
            if let Some(component) = slot
                && let Some(entity) = self.entities.entity_at(index)
            {
                return Some((entity, component));
            }
        }
        None
    }
}

/// Iterator over `(Entity, &mut C)`.
pub struct QueryMut<'a, C> {
    entities: &'a Entities,
    inner: Option<Enumerate<IterMut<'a, Option<C>>>>,
}

impl<'a, C> Iterator for QueryMut<'a, C> {
    type Item = (Entity, &'a mut C);
    fn next(&mut self) -> Option<Self::Item> {
        let inner = self.inner.as_mut()?;
        for (index, slot) in inner.by_ref() {
            if let Some(component) = slot
                && let Some(entity) = self.entities.entity_at(index)
            {
                return Some((entity, component));
            }
        }
        None
    }
}

/// Iterator over `(Entity, &mut B)` for entities that also have `A`.
pub struct QueryWith<'a, A, B> {
    entities: &'a Entities,
    inner: Option<Enumerate<IterMut<'a, Option<B>>>>,
    candidates: Vec<u32>,
    cursor: usize,
    _marker: std::marker::PhantomData<A>,
}

impl<'a, A, B> Iterator for QueryWith<'a, A, B> {
    type Item = (Entity, &'a mut B);
    fn next(&mut self) -> Option<Self::Item> {
        let inner = self.inner.as_mut()?;
        for (index, slot) in inner.by_ref() {
            while self.cursor < self.candidates.len()
                && (self.candidates[self.cursor] as usize) < index
            {
                self.cursor += 1;
            }
            if self.cursor >= self.candidates.len() {
                return None;
            }
            if self.candidates[self.cursor] as usize == index
                && let Some(component) = slot
            {
                self.cursor += 1;
                if let Some(entity) = self.entities.entity_at(index) {
                    return Some((entity, component));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Position(f32, f32, f32);

    #[derive(Debug, PartialEq, Clone, Copy)]
    struct Velocity(f32);

    #[test]
    fn spawn_and_despawn() {
        let mut world = World::new();
        let e = world.spawn();
        assert!(world.is_alive(e));
        assert_eq!(world.entities().len(), 1);
        assert!(world.despawn(e));
        assert!(!world.is_alive(e));
        assert!(!world.despawn(e), "double despawn must be a no-op");
        assert_eq!(world.entities().len(), 0);
    }

    #[test]
    fn generations_detect_stale_handles() {
        let mut world = World::new();
        let a = world.spawn_with(Position(1.0, 2.0, 3.0));
        world.despawn(a);
        let b = world.spawn_with(Position(9.0, 9.0, 9.0));
        assert_eq!(a.index(), b.index(), "slot should be reused");
        assert_ne!(a.generation(), b.generation());
        assert!(!world.is_alive(a));
        assert!(
            world.get::<Position>(a).is_none(),
            "stale handle sees nothing"
        );
        assert_eq!(world.get::<Position>(b), Some(&Position(9.0, 9.0, 9.0)));
    }

    #[test]
    fn insert_get_remove() {
        let mut world = World::new();
        let e = world.spawn();
        assert!(!world.has::<Position>(e));
        assert_eq!(world.insert(e, Position(1.0, 2.0, 3.0)), None);
        assert!(world.has::<Position>(e));
        assert_eq!(world.get::<Position>(e), Some(&Position(1.0, 2.0, 3.0)));
        world.get_mut::<Position>(e).unwrap().0 = 5.0;
        assert_eq!(world.get::<Position>(e), Some(&Position(5.0, 2.0, 3.0)));
        assert_eq!(world.remove::<Position>(e), Some(Position(5.0, 2.0, 3.0)));
        assert!(!world.has::<Position>(e));
        assert_eq!(world.count::<Position>(), 0);
    }

    #[test]
    fn despawn_drops_all_components() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, Position(0.0, 0.0, 0.0));
        world.insert(e, Velocity(1.0));
        world.despawn(e);
        assert_eq!(world.count::<Position>(), 0);
        assert_eq!(world.count::<Velocity>(), 0);
    }

    #[test]
    fn query_yields_all() {
        let mut world = World::new();
        let a = world.spawn_with(Position(1.0, 0.0, 0.0));
        let b = world.spawn_with(Position(2.0, 0.0, 0.0));
        world.spawn(); // no Position
        let mut seen: Vec<(Entity, f32)> =
            world.query::<Position>().map(|(e, p)| (e, p.0)).collect();
        seen.sort_by_key(|x| x.0);
        assert_eq!(seen, vec![(a, 1.0), (b, 2.0)]);
    }

    #[test]
    fn query_mut_mutates() {
        let mut world = World::new();
        let a = world.spawn_with(Velocity(1.0));
        let b = world.spawn_with(Velocity(2.0));
        for (_, v) in world.query_mut::<Velocity>() {
            v.0 *= 10.0;
        }
        assert_eq!(world.get::<Velocity>(a), Some(&Velocity(10.0)));
        assert_eq!(world.get::<Velocity>(b), Some(&Velocity(20.0)));
    }

    #[test]
    fn query_with_requires_both_components() {
        let mut world = World::new();
        let both = world.spawn();
        world.insert(both, Position(1.0, 2.0, 3.0));
        world.insert(both, Velocity(4.0));
        let only_pos = world.spawn_with(Position(0.0, 0.0, 0.0));
        let only_vel = world.spawn_with(Velocity(7.0));

        let mut hits = 0;
        for (entity, velocity) in world.query_with::<Position, Velocity>() {
            assert_eq!(entity, both);
            velocity.0 += 1.0;
            hits += 1;
        }
        assert_eq!(hits, 1);
        assert_eq!(world.get::<Velocity>(both), Some(&Velocity(5.0)));
        assert_eq!(
            world.get::<Velocity>(only_vel),
            Some(&Velocity(7.0)),
            "untouched"
        );
        assert!(!world.has::<Velocity>(only_pos));
    }

    #[test]
    fn component_type_names_are_recorded() {
        let mut world = World::new();
        let e = world.spawn_with(Position(0.0, 0.0, 0.0));
        world.insert(e, Velocity(0.0));
        let names = world.component_type_names();
        assert!(names.contains(&"Position"));
        assert!(names.contains(&"Velocity"));
    }

    #[test]
    fn insert_on_dead_entity_is_ignored() {
        let mut world = World::new();
        let e = world.spawn();
        world.despawn(e);
        assert_eq!(world.insert(e, Velocity(1.0)), None);
        assert!(!world.has::<Velocity>(e));
    }

    #[test]
    fn entities_iter_matches_live_count() {
        let mut world = World::new();
        let a = world.spawn();
        let b = world.spawn();
        world.spawn();
        world.despawn(b);
        let live: Vec<Entity> = world.entities().iter().collect();
        assert_eq!(live.len(), 2);
        assert!(live.contains(&a));
        assert!(!live.contains(&b));
    }
}
