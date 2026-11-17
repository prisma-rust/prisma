//! Horizon-aligned F3 debug overlay and system telemetry for Emergence.
//!
//! Normally hidden during gameplay so only the crosshair and hotbar remain
//! visible; pressing `F3` toggles two-column Horizon-formatted status,
//! simulation, memory, system, and targeted-block inspection readouts.

use std::fmt::Write as _;
use std::sync::{Mutex, OnceLock};

use glam::Vec3;
use prism_game_api::{EntityHandle, GameWorld};
use prism_scene::ui::{
    Anchors, Canvas, CanvasScaleMode, Margin, Node, Pivot, Style, Text, TextAlign,
};
use prism_scene::{Active, Name};

use crate::catalog::{collect_cell_debug_tags, identify_cell};
use crate::commands::CommandWorldState;
use crate::player::PlayerController;
use crate::sim::{SimWorld, CHUNK_DIMS_L1};

/// Horizon `DebugScreenOverlay.BACKGROUND` (`0x9050_5050` in RGBA float).
pub const F3_BG_COLOR: [f32; 4] = [80.0 / 255.0, 80.0 / 255.0, 80.0 / 255.0, 144.0 / 255.0];
/// Horizon `CommonColors.TEXT_GRAY` / `0xFFE0_E0E0` text color.
pub const F3_TEXT_COLOR: [f32; 4] = [224.0 / 255.0, 224.0 / 255.0, 224.0 / 255.0, 1.0];

const F3_FONT_SIZE: f32 = 13.5;
const F3_ROW_HEIGHT: f32 = 18.0;
const F3_EDGE_PAD: f32 = 2.0;
const F3_TEXT_PAD_X: f32 = 4.0;
const CHUNK_DIM_Y_SECTION: i32 = 16;

/// Snapshot of real host renderer metrics populated by `FrameHook` in player builds.
#[derive(Debug, Clone, Default)]
pub struct HostRenderStats {
    pub fps: f32,
    pub frame_time_ms: f32,
    pub gpu_frame_ms: Option<f32>,
    pub gpu_total_ms: Option<f32>,
    pub top_gpu_passes: Vec<(String, f32)>,
    pub cpu_p95_ms: Option<f32>,
    pub low_1pct_fps: Option<f32>,
    pub physical_size: (u32, u32),
    pub scale_factor: f64,
}

static HOST_TELEMETRY: OnceLock<Mutex<Option<HostRenderStats>>> = OnceLock::new();

fn telemetry_slot() -> &'static Mutex<Option<HostRenderStats>> {
    HOST_TELEMETRY.get_or_init(|| Mutex::new(None))
}

/// Publishes host renderer stats from `FrameHook::on_tick` in `player` builds.
#[cfg(feature = "player")]
pub fn publish_host_render_stats(stats: HostRenderStats) {
    if let Ok(mut guard) = telemetry_slot().try_lock() {
        *guard = Some(stats);
    }
}

fn read_host_render_stats() -> Option<HostRenderStats> {
    telemetry_slot()
        .try_lock()
        .ok()
        .and_then(|guard| guard.clone())
}

/// Estimates Segoe UI / proportional pixel width for tight line background sizing.
pub(crate) fn estimate_text_width(text: &str, font_size: f32) -> f32 {
    let mut width = 0.0f32;
    for ch in text.chars() {
        let factor = match ch {
            ' ' | 'i' | 'l' | 'I' | 'j' | 't' | 'f' | 'r' | '.' | ',' | ':' | ';' | '!' | '|'
            | '\'' | '(' | ')' | '[' | ']' | '/' | '-' => 0.34,
            'W' | 'M' | 'w' | 'm' | '@' | '%' | '#' => 0.86,
            'A'..='Z' => 0.66,
            '0'..='9' => 0.56,
            _ if ch.is_ascii() => 0.55,
            _ => 0.95,
        };
        width += font_size * factor;
    }
    width.ceil()
}

/// Maps camera forward vector to Horizon `(cardinal, towards, yaw_deg, pitch_deg)`.
pub fn facing_cardinal_and_angles(fwd: Vec3) -> (&'static str, &'static str, f32, f32) {
    let horiz = Vec3::new(fwd.x, 0.0, fwd.z);
    let mut yaw_deg = if horiz.length_squared() > 1e-6 {
        (-horiz.x).atan2(horiz.z).to_degrees()
    } else {
        0.0
    };
    while yaw_deg >= 180.0 {
        yaw_deg -= 360.0;
    }
    while yaw_deg < -180.0 {
        yaw_deg += 360.0;
    }
    let pitch_deg = (-fwd.y.clamp(-1.0, 1.0).asin()).to_degrees();
    let quadrant = ((yaw_deg / 90.0) + 0.5).floor() as i32 & 3;
    let (dir, towards) = match quadrant {
        0 => ("south", "Towards positive Z"),
        1 => ("west", "Towards negative X"),
        2 => ("north", "Towards negative Z"),
        _ => ("east", "Towards positive X"),
    };
    (dir, towards, yaw_deg, pitch_deg)
}

#[derive(Debug, Clone, Copy, Default)]
struct ProcessMemoryStats {
    used_mib: u64,
    allocated_mib: u64,
    total_mib: u64,
}

#[cfg(target_os = "windows")]
fn read_process_memory() -> Option<ProcessMemoryStats> {
    #[repr(C)]
    struct MemoryStatusEx {
        dw_length: u32,
        dw_memory_load: u32,
        ull_total_phys: u64,
        ull_avail_phys: u64,
        ull_total_page_file: u64,
        ull_avail_page_file: u64,
        ull_total_virtual: u64,
        ull_avail_virtual: u64,
        ull_avail_extended_virtual: u64,
    }

    #[repr(C)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn GlobalMemoryStatusEx(lp_buffer: *mut MemoryStatusEx) -> i32;
        fn K32GetProcessMemoryInfo(
            process: isize,
            ppsmem_counters: *mut ProcessMemoryCounters,
            cb: u32,
        ) -> i32;
    }

    let mut mem_status = MemoryStatusEx {
        dw_length: std::mem::size_of::<MemoryStatusEx>() as u32,
        dw_memory_load: 0,
        ull_total_phys: 0,
        ull_avail_phys: 0,
        ull_total_page_file: 0,
        ull_avail_page_file: 0,
        ull_total_virtual: 0,
        ull_avail_virtual: 0,
        ull_avail_extended_virtual: 0,
    };
    let mut counters = ProcessMemoryCounters {
        cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
        page_fault_count: 0,
        peak_working_set_size: 0,
        working_set_size: 0,
        quota_peak_paged_pool_usage: 0,
        quota_paged_pool_usage: 0,
        quota_peak_non_paged_pool_usage: 0,
        quota_non_paged_pool_usage: 0,
        pagefile_usage: 0,
        peak_pagefile_usage: 0,
    };

    // SAFETY: Both structs have their exact byte-size headers initialized and valid writable pointers.
    let ok_sys = unsafe { GlobalMemoryStatusEx(&mut mem_status) } != 0;
    let ok_proc =
        unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) } != 0;
    if !ok_sys || !ok_proc {
        return None;
    }
    const MIB: u64 = 1024 * 1024;
    Some(ProcessMemoryStats {
        used_mib: (counters.working_set_size as u64) / MIB,
        allocated_mib: (counters.pagefile_usage as u64) / MIB,
        total_mib: (mem_status.ull_total_phys / MIB).max(1),
    })
}

#[cfg(target_os = "linux")]
fn read_process_memory() -> Option<ProcessMemoryStats> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let parse_kb = |text: &str, key: &str| -> u64 {
        text.lines()
            .find(|line| line.starts_with(key))
            .and_then(|line| {
                line.split_whitespace()
                    .nth(1)
                    .and_then(|v| v.parse::<u64>().ok())
            })
            .unwrap_or(0)
    };
    let rss_mib = parse_kb(&status, "VmRSS:") / 1024;
    let alloc_mib = parse_kb(&status, "VmSize:") / 1024;
    let total_mib = (parse_kb(&meminfo, "MemTotal:") / 1024).max(1);
    Some(ProcessMemoryStats {
        used_mib: rss_mib,
        allocated_mib: alloc_mib,
        total_mib,
    })
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn read_process_memory() -> Option<ProcessMemoryStats> {
    None
}

fn cpu_summary_line() -> &'static str {
    static CPU_LINE: OnceLock<String> = OnceLock::new();
    CPU_LINE.get_or_init(|| {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let brand = read_cpu_brand();
        format!("CPU: {cores}x {brand}")
    })
}

fn percent_of(part: u64, total: u64) -> u64 {
    part.saturating_mul(100)
        .checked_div(total)
        .unwrap_or(0)
        .min(100)
}

fn read_cpu_brand() -> String {
    #[cfg(target_arch = "x86_64")]
    {
        let max_ext = core::arch::x86_64::__cpuid(0x8000_0000).eax;
        if max_ext >= 0x8000_0004 {
            let mut bytes = [0u8; 48];
            for (idx, leaf) in [0x8000_0002u32, 0x8000_0003, 0x8000_0004]
                .into_iter()
                .enumerate()
            {
                let res = core::arch::x86_64::__cpuid(leaf);
                let offset = idx * 16;
                bytes[offset..offset + 4].copy_from_slice(&res.eax.to_le_bytes());
                bytes[offset + 4..offset + 8].copy_from_slice(&res.ebx.to_le_bytes());
                bytes[offset + 8..offset + 12].copy_from_slice(&res.ecx.to_le_bytes());
                bytes[offset + 12..offset + 16].copy_from_slice(&res.edx.to_le_bytes());
            }
            let raw = String::from_utf8_lossy(&bytes);
            let trimmed = raw
                .trim_matches('\0')
                .split_whitespace()
                .collect::<Vec<_>>();
            if !trimmed.is_empty() {
                return trimmed.join(" ");
            }
        }
    }
    std::env::var("PROCESSOR_IDENTIFIER")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| std::env::consts::ARCH.to_string())
}

#[derive(Debug, Clone, Default)]
pub struct DebugColumns {
    pub left: Vec<String>,
    pub right: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct OverlayRow {
    bg: EntityHandle,
    text: EntityHandle,
}

const F3_CANVAS: Canvas = Canvas {
    reference_resolution: [1280.0, 720.0],
    scale_mode: CanvasScaleMode::ConstantPixelSize,
    match_width_or_height: 0.5,
};

/// Manages F3 toggle state, smoothed telemetry, and pooled ECS UI rows.
pub struct DebugHudState {
    pub open: bool,
    smoothed_fps: f32,
    smoothed_cpu_ms: f32,
    sim_step_ms: f32,
    alloc_window_sec: f32,
    last_used_mib: Option<u64>,
    alloc_rate_mib_s: u64,
    left_rows: Vec<OverlayRow>,
    right_rows: Vec<OverlayRow>,
    world_seed: u64,
}

impl Default for DebugHudState {
    fn default() -> Self {
        Self::new(2026)
    }
}

impl DebugHudState {
    pub fn new(world_seed: u64) -> Self {
        Self {
            open: false,
            smoothed_fps: 60.0,
            smoothed_cpu_ms: 16.67,
            sim_step_ms: 0.0,
            alloc_window_sec: 0.0,
            last_used_mib: None,
            alloc_rate_mib_s: 0,
            left_rows: Vec::new(),
            right_rows: Vec::new(),
            world_seed,
        }
    }

    pub fn record_sim_step_ms(&mut self, ms: f32) {
        self.sim_step_ms = if self.sim_step_ms <= 0.0 {
            ms
        } else {
            self.sim_step_ms * 0.85 + ms * 0.15
        };
    }

    pub fn update_telemetry(&mut self, dt: f32) {
        let clamped_dt = dt.clamp(1e-4, 0.25);
        let inst_fps = (1.0 / clamped_dt).clamp(1.0, 2000.0);
        let inst_ms = clamped_dt * 1000.0;
        self.smoothed_fps = self.smoothed_fps * 0.9 + inst_fps * 0.1;
        self.smoothed_cpu_ms = self.smoothed_cpu_ms * 0.9 + inst_ms * 0.1;

        self.alloc_window_sec += clamped_dt;
        if self.alloc_window_sec >= 1.0 {
            if let Some(mem) = read_process_memory() {
                if let Some(prev) = self.last_used_mib {
                    let delta = mem.used_mib.saturating_sub(prev);
                    let secs = self.alloc_window_sec.max(1.0);
                    self.alloc_rate_mib_s = ((delta as f32) / secs).round() as u64;
                }
                self.last_used_mib = Some(mem.used_mib);
            }
            self.alloc_window_sec = 0.0;
        }
    }

    /// Formats Horizon-aligned left and right debug overlay columns.
    pub fn build_columns(
        &self,
        screen: [f32; 2],
        sim: &SimWorld,
        player: &PlayerController,
        commands: &CommandWorldState,
    ) -> DebugColumns {
        let host = read_host_render_stats();
        let fps = host
            .as_ref()
            .filter(|h| h.fps > 0.5)
            .map(|h| h.fps.round() as u32)
            .unwrap_or_else(|| self.smoothed_fps.round() as u32);
        let cpu_ms = host
            .as_ref()
            .filter(|h| h.frame_time_ms > 0.0)
            .map(|h| h.frame_time_ms)
            .unwrap_or(self.smoothed_cpu_ms);

        let chunks_loaded = sim.chunks.len();
        let active_cells = sim.active_count();
        let tracked_cells = sim.tracked_cell_count();
        let view_chunks = 1;

        let day = commands.time_ticks / 24_000;
        let mem = read_process_memory().unwrap_or_default();
        let mem_pct = percent_of(mem.used_mib, mem.total_mib);
        let alloc_pct = percent_of(mem.allocated_mib, mem.total_mib);

        let feet = player.feet_pos;
        let bx = feet.x.floor() as i32;
        let by = feet.y.floor() as i32;
        let bz = feet.z.floor() as i32;
        let l2 = [
            (feet.x * 4.0).floor() as i32,
            (feet.y * 4.0).floor() as i32,
            (feet.z * 4.0).floor() as i32,
        ];
        let l3 = [
            (feet.x * 16.0).floor() as i32,
            (feet.y * 16.0).floor() as i32,
            (feet.z * 16.0).floor() as i32,
        ];
        let chunk_w = CHUNK_DIMS_L1[0] as i32;
        let chunk_d = CHUNK_DIMS_L1[2] as i32;
        let cx = bx.div_euclid(chunk_w);
        let cz = bz.div_euclid(chunk_d);
        let lx = bx.rem_euclid(chunk_w);
        let ly = by.rem_euclid(CHUNK_DIM_Y_SECTION);
        let lz = bz.rem_euclid(chunk_d);
        let rx = cx >> 5;
        let rz = cz >> 5;

        let (dir, towards, yaw_deg, pitch_deg) = facing_cardinal_and_angles(player.forward_dir());
        let horiz_speed = Vec3::new(player.velocity.x, 0.0, player.velocity.z).length();
        let total_speed = player.velocity.length();

        let left = vec![
            format!("{fps} fps T: inf vsync (frame {cpu_ms:.2}ms)"),
            format!(
                "C: {chunks_loaded}/{chunks_loaded} (s) D: {view_chunks}, Active: {active_cells}, Cells: {tracked_cells}"
            ),
            format!(
                "E: 60 Hz | Mode: {} | Tier: {} | Tool: {}",
                player.motion_mode_label(),
                player.tier_summary(),
                player.active_tool_label()
            ),
            format!(
                "W: {} ({}t, day {day}), Weather: {}, Rules: day={} wth={} fire={}",
                commands.time_phase_label(),
                commands.time_ticks,
                commands.weather.label(),
                commands.rules.do_daylight_cycle,
                commands.rules.do_weather_cycle,
                commands.rules.do_fire_tick,
            ),
            String::new(),
            format!("Mem: {mem_pct:2}% {:03}/{:03}MiB", mem.used_mib, mem.total_mib),
            format!("Allocation rate: {:03}MiB/s", self.alloc_rate_mib_s),
            format!(
                "Allocated: {alloc_pct:2}% {:03}MiB",
                mem.allocated_mib
            ),
            String::new(),
            format!("XYZ: {:.3} / {:.5} / {:.3}", feet.x, feet.y, feet.z),
            format!(
                "Block: {bx} {by} {bz} [L2: {} {} {} | L3: {} {} {}]",
                l2[0], l2[1], l2[2], l3[0], l3[1], l3[2]
            ),
            format!("Chunk: {lx} {ly} {lz} [{cx} {cz} in r.{rx}.{rz}.mca]"),
            format!("Facing: {dir} ({towards}) ({yaw_deg:.1} / {pitch_deg:.1})"),
            format!(
                "emergence:overworld FC: {chunks_loaded} | Gamemode: {} | Seed: {}",
                commands.gamemode_label(player),
                self.world_seed
            ),
            format!("Section-relative: {lx:02} {ly:02} {lz:02}"),
            format!("Speed: {horiz_speed:.2} m/s ({total_speed:.2} m/s)"),
            String::new(),
            "Debug: [F3] Toggle | [T]/[Enter]/[/] Chat | [Tab] Fly | [B] NEI | [1-3] Tier | [4-0/Q] Tools"
                .to_string(),
        ];

        let (logical_w, logical_h) = (
            if screen[0] > 1.0 { screen[0] } else { 1280.0 },
            if screen[1] > 1.0 { screen[1] } else { 720.0 },
        );
        let (phys_w, phys_h, scale) = match &host {
            Some(h) if h.physical_size.0 > 0 && h.physical_size.1 > 0 => (
                h.physical_size.0,
                h.physical_size.1,
                h.scale_factor.max(0.5),
            ),
            _ => (logical_w as u32, logical_h as u32, 1.0),
        };
        let frame_line = match &host {
            Some(h) => match h.gpu_frame_ms {
                Some(gpu_ms) => {
                    let p95 = h.cpu_p95_ms.unwrap_or(h.frame_time_ms);
                    let low1 = h.low_1pct_fps.map(|v| v.round() as u32).unwrap_or(fps);
                    format!(
                        "GPU: {gpu_ms:.2}ms (CPU {:.2}ms, p95 {p95:.2}ms, 1% low {low1} fps)",
                        h.frame_time_ms
                    )
                }
                None => format!("Frame: CPU {:.2}ms (player)", h.frame_time_ms),
            },
            None => format!("Frame: CPU {cpu_ms:.2}ms (preview-module)"),
        };

        let mut right = vec![
            "Emergence 0.1.0 (0.1.0/prisma)".to_string(),
            format!(
                "Integrated server @ {:.2}/16.7 ms, {active_cells} active, {tracked_cells} cells",
                self.sim_step_ms
            ),
            String::new(),
            cpu_summary_line().to_string(),
            format!("Display: {phys_w}x{phys_h} (Prisma Vulkan)"),
            format!("Window: {logical_w:.0}x{logical_h:.0} ({scale:.2}x pixel density)"),
            frame_line,
        ];

        if let Some(h) = &host {
            if !h.top_gpu_passes.is_empty() {
                let mut passes = String::from("GPU passes: ");
                for (i, (name, ms)) in h.top_gpu_passes.iter().take(2).enumerate() {
                    if i > 0 {
                        passes.push_str(", ");
                    }
                    let _ = write!(passes, "{name} {ms:.2}ms");
                }
                right.push(passes);
            }
        }

        if let Some(hit) = player.last_hit() {
            if let Some((mat, cell, l3_mask)) = sim.inspect_l2(hit.world_l2) {
                let mat_name = &sim.materials.get(mat).name;
                let (ident_name, kind) = identify_cell(&cell);
                let slug = mat_name.to_ascii_lowercase();
                right.push(String::new());
                right.push(format!(
                    "Targeted Block: {}, {}, {} [L2: {}, {}, {} | L3: {}, {}, {}]",
                    hit.world_l1[0],
                    hit.world_l1[1],
                    hit.world_l1[2],
                    hit.world_l2[0],
                    hit.world_l2[1],
                    hit.world_l2[2],
                    hit.world_l3[0],
                    hit.world_l3[1],
                    hit.world_l3[2],
                ));
                right.push(format!(
                    "emergence:{slug} ({ident_name} / {})",
                    kind.label()
                ));
                right.push(format!("matter: {:?}", cell.env.matter()));
                right.push(format!("temperature: {:?}", cell.env.temperature()));
                right.push(format!("pressure: {:?}", cell.env.pressure()));
                right.push(format!("density: {}/7", cell.env.density()));
                right.push(format!("bio: {:?}", cell.env.bio_activity()));
                right.push(format!("durability: {}/255", cell.durability));
                right.push(format!(
                    "l3_mask: {l3_mask:#018x} ({}/64)",
                    l3_mask.count_ones()
                ));
                right.push(format!(
                    "face: [{}, {}, {}] ({:.2}m)",
                    hit.normal[0], hit.normal[1], hit.normal[2], hit.distance
                ));
                for tag in collect_cell_debug_tags(&cell).into_iter().take(6) {
                    right.push(tag);
                }
            }
        }

        DebugColumns { left, right }
    }

    /// Synchronizes pooled ECS UI entities to display or hide the F3 overlay.
    pub fn sync_ui(
        &mut self,
        world: &mut GameWorld<'_>,
        sim: &SimWorld,
        player: &PlayerController,
        commands: &CommandWorldState,
    ) {
        if !self.open {
            for row in self.left_rows.iter().chain(self.right_rows.iter()) {
                world.set(row.bg, Active(false));
                world.set(row.text, Active(false));
            }
            return;
        }

        let screen = world.screen_size();
        let screen_w = if screen[0] > 1.0 { screen[0] } else { 1280.0 };
        let cols = self.build_columns(screen, sim, player, commands);

        Self::sync_column(world, &mut self.left_rows, &cols.left, false, screen_w);
        Self::sync_column(world, &mut self.right_rows, &cols.right, true, screen_w);
    }

    fn sync_column(
        world: &mut GameWorld<'_>,
        pool: &mut Vec<OverlayRow>,
        lines: &[String],
        right_aligned: bool,
        screen_w: f32,
    ) {
        while pool.len() < lines.len() {
            let idx = pool.len();
            let side = if right_aligned { "right" } else { "left" };
            let bg = world.spawn();
            world.set(bg, Name(format!("f3_{side}_bg_{idx}")));
            world.set(bg, Node::default());
            world.set(bg, F3_CANVAS);
            world.set(bg, Active(false));

            let text = world.spawn();
            world.set(text, Name(format!("f3_{side}_text_{idx}")));
            world.set(text, Node::default());
            world.set(text, F3_CANVAS);
            world.set(text, Active(false));

            pool.push(OverlayRow { bg, text });
        }

        for (idx, row) in pool.iter().enumerate() {
            let Some(line) = lines.get(idx) else {
                world.set(row.bg, Active(false));
                world.set(row.text, Active(false));
                continue;
            };
            if line.is_empty() {
                world.set(row.bg, Active(false));
                world.set(row.text, Active(false));
                continue;
            }
            let y = F3_EDGE_PAD + (idx as f32) * F3_ROW_HEIGHT;
            let text_w = estimate_text_width(line, F3_FONT_SIZE).max(8.0);
            let box_w = text_w + F3_TEXT_PAD_X * 2.0;
            let left = if right_aligned {
                (screen_w - F3_EDGE_PAD - box_w).max(F3_EDGE_PAD)
            } else {
                F3_EDGE_PAD
            };
            let text_left = if right_aligned {
                left
            } else {
                left + F3_TEXT_PAD_X
            };

            world.set(
                row.bg,
                Style {
                    width: Some(box_w),
                    height: Some(F3_ROW_HEIGHT),
                    anchors: Anchors::TOP_LEFT,
                    pivot: Pivot::TOP_LEFT,
                    margin: Margin {
                        left,
                        top: y,
                        ..Margin::default()
                    },
                    background: F3_BG_COLOR,
                    ..Style::default()
                },
            );
            world.set(row.bg, Active(true));

            world.set(
                row.text,
                Style {
                    width: Some(text_w + F3_TEXT_PAD_X),
                    height: Some(F3_ROW_HEIGHT),
                    anchors: Anchors::TOP_LEFT,
                    pivot: Pivot::TOP_LEFT,
                    margin: Margin {
                        left: text_left,
                        top: y,
                        ..Margin::default()
                    },
                    ..Style::default()
                },
            );
            world.set(
                row.text,
                Text {
                    content: line.clone(),
                    font_size: F3_FONT_SIZE,
                    color: F3_TEXT_COLOR,
                    alignment: if right_aligned {
                        TextAlign::Right
                    } else {
                        TextAlign::Left
                    },
                    wrap: false,
                    ..Text::default()
                },
            );
            world.set(row.text, Active(true));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::materials::MaterialTable;
    use crate::world_gen::generate_world;

    #[test]
    fn facing_matches_horizon_cardinal_axes_and_angles() {
        let (d, t, yaw, pitch) = facing_cardinal_and_angles(Vec3::Z);
        assert_eq!((d, t), ("south", "Towards positive Z"));
        assert!(yaw.abs() < 0.01 && pitch.abs() < 0.01);

        let (d, t, yaw, _) = facing_cardinal_and_angles(-Vec3::X);
        assert_eq!((d, t), ("west", "Towards negative X"));
        assert!((yaw - 90.0).abs() < 0.01);

        let (d, t, yaw, _) = facing_cardinal_and_angles(-Vec3::Z);
        assert_eq!((d, t), ("north", "Towards negative Z"));
        assert!((yaw + 180.0).abs() < 0.01);

        let (d, t, yaw, _) = facing_cardinal_and_angles(Vec3::X);
        assert_eq!((d, t), ("east", "Towards positive X"));
        assert!((yaw + 90.0).abs() < 0.01);
    }

    #[test]
    fn f3_columns_include_horizon_sections_and_targeted_block_tags() {
        let mats = MaterialTable::compiled_default();
        let gen = generate_world(&mats, 2026);
        let sim = SimWorld::new(mats, gen.chunks, gen.cells, gen.active);
        let mut ecs = prism_ecs::World::new();
        let mut world = GameWorld::host(&mut ecs);

        let mut player = PlayerController::new();
        player.attach_ecs(&mut world, &sim);
        player.feet_pos = Vec3::new(8.5, 4.0, 8.5);
        player.yaw = -std::f32::consts::FRAC_PI_2;
        player.pitch = -0.35;
        player.sync_camera_transform(&mut world);
        let mut sim_mut = sim;
        player.update_look_and_tools(&mut world, &mut sim_mut, 1.0 / 60.0);

        let commands = CommandWorldState::new();
        let mut hud = DebugHudState::new(2026);
        hud.record_sim_step_ms(1.25);

        let cols = hud.build_columns([1280.0, 720.0], &sim_mut, &player, &commands);
        let left_text = cols.left.join("\n");
        let right_text = cols.right.join("\n");

        assert!(cols.left[0].contains("fps T:"));
        assert!(left_text.contains("Mem:"));
        assert!(left_text.contains("Allocation rate:"));
        assert!(left_text.contains("XYZ:"));
        assert!(left_text.contains("Block:"));
        assert!(left_text.contains("Chunk:"));
        assert!(left_text.contains("Facing:"));
        assert!(left_text.contains("Section-relative:"));
        assert!(left_text.contains("Speed:"));
        assert!(cols.left.iter().any(|line| line.is_empty()));

        assert_eq!(cols.right[0], "Emergence 0.1.0 (0.1.0/prisma)");
        assert!(right_text.contains("Integrated server @"));
        assert!(right_text.contains("CPU:"));
        assert!(right_text.contains("Display:"));
        assert!(right_text.contains("Window:"));
        assert!(right_text.contains("Targeted Block:"));
        assert!(right_text.contains("#emergence:"));
    }

    #[test]
    fn f3_ui_pools_rows_and_hides_cleanly_when_closed() {
        let mats = MaterialTable::compiled_default();
        let gen = generate_world(&mats, 2026);
        let sim = SimWorld::new(mats, gen.chunks, gen.cells, gen.active);
        let mut ecs = prism_ecs::World::new();
        let mut world = GameWorld::host(&mut ecs);

        let mut player = PlayerController::new();
        player.attach_ecs(&mut world, &sim);
        let commands = CommandWorldState::new();
        let mut hud = DebugHudState::new(2026);

        hud.sync_ui(&mut world, &sim, &player, &commands);
        assert!(hud.left_rows.is_empty());

        hud.open = true;
        hud.sync_ui(&mut world, &sim, &player, &commands);
        assert!(!hud.left_rows.is_empty());
        assert!(!hud.right_rows.is_empty());
        assert_eq!(world.get::<Active>(hud.left_rows[0].bg), Some(Active(true)));
        assert_eq!(
            world.get::<Active>(hud.left_rows[0].text),
            Some(Active(true))
        );

        hud.open = false;
        hud.sync_ui(&mut world, &sim, &player, &commands);
        assert_eq!(
            world.get::<Active>(hud.left_rows[0].bg),
            Some(Active(false))
        );
        assert_eq!(
            world.get::<Active>(hud.left_rows[0].text),
            Some(Active(false))
        );
    }
}
