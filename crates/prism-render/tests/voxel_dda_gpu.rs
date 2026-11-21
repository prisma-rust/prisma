//! Headless Vulkan integration test for three-tier GPU `VoxelDdaPass`.

use std::sync::Arc;

use prism_render::descriptor::FrameUBOData;
use prism_render::render_graph::PostGrade;
use prism_render::{
    GraphRenderer, RenderMode, VoxelChunkUpload, VoxelCursorUpload, VoxelSceneUpload,
};

fn try_create_headless() -> Option<GraphRenderer> {
    match GraphRenderer::headless_new(None) {
        Ok(r) => Some(r),
        Err(e) => {
            let msg = format!("{e:#}");
            if msg.contains("VK_ERROR_INCOMPATIBLE_DRIVER")
                || msg.contains("Unable to find a Vulkan driver")
                || msg.contains("VK_ERROR_EXTENSION_NOT_PRESENT")
                || msg.contains("no devices")
                || msg.contains("No valid")
                || msg.contains("PhysicalDevice")
            {
                eprintln!("[SKIP] no Vulkan driver: {e:#}");
                None
            } else {
                panic!("headless_new failed: {e:#}");
            }
        }
    }
}

/// Column-major Vulkan RH perspective look-at matrix (y-flipped, depth in [0, 1]).
fn make_camera_vp(eye: [f32; 3], target: [f32; 3], aspect: f32) -> [[f32; 4]; 4] {
    fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
    }
    fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    }
    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }
    fn norm(v: [f32; 3]) -> [f32; 3] {
        let l = dot(v, v).sqrt();
        [v[0] / l, v[1] / l, v[2] / l]
    }
    fn mul_mat(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
        let mut r = [[0.0; 4]; 4];
        for col in 0..4 {
            for row in 0..4 {
                r[col][row] = a[0][row] * b[col][0]
                    + a[1][row] * b[col][1]
                    + a[2][row] * b[col][2]
                    + a[3][row] * b[col][3];
            }
        }
        r
    }

    let f = norm(sub(target, eye));
    let s = norm(cross(f, [0.0, 1.0, 0.0]));
    let u = cross(s, f);
    let view = [
        [s[0], u[0], -f[0], 0.0],
        [s[1], u[1], -f[1], 0.0],
        [s[2], u[2], -f[2], 0.0],
        [-dot(s, eye), -dot(u, eye), dot(f, eye), 1.0],
    ];
    let fov_y = 60.0f32.to_radians();
    let inv_tan = 1.0 / (fov_y * 0.5).tan();
    let near = 0.05f32;
    let far = 100.0f32;
    let q = far / (near - far);
    let proj = [
        [inv_tan / aspect, 0.0, 0.0, 0.0],
        [0.0, -inv_tan, 0.0, 0.0],
        [0.0, 0.0, q, -1.0],
        [0.0, 0.0, q * near, 0.0],
    ];
    mul_mat(proj, view)
}

const VIS_BURNING: u32 = 1 << 1;

const fn pack_rgba(r: u8, g: u8, b: u8, a: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16) | ((a as u32) << 24)
}

const fn pack_visual(flags16: u32, roughness_u8: u8, emissive_u8: u8) -> u32 {
    (flags16 & 0xFFFF) | ((roughness_u8 as u32) << 16) | ((emissive_u8 as u32) << 24)
}

#[test]
fn voxel_dda_renders_l1_l2_and_carved_l3_window() {
    let mut renderer = match try_create_headless() {
        Some(r) => r,
        None => return,
    };

    // Build a 4x4x4 L1 chunk centered around (2, 2, 2):
    // - L1 (0, 1, 2): uniform COW orange slab (`0xFFFFFFFF`).
    // - L1 (2, 1, 2): split L2 brick containing one L2 wall at local (0, 2, 0) [x:2.0..2.25, y:1.5..1.75, z:2.0..2.25]
    //   with its center 2x2 L3 tunnel along Z carved out (`mask` omits center mx in 1..3, my in 1..3).
    let dims = [4u32, 4u32, 4u32];
    let count = (dims[0] * dims[1] * dims[2]) as usize;
    let mut l1_cells = vec![[0u32; 4]; count];

    let l1_idx = |x: u32, y: u32, z: u32| -> usize { (x + dims[0] * (y + dims[1] * z)) as usize };
    // L1 uniform orange/copper block at (1, 1, 2) -> x=[1..2], y=[1..2], z=[2..3].
    let orange_rgba = 220 | (95 << 8) | (35 << 16) | (255 << 24);
    let solid_vis = 180u32 << 16;
    l1_cells[l1_idx(1, 1, 2)] = [u32::MAX, orange_rgba, solid_vis, 0];

    // L1 split brick at (2, 1, 2) -> brick index 0 (`ref = 1`).
    l1_cells[l1_idx(2, 1, 2)] = [1, 0, 0, 0];
    let sub = 8; // local L2 (0, 2, 0) -> world [2.0..2.25, 1.5..1.75, 2.0..2.25]
    let occ_lo = if sub < 32 { 1u32 << sub } else { 0 };
    let occ_hi = if sub >= 32 { 1u32 << (sub - 32) } else { 0 };
    let mut brick_cells = vec![[0u32; 4]; 64];

    // Build 64-bit mask missing the center 2x2 tunnel along Z (mx in 1..3, my in 1..3).
    let mut mask = 0u64;
    for mz in 0..4u32 {
        for my in 0..4u32 {
            for mx in 0..4u32 {
                if !((1..3).contains(&mx) && (1..3).contains(&my)) {
                    let b = mx + 4 * (my + 4 * mz);
                    mask |= 1u64 << b;
                }
            }
        }
    }
    let cyan_alloy = 60 | (185 << 8) | (220 << 16) | (255 << 24);
    // Transient WET flag (`1 << 0`) + METALLIC (`1 << 12`).
    let metal_vis = (1u32 | (1u32 << 12)) | (70u32 << 16);
    brick_cells[sub] = [cyan_alloy, metal_vis, mask as u32, (mask >> 32) as u32];

    let scene = Arc::new(VoxelSceneUpload {
        signature: 1,
        elapsed_seconds: 0.25,
        chunks: vec![Arc::new(VoxelChunkUpload {
            entity_id: 1,
            version: 1,
            origin_l1: [0, 0, 0],
            dims_l1: dims,
            l1_cells,
            brick_occ: vec![[occ_lo, occ_hi]],
            brick_cells,
        })],
        cursor: Some(VoxelCursorUpload {
            active: true,
            min: [2.0, 1.5, 2.0],
            max: [2.25, 1.75, 2.25],
            color: [1.0, 0.9, 0.2, 1.0],
        }),
        atlas: None,
    });
    renderer.set_voxel_scene(Some(scene));

    let eye = [2.125, 1.625, 1.25];
    let target = [2.125, 1.625, 2.0];
    let vp = make_camera_vp(eye, target, 1.0);

    let frame_data = FrameUBOData {
        view_proj: vp,
        camera_position: [eye[0], eye[1], eye[2], 0.0],
        light_direction: [0.4, 0.8, -0.45, 2.5],
        light_color: [1.0, 0.98, 0.92, 0.4],
        viewport_w: 256.0,
        viewport_h: 256.0,
        exposure: 1.0,
        ..Default::default()
    };

    let out_of_date = renderer
        .render(
            &[],
            &frame_data,
            [[0.0; 4]; 4],
            [[0.0; 4]; 4],
            0,
            0,
            0,
            0,
            0.0,
            0.0,
            &[],
            RenderMode::Raster,
            0,
            0.0,
            0,
            1.0,
            &[],
            PostGrade::default(),
            true,
            false,
            &[],
        )
        .expect("voxel render failed");
    assert!(!out_of_date);

    renderer
        .copy_offscreen_to_readback()
        .expect("copy_offscreen_to_readback failed");
    let pixels = renderer.readback_pixels().expect("readback_pixels failed");
    assert_eq!(pixels.len(), 256 * 256 * 4);

    let px = |x: usize, y: usize| -> [u8; 4] {
        let off = (y * 256 + x) * 4;
        [
            pixels[off],
            pixels[off + 1],
            pixels[off + 2],
            pixels[off + 3],
        ]
    };

    // Center pixel looks through the carved 2x2 L3 hole (dark clear color ~ [85, 85, 85, 255]).
    let hole = px(128, 128);
    // Offset pixel (-28 px from center) hits the solid cyan-metallic L3 frame.
    let rim = px(100, 128);
    // Right-side pixel (+82 px -> x ≈ 1.84 in [1..2]) hits the uniform COW L1 orange slab at (1, 1, 2).
    let cow = px(210, 128);
    assert!(
        rim[2] > rim[0] + 15,
        "L3 rim should render cyan/blue-tinted voxel, got rim={rim:?}, hole={hole:?}, cow={cow:?}"
    );
    assert!(
        cow[0] > cow[2] + 15,
        "L1 COW block should render orange-tinted voxel, got cow={cow:?}"
    );
    assert_ne!(
        rim, hole,
        "carved L3 pinhole must expose background vs solid rim: rim={rim:?}, hole={hole:?}"
    );
}

#[test]
fn emissive_voxel_contributes_smooth_gi_without_sunlight() {
    let mut renderer = match try_create_headless() {
        Some(r) => r,
        None => return,
    };

    let dims = [6u32, 4u32, 6u32];
    let count = (dims[0] * dims[1] * dims[2]) as usize;
    let l1_idx = |x: u32, y: u32, z: u32| -> usize { (x + dims[0] * (y + dims[1] * z)) as usize };
    let wall_rgba = pack_rgba(185, 185, 185, 255);
    let wall_vis = pack_visual(0, 220, 0);

    let build_room_l1 = |emitter_cell: Option<[u32; 4]>| {
        let mut cells = vec![[0u32; 4]; count];
        for z in 0..dims[2] {
            for y in 0..dims[1] {
                for x in 0..dims[0] {
                    let is_shell = x == 0
                        || x == dims[0] - 1
                        || y == 0
                        || y == dims[1] - 1
                        || z == 0
                        || z == 4;
                    if is_shell {
                        cells[l1_idx(x, y, z)] = [u32::MAX, wall_rgba, wall_vis, 0];
                    }
                }
            }
        }
        if let Some(cell) = emitter_cell {
            // Place off-camera inside the enclosed dark room to the right of view frustum.
            cells[l1_idx(4, 1, 3)] = cell;
        }
        cells
    };

    let render_room =
        |renderer: &mut GraphRenderer, sig: u64, l1_cells: Vec<[u32; 4]>| -> Vec<u8> {
            let scene = Arc::new(VoxelSceneUpload {
                signature: sig,
                elapsed_seconds: 1.0,
                chunks: vec![Arc::new(VoxelChunkUpload {
                    entity_id: 2,
                    version: sig,
                    origin_l1: [0, 0, 0],
                    dims_l1: dims,
                    l1_cells,
                    brick_occ: Vec::new(),
                    brick_cells: Vec::new(),
                })],
                cursor: None,
                atlas: None,
            });
            renderer.set_voxel_scene(Some(scene));

            let eye = [2.0, 1.8, 1.5];
            let target = [2.0, 1.7, 4.0];
            let vp = make_camera_vp(eye, target, 1.0);
            let frame_data = FrameUBOData {
                view_proj: vp,
                camera_position: [eye[0], eye[1], eye[2], 0.0],
                light_direction: [0.0, 1.0, 0.0, 0.0],
                light_color: [0.0, 0.0, 0.0, 0.0],
                viewport_w: 256.0,
                viewport_h: 256.0,
                exposure: 1.0,
                ..Default::default()
            };

            for _ in 0..2 {
                let out_of_date = renderer
                    .render(
                        &[],
                        &frame_data,
                        [[0.0; 4]; 4],
                        [[0.0; 4]; 4],
                        0,
                        0,
                        0,
                        0,
                        0.0,
                        0.0,
                        &[],
                        RenderMode::Raster,
                        0,
                        0.0,
                        0,
                        1.0,
                        &[],
                        PostGrade::default(),
                        true,
                        false,
                        &[],
                    )
                    .expect("room render failed");
                assert!(!out_of_date);
            }
            renderer
                .copy_offscreen_to_readback()
                .expect("copy_offscreen_to_readback failed");
            renderer.readback_pixels().expect("readback_pixels failed")
        };

    let stats_patch = |pixels: &[u8]| -> ([f64; 3], f64, u8) {
        let mut sum = [0.0f64; 3];
        let mut n = 0.0f64;
        let mut diff_sum = 0.0f64;
        let mut diff_n = 0.0f64;
        let mut max_diff = 0u8;
        for y in 108..148usize {
            for x in 96..160usize {
                let off = (y * 256 + x) * 4;
                let r = pixels[off];
                let g = pixels[off + 1];
                let b = pixels[off + 2];
                sum[0] += r as f64;
                sum[1] += g as f64;
                sum[2] += b as f64;
                n += 1.0;
                if x + 1 < 160 {
                    let off_r = (y * 256 + x + 1) * 4;
                    let dr = r.abs_diff(pixels[off_r]);
                    let dg = g.abs_diff(pixels[off_r + 1]);
                    let db = b.abs_diff(pixels[off_r + 2]);
                    let d = dr.max(dg).max(db);
                    max_diff = max_diff.max(d);
                    diff_sum += (dr as f64 + dg as f64 + db as f64) / 3.0;
                    diff_n += 1.0;
                }
            }
        }
        (
            [sum[0] / n, sum[1] / n, sum[2] / n],
            diff_sum / diff_n,
            max_diff,
        )
    };

    let dark_pixels = render_room(&mut renderer, 10, build_room_l1(None));
    let (dark_rgb, _, _) = stats_patch(&dark_pixels);

    // Test transient BURNING flag (emissive byte == 0) contributing colored indirect GI.
    let burning_vis = pack_visual(VIS_BURNING, 180, 0);
    let burning_rgba = pack_rgba(255, 110, 35, 255);
    let lit_pixels = render_room(
        &mut renderer,
        11,
        build_room_l1(Some([u32::MAX, burning_rgba, burning_vis, 0])),
    );
    let (lit_rgb, neighbor_diff, max_neighbor_diff) = stats_patch(&lit_pixels);

    let dark_luma = (dark_rgb[0] + dark_rgb[1] + dark_rgb[2]) / 3.0;
    let lit_luma = (lit_rgb[0] + lit_rgb[1] + lit_rgb[2]) / 3.0;
    // With no sun and no sky light the baseline is exactly zero, so the emissive
    // voxel is the only light source: the shaded wall averages ~5.3/255. A 3.0
    // threshold leaves room for driver/hardware variation while still catching an
    // NEE regression (falls back to 0) or a denoiser flattening the GI away.
    assert!(
        lit_luma > dark_luma + 3.0,
        "emissive BURNING voxel must bounce noticeable GI onto shaded wall: dark={dark_rgb:?}, lit={lit_rgb:?}"
    );
    assert!(
        lit_rgb[0] > lit_rgb[2] + 6.0,
        "warm emissive voxel GI must tint neutral gray wall warm: lit={lit_rgb:?}"
    );
    assert!(
        neighbor_diff < 2.5 && max_neighbor_diff < 12,
        "denoised flat GI wall must be grain-free: mean_diff={neighbor_diff:.2}, max_diff={max_neighbor_diff}, lit={lit_rgb:?}"
    );
}
