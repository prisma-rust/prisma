//! ECS-driven render system for RenderGraph path
//!
//! Defines [`SceneChanges`] snapshot struct (camera, lights, derived matrices)
//! and main [`render_system`] function, which queries ECS world each frame,
//! builds flat draw lists and submits to [`GraphRenderer::render`].

use glam::{self, Mat4, Vec3, Vec4};

use crate::dirty_router::{draw_items_signature, DirtyRouter};
use crate::render_settings::{RenderSettings, DEFAULT_SHADOW_DISTANCE};
use crate::scene;
use crate::scene::components::Camera;
use prism_ecs::World;
use prism_render::{
    DrawItem, FrameUBOData, GizmoInstance, GpuLight, GraphRenderer, PtAnalyticLight,
    SkyboxSettings, UiOverlayInput, GPU_LIGHT_KIND_POINT, GPU_LIGHT_KIND_SPOT, PT_LIGHT_MAX,
};
use prism_scene::CameraClearFlags;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Count of "dropped local lights" in one hint (warn only when count changes, avoid spam).
static LAST_DROPPED_LIGHTS: AtomicUsize = AtomicUsize::new(0);

/// Hint once when local lights exceed `LIGHT_MAX` (#468).
///
/// The cap itself is a hard constraint for forward shader (fixed-length SSBO), but user shouldn't have to guess
/// after "added a light but no effect". Warning every frame would spam, so only log when dropped count **changes**.
mod shadow;
pub use shadow::{dir_to_euler_xyz_deg, euler_xyz_deg_to_dir};
use shadow::{directional_light_ubo_direction, shadow_cascade_matrices};
#[cfg(test)]
mod tests;

fn warn_dropped_lights(total: usize, dropped: usize) {
    let previous = LAST_DROPPED_LIGHTS.swap(dropped, Ordering::Relaxed);
    if dropped > 0 && dropped != previous {
        log::warn!(
            "{dropped} of {total} local lights are not rendered: the forward pass only keeps the {} most important ones (intensity x range^2 / distance^2), see #468",
            prism_render::LIGHT_MAX
        );
    }
}

/// Pre-scale factor, replacing old GPU-side `exposure / PI` unit conversion.
/// Lux (or candela) multiplied by this factor on CPU, so shader receives effective radiance directly.
/// `exposure` afterwards becomes pure luminance multiplier applied to final composited HDR color.
const LUX_TO_RADIANCE_SCALE: f32 = 1.0 / (10_000.0 * std::f32::consts::PI);

/// Snapshot of all per-frame scene data (camera, lights, derived matrices).
///
/// Produced by [`collect_scene_changes`] and consumed by [`render_system`] to
/// build the [`FrameUBOData`] and [`FrameInput`].
#[derive(Clone, Default)]
pub struct SceneChanges {
    pub view_proj: Mat4,
    pub eye: Vec3,
    pub view: Mat4,
    /// Unrotated projection (GTAO pass needs the raw matrix for
    /// clip → view-space reconstruction; the surface-rotation is applied
    /// only to `view_proj`).
    pub projection: Mat4,
    pub inv_projection: Mat4,
    pub proj22: f32,
    pub proj32: f32,
    /// Direction TO the light, packed as `[x, y, z, intensity]`.
    pub light_direction: Vec4,
    /// Light colour + ambient, packed as `[r, g, b, ambient]`.
    pub light_color: Vec4,
    /// Enabled cameras composited this frame, ordered by `Camera.depth`
    /// ascending (#461). Always non-empty: a synthetic fallback entry keeps the
    /// "no camera" gray frame working.
    pub cameras: Vec<prism_render::render_graph::CameraView>,
    /// Light-space orthographic view-projection (shadow map).
    pub light_view_proj: Mat4,
    /// Far-cascade light-space VP (#526 CSM); only used when
    /// `shadow_cascade_count > 1`.
    pub light_view_proj1: Mat4,
    /// Camera-forward distance of the near/far cascade split.
    pub shadow_split: f32,
    /// 1 = single shadow map, 2 = two-cascade atlas.
    pub shadow_cascade_count: u32,
    /// Point lights collected from the ECS world (up to `LIGHT_MAX`).
    ///
    /// When over budget pick by importance (see [`lights::local_light_importance`]), not document order anymore.
    pub lights: Vec<GpuLight>,
    /// Total enabled local lights (point + spot) in scene, count before truncation.
    pub local_lights_total: usize,
    /// Actual number of local lights written to forward SSBO (≤ `LIGHT_MAX`).
    pub local_lights_kept: usize,
    /// Local light entities squeezed out by `LIGHT_MAX` (`.scene.json` ids; runtime entities have no id,
    /// only counted not named). Editor uses this to mark "this light isn't drawn" in checker.
    pub dropped_light_ids: Vec<u32>,
    /// Analytic lights for path tracing (directional + point + spot).
    pub pt_lights: Vec<PtAnalyticLight>,
    /// Exposure multiplier from the camera entity.
    pub exposure: f32,
    /// Whether a usable Camera entity was found in the ECS world.
    pub has_camera: bool,
    /// Whether active camera uses orthographic projection (`Camera::orthographic`). PT main ray needs it to
    /// decide between directional light and perspective cone (issue #10).
    pub camera_orthographic: bool,
    /// Clear color = active camera's `background_color` (engine default gray when no camera).
    pub clear_color: [f32; 4],
    /// Clear mode = active camera's `Camera.clear_flags` (`Skybox` when no camera).
    /// Only `Skybox` draws skybox background, see [`CameraClearFlags::draws_skybox`].
    pub clear_flags: CameraClearFlags,
    /// Whether main directional light casts shadow = `Light.shadows != None`.
    ///
    /// When `false` skip shadow map render for whole frame (`ShadowMode::None`),
    /// shader also gets fully lit result due to `shadow_strength = 0`.
    pub shadows_enabled: bool,
    /// Shadow strength copied from `Light.shadow_strength`, clamped to `0..=1`.
    pub shadow_strength: f32,
}

/// Run the extract phase: scene changes + draw-item list
/// producing a [`FramePacket`] for later consumption by [`render_system`].
///
/// **This is the API the sim (game‑logic) thread calls.**  [`render_system`]
/// consumes the packet without accessing the ECS world.
///
/// Transforms are **not** propagated here: [`crate::engine::Engine::update`]
/// does it at the end of the simulation step, so the extract phase is read-only
/// with respect to `LocalToWorld` and gameplay never sees stale matrices.
pub fn extract_frame_packet(
    world: &mut World,
    display_aspect: f32,
    surface_rotation: &Mat4,
) -> FramePacket {
    extract_frame_packet_with_shadow(
        world,
        display_aspect,
        surface_rotation,
        DEFAULT_SHADOW_DISTANCE,
    )
}

/// [`extract_frame_packet`], shadow coverage distance given by caller (`RenderSettings::shadow_distance`).
pub fn extract_frame_packet_with_shadow(
    world: &mut World,
    display_aspect: f32,
    surface_rotation: &Mat4,
    shadow_distance: f32,
) -> FramePacket {
    let mut packet = FramePacket::default();
    extract_frame_packet_into_with_shadow(
        world,
        display_aspect,
        surface_rotation,
        shadow_distance,
        &mut packet,
    );
    packet
}

/// [`extract_frame_packet`] into a recycled packet.
///
/// Every collection in the packet is cleared and refilled, so a caller that
/// keeps one packet alive across frames — `prism-app` recycles it through the
/// render thread — reuses the allocations instead of building new `Vec`s every
/// frame (#66).
pub fn extract_frame_packet_into(
    world: &mut World,
    display_aspect: f32,
    surface_rotation: &Mat4,
    packet: &mut FramePacket,
) {
    extract_frame_packet_into_with_shadow(
        world,
        display_aspect,
        surface_rotation,
        DEFAULT_SHADOW_DISTANCE,
        packet,
    );
}

/// [`extract_frame_packet_into`] plus shadow coverage distance (meters, see `RenderSettings::shadow_distance`).
///
/// Both size and center of shadow orthographic box follow it, so host passes it each frame instead of letting engine
/// guess a constant (#526).
pub fn extract_frame_packet_into_with_shadow(
    world: &mut World,
    display_aspect: f32,
    surface_rotation: &Mat4,
    shadow_distance: f32,
    packet: &mut FramePacket,
) {
    extract_frame_packet_into_with_shadow_cascades(
        world,
        display_aspect,
        surface_rotation,
        shadow_distance,
        1,
        packet,
    );
}

/// [`extract_frame_packet_into_with_shadow`] plus an explicit cascade count
/// (1 or 2). Hosts pass `RenderSettings::shadow_cascade_count` (#526 CSM).
pub fn extract_frame_packet_into_with_shadow_cascades(
    world: &mut World,
    display_aspect: f32,
    surface_rotation: &Mat4,
    shadow_distance: f32,
    shadow_cascade_count: u32,
    packet: &mut FramePacket,
) {
    extract_frame_packet_into_with_shadow_cascades_and_render_height(
        world,
        display_aspect,
        surface_rotation,
        shadow_distance,
        shadow_cascade_count,
        0.0,
        packet,
    );
}

/// [`extract_frame_packet_into_with_shadow_cascades`] plus the scene's render
/// target height in pixels (#978).
///
/// 2D lines thinner than 1 render pixel are widened so they stay continuous;
/// the host passes the actual scene extent height (`RenderSettings::render_height`),
/// or `0.0` to keep pure world-space geometry.
pub fn extract_frame_packet_into_with_shadow_cascades_and_render_height(
    world: &mut World,
    display_aspect: f32,
    surface_rotation: &Mat4,
    shadow_distance: f32,
    shadow_cascade_count: u32,
    render_height: f32,
    packet: &mut FramePacket,
) {
    collect_scene_changes_into(
        world,
        display_aspect,
        surface_rotation,
        shadow_distance,
        shadow_cascade_count,
        &mut packet.scene,
    );

    let scene_view = scene::SceneReadView::new(world);
    let camera_frustums: Vec<prism_render::culling::Frustum> = packet
        .scene
        .cameras
        .iter()
        .filter_map(|camera| prism_render::culling::Frustum::from_view_proj(&camera.view_proj))
        .collect();
    scene_view.draw_items_with_frustums_into(&camera_frustums, &mut packet.draw_items);
    scene::systems::skinned_draw::append_skinned_draw_items_with_frustums(
        world,
        &camera_frustums,
        &mut packet.draw_items,
        &mut packet.bone_matrices,
    );
    prism_render::sort_draw_items(&mut packet.draw_items, packet.scene.eye.to_array());
    let light_view_projs = if packet.scene.shadow_cascade_count > 1 {
        [
            packet.scene.light_view_proj.to_cols_array_2d(),
            packet.scene.light_view_proj1.to_cols_array_2d(),
        ]
    } else {
        [
            packet.scene.light_view_proj.to_cols_array_2d(),
            packet.scene.light_view_proj.to_cols_array_2d(),
        ]
    };
    scene::systems::render::cull_shadow_casters_multi(
        &mut packet.draw_items,
        &light_view_projs[..packet.scene.shadow_cascade_count.clamp(1, 2) as usize],
    );
    scene_view.gizmo_instances_into(&mut packet.gizmos);
    packet.skybox = if packet.scene.clear_flags.draws_skybox() {
        scene_view.skybox_settings()
    } else {
        None
    };
    let primary_frustum = if camera_frustums.len() == 1 {
        camera_frustums.first()
    } else {
        None
    };
    let line_scale =
        crate::scene::systems::render::LineScreenScale::new(packet.scene.view_proj, render_height);
    scene_view.sprite_items_with_frustum_and_line_scale_into(
        primary_frustum,
        line_scale,
        &mut packet.sprite_items,
    );
    scene_view.sprite_lights_into(&mut packet.sprite_lights);
    scene_view.particle_items_into(packet.scene.eye, &mut packet.particle_items);

    crate::ui::convert_ui_draw_list_to_overlay_into(world, &mut packet.ui_overlay);

    packet.voxel_scene = extract_voxel_scene(world);

    packet.settings = RenderSettings::default();
}

struct VoxelExtractCache {
    started_at: Instant,
    chunks: HashMap<u64, (u64, Arc<prism_render::VoxelChunkUpload>)>,
    atlas: Option<(std::path::PathBuf, Option<Arc<Vec<u32>>>)>,
}

impl Default for VoxelExtractCache {
    fn default() -> Self {
        Self {
            started_at: Instant::now(),
            chunks: HashMap::new(),
            atlas: None,
        }
    }
}

fn encode_voxel_chunk_upload(
    entity_id: u64,
    chunk: &prism_scene::VoxelChunk,
) -> prism_render::VoxelChunkUpload {
    let l1_count = chunk.l1_cell_count();
    let mut l1_cells = Vec::with_capacity(l1_count);
    for idx in 0..l1_count {
        let r = chunk.l1_brick_ref.get(idx).copied().unwrap_or(0);
        let rgba = chunk.l1_uniform_rgba.get(idx).copied().unwrap_or(0);
        let vis = chunk.l1_uniform_visual.get(idx).copied().unwrap_or(0);
        l1_cells.push([r, rgba, vis, 0]);
    }
    let mut brick_occ = Vec::with_capacity(chunk.bricks.len());
    let mut brick_cells = Vec::with_capacity(chunk.bricks.len() * prism_scene::VOXEL_BRICK_CELLS);
    for brick in &chunk.bricks {
        brick_occ.push([brick.occ_lo, brick.occ_hi]);
        for sub in 0..prism_scene::VOXEL_BRICK_CELLS {
            let rgba = brick.l2_rgba.get(sub).copied().unwrap_or(0);
            let vis = brick.l2_visual.get(sub).copied().unwrap_or(0);
            let lo = brick.l3_mask_lo.get(sub).copied().unwrap_or(0);
            let hi = brick.l3_mask_hi.get(sub).copied().unwrap_or(0);
            brick_cells.push([rgba, vis, lo, hi]);
        }
    }
    prism_render::VoxelChunkUpload {
        entity_id,
        version: chunk.version,
        origin_l1: chunk.origin_l1,
        dims_l1: chunk.dims_l1,
        l1_cells,
        brick_occ,
        brick_cells,
    }
}

fn extract_voxel_scene(world: &mut World) -> Option<Arc<prism_render::VoxelSceneUpload>> {
    let mut chunk_entries: Vec<(u64, u64)> = world
        .query::<prism_scene::VoxelChunk>()
        .map(|(entity, chunk)| {
            let key = ((entity.generation() as u64) << 32) | (entity.id() as u64);
            (key, chunk.version)
        })
        .collect();
    if chunk_entries.is_empty() {
        return None;
    }
    chunk_entries.sort_by_key(|&(id, _)| id);

    let mut cursor: Option<prism_render::VoxelCursorUpload> = None;
    for (_, hl) in world.query::<prism_scene::VoxelCursorHighlight>() {
        if hl.active {
            cursor = Some(prism_render::VoxelCursorUpload {
                active: true,
                min: hl.min,
                max: hl.max,
                color: hl.color,
            });
            break;
        }
    }

    if world.get_resource::<VoxelExtractCache>().is_none() {
        world.insert_resource(VoxelExtractCache::default());
    }

    let mut dirty_updates: Vec<(u64, u64, Arc<prism_render::VoxelChunkUpload>)> = Vec::new();
    {
        let cache = world.get_resource::<VoxelExtractCache>().unwrap();
        let mut stale_or_missing = Vec::new();
        for &(entity_key, version) in &chunk_entries {
            if cache
                .chunks
                .get(&entity_key)
                .is_none_or(|(cached_ver, _)| *cached_ver != version)
            {
                stale_or_missing.push(entity_key);
            }
        }
        if !stale_or_missing.is_empty() {
            for (entity, chunk) in world.query::<prism_scene::VoxelChunk>() {
                let key = ((entity.generation() as u64) << 32) | (entity.id() as u64);
                if stale_or_missing.binary_search(&key).is_ok() {
                    dirty_updates.push((
                        key,
                        chunk.version,
                        Arc::new(encode_voxel_chunk_upload(key, chunk)),
                    ));
                }
            }
        }
    }

    let cache = world.get_resource_mut::<VoxelExtractCache>().unwrap();
    cache.chunks.retain(|&key, _| {
        chunk_entries
            .binary_search_by_key(&key, |&(k, _)| k)
            .is_ok()
    });
    for (key, version, upload) in dirty_updates {
        cache.chunks.insert(key, (version, upload));
    }

    let mut signature = 0xcbf2_9ce4_8422_2325u64;
    let mut uploads = Vec::with_capacity(chunk_entries.len());
    for &(entity_key, version) in &chunk_entries {
        signature ^= entity_key;
        signature = signature.wrapping_mul(0x1000_0000_01b3);
        signature ^= version;
        signature = signature.wrapping_mul(0x1000_0000_01b3);
        if let Some((_, cached)) = cache.chunks.get(&entity_key) {
            uploads.push(Arc::clone(cached));
        }
    }
    let elapsed_seconds = cache.started_at.elapsed().as_secs_f32();
    let atlas = if crate::asset_paths::strict_pak_only() {
        None
    } else {
        match crate::asset_paths::asset_dir() {
            Some(dir) => match &cache.atlas {
                Some((cached_dir, cached_atlas)) if cached_dir == &dir => cached_atlas.clone(),
                _ => {
                    let loaded = prism_render::voxel_dda_pass::load_spbr_atlas_from_assets(&dir);
                    cache.atlas = Some((dir, loaded.clone()));
                    loaded
                }
            },
            None => None,
        }
    };
    Some(Arc::new(prism_render::VoxelSceneUpload {
        signature,
        elapsed_seconds,
        chunks: uploads,
        cursor,
        atlas,
    }))
}

/// Per-tick extract result: camera/light snapshot + draw items + UI overlay.
/// Produced by [`extract_frame_packet`], consumed by [`render_system`].
///
/// `Default` exists so a caller can hold one packet and refill it with
/// [`extract_frame_packet_into`] (see #66).
#[derive(Default)]
pub struct FramePacket {
    pub scene: SceneChanges,
    pub draw_items: Vec<DrawItem>,
    /// ECS `Gizmo` entities, already transformed into world space.
    pub gizmos: Vec<GizmoInstance>,
    /// ECS `Skybox` parameters for this frame (`None` = do not draw).
    pub skybox: Option<SkyboxSettings>,
    /// ECS `SpriteRenderer` entities resolved into flat 2D draw records
    /// (already sorted back-to-front).
    pub sprite_items: Vec<prism_render::SpriteDrawItem>,
    /// `Light2D` entities for the 2D sprite pass (#611).
    pub sprite_lights: Vec<prism_render::SpriteLight>,
    /// `ParticleSystem` emitters resolved into world-space billboards
    /// (already sorted back-to-front).
    pub particle_items: Vec<prism_render::ParticleDrawItem>,
    /// This frame's bone palettes (column-major mat4), skinned draw item's `bone_offset` points here
    /// (#560 P0-3b).
    pub bone_matrices: Vec<[[f32; 4]; 4]>,
    pub ui_overlay: UiOverlayInput,
    /// App-layer render settings (debug flags / tonemap / PT params). Written each frame by main thread,
    /// read-only by render thread — ensures `App::render_settings` changes (shortcut keys, project config)
    /// actually reach render thread.
    pub settings: RenderSettings,
    /// Three-tier sparse voxel payload extracted for [`prism_render::VoxelDdaPass`].
    pub voxel_scene: Option<Arc<prism_render::VoxelSceneUpload>>,
    /// This frame's already parsed post-processing params (#502 Phase 2 volume blend).
    ///
    /// `None` = no host parsing, `render_system` falls back to `settings.post_grade()`;
    /// host calls after finishing writing `settings`
    /// [`post_process::resolve_post_grade`](crate::scene::systems::post_process::resolve_post_grade)
    /// and fills this, enabling `PostProcessVolume`.
    pub grade: Option<prism_render::PostGrade>,
}

/// Read the ECS [`World`] and the orientation parameters, then produce
/// a [`SceneChanges`] snapshot for the current frame.
/// Capture one frame's scene snapshot (convenience wrapper without reuse, for tests).
#[cfg(test)]
fn collect_scene_changes(
    world: &mut World,
    display_aspect: f32,
    surface_rotation: &Mat4,
) -> SceneChanges {
    let mut out = SceneChanges::default();
    collect_scene_changes_into(
        world,
        display_aspect,
        surface_rotation,
        DEFAULT_SHADOW_DISTANCE,
        1,
        &mut out,
    );
    out
}

/// Capture into caller-held snapshot.
///
/// `out` is the previous frame's snapshot (the recycled frame packet), so the
/// light buffers keep their allocation instead of being re-collected (#66).
fn collect_scene_changes_into(
    world: &mut World,
    display_aspect: f32,
    surface_rotation: &Mat4,
    shadow_distance: f32,
    shadow_cascade_count: u32,
    out: &mut SceneChanges,
) {
    let shadow_cascade_count = shadow_cascade_count.clamp(1, 2);
    let fallback_dir = Vec4::new(
        -std::f32::consts::FRAC_1_SQRT_2,
        std::f32::consts::FRAC_1_SQRT_2,
        0.0,
        0.0,
    );
    let fallback_col = Vec4::new(1.0, 1.0, 1.0, 0.0);

    let (view_proj, eye, view, projection, exposure, has_camera, camera_entity) = {
        if let Some(cam_entity) = scene::systems::camera::active_camera(world) {
            if let Some(cam) = world.get_mut::<Camera>(cam_entity) {
                cam.aspect = display_aspect;
            }
        }
        match scene::systems::camera::compute_camera_output(world, surface_rotation) {
            Some(out) => (
                out.view_proj,
                out.eye,
                out.view,
                out.projection,
                out.exposure,
                true,
                Some(out.entity),
            ),
            None => {
                log::warn!("no usable Camera entity — using fallback");
                let fb = scene::systems::camera::fallback_camera_output(
                    surface_rotation,
                    display_aspect,
                );
                (
                    fb.view_proj,
                    fb.eye,
                    fb.view,
                    fb.projection,
                    fb.exposure,
                    false,
                    None,
                )
            }
        }
    };

    let inv_projection = projection.inverse();
    let proj22 = projection.col(2).z;
    let proj32 = projection.col(3).z;
    let _ = projection;

    let camera_entities = scene::systems::camera::all_cameras(world);
    for entity in &camera_entities {
        if world
            .get::<crate::scene::components::LocalToWorld>(*entity)
            .is_some()
        {
            if let Some(cam) = world.get_mut::<Camera>(*entity) {
                cam.aspect = display_aspect;
            }
        }
    }
    let scene_view = scene::SceneReadView::new(world);
    out.cameras = scene_view.cameras(surface_rotation, display_aspect);

    let camera_orthographic = camera_entity
        .and_then(|entity| world.get::<Camera>(entity))
        .map(|cam| cam.orthographic)
        .unwrap_or(false);
    let samples = scene_view.light_samples();
    let dir_light = samples
        .iter()
        .find(|sample| sample.kind == scene::systems::lights::LightKind::Directional)
        .copied();

    let light_direction = dir_light
        .map(|l| directional_light_ubo_direction(l.direction, l.intensity * LUX_TO_RADIANCE_SCALE))
        .unwrap_or(fallback_dir);
    let light_color = dir_light
        .map(|l| Vec4::new(l.color.x, l.color.y, l.color.z, l.ambient))
        .unwrap_or(fallback_col);

    let (shadows_enabled, shadow_strength) =
        scene::systems::lights::directional_shadow_state(&samples);

    let mut locals: Vec<(usize, f32)> = samples
        .iter()
        .enumerate()
        .filter(|(_, sample)| sample.kind != scene::systems::lights::LightKind::Directional)
        .map(|(index, sample)| {
            (
                index,
                scene::systems::lights::local_light_importance(sample, eye),
            )
        })
        .collect();
    out.local_lights_total = locals.len();
    locals.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    locals.truncate(prism_render::LIGHT_MAX as usize);
    locals.sort_unstable_by_key(|(index, _)| *index);
    out.local_lights_kept = locals.len();
    out.dropped_light_ids.clear();

    out.lights.clear();
    for (index, _) in &locals {
        let sample = &samples[*index];
        let radiance = sample.color * sample.intensity * LUX_TO_RADIANCE_SCALE;
        let is_spot = sample.kind == scene::systems::lights::LightKind::Spot;
        out.lights.push(GpuLight {
            position: [
                sample.position.x,
                sample.position.y,
                sample.position.z,
                sample.range,
            ],
            color: [
                radiance.x,
                radiance.y,
                radiance.z,
                if is_spot {
                    GPU_LIGHT_KIND_SPOT
                } else {
                    GPU_LIGHT_KIND_POINT
                },
            ],
            direction: [
                sample.direction.x,
                sample.direction.y,
                sample.direction.z,
                sample.cos_outer,
            ],
            params: [sample.cos_inner, 0.0, 0.0, 0.0],
        });
    }

    let dropped = out.local_lights_total - out.local_lights_kept;
    if dropped > 0 {
        let kept: std::collections::HashSet<usize> =
            locals.iter().map(|(index, _)| *index).collect();
        for (index, sample) in samples.iter().enumerate() {
            if kept.contains(&index)
                || sample.kind == scene::systems::lights::LightKind::Directional
            {
                continue;
            }
            if let Some(id) =
                world.get::<crate::scene::runtime_components::SceneEntityId>(sample.entity)
            {
                out.dropped_light_ids.push(id.0);
            }
        }
        out.dropped_light_ids.sort_unstable();
        out.dropped_light_ids.dedup();
    }
    warn_dropped_lights(out.local_lights_total, dropped);

    out.pt_lights.clear();
    for sample in samples
        .iter()
        .filter(|sample| sample.kind == scene::systems::lights::LightKind::Point)
    {
        if out.pt_lights.len() >= PT_LIGHT_MAX as usize {
            break;
        }
        let radiance = sample.color * sample.intensity * LUX_TO_RADIANCE_SCALE;
        out.pt_lights.push(PtAnalyticLight::point(
            sample.position.into(),
            radiance.into(),
            sample.range,
        ));
    }

    let camera_near_plane = if camera_orthographic {
        0.001
    } else {
        let near = proj32 / proj22;
        if near.is_finite() && near > 0.0 {
            near
        } else {
            0.001
        }
    };
    let inv_view_proj = (projection * view).inverse();
    let (light_view_proj, light_view_proj1, shadow_split) = shadow_cascade_matrices(
        &light_direction,
        &inv_view_proj,
        &view,
        proj22,
        proj32,
        camera_orthographic,
        camera_near_plane,
        shadow_distance,
        shadow_cascade_count,
    );

    let clear_color = camera_entity
        .map(|entity| scene::systems::camera::clear_color(world, entity))
        .unwrap_or(scene::systems::camera::DEFAULT_CLEAR_COLOR);
    let clear_flags = camera_entity
        .map(|entity| scene::systems::camera::clear_flags(world, entity))
        .unwrap_or_default();

    out.view_proj = view_proj;
    out.eye = eye;
    out.view = view;
    out.projection = projection;
    out.inv_projection = inv_projection;
    out.proj22 = proj22;
    out.proj32 = proj32;
    out.light_direction = light_direction;
    out.light_color = light_color;
    out.light_view_proj = light_view_proj;
    out.light_view_proj1 = light_view_proj1;
    out.shadow_split = shadow_split;
    out.shadow_cascade_count = shadow_cascade_count;
    out.exposure = exposure;
    out.has_camera = has_camera;
    out.camera_orthographic = camera_orthographic;
    out.clear_color = clear_color;
    out.clear_flags = clear_flags;
    out.shadows_enabled = shadows_enabled;
    out.shadow_strength = shadow_strength;
}

/// Consume a pre-extracted [`FramePacket`] and drive the GPU.
///
/// **Each call is stateless with respect to the ECS world.**  The simulation
/// thread owns [`World`](prism_ecs::World) and produces the packet; this
/// Function only reads packet and renderer
///
/// Returns `Err` only when [`GraphRenderer::render`] fails.
pub fn render_system(
    renderer: &mut GraphRenderer,
    packet: &FramePacket,
    dirty_router: &mut DirtyRouter,
) -> anyhow::Result<()> {
    let settings = &packet.settings;
    let FramePacket {
        ref scene,
        ref draw_items,
        ref gizmos,
        skybox,
        ref sprite_items,
        ref sprite_lights,
        ref particle_items,
        ref ui_overlay,
        ref bone_matrices,
        settings: _,
        ref voxel_scene,
        grade,
    } = *packet;

    let dirty_flags = dirty_router.update(scene);
    if dirty_flags.any() {
        log::trace!(
            "dirty_flags: camera={} dir_light={} point_lights={}",
            dirty_flags.camera,
            dirty_flags.directional_light,
            dirty_flags.point_lights,
        );
    }
    let SceneChanges {
        view_proj,
        eye,
        view,
        projection: _,
        inv_projection,
        proj22,
        proj32,
        light_direction,
        light_color,
        light_view_proj,
        ref lights,
        ref pt_lights,
        exposure,
        has_camera,
        camera_orthographic,
        clear_color: _,
        clear_flags: _,
        shadows_enabled,
        shadow_strength,
        ..
    } = scene;
    let light_count = lights.len() as f32;

    let frame_data = FrameUBOData {
        view_proj: view_proj.to_cols_array_2d(),
        camera_position: [eye.x, eye.y, eye.z, light_count],
        light_direction: light_direction.to_array(),
        light_color: light_color.to_array(),
        view: view.to_cols_array_2d(),
        light_view_proj: light_view_proj.to_cols_array_2d(),
        light_view_proj1: scene.light_view_proj1.to_cols_array_2d(),
        shadow_split: scene.shadow_split,
        shadow_cascade_count: scene.shadow_cascade_count,
        tonemap_mode: settings.tonemap_mode,
        viewport_w: renderer.extent().width as f32,
        viewport_h: renderer.extent().height as f32,
        exposure: *exposure,
        shadow_strength: *shadow_strength,
        prev_view_proj: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ],
        ..Default::default()
    };

    let draw_items_dirty = dirty_router.draw_items_dirty(draw_items_signature(draw_items));
    if draw_items_dirty {
        log::trace!("dirty: draw_items changed");
    }
    let pt_bounces_dirty = dirty_router.pt_bounces_dirty(settings.pt_max_bounces);
    let dispatch_plan =
        dirty_router.plan_dispatch(scene, draw_items_dirty, pt_bounces_dirty, 0, 0, 0);

    renderer.set_occlusion_culling(settings.occlusion_culling);
    let ctx = match renderer.begin_frame()? {
        Some(c) => c,
        None => return Ok(()),
    };
    let mut occlusion_items = Vec::new();
    let main_pass_items: &[prism_render::render_graph::DrawItem] = if settings.occlusion_culling {
        occlusion_items.extend_from_slice(draw_items);
        renderer.cull_occluded_draws(&mut occlusion_items);
        &occlusion_items
    } else {
        draw_items
    };
    let mut resolved_grade = grade.unwrap_or_else(|| settings.post_grade());
    resolved_grade.lut_strength = settings.lut_strength;
    let lut_path = if settings.lut.trim().is_empty() || crate::asset_paths::strict_pak_only() {
        None
    } else {
        crate::asset_paths::asset_path(settings.lut.trim())
    };
    let input = prism_render::FrameInput {
        draw_items: main_pass_items,
        gizmos,
        sprite_items,
        sprite_lights,
        particle_items,
        bone_matrices,
        skybox,
        frame_data: &frame_data,
        cameras: &scene.cameras,
        light_view_proj: light_view_proj.to_cols_array_2d(),
        light_view_proj1: frame_data.light_view_proj1,
        shadow_split: frame_data.shadow_split,
        shadow_cascade_count: frame_data.shadow_cascade_count,
        inv_projection: inv_projection.to_cols_array_2d(),
        debug_flags: settings.debug_flags,
        wireframe: settings.wireframe,
        shaded_wireframe: settings.shaded_wireframe,
        tonemap_mode: settings.tonemap_mode,
        aa_mode: settings.aa_mode,
        debug_rt: settings.debug_rt,
        grade: resolved_grade,
        lut_path: lut_path.as_deref(),
        proj22: *proj22,
        proj32: *proj32,
        lights,
        render_mode: settings.render_mode,
        pt_max_bounces: settings.pt_max_bounces,
        pt_ray_max_distance: settings.pt_ray_max_distance,
        pt_max_iterations: settings.pt_max_iterations,
        exposure: *exposure,
        pt_lights,
        pt_accum_dirty: dirty_flags.directional_light
            || dirty_flags.point_lights
            || draw_items_dirty
            || pt_bounces_dirty,
        has_camera: *has_camera,
        camera_orthographic: *camera_orthographic,
        clear_color: scene.clear_color,
        shadows_enabled: *shadows_enabled,
        ui_overlay: Some(ui_overlay),
        show_grid: settings.show_grid,
        dispatch_plan: Some(&dispatch_plan),
    };
    renderer.set_voxel_scene(voxel_scene.clone());
    renderer.prepare(&ctx, &input).inspect_err(|_e| {
        let _ = renderer.present(&ctx);
    })?;
    renderer.execute(&ctx, &input).inspect_err(|_e| {
        let _ = renderer.present(&ctx);
    })?;
    let _ = renderer.present(&ctx)?;
    Ok(())
}
