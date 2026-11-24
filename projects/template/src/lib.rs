//! Prisma user game project.
//!
//! Two build forms share the same gameplay code:
//!
//! - **player** (default): desktop binary `template` + Android `libtemplate_core.so`,
//!   directly links `prism-app` / `prism-engine`.
//! - **preview-module**: dynamic module loaded by editor, only depends on cross-ABI
//!   `prism-game-api` / `prism-project-api`, **does not link engine**.
//!
//! Project own types (`intro::IntroState`) always stay inside this crate: engine and editor
//! interact with project only via opaque handles and generic component schema.

pub mod intro;

#[cfg(all(feature = "player-core", not(debug_assertions)))]
mod embedded_config {
    include!(concat!(env!("OUT_DIR"), "/embedded_config.rs"));
}

/// Build app: engine loads intro scene entities from path manifest in `assets/scenes.pak`
/// (cooked from `assets/scenes/intro.scene.json`), this project only registers
/// `intro::tick_system` to drive animation.
#[cfg(feature = "player-core")]
pub fn build_app() -> prism_app::App {
    #[cfg(not(debug_assertions))]
    let mut app = prism_app::App::with_config(embedded_config::embedded_app_config())
        .with_subsystem(prism_app::Subsystem::Render);
    #[cfg(debug_assertions)]
    let mut app = prism_app::app().with_subsystem(prism_app::Subsystem::Render);
    app.add_game_system::<intro::IntroState, _>("intro::tick", intro::tick);
    app
}

#[cfg(feature = "preview-module")]
mod preview {
    use prism_game_api::GameWorld;

    use crate::intro::{self, IntroState};

    /// All state held by project module. It **will not** appear in host world: host only gets
    /// a `*mut c_void`.
    struct PreviewState {
        intro: Option<IntroState>,
    }

    fn new_state() -> PreviewState {
        PreviewState { intro: None }
    }

    fn tick_state(state: &mut PreviewState, world: &mut GameWorld<'_>, dt: f32) {
        intro::tick(&mut state.intro, world, dt);
    }

    prism_game_api::export_project! {
        state = PreviewState,
        init = new_state,
        tick = tick_state,
        log_name = "template",
    }
}

/// Entry called after GameActivity loads this library.
///
/// GameActivity's native glue resolves this symbol with `dlsym`, so it must
/// keep C linkage and an unmangled name; a plain Rust `fn` is not exported.
#[cfg(all(target_os = "android", feature = "player-core"))]
#[no_mangle]
pub extern "C" fn android_main(android_app: prism_app::AndroidApp) {
    prism_app::run_on_android(build_app, android_app).expect("fatal application error");
}
