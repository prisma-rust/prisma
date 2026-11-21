//! Headless Vulkan GPU integration test for MobileUpscalerPass (SGSR 2, Arm ASR,
//! Nearest, Linear) over the three-tier voxel DDA + GI pipeline.

use std::path::PathBuf;
use std::sync::Arc;

use prism_render::descriptor::FrameUBOData;
use prism_render::render_graph::{InternalScaleMode, MobileUpscaler, PostGrade};
use prism_render::{GraphRenderer, RenderMode, VoxelChunkUpload, VoxelSceneUpload};

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
    let fov_y = 58.0f32.to_radians();
    let inv_tan = 1.0 / (fov_y * 0.5).tan();
    let near = 0.05f32;
    let far = 120.0f32;
    let q = far / (near - far);
    let proj = [
        [inv_tan / aspect, 0.0, 0.0, 0.0],
        [0.0, -inv_tan, 0.0, 0.0],
        [0.0, 0.0, q, -1.0],
        [0.0, 0.0, q * near, 0.0],
    ];
    mul_mat(proj, view)
}

fn pack_rgba(r: u8, g: u8, b: u8, a: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16) | ((a as u32) << 24)
}

fn pack_visual(flags: u32, roughness: u8, emissive: u8) -> u32 {
    flags | ((roughness as u32) << 16) | ((emissive as u32) << 24)
}

fn push_l3_brick(
    brick_occ: &mut Vec<[u32; 2]>,
    brick_cells: &mut Vec<[u32; 4]>,
    mut f: impl FnMut(u32, u32, u32) -> Option<(u32, u32)>,
) -> u32 {
    let b_idx = brick_occ.len() as u32;
    let mut occ_lo = 0u32;
    let mut occ_hi = 0u32;
    for lz in 0..4u32 {
        for ly in 0..4u32 {
            for lx in 0..4u32 {
                let bit = lx + 4 * (ly + 4 * lz);
                if let Some((rgba, vis)) = f(lx, ly, lz) {
                    if bit < 32 {
                        occ_lo |= 1 << bit;
                    } else {
                        occ_hi |= 1 << (bit - 32);
                    }
                    brick_cells.push([rgba, vis, u32::MAX, u32::MAX]);
                } else {
                    brick_cells.push([0, 0, 0, 0]);
                }
            }
        }
    }
    brick_occ.push([occ_lo, occ_hi]);
    b_idx
}

/// Builds a sunlit Cornell-style stone temple atrium with colored bounce walls,
/// recessed cyan/amber emissive lanterns, metallic bronze pillars, and L3 latticework.
fn build_test_voxel_scene() -> Arc<VoxelSceneUpload> {
    let dims = [16u32, 10u32, 16u32];
    let count = (dims[0] * dims[1] * dims[2]) as usize;
    let l1_idx = |x: u32, y: u32, z: u32| -> usize { (x + dims[0] * (y + dims[1] * z)) as usize };
    let mut l1_cells = vec![[0u32; 4]; count];
    let mut brick_occ = Vec::new();
    let mut brick_cells = Vec::new();

    let travertine_a = pack_rgba(218, 212, 200, 255);
    let travertine_b = pack_rgba(196, 189, 176, 255);
    let travertine_vis = pack_visual(0, 205, 0);

    let slate_dark = pack_rgba(72, 78, 90, 255);
    let slate_vis = pack_visual(0, 180, 0);

    let terracotta = pack_rgba(205, 68, 48, 255);
    let terracotta_vis = pack_visual(0, 215, 0);

    let jade = pack_rgba(52, 158, 108, 255);
    let jade_vis = pack_visual(0, 210, 0);

    let bronze = pack_rgba(214, 158, 92, 255);
    let bronze_vis = pack_visual(1 << 12, 55, 0);

    let cyan_glow = pack_rgba(75, 225, 255, 255);
    let cyan_vis = pack_visual(1 << 11, 30, 255);

    let amber_glow = pack_rgba(255, 138, 36, 255);
    let amber_vis = pack_visual(1 << 11, 30, 255);

    // 1. Enclosed atrium: travertine floor & ceiling (with skylight opening),
    //    terracotta left wall (bounces warm red GI), jade right wall (bounces cool green GI),
    //    travertine back wall with recessed dark slate niches.
    for z in 0..dims[2] {
        for y in 0..dims[1] {
            for x in 0..dims[0] {
                let hash = (x * 7 + y * 13 + z * 19) % 2;
                let stone = if hash == 0 {
                    travertine_a
                } else {
                    travertine_b
                };

                // Floor (y = 0): travertine with central slate inlay.
                if y == 0 {
                    let in_inlay = (5..=10).contains(&x) && (5..=11).contains(&z);
                    let (c, v) = if in_inlay && (x == 5 || x == 10 || z == 5 || z == 11) {
                        (slate_dark, slate_vis)
                    } else {
                        (stone, travertine_vis)
                    };
                    l1_cells[l1_idx(x, y, z)] = [u32::MAX, c, v, 0];
                    continue;
                }

                // Ceiling (y = 9): solid except a 5x4 skylight opening so sun casts crisp shafts + soft GI fill.
                if y == dims[1] - 1 {
                    let skylight = (6..=10).contains(&x) && (7..=10).contains(&z);
                    if !skylight {
                        l1_cells[l1_idx(x, y, z)] = [u32::MAX, stone, travertine_vis, 0];
                    }
                    continue;
                }

                // Left wall (x = 0): warm terracotta bounce wall.
                if x == 0 {
                    l1_cells[l1_idx(x, y, z)] = [u32::MAX, terracotta, terracotta_vis, 0];
                    continue;
                }
                // Right wall (x = 15): jade-green bounce wall.
                if x == dims[0] - 1 {
                    l1_cells[l1_idx(x, y, z)] = [u32::MAX, jade, jade_vis, 0];
                    continue;
                }
                // Back wall (z = 15) and front wall behind camera (z = 0).
                if z == dims[2] - 1 || z == 0 {
                    l1_cells[l1_idx(x, y, z)] = [u32::MAX, stone, travertine_vis, 0];
                }
            }
        }
    }

    // 2. Stepped altar & metallic bronze colonnade framing the center.
    for x in 5..=10 {
        l1_cells[l1_idx(x, 1, 12)] = [u32::MAX, slate_dark, slate_vis, 0];
        l1_cells[l1_idx(x, 1, 13)] = [u32::MAX, slate_dark, slate_vis, 0];
        l1_cells[l1_idx(x, 1, 14)] = [u32::MAX, slate_dark, slate_vis, 0];
    }
    for x in 6..=9 {
        l1_cells[l1_idx(x, 2, 13)] = [u32::MAX, travertine_a, travertine_vis, 0];
    }
    // Central bronze pedestal + floating cyan crystal core.
    l1_cells[l1_idx(7, 2, 11)] = [u32::MAX, bronze, bronze_vis, 0];
    l1_cells[l1_idx(8, 2, 11)] = [u32::MAX, bronze, bronze_vis, 0];
    l1_cells[l1_idx(7, 1, 11)] = [u32::MAX, slate_dark, slate_vis, 0];
    l1_cells[l1_idx(8, 1, 11)] = [u32::MAX, slate_dark, slate_vis, 0];

    // Carved L3 floating octahedron-like crystal on the central pedestal.
    let crystal_brick = push_l3_brick(&mut brick_occ, &mut brick_cells, |lx, ly, lz| {
        let dx = lx as i32 - 1;
        let dy = ly as i32 - 1;
        let dz = lz as i32 - 1;
        if dx.abs() + dy.abs() + dz.abs() <= 2 {
            Some((cyan_glow, cyan_vis))
        } else {
            None
        }
    });
    l1_cells[l1_idx(7, 3, 11)] = [crystal_brick, 0, 0, 0];

    // Twin bronze columns with L3 fluted capitals on left & right.
    for &(cx, cz) in &[(4u32, 8u32), (11u32, 8u32), (4u32, 12u32), (11u32, 12u32)] {
        for y in 1..=7 {
            l1_cells[l1_idx(cx, y, cz)] = [u32::MAX, bronze, bronze_vis, 0];
        }
        let cap = push_l3_brick(&mut brick_occ, &mut brick_cells, |lx, ly, lz| {
            let rim = lx == 0 || lx == 3 || lz == 0 || lz == 3;
            if ly == 3 || (ly >= 1 && !rim) || (ly == 0 && (lx + lz) % 2 == 0) {
                Some((travertine_a, travertine_vis))
            } else {
                None
            }
        });
        l1_cells[l1_idx(cx, 8, cz)] = [cap, 0, 0, 0];
    }

    // 3. Recessed emissive GI braziers in the deep shaded side alcoves:
    //    Left alcove (x=1, z=10): warm amber brazier behind L3 stone grate.
    l1_cells[l1_idx(1, 1, 10)] = [u32::MAX, slate_dark, slate_vis, 0];
    l1_cells[l1_idx(1, 2, 10)] = [u32::MAX, amber_glow, amber_vis, 0];
    l1_cells[l1_idx(1, 2, 9)] = [u32::MAX, amber_glow, amber_vis, 0];
    let grate_l = push_l3_brick(&mut brick_occ, &mut brick_cells, |lx, ly, lz| {
        if (lx == 0 || lx == 3) && (ly == 0 || ly == 3 || lz == 0 || lz == 3) {
            Some((slate_dark, slate_vis))
        } else {
            None
        }
    });
    l1_cells[l1_idx(2, 2, 10)] = [grate_l, 0, 0, 0];

    //    Right alcove (x=14, z=10): cool cyan brazier behind L3 stone grate.
    l1_cells[l1_idx(14, 1, 10)] = [u32::MAX, slate_dark, slate_vis, 0];
    l1_cells[l1_idx(14, 2, 10)] = [u32::MAX, cyan_glow, cyan_vis, 0];
    l1_cells[l1_idx(14, 2, 9)] = [u32::MAX, cyan_glow, cyan_vis, 0];
    l1_cells[l1_idx(13, 2, 10)] = [grate_l, 0, 0, 0];

    // 4. Overhead L3 skylight lattice to cast delicate shadows & showcase upscaler edge reconstruction.
    let lattice = push_l3_brick(&mut brick_occ, &mut brick_cells, |lx, ly, lz| {
        if ly == 0 && (lx == 0 || lz == 0) {
            Some((travertine_b, travertine_vis))
        } else {
            None
        }
    });
    for x in 6..=10 {
        for z in 7..=10 {
            if (x + z) % 2 == 0 {
                l1_cells[l1_idx(x, 8, z)] = [lattice, 0, 0, 0];
            }
        }
    }

    Arc::new(VoxelSceneUpload {
        signature: 108,
        elapsed_seconds: 2.0,
        chunks: vec![Arc::new(VoxelChunkUpload {
            entity_id: 1,
            version: 1,
            origin_l1: [0, 0, 0],
            dims_l1: dims,
            l1_cells,
            brick_occ,
            brick_cells,
        })],
        cursor: None,
        atlas: None,
    })
}

#[test]
fn mobile_upscalers_render_valid_images_on_gpu() {
    let mut renderer = match try_create_headless() {
        Some(r) => r,
        None => return,
    };

    let (out_w, out_h) = std::env::var("PRISMA_UPSCALER_OUT_SIZE")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x').or_else(|| s.split_once('X'))?;
            Some((w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?))
        })
        .unwrap_or((960, 540));
    renderer
        .resize_offscreen(out_w, out_h)
        .expect("resize_offscreen");
    renderer.set_voxel_scene(Some(build_test_voxel_scene()));

    let eye = [7.8, 3.8, 2.2];
    let target = [7.8, 2.7, 11.2];
    let vp = make_camera_vp(eye, target, out_w as f32 / out_h as f32);

    let dump_dir = std::env::var_os("PRISMA_UPSCALER_DUMP_DIR").map(PathBuf::from);
    if let Some(ref dir) = dump_dir {
        std::fs::create_dir_all(dir).expect("create dump dir");
    }

    let modes = [
        ("native", MobileUpscaler::Off, [0, 0], 8),
        (
            "nearest",
            MobileUpscaler::Nearest,
            [out_w / 2, out_h / 2],
            4,
        ),
        ("linear", MobileUpscaler::Linear, [out_w / 2, out_h / 2], 4),
        ("sgsr2", MobileUpscaler::Sgsr2, [out_w / 2, out_h / 2], 12),
        ("asr", MobileUpscaler::ArmAsr, [out_w / 2, out_h / 2], 12),
    ];

    for (name, mode, internal_res, frames) in modes {
        renderer.set_mobile_upscaler(mode, 0.5);
        renderer.set_internal_resolution(internal_res, InternalScaleMode::Stretch);

        for _ in 0..frames {
            let mut frame_data = FrameUBOData {
                view_proj: vp,
                camera_position: [eye[0], eye[1], eye[2], 0.0],
                light_direction: [0.42, 0.80, -0.42, 0.0],
                light_color: [1.0, 0.88, 0.72, 4.2],
                viewport_w: out_w as f32,
                viewport_h: out_h as f32,
                exposure: 1.1,
                ..Default::default()
            };
            renderer.apply_taa_jitter(&mut frame_data);

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
                .expect("render");
            assert!(!out_of_date);
        }

        renderer
            .copy_offscreen_to_readback()
            .expect("copy_offscreen_to_readback");
        let rgba = renderer.readback_pixels().expect("readback_pixels");
        assert_eq!(rgba.len(), (out_w * out_h * 4) as usize);

        let non_black = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[0] > 5 || px[1] > 5 || px[2] > 5)
            .count();
        assert!(
            non_black > (out_w * out_h / 4) as usize,
            "{name} rendered mostly black ({non_black} non-black pixels)"
        );

        if let Some(ref dir) = dump_dir {
            let path = dir.join(format!("{name}.png"));
            let img =
                image::RgbaImage::from_raw(out_w, out_h, rgba).expect("valid RGBA image buffer");
            img.save(&path).expect("save PNG");
            eprintln!("[gpu-render] saved {}", path.display());
        }
    }
}
