//! Distance-based LOD (#491).
//!
//! When parent entity has [`LodGroup`], its [`Child`] list ordered represents LOD0, LOD1...
//! Each frame selects one level per `LodGroup` config: when `screen_ratios` non-empty, use camera projection to compute screen
//! height ratio, otherwise distance from parent entity to **nearest enabled camera**. After selection writes to each child entity
//! runtime [`LodHidden`]:
//!
//! - current level `LodHidden(false)`;
//! - other levels `LodHidden(true)`.
//!
//! Hiding only affects render-class systems (`is_active_in_hierarchy` skips it), **does not rewrite**
//! author `Active`, nor touch editor `Hidden`. With no camera or non-limited distance, display
//! LOD0, ensuring missing data will not hide whole group.
//!
//! System runs after `hierarchy_system` / `active_hierarchy_system` in `Engine::update`,
//! so same-frame extraction sees distance computed from current world matrix.

use prism_ecs::{Entity, World};

use crate::scene::components::{Camera, Child, LocalToWorld, LodGroup, LodHidden};

/// Write `LodHidden` for each `LodGroup` child entity by distance.
pub fn lod_system(world: &mut World) {
    let marked: Vec<Entity> = world
        .query::<LodHidden>()
        .map(|(entity, _)| entity)
        .collect();
    for entity in marked {
        if let Some(hidden) = world.get_mut::<LodHidden>(entity) {
            hidden.0 = false;
        }
    }

    let cameras: Vec<(glam::Vec3, f32, bool)> = world
        .query2::<Camera, LocalToWorld>()
        .filter(|(_, camera, _)| camera.enabled)
        .map(|(_, camera, world)| {
            let scale = camera.projection_matrix(camera.aspect).y_axis.y.abs();
            (world.0.w_axis.truncate(), scale, camera.orthographic)
        })
        .collect();

    let groups: Vec<(Entity, LodGroup, glam::Vec3)> = world
        .query2::<LodGroup, LocalToWorld>()
        .map(|(entity, group, world)| (entity, group.clone(), world.0.w_axis.truncate()))
        .collect();

    for (entity, group, position) in groups {
        let children: Vec<Entity> = world
            .get::<Child>(entity)
            .map(|children| children.0.clone())
            .unwrap_or_default();
        if children.is_empty() {
            continue;
        }
        let distance = cameras
            .iter()
            .map(|(eye, _, _)| (*eye - position).length())
            .fold(f32::INFINITY, f32::min);

        let radius = group.bounds_radius;
        let use_screen_ratio = !group.screen_ratios.is_empty()
            && radius.is_finite()
            && radius > 0.0
            && !cameras.is_empty();
        let level = if use_screen_ratio {
            let ratio = cameras
                .iter()
                .map(|(eye, scale, orthographic)| {
                    if *orthographic {
                        radius * *scale
                    } else {
                        let distance = (*eye - position).length();
                        if distance > 1e-6 {
                            radius * *scale / distance
                        } else {
                            1.0
                        }
                    }
                })
                .fold(0.0, f32::max);
            group
                .level_for_screen_ratio(ratio, children.len())
                .or_else(|| group.level_for_distance(distance, children.len()))
        } else {
            group.level_for_distance(distance, children.len())
        };
        let Some(level) = level else {
            continue;
        };
        for (index, child) in children.iter().enumerate() {
            world.insert(*child, LodHidden(index != level));
        }
    }
}

#[cfg(test)]
#[path = "lod_tests.rs"]
mod tests;
