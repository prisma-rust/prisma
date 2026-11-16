//! Emergence sandbox project entry crate.
//!
//! Shares the Procedural Map Gen, v3.0 Multi-Rate Simulation, Three-Tier Voxel
//! World (`L1 -> L2 -> L3`), and Player Controller across both the static player
//! binary (`player` feature) and the editor dynamic module (`preview-module`
//! feature).

#![deny(warnings)]

pub mod materials;
pub mod player;
pub mod sim;
pub mod tags;
pub mod world_gen;

use prism_game_api::GameWorld;

use crate::materials::MaterialTable;
use crate::player::PlayerController;
use crate::sim::SimWorld;
use crate::tags::{
    BioActivity, EnvPacked, FlowDir, MaterialFlags, MatterState, PhysicalCell, Pressure,
    SignalClockState, StimulusFlags, Temperature, TopologyFlags,
};
use crate::world_gen::generate_world;

/// Minimal runtime state driving standalone unit verification of `PhysicalCell`.
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

/// Complete Emergence MVP runtime holding map geometry, physics world, and player state.
pub struct EmergenceRuntime {
    pub sim: SimWorld,
    pub player: PlayerController,
}

impl EmergenceRuntime {
    pub fn bootstrap(world: &mut GameWorld<'_>, seed: u64) -> Self {
        let materials = MaterialTable::load(Some(world));
        let gen = generate_world(&materials, seed);
        let mut sim = SimWorld::new(materials, gen.chunks, gen.cells, gen.active);
        sim.attach_ecs(world);
        let mut player = PlayerController::new();
        player.attach_ecs(world, &sim);
        Self { sim, player }
    }
}

/// Fixed simulation system (`60 Hz`): physics kinematics + multi-rate v3.0 cellular ticks + dirty chunk sync.
pub fn emergence_fixed_tick(
    state: &mut Option<EmergenceRuntime>,
    world: &mut GameWorld<'_>,
    dt: f32,
) {
    let runtime = state.get_or_insert_with(|| EmergenceRuntime::bootstrap(world, 2026));
    runtime.player.step_physics(world, &runtime.sim, dt);
    runtime.sim.step(dt);
    runtime.sim.sync_dirty_chunks(world);
}

/// Per-frame interaction & camera look system: responsive mouse aim, L1/L2/L3 carve/place, HUD.
pub fn emergence_frame_tick(
    state: &mut Option<EmergenceRuntime>,
    world: &mut GameWorld<'_>,
    dt: f32,
) {
    let runtime = state.get_or_insert_with(|| EmergenceRuntime::bootstrap(world, 2026));
    runtime
        .player
        .update_look_and_tools(world, &mut runtime.sim, dt);
    runtime.sim.sync_dirty_chunks(world);
}

#[cfg(feature = "player")]
pub fn build_app() -> prism_app::App {
    let mut app = prism_app::app()
        .with_subsystem(prism_app::Subsystem::Render)
        .with_subsystem(prism_app::Subsystem::Audio);
    app.add_fixed_game_system::<EmergenceRuntime, _>(
        "emergence::fixed_tick",
        emergence_fixed_tick,
    );
    app.add_game_system::<EmergenceRuntime, _>(
        "emergence::frame_tick",
        emergence_frame_tick,
    );
    app
}

#[cfg(feature = "preview-module")]
mod preview {
    use prism_game_api::GameWorld;

    use crate::{emergence_fixed_tick, emergence_frame_tick, EmergenceRuntime};

    struct PreviewState {
        runtime: Option<EmergenceRuntime>,
        fixed_accum: f32,
    }

    fn new_state() -> PreviewState {
        PreviewState {
            runtime: None,
            fixed_accum: 0.0,
        }
    }

    fn tick_state(state: &mut PreviewState, world: &mut GameWorld<'_>, dt: f32) {
        const FIXED_DT: f32 = 1.0 / 60.0;
        emergence_frame_tick(&mut state.runtime, world, dt);
        state.fixed_accum += dt.clamp(0.0, 0.1);
        while state.fixed_accum >= FIXED_DT {
            state.fixed_accum -= FIXED_DT;
            emergence_fixed_tick(&mut state.runtime, world, FIXED_DT);
        }
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
