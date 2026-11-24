#![cfg(feature = "plugin")]
#![deny(warnings)]

use prism_game_api::__project_api::{ProjectApi, SUCCESS};

struct FixedState {
    fixed_ticks: u32,
}

fn init() -> FixedState {
    FixedState { fixed_ticks: 0 }
}

fn tick_state(state: &mut FixedState, _world: &mut prism_game_api::GameWorld<'_>, _dt: f32) {
    let _ = state.fixed_ticks;
}

fn fixed_state(state: &mut FixedState, _world: &mut prism_game_api::GameWorld<'_>, _dt: f32) {
    state.fixed_ticks += 1;
}

prism_game_api::export_project! {
    state = FixedState,
    init = init,
    tick = tick_state,
    fixed_update = fixed_state,
    log_name = "entry-fixed-test",
}

#[test]
fn export_project_with_fixed_update_wires_callback() {
    let mut api = std::mem::MaybeUninit::<ProjectApi>::uninit();
    // SAFETY: `get_api` writes a complete `ProjectApi` table to `api`.
    let code = unsafe { get_api(api.as_mut_ptr()) };
    assert_eq!(code, SUCCESS);
    // SAFETY: `get_api` returned `SUCCESS`.
    let api = unsafe { api.assume_init() };
    assert!(api.fixed_update.is_some());
}
