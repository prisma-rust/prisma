//! Headless GPU render test using Emergence's real fixed-seed (`2026`)
//! procedural world generator (`EmergenceRuntime::bootstrap`).

use std::path::PathBuf;

use emergence_core::EmergenceRuntime;
use prism_ecs::World;
use prism_engine::render_system::extract_frame_packet;
use prism_game_api::GameWorld;
use prism_render::descriptor::FrameUBOData;
use prism_render::render_graph::{InternalScaleMode, MobileUpscaler, PostGrade};
use prism_render::{GraphRenderer, RenderMode};

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

#[test]
fn emergence_seed_2026_renders_with_sgsr2_and_asr() {
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
        .unwrap_or((1280, 720));

    renderer
        .resize_offscreen(out_w, out_h)
        .expect("resize_offscreen");

    // Bootstrap the real fixed-seed (2026) Emergence world, step once so daylight/sky/chunks sync,
    // and propagate LocalToWorld matrices via hierarchy_system.
    let mut ecs = World::new();
    let mut runtime = {
        let mut gw = GameWorld::host(&mut ecs);
        let rt = EmergenceRuntime::bootstrap(&mut gw, 2026);
        rt
    };
    {
        let mut gw = GameWorld::host(&mut ecs);
        let mut state = Some(runtime);
        emergence_core::emergence_fixed_tick(&mut state, &mut gw, 1.0 / 60.0);
        runtime = state.unwrap();
    }
    let _ = runtime;
    prism_engine::scene::systems::hierarchy::hierarchy_system(&mut ecs);

    let aspect = out_w as f32 / out_h as f32;
    let packet = extract_frame_packet(&mut ecs, aspect, &glam::Mat4::IDENTITY);
    renderer.set_voxel_scene(packet.voxel_scene.clone());

    let dump_dir = std::env::var_os("PRISMA_UPSCALER_DUMP_DIR").map(PathBuf::from);
    if let Some(ref dir) = dump_dir {
        std::fs::create_dir_all(dir).expect("create dump dir");
    }

    let modes = [
        ("native", MobileUpscaler::Off, [0, 0], 8),
        ("sgsr2", MobileUpscaler::Sgsr2, [out_w / 2, out_h / 2], 12),
        ("asr", MobileUpscaler::ArmAsr, [out_w / 2, out_h / 2], 12),
        ("linear", MobileUpscaler::Linear, [out_w / 2, out_h / 2], 4),
        (
            "nearest",
            MobileUpscaler::Nearest,
            [out_w / 2, out_h / 2],
            4,
        ),
    ];

    for (name, mode, internal_res, frames) in modes {
        renderer.set_mobile_upscaler(mode, 0.5);
        renderer.set_internal_resolution(internal_res, InternalScaleMode::Stretch);

        for _ in 0..frames {
            let mut frame_data = FrameUBOData {
                viewProj: packet.scene.view_proj.to_cols_array_2d(),
                view: packet.scene.view.to_cols_array_2d(),
                cameraPosition: [
                    packet.scene.eye[0],
                    packet.scene.eye[1],
                    packet.scene.eye[2],
                    packet.scene.lights.len() as f32,
                ],
                lightDirection: packet.scene.light_direction.to_array(),
                lightColor: packet.scene.light_color.to_array(),
                lightViewProj: packet.scene.light_view_proj.to_cols_array_2d(),
                lightViewProj1: packet.scene.light_view_proj1.to_cols_array_2d(),
                shadowSplit: packet.scene.shadow_split,
                shadowCascadeCount: packet.scene.shadow_cascade_count,
                viewportW: out_w as f32,
                viewportH: out_h as f32,
                exposure: packet.scene.exposure,
                shadowStrength: packet.scene.shadow_strength,
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
            eprintln!("[emergence-seed-2026] saved {}", path.display());
        }
    }
}
