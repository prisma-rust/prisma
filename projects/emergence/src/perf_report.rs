//! Command-line enabled session performance report exported to `<project>/.cache/perf/`.
//!
//! Collects every frame's F3-aligned CPU/GPU timings, per-pass Vulkan timings,
//! fixed-tick simulation cost, chunk/cell counts, process memory, worst frame
//! spikes, and 1-second timeline buckets, then atomically writes one self-contained
//! JSON report file on exit.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
#[cfg(any(not(target_os = "windows"), test))]
use std::time::{SystemTime, UNIX_EPOCH};

use prism_render_edges::HISTOGRAM_EDGES_MS;
use serde::Serialize;

use crate::debug_hud::{
    cpu_summary_line, read_cpu_brand, read_host_render_stats, read_process_memory, HostRenderStats,
    ProcessMemoryStats,
};

mod prism_render_edges {
    /// Aligned with `prism_render::frame_stats::FRAME_HISTOGRAM_EDGES_MS`.
    pub const HISTOGRAM_EDGES_MS: [f32; 24] = [
        0.0, 1.0, 2.0, 3.0, 4.17, 5.0, 6.0, 7.0, 8.33, 10.0, 12.0, 14.0, 16.67, 20.0, 25.0, 33.33,
        40.0, 50.0, 66.67, 100.0, 150.0, 250.0, 500.0, 1000.0,
    ];
}

pub const REPORT_FORMAT: &str = "emergence-perf-report";
pub const REPORT_VERSION: u32 = 2;
pub const PERF_ENV_KEY: &str = "EMERGENCE_PERF_REPORT";
pub const PERF_WARMUP_ENV_KEY: &str = "EMERGENCE_PERF_WARMUP";
pub const CACHE_DIR_NAME: &str = ".cache";
pub const PERF_SUBDIR_NAME: &str = "perf";
const DEFAULT_WARMUP_FRAMES: u64 = 60;
const MAX_SERIES_SAMPLES: usize = 500_000;
const MAX_WORST_FRAMES: usize = 10;
const MAX_TIMELINE_BUCKETS: usize = 3600;
const HISTOGRAM_BUCKETS: usize = HISTOGRAM_EDGES_MS.len() + 1;

/// Output destination strategy when performance reporting is enabled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PerfOutputTarget {
    /// Disabled: zero per-frame recording and no file written on exit.
    Disabled,
    /// Write single report inside `<project_root>/.cache/perf/emergence_perf_<stamp>_pid<pid>.json`.
    ProjectCacheAuto,
    /// Write single report inside an explicit directory using the standard file naming rule.
    CustomDir(PathBuf),
    /// Write directly to the explicit file path provided by CLI/env.
    CustomFile(PathBuf),
}

/// Session configuration for performance collection & exit export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerfReportConfig {
    pub target: PerfOutputTarget,
    pub warmup_frames: u64,
    pub trigger: &'static str,
}

impl Default for PerfReportConfig {
    fn default() -> Self {
        Self {
            target: PerfOutputTarget::Disabled,
            warmup_frames: DEFAULT_WARMUP_FRAMES,
            trigger: "disabled",
        }
    }
}

impl PerfReportConfig {
    pub fn is_enabled(&self) -> bool {
        !matches!(self.target, PerfOutputTarget::Disabled)
    }

    pub fn project_cache_default() -> Self {
        Self {
            target: PerfOutputTarget::ProjectCacheAuto,
            warmup_frames: DEFAULT_WARMUP_FRAMES,
            trigger: "cli",
        }
    }
}

/// Result of parsing `emergence` CLI arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliAction {
    Run(PerfReportConfig),
    PrintHelpAndExit,
}

pub fn cli_usage_text() -> &'static str {
    "\
Emergence — Prisma v3.0 Voxel Sandbox\n\
\n\
USAGE:\n\
  emergence [OPTIONS]\n\
\n\
OPTIONS:\n\
  --perf-report, --perf         Enable F3-detailed performance telemetry and write one\n\
                                JSON report on exit under <project>/.cache/perf/\n\
  --perf-report=<PATH>          Write the exit performance report to <PATH>\n\
  --perf-out <PATH>             Explicit file or directory path for the perf report\n\
  --perf-dir <DIR>              Directory inside which standard-named perf JSON is written\n\
  --perf-warmup=<N>             Warmup frames excluded from post_warmup stats (default: 60)\n\
  -h, --help                    Print this help message and exit\n"
}

/// Parses command-line arguments (including `argv[0]` or skipped binary name).
pub fn parse_cli_args<I, S>(args: I) -> Result<CliAction, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let raw: Vec<String> = args.into_iter().map(|s| s.as_ref().to_string()).collect();
    let mut idx = 0usize;
    if raw.first().is_some_and(|a| !a.starts_with('-')) {
        idx = 1;
    }

    let mut cfg = env_fallback_config();
    while idx < raw.len() {
        let arg = raw[idx].as_str();
        match arg {
            "-h" | "--help" => return Ok(CliAction::PrintHelpAndExit),
            "--perf" | "--perf-report" => {
                cfg.target = PerfOutputTarget::ProjectCacheAuto;
                cfg.trigger = "cli";
            }
            "--perf-out" => {
                idx += 1;
                let Some(val) = raw.get(idx) else {
                    return Err("--perf-out requires a file or directory path".to_string());
                };
                cfg.target = target_from_path_arg(val, false);
                cfg.trigger = "cli";
            }
            "--perf-dir" => {
                idx += 1;
                let Some(val) = raw.get(idx) else {
                    return Err("--perf-dir requires a directory path".to_string());
                };
                cfg.target = target_from_path_arg(val, true);
                cfg.trigger = "cli";
            }
            "--perf-warmup" => {
                idx += 1;
                let Some(val) = raw.get(idx) else {
                    return Err("--perf-warmup requires an integer frame count".to_string());
                };
                cfg.warmup_frames = val
                    .parse::<u64>()
                    .map_err(|_| format!("invalid --perf-warmup value: {val}"))?;
            }
            _ if arg.starts_with("--perf-report=") => {
                let val = &arg["--perf-report=".len()..];
                cfg.target = target_from_path_arg(val, false);
                cfg.trigger = "cli";
            }
            _ if arg.starts_with("--perf=") => {
                let val = &arg["--perf=".len()..];
                cfg.target = target_from_path_arg(val, false);
                cfg.trigger = "cli";
            }
            _ if arg.starts_with("--perf-out=") => {
                let val = &arg["--perf-out=".len()..];
                cfg.target = target_from_path_arg(val, false);
                cfg.trigger = "cli";
            }
            _ if arg.starts_with("--perf-dir=") => {
                let val = &arg["--perf-dir=".len()..];
                cfg.target = target_from_path_arg(val, true);
                cfg.trigger = "cli";
            }
            _ if arg.starts_with("--perf-warmup=") => {
                let val = &arg["--perf-warmup=".len()..];
                cfg.warmup_frames = val
                    .parse::<u64>()
                    .map_err(|_| format!("invalid --perf-warmup value: {val}"))?;
            }
            other => {
                log::warn!("unrecognized emergence argument ignored: {other}");
            }
        }
        idx += 1;
    }

    Ok(CliAction::Run(cfg))
}

fn target_from_path_arg(raw: &str, force_dir: bool) -> PerfOutputTarget {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return PerfOutputTarget::ProjectCacheAuto;
    }
    let path = PathBuf::from(trimmed);
    if force_dir || trimmed.ends_with('/') || trimmed.ends_with('\\') || path.is_dir() {
        PerfOutputTarget::CustomDir(path)
    } else {
        PerfOutputTarget::CustomFile(path)
    }
}

fn env_fallback_config() -> PerfReportConfig {
    let mut cfg = PerfReportConfig::default();
    if let Ok(val) = std::env::var(PERF_ENV_KEY) {
        let trimmed = val.trim();
        if !trimmed.is_empty() && trimmed != "0" && !trimmed.eq_ignore_ascii_case("false") {
            cfg.trigger = "env";
            if matches!(
                trimmed.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            ) {
                cfg.target = PerfOutputTarget::ProjectCacheAuto;
            } else {
                cfg.target = target_from_path_arg(trimmed, false);
            }
        }
    }
    if let Some(warmup) = std::env::var(PERF_WARMUP_ENV_KEY)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
    {
        cfg.warmup_frames = warmup;
    }
    cfg
}

static CLI_PERF_CONFIG: OnceLock<Mutex<Option<PerfReportConfig>>> = OnceLock::new();

/// Stores parsed CLI perf config so `build_app()` picks it up inside `EmergenceRuntime`.
pub fn install_cli_config(config: PerfReportConfig) {
    let slot = CLI_PERF_CONFIG.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = slot.lock() {
        *guard = Some(config);
    }
}

pub fn active_config() -> PerfReportConfig {
    if let Some(slot) = CLI_PERF_CONFIG.get() {
        if let Ok(guard) = slot.lock() {
            if let Some(cfg) = guard.as_ref() {
                return cfg.clone();
            }
        }
    }
    env_fallback_config()
}

/// Resolves the project root (`projects/emergence`) cleanly across dev `cargo run` and standalone exe launches.
pub fn resolve_project_root() -> PathBuf {
    let env_root = std::env::var_os("PRISMA_ASSET_ROOT").map(PathBuf::from);
    let cwd = std::env::current_dir().ok();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    resolve_project_root_from(env_root.as_deref(), cwd.as_deref(), exe_dir.as_deref())
}

fn is_emergence_project_dir(dir: &Path) -> bool {
    dir.join("Cargo.toml").is_file()
        && dir
            .join("assets")
            .join("data")
            .join("materials.json")
            .is_file()
}

pub(crate) fn resolve_project_root_from(
    env_root: Option<&Path>,
    cwd: Option<&Path>,
    exe_dir: Option<&Path>,
) -> PathBuf {
    if let Some(root) = env_root.filter(|p| p.join("assets").is_dir()) {
        return root.to_path_buf();
    }

    // Walk up from executable and working directory looking specifically for the `emergence` crate root.
    for start in [exe_dir, cwd].into_iter().flatten() {
        let mut cur = Some(start);
        while let Some(dir) = cur {
            if is_emergence_project_dir(dir) {
                return dir.to_path_buf();
            }
            let nested = dir.join("projects").join("emergence");
            if is_emergence_project_dir(&nested) {
                return nested;
            }
            cur = dir.parent();
        }
    }

    // Fallback for standalone packaged builds with `<dir>/assets` next to the binary.
    for start in [cwd, exe_dir].into_iter().flatten() {
        let mut cur = Some(start);
        while let Some(dir) = cur {
            if dir.join("assets").is_dir() {
                return dir.to_path_buf();
            }
            cur = dir.parent();
        }
    }

    cwd.or(exe_dir)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Returns `<project_root>/.cache/perf/`.
pub fn default_perf_cache_dir() -> PathBuf {
    resolve_project_root()
        .join(CACHE_DIR_NAME)
        .join(PERF_SUBDIR_NAME)
}

/// Standard file naming rule: `emergence_perf_<YYYYMMDD-HHMMSS>_pid<PID>.json`.
pub fn format_report_filename(stamp_compact: &str, pid: u32) -> String {
    format!("emergence_perf_{stamp_compact}_pid{pid}.json")
}

/// Resolves final output file path for `config` at session end.
pub fn resolve_output_path(
    config: &PerfReportConfig,
    stamp_compact: &str,
    pid: u32,
) -> Option<PathBuf> {
    match &config.target {
        PerfOutputTarget::Disabled => None,
        PerfOutputTarget::ProjectCacheAuto => {
            Some(default_perf_cache_dir().join(format_report_filename(stamp_compact, pid)))
        }
        PerfOutputTarget::CustomDir(dir) => {
            Some(dir.join(format_report_filename(stamp_compact, pid)))
        }
        PerfOutputTarget::CustomFile(path) => Some(path.clone()),
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CalendarStamp {
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    is_utc: bool,
}

impl CalendarStamp {
    pub(crate) fn now() -> Self {
        #[cfg(target_os = "windows")]
        {
            #[repr(C)]
            struct SystemTimeWin {
                year: u16,
                month: u16,
                _day_of_week: u16,
                day: u16,
                hour: u16,
                minute: u16,
                second: u16,
                _milliseconds: u16,
            }
            #[link(name = "kernel32")]
            extern "system" {
                fn GetLocalTime(lp_system_time: *mut SystemTimeWin);
            }
            let mut st = SystemTimeWin {
                year: 0,
                month: 0,
                _day_of_week: 0,
                day: 0,
                hour: 0,
                minute: 0,
                second: 0,
                _milliseconds: 0,
            };
            // SAFETY: Valid mutable pointer to initialized repr(C) `SYSTEMTIME`.
            unsafe { GetLocalTime(&mut st) };
            Self {
                year: st.year,
                month: st.month as u8,
                day: st.day as u8,
                hour: st.hour as u8,
                minute: st.minute as u8,
                second: st.second as u8,
                is_utc: false,
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            Self::from_unix_utc(secs)
        }
    }

    #[allow(dead_code)]
    fn from_unix_utc(unix_secs: u64) -> Self {
        let days = (unix_secs / 86_400) as i64;
        let rem = unix_secs % 86_400;
        let hour = (rem / 3_600) as u8;
        let minute = ((rem % 3_600) / 60) as u8;
        let second = (rem % 60) as u8;

        // Howard Hinnant civil-from-days algorithm in Z >= 0 domain.
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = (y + i64::from(m <= 2)) as u16;
        Self {
            year,
            month: m as u8,
            day: d as u8,
            hour,
            minute,
            second,
            is_utc: true,
        }
    }

    pub(crate) fn compact_for_filename(&self) -> String {
        let suffix = if self.is_utc { "Z" } else { "" };
        format!(
            "{:04}{:02}{:02}-{:02}{:02}{:02}{suffix}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    pub(crate) fn iso_display(&self) -> String {
        let suffix = if self.is_utc { "Z" } else { "" };
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}{suffix}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

#[derive(Debug, Clone, Default)]
struct SeriesAccumulator {
    samples: Vec<f32>,
}

impl SeriesAccumulator {
    fn push(&mut self, value: f32) {
        if !value.is_finite() || value < 0.0 || self.samples.len() >= MAX_SERIES_SAMPLES {
            return;
        }
        self.samples.push(value);
    }

    fn len(&self) -> usize {
        self.samples.len()
    }

    fn summarize(&self) -> Option<SeriesStats> {
        if self.samples.is_empty() {
            return None;
        }
        let mut sorted = self.samples.clone();
        sorted.sort_by(f32::total_cmp);

        let n = sorted.len();
        let sum: f64 = sorted.iter().map(|&v| v as f64).sum();
        let mean = (sum / n as f64) as f32;
        let var: f64 = sorted
            .iter()
            .map(|&v| {
                let d = v as f64 - mean as f64;
                d * d
            })
            .sum::<f64>()
            / n as f64;

        let mut histogram = [0u32; HISTOGRAM_BUCKETS];
        for &v in &sorted {
            let bucket = HISTOGRAM_EDGES_MS
                .iter()
                .position(|&edge| v <= edge)
                .unwrap_or(HISTOGRAM_EDGES_MS.len());
            histogram[bucket] += 1;
        }

        Some(SeriesStats {
            samples: n,
            min_ms: round3(sorted[0]),
            mean_ms: round3(mean),
            p50_ms: round3(percentile(&sorted, 0.50)),
            p95_ms: round3(percentile(&sorted, 0.95)),
            p99_ms: round3(percentile(&sorted, 0.99)),
            p99_9_ms: round3(percentile(&sorted, 0.999)),
            max_ms: round3(sorted[n - 1]),
            stddev_ms: round3(var.sqrt() as f32),
            histogram_counts: histogram.to_vec(),
        })
    }

    fn summarize_fps(&self) -> Option<FpsStats> {
        if self.samples.is_empty() {
            return None;
        }
        let mut sorted_ms = self.samples.clone();
        sorted_ms.sort_by(f32::total_cmp);
        let n = sorted_ms.len();
        let mean_ms = (sorted_ms.iter().map(|&v| v as f64).sum::<f64>() / n as f64) as f32;
        let p50_ms = percentile(&sorted_ms, 0.50);
        let p95_ms = percentile(&sorted_ms, 0.95);
        let min_ms = sorted_ms[0];
        let max_ms = sorted_ms[n - 1];

        let tail_1pct_start = ((n as f64) * 0.99).floor() as usize;
        let tail_01pct_start = ((n as f64) * 0.999).floor() as usize;
        let low_1pct = tail_mean_fps(&sorted_ms[tail_1pct_start.min(n - 1)..]);
        let low_0_1pct = tail_mean_fps(&sorted_ms[tail_01pct_start.min(n - 1)..]);

        Some(FpsStats {
            avg: ms_to_fps(mean_ms),
            p50: ms_to_fps(p50_ms),
            p5_frame_time: ms_to_fps(p95_ms),
            min: ms_to_fps(max_ms),
            max: ms_to_fps(min_ms),
            low_1pct,
            low_0_1pct,
        })
    }
}

fn percentile(sorted: &[f32], q: f64) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) as f64 * q.clamp(0.0, 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn tail_mean_fps(tail_sorted_ms: &[f32]) -> f32 {
    if tail_sorted_ms.is_empty() {
        return 0.0;
    }
    let mean = tail_sorted_ms.iter().map(|&v| v as f64).sum::<f64>() / tail_sorted_ms.len() as f64;
    ms_to_fps(mean as f32)
}

fn ms_to_fps(ms: f32) -> f32 {
    if ms <= 0.001 {
        return 0.0;
    }
    round1((1000.0 / ms).clamp(0.0, 10_000.0))
}

fn round1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

fn round3(v: f32) -> f32 {
    (v * 1000.0).round() / 1000.0
}

#[derive(Debug, Clone, Default)]
struct ScalarAccumulator {
    count: u64,
    start: f64,
    end: f64,
    min: f64,
    max: f64,
    sum: f64,
}

impl ScalarAccumulator {
    fn push(&mut self, val: f64) {
        if !val.is_finite() {
            return;
        }
        if self.count == 0 {
            self.start = val;
            self.min = val;
            self.max = val;
        } else {
            self.min = self.min.min(val);
            self.max = self.max.max(val);
        }
        self.end = val;
        self.sum += val;
        self.count += 1;
    }

    fn summarize(&self) -> Option<ScalarStats> {
        if self.count == 0 {
            return None;
        }
        Some(ScalarStats {
            samples: self.count,
            start: round3_f64(self.start),
            end: round3_f64(self.end),
            min: round3_f64(self.min),
            max: round3_f64(self.max),
            mean: round3_f64(self.sum / self.count as f64),
        })
    }
}

fn round3_f64(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

#[derive(Debug, Clone, Default)]
struct FrameDistributionAcc {
    main_frame_ms: SeriesAccumulator,
    logic_tick_ms: SeriesAccumulator,
    /// Fence-inclusive render-thread frame time, recorded once per unique `render_seq`.
    render_thread_ms: SeriesAccumulator,
    gpu_frame_ms: SeriesAccumulator,
    gpu_passes_total_ms: SeriesAccumulator,
    hitch_over_16_67ms: u64,
    hitch_over_33_33ms: u64,
    hitch_over_50ms: u64,
    hitch_over_100ms: u64,
}

impl FrameDistributionAcc {
    fn push_main(&mut self, main_ms: f32, logic_ms: f32) {
        self.main_frame_ms.push(main_ms);
        self.logic_tick_ms.push(logic_ms);
    }

    fn push_render(
        &mut self,
        render_ms: Option<f32>,
        gpu_frame_ms: Option<f32>,
        gpu_total_ms: Option<f32>,
    ) {
        if let Some(r) = render_ms {
            self.render_thread_ms.push(r);
            if r > 16.67 {
                self.hitch_over_16_67ms += 1;
            }
            if r > 33.33 {
                self.hitch_over_33_33ms += 1;
            }
            if r > 50.0 {
                self.hitch_over_50ms += 1;
            }
            if r > 100.0 {
                self.hitch_over_100ms += 1;
            }
        }
        if let Some(g) = gpu_frame_ms {
            self.gpu_frame_ms.push(g);
        }
        if let Some(gt) = gpu_total_ms {
            self.gpu_passes_total_ms.push(gt);
        }
    }

    fn summarize(&self) -> FrameDistribution {
        FrameDistribution {
            frames: self.main_frame_ms.len() as u64,
            render_frames: self.render_thread_ms.len() as u64,
            fps: self.main_frame_ms.summarize_fps(),
            render_fps: self.render_thread_ms.summarize_fps(),
            main_frame_ms: self.main_frame_ms.summarize(),
            logic_tick_ms: self.logic_tick_ms.summarize(),
            render_thread_ms: self.render_thread_ms.summarize(),
            gpu_frame_ms: self.gpu_frame_ms.summarize(),
            gpu_passes_total_ms: self.gpu_passes_total_ms.summarize(),
            render_hitches: HitchCounts {
                over_16_67ms: self.hitch_over_16_67ms,
                over_33_33ms: self.hitch_over_33_33ms,
                over_50ms: self.hitch_over_50ms,
                over_100ms: self.hitch_over_100ms,
            },
        }
    }
}

#[derive(Debug, Clone, Default)]
struct TimelineBucketAcc {
    second: u64,
    frames: u64,
    main_sum_ms: f64,
    main_max_ms: f32,
    logic_sum_ms: f64,
    render_frames: u64,
    render_sum_ms: f64,
    render_max_ms: f32,
    gpu_sum_ms: f64,
    gpu_count: u64,
    sim_sum_ms: f64,
    sim_count: u64,
    chunks_end: usize,
    active_cells_end: usize,
    tracked_cells_end: usize,
    dirty_chunks_sum: u64,
    mem_used_mib_end: u64,
    alloc_rate_mib_s_max: u64,
}

impl TimelineBucketAcc {
    fn freeze(&self) -> TimelineBucket {
        let main_mean = if self.frames > 0 {
            self.main_sum_ms / self.frames as f64
        } else {
            0.0
        };
        let logic_mean = if self.frames > 0 {
            self.logic_sum_ms / self.frames as f64
        } else {
            0.0
        };
        let render_mean =
            (self.render_frames > 0).then(|| self.render_sum_ms / self.render_frames as f64);
        TimelineBucket {
            second: self.second,
            frames: self.frames,
            fps_avg: ms_to_fps(main_mean as f32),
            main_mean_ms: round3_f64(main_mean),
            main_max_ms: round3(self.main_max_ms),
            logic_mean_ms: round3_f64(logic_mean),
            render_frames: self.render_frames,
            render_fps_avg: render_mean.map(|m| ms_to_fps(m as f32)),
            render_mean_ms: render_mean.map(round3_f64),
            render_max_ms: (self.render_frames > 0).then(|| round3(self.render_max_ms)),
            gpu_mean_ms: (self.gpu_count > 0)
                .then(|| round3_f64(self.gpu_sum_ms / self.gpu_count as f64)),
            sim_mean_ms: (self.sim_count > 0)
                .then(|| round3_f64(self.sim_sum_ms / self.sim_count as f64)),
            chunks_loaded: self.chunks_end,
            active_cells: self.active_cells_end,
            tracked_cells: self.tracked_cells_end,
            dirty_chunks_total: self.dirty_chunks_sum,
            working_set_mib: self.mem_used_mib_end,
            alloc_rate_mib_s: self.alloc_rate_mib_s_max,
        }
    }
}

/// Snapshot passed from `emergence_frame_tick` into [`EmergencePerfRecorder`].
#[derive(Debug, Clone)]
pub struct FrameTelemetryInput<'a> {
    pub dt: f32,
    pub logic_tick_ms: f32,
    pub chunks_loaded: usize,
    pub active_cells: usize,
    pub tracked_cells: usize,
    pub dirty_chunks: usize,
    pub screen_logical: [f32; 2],
    pub alloc_rate_mib_s: u64,
    pub player_pos: [f32; 3],
    pub gamemode: &'a str,
    pub weather: &'a str,
    pub world_time_ticks: u64,
    pub rule_daylight: bool,
    pub rule_weather: bool,
    pub rule_fire: bool,
}

/// Per-session F3-aligned performance recorder that exports a single JSON file on drop.
pub struct EmergencePerfRecorder {
    config: PerfReportConfig,
    started_instant: Instant,
    last_frame_instant: Option<Instant>,
    started_stamp: CalendarStamp,
    total_frames: u64,
    total_fixed_ticks: u64,
    last_render_seq: Option<u64>,
    latest_sim_ms: Option<f32>,
    first_frame_ms: Option<f32>,
    warmup_max_ms: f32,

    // World & display metadata
    world_seed: u64,
    gamemode_end: String,
    weather_end: String,
    start_time_ticks: Option<u64>,
    end_time_ticks: u64,
    start_pos: Option<[f32; 3]>,
    end_pos: [f32; 3],
    distance_traveled_m: f64,
    rule_daylight: bool,
    rule_weather: bool,
    rule_fire: bool,
    physical_size: (u32, u32),
    logical_size: [f32; 2],
    scale_factor: f64,

    // Statistics accumulators
    overall: FrameDistributionAcc,
    post_warmup: FrameDistributionAcc,
    sim_step_ms: SeriesAccumulator,
    gpu_passes: BTreeMap<String, SeriesAccumulator>,
    chunks_loaded: ScalarAccumulator,
    active_cells: ScalarAccumulator,
    tracked_cells: ScalarAccumulator,
    dirty_chunks: ScalarAccumulator,
    mem_used_mib: ScalarAccumulator,
    mem_allocated_mib: ScalarAccumulator,
    alloc_rate_mib_s: ScalarAccumulator,
    mem_total_mib: u64,
    worst_frames: Vec<WorstFrameEntry>,
    timeline: Vec<TimelineBucketAcc>,
    flushed: bool,
}

impl EmergencePerfRecorder {
    pub fn from_active_config(seed: u64) -> Self {
        Self::with_config(active_config(), seed)
    }

    pub fn with_config(config: PerfReportConfig, world_seed: u64) -> Self {
        Self {
            config,
            started_instant: Instant::now(),
            last_frame_instant: None,
            started_stamp: CalendarStamp::now(),
            total_frames: 0,
            total_fixed_ticks: 0,
            last_render_seq: None,
            latest_sim_ms: None,
            first_frame_ms: None,
            warmup_max_ms: 0.0,
            world_seed,
            gamemode_end: String::new(),
            weather_end: String::new(),
            start_time_ticks: None,
            end_time_ticks: 0,
            start_pos: None,
            end_pos: [0.0; 3],
            distance_traveled_m: 0.0,
            rule_daylight: true,
            rule_weather: true,
            rule_fire: true,
            physical_size: (0, 0),
            logical_size: [1280.0, 720.0],
            scale_factor: 1.0,
            overall: FrameDistributionAcc::default(),
            post_warmup: FrameDistributionAcc::default(),
            sim_step_ms: SeriesAccumulator::default(),
            gpu_passes: BTreeMap::new(),
            chunks_loaded: ScalarAccumulator::default(),
            active_cells: ScalarAccumulator::default(),
            tracked_cells: ScalarAccumulator::default(),
            dirty_chunks: ScalarAccumulator::default(),
            mem_used_mib: ScalarAccumulator::default(),
            mem_allocated_mib: ScalarAccumulator::default(),
            alloc_rate_mib_s: ScalarAccumulator::default(),
            mem_total_mib: 0,
            worst_frames: Vec::new(),
            timeline: Vec::new(),
            flushed: false,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.config.is_enabled()
    }

    pub fn record_fixed_step(&mut self, sim_step_ms: f32) {
        if !self.config.is_enabled() {
            return;
        }
        self.total_fixed_ticks += 1;
        self.latest_sim_ms = Some(sim_step_ms);
        self.sim_step_ms.push(sim_step_ms);
    }

    pub fn record_frame(&mut self, input: FrameTelemetryInput<'_>) {
        if !self.config.is_enabled() {
            return;
        }
        let host = read_host_render_stats();
        let mem = read_process_memory();
        self.record_frame_with_sources(input, host.as_ref(), mem);
    }

    pub(crate) fn record_frame_with_sources(
        &mut self,
        input: FrameTelemetryInput<'_>,
        host: Option<&HostRenderStats>,
        mem: Option<ProcessMemoryStats>,
    ) {
        if !self.config.is_enabled() {
            return;
        }

        let now = Instant::now();
        let wall_interval_ms = self
            .last_frame_instant
            .map(|prev| now.duration_since(prev).as_secs_f32() * 1000.0);
        self.last_frame_instant = Some(now);
        let elapsed_sec = now.duration_since(self.started_instant).as_secs_f64();

        // Main-loop frame cadence is measured from wall clock between frame ticks (matching `timeline.frames`),
        // never mixed with the render thread's fence-inclusive `frame_time_ms`.
        let main_ms = wall_interval_ms
            .filter(|&ms| ms > 0.0)
            .unwrap_or_else(|| (input.dt.max(1e-4) * 1000.0).min(1000.0))
            .max(input.logic_tick_ms);

        // Deduplicate render-thread & Vulkan GPU timings strictly per completed `render_seq`.
        let new_render =
            host.filter(|h| h.render_seq > 0 && Some(h.render_seq) != self.last_render_seq);
        if let Some(h) = new_render {
            self.last_render_seq = Some(h.render_seq);
        }
        let render_seq = new_render.map(|h| h.render_seq);
        let render_ms = new_render.and_then(|h| (h.frame_time_ms > 0.0).then_some(h.frame_time_ms));
        let gpu_frame_ms = new_render.and_then(|h| h.gpu_frame_ms);
        let gpu_total_ms = new_render.and_then(|h| h.gpu_total_ms);

        let frame_idx = self.total_frames;
        self.total_frames += 1;
        if frame_idx == 0 {
            self.first_frame_ms = Some(round3(main_ms));
        }

        let is_warmup = frame_idx < self.config.warmup_frames;
        if is_warmup {
            self.warmup_max_ms = self.warmup_max_ms.max(render_ms.unwrap_or(main_ms));
        }

        self.overall.push_main(main_ms, input.logic_tick_ms);
        if !is_warmup {
            self.post_warmup.push_main(main_ms, input.logic_tick_ms);
        }

        let mut top_gpu_pass = None;
        if let Some(h) = new_render {
            self.overall
                .push_render(render_ms, gpu_frame_ms, gpu_total_ms);
            if !is_warmup {
                self.post_warmup
                    .push_render(render_ms, gpu_frame_ms, gpu_total_ms);
            }
            if h.physical_size.0 > 0 && h.physical_size.1 > 0 {
                self.physical_size = h.physical_size;
                self.scale_factor = h.scale_factor.max(0.5);
            }
            let pass_slice = if !h.all_gpu_passes.is_empty() {
                &h.all_gpu_passes
            } else {
                &h.top_gpu_passes
            };
            for (name, ms) in pass_slice {
                if top_gpu_pass.is_none() {
                    top_gpu_pass = Some(PassPeak {
                        name: name.clone(),
                        ms: round3(*ms),
                    });
                }
                self.gpu_passes.entry(name.clone()).or_default().push(*ms);
            }
        } else if let Some(h) = host {
            if h.physical_size.0 > 0 && h.physical_size.1 > 0 {
                self.physical_size = h.physical_size;
                self.scale_factor = h.scale_factor.max(0.5);
            }
        }
        if input.screen_logical[0] > 1.0 && input.screen_logical[1] > 1.0 {
            self.logical_size = input.screen_logical;
        }

        // World telemetry updates
        if self.start_time_ticks.is_none() {
            self.start_time_ticks = Some(input.world_time_ticks);
        }
        self.end_time_ticks = input.world_time_ticks;
        if let Some(prev) = self.start_pos {
            let _ = prev;
            let dx = (input.player_pos[0] - self.end_pos[0]) as f64;
            let dy = (input.player_pos[1] - self.end_pos[1]) as f64;
            let dz = (input.player_pos[2] - self.end_pos[2]) as f64;
            self.distance_traveled_m += (dx * dx + dy * dy + dz * dz).sqrt();
        } else {
            self.start_pos = Some(input.player_pos);
        }
        self.end_pos = input.player_pos;
        self.gamemode_end = input.gamemode.to_string();
        self.weather_end = input.weather.to_string();
        self.rule_daylight = input.rule_daylight;
        self.rule_weather = input.rule_weather;
        self.rule_fire = input.rule_fire;

        self.chunks_loaded.push(input.chunks_loaded as f64);
        self.active_cells.push(input.active_cells as f64);
        self.tracked_cells.push(input.tracked_cells as f64);
        self.dirty_chunks.push(input.dirty_chunks as f64);
        self.alloc_rate_mib_s.push(input.alloc_rate_mib_s as f64);

        let used_mib = mem.map(|m| m.used_mib).unwrap_or(0);
        if let Some(m) = mem {
            self.mem_total_mib = self.mem_total_mib.max(m.total_mib);
            self.mem_used_mib.push(m.used_mib as f64);
            self.mem_allocated_mib.push(m.allocated_mib as f64);
        }

        // Maintain top-N slowest frames across the session.
        self.push_worst_frame(WorstFrameEntry {
            frame: frame_idx,
            render_seq,
            elapsed_sec: round3_f64(elapsed_sec),
            main_ms: round3(main_ms),
            logic_ms: round3(input.logic_tick_ms),
            render_ms: render_ms.map(round3),
            gpu_ms: gpu_frame_ms.map(round3),
            sim_ms: self.latest_sim_ms.map(round3),
            chunks_loaded: input.chunks_loaded,
            active_cells: input.active_cells,
            tracked_cells: input.tracked_cells,
            dirty_chunks: input.dirty_chunks,
            working_set_mib: used_mib,
            player_pos: [
                round3(input.player_pos[0]),
                round3(input.player_pos[1]),
                round3(input.player_pos[2]),
            ],
            top_gpu_pass,
        });

        // Aggregate 1-second timeline bucket (capped to keep output compact).
        let sec = elapsed_sec.floor().max(0.0) as u64;
        if self.timeline.len() < MAX_TIMELINE_BUCKETS {
            if self.timeline.last().is_none_or(|b| b.second != sec) {
                self.timeline.push(TimelineBucketAcc {
                    second: sec,
                    ..TimelineBucketAcc::default()
                });
            }
            if let Some(bucket) = self.timeline.last_mut() {
                bucket.frames += 1;
                bucket.main_sum_ms += main_ms as f64;
                bucket.main_max_ms = bucket.main_max_ms.max(main_ms);
                bucket.logic_sum_ms += input.logic_tick_ms as f64;
                if let Some(r) = render_ms {
                    bucket.render_frames += 1;
                    bucket.render_sum_ms += r as f64;
                    bucket.render_max_ms = bucket.render_max_ms.max(r);
                }
                if let Some(g) = gpu_frame_ms {
                    bucket.gpu_sum_ms += g as f64;
                    bucket.gpu_count += 1;
                }
                if let Some(s) = self.latest_sim_ms {
                    bucket.sim_sum_ms += s as f64;
                    bucket.sim_count += 1;
                }
                bucket.chunks_end = input.chunks_loaded;
                bucket.active_cells_end = input.active_cells;
                bucket.tracked_cells_end = input.tracked_cells;
                bucket.dirty_chunks_sum += input.dirty_chunks as u64;
                bucket.mem_used_mib_end = used_mib;
                bucket.alloc_rate_mib_s_max =
                    bucket.alloc_rate_mib_s_max.max(input.alloc_rate_mib_s);
            }
        }
    }

    fn push_worst_frame(&mut self, entry: WorstFrameEntry) {
        if self.worst_frames.len() == MAX_WORST_FRAMES
            && self
                .worst_frames
                .last()
                .is_some_and(|w| entry.sort_ms() <= w.sort_ms())
        {
            return;
        }
        self.worst_frames.push(entry);
        self.worst_frames
            .sort_by(|a, b| b.sort_ms().total_cmp(&a.sort_ms()));
        self.worst_frames.truncate(MAX_WORST_FRAMES);
    }

    /// Builds the complete structured report snapshot.
    pub fn build_report(&self) -> EmergencePerfReport {
        let ended_stamp = CalendarStamp::now();
        let duration_sec = round3_f64(self.started_instant.elapsed().as_secs_f64());
        let post_frames = self.post_warmup.main_frame_ms.len() as u64;
        let total_mem_mib = self
            .mem_total_mib
            .max(read_process_memory().map(|m| m.total_mib).unwrap_or(0));

        let build_feature = if cfg!(feature = "player") {
            "player"
        } else {
            "preview-module"
        };

        let gpu_mean_total = self
            .gpu_passes
            .values()
            .filter_map(|acc| acc.summarize().map(|s| s.mean_ms as f64))
            .sum::<f64>();
        let mut gpu_passes_report: Vec<GpuPassReport> = self
            .gpu_passes
            .iter()
            .filter_map(|(name, acc)| {
                let stats = acc.summarize()?;
                let share_pct = if gpu_mean_total > 1e-6 {
                    round1(((stats.mean_ms as f64 / gpu_mean_total) * 100.0) as f32)
                } else {
                    0.0
                };
                Some(GpuPassReport {
                    name: name.clone(),
                    share_pct,
                    stats,
                })
            })
            .collect();
        gpu_passes_report.sort_by(|a, b| b.stats.mean_ms.total_cmp(&a.stats.mean_ms));

        let mem_used_stats = self.mem_used_mib.summarize();
        let peak_mem_pct = mem_used_stats
            .as_ref()
            .filter(|_| total_mem_mib > 0)
            .map(|m| round2(((m.max / total_mem_mib as f64) * 100.0).clamp(0.0, 100.0) as f32))
            .unwrap_or(0.0);

        let (phys_w, phys_h) = if self.physical_size.0 > 0 && self.physical_size.1 > 0 {
            self.physical_size
        } else {
            (
                self.logical_size[0].round() as u32,
                self.logical_size[1].round() as u32,
            )
        };

        EmergencePerfReport {
            format: REPORT_FORMAT,
            version: REPORT_VERSION,
            session: SessionSummary {
                started_at: self.started_stamp.iso_display(),
                ended_at: ended_stamp.iso_display(),
                duration_sec,
                trigger: self.config.trigger,
                warmup_frames_configured: self.config.warmup_frames,
                total_frames: self.total_frames,
                post_warmup_frames: post_frames,
                total_fixed_ticks: self.total_fixed_ticks,
                first_frame_ms: self.first_frame_ms,
                warmup_max_ms: (self.total_frames > 0 && self.config.warmup_frames > 0)
                    .then(|| round3(self.warmup_max_ms)),
            },
            system: SystemInfo {
                app: "Emergence 0.1.0 (0.1.0/prisma)",
                build_feature,
                cpu_summary: cpu_summary_line().to_string(),
                cpu_brand: read_cpu_brand(),
                logical_cores: std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(1),
                total_mem_mib,
                display_physical_px: [phys_w, phys_h],
                window_logical_px: [
                    self.logical_size[0].round() as u32,
                    self.logical_size[1].round() as u32,
                ],
                scale_factor: round3_f64(self.scale_factor),
            },
            world: WorldSummary {
                seed: self.world_seed,
                gamemode: if self.gamemode_end.is_empty() {
                    "Creative".to_string()
                } else {
                    self.gamemode_end.clone()
                },
                weather: if self.weather_end.is_empty() {
                    "Clear".to_string()
                } else {
                    self.weather_end.clone()
                },
                start_time_ticks: self.start_time_ticks.unwrap_or(self.end_time_ticks),
                end_time_ticks: self.end_time_ticks,
                start_pos: self.start_pos.unwrap_or(self.end_pos).map(round3),
                end_pos: self.end_pos.map(round3),
                distance_traveled_m: round3_f64(self.distance_traveled_m),
                rules: WorldRulesSummary {
                    do_daylight_cycle: self.rule_daylight,
                    do_weather_cycle: self.rule_weather,
                    do_fire_tick: self.rule_fire,
                },
            },
            frame_timing: FrameTimingSummary {
                histogram_edges_ms: &HISTOGRAM_EDGES_MS,
                overall: self.overall.summarize(),
                post_warmup: (post_frames > 0).then(|| self.post_warmup.summarize()),
            },
            gpu_passes: gpu_passes_report,
            simulation: SimulationSummary {
                fixed_hz: 60,
                ticks_executed: self.total_fixed_ticks,
                sim_step_ms: self.sim_step_ms.summarize(),
                chunks_loaded: self.chunks_loaded.summarize(),
                active_cells: self.active_cells.summarize(),
                tracked_cells: self.tracked_cells.summarize(),
                dirty_chunks: self.dirty_chunks.summarize(),
            },
            memory: MemorySummary {
                working_set_mib: mem_used_stats,
                allocated_commit_mib: self.mem_allocated_mib.summarize(),
                alloc_rate_mib_s: self.alloc_rate_mib_s.summarize(),
                peak_working_set_pct: peak_mem_pct,
            },
            worst_frames: self.worst_frames.clone(),
            timeline: self
                .timeline
                .iter()
                .map(TimelineBucketAcc::freeze)
                .collect(),
        }
    }

    /// Writes the single report JSON atomically to disk and returns the target path.
    pub fn flush(&mut self) -> io::Result<Option<PathBuf>> {
        if self.flushed || !self.config.is_enabled() {
            return Ok(None);
        }
        self.flushed = true;

        // Pull one final render stats snapshot if exit occurred before the next tick read it back.
        if let Some(host) = read_host_render_stats() {
            if self.physical_size == (0, 0) && host.physical_size.0 > 0 {
                self.physical_size = host.physical_size;
                self.scale_factor = host.scale_factor.max(0.5);
            }
        }

        let pid = std::process::id();
        let stamp = self.started_stamp.compact_for_filename();
        let Some(path) = resolve_output_path(&self.config, &stamp, pid) else {
            return Ok(None);
        };
        let report = self.build_report();
        write_report_atomic(&path, &report)?;
        Ok(Some(path))
    }
}

impl Drop for EmergencePerfRecorder {
    fn drop(&mut self) {
        if !self.config.is_enabled() || self.flushed {
            return;
        }
        match self.flush() {
            Ok(Some(path)) => {
                let msg = format!(
                    "[emergence][perf] wrote performance report -> {}",
                    path.display()
                );
                log::info!("{msg}");
                eprintln!("{msg}");
            }
            Ok(None) => {}
            Err(err) => {
                log::error!("[emergence][perf] failed to write performance report: {err}");
                eprintln!("[emergence][perf] failed to write performance report: {err}");
            }
        }
    }
}

fn write_report_atomic(path: &Path, report: &EmergencePerfReport) -> io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(report)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "emergence_perf.json".to_string());
    let temp = path.with_file_name(format!("{file_name}.tmp-{}", std::process::id()));
    fs::write(&temp, json.as_bytes())?;
    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&temp);
            Err(err)
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EmergencePerfReport {
    pub format: &'static str,
    pub version: u32,
    pub session: SessionSummary,
    pub system: SystemInfo,
    pub world: WorldSummary,
    pub frame_timing: FrameTimingSummary,
    pub gpu_passes: Vec<GpuPassReport>,
    pub simulation: SimulationSummary,
    pub memory: MemorySummary,
    pub worst_frames: Vec<WorstFrameEntry>,
    pub timeline: Vec<TimelineBucket>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    pub started_at: String,
    pub ended_at: String,
    pub duration_sec: f64,
    pub trigger: &'static str,
    pub warmup_frames_configured: u64,
    pub total_frames: u64,
    pub post_warmup_frames: u64,
    pub total_fixed_ticks: u64,
    pub first_frame_ms: Option<f32>,
    pub warmup_max_ms: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemInfo {
    pub app: &'static str,
    pub build_feature: &'static str,
    pub cpu_summary: String,
    pub cpu_brand: String,
    pub logical_cores: usize,
    pub total_mem_mib: u64,
    pub display_physical_px: [u32; 2],
    pub window_logical_px: [u32; 2],
    pub scale_factor: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorldSummary {
    pub seed: u64,
    pub gamemode: String,
    pub weather: String,
    pub start_time_ticks: u64,
    pub end_time_ticks: u64,
    pub start_pos: [f32; 3],
    pub end_pos: [f32; 3],
    pub distance_traveled_m: f64,
    pub rules: WorldRulesSummary,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorldRulesSummary {
    pub do_daylight_cycle: bool,
    pub do_weather_cycle: bool,
    pub do_fire_tick: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FrameTimingSummary {
    pub histogram_edges_ms: &'static [f32],
    pub overall: FrameDistribution,
    pub post_warmup: Option<FrameDistribution>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FrameDistribution {
    pub frames: u64,
    pub render_frames: u64,
    pub fps: Option<FpsStats>,
    pub render_fps: Option<FpsStats>,
    pub main_frame_ms: Option<SeriesStats>,
    pub logic_tick_ms: Option<SeriesStats>,
    pub render_thread_ms: Option<SeriesStats>,
    pub gpu_frame_ms: Option<SeriesStats>,
    pub gpu_passes_total_ms: Option<SeriesStats>,
    pub render_hitches: HitchCounts,
}

#[derive(Debug, Clone, Serialize)]
pub struct HitchCounts {
    pub over_16_67ms: u64,
    pub over_33_33ms: u64,
    pub over_50ms: u64,
    pub over_100ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FpsStats {
    pub avg: f32,
    pub p50: f32,
    pub p5_frame_time: f32,
    pub min: f32,
    pub max: f32,
    pub low_1pct: f32,
    pub low_0_1pct: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeriesStats {
    pub samples: usize,
    pub min_ms: f32,
    pub mean_ms: f32,
    pub p50_ms: f32,
    pub p95_ms: f32,
    pub p99_ms: f32,
    pub p99_9_ms: f32,
    pub max_ms: f32,
    pub stddev_ms: f32,
    pub histogram_counts: Vec<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GpuPassReport {
    pub name: String,
    pub share_pct: f32,
    #[serde(flatten)]
    pub stats: SeriesStats,
}

#[derive(Debug, Clone, Serialize)]
pub struct SimulationSummary {
    pub fixed_hz: u32,
    pub ticks_executed: u64,
    pub sim_step_ms: Option<SeriesStats>,
    pub chunks_loaded: Option<ScalarStats>,
    pub active_cells: Option<ScalarStats>,
    pub tracked_cells: Option<ScalarStats>,
    pub dirty_chunks: Option<ScalarStats>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemorySummary {
    pub working_set_mib: Option<ScalarStats>,
    pub allocated_commit_mib: Option<ScalarStats>,
    pub alloc_rate_mib_s: Option<ScalarStats>,
    pub peak_working_set_pct: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScalarStats {
    pub samples: u64,
    pub start: f64,
    pub end: f64,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PassPeak {
    pub name: String,
    pub ms: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorstFrameEntry {
    pub frame: u64,
    pub render_seq: Option<u64>,
    pub elapsed_sec: f64,
    pub main_ms: f32,
    pub logic_ms: f32,
    pub render_ms: Option<f32>,
    pub gpu_ms: Option<f32>,
    pub sim_ms: Option<f32>,
    pub chunks_loaded: usize,
    pub active_cells: usize,
    pub tracked_cells: usize,
    pub dirty_chunks: usize,
    pub working_set_mib: u64,
    pub player_pos: [f32; 3],
    pub top_gpu_pass: Option<PassPeak>,
}

impl WorstFrameEntry {
    fn sort_ms(&self) -> f32 {
        self.render_ms.unwrap_or(self.main_ms).max(self.main_ms)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TimelineBucket {
    pub second: u64,
    pub frames: u64,
    pub fps_avg: f32,
    pub main_mean_ms: f64,
    pub main_max_ms: f32,
    pub logic_mean_ms: f64,
    pub render_frames: u64,
    pub render_fps_avg: Option<f32>,
    pub render_mean_ms: Option<f64>,
    pub render_max_ms: Option<f32>,
    pub gpu_mean_ms: Option<f64>,
    pub sim_mean_ms: Option<f64>,
    pub chunks_loaded: usize,
    pub active_cells: usize,
    pub tracked_cells: usize,
    pub dirty_chunks_total: u64,
    pub working_set_mib: u64,
    pub alloc_rate_mib_s: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "emergence-perf-test-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create temp test dir");
        dir
    }

    #[test]
    fn cli_parser_supports_default_cache_custom_path_warmup_and_help() {
        assert_eq!(
            parse_cli_args(["emergence.exe", "--help"]).unwrap(),
            CliAction::PrintHelpAndExit
        );

        let CliAction::Run(default_cfg) =
            parse_cli_args(["emergence.exe", "--perf-report"]).unwrap()
        else {
            panic!("expected Run");
        };
        assert_eq!(default_cfg.target, PerfOutputTarget::ProjectCacheAuto);
        assert_eq!(default_cfg.trigger, "cli");
        assert_eq!(default_cfg.warmup_frames, DEFAULT_WARMUP_FRAMES);

        let CliAction::Run(short_cfg) =
            parse_cli_args(["emergence.exe", "--perf", "--perf-warmup=15"]).unwrap()
        else {
            panic!("expected Run");
        };
        assert_eq!(short_cfg.target, PerfOutputTarget::ProjectCacheAuto);
        assert_eq!(short_cfg.warmup_frames, 15);

        let CliAction::Run(custom_cfg) =
            parse_cli_args(["emergence.exe", "--perf-out", ".cache/perf/custom.json"]).unwrap()
        else {
            panic!("expected Run");
        };
        assert_eq!(
            custom_cfg.target,
            PerfOutputTarget::CustomFile(PathBuf::from(".cache/perf/custom.json"))
        );
    }

    #[test]
    fn resolve_project_root_finds_emergence_from_repo_root_and_exe_dir() {
        let tmp = unique_temp_dir("root");
        let proj = tmp.join("projects").join("emergence");
        let exe_dir = proj.join("target").join("debug");
        fs::create_dir_all(proj.join("assets").join("data")).unwrap();
        fs::create_dir_all(&exe_dir).unwrap();
        fs::write(proj.join("Cargo.toml"), "[package]\nname=\"emergence\"\n").unwrap();
        fs::write(
            proj.join("assets").join("data").join("materials.json"),
            "{}",
        )
        .unwrap();

        // Launched from repo root (`cargo run --manifest-path projects/emergence/Cargo.toml`).
        let from_repo = resolve_project_root_from(None, Some(&tmp), Some(&exe_dir));
        assert_eq!(from_repo, proj);

        // Launched directly from target/debug even with an unrelated cwd.
        let other = tmp.join("unrelated");
        fs::create_dir_all(&other).unwrap();
        let from_exe = resolve_project_root_from(None, Some(&other), Some(&exe_dir));
        assert_eq!(from_exe, proj);
        assert_eq!(
            default_perf_cache_dir()
                .file_name()
                .and_then(|n| n.to_str()),
            Some("perf")
        );
        assert_eq!(
            default_perf_cache_dir()
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str()),
            Some(".cache")
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn collector_records_f3_metrics_and_writes_single_report_on_drop() {
        let tmp = unique_temp_dir("drop");
        let out_dir = tmp.join(".cache").join("perf");

        let cfg = PerfReportConfig {
            target: PerfOutputTarget::CustomDir(out_dir.clone()),
            warmup_frames: 2,
            trigger: "cli",
        };

        {
            let mut rec = EmergencePerfRecorder::with_config(cfg, 2026);
            rec.record_fixed_step(1.25);
            rec.record_fixed_step(2.50);

            for i in 0..6u32 {
                // Intentionally emit duplicate `render_seq` values (only 3 distinct GPU frames across 6 main ticks)
                // to verify render-thread & GPU pass metrics are deduplicated strictly per completed Vulkan frame.
                let render_seq = match i {
                    0 => 1,
                    1 | 2 => 2,
                    _ => 3,
                };
                let render_ms = match render_seq {
                    1 => 42.0,
                    2 => 18.0,
                    _ => 9.0,
                };
                let host = HostRenderStats {
                    render_seq,
                    fps: 1000.0 / render_ms,
                    frame_time_ms: render_ms,
                    gpu_frame_ms: Some(render_ms * 0.65),
                    gpu_total_ms: Some(render_ms * 0.60),
                    top_gpu_passes: vec![("VoxelDdaPass".to_string(), render_ms * 0.45)],
                    all_gpu_passes: vec![
                        ("VoxelDdaPass".to_string(), render_ms * 0.45),
                        ("PostPass".to_string(), render_ms * 0.15),
                    ],
                    cpu_p95_ms: Some(render_ms),
                    low_1pct_fps: Some(30.0),
                    physical_size: (1280, 720),
                    scale_factor: 1.0,
                };
                rec.record_frame_with_sources(
                    FrameTelemetryInput {
                        dt: 0.012,
                        logic_tick_ms: 0.4 + i as f32 * 0.1,
                        chunks_loaded: 16 + i as usize,
                        active_cells: 120 + i as usize * 10,
                        tracked_cells: 4000,
                        dirty_chunks: i as usize,
                        screen_logical: [1280.0, 720.0],
                        alloc_rate_mib_s: 4,
                        player_pos: [6.5 + i as f32, 4.0, 6.5],
                        gamemode: "Creative",
                        weather: "Clear",
                        world_time_ticks: 6000 + i as u64,
                        rule_daylight: true,
                        rule_weather: true,
                        rule_fire: true,
                    },
                    Some(&host),
                    Some(ProcessMemoryStats {
                        used_mib: 420 + i as u64 * 4,
                        allocated_mib: 640,
                        total_mib: 16384,
                    }),
                );
            }
        }

        let files: Vec<PathBuf> = fs::read_dir(&out_dir)
            .expect("perf dir created on exit")
            .filter_map(|e| e.ok().map(|entry| entry.path()))
            .collect();
        assert_eq!(
            files.len(),
            1,
            "expected exactly 1 report file, got {files:?}"
        );
        let fname = files[0].file_name().unwrap().to_string_lossy();
        assert!(fname.starts_with("emergence_perf_"), "got {fname}");
        assert!(fname.ends_with(".json"), "got {fname}");

        let raw = fs::read_to_string(&files[0]).unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(val["format"], REPORT_FORMAT);
        assert_eq!(val["version"], REPORT_VERSION);
        assert_eq!(val["session"]["total_frames"], 6);
        assert_eq!(val["session"]["post_warmup_frames"], 4);
        assert_eq!(val["session"]["total_fixed_ticks"], 2);
        assert_eq!(val["frame_timing"]["overall"]["frames"], 6);
        assert_eq!(val["frame_timing"]["overall"]["render_frames"], 3);
        assert_eq!(val["gpu_passes"][0]["name"], "VoxelDdaPass");
        assert_eq!(val["gpu_passes"][0]["samples"], 3);
        assert_eq!(val["gpu_passes"][1]["name"], "PostPass");
        assert_eq!(val["gpu_passes"][1]["samples"], 3);
        assert_eq!(val["simulation"]["chunks_loaded"]["start"], 16.0);
        assert_eq!(val["simulation"]["chunks_loaded"]["end"], 21.0);
        assert_eq!(val["simulation"]["dirty_chunks"]["max"], 5.0);
        assert_eq!(val["worst_frames"][0]["frame"], 0);
        assert_eq!(val["worst_frames"][0]["render_seq"], 1);
        let bucket = &val["timeline"][0];
        assert_eq!(bucket["frames"], 6);
        assert_eq!(bucket["render_frames"], 3);
        assert!(bucket["fps_avg"].as_f64().is_some());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn disabled_collector_writes_no_files_on_drop() {
        let tmp = unique_temp_dir("disabled");
        let nonexistent = tmp.join(".cache").join("perf");
        {
            let mut rec = EmergencePerfRecorder::with_config(PerfReportConfig::default(), 2026);
            rec.record_fixed_step(1.0);
        }
        assert!(!nonexistent.exists());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn naming_and_utc_calendar_formatting_stay_lexicographically_sortable() {
        let s1 = CalendarStamp::from_unix_utc(1_763_413_345);
        let name = format_report_filename(&s1.compact_for_filename(), 42);
        assert!(name.starts_with("emergence_perf_20"));
        assert!(name.ends_with("_pid42.json"));
    }
}
