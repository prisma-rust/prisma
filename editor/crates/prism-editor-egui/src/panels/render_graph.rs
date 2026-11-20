//! Render Graph panel UI.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneRenderMode {
    Shaded,
    Wireframe,
    ShadedWireframe,
    Direct,
    Specular,
    Metallic,
    Roughness,
    AmbientOcclusion,
    Emissive,
    Depth,
    ViewNormals,
    SkinWeights,
}

impl SceneRenderMode {
    pub const ALL: [SceneRenderMode; 12] = [
        SceneRenderMode::Shaded,
        SceneRenderMode::Wireframe,
        SceneRenderMode::ShadedWireframe,
        SceneRenderMode::Direct,
        SceneRenderMode::Specular,
        SceneRenderMode::Metallic,
        SceneRenderMode::Roughness,
        SceneRenderMode::AmbientOcclusion,
        SceneRenderMode::Emissive,
        SceneRenderMode::Depth,
        SceneRenderMode::ViewNormals,
        SceneRenderMode::SkinWeights,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SceneRenderMode::Shaded => "Shaded",
            SceneRenderMode::Wireframe => "Wireframe",
            SceneRenderMode::ShadedWireframe => "Shaded Wireframe",
            SceneRenderMode::Direct => "Direct",
            SceneRenderMode::Specular => "Specular",
            SceneRenderMode::Metallic => "Metallic",
            SceneRenderMode::Roughness => "Roughness",
            SceneRenderMode::AmbientOcclusion => "AO",
            SceneRenderMode::Emissive => "Emissive",
            SceneRenderMode::Depth => "Depth",
            SceneRenderMode::ViewNormals => "View Normals",
            SceneRenderMode::SkinWeights => "Skin Weights",
        }
    }

    fn debug_flags(self) -> u32 {
        match self {
            SceneRenderMode::Direct => 1 << 0,
            SceneRenderMode::Specular => 1 << 2,
            SceneRenderMode::Metallic => 1 << 3,
            SceneRenderMode::Roughness => 1 << 4,
            SceneRenderMode::AmbientOcclusion => 1 << 8,
            SceneRenderMode::Emissive => 1 << 9,

            SceneRenderMode::SkinWeights => 1 << 15,
            _ => 0,
        }
    }

    fn wireframe(self) -> bool {
        matches!(self, SceneRenderMode::Wireframe)
    }

    fn shaded_wireframe(self) -> bool {
        matches!(self, SceneRenderMode::ShadedWireframe)
    }

    fn debug_rt(self) -> u32 {
        match self {
            SceneRenderMode::Depth => 1,
            SceneRenderMode::ViewNormals => 2,
            _ => 0,
        }
    }

    pub fn from_settings(settings: &prism_engine::render_settings::RenderSettings) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| {
            mode.debug_flags() == settings.debug_flags
                && mode.debug_rt() == settings.debug_rt
                && mode.wireframe() == settings.wireframe
                && mode.shaded_wireframe() == settings.shaded_wireframe
        })
    }

    pub fn apply(self, settings: &mut prism_engine::render_settings::RenderSettings) {
        settings.debug_flags = self.debug_flags();
        settings.debug_rt = self.debug_rt();
        settings.wireframe = self.wireframe();
        settings.shaded_wireframe = self.shaded_wireframe();
    }
}

pub fn scene_render_mode_combo(
    ui: &mut Ui,
    settings: &mut prism_engine::render_settings::RenderSettings,
) -> bool {
    let current = SceneRenderMode::from_settings(settings);
    let selected = current.map(|mode| mode.label()).unwrap_or("Custom");
    let mut chosen = current;
    egui::ComboBox::from_id_salt("scene-render-mode")
        .selected_text(selected)
        .show_ui(ui, |ui| {
            for mode in SceneRenderMode::ALL {
                if ui
                    .selectable_label(current == Some(mode), mode.label())
                    .clicked()
                {
                    chosen = Some(mode);
                }
            }
        });
    match chosen {
        Some(mode) if current != Some(mode) => {
            mode.apply(settings);
            true
        }
        _ => false,
    }
}

pub fn render_graph(
    ui: &mut Ui,
    report: Option<&str>,
    settings: &mut prism_engine::render_settings::RenderSettings,
    viewport_scale: &mut f32,
    paused: &mut bool,
    selected_call: &mut Option<usize>,
    frame_budget_ms: &mut Option<f32>,
) -> bool {
    let mut changed = false;
    ui.heading("Render Settings");
    egui::Grid::new("render-settings")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label("Mode");
            changed |= scene_render_mode_combo(ui, settings);
            ui.end_row();

            ui.label("Tonemap");
            egui::ComboBox::from_id_salt("tonemap")
                .selected_text(match settings.tonemap_mode {
                    0 => "Reinhard",
                    1 => "ACES",
                    _ => "Passthrough",
                })
                .show_ui(ui, |ui| {
                    for (mode, label) in [(0u32, "Reinhard"), (1, "ACES"), (2, "Passthrough")] {
                        if ui
                            .selectable_label(settings.tonemap_mode == mode, label)
                            .clicked()
                        {
                            settings.tonemap_mode = mode;
                            changed = true;
                        }
                    }
                });
            ui.end_row();

            ui.label("Contrast");
            changed |= ui
                .add(egui::Slider::new(&mut settings.contrast, 0.0..=3.0))
                .changed();
            ui.end_row();

            ui.label("Saturation");
            changed |= ui
                .add(egui::Slider::new(&mut settings.saturation, 0.0..=3.0))
                .changed();
            ui.end_row();

            ui.label("Vignette");
            changed |= ui
                .add(egui::Slider::new(&mut settings.vignette, 0.0..=1.0))
                .changed();
            ui.end_row();

            ui.label("Chromatic Aberration");
            changed |= ui
                .add(egui::Slider::new(
                    &mut settings.chromatic_aberration,
                    0.0..=1.0,
                ))
                .on_hover_text("Radial red/blue split toward the frame edges.")
                .changed();
            ui.end_row();

            ui.label("Film Grain");
            changed |= ui
                .add(egui::Slider::new(&mut settings.film_grain, 0.0..=1.0))
                .on_hover_text("Monochrome noise over the final image.")
                .changed();
            ui.end_row();

            ui.label("Temperature");
            changed |= ui
                .add(egui::Slider::new(&mut settings.temperature, -1.0..=1.0))
                .on_hover_text("White balance: negative is cooler (blue), positive is warmer (orange).")
                .changed();
            ui.end_row();

            ui.label("Tint");
            changed |= ui
                .add(egui::Slider::new(&mut settings.tint, -1.0..=1.0))
                .on_hover_text("White balance: negative is green, positive is magenta.")
                .changed();
            ui.end_row();

            ui.label("Depth of Field");
            changed |= ui
                .add(egui::Slider::new(&mut settings.dof_aperture, 0.0..=1.0))
                .on_hover_text("Depth-of-field strength; 0 disables it.")
                .changed();
            ui.end_row();

            ui.label("Focus Distance");
            changed |= ui
                .add(egui::Slider::new(
                    &mut settings.dof_focus_distance,
                    0.1..=100.0,
                ))
                .on_hover_text("Distance from the camera that stays sharp, in metres.")
                .changed();
            ui.end_row();

            ui.label("Max Blur");
            changed |= ui
                .add(egui::Slider::new(&mut settings.dof_max_blur, 0.0..=32.0))
                .on_hover_text("Largest depth-of-field blur radius in pixels.")
                .changed();
            ui.end_row();

            ui.label("LUT");
            changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut settings.lut)
                        .hint_text("luts/grade.cube")
                        .desired_width(180.0),
                )
                .on_hover_text("3D LUT path relative to assets/. Empty disables it.")
                .changed();
            ui.end_row();

            ui.label("LUT Strength");
            changed |= ui
                .add_enabled(
                    !settings.lut.trim().is_empty(),
                    egui::Slider::new(&mut settings.lut_strength, 0.0..=1.0),
                )
                .on_hover_text("Blend toward the LUT; 0 disables it, 1 uses it fully.")
                .changed();
            ui.end_row();

            ui.label("Motion Blur");
            changed |= ui
                .add(egui::Slider::new(&mut settings.motion_blur, 0.0..=1.0))
                .on_hover_text("Camera motion-blur strength; 0 disables it.")
                .changed();
            ui.end_row();

            ui.label("Motion Trail");
            changed |= ui
                .add(egui::Slider::new(
                    &mut settings.motion_blur_max_pixels,
                    0.0..=64.0,
                ))
                .on_hover_text("Largest motion-blur trail in pixels.")
                .changed();
            ui.end_row();

            ui.label("TAA");
            changed |= ui.checkbox(&mut settings.taa, "Enable").changed();
            ui.end_row();

            ui.label("Occlusion Culling");
            changed |= ui
                .checkbox(&mut settings.occlusion_culling, "Enable")
                .on_hover_text(
                    "Skip draws that the previous frame's depth fully covers; \
                     shadows are unaffected. Off by default.",
                )
                .changed();
            ui.end_row();

            ui.label("TAA Strength");
            changed |= ui
                .add_enabled(
                    settings.taa,
                    egui::Slider::new(&mut settings.taa_strength, 0.0..=0.95),
                )
                .on_hover_text("History blend weight; 0 is current frame only.")
                .changed();
            ui.end_row();

            ui.label("Bloom");
            changed |= ui.checkbox(&mut settings.bloom, "Enable").changed();
            ui.end_row();

            ui.label("Bloom Intensity");
            changed |= ui
                .add_enabled(
                    settings.bloom,
                    egui::Slider::new(&mut settings.bloom_intensity, 0.0..=1.0),
                )
                .changed();
            ui.end_row();

            ui.label("Bloom Threshold");
            changed |= ui
                .add_enabled(
                    settings.bloom,
                    egui::Slider::new(&mut settings.bloom_threshold, 0.0..=8.0),
                )
                .on_hover_text("HDR luminance above which pixels start to bloom.")
                .changed();
            ui.end_row();

            ui.label("Shadow Distance");
            changed |= ui
                .add(
                    egui::Slider::new(&mut settings.shadow_distance, 10.0..=300.0)
                        .suffix(" m")
                        .fixed_decimals(1),
                )
                .on_hover_text(
                    "Directional shadow coverage. Larger sees further, each texel covers more                      world, so contact shadows get blurrier.",
                )
                .changed();
            ui.end_row();

            ui.label("Shadow Cascades");
            egui::ComboBox::from_id_salt("shadow-cascades")
                .selected_text(match settings.shadow_cascade_count {
                    2 => "2 (atlas)",
                    _ => "1",
                })
                .show_ui(ui, |ui| {
                    for count in [1u32, 2] {
                        let label = if count == 1 { "1" } else { "2 (atlas)" };
                        if ui
                            .selectable_label(settings.shadow_cascade_count == count, label)
                            .clicked()
                        {
                            settings.shadow_cascade_count = count;
                            changed = true;
                        }
                    }
                })
                .response
                .on_hover_text(
                    "1 = single shadow map; 2 = near/far cascades in a 2×1 atlas. Requires recreating the renderer.",
                );
            ui.end_row();

            ui.label("Anti-aliasing");
            egui::ComboBox::from_id_salt("aa-mode")
                .selected_text(match settings.aa_mode {
                    1 => "FXAA",
                    _ => "Off",
                })
                .show_ui(ui, |ui| {
                    for (mode, label) in [(0u32, "Off"), (1, "FXAA")] {
                        if ui
                            .selectable_label(settings.aa_mode == mode, label)
                            .clicked()
                        {
                            settings.aa_mode = mode;
                            changed = true;
                        }
                    }
                });
            ui.end_row();

            ui.label("Path trace");
            let mut path_trace = matches!(
                settings.render_mode,
                prism_engine::render_settings::RenderMode::PathTrace
            );
            if ui.checkbox(&mut path_trace, "Path traced").changed() {
                settings.render_mode = if path_trace {
                    prism_engine::render_settings::RenderMode::PathTrace
                } else {
                    prism_engine::render_settings::RenderMode::Raster
                };
                changed = true;
            }
            ui.end_row();

            if path_trace {
                ui.label("Max bounces");
                changed |= ui
                    .add(egui::Slider::new(&mut settings.pt_max_bounces, 1..=16))
                    .changed();
                ui.end_row();
                ui.label("Samples");
                changed |= ui
                    .add(egui::Slider::new(&mut settings.pt_max_iterations, 0..=4096))
                    .changed();
                ui.end_row();
            }

            ui.label("Grid");
            changed |= ui
                .checkbox(&mut settings.show_grid, "Show ground grid")
                .changed();
            ui.end_row();

            ui.label("Viewport res");
            changed |= ui
                .add(egui::Slider::new(viewport_scale, 0.5..=1.5).suffix("×"))
                .changed();
            ui.end_row();
        });

    ui.separator();
    ui.heading("Render Graph");
    let mut budget = frame_budget_ms.unwrap_or(FRAME_BUDGET_MS);
    ui.horizontal(|ui| {
        ui.label("Frame budget");
        if ui
            .add(
                egui::DragValue::new(&mut budget)
                    .range(1.0..=50.0)
                    .speed(0.1)
                    .suffix(" ms"),
            )
            .changed()
        {
            *frame_budget_ms = Some(budget);
        }
        if frame_budget_ms.is_some() && ui.small_button("Auto").clicked() {
            *frame_budget_ms = None;
        }
        ui.label(
            RichText::new(if frame_budget_ms.is_some() {
                "custom"
            } else {
                "auto 16.6 ms"
            })
            .weak()
            .size(10.0),
        );
    });
    ui.horizontal(|ui| {
        let label = if *paused {
            "Resume live capture"
        } else {
            "Pause capture"
        };
        if ui.button(label).clicked() {
            *paused = !*paused;
        }
        if *paused {
            ui.label(
                RichText::new("showing the frame captured when paused")
                    .weak()
                    .size(10.0),
            );
        }
    });
    ui.label(
        RichText::new(
            "Shadow → Scene → GTAO → PathTrace → Sprite → Sky → Post run on one queue; \
             the Scene/Game viewports sample the final offscreen image directly.",
        )
        .weak()
        .size(11.0),
    );
    match report {
        Some(json) => {
            ui.label(
                RichText::new("target/prism-render-graph.json")
                    .weak()
                    .size(10.0),
            );
            let parsed = prism_editor_core::render_graph::RenderGraphReport::parse(json);
            if let Some(parsed) = &parsed {
                render_graph_timings(ui, parsed, budget);
                render_graph_history(ui, parsed, budget);
                render_graph_stats(ui, parsed);
                render_graph_draw_calls(ui, parsed, paused, selected_call);
            }
            ui.collapsing("Raw JSON", |ui| {
                egui::ScrollArea::both()
                    .id_salt("graph-scroll")
                    .auto_shrink([false, false])
                    .max_height(180.0)
                    .show(ui, |ui| {
                        ui.label(RichText::new(json).monospace().size(10.0));
                    });
            });
        }
        None => {
            ui.label(
                RichText::new(
                    "No report yet — the Scene view writes one to                      target/prism-render-graph.json after its first frame.",
                )
                .weak(),
            );
        }
    }
    changed
}

pub(super) fn render_graph_draw_calls(
    ui: &mut Ui,
    report: &prism_editor_core::render_graph::RenderGraphReport,
    paused: &mut bool,
    selected: &mut Option<usize>,
) {
    let count = report.draw_calls.len();
    if count == 0 {
        return;
    }
    if selected.is_some_and(|index| index >= count) {
        *selected = Some(count - 1);
    }

    ui.add_space(4.0);
    let mut step = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Draw calls").strong());
        ui.label(RichText::new(format!("{count} recorded")).weak().size(10.0));
        if ui.button("◀ Prev").clicked() {
            step = Some(-1);
        }
        if ui.button("Next ▶").clicked() {
            step = Some(1);
        }
        match *selected {
            Some(index) => ui.label(
                RichText::new(format!("call {}/{}", index + 1, count))
                    .weak()
                    .size(10.0),
            ),
            None => ui.label(RichText::new("no call selected").weak().size(10.0)),
        };
    });
    if let Some(delta) = step {
        *selected = stepped_call(*selected, count, delta);

        *paused = true;
    }

    egui::ScrollArea::vertical()
        .id_salt("graph-draw-calls")
        .max_height(160.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Grid::new("graph-draw-call-grid")
                .num_columns(4)
                .spacing([12.0, 3.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Call").weak().size(10.0));
                    ui.label(RichText::new("Inst.").weak().size(10.0));
                    ui.label(RichText::new("Tris").weak().size(10.0));
                    ui.label(RichText::new("State").weak().size(10.0));
                    ui.end_row();
                    for (index, call) in report.draw_calls.iter().enumerate() {
                        let mut text = RichText::new(call.summary()).size(11.0);
                        if !call.visible {
                            text = text.weak().italics();
                        }
                        if ui
                            .selectable_label(*selected == Some(index), text)
                            .clicked()
                        {
                            *selected = Some(index);
                            *paused = true;
                        }
                        ui.label(RichText::new(call.instances.to_string()).size(11.0));
                        ui.label(RichText::new(call.triangles.to_string()).size(11.0));
                        let mut state = RichText::new(call.state.as_deref().unwrap_or("—"))
                            .weak()
                            .size(10.0);
                        if !call.visible {
                            state = state.italics();
                        }
                        ui.label(state);
                        ui.end_row();
                    }
                });
        });

    let Some(index) = *selected else {
        return;
    };
    let Some(call) = report.draw_calls.get(index) else {
        return;
    };
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(
            RichText::new(format!("Call {}/{} · {}", index + 1, count, call.summary()))
                .strong()
                .size(11.0),
        );
        ui.label(
            RichText::new(format!(
                "instances {} · triangles {} · visible {}",
                call.instances,
                call.triangles,
                if call.visible { "yes" } else { "no (culled)" }
            ))
            .size(10.0),
        );
        ui.label(
            RichText::new(format!(
                "pipeline: {}",
                call.pipeline.as_deref().unwrap_or("not recorded")
            ))
            .size(10.0),
        );
        let descriptor_sets = if call.descriptor_sets.is_empty() {
            "none".to_string()
        } else {
            call.descriptor_sets.join(", ")
        };
        ui.label(RichText::new(format!("descriptor sets: {descriptor_sets}")).size(10.0));
        ui.label(
            RichText::new(format!(
                "vertices: {}",
                call.vertices
                    .map(|vertices| vertices.to_string())
                    .unwrap_or_else(|| "not recorded".to_string())
            ))
            .size(10.0),
        );
        ui.label(
            RichText::new(format!(
                "state: {}",
                call.state.as_deref().unwrap_or("not recorded")
            ))
            .size(10.0),
        );
    });
}

pub(super) fn stepped_call(current: Option<usize>, count: usize, delta: isize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let start = match current {
        Some(index) => index.min(count - 1) as isize,

        None if delta < 0 => count as isize,
        None => -1,
    };
    let next = (start + delta).clamp(0, count as isize - 1);
    Some(next as usize)
}

pub(super) fn render_graph_stats(
    ui: &mut Ui,
    report: &prism_editor_core::render_graph::RenderGraphReport,
) {
    let Some(stats) = report.stats else {
        return;
    };
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Resources").strong());
        ui.label(
            RichText::new(format!("~{} resident", format_bytes(stats.total_bytes())))
                .weak()
                .size(10.0),
        );
    });
    egui::Grid::new("graph-resource-stats")
        .num_columns(2)
        .spacing([12.0, 3.0])
        .show(ui, |ui| {
            for (label, count, bytes) in stats.rows() {
                ui.label(RichText::new(label).weak().size(11.0));
                ui.label(RichText::new(format!("{count}  ·  {}", format_bytes(bytes))).size(11.0));
                ui.end_row();
            }
        });
}

pub(super) const FRAME_BUDGET_MS: f32 = 16.6;

pub(super) fn per_pass_budget_ms_with_total(pass_count: usize, frame_budget_ms: f32) -> f32 {
    if pass_count == 0 || !frame_budget_ms.is_finite() || frame_budget_ms <= 0.0 {
        0.0
    } else {
        frame_budget_ms / pass_count as f32
    }
}

pub(super) fn render_graph_timings(
    ui: &mut Ui,
    report: &prism_editor_core::render_graph::RenderGraphReport,
    frame_budget_ms: f32,
) {
    let Some(total) = report.gpu_total_ms() else {
        ui.label(
            RichText::new("No GPU timings (device does not report timestamps).")
                .weak()
                .size(11.0),
        );
        return;
    };

    let passes = report.passes_by_cost();
    let measured = passes.iter().filter(|pass| pass.gpu_ms.is_some()).count();
    let budget = per_pass_budget_ms_with_total(measured, frame_budget_ms);

    ui.horizontal(|ui| {
        ui.label(RichText::new("GPU").strong());
        let color = if total > frame_budget_ms {
            Color32::from_rgb(232, 96, 96)
        } else {
            Color32::from_rgb(120, 200, 140)
        };
        ui.label(RichText::new(format!("{total:.2} ms")).color(color));
        ui.label(
            RichText::new(format!(
                "budget {frame_budget_ms:.1} ms · {budget:.2} ms/pass"
            ))
            .weak()
            .size(10.0),
        );
    });

    egui::Grid::new("graph-pass-timings")
        .num_columns(4)
        .spacing([12.0, 3.0])
        .show(ui, |ui| {
            ui.label(RichText::new("Pass").weak().size(10.0));
            ui.label(RichText::new("Kind").weak().size(10.0));
            ui.label(RichText::new("GPU ms").weak().size(10.0));
            ui.label(RichText::new("Budget").weak().size(10.0));
            ui.end_row();
            for pass in &passes {
                ui.label(RichText::new(&pass.name).size(11.0));
                ui.label(RichText::new(&pass.kind).weak().size(11.0));
                match pass.gpu_ms {
                    Some(ms) => {
                        let color = if budget > 0.0 && ms > budget {
                            Color32::from_rgb(232, 96, 96)
                        } else if budget > 0.0 && ms > budget * 0.8 {
                            Color32::from_rgb(226, 178, 84)
                        } else {
                            ui.visuals().text_color()
                        };
                        ui.label(RichText::new(format!("{ms:.2}")).size(11.0).color(color));
                        ui.label(RichText::new(format!("{budget:.2}")).weak().size(10.0));
                    }
                    None => {
                        ui.label(RichText::new("—").weak().size(11.0));
                        ui.label(RichText::new("—").weak().size(10.0));
                    }
                }
                ui.end_row();
            }
        });
}

pub(super) fn render_graph_history(
    ui: &mut Ui,
    report: &prism_editor_core::render_graph::RenderGraphReport,
    frame_budget_ms: f32,
) {
    let samples = &report.gpu_history_ms;
    if samples.len() < 2 {
        return;
    }
    ui.add_space(4.0);
    let average = samples.iter().sum::<f32>() / samples.len() as f32;
    ui.horizontal(|ui| {
        ui.label(RichText::new("GPU history").strong());
        ui.label(
            RichText::new(format!("{average:.2} ms avg · {} frames", samples.len()))
                .weak()
                .size(10.0),
        );
    });

    let desired = egui::vec2(ui.available_width().clamp(120.0, 360.0), 44.0);
    let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);

    let max = samples
        .iter()
        .copied()
        .fold(frame_budget_ms * 1.25, f32::max)
        .max(1.0);
    let y_for = |ms: f32| rect.bottom() - (ms / max).clamp(0.0, 1.0) * rect.height();

    let budget_y = y_for(frame_budget_ms);
    painter.line_segment(
        [
            egui::pos2(rect.left(), budget_y),
            egui::pos2(rect.right(), budget_y),
        ],
        egui::Stroke::new(1.0_f32, Color32::from_rgb(226, 178, 84)),
    );

    let last = (samples.len() - 1) as f32;
    let points: Vec<egui::Pos2> = samples
        .iter()
        .enumerate()
        .map(|(index, ms)| egui::pos2(rect.left() + rect.width() * index as f32 / last, y_for(*ms)))
        .collect();
    painter.add(egui::Shape::line(
        points,
        egui::Stroke::new(1.5_f32, Color32::from_rgb(120, 200, 140)),
    ));
}

#[cfg(test)]
mod render_mode_tests {
    use super::*;
    use prism_engine::render_settings::RenderSettings;

    #[test]
    fn every_scene_mode_round_trips_through_render_settings() {
        for mode in SceneRenderMode::ALL {
            let mut settings = RenderSettings::default();
            mode.apply(&mut settings);
            assert_eq!(
                SceneRenderMode::from_settings(&settings),
                Some(mode),
                "{} did not round-trip",
                mode.label()
            );
        }
    }

    #[test]
    fn shaded_clears_flags_and_unknown_combinations_report_none() {
        let mut settings = RenderSettings::default();
        SceneRenderMode::Metallic.apply(&mut settings);
        assert_ne!(settings.debug_flags, 0);
        SceneRenderMode::Shaded.apply(&mut settings);
        assert_eq!((settings.debug_flags, settings.debug_rt), (0, 0));
        assert!(!settings.wireframe);
        assert!(!settings.shaded_wireframe);

        SceneRenderMode::Wireframe.apply(&mut settings);
        assert!(settings.wireframe);
        assert!(!settings.shaded_wireframe);
        assert_eq!((settings.debug_flags, settings.debug_rt), (0, 0));

        SceneRenderMode::ShadedWireframe.apply(&mut settings);
        assert!(!settings.wireframe);
        assert!(settings.shaded_wireframe);
        assert_eq!((settings.debug_flags, settings.debug_rt), (0, 0));

        settings.debug_flags = (1 << 0) | (1 << 3);
        assert_eq!(SceneRenderMode::from_settings(&settings), None);
    }
}
