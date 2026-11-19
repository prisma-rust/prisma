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

    harness.renderer_mut().settings_mut().dlss_mode = DlssMode::Quality;
    let item = draw_item(harness.mesh_handle());
    let pixels = harness.render_draws(std::slice::from_ref(&item), FrameOptions::default());
    assert_eq!(pixels.len(), 256 * 256 * 4);
}
