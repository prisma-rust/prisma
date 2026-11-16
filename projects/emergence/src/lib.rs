//! Emergence sandbox project entry crate.
//!
//! Shares the same v3.0 tag simulation state across both the static player
//! binary (`player` feature) and the editor dynamic module (`preview-module`
//! feature).

#![deny(warnings)]

pub mod tags;

use prism_game_api::GameWorld;

use crate::tags::{
    BioActivity, EnvPacked, FlowDir, MaterialFlags, MatterState, PhysicalCell, Pressure,
    SignalClockState, StimulusFlags, Temperature, TopologyFlags,
};

/// Minimal runtime state driving the initial v3.0 lab cells and clock divider.
pub struct SandboxState {
    accumulator: f32,
    clock_accumulator: f32,
    cells: Vec<PhysicalCell>,
}

impl Default for SandboxState {
    fn default() -> Self {
        Self::new()
    }
}

impl SandboxState {
    pub fn new() -> Self {
        let cryo_bio = PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Cryo,
                Pressure::Normal,
                5,
                BioActivity::Cryptosis,
                FlowDir::None,
            ),
            material: MaterialFlags::ORGANIC.union(MaterialFlags::FLAMMABLE),
            topology: TopologyFlags::EDIBLE,
            clocks: SignalClockState::new(120, 0, 0, 1),
            ..PhysicalCell::default()
        };
        let conduit_cell = PhysicalCell {
            env: EnvPacked::new(
                MatterState::Liquid,
                Temperature::Ambient,
                Pressure::Pressurized,
                3,
                BioActivity::Inert,
                FlowDir::E,
            ),
            material: MaterialFlags::METALLIC
                .union(MaterialFlags::AIRTIGHT)
                .union(MaterialFlags::PRESSURE_RATED),
            topology: TopologyFlags::CONDUIT.union(TopologyFlags::VALVE),
            clocks: SignalClockState::new(0, 0, 8, 5),
            ..PhysicalCell::default()
        };
        Self {
            accumulator: 0.0,
            clock_accumulator: 0.0,
            cells: vec![cryo_bio, conduit_cell],
        }
    }

    pub fn cells(&self) -> &[PhysicalCell] {
        &self.cells
    }

    pub fn inject_stimulus(&mut self, index: usize, stimulus: StimulusFlags) {
        if let Some(cell) = self.cells.get_mut(index) {
            cell.apply_stimulus(stimulus);
        }
    }

    pub fn step(&mut self, dt: f32) {
        const FIXED_STEP: f32 = 1.0 / 60.0;
        self.accumulator += dt.clamp(0.0, 0.1);
        while self.accumulator >= FIXED_STEP {
            self.accumulator -= FIXED_STEP;
            self.clock_accumulator += FIXED_STEP;
            let advance_clock = if self.clock_accumulator >= 1.0 {
                self.clock_accumulator -= 1.0;
                true
            } else {
                false
            };
            for cell in &mut self.cells {
                cell.step_reaction(advance_clock);
            }
        }
    }
}

pub fn sandbox_tick(state: &mut Option<SandboxState>, _world: &mut GameWorld<'_>, dt: f32) {
    let sandbox = state.get_or_insert_with(SandboxState::new);
    sandbox.step(dt);
}

#[cfg(feature = "player")]
pub fn build_app() -> prism_app::App {
    let mut app = prism_app::app()
        .with_subsystem(prism_app::Subsystem::Render)
        .with_subsystem(prism_app::Subsystem::Audio);
    app.add_game_system::<SandboxState, _>("emergence::sandbox_tick", sandbox_tick);
    app
}

#[cfg(feature = "preview-module")]
mod preview {
    use prism_game_api::GameWorld;

    use crate::{sandbox_tick, SandboxState};

    struct PreviewState {
        sandbox: Option<SandboxState>,
    }

    fn new_state() -> PreviewState {
        PreviewState { sandbox: None }
    }

    fn tick_state(state: &mut PreviewState, world: &mut GameWorld<'_>, dt: f32) {
        sandbox_tick(&mut state.sandbox, world, dt);
    }

    prism_game_api::export_project! {
        state = PreviewState,
        init = new_state,
        tick = tick_state,
        log_name = "emergence",
    }
}

#[cfg(all(target_os = "android", feature = "player"))]
#[no_mangle]
pub extern "C" fn android_main(android_app: prism_app::AndroidApp) {
    prism_app::run_on_android(build_app, android_app).expect("fatal application error");
}
