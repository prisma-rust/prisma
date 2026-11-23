//! Render graph report (JSON) — export runtime `RenderGraphSnapshot` into **pure data**,
//! for editor / external tools to visualize as DAG.
//!
//! ```text
//! Render thread GraphRenderer ──snapshot()──► RenderGraphReport ──JSON──► editor panel
//! ```
//!
//! Here only DTO and serialization: **no** `vk::*` handles, no dependency on ECS / engine settings types.
//! Engine's `RenderSettings` → [`ReportSettings`] mapping lives in `prism-engine`
//! (`RenderSettings::to_report_settings`), so this module lets `prism-app` and
//! `editor` share one implementation.
//!
//! Writing file uses "temp file + rename", avoid readers seeing half-written JSON.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::render_graph::{PassInfo, PassKind, RenderGraphSnapshot, RenderMode, ResourceHandle};

/// Env var key for game process report output path (editor writes local file itself, ignores it).
pub const ENV_KEY: &str = "PRISMA_RENDER_GRAPH_REPORT";

/// Refresh report every few frames (≈1 second @60fps; graph structure doesn't change frequently).
pub const REFRESH_INTERVAL_FRAMES: u64 = 60;

/// Manager-side real-time resource stats (#518 Phase 2).
///
/// Different from `resources`: those are targets allocated by render graph itself; here counts
/// **resident amount** of textures / meshes / materials (accumulated over renderer lifetime, drops after hot reload). Byte count is estimate:
/// texture counts base mip only (not multiplied by mip chain), mesh uses standard vertex layout + 4-byte index.
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReportStats {
    pub textures: usize,
    pub texture_bytes: u64,
    pub meshes: usize,
    pub mesh_bytes: u64,
    pub materials: usize,
    pub material_bytes: u64,
    /// Occlusion culling tested / culled draw counts (#1007); 0 when disabled.
    #[serde(default)]
    pub occlusion_tested: usize,
    #[serde(default)]
    pub occlusion_culled: usize,
    /// VRAM suballocation stats (#1030): total reserved VRAM bytes and active used bytes.
    #[serde(default)]
    pub suballoc_reserved_bytes: u64,
    #[serde(default)]
    pub suballoc_active_bytes: u64,
}

impl ReportStats {
    /// Estimated byte count of all three resource classes combined.
    pub fn total_bytes(&self) -> u64 {
        self.texture_bytes + self.mesh_bytes + self.material_bytes
    }
}

/// One draw call record within a frame (#518 Phase 2 Frame Debugger data layer).
///
/// Only records info determinable on CPU side (pass / batch tag / instance count / triangle count / whether visible in main
/// channel), no pipeline handle or descriptor set — replay left for later.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReportDrawCall {
    /// `forward` / `sprite` / `particle`.
    pub pass: String,
    /// Human-readable tag (mesh index + material slot / blend state / particle group).
    pub label: String,
    /// Instance count (mesh usually 1, sprite / particle is instance count of same batch).
    pub instances: usize,
    /// Triangle count (instances already multiplied in).
    pub triangles: u64,
    /// Whether drawn in camera main channel (shadow-only / culled mesh is `false`).
    pub visible: bool,
    /// Pipeline state summary (queue / cull / depth write / blend); absent in old reports.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Pipeline class (`forward/opaque`, `forward/transparent`, `sprite/batch`...);
    /// absent in old reports (#518 Phase 2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pipeline: Option<String>,
    /// Names of descriptor sets bound by this call (by set index); omitted in old reports.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub descriptor_sets: Vec<String>,
    /// Vertex count (mesh is vertex count at upload; sprite / particle is instance count × 4); omitted in old reports.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vertices: Option<u64>,
}

/// Max draw calls kept in report (prevent huge scenes from blowing up JSON).
pub const MAX_REPORTED_DRAW_CALLS: usize = 256;

/// JSON shape of `RenderGraphSnapshot`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderGraphReport {
    /// Unix millisecond timestamp.
    pub generated_at_ms: u64,
    /// Frame number at sampling.
    pub frame: u64,
    /// Render settings at sampling.
    pub settings: ReportSettings,
    /// Passes in execution order.
    pub passes: Vec<ReportPass>,
    /// Graph-managed resources.
    pub resources: Vec<ReportResource>,
    /// Total GPU time of previous frame (ms, #518). `None` when device lacks timestamp support.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_total_ms: Option<f32>,
    /// Real-time resident stats of textures / meshes / materials (#518 Phase 2); absent in old reports.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<ReportStats>,
    /// Draw call list of this frame (#518 Phase 2); omitted when empty / old report.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub draw_calls: Vec<ReportDrawCall>,
    /// Total GPU time of recent frames (ms, old → new, #518 Phase 2 history chart);
    /// omitted when device lacks timestamps / old report.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gpu_history_ms: Vec<f32>,
}

/// Subset of render settings shown in report (aligned with editor `RenderSettingsData`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportSettings {
    pub render_mode: &'static str,
    pub debug_flags: u32,
    pub tonemap_mode: u32,
    pub debug_rt: u32,
    pub pt_max_bounces: u32,
    pub pt_ray_max_distance: f32,
    pub pt_max_iterations: u32,
}

impl ReportSettings {
    /// Construct from original fields (`prism-engine`'s `RenderSettings` uses this to fill table).
    pub fn new(
        render_mode: RenderMode,
        debug_flags: u32,
        tonemap_mode: u32,
        debug_rt: u32,
        pt_max_bounces: u32,
        pt_ray_max_distance: f32,
        pt_max_iterations: u32,
    ) -> Self {
        Self {
            render_mode: match render_mode {
                RenderMode::Raster => "raster",
                RenderMode::PathTrace => "path_trace",
                RenderMode::Sprites2D => "sprites_2d",
            },
            debug_flags,
            tonemap_mode,
            debug_rt,
            pt_max_bounces,
            pt_ray_max_distance,
            pt_max_iterations,
        }
    }
}

impl Default for ReportSettings {
    fn default() -> Self {
        Self::new(RenderMode::Raster, 0, 0, 0, 3, 1000.0, 0)
    }
}

/// Node data of one pass.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportPass {
    pub index: usize,
    pub name: String,
    /// `shadow` / `forward` / `gtao` / `post` / `pt` / `rt` / `unknown`.
    pub kind: &'static str,
    pub inputs: Vec<ReportHandle>,
    pub outputs: Vec<ReportHandle>,
    /// GPU time of this pass on previous frame (ms, #518); `None` when no timing data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_ms: Option<f32>,
}

/// Node data of one resource.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportResource {
    pub handle: u32,
    /// Readable name (well-known handles have names; others `resource#N`).
    pub name: String,
    /// `color` / `depth` / `storage_image` / `storage_buffer` / `other`.
    pub kind: &'static str,
    /// Like `R16G16B16A16_SFLOAT`, may be empty.
    pub format: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub sample_count: Option<u32>,
    pub size_bytes: Option<u64>,
    pub allocated: bool,
}

/// Edge reference: the handle itself (frontend connects passes by handle).
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(transparent)]
pub struct ReportHandle(pub u32);

impl RenderGraphReport {
    /// Build report from snapshot. `settings` is render settings actually in effect this frame,
    /// `frame` is caller's frame counter.
    pub fn from_snapshot(
        snapshot: &RenderGraphSnapshot,
        settings: ReportSettings,
        frame: u64,
    ) -> Self {
        Self::from_snapshot_with_timings(snapshot, settings, frame, None)
    }

    /// Same as [`Self::from_snapshot`], but with per-pass GPU times (#518).
    pub fn from_snapshot_with_timings(
        snapshot: &RenderGraphSnapshot,
        settings: ReportSettings,
        frame: u64,
        timings: Option<&crate::gpu_timer::GpuTimings>,
    ) -> Self {
        let passes = snapshot
            .passes
            .iter()
            .map(|pass| ReportPass {
                index: pass.index,
                name: pass.name.clone(),
                kind: pass_kind(pass),
                gpu_ms: timings.and_then(|timings| timings.pass_ms(&pass.name)),
                inputs: visible_inputs(pass, &settings)
                    .into_iter()
                    .map(|h| ReportHandle(h.0))
                    .collect(),
                outputs: pass.outputs.iter().map(|h| ReportHandle(h.0)).collect(),
            })
            .collect();

        let resources = snapshot
            .resources
            .iter()
            .map(|res| {
                let (kind, format, width, height, sample_count, size_bytes) = match &res.res_type {
                    crate::render_graph::ResourceType::ColorAttachment {
                        format,
                        extent,
                        sample_count,
                    } => (
                        "color",
                        Some(format!("{format:?}")),
                        Some(extent.width),
                        Some(extent.height),
                        Some(sample_count.as_raw()),
                        None,
                    ),
                    crate::render_graph::ResourceType::DepthAttachment {
                        extent,
                        sample_count,
                    } => (
                        "depth",
                        None,
                        Some(extent.width),
                        Some(extent.height),
                        Some(sample_count.as_raw()),
                        None,
                    ),
                    crate::render_graph::ResourceType::StorageImage { format, extent } => (
                        "storage_image",
                        Some(format!("{format:?}")),
                        Some(extent.width),
                        Some(extent.height),
                        None,
                        None,
                    ),
                    crate::render_graph::ResourceType::StorageBuffer { size } => {
                        ("storage_buffer", None, None, None, None, Some(*size))
                    }
                };
                ReportResource {
                    handle: res.handle.0,
                    name: resource_name(res.handle),
                    kind,
                    format,
                    width,
                    height,
                    sample_count,
                    size_bytes,
                    allocated: res.allocated,
                }
            })
            .collect();

        Self {
            generated_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            frame,
            settings,
            passes,
            resources,
            gpu_total_ms: timings
                .map(|timings| timings.total_ms)
                .filter(|total| *total > 0.0),
            stats: None,
            draw_calls: Vec::new(),
            gpu_history_ms: Vec::new(),
        }
    }

    /// Attach total GPU times of recent N frames (#518 Phase 2 history chart).
    ///
    /// Non-finite / negative samples discarded; at most 240 points kept, avoid exceptional caller bloating JSON.
    pub fn with_gpu_history(mut self, samples: impl IntoIterator<Item = f32>) -> Self {
        self.gpu_history_ms = samples
            .into_iter()
            .filter(|ms| ms.is_finite() && *ms >= 0.0)
            .take(240)
            .collect();
        self
    }

    /// Attach manager-side resource stats (#518 Phase 2).
    pub fn with_stats(mut self, stats: ReportStats) -> Self {
        self.stats = Some(stats);
        self
    }

    /// Attach draw call list of this frame (#518 Phase 2 Frame Debugger).
    pub fn with_draw_calls(mut self, calls: Vec<ReportDrawCall>) -> Self {
        self.draw_calls = calls;
        self
    }

    /// Atomically write JSON (temp file + rename).
    pub fn write_json(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)
    }
}

/// Report section: only writes file when env var exists and refresh interval reached.
///
/// Game process uses `from_env()`; editor decides output path itself (panel wants local file).
pub struct ReportWriter {
    path: Option<std::path::PathBuf>,
    last_frame: Option<u64>,
}

impl ReportWriter {
    /// Read env var; when unset returns a writer that never writes.
    pub fn from_env() -> Self {
        Self {
            path: std::env::var_os(ENV_KEY).map(std::path::PathBuf::from),
            last_frame: None,
        }
    }

    /// Specify output path directly (for editor).
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            path: Some(path.into()),
            last_frame: None,
        }
    }

    /// Disable writing (when editor has no project root yet).
    pub fn disabled() -> Self {
        Self {
            path: None,
            last_frame: None,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.path.is_some()
    }

    /// First frame + once every [`REFRESH_INTERVAL_FRAMES`] frames.
    pub fn maybe_write(
        &mut self,
        snapshot: &RenderGraphSnapshot,
        settings: ReportSettings,
        frame: u64,
    ) {
        self.maybe_write_with_timings(
            snapshot,
            settings,
            frame,
            None,
            None,
            Vec::new(),
            Vec::new(),
        );
    }

    /// Same as [`Self::maybe_write`], with per-pass GPU times (#518).
    #[expect(
        clippy::too_many_arguments,
        reason = "timing report pulls scene, frame and pass data in one call"
    )]
    pub fn maybe_write_with_timings(
        &mut self,
        snapshot: &RenderGraphSnapshot,
        settings: ReportSettings,
        frame: u64,
        timings: Option<&crate::gpu_timer::GpuTimings>,
        stats: Option<ReportStats>,
        draw_calls: Vec<ReportDrawCall>,
        gpu_history: Vec<f32>,
    ) {
        let Some(path) = self.path.as_deref() else {
            return;
        };
        let due = match self.last_frame {
            None => true,
            Some(last) => frame.saturating_sub(last) >= REFRESH_INTERVAL_FRAMES,
        };
        if !due {
            return;
        }
        self.last_frame = Some(frame);

        let mut report =
            RenderGraphReport::from_snapshot_with_timings(snapshot, settings, frame, timings);
        if let Some(stats) = stats {
            report = report.with_stats(stats);
        }
        if !draw_calls.is_empty() {
            report = report.with_draw_calls(draw_calls);
        }
        if !gpu_history.is_empty() {
            report = report.with_gpu_history(gpu_history);
        }
        if let Err(error) = report.write_json(path) {
            log::warn!(
                "render graph report write failed ({}): {error}",
                path.display()
            );
        }
    }
}

fn pass_kind(pass: &PassInfo) -> &'static str {
    match pass.kind {
        PassKind::Shadow => "shadow",
        PassKind::Forward => "forward",
        PassKind::Gtao => "gtao",
        PassKind::Post => "post",
        PassKind::Pt => "pt",
        PassKind::Rt => "rt",
        PassKind::Anf => "anf",
        PassKind::MobileUpscaler => "mobile-upscaler",
        PassKind::Dlss => "dlss",
        PassKind::RestirGi => "restir-gi",
        PassKind::SharcTrain => "sharc-train",
        PassKind::SharcResolve => "sharc-resolve",
        PassKind::Voxel => "voxel",
        PassKind::Unknown => "unknown",
    }
}

/// Pass inputs shown in report.
///
/// `PostPass`'s `graph_info` only declares rasterization input (`FORWARD_COLOR_H`), but
/// in path-tracing mode actually reads `PT_COLOR_H`. Graph structure declared by passes,
/// here correct visual edges according to current render mode.
fn visible_inputs(pass: &PassInfo, settings: &ReportSettings) -> Vec<ResourceHandle> {
    if pass.kind == PassKind::Post && settings.render_mode == "path_trace" {
        vec![crate::render_graph::PT_COLOR_H]
    } else {
        pass.inputs.clone()
    }
}

fn resource_name(handle: ResourceHandle) -> String {
    match handle.0 {
        handle if handle == crate::render_graph::FORWARD_DEPTH_H.0 => "forward.depth".into(),
        handle if handle == crate::render_graph::FORWARD_NORMAL_H.0 => "forward.normal".into(),
        handle if handle == crate::render_graph::FORWARD_COLOR_H.0 => "forward.color".into(),
        handle if handle == crate::render_graph::ANF_COLOR_H.0 => "anf.color".into(),
        handle if handle == crate::render_graph::UPSCALED_COLOR_H.0 => "upscaled.color".into(),
        handle if handle == crate::render_graph::DLSS_COLOR_H.0 => "dlss.color".into(),
        handle if handle == crate::render_graph::PT_COLOR_H.0 => "pt.color".into(),
        handle if handle == crate::render_graph::FORWARD_VELOCITY_H.0 => "forward.velocity".into(),
        handle if handle == crate::render_graph::FORWARD_ALBEDO_H.0 => "forward.albedo".into(),
        handle if handle == crate::render_graph::RESTIR_GI_COLOR_H.0 => "restir_gi.color".into(),
        handle if handle == crate::render_graph::SHARC_HASH_ENTRIES_H.0 => {
            "sharc.hash_entries".into()
        }
        handle if handle == crate::render_graph::SHARC_ACCUMULATION_H.0 => {
            "sharc.accumulation".into()
        }
        handle if handle == crate::render_graph::SHARC_RESOLVED_H.0 => "sharc.resolved".into(),
        handle if handle == crate::render_graph::GTAO_AO_H.0 => "gtao.ao".into(),
        handle if handle == crate::render_graph::VOXEL_COLOR_H.0 => "voxel.color".into(),
        handle if handle == crate::render_graph::VOXEL_DEPTH_H.0 => "voxel.depth".into(),
        handle if handle == crate::render_graph::FINAL_COLOR_H.0 => "final.color".into(),
        handle => format!("resource#{handle}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> RenderGraphSnapshot {
        RenderGraphSnapshot {
            passes: vec![
                PassInfo {
                    index: 0,
                    name: "Forward".into(),
                    kind: PassKind::Forward,
                    inputs: vec![],
                    outputs: vec![crate::render_graph::FORWARD_COLOR_H],
                },
                PassInfo {
                    index: 1,
                    name: "Post".into(),
                    kind: PassKind::Post,
                    inputs: vec![crate::render_graph::FORWARD_COLOR_H],
                    outputs: vec![],
                },
            ],
            resources: vec![],
            settings: Default::default(),
        }
    }

    #[test]
    fn report_serializes_edges_and_settings() {
        let report = RenderGraphReport::from_snapshot(&snapshot(), ReportSettings::default(), 42);
        assert_eq!(report.passes.len(), 2);
        assert_eq!(report.passes[1].inputs[0].0, 1002);
        assert_eq!(report.settings.render_mode, "raster");
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["passes"][0]["kind"], "forward");
    }

    #[test]
    fn draw_calls_are_optional_and_serialize_in_camel_case() {
        let plain = RenderGraphReport::from_snapshot(&snapshot(), ReportSettings::default(), 1);
        assert!(plain.draw_calls.is_empty());
        let json = serde_json::to_value(&plain).unwrap();
        assert!(
            json.get("drawCalls").is_none(),
            "Old report should not gain drawCalls out of nowhere"
        );

        let calls = vec![
            ReportDrawCall {
                pass: "forward".into(),
                label: "mesh #0 · material 2".into(),
                instances: 1,
                triangles: 12,
                visible: true,
                state: Some("queue 2000 · cull Back · z-write true · One/Zero Add".into()),
                pipeline: Some("forward/opaque".into()),
                descriptor_sets: vec!["frame+materials".into(), "bindless-textures".into()],
                vertices: Some(24),
            },
            ReportDrawCall {
                pass: "sprite".into(),
                label: "SrcAlpha / OneMinusSrcAlpha".into(),
                instances: 4,
                triangles: 8,
                visible: true,
                state: None,
                pipeline: None,
                descriptor_sets: Vec::new(),
                vertices: None,
            },
        ];
        let report = RenderGraphReport::from_snapshot(&snapshot(), ReportSettings::default(), 1)
            .with_draw_calls(calls);
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["drawCalls"][0]["pass"], "forward");
        assert!(json["drawCalls"][0]["state"]
            .as_str()
            .unwrap()
            .contains("cull Back"));
        assert!(
            json["drawCalls"][1].get("state").is_none(),
            "Omit field when state is None"
        );
        assert_eq!(json["drawCalls"][1]["instances"], 4);
        assert_eq!(json["drawCalls"][1]["triangles"], 8);
        assert_eq!(json["drawCalls"][0]["pipeline"], "forward/opaque");
        assert_eq!(
            json["drawCalls"][0]["descriptorSets"][1],
            "bindless-textures"
        );
        assert_eq!(json["drawCalls"][0]["vertices"], 24);
        assert!(
            json["drawCalls"][1].get("pipeline").is_none(),
            "Omit field when pipeline is None"
        );
        assert!(
            json["drawCalls"][1].get("descriptorSets").is_none(),
            "Omit field when descriptorSets empty"
        );
    }

    #[test]
    fn resource_stats_are_optional_and_serialize_in_camel_case() {
        let plain = RenderGraphReport::from_snapshot(&snapshot(), ReportSettings::default(), 1);
        assert!(plain.stats.is_none());
        let json = serde_json::to_value(&plain).unwrap();
        assert!(
            json.get("stats").is_none(),
            "Old report should not gain stats out of nowhere"
        );

        let stats = ReportStats {
            textures: 3,
            texture_bytes: 1024,
            meshes: 2,
            mesh_bytes: 2048,
            materials: 4,
            material_bytes: 384,
            occlusion_tested: 0,
            occlusion_culled: 0,
            suballoc_reserved_bytes: 0,
            suballoc_active_bytes: 0,
        };
        assert_eq!(stats.total_bytes(), 3456);
        let report = RenderGraphReport::from_snapshot(&snapshot(), ReportSettings::default(), 1)
            .with_stats(stats);
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["stats"]["textures"], 3);
        assert_eq!(json["stats"]["textureBytes"], 1024);
        assert_eq!(json["stats"]["materialBytes"], 384);
    }

    #[test]
    fn path_trace_mode_rewires_the_post_pass_input() {
        let settings = ReportSettings::new(RenderMode::PathTrace, 0, 0, 0, 3, 1000.0, 0);
        let report = RenderGraphReport::from_snapshot(&snapshot(), settings, 7);
        assert_eq!(report.passes[1].inputs[0].0, 1003);
        assert_eq!(report.settings.render_mode, "path_trace");
    }

    #[test]
    fn well_known_resources_get_names() {
        assert_eq!(
            resource_name(crate::render_graph::FORWARD_COLOR_H),
            "forward.color"
        );
        assert_eq!(resource_name(ResourceHandle(7)), "resource#7");
    }

    #[test]
    fn writer_writes_only_every_interval() {
        let dir = std::env::temp_dir().join("prism-render-graph-report");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("graph.json");
        let mut writer = ReportWriter::new(path.clone());
        let snap = snapshot();

        writer.maybe_write(&snap, ReportSettings::default(), 1);
        assert!(path.is_file());
        let first = std::fs::metadata(&path).unwrap().len();

        writer.maybe_write(&snap, ReportSettings::default(), 30);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), first);
        writer.maybe_write(
            &snap,
            ReportSettings::default(),
            REFRESH_INTERVAL_FRAMES + 1,
        );
        assert!(path.is_file());

        let parsed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(parsed["frame"], REFRESH_INTERVAL_FRAMES + 1);
    }

    #[test]
    fn a_disabled_writer_never_touches_disk() {
        let dir = std::env::temp_dir().join("prism-render-graph-report-off");
        let _ = std::fs::remove_dir_all(&dir);
        let mut writer = ReportWriter::disabled();
        writer.maybe_write(&snapshot(), ReportSettings::default(), 1);
        assert!(!dir.exists());
        assert!(!writer.is_enabled());
    }
}
