//! Emergence sandbox project entry crate.
//!
//! Shares the Procedural Map Gen, v3.0 Multi-Rate Simulation, Three-Tier Voxel
//! World (`L1 -> L2 -> L3`), and Player Controller across both the static player
//! binary (`player` feature) and the editor dynamic module (`preview-module`
//! feature).

#![deny(warnings)]

pub mod catalog;
pub mod commands;
pub mod debug_hud;
pub mod grid;
pub mod log_persist;
pub mod materials;
pub mod nei_ui;
pub mod pbr_materials;
pub mod perf_report;
pub mod player;
pub mod render_settings_ui;
pub mod sim;
pub mod structures;
pub mod tags;
pub mod terrain;
pub mod world_gen;

use prism_game_api::{keys, GameWorld, UiQuad};

use crate::commands::CommandWorldState;
use crate::debug_hud::DebugHudState;
use crate::grid::CellGrid;
use crate::materials::MaterialTable;
use crate::nei_ui::{EmergenceNeiHud, NeiHudState};
use crate::perf_report::{EmergencePerfRecorder, FrameTelemetryInput};
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
    pub commands: CommandWorldState,
    pub debug_hud: DebugHudState,
    pub nei: EmergenceNeiHud,
    pub render_settings_ui: render_settings_ui::RenderSettingsUi,
    pub perf: EmergencePerfRecorder,
    pub pending_dirty_chunks: usize,
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
        Self {
            sim,
            player,
            commands: CommandWorldState::new(),
            debug_hud: DebugHudState::new(seed),
            nei: EmergenceNeiHud::new(),
            render_settings_ui: render_settings_ui::RenderSettingsUi::default(),
            perf: EmergencePerfRecorder::from_active_config(seed),
            pending_dirty_chunks: 0,
        }
    }
}

/// Fixed simulation system (`60 Hz`): physics kinematics + daylight/weather + chunk streaming.
pub fn emergence_fixed_tick(
    state: &mut Option<EmergenceRuntime>,
    world: &mut GameWorld<'_>,
    dt: f32,
) {
    let runtime = state.get_or_insert_with(|| EmergenceRuntime::bootstrap(world, 2026));
    let panel_open =
        runtime.commands.console_open || runtime.render_settings_ui.open || runtime.nei.open;
    runtime
        .player
        .step_physics_with_input(world, &runtime.sim, dt, !panel_open);
    runtime
        .commands
        .step(dt, world, &mut runtime.sim, runtime.player.feet_pos);
    runtime
        .sim
        .stream_around(runtime.player.feet_pos, Some(world), 1);
    let sim_t0 = std::time::Instant::now();
    runtime.sim.step(dt);
    let sim_ms = sim_t0.elapsed().as_secs_f32() * 1000.0;
    runtime.debug_hud.record_sim_step_ms(sim_ms);
    runtime.perf.record_fixed_step(sim_ms);
    runtime.pending_dirty_chunks += runtime.sim.sync_dirty_chunks(world);
}

/// Per-frame interaction & camera look system: command console, F3 overlay, mouse aim, voxel tools.
pub fn emergence_frame_tick(
    state: &mut Option<EmergenceRuntime>,
    world: &mut GameWorld<'_>,
    dt: f32,
) {
    let logic_t0 = std::time::Instant::now();
    let runtime = state.get_or_insert_with(|| EmergenceRuntime::bootstrap(world, 2026));
    runtime.debug_hud.update_telemetry(dt);

    let input = world.input();
    let screen = world.screen_size();
    let console_active =
        runtime
            .commands
            .handle_console_input(&input, world, &mut runtime.sim, &mut runtime.player);
    if !console_active {
        if input.key_pressed(keys::F3) {
            runtime.debug_hud.open = !runtime.debug_hud.open;
        }
        let settings_consumed =
            runtime
                .render_settings_ui
                .handle_input(&input, screen, console_active);
        let ui_consumed = settings_consumed
            || runtime
                .nei
                .handle_input(&input, screen, console_active, &mut runtime.player)
            || runtime.render_settings_ui.open
            || runtime.nei.open;
        runtime
            .player
            .update_look_and_tools_with_ui(world, &mut runtime.sim, dt, ui_consumed);
    }

    runtime.commands.sync_chat_ui(world, dt);
    runtime
        .nei
        .sync_ui(world, &runtime.sim, &runtime.player, console_active);
    runtime.render_settings_ui.sync_ui(world, console_active);
    runtime
        .debug_hud
        .sync_ui(world, &runtime.sim, &runtime.player, &runtime.commands);
    let dirty_chunks = runtime.pending_dirty_chunks + runtime.sim.sync_dirty_chunks(world);
    runtime.pending_dirty_chunks = 0;

    if runtime.perf.is_enabled() {
        let logic_tick_ms = logic_t0.elapsed().as_secs_f32() * 1000.0;
        let gamemode = runtime.commands.gamemode_label(&runtime.player);
        let weather = runtime.commands.weather.label();
        runtime.perf.record_frame(FrameTelemetryInput {
            dt,
            logic_tick_ms,
            chunks_loaded: runtime.sim.chunks.len(),
            active_cells: runtime.sim.active_count(),
            tracked_cells: runtime.sim.tracked_cell_count(),
            dirty_chunks,
            screen_logical: screen,
            alloc_rate_mib_s: runtime.debug_hud.alloc_rate_mib_s(),
            player_pos: runtime.player.feet_pos.to_array(),
            gamemode,
            weather,
            world_time_ticks: runtime.commands.time_ticks,
            rule_daylight: runtime.commands.rules.do_daylight_cycle,
            rule_weather: runtime.commands.rules.do_weather_cycle,
            rule_fire: runtime.commands.rules.do_fire_tick,
        });
    }
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
#[derive(Default)]
struct EmergenceHostHook;

#[cfg(feature = "player")]
impl prism_app::FrameHook for EmergenceHostHook {
    fn on_tick(
        &mut self,
        window: &prism_app::PlatformWindow,
        _world: &mut prism_app::World,
        settings: &mut prism_app::RenderSettings,
        stats: &prism_app::RenderStats,
        _shared: &prism_app::RenderShared,
    ) {
        let (mode_id, scale, sharpness) = commands::current_upscaler_config();
        let mode = match mode_id {
            1 => prism_app::MobileUpscaler::Sgsr2,
            2 => prism_app::MobileUpscaler::ArmAsr,
            3 => prism_app::MobileUpscaler::Nearest,
            4 => prism_app::MobileUpscaler::Linear,
            5 => prism_app::MobileUpscaler::Dlss,
            6 => prism_app::MobileUpscaler::Fsr,
            _ => prism_app::MobileUpscaler::Off,
        };
        settings.mobile_upscaler = mode;
        settings.upscaler_sharpness = sharpness;
        settings.dlss_mode = if mode == prism_app::MobileUpscaler::Dlss {
            if scale >= 0.99 {
                prism_engine::render_settings::DlssMode::Dlaa
            } else if scale >= 0.665 {
                prism_engine::render_settings::DlssMode::Quality
            } else if scale >= 0.575 {
                prism_engine::render_settings::DlssMode::Balanced
            } else if scale >= 0.495 {
                prism_engine::render_settings::DlssMode::Performance
            } else {
                prism_engine::render_settings::DlssMode::UltraPerformance
            }
        } else {
            prism_engine::render_settings::DlssMode::Off
        };
        let (dlfg_id, ddgi_on) = commands::current_rtx_toggles();
        settings.dlfg_mode = match dlfg_id {
            1 => prism_engine::render_settings::DlssgMode::On,
            2 => prism_engine::render_settings::DlssgMode::Auto,
            _ => prism_engine::render_settings::DlssgMode::Off,
        };
        settings.dlss_pre_upscale = match commands::current_dlss_pre_upscale() {
            1 => prism_engine::render_settings::DlssPreUpscaleMode::Nearest,
            2 => prism_engine::render_settings::DlssPreUpscaleMode::Off,
            _ => prism_engine::render_settings::DlssPreUpscaleMode::Linear,
        };
        if ddgi_on {
            settings.raster_gi = prism_engine::render_settings::RasterGiMode::Ddgi;
        }
        if !mode.uses_internal_resolution() {
            settings.internal_resolution = [0, 0];
        } else {
            let effective_scale = if settings.dlss_mode
                == prism_engine::render_settings::DlssMode::UltraPerformance
                && settings.dlss_pre_upscale
                    == prism_engine::render_settings::DlssPreUpscaleMode::Off
            {
                1.0 / 3.0
            } else {
                scale
            };
            let (w, h) = window.inner_size();
            let rw = ((w.max(1) as f32) * effective_scale)
                .round()
                .max(16.0) as u32;
            let rh = ((h.max(1) as f32) * effective_scale)
                .round()
                .max(16.0) as u32;
            settings.internal_resolution = [rw, rh];
        }
        let all_gpu_passes = stats
            .gpu_timings
            .as_ref()
            .filter(|g| g.supported && !g.passes.is_empty())
            .map(|g| {
                let mut passes: Vec<(String, f32)> = g
                    .passes
                    .iter()
                    .map(|p| (p.name.to_string(), p.ms))
                    .collect();
                passes.sort_by(|a, b| b.1.total_cmp(&a.1));
                passes
            })
            .unwrap_or_default();
        let top_gpu_passes = all_gpu_passes.iter().take(2).cloned().collect();

        debug_hud::publish_host_render_stats(debug_hud::HostRenderStats {
            render_seq: stats.render_seq,
            fps: stats.fps,
            frame_time_ms: stats.frame_time_ms,
            gpu_frame_ms: stats
                .gpu_timings
                .as_ref()
                .filter(|g| g.supported)
                .map(|g| g.frame_ms),
            gpu_total_ms: stats
                .gpu_timings
                .as_ref()
                .filter(|g| g.supported)
                .map(|g| g.total_ms),
            top_gpu_passes,
            all_gpu_passes,
            cpu_p95_ms: stats.frame_stats.cpu.as_ref().map(|c| c.p95_ms),
            low_1pct_fps: stats.frame_stats.cpu.as_ref().map(|c| c.low_1pct_fps),
            physical_size: window.inner_size(),
            scale_factor: window.scale_factor(),
        });
    }
}

/// Builds the full playable Emergence application (`player` feature).
#[cfg(feature = "player")]
pub fn build_app() -> prism_app::App {
    commands::init_upscaler_from_env();
    let mut app = prism_app::app()
        .with_subsystem(prism_app::Subsystem::Render)
        .with_subsystem(prism_app::Subsystem::Audio)
        .with_frame_hook(EmergenceHostHook);
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

#[cfg(test)]
mod runtime_tests {
    use super::*;
    use crate::player::TierMode;
    use prism_game_api::{GameWorld, InputSnapshot};

    fn press_key_snapshot(key_idx: usize) -> InputSnapshot {
        let mut input = InputSnapshot::default();
        input.keys[key_idx / 64] |= 1u64 << (key_idx % 64);
        input.key_edges[key_idx / 64] |= 1u64 << (key_idx % 64);
        input
    }

    #[test]
    fn frame_tick_gates_f3_while_chat_is_open_and_frees_t_from_tier_toggle() {
        let mut ecs = prism_ecs::World::new();
        let mut world = GameWorld::host(&mut ecs);
        let mut state: Option<EmergenceRuntime> = None;

        // Bootstrap starts with F3 overlay and chat console hidden.
        world.set_input(InputSnapshot::default());
        emergence_frame_tick(&mut state, &mut world, 1.0 / 60.0);
        {
            let rt = state.as_ref().expect("bootstrapped");
            assert!(!rt.debug_hud.open);
            assert!(!rt.commands.console_open);
            assert_eq!(rt.player.tier_mode, TierMode::Auto);
        }

        // Pressing `T` opens chat console and does not flip player TierMode.
        world.set_input(press_key_snapshot(keys::KEY_T));
        emergence_frame_tick(&mut state, &mut world, 1.0 / 60.0);
        {
            let rt = state.as_ref().unwrap();
            assert!(rt.commands.console_open);
            assert_eq!(rt.player.tier_mode, TierMode::Auto);
        }

        // Pressing `F3` while typing in chat must not toggle the debug overlay.
        world.set_input(press_key_snapshot(keys::F3));
        emergence_frame_tick(&mut state, &mut world, 1.0 / 60.0);
        assert!(!state.as_ref().unwrap().debug_hud.open);

        // Close chat with `Escape`, then press `F3` to toggle overlay on and back off.
        world.set_input(press_key_snapshot(keys::ESCAPE));
        emergence_frame_tick(&mut state, &mut world, 1.0 / 60.0);
        assert!(!state.as_ref().unwrap().commands.console_open);

        world.set_input(press_key_snapshot(keys::F3));
        emergence_frame_tick(&mut state, &mut world, 1.0 / 60.0);
        assert!(state.as_ref().unwrap().debug_hud.open);

        world.set_input(press_key_snapshot(keys::F3));
        emergence_frame_tick(&mut state, &mut world, 1.0 / 60.0);
        assert!(!state.as_ref().unwrap().debug_hud.open);

        // Top-center tooltip activates when aiming at blocks and [B] toggles NEI only when chat is closed.
        {
            let rt = state.as_ref().unwrap();
            assert!(rt.nei.open);
            assert!(rt.nei.build_top_tooltip(&rt.sim, &rt.player).is_some());
        }

        world.set_input(press_key_snapshot(keys::OTHER_BASE + b'B' as usize));
        emergence_frame_tick(&mut state, &mut world, 1.0 / 60.0);
        assert!(!state.as_ref().unwrap().nei.open);

        world.set_input(press_key_snapshot(keys::KEY_T));
        emergence_frame_tick(&mut state, &mut world, 1.0 / 60.0);
        assert!(state.as_ref().unwrap().commands.console_open);

        world.set_input(press_key_snapshot(keys::OTHER_BASE + b'B' as usize));
        emergence_frame_tick(&mut state, &mut world, 1.0 / 60.0);
        assert!(!state.as_ref().unwrap().nei.open);

        world.set_input(press_key_snapshot(keys::ESCAPE));
        emergence_frame_tick(&mut state, &mut world, 1.0 / 60.0);
        assert!(!state.as_ref().unwrap().commands.console_open);

        world.set_input(press_key_snapshot(keys::OTHER_BASE + b'B' as usize));
        emergence_frame_tick(&mut state, &mut world, 1.0 / 60.0);
        assert!(state.as_ref().unwrap().nei.open);
    }
}
