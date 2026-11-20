//! Smoke test verifying Streamline initialization and DLSS graph execution on NVIDIA GPUs.

#[path = "golden_support/mod.rs"]
mod golden_support;

use golden_support::{draw_item, FrameHarness, FrameOptions};
use prism_render::DlssMode;

struct StderrLogger;
impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }
    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!("[{}] {}", record.level(), record.args());
        }
    }
    fn flush(&self) {}
}
static LOGGER: StderrLogger = StderrLogger;

#[test]
fn streamline_dlss_headless_smoke() {
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Info);

    let Some(mut harness) = FrameHarness::new(256) else {
        return;
    };

    let caps = harness.renderer().streamline_caps();
    eprintln!("Streamline caps: {caps:?}");

    if let Some(sl) = harness.renderer().streamline_runtime() {
        for (name, mode) in [
            ("Quality", prism_render::streamline::DlssMode::MaxQuality),
            ("Balanced", prism_render::streamline::DlssMode::Balanced),
            (
                "Performance",
                prism_render::streamline::DlssMode::MaxPerformance,
            ),
            (
                "UltraPerformance",
                prism_render::streamline::DlssMode::UltraPerformance,
            ),
        ] {
            let opts = prism_render::streamline::DlssOptions {
                mode,
                output_width: 1920,
                output_height: 1080,
                ..Default::default()
            };
            if let Some(opt) = sl.dlss_optimal_settings(&opts) {
                eprintln!(
                    "DLSS 1920x1080 {name}: optimal={}x{} min={}x{} max={}x{}",
                    opt.optimal_render_width,
                    opt.optimal_render_height,
                    opt.render_width_min,
                    opt.render_height_min,
                    opt.render_width_max,
                    opt.render_height_max
                );
            }
        }
    }

    harness.renderer_mut().settings_mut().dlss_mode = DlssMode::Quality;
    let item = draw_item(harness.mesh_handle());
    let pixels = harness.render_draws(std::slice::from_ref(&item), FrameOptions::default());
    assert_eq!(pixels.len(), 256 * 256 * 4);
}
