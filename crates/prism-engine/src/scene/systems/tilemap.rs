//! 2D tile map (`Tilemap` → `SpriteDrawItem`, #529).
//!
//! One mesh occupies one entity: this expands **non-empty** cells into quads, hands them to `SpritePass`
//! instance buffer, sharing the same pass and sort key with sprites / lines / 2D particles
//! (`sorting_layer → order_in_layer → z → entity id`).
//!
//! Clip: when main camera frustum present, per-cell AABB intersection test (only culls rows/cols, no LOD / chunk).
//! Without frustum (headless, tests, offline tools) output all non-empty cells.

use glam::{Mat4, Quat, Vec3};
use prism_ecs::World;
use prism_render::culling::{Aabb, Frustum};
use prism_render::SpriteDrawItem;
use prism_scene::components::{LocalToWorld, Tilemap};

use super::render::SortKey;
use crate::scene::runtime_components::TilemapRef;

/// Draw items for one tile map entity.
///
/// `tileset.generation == 0` (not yet uploaded) or empty mesh returns empty list.
pub(crate) fn tilemap_draw_items(
    tilemap: &Tilemap,
    tileset: &TilemapRef,
    transform: &Mat4,
    entity_id: u32,
    frustum: Option<&Frustum>,
) -> Vec<(SortKey, SpriteDrawItem)> {
    if !tilemap.has_tiles() || tileset.generation == 0 {
        return Vec::new();
    }
    let (columns, rows) = tilemap.grid();
    let cells = columns as usize * rows as usize;
    let (cell_w, cell_h) = (tilemap.cell_size[0], tilemap.cell_size[1]);
    let mut out = Vec::new();

    for row in 0..rows {
        for column in 0..columns {
            let index = (row as usize) * (columns as usize) + column as usize;
            if index >= cells || index >= tilemap.tiles.len() {
                break;
            }
            let tile = tilemap.tiles[index];
            if tile == 0 {
                continue;
            }
            let center = tilemap.cell_center(column, row, rows);
            let model = *transform
                * Mat4::from_scale_rotation_translation(
                    Vec3::new(cell_w, cell_h, 1.0),
                    Quat::IDENTITY,
                    Vec3::new(center[0], center[1], 0.0),
                );

            if let Some(frustum) = frustum {
                if !frustum.intersects_aabb(&cell_bounds(&model)) {
                    continue;
                }
            }

            let key = (
                tilemap.sorting_layer,
                tilemap.order_in_layer,
                model.w_axis.z,
                entity_id,
            );
            out.push((
                key,
                SpriteDrawItem {
                    model: model.to_cols_array_2d(),
                    corners: None,
                    uv_rect: tilemap.tile_uv(tile - 1, tileset.size),
                    color: tilemap.color,
                    texture: tileset.texture_slot,
                    src_blend: prism_scene::render_enums::BlendMode::SrcAlpha,
                    dst_blend: prism_scene::render_enums::BlendMode::OneMinusSrcAlpha,
                    blend_op: prism_scene::render_enums::BlendOp::Add,
                    clip: prism_render::NO_CLIP,
                    receive_lights: false,
                },
            ));
        }
    }
    out
}

/// **World-space** AABB of one cell (min/max after transforming four corners of identity quad).
///
/// `model` already contains cell size and entity transform, so this uses `±0.5` local coords,
/// must not multiply `cell_size` again.
fn cell_bounds(model: &Mat4) -> Aabb {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for (x, y) in [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
        let point = model.transform_point3(Vec3::new(x, y, 0.0));
        for axis in 0..3 {
            min[axis] = min[axis].min(point[axis]);
            max[axis] = max[axis].max(point[axis]);
        }
    }
    Aabb { min, max }
}

/// Draw items for all `Tilemap` entities in world (with frustum clip).
pub(crate) fn tilemap_entries(
    world: &World,
    frustum: Option<&Frustum>,
) -> Vec<(SortKey, SpriteDrawItem)> {
    let mut out = Vec::new();
    for (entity, transform, tilemap) in world.query2::<LocalToWorld, Tilemap>() {
        if !crate::scene::systems::hierarchy::is_active_in_hierarchy(world, entity) {
            continue;
        }
        let Some(tileset) = world.get::<TilemapRef>(entity) else {
            continue;
        };
        out.extend(tilemap_draw_items(
            tilemap,
            tileset,
            &transform.0,
            entity.id(),
            frustum,
        ));
    }
    out
}

#[cfg(test)]
#[path = "tilemap_tests.rs"]
mod tests;
