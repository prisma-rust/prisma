//! Generic ECS entity pool built on top of [`GameWorld`].
//!
//! Entities are deactivated (`Active(false)`) on release instead of despawned so
//! their resolved GPU component handles (such as `SpriteRef`) remain valid when reacquired.

use std::collections::HashMap;

use crate::{EntityHandle, GameWorld};

/// Reusable pool of [`EntityHandle`]s partitioned by caller-chosen category IDs.
#[derive(Debug, Default)]
pub struct EntityPool {
    buckets: HashMap<u32, Vec<EntityHandle>>,
}

impl EntityPool {
    pub fn new() -> Self {
        Self::default()
    }

    /// Acquire an entity for `kind`, returning `(entity, is_newly_spawned)`.
    ///
    /// Reuses an inactive entity from the pool if one is still alive (activating it via
    /// [`GameWorld::set_entity_active`]); otherwise spawns a new entity.
    pub fn acquire(&mut self, world: &mut GameWorld<'_>, kind: u32) -> (EntityHandle, bool) {
        if let Some(bucket) = self.buckets.get_mut(&kind) {
            while let Some(entity) = bucket.pop() {
                if world.is_alive(entity) {
                    world.set_entity_active(entity, true);
                    return (entity, false);
                }
            }
        }
        let entity = world.spawn();
        world.set_entity_active(entity, true);
        (entity, true)
    }

    /// Return `entity` to the `kind` bucket and deactivate it.
    pub fn release(&mut self, world: &mut GameWorld<'_>, kind: u32, entity: EntityHandle) {
        if !world.is_alive(entity) {
            return;
        }
        let bucket = self.buckets.entry(kind).or_default();
        if bucket.contains(&entity) {
            return;
        }
        world.set_entity_active(entity, false);
        bucket.push(entity);
    }

    /// Number of currently idle pooled entities for `kind`.
    pub fn idle_count(&self, kind: u32) -> usize {
        self.buckets.get(&kind).map_or(0, Vec::len)
    }

    /// Drop all tracked handles (for example after a scene reload invalidated old handles).
    pub fn clear(&mut self) {
        self.buckets.clear();
    }

    /// Despawn every idle entity currently held in the pool.
    pub fn despawn_idle(&mut self, world: &mut GameWorld<'_>) {
        for (_, bucket) in self.buckets.drain() {
            for entity in bucket {
                if world.is_alive(entity) {
                    world.despawn(entity);
                }
            }
        }
    }
}
