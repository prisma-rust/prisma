//! Prisma platform application layer
//!
//! Owns event loop, window and frame orchestration---glue layer connecting platform input, engine (ECS/logic) and
//! renderer (Vulkan) glue layer.
//!
//! ## Window backend (feature)
//!
//! - `winit-backend` (default): winit event loop + `ApplicationHandler`.
//! - `win32-native` (default off, Windows only): Win32 native window + own message loop
//!   ([`App::run_win32`]). Enable it must turn off `winit-backend`, otherwise this crate
//!   still has winit code compiled (size not saved). Both on then winit takes precedence.
//!
//! Both backends share same engine/render logic ([`App::tick_frame`] / [`App::shutdown`]),
//! difference only in "how window comes, how events dispatched".
//!
//! ## User project access (full ECS experience)
//!
//! Engine init, event loop, render all handled by this layer; user project via
//! [`app`] gets [`App`], uses builder methods to register own ECS content then
//! starts event loop:
//!
//! ```no_run
//! use prism_app::app;
//!
//! fn main() {
//!     let mut app = app();
//!     app.insert_resource(MyState::default());
//!     app.add_system("my::system", |world, dt| { /* ... */ });
//!     app.run();
//! }
//! # struct MyState;
//! # impl Default for MyState { fn default() -> Self { MyState } }
//! ```
//!
//! - [`App::add_system`] / [`App::insert_resource`]: register ECS systems and resources;
//! - [`App::engine_mut`]: full ECS access (world / schedule / timer);
//! - [`App::run`]: desktop entry (own EventLoop / Win32 message loop);
//! - [`App::run_on`]: Android entry (external winit EventLoop, `winit-backend` only).
//!
//! UI uses engine self-developed ECS component UI (`Node` / `Style` / `Text`), with main render
//! channel drawn each frame.
//!
//! ## Neutral extension point (debug host / future host)
//!
//! This crate does not depend on any specific UI framework. External host can
//! inject [`FrameHook`] via [`App::with_frame_hook`]:
//!
//! - [`FrameHook::on_window_event`]: consume window events first (return `true` means consumed);
//! - [`FrameHook::on_tick`]: access world / render settings / stats each frame, and can via
//!   [`RenderShared::send_overlay_message`] deliver **type-erased**
//!   overlay messages;
//! - [`FrameHook::overlay`]: provides render-thread-side
//!   `prism_render::external_overlay::SwapchainOverlay` factory.
//!
//! When adding UI functions **do not** add specific UI framework dependency back into this crate---extend
//! hook interface suffices.
//!
//! ## Architecture (multi-thread)
//!
//! ```text
//! Main thread                              Render thread
//!   ────────────                        ────────────
//!   about_to_wait:                       loop:
//!     engine.fixed_update × N              wait_for_packet()
//!     engine.update                        begin_frame()
//!     engine.late_update                   execute()
//!     audio.update                         present()    <- vertical sync here
//!     extract_frame_packet ──packet──►   │
//!     frame_hook.on_tick ──overlay_msg──►│
//!
//! resumed(): main thread creates window (fast, ~ms) -> generates render thread (internal async
//!   builds GraphRenderer, ~hundreds ms, does not block window events) -> returns immediately
//! suspended(): stop render thread (destroy renderer, release Vulkan surface)
//! ```
//!
//! Startup asset parsing via asset_requests/asset_results channel: main thread CPU side
//! prepares upload requests, render thread GPU side uploads and returns handles.

#[cfg(any(target_os = "android", test))]
mod android_assets;
mod app;
#[cfg(feature = "audio")]
mod audio_decode_runner;
mod bench_report;
pub mod crash_dialog;
mod debug_hud;
mod hook;
pub mod logging;
#[cfg(feature = "physics")]
mod physics_runner;
/// Main-thread physics driver. Exposed so headless scenario tests can step the
/// same runtime the app uses without creating a window.
#[cfg(feature = "physics")]
pub use physics_runner::PhysicsRuntime;
/// Kinematic character moves (#560 P2): write requests into
/// `CharacterMoveRequests`, read the corrected moves back from
/// `CharacterMoveResults` after the step.
#[cfg(feature = "physics")]
pub use physics_runner::{
    CharacterMoveInput, CharacterMoveRequest, CharacterMoveRequests, CharacterMoveResult,
    CharacterMoveResults, CharacterMoveSettings,
};
#[cfg(feature = "audio")]
pub mod one_shot;
pub mod plugin;
mod render_graph_report;
mod render_runner;
mod render_shared;
#[cfg(feature = "audio")]
pub mod scene_audio;
mod screenshot;

use prism_engine::config::AppConfig;

#[cfg(all(feature = "win32-native", not(windows)))]
compile_error!("prism-app: `win32-native` valid on Windows target only");

#[cfg(not(any(feature = "winit-backend", all(windows, feature = "win32-native"))))]
compile_error!(
    "prism-app: enable at least one window backend (`winit-backend`, or `win32-native` on Windows)"
);

pub use app::{App, PointerLockPolicy, Subsystem};
pub use debug_hud::DebugHud;
pub use hook::FrameHook;
pub use plugin::{
    AppPlugin, DefaultAudioPlugin, DefaultPhysicsPlugin, DefaultPlugins, DefaultRenderPlugin,
    PluginGroup,
};
pub use prism_engine::ecs::World;
pub use prism_engine::render_settings::{MobileUpscaler, RenderSettings};
pub use prism_platform::{
    current_platform, is_android, is_desktop, is_mobile, PlatformWindow, TargetPlatform,
};
pub use render_shared::{FrameImage, RenderShared, RenderStats};

/// Platform handle needed for user project Android entry; specific window lib type encapsulated by app layer.
#[cfg(all(target_os = "android", feature = "winit-backend"))]
pub use winit::platform::android::activity::AndroidApp;

/// Copy APK assets to internal storage and switch cwd.
///
/// [`run_on_android`] called automatically before [`App`] construction; if user project needs earlier access
/// to APK resources, can also call itself.
#[cfg(target_os = "android")]
pub use android_assets::init_android_assets;

/// Read config from app asset directory; filesystem access belongs to app layer, parsing logic by engine
/// of `AppConfig::from_ini` provides.
///
/// Prefer `assets/settings.ini`; when absent fall back to read old `assets/settings.toml`
/// (and hint migration). Path resolved via `prism_engine::asset_paths` (cwd / executable
/// directory / parent dir / `PRISMA_ASSET_ROOT`), thus launching exe from any directory can read config.
pub fn load_config() -> AppConfig {
    let _span = prism_engine::util::timing::span("app::load_config");
    if prism_engine::asset_paths::strict_pak_only() {
        return AppConfig::default();
    }
    for (name, legacy) in [("settings.ini", false), ("settings.toml", true)] {
        let Some(path) = prism_engine::asset_paths::asset_path(name) else {
            return AppConfig::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                if legacy {
                    log::warn!(
                        "{} is deprecated: run `prism-build-pipeline build-scenes` to migrate \
                         it to settings.ini",
                        path.display()
                    );
                }
                return AppConfig::from_ini(&text);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                log::warn!("{name} ({}): {error} - using defaults", path.display());
                return AppConfig::default();
            }
        }
    }
    AppConfig::default()
}

/// Create app and complete engine init (config load, scene, runtime init).
///
/// User project registers ECS content after this (`add_system` / `insert_resource` /
/// `engine_mut`), then [`App::run`] (desktop) or [`App::run_on`] (Android).
/// Editor etc host can inject [`FrameHook`] via [`App::with_frame_hook`].
pub fn app() -> App {
    App::with_config(load_config())
}

/// Start app on Android (for user project `android_main` call).
///
/// User project passes a closure constructing [`App`], not already-constructed `App`:
/// This function first copies APK assets to internal storage, switches cwd, injects launch config,
/// then calls closure. Closure `[`app`]` can load `scenes.pak`, `settings.ini`, env texture
/// with `assets/...` paths same as desktop.
///
/// User project cdylib exports `#[no_mangle] fn android_main(...)`, called here,
/// this function, JNI boilerplate (android_logger, crash handler, AssetManager copy,
/// EventLoop construction) all handled by this function:
///
/// ```no_run
/// use prism_app::{app, run_on_android};
///
/// fn build_app() -> prism_app::App {
///     let mut app = app();
///     // ...register ECS content (add_system / insert_resource)...
///     app
/// }
///
/// #[cfg(target_os = "android")]
/// #[no_mangle]
/// fn android_main(android_app: prism_app::AndroidApp) {
///     run_on_android(build_app, android_app).expect("fatal application error");
/// }
/// ```
///
/// Equivalent to desktop [`App::run`], but uses externally passed winit EventLoop.
#[cfg(all(target_os = "android", feature = "winit-backend"))]
pub fn run_on_android<F>(
    build_app: F,
    android_app: winit::platform::android::activity::AndroidApp,
) -> anyhow::Result<()>
where
    F: FnOnce() -> App,
{
    let data_dir = android_assets::init_android_assets(&android_app)?;
    inject_launch_config(&data_dir);

    let level = prism_engine::launch_config::LaunchConfig::from_env()
        .log_level_filter()
        .unwrap_or(log::LevelFilter::Debug);
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(level)
            .with_tag("Prisma"),
    );

    log::info!("Prisma Android starting");

    crate::crash_dialog::register_android_app(&android_app);
    struct AndroidAppGuard;
    impl Drop for AndroidAppGuard {
        fn drop(&mut self) {
            crate::crash_dialog::unregister_android_app();
        }
    }
    let _app_guard = AndroidAppGuard;

    let app = build_app();
    let event_loop = prism_platform::build_android_event_loop(android_app)?;

    app.run_on(event_loop)
}

/// Bridge the hub's `filesDir/launch_config.json` into the same env var used
/// by desktop. On re-entry, always read the latest file (or clear the env
/// var if the file was removed) so stale launch configs do not persist across runs.
#[cfg(any(target_os = "android", test))]
fn inject_launch_config(data_dir: &std::path::Path) {
    let path = data_dir.join("launch_config.json");
    match std::fs::read_to_string(&path) {
        Ok(json) => std::env::set_var(prism_engine::launch_config::ENV_KEY, json),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::env::remove_var(prism_engine::launch_config::ENV_KEY);
        }
        Err(error) => log::warn!("read {}: {error}", path.display()),
    }
}

#[cfg(test)]
mod launch_config_reentrancy_tests {
    use super::*;

    #[test]
    fn inject_launch_config_updates_and_cleans_up_on_reentry() {
        let temp_dir = std::env::temp_dir().join(format!(
            "prism_test_launch_config_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let cfg_path = temp_dir.join("launch_config.json");

        std::fs::write(&cfg_path, r#"{"scene":"intro","log_level":"debug"}"#).unwrap();
        inject_launch_config(&temp_dir);
        assert_eq!(
            std::env::var(prism_engine::launch_config::ENV_KEY).as_deref(),
            Ok(r#"{"scene":"intro","log_level":"debug"}"#)
        );

        std::fs::write(&cfg_path, r#"{"scene":"gameplay","log_level":"warn"}"#).unwrap();
        inject_launch_config(&temp_dir);
        assert_eq!(
            std::env::var(prism_engine::launch_config::ENV_KEY).as_deref(),
            Ok(r#"{"scene":"gameplay","log_level":"warn"}"#)
        );

        std::fs::remove_file(&cfg_path).unwrap();
        inject_launch_config(&temp_dir);
        assert!(std::env::var(prism_engine::launch_config::ENV_KEY).is_err());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
