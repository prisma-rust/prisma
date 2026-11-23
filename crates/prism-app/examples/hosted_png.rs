//! Offscreen render -> PNG: no window created, no WSI (Wayland/X11 surface),
//! fully via [`App::hosted`] offscreen target + readback.
//!
//! Usage:
//! ```bash
//! cargo run -p prism-app --example hosted_png -- \
//!     assets/scenes/default.scene.json /tmp/opencode/offscreen.png [frames]
//! ```
//!
//! Purpose: verify GPU render path in environment without available surface (e.g. turnip + Xvfb no DRI3, or CI/headless machine),
//! capture material screenshots, before/after comparison for art tuning.

use std::path::Path;

use prism_app::App;
use prism_engine::config::AppConfig;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut args = std::env::args().skip(1);
    let scene = args
        .next()
        .unwrap_or_else(|| "assets/scenes/default.scene.json".to_string());
    let output = args
        .next()
        .unwrap_or_else(|| "/tmp/opencode/offscreen.png".to_string());
    let frames: u32 = args
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(30);
    let width: u32 = std::env::var("WIDTH")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1280);
    let height: u32 = std::env::var("HEIGHT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(720);

    let mut app = App::hosted(AppConfig::default());
    app.load_scene_json_file(Path::new(&scene))
        .map_err(|error| anyhow::anyhow!("load scene '{scene}': {error}"))?;
    app.start_hosted(width, height);

    // Render thread is async: per frame request_frame -> wait readback (wait up to 5s/frame).
    let mut frame = None;
    for index in 0..frames.max(1) {
        app.host_tick(true);
        for _ in 0..100 {
            if let Some(failure) = app.render_failure() {
                anyhow::bail!("render thread failed: {failure}");
            }
            if let Some(image) = app.take_hosted_frame() {
                frame = Some(image);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if frame.is_none() && index == 0 {
            log::warn!("hosted_png: no frame after first tick; retrying");
        }
    }

    let frame = frame.ok_or_else(|| anyhow::anyhow!("hosted renderer produced no frame"))?;
    let image = image::RgbaImage::from_raw(frame.width, frame.height, frame.pixels)
        .ok_or_else(|| anyhow::anyhow!("frame buffer size mismatch"))?;
    if let Some(dir) = Path::new(&output).parent() {
        std::fs::create_dir_all(dir).ok();
    }
    image.save(&output)?;
    println!(
        "offscreen: {}x{} -> {output} ({} frames)",
        frame.width, frame.height, frames
    );

    app.shutdown_hosted();
    Ok(())
}
