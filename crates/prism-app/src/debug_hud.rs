//! Runtime diagnostic HUD (no UI framework dependency).
//!
//! After egui editor removed, screen display of frame rate / render mode / debug state taken over by this module:
//! It only appends text commands to the existing ECS UI overlay ([`prism_ui_draw::UiOverlayInput`]),
//! rasterized uniformly via `prism-render` `ui_overlay` channel, so no external UI
//! framework or extra render pass.
//!
//! Hotkeys (`prism-app` handles):
//! - `F3`: show / hide HUD.

use prism_engine::render_settings::{pbr_flag_label, RenderMode, RenderSettings};
use prism_ui_draw::UiOverlayInput;
use prism_ui_draw::{UiTextAlign, UiTextInput};

use crate::render_shared::RenderStats;

const MARGIN: f32 = 10.0;
const LINE_HEIGHT: f32 = 20.0;
const FONT_SIZE: f32 = 15.0;
const TEXT_COLOR: [f32; 4] = [0.92, 0.92, 0.92, 0.95];
const WARN_COLOR: [f32; 4] = [1.0, 0.72, 0.3, 1.0];

/// Runtime HUD display state.
#[derive(Clone, Copy, Debug, Default)]
pub struct DebugHud {
    /// `F3` toggles.
    pub show: bool,
}

impl DebugHud {
    /// Append current frame diagnostic text to UI overlay.
    ///
    /// `width` / `height` are swapchain logical size (pixels), used to place HUD at top-left
    /// and draw centered "no camera" hint.
    pub fn append(
        &self,
        ui: &mut UiOverlayInput,
        stats: &RenderStats,
        settings: &RenderSettings,
        has_camera: bool,
        width: f32,
        height: f32,
    ) {
        if !self.show {
            return;
        }

        let mut lines = Vec::with_capacity(5);
        let mut line = format!("{:.1} ms  |  {:.0} FPS", stats.frame_time_ms, stats.fps);
        match stats.gpu_timings.as_ref().filter(|gpu| gpu.supported) {
            // When full-frame interval exists use full-frame time, otherwise fall back to pass sum: two metrics must be distinguishable.
            Some(gpu) => line.push_str(&format!("  |  GPU {:.1} ms", gpu.frame_or_total_ms())),
            // Device does not support timestamps: explicitly write n/a, do not fake GPU time with CPU wall clock.
            None => line.push_str("  |  GPU n/a"),
        }
        if settings.render_mode == RenderMode::PathTrace {
            let max = if settings.pt_max_iterations > 0 {
                settings.pt_max_iterations.to_string()
            } else {
                "inf".to_string()
            };
            let samples = stats.pt_frame_count.unwrap_or(0);
            line.push_str(&format!("  |  PT {samples}/{max} smp"));
        }
        lines.push(line);

        // Percentiles need a few dozen frames before they mean anything.
        if let Some(cpu) = stats
            .frame_stats
            .cpu
            .as_ref()
            .filter(|cpu| cpu.samples >= 60)
        {
            lines.push(format!(
                "p50 {:.1} ms  |  p99 {:.1} ms  |  1% low {:.0} FPS",
                cpu.p50_ms, cpu.p99_ms, cpu.low_1pct_fps
            ));
        }

        let mode = match settings.render_mode {
            RenderMode::Raster => "Raster",
            RenderMode::PathTrace => "PathTrace",
            RenderMode::Sprites2D => "Sprites2D",
        };
        let tonemap = match settings.tonemap_mode {
            0 => "Reinhard",
            1 => "ACES",
            _ => "Passthrough",
        };
        let upscaler = if settings.anf_sr {
            "ANF-SR"
        } else if settings.taa {
            "TAA"
        } else {
            "None"
        };
        lines.push(format!(
            "{mode}  |  Tonemap: {tonemap}  |  Upscaler: {upscaler}  |  PBR: {}",
            pbr_flag_label(settings.debug_flags)
        ));

        // Most expensive 3 passes: same source as render graph panel (last frame readback timestamp results).
        if let Some(gpu) = stats.gpu_timings.as_ref().filter(|gpu| gpu.supported) {
            if !gpu.passes.is_empty() {
                let mut passes: Vec<_> = gpu.passes.iter().collect();
                passes.sort_by(|a, b| b.ms.total_cmp(&a.ms));
                let top: Vec<String> = passes
                    .iter()
                    .take(3)
                    .map(|pass| format!("{} {:.1}", pass.name, pass.ms))
                    .collect();
                let suffix = if passes.len() > top.len() { " …" } else { "" };
                // Part not covered by pass inside full-frame interval: barrier, out-of-graph dispatch, copy, etc.
                // Only show when really got interval, otherwise 0 would be misread as "no out-of-graph overhead".
                let gap = if gpu.has_frame_span() && gpu.unmeasured_ms() >= 0.05 {
                    format!(" (+{:.1} unmeasured)", gpu.unmeasured_ms())
                } else {
                    String::new()
                };
                lines.push(format!("GPU passes: {}{suffix}{gap}", top.join(" · ")));
            }
        }

        if settings.debug_rt != 0 {
            let target = match settings.debug_rt {
                1 => "depth",
                2 => "normal (view-space)",
                _ => "unknown",
            };
            lines.push(format!("Debug RT: {target}"));
        }

        if !has_camera {
            ui.push_text(UiTextInput {
                rect: [0.0, (height * 0.5 - 12.0).max(0.0), width, 24.0],
                content: "[ No camera ]".to_string(),
                font_size: 20.0,
                color: WARN_COLOR,
                alignment: UiTextAlign::Center,
                font: None,
                wrap: false,
                ..Default::default()
            });
        }

        for (i, line) in lines.iter().enumerate() {
            ui.push_text(UiTextInput {
                rect: [
                    MARGIN,
                    MARGIN + i as f32 * LINE_HEIGHT,
                    (width - MARGIN * 2.0).max(1.0),
                    LINE_HEIGHT,
                ],
                content: line.clone(),
                font_size: FONT_SIZE,
                color: TEXT_COLOR,
                alignment: UiTextAlign::Left,
                font: None,
                wrap: false,
                ..Default::default()
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_render::gpu_timer::{GpuTimings, PassTiming};

    fn append(stats: &RenderStats) -> Vec<String> {
        let mut ui = UiOverlayInput::default();
        DebugHud { show: true }.append(
            &mut ui,
            stats,
            &RenderSettings::default(),
            true,
            800.0,
            600.0,
        );
        ui.texts.into_iter().map(|text| text.content).collect()
    }

    #[test]
    fn hud_shows_gpu_total_and_the_three_slowest_passes() {
        let stats = RenderStats {
            render_seq: 1,
            frame_time_ms: 16.0,
            fps: 60.0,
            pt_frame_count: None,
            frame_stats: Default::default(),
            gpu_timings: Some(GpuTimings {
                supported: true,
                frame_ms: 0.0,
                total_ms: 12.5,
                passes: vec![
                    PassTiming {
                        name: "shadow".into(),
                        ms: 2.0,
                    },
                    PassTiming {
                        name: "forward".into(),
                        ms: 8.0,
                    },
                    PassTiming {
                        name: "post".into(),
                        ms: 0.5,
                    },
                    PassTiming {
                        name: "skybox".into(),
                        ms: 0.25,
                    },
                ],
            }),
        };

        let lines = append(&stats);
        assert!(lines[0].contains("GPU 12.5 ms"), "{lines:?}");
        assert_eq!(
            lines[2],
            "GPU passes: forward 8.0 · shadow 2.0 · post 0.5 …"
        );
    }

    /// When full-frame interval exists, HUD shows full-frame time, exposes part not covered by pass.
    #[test]
    fn hud_reports_the_frame_span_and_the_unmeasured_gap() {
        let stats = RenderStats {
            render_seq: 1,
            frame_time_ms: 16.0,
            fps: 60.0,
            pt_frame_count: None,
            frame_stats: Default::default(),
            gpu_timings: Some(GpuTimings {
                supported: true,
                frame_ms: 15.0,
                total_ms: 12.0,
                passes: vec![PassTiming {
                    name: "forward".into(),
                    ms: 12.0,
                }],
            }),
        };

        let lines = append(&stats);
        assert!(lines[0].contains("GPU 15.0 ms"), "{lines:?}");
        assert!(
            lines.iter().any(|line| line.contains("(+3.0 unmeasured)")),
            "{lines:?}"
        );
    }

    #[test]
    fn hud_marks_gpu_timing_unavailable_instead_of_faking_it() {
        let stats = RenderStats::default();
        let lines = append(&stats);
        assert!(lines[0].contains("GPU n/a"), "{lines:?}");
        assert!(lines.iter().all(|line| !line.starts_with("GPU passes")));
    }

    /// #1002: the tail is only shown once the ring buffer has enough samples.
    #[test]
    fn hud_shows_percentiles_after_enough_frames() {
        let mut frames = prism_render::frame_stats::FrameStats::new(120);
        for _ in 0..60 {
            frames.push_frame(16.0, None);
        }
        let stats = RenderStats {
            render_seq: 1,
            frame_time_ms: 16.0,
            fps: 60.0,
            pt_frame_count: None,
            gpu_timings: None,
            frame_stats: frames.summary(),
        };
        let lines = append(&stats);
        assert!(
            lines.iter().any(|line| line.contains("p50 16.0 ms")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("1% low")),
            "{lines:?}"
        );
    }

    #[test]
    fn hud_hides_percentiles_before_enough_frames() {
        let mut frames = prism_render::frame_stats::FrameStats::new(120);
        frames.push_frame(16.0, None);
        let stats = RenderStats {
            render_seq: 1,
            frame_time_ms: 16.0,
            fps: 60.0,
            pt_frame_count: None,
            gpu_timings: None,
            frame_stats: frames.summary(),
        };
        let lines = append(&stats);
        assert!(
            !lines.iter().any(|line| line.contains("p50")),
            "a single sample is not a distribution: {lines:?}"
        );
    }
}
