//! Hierarchy system  — computes [`LocalToWorld`] for every entity
//!
//! Walks the scene tree  from root entities in DFS order, accumulating parent
//! transforms: `world_child = world_parent × local_child`.
//!
//! Run once per frame **after** any local-transform or reparenting changes.
//!
//! When tree shape inconsistent, fall back to unified semantics (see `#48`):
//! - Intermediate node without `LocalTransform` **does not block propagation**: it keeps no
//!   `LocalToWorld`, but child nodes still get its accumulated matrix;
//! - `Parent` pointing to dead entity, or parent `Child` not listing it,
//!   treated as root (`LocalToWorld = LocalTransform`), not frozen in old value forever.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use prism_ecs::{Entity, World};

use crate::scene::components::*;

#[derive(Default)]
struct HierarchyScratch {
    candidates: Vec<Entity>,
    roots: Vec<Entity>,
    seen: HashSet<Entity>,
    visited: HashSet<Entity>,
    active_entities: Vec<Entity>,
    resolved: HashMap<Entity, bool>,
    chain: Vec<Entity>,
}

thread_local! {
    static HIERARCHY_SCRATCH: RefCell<HierarchyScratch> = RefCell::new(HierarchyScratch::default());
}

fn write_local_to_world_if_changed(world: &mut World, entity: Entity, matrix: glam::Mat4) {
    if world
        .get::<LocalToWorld>(entity)
        .is_some_and(|existing| existing.0 == matrix)
    {
        return;
    }
    world.insert(entity, LocalToWorld(matrix));
}

/// Recompute [`LocalToWorld`] for every entity  in the hierarchy.
///
/// - Roots (entities without [`Parent`]) get `LocalToWorld = LocalTransform`.
/// - Child get `LocalToWorld = parent_WorldTransform × local_Transform`.
/// - Entities without `LocalTransform` are not given a `LocalToWorld`, but they
///   no longer block their children (the accumulated parent matrix is passed
///   through).
///
/// This function is intended to be called once per frame, after all local
/// transform changes have been applied.
pub fn hierarchy_system(world: &mut World) {
    HIERARCHY_SCRATCH.with(|cell| {
        let mut scratch = cell.borrow_mut();
        let scratch = &mut *scratch;
        scratch.candidates.clear();
        scratch.seen.clear();
        for (entity, _) in world.query::<LocalTransform>() {
            if scratch.seen.insert(entity) {
                scratch.candidates.push(entity);
            }
        }
        for (entity, _) in world.query::<Child>() {
            if scratch.seen.insert(entity) {
                scratch.candidates.push(entity);
            }
        }

        scratch.roots.clear();
        for &entity in &scratch.candidates {
            if !has_live_parent(world, entity) {
                scratch.roots.push(entity);
            }
        }

        scratch.visited.clear();
        for i in 0..scratch.roots.len() {
            let root = scratch.roots[i];
            if !scratch.visited.insert(root) {
                continue;
            }
            let world_mat = match world.get::<LocalTransform>(root).cloned() {
                Some(local) => {
                    let matrix = local.to_model_matrix();
                    write_local_to_world_if_changed(world, root, matrix);
                    matrix
                }
                None => glam::Mat4::IDENTITY,
            };
            visit_children(world, root, world_mat, &mut scratch.visited);
        }
    });
}

/// Whether entity hangs under a **valid** parent: parent alive, and parent `Child` list
/// actually contains it. Any failure treated as root (children promoted to root).
fn has_live_parent(world: &World, entity: Entity) -> bool {
    let Some(parent) = world.get::<Parent>(entity).map(|parent| parent.0) else {
        return false;
    };
    world.is_alive(parent)
        && world
            .get::<Child>(parent)
            .is_some_and(|children| children.0.contains(&entity))
}

/// Recursively visit children, compute and store their world transform.
fn visit_children(
    world: &mut World,
    parent: Entity,
    parent_world: glam::Mat4,
    visited: &mut HashSet<Entity>,
) {
    let child_count = world
        .get::<Child>(parent)
        .map_or(0usize, |children| children.0.len());

    for index in 0..child_count {
        let Some(child) = world
            .get::<Child>(parent)
            .and_then(|children| children.0.get(index).copied())
        else {
            break;
        };
        if !world.is_alive(child) || !visited.insert(child) {
            continue;
        }
        let child_world = match world.get::<LocalTransform>(child).cloned() {
            Some(local) => {
                let matrix = parent_world * local.to_model_matrix();
                write_local_to_world_if_changed(world, child, matrix);
                matrix
            }
            None => parent_world,
        };
        visit_children(world, child, child_world, visited);
    }
}

/// Propagate [`Active`] along parent-child relations into [`ActiveInHierarchy`].
///
/// Unity `activeSelf` (editor eye icon) and `activeInHierarchy` (actually participating in
/// render / logic) are two things: any ancestor inactive, whole child tree counts inactive. Previously engine
/// only filtered by single entity `Active`, so disabling parent left children appearing normally (#433).
///
/// Like [`hierarchy_system`], runs once at end of `Engine::update` — user disabling parent in tick
/// should be visible to extract in same frame. Runs separately rather than merged into
/// `hierarchy_system`: matrix propagation only builds tree for entities with `LocalTransform` / `Child`,
/// while `Active` can be attached to pure logic nodes without transform.
pub fn active_hierarchy_system(world: &mut World) {
    HIERARCHY_SCRATCH.with(|cell| {
        let mut scratch = cell.borrow_mut();
        let scratch = &mut *scratch;
        scratch.active_entities.clear();
        scratch
            .active_entities
            .extend(world.query::<Active>().map(|(entity, _)| entity));
        scratch
            .active_entities
            .extend(world.query::<Parent>().map(|(entity, _)| entity));
        scratch
            .active_entities
            .sort_unstable_by_key(|entity| entity.id());
        scratch.active_entities.dedup();

        scratch.resolved.clear();
        for i in 0..scratch.active_entities.len() {
            let entity = scratch.active_entities[i];
            scratch.chain.clear();
            let mut effective = true;
            let mut cursor = Some(entity);
            while let Some(current) = cursor {
                if let Some(done) = scratch.resolved.get(&current) {
                    effective &= *done;
                    break;
                }
                if scratch.chain.contains(&current) {
                    break;
                }
                scratch.chain.push(current);
                effective &= world
                    .get::<Active>(current)
                    .map(|active| active.0)
                    .unwrap_or(true);
                cursor = world
                    .get::<Parent>(current)
                    .map(|parent| parent.0)
                    .filter(|parent| world.is_alive(*parent));
            }
            for &item in &scratch.chain {
                scratch.resolved.insert(item, effective);
            }
        }

        for (&entity, &value) in &scratch.resolved {
            if world
                .get::<ActiveInHierarchy>(entity)
                .is_some_and(|existing| existing.0 == value)
            {
                continue;
            }
            world.insert(entity, ActiveInHierarchy(value));
        }
    });
}

/// Effective active state used by render / light / UI filters: derived value first, then own [`Active`].
///
/// When derived component missing (entity just spawned, system not run) fall back to own `Active`, legacy behavior.
/// Editor Scene visibility ([`Hidden`]) layered last: hiding only affects render-class systems,
/// does not change `Active` / `ActiveInHierarchy`, nor affect physics or scripts.
pub fn is_active_in_hierarchy(world: &World, entity: Entity) -> bool {
    if world.get::<Hidden>(entity).is_some_and(|hidden| hidden.0) {
        return false;
    }
    if world
        .get::<LodHidden>(entity)
        .is_some_and(|hidden| hidden.0)
    {
        return false;
    }
    if let Some(derived) = world.get::<ActiveInHierarchy>(entity) {
        return derived.0;
    }
    world
        .get::<Active>(entity)
        .map(|active| active.0)
        .unwrap_or(true)
}

#[cfg(test)]
#[path = "hierarchy_tests.rs"]
mod tests;
