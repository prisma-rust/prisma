#![cfg(feature = "prt")]
//! End-to-end GPU test for `PrtPass` (`RasterGiMode::Prt`) on a real Vulkan device.
//!
//! Skips gracefully when no Vulkan device is present.

use prism_render::prt_volume::{PrtNode, PrtVolume, NO_CHILD, NO_LEAF, SH_COEFF_COUNT};
use prism_render::render_graph::{PostGrade, RasterGiMode, RenderMode};
use prism_render::{FrameInput, FrameUBOData, GpuLight, GraphRenderer};

const SIZE: usize = 128;

fn try_create() -> Option<GraphRenderer> {
    match GraphRenderer::offscreen_new(SIZE as u32, SIZE as u32, None) {
        Ok(mut renderer) => {
            renderer.settings_mut().raster_gi = RasterGiMode::Prt;
            renderer.settings_mut().prt_leak_bias = 0.05;
            Some(renderer)
        }
        Err(error) => {
            let message = format!("{error:#}");
            if message.contains("VK_ERROR_INCOMPATIBLE_DRIVER")
                || message.contains("VK_ERROR_EXTENSION_NOT_PRESENT")
                || message.contains("no devices")
                || message.contains("No valid")
                || message.contains("PhysicalDevice")
            {
                eprintln!("[SKIP] no Vulkan driver: {error:#}");
                None
            } else {
                panic!("offscreen_new failed: {error:#}");
            }
        }
    }
}

fn sample_prt_volume() -> PrtVolume {
    let mut c0 = [[0.0f32; 3]; SH_COEFF_COUNT];
    c0[0] = [1.0, 0.8, 0.6];
    c0[2] = [0.4, 0.3, 0.2];
    PrtVolume {
        cluster_cells: 1,
        world_origin: [-8.0, -8.0, -8.0],
        world_extent: 16.0,
        leaf_grid: 1,
        built_from_sha: [0u8; 8],
        nodes: vec![
            PrtNode {
                first_child: 1,
                leaf_index: NO_LEAF,
                child_mask: 1,
            },
            PrtNode {
                first_child: NO_CHILD,
                leaf_index: 0,
                child_mask: 0,
            },
        ],
        clusters: vec![c0],
        cluster_valid: Some(vec![true]),
    }
}

#[test]
fn prt_pass_executes_with_dynamic_lights_on_gpu() {
    let Some(mut renderer) = try_create() else {
        return;
    };
    assert!(renderer.load_prt_volume(sample_prt_volume()));

    let mut view_proj = [[0.0f32; 4]; 4];
    for (i, row) in view_proj.iter_mut().enumerate() {
        row[i] = 1.0;
    }

    let ubo = FrameUBOData {
        viewProj: view_proj,
        cameraPosition: [0.0, 1.0, 4.0, 1.0],
        lightDirection: [0.0, 1.0, 0.0, 0.0],
        lightColor: [1.0, 0.95, 0.9, 0.2],
        view: view_proj,
        lightViewProj: view_proj,
        tonemapMode: 2,
        viewportW: SIZE as f32,
        viewportH: SIZE as f32,
        exposure: 1.0,
        shadowStrength: 1.0,
        lightViewProj1: view_proj,
        shadowSplit: 15.0,
        shadowCascadeCount: 1,
        prevViewProj: view_proj,
        ..Default::default()
    };

    let lights = [GpuLight {
        position: [0.0, 2.0, 0.0, 10.0],
        color: [1.0, 0.6, 0.3, 2.0],
        direction: [0.0, -1.0, 0.0, 0.0],
        params: [0.0; 4],
    }];

    // Run 3 frames so 1-frame latency gi_volume is populated and sampled by ForwardPass.
    for _ in 0..3 {
        let ctx = renderer.begin_frame().expect("begin_frame").expect("frame");
        let input = FrameInput {
            draw_items: &[],
            gizmos: &[],
            sprite_items: &[],
            sprite_lights: &[],
            particle_items: &[],
            bone_matrices: &[],
            skybox: None,
            frame_data: &ubo,
            cameras: &[],
            light_view_proj: view_proj,
            light_view_proj1: view_proj,
            shadow_split: 15.0,
            shadow_cascade_count: 1,
            inv_projection: view_proj,
            debug_flags: 0xFFFF,
            wireframe: false,
            shaded_wireframe: false,
            tonemap_mode: 2,
            aa_mode: 0,
            debug_rt: 0,
            grade: PostGrade::default(),
            lut_path: None,
            proj22: view_proj[2][2],
            proj32: view_proj[3][2],
            lights: &lights,
            render_mode: RenderMode::Raster,
            pt_max_bounces: 3,
            pt_ray_max_distance: 100.0,
            pt_max_iterations: 0,
            exposure: 1.0,
            pt_lights: &[],
            pt_accum_dirty: false,
            has_camera: true,
            camera_orthographic: false,
            clear_color: [0.1, 0.1, 0.1, 1.0],
            shadows_enabled: false,
            ui_overlay: None,
            show_grid: false,
            dispatch_plan: None,
        };
        renderer.prepare(&ctx, &input).expect("prepare");
        renderer.execute(&ctx, &input).expect("execute");
        let out_of_date = renderer.present(&ctx).expect("present");
        renderer.end_frame(&ctx, out_of_date).expect("end_frame");
    }
}

#[test]
fn prt_16_dynamic_lights_and_overflow_abort_on_gpu() {
    let Some(mut renderer) = try_create() else {
        return;
    };
    assert!(renderer.load_prt_volume(sample_prt_volume()));

    let mut view_proj = [[0.0f32; 4]; 4];
    for (i, row) in view_proj.iter_mut().enumerate() {
        row[i] = 1.0;
    }

    let ubo = FrameUBOData {
        viewProj: view_proj,
        cameraPosition: [0.0, 1.0, 4.0, 1.0],
        lightDirection: [0.0, 1.0, 0.0, 0.0],
        lightColor: [1.0, 0.95, 0.9, 0.2],
        view: view_proj,
        lightViewProj: view_proj,
        tonemapMode: 2,
        viewportW: SIZE as f32,
        viewportH: SIZE as f32,
        exposure: 1.0,
        shadowStrength: 1.0,
        lightViewProj1: view_proj,
        shadowSplit: 15.0,
        shadowCascadeCount: 1,
        prevViewProj: view_proj,
        ..Default::default()
    };

    let lights_16: Vec<GpuLight> = (0..16)
        .map(|i| GpuLight {
            position: [(i as f32) * 0.5 - 4.0, 2.0, 0.0, 12.0],
            color: [1.0, 0.7, 0.4, 1.5],
            direction: [0.0, -1.0, 0.0, 0.0],
            params: [0.0; 4],
        })
        .collect();

    let lights_65: Vec<GpuLight> = (0..65)
        .map(|i| GpuLight {
            position: [(i as f32) * 0.1, 2.0, 0.0, 12.0],
            color: [1.0, 0.7, 0.4, 1.0],
            direction: [0.0, -1.0, 0.0, 0.0],
            params: [0.0; 4],
        })
        .collect();

    for (frame_idx, lights) in [
        lights_16.as_slice(),
        lights_16.as_slice(),
        lights_65.as_slice(),
        lights_16.as_slice(),
    ]
    .into_iter()
    .enumerate()
    {
        let ctx = renderer.begin_frame().expect("begin_frame").expect("frame");
        let input = FrameInput {
            draw_items: &[],
            gizmos: &[],
            sprite_items: &[],
            sprite_lights: &[],
            particle_items: &[],
            bone_matrices: &[],
            skybox: None,
            frame_data: &ubo,
            cameras: &[],
            light_view_proj: view_proj,
            light_view_proj1: view_proj,
            shadow_split: 15.0,
            shadow_cascade_count: 1,
            inv_projection: view_proj,
            debug_flags: 0xFFFF,
            wireframe: false,
            shaded_wireframe: false,
            tonemap_mode: 2,
            aa_mode: 0,
            debug_rt: 0,
            grade: PostGrade::default(),
            lut_path: None,
            proj22: view_proj[2][2],
            proj32: view_proj[3][2],
            lights,
            render_mode: RenderMode::Raster,
            pt_max_bounces: 3,
            pt_ray_max_distance: 100.0,
            pt_max_iterations: 0,
            exposure: 1.0,
            pt_lights: &[],
            pt_accum_dirty: false,
            has_camera: true,
            camera_orthographic: false,
            clear_color: [0.1, 0.1, 0.1, 1.0],
            shadows_enabled: false,
            ui_overlay: None,
            show_grid: false,
            dispatch_plan: None,
        };
        renderer.prepare(&ctx, &input).expect("prepare");
        renderer.execute(&ctx, &input).expect("execute");
        let out_of_date = renderer.present(&ctx).expect("present");
        renderer.end_frame(&ctx, out_of_date).expect("end_frame");
        let _ = frame_idx;
    }
}
