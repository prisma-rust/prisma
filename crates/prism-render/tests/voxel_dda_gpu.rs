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
    let sub = 0 + 4 * (2 + 4 * 0); // local L2 (0, 2, 0) -> world [2.0..2.25, 1.5..1.75, 2.0..2.25]
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
        chunks: vec![VoxelChunkUpload {
            entity_id: 1,
            version: 1,
            origin_l1: [0, 0, 0],
            dims_l1: dims,
            l1_cells,
            brick_occ: vec![[occ_lo, occ_hi]],
            brick_cells,
        }],
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

    let mut frame_data = FrameUBOData::default();
    frame_data.viewProj = vp;
    frame_data.cameraPosition = [eye[0], eye[1], eye[2], 0.0];
    frame_data.lightDirection = [0.4, 0.8, -0.45, 2.5];
    frame_data.lightColor = [1.0, 0.98, 0.92, 0.4];
    frame_data.viewportW = 256.0;
    frame_data.viewportH = 256.0;
    frame_data.exposure = 1.0;

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
