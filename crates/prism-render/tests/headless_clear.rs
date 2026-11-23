//! Headless Vulkan initialisation test.
//!
//! Creates a [`GraphRenderer`] without any window surface, clears the offscreen
//! image to a known colour, reads the pixels back via a host-visible buffer,
//! and asserts the RGBA values.
//!
//! This test proves the entire GPU initialisation chain works:
//!   Instance → Physical Device → Logical Device → Queues → Command Pool
//!
//! Unlike a windowed test it runs on CI servers and in Termux without display
//! hardware — only a conformant Vulkan driver (e.g. `libvulkan.so`) is needed.

use prism_render::GraphRenderer;

/// Tries to create a headless GraphRenderer.
///
/// Returns `None` when no Vulkan driver is available (graceful skip).
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
                return None;
            }
            panic!("headless_new failed: {e:#}");
        }
    }
}

#[test]
fn headless_clear_to_red_and_readback() {
    let mut renderer = match try_create_headless() {
        Some(r) => r,
        None => return,
    };

    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    renderer
        .clear_offscreen(RED)
        .expect("clear_offscreen failed");

    let pixels = renderer.readback_pixels().expect("readback_pixels failed");

    assert_eq!(pixels.len(), 256 * 256 * 4, "unexpected pixel buffer size");

    let w = 256usize;
    let stride = 4;

    fn pixel_at(buf: &[u8], x: usize, y: usize, w: usize, stride: usize) -> [u8; 4] {
        let off = (y * w + x) * stride;
        [buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]
    }

    for &(x, y) in &[(0, 0), (255, 0), (0, 255), (255, 255), (128, 128)] {
        let px = pixel_at(&pixels, x, y, w, stride);
        assert_eq!(
            px,
            [255, 0, 0, 255],
            "pixel at ({x},{y}) expected [255,0,0,255] got {px:?}"
        );
    }
}

#[test]
fn headless_clear_to_green_and_readback() {
    let mut renderer = match try_create_headless() {
        Some(r) => r,
        None => return,
    };

    renderer
        .clear_offscreen([0.0, 1.0, 0.0, 1.0])
        .expect("clear_offscreen to green failed");

    let pixels = renderer.readback_pixels().expect("readback_pixels failed");

    let px = &pixels[0..4];
    assert_eq!(px, [0, 255, 0, 255], "first pixel should be pure green");
}

#[test]
fn headless_clear_to_blue_and_readback() {
    let mut renderer = match try_create_headless() {
        Some(r) => r,
        None => return,
    };

    renderer
        .clear_offscreen([0.0, 0.0, 1.0, 1.0])
        .expect("clear_offscreen to blue failed");

    let pixels = renderer.readback_pixels().expect("readback_pixels failed");

    let px = &pixels[0..4];
    assert_eq!(px, [0, 0, 255, 255], "first pixel should be pure blue");
}

#[test]
fn headless_render_real_frame_and_readback() {
    let mut renderer = match try_create_headless() {
        Some(r) => r,
        None => return,
    };

    let frame_data = prism_render::descriptor::FrameUBOData::default();
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
            prism_render::RenderMode::Raster,
            0,
            0.0,
            0,
            1.0,
            &[],
            prism_render::render_graph::PostGrade::default(),
            false,
            false,
            &[],
        )
        .expect("headless render failed");

    assert!(!out_of_date);

    renderer
        .copy_offscreen_to_readback()
        .expect("copy_offscreen_to_readback failed");

    let pixels = renderer.readback_pixels().expect("readback_pixels failed");
    assert_eq!(pixels.len(), 256 * 256 * 4);

    let px = &pixels[0..4];
    assert!(
        px[0] > 0 && px[1] > 0 && px[2] > 0,
        "first pixel should contain rendered color, got {px:?}"
    );
}

/// #431: When device supports `fillModeNonSolid`, wireframe pipeline can create and present normally.
///
/// Empty draw list also goes through `ForwardPass::ensure_pipelines`, so this covers
/// `POLYGON_MODE_LINE` pipeline creation, not just setting the field.
#[test]
fn headless_wireframe_pipeline_builds_and_renders() {
    let mut renderer = match try_create_headless() {
        Some(r) => r,
        None => return,
    };
    if !renderer.context().wireframe_supported {
        eprintln!("[SKIP] fillModeNonSolid not supported by this driver");
        return;
    }

    let frame_data = prism_render::descriptor::FrameUBOData::default();
    renderer
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
            prism_render::RenderMode::Raster,
            0,
            0.0,
            0,
            1.0,
            &[],
            prism_render::render_graph::PostGrade::default(),
            true,
            false,
            &[],
        )
        .expect("wireframe render failed");

    renderer
        .copy_offscreen_to_readback()
        .expect("copy_offscreen_to_readback failed");
    let pixels = renderer.readback_pixels().expect("readback_pixels failed");
    assert_eq!(pixels.len(), 256 * 256 * 4);
}

#[test]
fn headless_post_pass_never_ends_in_present_src() {
    let renderer = match try_create_headless() {
        Some(r) => r,
        None => return,
    };

    let post = renderer
        .graph()
        .pass_ref::<prism_render::PostPass>()
        .expect("PostPass should be registered in the render graph");

    assert_eq!(
        post.final_layout(),
        ash::vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        "headless PostPass must leave the offscreen image in COLOR_ATTACHMENT_OPTIMAL"
    );
}

#[test]
fn rt_scheduler_is_registered_in_graph() {
    let renderer = match try_create_headless() {
        Some(r) => r,
        None => return,
    };

    assert!(
        renderer
            .graph()
            .pass_ref::<prism_render::rt_scheduler::RenderTextureScheduler>()
            .is_some(),
        "RenderTextureScheduler should be registered in graph"
    );
}

#[test]
fn headless_context_has_validation_when_required() {
    let required = std::env::var("PRISM_REQUIRE_VALIDATION")
        .map(|value| !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false"))
        .unwrap_or(false);
    if !required {
        return;
    }

    let renderer = GraphRenderer::headless_new(None)
        .expect("PRISM_REQUIRE_VALIDATION is set but headless context creation failed");
    assert!(
        renderer.context().validation_enabled(),
        "PRISM_REQUIRE_VALIDATION is set but VK_LAYER_KHRONOS_validation was not enabled"
    );
}

#[test]
fn headless_path_trace_pass_registered_if_supported() {
    let renderer = match try_create_headless() {
        Some(r) => r,
        None => return,
    };

    let has_pt = renderer
        .graph()
        .pass_ref::<prism_render::PathTracePass>()
        .is_some();
    let rt_supported = renderer.context().rt_caps.acceleration_structure;
    assert_eq!(
        has_pt, rt_supported,
        "PathTracePass registration must match acceleration_structure support"
    );
}
