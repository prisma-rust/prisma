//! Scene render system — collects [`DrawItem`]s from ECS entities.
//!
//! Iterates entities that carry [`LocalToWorld`] and [`MeshRef`] and are
//! The resulting active set is fed to [`prism_render`]'s [`GraphRenderer`] each frame

use prism_ecs::World;
use prism_render::culling::{Aabb, Frustum};
use prism_render::{DrawItem, DrawState};

use crate::scene::components::*;

/// Construct draw state from `MeshRenderer`'s render state fields (material / renderer state).
///
/// Runtime entities without that component use [`DrawState::default`] (opaque, queue 2000,
/// back-face cull, depth write).
pub(crate) fn draw_state(renderer: Option<&MeshRenderer>) -> DrawState {
    match renderer {
        Some(renderer) => DrawState {
            cull_mode: renderer.cull_mode,
            render_queue: renderer.render_queue.value(),
            z_write: renderer.z_write,
            src_blend: renderer.src_blend,
            dst_blend: renderer.dst_blend,
            blend_op: renderer.blend_op,
        },
        None => DrawState::default(),
    }
}

/// Collect all visible draw items from the ECS [`World`].
///
/// An entity produces a `DrawItem` if and only if it has all of:
/// - [`LocalToWorld`]
/// - [`MeshRef`]
/// - [`Active(true)`]
///
/// [`MaterialRef`] is **optional**: a mesh-only renderer (empty
/// `MeshRenderer::material_path`, or an entity spawned by game code without a
/// resolved material) still draws, with [`DrawItem::material`] left as `None`
/// so the renderer falls back to its default material slot.
///
/// Entities without an `Active` component default to active (the component
/// defaults to `true`).
pub fn scene_render_system(world: &World) -> Vec<DrawItem> {
    let mut out = Vec::new();
    scene_render_system_into(world, &mut out);
    out
}

/// [`scene_render_system`] into a caller-owned buffer.
///
/// The frame packet keeps this `Vec` across frames, so `clear()` + `extend`
/// reuses the allocation instead of doing one `collect()` per frame (#66).
pub fn scene_render_system_into(world: &World, out: &mut Vec<DrawItem>) {
    scene_render_system_into_with_frustum(world, None, out);
}

/// Mesh local bounding box table (`MeshHandle → local AABB`), written back by asset parsing.
///
/// Stored in a `World` resource rather than on each [`MeshRef`]: when one mesh is reused by multiple entities only one copy is stored,
/// and hot reload swapping handles updates only one record. Meshes with no record **do not participate in culling** (keep
/// old behavior when data unknown), so procedural meshes (calibration spheres, gizmos) still draw as before.
#[derive(Debug, Default)]
pub struct MeshBounds {
    bounds:
        std::collections::HashMap<prism_render::managers::MeshHandle, prism_render::culling::Aabb>,
}

impl MeshBounds {
    pub fn insert(
        &mut self,
        handle: prism_render::managers::MeshHandle,
        bounds: prism_render::culling::Aabb,
    ) {
        self.bounds.insert(handle, bounds);
    }

    pub fn get(
        &self,
        handle: prism_render::managers::MeshHandle,
    ) -> Option<&prism_render::culling::Aabb> {
        self.bounds.get(&handle)
    }

    pub fn len(&self) -> usize {
        self.bounds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bounds.is_empty()
    }
}

/// [`scene_render_system_into`] with an optional camera frustum (#491).
///
/// When `frustum == None` fully consistent with old behavior (no culling). With frustum, mesh's local
/// bounding box transformed to world space by entity model matrix then conservative intersection test; **entities without bounding box or with non-
/// finite transform** submitted as before (see [`MeshBounds`]).
///
/// For multiple cameras (#461) use [`scene_render_system_into_with_frustums`]: culling only by main camera
/// would miss objects visible only in second camera.
pub fn scene_render_system_into_with_frustum(
    world: &World,
    frustum: Option<&Frustum>,
    out: &mut Vec<DrawItem>,
) {
    match frustum {
        Some(frustum) => {
            scene_render_system_into_with_frustums(world, std::slice::from_ref(frustum), out)
        }
        None => scene_render_system_into_with_frustums(world, &[], out),
    }
}

/// [`scene_render_system_into_with_frustum`] for a **set** of camera frustums.
///
/// Under multi-camera composition (#461), an entity must be submitted if inside **any** enabled camera's frustum:
/// cameras at each layer share same draw list; things main camera cannot see but second camera can
/// must not be culled early. Empty slice means no culling.
///
/// Entities without world bounding box (procedural mesh / handle not yet parsed) stay submitted — same as single-camera path,
/// do not risk making objects disappear when data unknown.
pub fn scene_render_system_into_with_frustums(
    world: &World,
    frustums: &[Frustum],
    out: &mut Vec<DrawItem>,
) {
    out.clear();
    let bounds = world.get_resource::<MeshBounds>();
    for (entity, wt, mr) in world.query2::<LocalToWorld, MeshRef>() {
        // Skinned entities go through `skinned_draw::append_skinned_draw_items` (#560 P0-3b):
        // same entity with two render components at once still drawn once.
        if world
            .get::<crate::scene::components::SkinnedMeshRenderer>(entity)
            .is_some()
        {
            continue;
        }
        if !crate::scene::systems::hierarchy::is_active_in_hierarchy(world, entity) {
            continue;
        }
        let model = wt.0.to_cols_array_2d();
        let world_bounds = bounds
            .and_then(|bounds| bounds.get(mr.render_handle))
            .and_then(|local| local.transformed(&model));
        let renderer = world.get::<MeshRenderer>(entity);
        let mode = renderer
            .map(|renderer| renderer.shadow_casting_mode)
            .unwrap_or_default();
        let visible_in_main_pass = visible_in_frustums(frustums, world_bounds.as_ref());
        if !visible_in_main_pass && !mode.casts_shadows() {
            continue;
        }

        // `MeshRenderer.shadow_casting_mode` / `cull_mode` (
        // `shadowCastingMode` / `CullMode`). Runtime entities without that component use
        // defaults (`On` / `Back`).
        let state = draw_state(renderer);
        let material_ref = world.get::<MaterialRef>(entity);
        let material_slots = world.get::<MaterialSlots>(entity);
        let shader = if renderer.is_some_and(|r| r.shader_path.is_empty()) {
            None
        } else {
            world
                .get::<crate::scene::runtime_components::ShaderRef>(entity)
                .filter(|s| s.generation > 0)
                .map(|s| s.shader_handle)
        };
        let submeshes = world.get::<MeshSubmeshes>(entity);
        let mut splat_slots = [u32::MAX; 4];
        let splat_tiling = if let Some(splat) =
            world.get::<crate::scene::systems::terrain::TerrainSplat>(entity)
        {
            if let Some(slots) = world.get::<MaterialSlots>(entity) {
                for (index, slot) in slots.slots.iter().take(4).enumerate() {
                    if let Some(slot) = slot {
                        splat_slots[index] = *slot;
                    }
                }
            }
            splat.tiling
        } else {
            0.0
        };

        // With submesh table split into multiple DrawItems by range (#520); otherwise keep old behavior:
        // one entity one draw, covering entire index buffer.
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
                    // Skinned draw items generated by dedicated `SkinnedMeshRenderer` branch (P0-3b).
                    bone_offset: None,
                    splat_slots,
                    splat_tiling,
                    shader,
                });
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
                // Skinned draw items generated by dedicated `SkinnedMeshRenderer` branch (P0-3b).
                bone_offset: None,
                splat_slots,
                splat_tiling,
                shader,
            });
        }
    }
}

/// Whether one world bounding box is inside **at least one** camera frustum (#491).
///
/// Empty frustum set = no culling; `bounds == None` also stays submitted (data unknown).
pub(crate) fn visible_in_frustums(frustums: &[Frustum], bounds: Option<&Aabb>) -> bool {
    if frustums.is_empty() {
        return true;
    }
    match bounds {
        Some(bounds) => frustums
            .iter()
            .any(|frustum| frustum.intersects_aabb(bounds)),
        None => true,
    }
}

/// Cull shadow casters by **light source frustum** (#526).
///
/// `light_view_proj` is this frame's directional light shadow box light-space matrix; shadow pass only needs to cover this
/// box. Entities without world AABB (procedural mesh / handle not yet parsed) keep `cast_shadow`
/// unchanged — same as main camera culling, do not risk making shadows disappear when data unknown.
pub fn cull_shadow_casters(items: &mut [DrawItem], light_view_proj: &[[f32; 4]; 4]) {
    cull_shadow_casters_multi(items, std::slice::from_ref(light_view_proj));
}

/// Multi-cascade version: caster kept if inside **any one** cascade box (#526 CSM).
///
/// Degenerate matrices are skipped; if all matrices degenerate, conservatively cull nothing.
pub fn cull_shadow_casters_multi(items: &mut [DrawItem], light_view_projs: &[[[f32; 4]; 4]]) {
    let frustums: Vec<_> = light_view_projs
        .iter()
        .filter_map(prism_render::culling::Frustum::from_view_proj)
        .collect();
    if frustums.is_empty() {
        return;
    }
    for item in items {
        if !item.cast_shadow {
            continue;
        }
        let Some(bounds) = item.world_bounds else {
            continue;
        };
        if !frustums
            .iter()
            .any(|frustum| frustum.intersects_aabb(&bounds))
        {
            item.cast_shadow = false;
        }
    }
}

/// Sort key for 2D draw items: `(sorting_layer, order_in_layer, z, entity_id)`.
pub(crate) type SortKey = (i32, i32, f32, u32);

/// Solid-color draw flag: when `SpriteDrawItem.texture` takes this value shader does not sample texture.
pub const SOLID_TEXTURE: u32 = u32::MAX;

/// Main-line camera world → render pixels ratio, used to widen sub-pixel lines to 1 pixel (#978).
///
/// Lines are triangulated on CPU into world-space quads; when width less than 1 pixel, pixel centers will
/// fall outside triangles and appear as dotted points on screen. Here convert "1 pixel = how many
/// world units" by main camera, giving `LineRenderer::width` a screen-space lower bound.
#[derive(Clone, Copy, Debug)]
pub struct LineScreenScale {
    /// Main camera `view_proj` (`surface_rotation * projection * view`).
    pub view_proj: glam::Mat4,
    /// Scene actual render pixel height (internal height when `internal_resolution` active).
    pub render_height: f32,
}

impl LineScreenScale {
    /// Only return ratio when pixel height is truly positive; otherwise caller keeps old behavior (no widening).
    pub fn new(view_proj: glam::Mat4, render_height: f32) -> Option<Self> {
        (render_height.is_finite() && render_height > 0.0 && view_proj.is_finite()).then_some(
            Self {
                view_proj,
                render_height,
            },
        )
    }

    /// World length corresponding to 1 render pixel near `point` (measured along world +Y).
    ///
    /// Independent of orthographic camera and position; for perspective camera converted by depth at that point. Point on camera plane
    /// (`w == 0`) or degenerate conversion returns `None`.
    pub fn world_per_pixel(&self, point: glam::Vec3) -> Option<f32> {
        let clip_a = self.view_proj * point.extend(1.0);
        let clip_b = self.view_proj * (point + glam::Vec3::Y).extend(1.0);
        if clip_a.w.abs() < 1e-6 || clip_b.w.abs() < 1e-6 {
            return None;
        }
        let ndc_delta = (clip_b.y / clip_b.w) - (clip_a.y / clip_a.w);
        let pixels = ndc_delta.abs() * 0.5 * self.render_height;
        (pixels.is_finite() && pixels > 1e-6).then(|| pixels.recip())
    }
}

/// Collect all 2D sprite draw items from the ECS world, sorted back-to-front.
///
/// An entity produces a [`SpriteDrawItem`](prism_render::SpriteDrawItem) when it
/// has [`LocalToWorld`], [`SpriteRenderer`] with a texture path, and a resolved
/// [`SpriteRef`] (`generation > 0`).
///
/// Sort key: `sorting_layer` → `order_in_layer` → world `z` → entity id.
/// Painter's algorithm draws far-to-near, so `z` ascending (camera looks −Z and
/// sits at +Z: larger `z` is closer).
pub fn scene_sprite_system(world: &World) -> Vec<prism_render::SpriteDrawItem> {
    sort_draw_items(sprite_entries(world))
}

/// Collect all 2D line draw items (`LineRenderer`) as non-overlapping
/// triangles (miter/bevel joins, butt/square/round caps — #604).
pub fn scene_line_system(world: &World) -> Vec<prism_render::SpriteDrawItem> {
    sort_draw_items(line_entries(world, None))
}

/// Every 2D draw item (sprites **and** lines) in one back-to-front list.
///
/// This is what the renderer consumes: sprites and lines share one pass, so
/// they must be sorted against each other, not in two independent lists.
pub fn scene_2d_system(world: &World) -> Vec<prism_render::SpriteDrawItem> {
    let mut out = Vec::new();
    scene_2d_system_into(world, &mut out);
    out
}

/// [`scene_2d_system`] into a caller-owned buffer (see
/// [`scene_render_system_into`] for why).
pub fn scene_2d_system_into(world: &World, out: &mut Vec<prism_render::SpriteDrawItem>) {
    scene_2d_system_into_with_frustum(world, None, out);
}

/// [`scene_2d_system_into`] plus main camera frustum: tilemap clipped per cell (#529).
///
/// sprite / polyline / particles do not participate in culling (they are small instance counts anyway); tile cells can be thousands,
/// clipping off-screen rows and columns significantly reduces instance buffer writes.
pub fn scene_2d_system_into_with_frustum(
    world: &World,
    frustum: Option<&prism_render::culling::Frustum>,
    out: &mut Vec<prism_render::SpriteDrawItem>,
) {
    scene_2d_system_into_with_frustum_and_line_scale(world, frustum, None, out);
}

/// [`scene_2d_system_into_with_frustum`] plus main-line camera pixel ratio (#978).
///
/// When `line_scale` is `Some`, lines narrower than 1 render pixel widened to 1 pixel,
/// otherwise thin lines appear dotted on screen. `None` keeps old behavior (pure world-space geometry).
pub fn scene_2d_system_into_with_frustum_and_line_scale(
    world: &World,
    frustum: Option<&prism_render::culling::Frustum>,
    line_scale: Option<LineScreenScale>,
    out: &mut Vec<prism_render::SpriteDrawItem>,
) {
    let mut entries = sprite_entries(world);
    entries.extend(line_entries(world, line_scale));
    // 2D particles are solid-color quads, share same channel and sort as sprites (#594).
    entries.extend(super::particles2d::particle_entries_2d(world));
    // tilemap: one entity one whole mesh (#529).
    entries.extend(super::tilemap::tilemap_entries(world, frustum));
    sort_draw_items_into(entries, out);
}

fn sort_draw_items(
    entries: Vec<(SortKey, prism_render::SpriteDrawItem)>,
) -> Vec<prism_render::SpriteDrawItem> {
    let mut out = Vec::new();
    sort_draw_items_into(entries, &mut out);
    out
}

fn sort_draw_items_into(
    mut entries: Vec<(SortKey, prism_render::SpriteDrawItem)>,
    out: &mut Vec<prism_render::SpriteDrawItem>,
) {
    entries.sort_by(|a, b| {
        let (ak, bk) = (&a.0, &b.0);
        ak.0.cmp(&bk.0)
            .then(ak.1.cmp(&bk.1))
            .then(ak.2.partial_cmp(&bk.2).unwrap_or(std::cmp::Ordering::Equal))
            .then(ak.3.cmp(&bk.3))
    });
    out.clear();
    out.extend(entries.into_iter().map(|(_, item)| item));
}

/// Collect [`Light2D`] in scene (#611), by contribution strongest to weakest, at most
/// [`prism_render::SPRITE_LIGHT_MAX`].
///
/// Only sprites with `SpriteRenderer.receive_lights == true` use this data, so
/// when no lights return empty list — shader sees `light_count == 0` and stays unlit.
///
/// 3D lights (`PointLight` / `DirectionalLight` / `SpotLight`) will not illuminate 2D sprites;
/// when only 3D lights present warn once here, avoid "component legal but totally ineffective" being silently swallowed.
pub fn sprite_light_entries(world: &World) -> Vec<prism_render::SpriteLight> {
    use std::sync::Once;
    static WARNED: Once = Once::new();

    let mut lights: Vec<prism_render::SpriteLight> = Vec::new();
    for (entity, transform, light) in world.query2::<LocalToWorld, Light2D>() {
        if !crate::scene::systems::hierarchy::is_active_in_hierarchy(world, entity)
            || !light.contributes()
        {
            continue;
        }
        let origin = transform.0.w_axis;
        let position = [origin.x, origin.y];
        let color = light.color;
        let intensity = light.strength();
        let radius = light.falloff_radius();
        lights.push(match light.kind {
            prism_scene::components::Light2DKind::Ambient => {
                prism_render::SpriteLight::ambient(color, intensity)
            }
            prism_scene::components::Light2DKind::Point => {
                prism_render::SpriteLight::point(position, color, intensity, radius)
            }
            prism_scene::components::Light2DKind::Directional => {
                prism_render::SpriteLight::directional(light.direction, color, intensity)
            }
        });
    }

    if lights.is_empty() {
        let has_3d_light = world
            .query::<prism_scene::components::PointLight>()
            .next()
            .is_some()
            || world
                .query::<prism_scene::components::SpotLight>()
                .next()
                .is_some()
            || world
                .query::<prism_scene::components::DirectionalLight>()
                .next()
                .is_some();
        if has_3d_light {
            WARNED.call_once(|| {
                log::warn!(
                    "2D sprites ignore 3D lights (PointLight / SpotLight / DirectionalLight); \
                     add a `Light2D` entity and enable `SpriteRenderer.receive_lights` (#611)"
                );
            });
        }
    }

    prism_render::trim_sprite_lights(&mut lights);
    lights
}

fn sprite_entries(world: &World) -> Vec<(SortKey, prism_render::SpriteDrawItem)> {
    let mut out = Vec::new();
    for (entity, world_transform, sprite) in world.query2::<LocalToWorld, SpriteRenderer>() {
        if !sprite.has_sprite()
            || !crate::scene::systems::hierarchy::is_active_in_hierarchy(world, entity)
        {
            continue;
        }
        let (texture_slot, texture_size, ref_uv_rect) = match world.get::<SpriteRef>(entity) {
            Some(sref) if sref.generation > 0 => (sref.texture_slot, sref.size, sref.uv_rect),
            _ if sprite.sprite_path == prism_scene::BUILTIN_WHITE_SPRITE => (
                prism_render::bindless::TextureHandle::WHITE.0,
                [1, 1],
                [0.0, 0.0, 1.0, 1.0],
            ),
            _ => continue,
        };
        let size = sprite.world_size(texture_size);
        // Place the entity origin on the sprite pivot (convention:
        // (0,0) is the bottom-left corner).
        let offset = glam::vec3(
            -(sprite.pivot[0] - 0.5) * size[0],
            -(sprite.pivot[1] - 0.5) * size[1],
            0.0,
        );
        let local = glam::Mat4::from_scale_rotation_translation(
            glam::vec3(size[0], size[1], 1.0),
            glam::Quat::IDENTITY,
            offset,
        );
        let model = world_transform.0 * local;
        let key: SortKey = (
            sprite.sorting_layer,
            sprite.order_in_layer,
            world_transform.0.w_axis.z,
            entity.id(),
        );
        let uv_rect = if ref_uv_rect != [0.0, 0.0, 1.0, 1.0] {
            let mut uv = ref_uv_rect;
            if sprite.flip_x {
                uv.swap(0, 2);
            }
            if sprite.flip_y {
                uv.swap(1, 3);
            }
            uv
        } else {
            sprite_uv_rect(sprite, texture_size)
        };
        out.push((
            key,
            prism_render::SpriteDrawItem {
                model: model.to_cols_array_2d(),
                corners: None,
                uv_rect,
                color: sprite.color,
                texture: texture_slot,
                src_blend: sprite.src_blend,
                dst_blend: sprite.dst_blend,
                blend_op: sprite.blend_op,
                clip: prism_render::clip_bounds(sprite.clip_enabled, sprite.clip_rect),
                receive_lights: sprite.receive_lights,
            },
        ));
    }
    out
}

fn line_entries(
    world: &World,
    line_scale: Option<LineScreenScale>,
) -> Vec<(SortKey, prism_render::SpriteDrawItem)> {
    let mut out = Vec::new();
    for (entity, line) in world.query::<LineRenderer>() {
        let active = crate::scene::systems::hierarchy::is_active_in_hierarchy(world, entity);
        if !active || !line.has_line() {
            continue;
        }
        let width = if line.width.is_finite() && line.width > 0.0 {
            line.width
        } else {
            0.1
        };
        // Sub-pixel lines widened to 1 render pixel (#978): when pixel center outside triangle,
        // screen shows evenly spaced dots rather than a line. Ratio converted by depth of line's first point under main camera
        // (independent of orthographic camera and position).
        let width = match line_scale
            .and_then(|scale| scale.world_per_pixel(glam::Vec3::from_array(line.points[0])))
        {
            Some(world_per_pixel) => width.max(world_per_pixel),
            None => width,
        };
        let z = world
            .get::<LocalToWorld>(entity)
            .map(|t| t.0.w_axis.z)
            .unwrap_or(0.0);
        // All triangles of same polyline share sort key: `sort_by` is stable, triangles drawn in
        // generation order, and they don't overlap, so internal order doesn't affect result (#604).
        let key: SortKey = (line.sorting_layer, line.order_in_layer, z, entity.id());
        let model = glam::Mat4::IDENTITY.to_cols_array_2d();

        for triangle in tessellate_line(&line.points, width, line.join, line.cap, line.miter_limit)
        {
            out.push((
                key,
                prism_render::SpriteDrawItem {
                    model,
                    corners: Some([triangle[0], triangle[1], triangle[2], triangle[2]]),
                    uv_rect: [0.0, 0.0, 1.0, 1.0],
                    color: line.color,
                    texture: SOLID_TEXTURE,
                    // Polyline has no authored blend field: keep standard alpha blend.
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

/// Triangulate a world-space polyline into **non-overlapping** stroke (#604).
///
/// Each segment is a quad (two triangles). At joints:
///
/// - Inner two segments share same vertex (intersection of two inner offset lines), so inner side has neither gap
///   nor overlap;
/// - `Miter` outer side also shares miter point; when exceeding `miter_limit` (relative half width) degenerates to
///   `Bevel`, outer segments stop at own offset points, then fill corner with one triangle.
///
/// So each pixel of semi-transparent polyline is blended once. `cap` only affects two endpoints:
/// `Butt` / `Square` / `Round` (8-segment fan).
///
/// Consecutive duplicate points in input are removed first — zero-length segments have no direction and no area.
pub(crate) fn tessellate_line(
    points: &[[f32; 3]],
    width: f32,
    join: LineJoin,
    cap: LineCap,
    miter_limit: f32,
) -> Vec<[[f32; 3]; 3]> {
    use glam::Vec2;

    let mut pts: Vec<(Vec2, f32)> = Vec::with_capacity(points.len());
    for point in points {
        let v = Vec2::new(point[0], point[1]);
        if pts
            .last()
            .is_none_or(|(last, _)| last.distance_squared(v) > 1e-10)
        {
            pts.push((v, point[2]));
        }
    }
    if pts.len() < 2 {
        return Vec::new();
    }

    let half = (width * 0.5).max(1e-5);
    let miter_limit = if miter_limit.is_finite() && miter_limit >= 1.0 {
        miter_limit
    } else {
        4.0
    };
    let n = pts.len();
    let dirs: Vec<Vec2> = pts
        .windows(2)
        .map(|pair| (pair[1].0 - pair[0].0).normalize())
        .collect();
    // left normal, length = half line width.
    let normals: Vec<Vec2> = dirs
        .iter()
        .map(|dir| Vec2::new(-dir.y, dir.x) * half)
        .collect();

    // `start[i]` = (left, right) on both sides of segment i start; `end[i]` = both sides of segment i end.
    let mut start: Vec<(Vec2, Vec2)> = vec![(Vec2::ZERO, Vec2::ZERO); n];
    let mut end: Vec<(Vec2, Vec2)> = vec![(Vec2::ZERO, Vec2::ZERO); n];
    let mut bevels: Vec<(Vec2, Vec2, Vec2)> = Vec::new();

    // Endpoints: square cap extends half line width outward along segment direction, round cap filled by later fan.
    let shift = |dir: Vec2| match cap {
        LineCap::Square => dir * half,
        _ => Vec2::ZERO,
    };
    start[0] = (
        pts[0].0 + normals[0] - shift(dirs[0]),
        pts[0].0 - normals[0] - shift(dirs[0]),
    );
    end[n - 2] = (
        pts[n - 1].0 + normals[n - 2] + shift(dirs[n - 2]),
        pts[n - 1].0 - normals[n - 2] + shift(dirs[n - 2]),
    );

    for i in 1..n - 1 {
        let point = pts[i].0;
        let (n0, n1) = (normals[i - 1], normals[i]);
        let cross = dirs[i - 1].perp_dot(dirs[i]);
        // `n` is left normal: right turn (cross < 0) puts outer side on left edge.
        let outer_sign = if cross < 0.0 { 1.0 } else { -1.0 };
        let sum = n0.normalize_or_zero() + n1.normalize_or_zero();
        // When turn approaches 180° two offset lines nearly parallel, miter point runs to infinity.
        let cos_half = (sum * 0.5).length();
        let bisector = sum.normalize_or_zero();
        let miter = if cos_half > 1e-3 && bisector != Vec2::ZERO {
            half / cos_half
        } else {
            f32::INFINITY
        };

        if join == LineJoin::Miter && miter <= half * miter_limit {
            let offset = bisector * miter * outer_sign;
            start[i] = (point + offset, point - offset);
            end[i - 1] = start[i];
            continue;
        }

        // Bevel: inner side shares miter point, outer segments stop at own offset points.
        let inner = point - bisector * miter * outer_sign;
        let outer_prev = point + n0 * outer_sign;
        let outer_next = point + n1 * outer_sign;
        if outer_sign > 0.0 {
            end[i - 1] = (outer_prev, inner);
            start[i] = (outer_next, inner);
        } else {
            end[i - 1] = (inner, outer_prev);
            start[i] = (inner, outer_next);
        }
        bevels.push((point, outer_prev, outer_next));
    }

    let z_at = |index: usize| pts[index].1;
    let mut out: Vec<[[f32; 3]; 3]> = Vec::new();
    let point3 = |v: Vec2, z: f32| [v.x, v.y, z];

    for index in 0..n - 1 {
        let (left0, right0) = start[index];
        let (left1, right1) = end[index];
        let z = z_at(index);
        out.push([point3(left0, z), point3(right0, z), point3(right1, z)]);
        out.push([point3(left0, z), point3(right1, z), point3(left1, z)]);
    }

    for (point, outer_prev, outer_next) in bevels {
        let z = z_at(0);
        out.push([
            point3(point, z),
            point3(outer_prev, z),
            point3(outer_next, z),
        ]);
    }

    if cap == LineCap::Round {
        const CAP_SEGMENTS: usize = 8;
        let rotate = |v: Vec2, angle: f32| {
            let (sin, cos) = angle.sin_cos();
            Vec2::new(v.x * cos - v.y * sin, v.x * sin + v.y * cos)
        };
        // Start sweeps toward -dir, end toward +dir: +90° turns from n to -dir, -90° to +dir.
        for (index, sign) in [(0usize, 1.0f32), (n - 1, -1.0)] {
            let point = pts[index].0;
            let z = z_at(index);
            let normal = normals[index.min(n - 2)];
            for step in 0..CAP_SEGMENTS {
                let a0 = std::f32::consts::PI * step as f32 / CAP_SEGMENTS as f32;
                let a1 = std::f32::consts::PI * (step + 1) as f32 / CAP_SEGMENTS as f32;
                out.push([
                    point3(point, z),
                    point3(point + rotate(normal, a0 * sign), z),
                    point3(point + rotate(normal, a1 * sign), z),
                ]);
            }
        }
    }

    out
}

/// UV rect `(u0, v0, u1, v1)` of one sprite-sheet frame.
///
/// `v = 0` is the texture's first row (image top), matching `sprites.slang`'s
/// vertex mapping; `flip_*` swaps the corresponding edges so no extra vertex
/// transform is needed.
fn sprite_uv_rect(sprite: &SpriteRenderer, texture_size: [u32; 2]) -> [f32; 4] {
    let tex_w = texture_size[0].max(1) as f32;
    let tex_h = texture_size[1].max(1) as f32;
    if sprite.region_px[2] > 0 && sprite.region_px[3] > 0 {
        let frame_w = sprite.region_px[2] as f32;
        let frame_h = sprite.region_px[3] as f32;
        let columns = if sprite.sheet_columns > 0 {
            sprite.sheet_columns.max(1)
        } else {
            ((tex_w - sprite.region_px[0] as f32).max(frame_w) / frame_w) as u32
        }
        .max(1);
        let column = sprite.frame_index % columns;
        let x0 = sprite.region_px[0] as f32 + column as f32 * frame_w;
        let y0 = sprite.region_px[1] as f32;
        let mut u0 = (x0 / tex_w).clamp(0.0, 1.0);
        let mut u1 = ((x0 + frame_w) / tex_w).clamp(0.0, 1.0);
        let mut v0 = (y0 / tex_h).clamp(0.0, 1.0);
        let mut v1 = ((y0 + frame_h) / tex_h).clamp(0.0, 1.0);
        if sprite.flip_x {
            std::mem::swap(&mut u0, &mut u1);
        }
        if sprite.flip_y {
            std::mem::swap(&mut v0, &mut v1);
        }
        return [u0, v0, u1, v1];
    }

    let frame = sprite.frame_size_or(texture_size);
    let frame_w = frame[0].max(1) as f32;
    let frame_h = frame[1].max(1) as f32;

    // Sheets are laid out row-major. `frame_index` counts within the selected
    // row first; only an index past the last column continues to the next row.
    let columns = sprite.sheet_columns_or(texture_size);
    let rows = sprite.sheet_rows(texture_size);
    let row = sprite.sheet_row.min(rows - 1);
    let column = sprite.frame_index % columns;
    let row = (row + sprite.frame_index / columns).min(rows - 1);

    let mut u0 = column as f32 * frame_w / tex_w;
    let mut u1 = ((column as f32 + 1.0) * frame_w / tex_w).min(1.0);
    let mut v0 = row as f32 * frame_h / tex_h;
    let mut v1 = ((row as f32 + 1.0) * frame_h / tex_h).min(1.0);

    if sprite.flip_x {
        std::mem::swap(&mut u0, &mut u1);
    }
    if sprite.flip_y {
        std::mem::swap(&mut v0, &mut v1);
    }
    [u0, v0, u1, v1]
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
