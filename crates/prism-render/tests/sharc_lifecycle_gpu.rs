#![cfg(feature = "sharc")]
//! SHaRC world cache lifecycle rules (fix spec §8).
//!
//! Cache has two lifetimes: screen history (accumulation, motion vector) follows camera and extent changes; world cache
//! stores radiance by world position, invalidated only when radiance field itself changes. This test uses
//! [`GraphRenderer::sharc_clear_count`] to observe whether clear truly happens:
//!
//! - resize only drops screen history, world cache kept;
//! - light change, geometry / scene switch, skybox change each invalidate cache;
//! - when cache idle (raster mode), recorded invalidate reason kept across frames, consumed when cache used again;
//! - `instances_dirty` does not count as invalidate: draw list rebuilt each frame, signature changes when camera moves.
//!
//! Skipped when no Vulkan device or no ray query, following existing GPU test convention.

use prism_render::render_graph::{DrawItem, DrawState, PostGrade, PtGiMode, RenderMode};
use prism_render::{DirtyDispatchPlan, FrameInput, FrameUBOData, GraphRenderer};

const SIZE: usize = 128;

fn try_create() -> Option<GraphRenderer> {
    match GraphRenderer::offscreen_new(SIZE as u32, SIZE as u32, None) {
        Ok(mut renderer) => {
            if !renderer.context().rt_caps.has_ray_query() {
                eprintln!("[SKIP] no VK_KHR_ray_query support");
                return None;
            }
            renderer.settings_mut().pt_gi = PtGiMode::Sharc;
            renderer.settings_mut().sharc_capacity = 1 << 14;
            renderer.settings_mut().sharc_scene_scale = 50.0;
            Some(renderer)
        }
        Err(error) => {
            let message = format!("{error:#}");
            if message.contains("VK_ERROR_INCOMPATIBLE_DRIVER")
                || message.contains("Unable to find a Vulkan driver")
                || message.contains("VK_ERROR_EXTENSION_NOT_PRESENT")
                || message.contains("no devices")
                || message.contains("No valid")
                || message.contains("PhysicalDevice")
            {
                eprintln!("[SKIP] no Vulkan driver: {error:#}");
                return None;
            }
            panic!("offscreen_new failed: {error:#}");
        }
    }
}

/// Vulkan perspective matrix at origin looking −Z (near 0.1 / far 100).
fn perspective() -> [[f32; 4]; 4] {
    let fov = 45f32.to_radians();
    let (near, far) = (0.1f32, 100.0f32);
    let f = 1.0 / (fov * 0.5).tan();
    let mut m = [[0.0f32; 4]; 4];
    m[0][0] = f;
    m[1][1] = -f;
    m[2][2] = far / (near - far);
    m[2][3] = -1.0;
    m[3][2] = (near * far) / (near - far);
    m
}

fn translation_scale(x: f32, y: f32, z: f32, scale: f32) -> [[f32; 4]; 4] {
    let mut m = [[0.0f32; 4]; 4];
    m[0][0] = scale;
    m[1][1] = scale;
    m[2][2] = scale;
    m[3] = [x, y, z, 1.0];
    m
}

fn draw_item(
    mesh: prism_render::managers::MeshHandle,
    model: [[f32; 4]; 4],
    entity_id: u32,
) -> DrawItem {
    DrawItem {
        mesh,
        entity_id,
        model,
        material: None,
        cast_shadow: false,
        visible_in_main_pass: true,
        cast_shadow_two_sided: false,
        state: DrawState::default(),
        world_bounds: None,
        index_start: 0,
        index_count: None,
        bone_offset: None,
        splat_slots: [u32::MAX; 4],
        splat_tiling: 0.0,
        shader: None,
    }
}

fn frame_data(view_proj: [[f32; 4]; 4], extent: (usize, usize)) -> FrameUBOData {
    FrameUBOData {
        view_proj,
        camera_position: [0.0, 0.0, 0.0, 0.0],
        light_direction: [0.3, 0.8, 0.5, 0.8],
        light_color: [1.0, 1.0, 1.0, 1.0],
        view: view_proj,
        light_view_proj: view_proj,
        light_view_proj1: view_proj,
        shadow_split: 0.0,
        shadow_cascade_count: 1,
        tonemap_mode: 2,
        viewport_w: extent.0 as f32,
        viewport_h: extent.1 as f32,
        exposure: 1.0,
        shadow_strength: 0.0,
        prev_view_proj: view_proj,
        ..Default::default()
    }
}

/// Run through five phases and read back one frame. Empty `plan` means "no dirty data this frame".
fn render_frame(
    renderer: &mut GraphRenderer,
    items: &[DrawItem],
    mode: RenderMode,
    plan: Option<&DirtyDispatchPlan>,
    extent: (usize, usize),
) {
    let view_proj = perspective();
    let ctx = renderer.begin_frame().expect("begin_frame").expect("frame");
    let data = frame_data(view_proj, extent);
    let input = FrameInput {
        draw_items: items,
        gizmos: &[],
        sprite_items: &[],
        sprite_lights: &[],
        particle_items: &[],
        bone_matrices: &[],
        skybox: None,
        frame_data: &data,
        cameras: &[],
        light_view_proj: view_proj,
        light_view_proj1: view_proj,
        shadow_split: 0.0,
        shadow_cascade_count: 1,
        inv_projection: view_proj,
        debug_flags: 0,
        wireframe: false,
        shaded_wireframe: false,
        tonemap_mode: 2,
        aa_mode: 0,
        debug_rt: 0,
        grade: PostGrade::default(),
        lut_path: None,
        proj22: view_proj[2][2],
        proj32: view_proj[3][2],
        lights: &[],
        render_mode: mode,
        pt_max_bounces: 3,
        pt_ray_max_distance: 100.0,
        pt_max_iterations: 0,
        exposure: 1.0,
        pt_lights: &[],
        pt_accum_dirty: true,
        has_camera: true,
        camera_orthographic: false,
        clear_color: [0.1, 0.1, 0.1, 1.0],
        shadows_enabled: false,
        ui_overlay: None,
        show_grid: false,
        dispatch_plan: plan,
    };
    renderer.prepare(&ctx, &input).expect("prepare");
    renderer.execute(&ctx, &input).expect("execute");
    let out_of_date = renderer.present(&ctx).expect("present");
    renderer.end_frame(&ctx, out_of_date).expect("end_frame");
    renderer
        .copy_offscreen_to_readback()
        .expect("copy_offscreen_to_readback");
    renderer.readback_pixels().expect("readback_pixels");
}

fn box_items(renderer: &mut GraphRenderer) -> Vec<DrawItem> {
    let mesh = renderer
        .register_mesh(&prism_render::primitives::builtin("box").expect("builtin box"))
        .expect("register box");
    vec![
        draw_item(mesh, translation_scale(0.0, 0.0, -4.0, 2.0), 1),
        draw_item(mesh, translation_scale(0.0, -1.5, -6.0, 2.0), 2),
    ]
}

#[test]
fn resize_reprojects_the_world_cache_instead_of_clearing_it() {
    let Some(mut renderer) = try_create() else {
        return;
    };
    let items = box_items(&mut renderer);

    render_frame(
        &mut renderer,
        &items,
        RenderMode::PathTrace,
        None,
        (SIZE, SIZE),
    );
    let after_init = renderer.sharc_clear_count();
    assert_eq!(after_init, 1, "cache allocation clears exactly once");

    render_frame(
        &mut renderer,
        &items,
        RenderMode::PathTrace,
        None,
        (SIZE, SIZE),
    );
    assert_eq!(
        renderer.sharc_clear_count(),
        after_init,
        "a steady frame must not touch the world cache"
    );

    renderer
        .resize_offscreen(192, 160)
        .expect("resize_offscreen");
    render_frame(
        &mut renderer,
        &items,
        RenderMode::PathTrace,
        None,
        (192, 160),
    );
    assert_eq!(
        renderer.sharc_clear_count(),
        after_init,
        "resize must keep the world-space cache"
    );
}

#[test]
fn light_change_clears_the_cache() {
    let Some(mut renderer) = try_create() else {
        return;
    };
    let items = box_items(&mut renderer);
    render_frame(
        &mut renderer,
        &items,
        RenderMode::PathTrace,
        None,
        (SIZE, SIZE),
    );
    let after_init = renderer.sharc_clear_count();

    let instances_only = DirtyDispatchPlan {
        instances_dirty: true,
        emissive_table_dirty: true,
        ..Default::default()
    };
    render_frame(
        &mut renderer,
        &items,
        RenderMode::PathTrace,
        Some(&instances_only),
        (SIZE, SIZE),
    );
    assert_eq!(
        renderer.sharc_clear_count(),
        after_init,
        "instances_dirty alone must not clear the world cache"
    );

    let lights = DirtyDispatchPlan {
        lights_dirty: true,
        pt_accum_dirty: true,
        ..Default::default()
    };
    render_frame(
        &mut renderer,
        &items,
        RenderMode::PathTrace,
        Some(&lights),
        (SIZE, SIZE),
    );
    assert_eq!(
        renderer.sharc_clear_count(),
        after_init + 1,
        "a lighting change must clear the world cache"
    );

    render_frame(
        &mut renderer,
        &items,
        RenderMode::PathTrace,
        None,
        (SIZE, SIZE),
    );
    assert_eq!(renderer.sharc_clear_count(), after_init + 1);
    assert!(!renderer.sharc_invalidation_pending());
}

#[test]
fn sky_change_clears_the_cache() {
    let Some(mut renderer) = try_create() else {
        return;
    };
    let items = box_items(&mut renderer);
    render_frame(
        &mut renderer,
        &items,
        RenderMode::PathTrace,
        None,
        (SIZE, SIZE),
    );
    let after_init = renderer.sharc_clear_count();

    renderer.set_environment(None).expect("set_environment");
    assert!(
        renderer.sharc_invalidation_pending(),
        "swapping the IBL must queue an invalidation"
    );
    render_frame(
        &mut renderer,
        &items,
        RenderMode::PathTrace,
        None,
        (SIZE, SIZE),
    );
    assert_eq!(
        renderer.sharc_clear_count(),
        after_init + 1,
        "an environment swap must clear the world cache"
    );
}

#[test]
fn invalidation_stays_pending_while_the_cache_is_idle() {
    let Some(mut renderer) = try_create() else {
        return;
    };
    let items = box_items(&mut renderer);
    render_frame(
        &mut renderer,
        &items,
        RenderMode::PathTrace,
        None,
        (SIZE, SIZE),
    );
    let after_init = renderer.sharc_clear_count();

    renderer.invalidate_pt_geometry();
    assert!(renderer.sharc_invalidation_pending());
    render_frame(
        &mut renderer,
        &items,
        RenderMode::Raster,
        None,
        (SIZE, SIZE),
    );
    assert_eq!(
        renderer.sharc_clear_count(),
        after_init,
        "a raster frame must not touch the cache"
    );
    assert!(
        renderer.sharc_invalidation_pending(),
        "the reason must survive an idle frame"
    );

    render_frame(
        &mut renderer,
        &items,
        RenderMode::PathTrace,
        None,
        (SIZE, SIZE),
    );
    assert_eq!(renderer.sharc_clear_count(), after_init + 1);
    assert!(!renderer.sharc_invalidation_pending());
}

#[test]
fn voxel_scale_change_clears_the_cache() {
    let Some(mut renderer) = try_create() else {
        return;
    };
    let items = box_items(&mut renderer);
    render_frame(
        &mut renderer,
        &items,
        RenderMode::PathTrace,
        None,
        (SIZE, SIZE),
    );
    let after_init = renderer.sharc_clear_count();

    render_frame(
        &mut renderer,
        &items,
        RenderMode::PathTrace,
        None,
        (SIZE, SIZE),
    );
    assert_eq!(renderer.sharc_clear_count(), after_init);

    renderer.settings_mut().sharc_scene_scale = 25.0;
    render_frame(
        &mut renderer,
        &items,
        RenderMode::PathTrace,
        None,
        (SIZE, SIZE),
    );
    assert_eq!(
        renderer.sharc_clear_count(),
        after_init + 1,
        "a voxel-format change must clear the cache"
    );

    render_frame(
        &mut renderer,
        &items,
        RenderMode::PathTrace,
        None,
        (SIZE, SIZE),
    );
    assert_eq!(renderer.sharc_clear_count(), after_init + 1);
}
