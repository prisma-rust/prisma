//! Emergence sandbox project entry crate.
//!
//! Shares the Procedural Map Gen, v3.0 Multi-Rate Simulation, Three-Tier Voxel
//! World (`L1 -> L2 -> L3`), and Player Controller across both the static player
//! binary (`player` feature) and the editor dynamic module (`preview-module`
//! feature).

#![deny(warnings)]

pub mod catalog;
pub mod grid;
pub mod materials;
pub mod nei_ui;
pub mod pbr_materials;
pub mod player;
pub mod sim;
pub mod structures;
pub mod tags;
pub mod terrain;
pub mod world_gen;

use prism_game_api::{keys, GameWorld, UiQuad};

use crate::grid::CellGrid;
use crate::materials::MaterialTable;
use crate::nei_ui::NeiHudState;
use crate::player::PlayerController;
use crate::sim::SimWorld;
use crate::tags::{
    BioActivity, EnvPacked, FlowDir, MaterialFlags, MatterState, PhysicalCell, Pressure,
    StimulusFlags, Temperature, TopologyFlags, TransientFlags,
};
use crate::terrain::{generate_terrain, BiomeKind};
use crate::world_gen::generate_world;

const GRID_WIDTH: usize = 64;
const GRID_HEIGHT: usize = 36;
const CELL_PX: f32 = 8.0;

/// Minimal runtime state driving the v3.0 lab grid, multi-rate dividers, and NEI HUD.
pub struct SandboxState {
    accumulator: f32,
    buoyancy_accumulator: f32,
    clock_accumulator: f32,
    probe_x: usize,
    probe_y: usize,
    prev_input_bits: u16,
    grid: CellGrid,
    biomes: Vec<BiomeKind>,
    nei: NeiHudState,
}

impl Default for SandboxState {
    fn default() -> Self {
        Self::new()
    }
}

impl SandboxState {
    pub fn new() -> Self {
        Self::with_seed(0x4D43_5F56_3330)
    }

    pub fn with_seed(seed: u64) -> Self {
        let generated = generate_terrain(GRID_WIDTH, GRID_HEIGHT, seed);
        let probe_x = GRID_WIDTH / 2;
        let probe_y = generated.sea_level.min(GRID_HEIGHT - 2);
        Self {
            accumulator: 0.0,
            buoyancy_accumulator: 0.0,
            clock_accumulator: 0.0,
            probe_x,
            probe_y,
            prev_input_bits: 0,
            grid: generated.grid,
            biomes: generated.biomes,
            nei: NeiHudState::new(),
        }
    }

    pub fn biomes(&self) -> &[BiomeKind] {
        &self.biomes
    }

    pub fn grid(&self) -> &CellGrid {
        &self.grid
    }

    pub fn nei(&self) -> &NeiHudState {
        &self.nei
    }

    pub fn inject_stimulus(&mut self, x: usize, y: usize, stimulus: StimulusFlags) {
        self.grid.apply_stimulus(x, y, stimulus);
    }

    pub fn step(&mut self, dt: f32) {
        const FIXED_STEP: f32 = 1.0 / 60.0;
        const BUOYANCY_STEP: f32 = 0.2;
        self.accumulator += dt.clamp(0.0, 0.1);
        while self.accumulator >= FIXED_STEP {
            self.accumulator -= FIXED_STEP;
            self.buoyancy_accumulator += FIXED_STEP;
            self.clock_accumulator += FIXED_STEP;

            let advance_clock = if self.clock_accumulator >= 1.0 {
                self.clock_accumulator -= 1.0;
                true
            } else {
                false
            };

            self.grid.step_reaction(advance_clock);

            if self.buoyancy_accumulator >= BUOYANCY_STEP {
                self.buoyancy_accumulator -= BUOYANCY_STEP;
                self.grid.step_buoyancy();
            }
        }
    }

    fn handle_input(&mut self, world: &GameWorld<'_>) {
        let input = world.input();
        let screen = world.screen_size();

        let mut bits = 0u16;
        if input.key_pressed(keys::KEY_A) || input.key_pressed(keys::ARROW_LEFT) {
            bits |= 1 << 0;
        }
        if input.key_pressed(keys::KEY_D) || input.key_pressed(keys::ARROW_RIGHT) {
            bits |= 1 << 1;
        }
        if input.key_pressed(keys::KEY_W) || input.key_pressed(keys::ARROW_UP) {
            bits |= 1 << 2;
        }
        if input.key_pressed(keys::KEY_S) || input.key_pressed(keys::ARROW_DOWN) {
            bits |= 1 << 3;
        }
        if input.key_pressed(keys::F1) {
            bits |= 1 << 4;
        }
        if input.key_pressed(keys::F2) {
            bits |= 1 << 5;
        }
        if input.key_pressed(keys::F3) {
            bits |= 1 << 6;
        }
        if input.key_pressed(keys::F4) {
            bits |= 1 << 7;
        }
        if input.key_pressed(keys::F5) {
            bits |= 1 << 8;
        }
        if input.key_pressed(keys::F6) {
            bits |= 1 << 9;
        }
        if input.key_pressed(keys::F7) {
            bits |= 1 << 10;
        }
        if input.key_pressed(keys::F8) {
            bits |= 1 << 11;
        }

        let pressed = bits & !self.prev_input_bits;
        self.prev_input_bits = bits;

        if (pressed & (1 << 0)) != 0 && self.probe_x > 0 {
            self.probe_x -= 1;
        }
        if (pressed & (1 << 1)) != 0 && self.probe_x + 1 < GRID_WIDTH {
            self.probe_x += 1;
        }
        if (pressed & (1 << 2)) != 0 && self.probe_y > 0 {
            self.probe_y -= 1;
        }
        if (pressed & (1 << 3)) != 0 && self.probe_y + 1 < GRID_HEIGHT {
            self.probe_y += 1;
        }

        let actions = self.nei.update_input(
            &input,
            screen,
            (GRID_WIDTH, GRID_HEIGHT),
            CELL_PX,
            &mut self.probe_x,
            &mut self.probe_y,
        );
        if let Some((gx, gy, cell)) = actions.place_at {
            self.grid.set_cell(gx, gy, cell);
        } else if let Some((gx, gy)) = actions.erase_at {
            let vacuum = PhysicalCell {
                env: EnvPacked::new(
                    MatterState::Gas,
                    Temperature::Ambient,
                    Pressure::HardVacuum,
                    0,
                    BioActivity::Inert,
                    FlowDir::None,
                ),
                transient: TransientFlags::DORMANT,
                ..PhysicalCell::default()
            };
            self.grid.set_cell(gx, gy, vacuum);
        }

        let mut stimulus = StimulusFlags::NONE;
        if (pressed & (1 << 4)) != 0 {
            stimulus.insert(StimulusFlags::HEAT);
        }
        if (pressed & (1 << 5)) != 0 {
            stimulus.insert(StimulusFlags::COLD);
        }
        if (pressed & (1 << 6)) != 0 {
            stimulus.insert(StimulusFlags::SHOCK);
        }
        if (pressed & (1 << 7)) != 0 {
            stimulus.insert(StimulusFlags::ACID);
        }
        if (pressed & (1 << 8)) != 0 {
            stimulus.insert(StimulusFlags::KINETIC);
        }
        if (pressed & (1 << 9)) != 0 {
            stimulus.insert(StimulusFlags::BIO);
        }
        if (pressed & (1 << 10)) != 0 {
            stimulus.insert(StimulusFlags::RADIATION);
        }
        if (pressed & (1 << 11)) != 0 {
            stimulus.insert(StimulusFlags::LIGHT);
        }
        if !stimulus.is_empty() {
            self.inject_stimulus(self.probe_x, self.probe_y, stimulus);
        }
    }

    fn draw_grid(&self, world: &mut GameWorld<'_>) {
        for y in 0..GRID_HEIGHT {
            for x in 0..GRID_WIDTH {
                let Some(cell) = self.grid.get(x, y) else {
                    continue;
                };
                if cell.env.pressure() == Pressure::HardVacuum
                    && cell.material.is_empty()
                    && cell.transient.contains(TransientFlags::DORMANT)
                {
                    continue;
                }
                let color = cell_color(cell);
                world.draw_quad(UiQuad {
                    rect: [
                        x as f32 * CELL_PX,
                        y as f32 * CELL_PX,
                        CELL_PX - 1.0,
                        CELL_PX - 1.0,
                    ],
                    color,
                    border_radius: 0.0,
                    layer: 1,
                });
            }
        }

        // Draw probe selection outline.
        world.draw_quad(UiQuad {
            rect: [
                self.probe_x as f32 * CELL_PX,
                self.probe_y as f32 * CELL_PX,
                CELL_PX,
                CELL_PX,
            ],
            color: [1.0, 1.0, 1.0, 0.35],
            border_radius: 0.0,
            layer: 2,
        });
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
        sim.seed = seed;
        sim.attach_ecs(world);
        let mut player = PlayerController::new();
        player.attach_ecs(world, &sim);
        sim.stream_around(player.feet_pos, Some(world), 8);
        Self { sim, player }
    }
}

/// Fixed simulation system (`60 Hz`): physics kinematics + multi-rate v3.0 cellular ticks + chunk streaming.
pub fn emergence_fixed_tick(
    state: &mut Option<EmergenceRuntime>,
    world: &mut GameWorld<'_>,
    dt: f32,
) {
    let runtime = state.get_or_insert_with(|| EmergenceRuntime::bootstrap(world, 2026));
    runtime.player.step_physics(world, &runtime.sim, dt);
    runtime
        .sim
        .stream_around(runtime.player.feet_pos, Some(world), 1);
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

fn cell_color(cell: &PhysicalCell) -> [f32; 4] {
    if cell.transient.contains(TransientFlags::BURNING) {
        return [1.0, 0.35, 0.1, 1.0];
    }
    if cell.transient.contains(TransientFlags::WARNING) {
        return [1.0, 0.8, 0.1, 1.0];
    }
    if cell.transient.contains(TransientFlags::CHARGED) {
        return [0.3, 0.85, 1.0, 1.0];
    }
    if cell.transient.contains(TransientFlags::FROZEN) {
        return [0.65, 0.9, 1.0, 1.0];
    }
    if cell.transient.contains(TransientFlags::INFECTED)
        || cell.env.bio_activity() == BioActivity::HyperNeo
    {
        return [0.45, 0.95, 0.3, 1.0];
    }
    if cell.transient.contains(TransientFlags::BROKEN) {
        return [0.35, 0.25, 0.25, 0.85];
    }
    // Distinct geological & ore palette for terrain and biome slices.
    if cell.topology.contains(TopologyFlags::SEALED)
        && cell.env.pressure() == Pressure::GigaPressure
    {
        return [0.16, 0.16, 0.19, 1.0]; // bedrock
    }
    if cell.material.contains(MaterialFlags::GLASS)
        && cell.material.contains(MaterialFlags::PRESSURE_RATED)
        && cell.env.pressure() == Pressure::GigaPressure
    {
        return [0.35, 0.88, 0.95, 1.0]; // diamond_ore
    }
    if cell.material.contains(MaterialFlags::PIEZOELECTRIC)
        && cell.material.contains(MaterialFlags::CONDUCTIVE)
    {
        return [0.85, 0.16, 0.14, 1.0]; // redstone_ore
    }
    if cell.material.contains(MaterialFlags::METALLIC)
        && cell.material.contains(MaterialFlags::HOT_RESISTANT)
    {
        return [0.95, 0.78, 0.26, 1.0]; // gold_ore
    }
    if cell.material.contains(MaterialFlags::FLAMMABLE)
        && cell.topology.contains(TopologyFlags::RIGID_WELD)
    {
        return [0.18, 0.19, 0.21, 1.0]; // coal_ore
    }
    if cell.material.contains(MaterialFlags::METALLIC)
        && cell.material.contains(MaterialFlags::CONDUCTIVE)
        && cell.topology.contains(TopologyFlags::RIGID_WELD)
    {
        return [0.66, 0.56, 0.49, 1.0]; // iron/copper ore
    }
    if cell.env.matter() == MatterState::Liquid && cell.env.temperature() >= Temperature::PlasmaHeat
    {
        return [1.0, 0.32, 0.05, 1.0]; // magma_lava
    }
    if cell.material.contains(MaterialFlags::GRANULAR) {
        if cell.material.contains(MaterialFlags::GLASS) {
            return [0.86, 0.79, 0.56, 1.0]; // sand
        }
        if cell.env.temperature() <= Temperature::Cryo {
            return [0.94, 0.96, 1.0, 1.0]; // snow_block
        }
    }
    if cell.material.contains(MaterialFlags::ORGANIC)
        && cell.env.bio_activity() == BioActivity::Vital
    {
        return [0.32, 0.65, 0.24, 1.0]; // grass_block
    }
    if cell.material.contains(MaterialFlags::GLASS) && cell.env.temperature() <= Temperature::Cryo {
        return [0.62, 0.84, 0.98, 0.85]; // ice_block
    }
    if cell.material.contains(MaterialFlags::ABSORBENT) && cell.env.matter() == MatterState::Solid {
        return [0.48, 0.33, 0.22, 1.0]; // dirt
    }
    if cell.material.contains(MaterialFlags::HOT_RESISTANT)
        && cell.material.contains(MaterialFlags::PRESSURE_RATED)
        && cell.env.matter() == MatterState::Solid
    {
        return [0.26, 0.27, 0.30, 1.0]; // deepslate
    }
    if cell.material.contains(MaterialFlags::HOT_RESISTANT)
        && cell.env.matter() == MatterState::Solid
    {
        return [0.50, 0.52, 0.54, 1.0]; // stone
    }
    match cell.env.matter() {
        MatterState::Solid => [0.55, 0.6, 0.68, 1.0],
        MatterState::Liquid => [0.2, 0.45, 0.85, 0.9],
        MatterState::Gas => [0.7, 0.75, 0.8, 0.4],
        MatterState::Plasma => [0.95, 0.4, 0.95, 0.9],
    }
}

pub fn sandbox_tick(state: &mut Option<SandboxState>, world: &mut GameWorld<'_>, dt: f32) {
    let sandbox = state.get_or_insert_with(SandboxState::new);
    sandbox.handle_input(world);
    sandbox.step(dt);
    sandbox.draw_grid(world);
    sandbox
        .nei
        .render(world, &sandbox.grid, sandbox.probe_x, sandbox.probe_y);
}

#[cfg(feature = "player")]
pub fn build_app() -> prism_app::App {
    let mut app = prism_app::app()
        .with_subsystem(prism_app::Subsystem::Render)
        .with_subsystem(prism_app::Subsystem::Audio);
    app.add_fixed_game_system::<EmergenceRuntime, _>("emergence::fixed_tick", emergence_fixed_tick);
    app.add_game_system::<EmergenceRuntime, _>("emergence::frame_tick", emergence_frame_tick);
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
