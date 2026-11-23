// Skinning (#560 P0-3b): hook `SkinnedMeshRenderer` entities into draw items.
//
// Separate file instead of stuffing into `render.rs`: static mesh loop already full, this branch
// also needs to append each entity bone palette into frame pack.

use prism_ecs::{Entity, World};
use prism_render::culling::{Aabb, Frustum};
use prism_render::render_graph::DrawItem;

use super::animator::BonePalette;
use super::render::{draw_state, visible_in_frustums};
use crate::scene::components::{
    LocalToWorld, MaterialRef, MaterialSlots, MeshRef, MeshSubmeshes, SkinnedMeshRenderer,
};
use crate::scene::systems::render::MeshBounds;

/// Max bone matrices per frame (consistent with `ForwardPass::BONE_CAPACITY`).
///
/// Entities over budget fall back to bind pose (`bone_offset = None`) and warn once, rather than
/// letting multiple instance palettes overwrite each other.
const BONE_BUDGET: usize = prism_render::forward_pass::ForwardPass::BONE_CAPACITY;

/// Append `SkinnedMeshRenderer` entities to draw list, collect their bone palettes.
///
/// `bones` is whole-frame shared palette buffer: each entity appends its `BonePalette` matrices at
/// tail, draw item only records `bone_offset` (vertex stage indexes SSBO by it), so same skeleton
/// reused by multiple meshes needs no copy.
///
/// Return count of appended draw items.
pub fn append_skinned_draw_items(
    world: &World,
    frustum: Option<&Frustum>,
    out: &mut Vec<DrawItem>,
    bones: &mut Vec<[[f32; 4]; 4]>,
) -> usize {
    match frustum {
        Some(frustum) => append_skinned_draw_items_with_frustums(
            world,
            std::slice::from_ref(frustum),
            out,
            bones,
        ),
        None => append_skinned_draw_items_with_frustums(world, &[], out, bones),
    }
}

/// [`append_skinned_draw_items`] for a set of camera frustums (#491).
///
/// Consistent with static mesh: keep if any camera can see, empty slice = not culled.
pub fn append_skinned_draw_items_with_frustums(
    world: &World,
    frustums: &[Frustum],
    out: &mut Vec<DrawItem>,
    bones: &mut Vec<[[f32; 4]; 4]>,
) -> usize {
    let bounds = world.get_resource::<MeshBounds>();
    let mut appended = 0;

    for (entity, wt, mr, skinned) in world.query3::<LocalToWorld, MeshRef, SkinnedMeshRenderer>() {
        if !crate::scene::systems::hierarchy::is_active_in_hierarchy(world, entity) {
            continue;
        }
        let model = wt.0.to_cols_array_2d();
        let palette = world
            .get::<BonePalette>(entity)
            .filter(|palette| !palette.matrices.is_empty());
        // Cull uses **post-skinning** bounding box: bind pose box is off when character in large pose,
        // using it would cull character still on screen (#560 P0-3c).
        let local_bounds = bounds
            .and_then(|bounds| bounds.get(mr.render_handle))
            .map(|bind| match palette {
                Some(palette) => skinned_local_bounds(bind, &palette.matrices).unwrap_or(*bind),
                None => *bind,
            });
        let world_bounds = local_bounds.and_then(|local| local.transformed(&model));
        let mode = skinned.shadow_casting_mode;
        let visible_in_main_pass = visible_in_frustums(frustums, world_bounds.as_ref());
        if !visible_in_main_pass && !mode.casts_shadows() {
            continue;
        }

        let bone_offset = append_palette(palette, entity, BONE_BUDGET, bones);
        let renderer = skinned.as_mesh_renderer();
        let state = draw_state(Some(&renderer));
        let material_ref = world.get::<MaterialRef>(entity);
        let material_slots = world.get::<MaterialSlots>(entity);
        let submeshes = world.get::<MeshSubmeshes>(entity);
        let shader = if skinned.shader_path.is_empty() {
            None
        } else {
            world
                .get::<crate::scene::runtime_components::ShaderRef>(entity)
                .filter(|s| s.generation > 0)
                .map(|s| s.shader_handle)
        };

        let has_submeshes = submeshes.is_some_and(|submeshes| !submeshes.submeshes.is_empty());
        if has_submeshes {
            for (index, submesh) in submeshes.unwrap().submeshes.iter().enumerate() {
                let material = match material_slots {
                    Some(slots) => slots.slots.get(index).copied().flatten(),
                    None => material_ref.map(|material| material.material_slot),
                };
                out.push(DrawItem {
                    mesh: mr.render_handle,
                    entity_id: entity.id(),
                    model,
                    material,
                    cast_shadow: mode.casts_shadows(),
                    visible_in_main_pass: visible_in_main_pass && mode.visible_in_main_pass(),
                    cast_shadow_two_sided: mode.casts_two_sided_shadows(),
                    state,
                    world_bounds,
                    index_start: submesh.index_start,
                    index_count: Some(submesh.index_count),
                    bone_offset,
                    splat_slots: [u32::MAX; 4],
                    splat_tiling: 0.0,
                    shader,
                });
                appended += 1;
            }
        } else {
            let material = match material_slots {
                Some(slots) => slots.slots.first().copied().flatten(),
                None => material_ref.map(|material| material.material_slot),
            };
            out.push(DrawItem {
                mesh: mr.render_handle,
                entity_id: entity.id(),
                model,
                material,
                cast_shadow: mode.casts_shadows(),
                visible_in_main_pass: visible_in_main_pass && mode.visible_in_main_pass(),
                cast_shadow_two_sided: mode.casts_two_sided_shadows(),
                state,
                world_bounds,
                index_start: 0,
                index_count: None,
                bone_offset,
                splat_slots: [u32::MAX; 4],
                splat_tiling: 0.0,
                shader,
            });
            appended += 1;
        }
    }

    appended
}

/// Append one entity bone palette, return its start index in whole-frame buffer.
///
/// Return `None` when no `BonePalette` (animation system not run / missing skeleton) or over budget,
/// draw item rendered in bind pose.
fn append_palette(
    palette: Option<&BonePalette>,
    entity: Entity,
    budget: usize,
    bones: &mut Vec<[[f32; 4]; 4]>,
) -> Option<u32> {
    let palette = palette?;
    if palette.matrices.is_empty() {
        return None;
    }
    if bones.len() + palette.matrices.len() > budget {
        log::warn!(
            "skinning: bone palette budget exhausted ({} + {} > {budget}); \
             entity {} is drawn in its bind pose",
            bones.len(),
            palette.matrices.len(),
            entity.id()
        );
        return None;
    }
    let offset = bones.len() as u32;
    bones.extend_from_slice(&palette.matrices);
    Some(offset)
}

/// Post-skinning local bounding box: transform bind box by each bone matrix in palette then union.
///
/// Skinned vertex = `Σ wᵢ · (Mᵢ · p)` (non-negative weights sum to 1), therefore it must fall in
/// convex hull of `{Mᵢ · p}` — union is conservative upper bound, will not cull visible geometry.
/// Return `None` when palette empty or all matrices non-finite, caller falls back to bind box.
fn skinned_local_bounds(bind: &Aabb, palette: &[[[f32; 4]; 4]]) -> Option<Aabb> {
    let mut merged: Option<Aabb> = None;
    for matrix in palette {
        if let Some(bounds) = bind.transformed(matrix) {
            merged = match merged {
                Some(accumulated) => accumulated.union(&bounds),
                None => Some(bounds),
            };
        }
    }
    merged
}

#[cfg(test)]
#[path = "skinned_draw_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "skinned_draw_bounds_tests.rs"]
mod bounds_tests;
