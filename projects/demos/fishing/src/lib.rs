//! Cozy Fishing library target.
//!
//! Two forms (`[features]` in `Cargo.toml`):
//!
//! - **player** (default): desktop side calls [`build_app`] from `src/main.rs`; Android side
//!   exports `android_main` JNI entry (produces `libfishing.so`). This form links engine.
//! - **preview-module**: dynamic module loaded by editor Play. Only depends on `prism-scene` /
//!   `prism-game-api` (plugin backend), **does not link engine**, reads/writes host world across C ABI.
//!
//! Gameplay code (`game::*`) shared by both forms: `tick(&mut state, &mut GameWorld, dt)`
//! gets a zero-copy borrow of `GameWorld` in host, ABI calls inside module.

pub mod game;

/// Build app: engine loads lake scene from `assets/scenes.pak`, this project registers gameplay systems.
#[cfg(feature = "player")]
pub fn build_app() -> prism_app::App {
    let mut app = prism_app::app()
        .with_subsystem(prism_app::Subsystem::Render)
        .with_subsystem(prism_app::Subsystem::Audio);

    app.add_game_system::<game::world::LakeWorld, _>("game::world", game::world::tick)
        .add_game_system::<game::boat::BoatState, _>("game::boat", game::boat::tick)
        .add_game_system::<game::fishing::FishingState, _>("game::fishing", game::fishing::tick);
    app
}

#[cfg(feature = "preview-module")]
mod preview {
    use prism_game_api::GameWorld;

    use crate::game;

    /// Every piece of gameplay state the module owns. It never reaches the host
    /// world: the host only holds a `*mut c_void`. The player form hands the same
    /// three types to `add_game_system`, so both forms tick identical state.
    struct PreviewState {
        world: Option<game::world::LakeWorld>,
        fishing: Option<game::fishing::FishingState>,
        boat: Option<game::boat::BoatState>,
    }

    fn new_state() -> PreviewState {
        PreviewState {
            world: None,
            fishing: None,
            boat: None,
        }
    }

    fn tick_state(state: &mut PreviewState, world: &mut GameWorld<'_>, dt: f32) {
        let first_tick = state.world.is_none();
        game::world::tick(&mut state.world, world, dt);
        game::boat::tick(&mut state.boat, world, dt);
        game::fishing::tick(&mut state.fishing, world, dt);
        if first_tick {
            let layers = state
                .world
                .as_ref()
                .map(|lake| lake.layer_count())
                .unwrap_or(0);
            log::info!("fishing: lake world rebuilt, {layers} layers");
        }
    }

    prism_game_api::export_project! {
        state = PreviewState,
        init = new_state,
        tick = tick_state,
        log_name = "fishing",
    }
}

/// Entry called after GameActivity loads this library.
///
/// GameActivity's native glue resolves this symbol via `dlsym`, so it must keep
/// C linkage and an unmangled name; a plain Rust `fn` is not exported.
#[cfg(all(target_os = "android", feature = "player"))]
#[no_mangle]
pub extern "C" fn android_main(android_app: prism_app::AndroidApp) {
    prism_app::run_on_android(build_app, android_app).expect("fatal application error");
}
