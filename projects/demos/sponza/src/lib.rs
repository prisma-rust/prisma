//! Sponza user project: uses engine public interface to start real-time path tracing and integrate SHaRC tuning panel.

#[cfg(feature = "player")]
pub mod tuning_ui;
#[cfg(feature = "player")]
pub use tuning_ui::SponzaTuningHook;

#[cfg(feature = "player")]
pub fn build_app() -> prism_app::App {
    use prism_engine::render_settings::{PtGiMode, RasterGiMode, RenderMode, RenderSettings};

    prism_app::app()
        .with_subsystem(prism_app::Subsystem::Render)
        .with_render_settings(|settings: &mut RenderSettings| {
            let mut auto_exposure = true;
            if let Ok(mode) = std::env::var("SPONZA_RENDER_MODE") {
                if mode.eq_ignore_ascii_case("raster") {
                    settings.render_mode = RenderMode::Raster;
                    settings.raster_gi = RasterGiMode::BakedProbe;
                    settings.tonemap_mode = 2;
                    auto_exposure = false;
                } else if mode.eq_ignore_ascii_case("pt_off") {
                    settings.render_mode = RenderMode::PathTrace;
                    settings.raster_gi = RasterGiMode::BakedProbe;
                } else {
                    settings.render_mode = RenderMode::PathTrace;
                    settings.pt_gi = PtGiMode::Sharc;
                }
            } else {
                settings.render_mode = RenderMode::PathTrace;
                settings.pt_gi = PtGiMode::Sharc;
            }
            if let Ok(tm) = std::env::var("SPONZA_TONEMAP_MODE") {
                if let Ok(val) = tm.parse::<u32>() {
                    settings.tonemap_mode = val;
                }
            }
            if let Ok(ae) = std::env::var("SPONZA_AUTO_EXPOSURE") {
                auto_exposure = ae == "1" || ae.eq_ignore_ascii_case("true");
            }
            settings.pt_max_bounces = 4;
            settings.pt_max_iterations = 0;
            settings.pt_ray_max_distance = 1000.0;
            settings.auto_exposure = auto_exposure;
            settings.debug_flags = 0;
            if let Ok(flags) = std::env::var("SPONZA_DEBUG_FLAGS") {
                match flags.parse::<u32>() {
                    Ok(flags) => settings.debug_flags = flags,
                    Err(_) => log::warn!("SPONZA_DEBUG_FLAGS is not a u32: {flags}"),
                }
            }
        })
        .with_frame_hook(SponzaTuningHook::default())
}

#[cfg(feature = "preview-module")]
mod preview {
    use prism_game_api::GameWorld;

    struct PreviewState;

    fn new_state() -> PreviewState {
        PreviewState
    }

    fn tick_state(_state: &mut PreviewState, _world: &mut GameWorld<'_>, _dt: f32) {}

    prism_game_api::export_project! {
        state = PreviewState,
        init = new_state,
        tick = tick_state,
        log_name = "sponza",
    }
}

#[cfg(all(target_os = "android", feature = "player"))]
#[no_mangle]
pub extern "C" fn android_main(android_app: prism_app::AndroidApp) {
    prism_app::run_on_android(build_app, android_app).expect("fatal Sponza application error");
}
