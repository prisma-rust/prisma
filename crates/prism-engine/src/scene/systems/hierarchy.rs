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

use std::collections::HashSet;

use prism_ecs::{Entity, World};

use crate::scene::components::*;

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
    let roots: Vec<Entity> = candidate_entities(world)
        .into_iter()
        .filter(|entity| !has_live_parent(world, *entity))
        .collect();

    let mut visited: HashSet<Entity> = HashSet::new();
    for root in roots {
        if !visited.insert(root) {
            continue;
        }
        // Root without `LocalTransform` (only `Child`) propagates downward with identity matrix.
        let world_mat = match world.get::<LocalTransform>(root).cloned() {
            Some(local) => {
                let matrix = local.to_model_matrix();
                world.insert(root, LocalToWorld(matrix));
                matrix
            }
            None => glam::Mat4::IDENTITY,
        };
        visit_children(world, root, world_mat, &mut visited);
    }
}

/// Entities that can participate in propagation: those with `LocalTransform`, and those with `Child`
/// (latter, even without own transform, must let child tree keep propagating).
fn candidate_entities(world: &World) -> Vec<Entity> {
    let mut entities: Vec<Entity> = world.query::<LocalTransform>().map(|(e, _)| e).collect();
    entities.extend(world.query::<Child>().map(|(e, _)| e));
    let mut seen: HashSet<Entity> = HashSet::new();
    entities.retain(|entity| seen.insert(*entity));
    entities
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
    // Index into the parent's child list instead of cloning it: the recursive
    // calls mutate components, so the `&Child` borrow cannot be held across
    // them, but re-reading one element per child is still allocation-free
    // (#66 — this used to clone the whole list for every node, per frame).
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
        // `visited` also catches cycles in dirty data (A.Child=[B], B.Child=[A]).
        if !world.is_alive(child) || !visited.insert(child) {
            continue;
        }
        let child_world = match world.get::<LocalTransform>(child).cloned() {
            Some(local) => {
                let matrix = parent_world * local.to_model_matrix();
                world.insert(child, LocalToWorld(matrix));
                matrix
            }
            // No-transform intermediate node: does not write `LocalToWorld` itself, but passes parent matrix as-is
            // to descendants (missing Transform equivalent to identity transform).
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
    let mut entities: Vec<Entity> = world.query::<Active>().map(|(entity, _)| entity).collect();
    entities.extend(world.query::<Parent>().map(|(entity, _)| entity));
    entities.sort_unstable_by_key(|entity| entity.id());
    entities.dedup();

    // Multiply along parent chain, reuse already-computed ancestor (deep tree also O(n)).
    let mut resolved: std::collections::HashMap<Entity, bool> = std::collections::HashMap::new();
    for entity in entities {
        let mut chain: Vec<Entity> = Vec::new();
        let mut effective = true;
        let mut cursor = Some(entity);
        while let Some(current) = cursor {
            if let Some(done) = resolved.get(&current) {
                effective &= *done;
                break;
            }
            // Dirty data can cycle (A→B→A): finish with already accumulated result, avoid infinite loop.
            if chain.contains(&current) {
                break;
            }
            chain.push(current);
            effective &= world
                .get::<Active>(current)
                .map(|active| active.0)
                .unwrap_or(true);
            cursor = world
                .get::<Parent>(current)
                .map(|parent| parent.0)
                .filter(|parent| world.is_alive(*parent));
        }
        for item in chain {
            resolved.insert(item, effective);
        }
    }

    for (entity, value) in resolved {
        world.insert(entity, ActiveInHierarchy(value));
    }
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
    // Distance LOD (#491): runtime system selects level then marks others hidden. It is separate from editor
    // own `Hidden`, to avoid LOD overriding author / editor visibility choice.
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
