//! End-to-end offscreen integration test for Emergence with DLSS pre-upscale and clean exit.

#[test]
fn headless_emergence_dlss_5pct_and_clean_exit() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    if !manifest_dir.join("assets/scenes.pak").is_file() {
        eprintln!("[SKIP] emergence assets/scenes.pak not built");
        return;
    }
    std::env::set_current_dir(manifest_dir).expect("set cwd to emergence project dir");

    emergence_core::commands::set_upscaler_config(5, 0.05, 0.5);
    emergence_core::commands::set_dlss_pre_upscale(0); 

    emergence_core::build_app()
        .run_headless(12, 1280, 720)
        .expect("offscreen emergence DLSS 5% run and clean exit must succeed");
}
