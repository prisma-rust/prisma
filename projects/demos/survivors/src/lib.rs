//! Potato & Survivors — Vampire Survivors + Brotato style 2D roguelite demo.
//!
//! Built cleanly atop `prism-game-api` and `prism-scene` without any engine workarounds:
//! - Pure 2D sprite batching via data-driven JSON slicing (`assets/data/sprites.json`)
//!   and the engine's built-in 1x1 white texture (`"__builtin/white"`).
//! - Deterministic `fixed_step` simulation paired with per-frame HUD and shop UI.
//! - Supports both standalone `player` binary and editor `preview-module` hot-reload.

#![deny(warnings)]

pub mod combat;
pub mod config;
pub mod state;
pub mod ui_hud;

use prism_game_api::GameWorld;

use crate::config::GameConfig;
use crate::state::SurvivorsState;

fn ensure_state<'a>(
    slot: &'a mut Option<SurvivorsState>,
    world: &mut GameWorld<'_>,
) -> Option<&'a mut SurvivorsState> {
    if slot.is_none() {
        match GameConfig::load_from_world(world) {
            Ok(cfg) => {
                let mut st = SurvivorsState::new(cfg);
                combat::ensure_world_initialized(&mut st, world);
                *slot = Some(st);
            }
            Err(err) => {
                log::error!("survivors: failed to load config: {err}");
                return None;
            }
        }
    }
    slot.as_mut()
}

/// Fixed-timestep combat, weapon auto-targeting, enemy horde AI, pickups, and wave progression.
pub fn fixed_tick(slot: &mut Option<SurvivorsState>, world: &mut GameWorld<'_>, dt: f32) {
    if let Some(state) = ensure_state(slot, world) {
        combat::fixed_step(state, world, dt);
    }
}

/// Variable-timestep HUD rendering, floating damage numbers, world HP bars, and shop/upgrade UI.
pub fn frame_tick(slot: &mut Option<SurvivorsState>, world: &mut GameWorld<'_>, dt: f32) {
    if let Some(state) = ensure_state(slot, world) {
        combat::ensure_world_initialized(state, world);
        ui_hud::frame_step(state, world, dt);
    }
}

/// Construct the standalone runnable application (`player` feature).
#[cfg(feature = "player")]
pub fn build_app() -> prism_app::App {
    let mut app = prism_app::app()
        .with_subsystem(prism_app::Subsystem::Render)
        .with_subsystem(prism_app::Subsystem::Audio)
        .with_pointer_lock_policy(prism_app::PointerLockPolicy::Manual);

    app.add_fixed_game_system::<SurvivorsState, _>("survivors::fixed", fixed_tick)
        .add_game_system::<SurvivorsState, _>("survivors::frame", frame_tick);
    app
}

#[cfg(feature = "preview-module")]
mod preview {
    use crate::state::SurvivorsState;

    fn init_state() -> Option<SurvivorsState> {
        None
    }

    prism_game_api::export_project! {
        state = Option<SurvivorsState>,
        init = init_state,
        tick = crate::frame_tick,
        fixed_update = crate::fixed_tick,
        log_name = "survivors",
    }
}

/// Entry called after GameActivity loads this library on Android.
#[cfg(all(target_os = "android", feature = "player"))]
#[no_mangle]
pub extern "C" fn android_main(android_app: prism_app::AndroidApp) {
    prism_app::run_on_android(build_app, android_app).expect("fatal survivors application error");
}
