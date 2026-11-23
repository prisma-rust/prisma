//! End-to-end smoke test for host (editor viewport) mode.
//!
//! Covers the chain editor Scene / Game view truly depends on:
//! `App::hosted` -> `load_scene_json_file` (loose `.scene.json`) ->
//! `start_hosted` (windowless offscreen render thread) -> `host_tick` -> `take_hosted_frame`.
//!
//! Gracefully skip when no Vulkan driver (CI container / no GPU machine).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use prism_app::App;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(PathBuf::from)
        .expect("crates/prism-app has a grandparent")
}

#[test]
fn hosted_viewport_renders_a_frame_from_a_loose_scene_json() {
    let _ = env_logger::builder().is_test(true).try_init();

    let scene = repo_root().join("assets/scenes/default.scene.json");
    if !scene.is_file() {
        eprintln!("[SKIP] {} does not exist", scene.display());
        return;
    }

    let mut app = App::hosted(prism_engine::config::AppConfig::default());
    let entities = app
        .load_scene_json_file(&scene)
        .expect("loose .scene.json should load");
    assert!(
        !entities.is_empty(),
        "scene should have at least one entity"
    );

    app.start_hosted(320, 180);

    // Renderer asynchronously created in render thread (hundreds of ms), frames only after.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut frame = None;
    while Instant::now() < deadline {
        app.host_tick(false);
        if let Some(image) = app.take_hosted_frame() {
            frame = Some(image);
            break;
        }
        if let Some(failure) = app.render_failure() {
            eprintln!("[SKIP] render thread unavailable: {failure}");
            app.shutdown_hosted();
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let frame = frame.expect("no frame produced within 20s");
    assert_eq!(
        (frame.width, frame.height),
        (320, 180),
        "frame size should equal host requested offscreen size"
    );
    assert_eq!(
        frame.pixels.len(),
        320 * 180 * 4,
        "frame must be packed RGBA8"
    );
    // All black usually means PostPass did not really write to offscreen target.
    let lit = frame
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .any(|px| px[0] > 0 || px[1] > 0 || px[2] > 0);
    assert!(lit, "rendered frame is all black");

    // Scaling should not break: run `resize_offscreen` then take a frame.
    app.host_resize(160, 90);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut resized = None;
    while Instant::now() < deadline {
        app.host_tick(false);
        if let Some(image) = app.take_hosted_frame() {
            if (image.width, image.height) == (160, 90) {
                resized = Some(image);
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let resized = resized.expect("did not get new-size frame after resize");
    assert_eq!(resized.pixels.len(), 160 * 90 * 4);

    app.shutdown_hosted();
}
