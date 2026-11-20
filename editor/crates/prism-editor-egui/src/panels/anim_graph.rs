//! Anim Graph panel UI.

use super::inspector::{clip_label, joint_depth};
use super::*;

pub(super) const CLIP_TRACKS: [(&str, &str, [f32; 3]); 3] = [
    ("position", "Position", [0.0, 0.0, 0.0]),
    ("euler_xyz", "Rotation (deg)", [0.0, 0.0, 0.0]),
    ("scale", "Scale", [1.0, 1.0, 1.0]),
];

pub fn key_tick_offsets(times: &[f32], duration: f32, width: f32) -> Vec<f32> {
    if duration <= 0.0 || !duration.is_finite() || width <= 0.0 {
        return Vec::new();
    }
    times
        .iter()
        .filter(|time| time.is_finite() && **time >= 0.0 && **time <= duration)
        .map(|time| time / duration * width)
        .collect()
}

pub fn key_tick_hit(
    times: &[f32],
    duration: f32,
    width: f32,
    x: f32,
    tolerance: f32,
) -> Option<f32> {
    if !x.is_finite() || !tolerance.is_finite() || tolerance < 0.0 {
        return None;
    }
    let mut best: Option<(f32, f32)> = None;
    for time in times {
        if !time.is_finite() || *time < 0.0 || *time > duration {
            continue;
        }
        let distance = (*time / duration * width - x).abs();
        if distance > tolerance {
            continue;
        }
        match best {
            Some((current, _)) if current <= distance => {}
            _ => best = Some((distance, *time)),
        }
    }
    best.map(|(_, time)| time)
}

pub(super) const KEY_TICK_HIT_TOLERANCE: f32 = 4.0;

pub fn notify_markers(notifies: &[(f32, String)], duration: f32, width: f32) -> Vec<(f32, String)> {
    if duration <= 0.0 || !duration.is_finite() || width <= 0.0 {
        return Vec::new();
    }
    notifies
        .iter()
        .filter(|(time, _)| time.is_finite() && *time >= 0.0 && *time <= duration)
        .map(|(time, name)| (time / duration * width, name.clone()))
        .collect()
}

pub(super) fn draw_notify_markers(
    ui: &mut egui::Ui,
    slider_rect: egui::Rect,
    notifies: &[(f32, String)],
    duration: f32,
) {
    let markers = notify_markers(notifies, duration, slider_rect.width());
    if markers.is_empty() {
        return;
    }
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(slider_rect.width(), 7.0), egui::Sense::hover());
    let painter = ui.painter();
    let mut hovered: Option<&str> = None;
    for (offset, name) in &markers {
        let x = rect.left() + offset;
        let near = response
            .hover_pos()
            .is_some_and(|pos| (pos.x - x).abs() <= KEY_TICK_HIT_TOLERANCE);
        if near {
            hovered = Some(name.as_str());
        }

        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(x, rect.top() + 1.0),
                egui::pos2(x - 2.5, rect.bottom() - 1.0),
                egui::pos2(x + 2.5, rect.bottom() - 1.0),
            ],
            egui::Color32::from_rgb(110, 200, 255),
            egui::Stroke::NONE,
        ));
    }
    if let Some(name) = hovered {
        response.on_hover_text(format!("Notify: {name}"));
    }
}

pub(super) fn draw_key_ticks(
    ui: &mut egui::Ui,
    slider_rect: egui::Rect,
    times: &[f32],
    duration: f32,
) -> Option<f32> {
    let offsets = key_tick_offsets(times, duration, slider_rect.width());
    if offsets.is_empty() {
        return None;
    }
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(slider_rect.width(), 6.0), egui::Sense::click());
    let painter = ui.painter();
    let hovered = response.hover_pos().map(|pos| pos.x - rect.left());
    for offset in &offsets {
        let x = rect.left() + offset;
        let near =
            hovered.is_some_and(|x_hover| (x_hover - offset).abs() <= KEY_TICK_HIT_TOLERANCE);
        painter.line_segment(
            [
                egui::pos2(x, rect.top() + 1.0),
                egui::pos2(x, rect.bottom() - 1.0),
            ],
            egui::Stroke::new(
                if near { 2.0_f32 } else { 1.0_f32 },
                egui::Color32::from_rgb(255, 206, 84),
            ),
        );
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let response = response.on_hover_text("Jump to this keyframe");

    let click = response.interact_pointer_pos()?;
    if !response.clicked() {
        return None;
    }
    key_tick_hit(
        times,
        duration,
        rect.width(),
        click.x - rect.left(),
        KEY_TICK_HIT_TOLERANCE,
    )
}

pub fn blend_axis_range(positions: &[f32]) -> Option<(f32, f32)> {
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    let mut any = false;
    for position in positions.iter().copied().filter(|value| value.is_finite()) {
        any = true;
        min = min.min(position);
        max = max.max(position);
    }
    any.then_some((min, max))
}

pub fn blend_axis_offsets(positions: &[f32], width: f32) -> Vec<f32> {
    if width <= 0.0 || !width.is_finite() {
        return Vec::new();
    }
    let Some((min, max)) = blend_axis_range(positions) else {
        return Vec::new();
    };
    let span = max - min;
    positions
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .map(|value| {
            if span <= 1e-6 {
                width * 0.5
            } else {
                (value - min) / span * width
            }
        })
        .collect()
}

pub fn blend_axis_value_at(x: f32, width: f32, min: f32, max: f32) -> Option<f32> {
    if width <= 0.0 || !width.is_finite() || !x.is_finite() {
        return None;
    }
    if !min.is_finite() || !max.is_finite() {
        return None;
    }
    let t = (x / width).clamp(0.0, 1.0);
    Some(min + (max - min) * t)
}

pub fn blend_pad_bounds(positions: &[[f32; 2]]) -> Option<([f32; 2], [f32; 2])> {
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    let mut any = false;
    for position in positions {
        if !position[0].is_finite() || !position[1].is_finite() {
            continue;
        }
        any = true;
        min[0] = min[0].min(position[0]);
        min[1] = min[1].min(position[1]);
        max[0] = max[0].max(position[0]);
        max[1] = max[1].max(position[1]);
    }
    if !any {
        return None;
    }
    for axis in 0..2 {
        if max[axis] - min[axis] <= 1e-6 {
            min[axis] -= 1.0;
            max[axis] += 1.0;
        }
    }
    Some((min, max))
}

pub fn blend_pad_value_at(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    min: [f32; 2],
    max: [f32; 2],
) -> Option<[f32; 2]> {
    if width <= 0.0
        || height <= 0.0
        || !width.is_finite()
        || !height.is_finite()
        || !x.is_finite()
        || !y.is_finite()
    {
        return None;
    }
    let tx = (x / width).clamp(0.0, 1.0);
    let ty = (1.0 - (y / height).clamp(0.0, 1.0)).clamp(0.0, 1.0);
    Some([
        min[0] + (max[0] - min[0]) * tx,
        min[1] + (max[1] - min[1]) * ty,
    ])
}

pub(super) fn draw_blend_axis(ui: &mut egui::Ui, positions: &[f32], parameter: f32) -> Option<f32> {
    let Some((min, max)) = blend_axis_range(positions) else {
        return None;
    };
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(80.0), 16.0),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(32, 34, 38));

    for offset in blend_axis_offsets(positions, rect.width()) {
        let x = rect.left() + offset;
        painter.line_segment(
            [
                egui::pos2(x, rect.top() + 3.0),
                egui::pos2(x, rect.bottom() - 3.0),
            ],
            egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(120, 124, 132)),
        );
    }

    let span = max - min;
    let normalized = if span <= 1e-6 {
        0.5
    } else {
        ((parameter.clamp(min, max) - min) / span).clamp(0.0, 1.0)
    };
    let marker = rect.left() + normalized * rect.width();
    painter.circle_filled(
        egui::pos2(marker, rect.center().y),
        4.0,
        egui::Color32::from_rgb(255, 206, 84),
    );

    if response.clicked() || response.dragged() {
        if let Some(pointer) = response.interact_pointer_pos() {
            return blend_axis_value_at(pointer.x - rect.left(), rect.width(), min, max);
        }
    }
    response.on_hover_text("Blend parameter — click or drag to scrub the blend");
    None
}

pub(super) fn draw_blend_pad(
    ui: &mut egui::Ui,
    positions: &[[f32; 2]],
    point: [f32; 2],
) -> Option<[f32; 2]> {
    let Some((min, max)) = blend_pad_bounds(positions) else {
        return None;
    };
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(120.0), 120.0),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(32, 34, 38));

    let to_screen = |value: [f32; 2]| {
        let tx = (value[0] - min[0]) / (max[0] - min[0]);
        let ty = (value[1] - min[1]) / (max[1] - min[1]);
        egui::pos2(
            rect.left() + tx.clamp(0.0, 1.0) * rect.width(),
            rect.bottom() - ty.clamp(0.0, 1.0) * rect.height(),
        )
    };
    for position in positions {
        if position[0].is_finite() && position[1].is_finite() {
            painter.circle_filled(
                to_screen(*position),
                3.0,
                egui::Color32::from_rgb(120, 124, 132),
            );
        }
    }
    painter.circle_filled(to_screen(point), 5.0, egui::Color32::from_rgb(255, 206, 84));

    if response.clicked() || response.dragged() {
        if let Some(pointer) = response.interact_pointer_pos() {
            return blend_pad_value_at(
                pointer.x - rect.left(),
                pointer.y - rect.top(),
                rect.width(),
                rect.height(),
                min,
                max,
            );
        }
    }
    response.on_hover_text("2D blend parameter — click or drag on the plane");
    None
}

pub(super) fn animator_editor(
    ui: &mut Ui,
    workspace: &mut Workspace,
    state: &UiState,
    entity_id: u32,
    component: &str,
    value: &Value,
) {
    let clip_path = value
        .get("clip_path")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let duration = state
        .clip_durations
        .get(&clip_path)
        .copied()
        .unwrap_or_default();
    let mut time = value.get("time").and_then(Value::as_f64).unwrap_or(0.0) as f32;
    let mut speed = value.get("speed").and_then(Value::as_f64).unwrap_or(1.0) as f32;
    let mut playing = value
        .get("playing")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mut looped = value.get("looped").and_then(Value::as_bool).unwrap_or(true);
    let blend_positions: Vec<f32> = value
        .get("blend_positions")
        .and_then(Value::as_array)
        .map(|positions| {
            positions
                .iter()
                .filter_map(Value::as_f64)
                .map(|value| value as f32)
                .collect()
        })
        .unwrap_or_default();
    let mut blend_parameter = value
        .get("blend_parameter")
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as f32;
    let blend_2d_positions: Vec<[f32; 2]> = value
        .get("blend_2d_positions")
        .and_then(Value::as_array)
        .map(|positions| {
            positions
                .iter()
                .filter_map(|position| {
                    let pair = position.as_array()?;
                    Some([
                        pair.first()?.as_f64()? as f32,
                        pair.get(1)?.as_f64()? as f32,
                    ])
                })
                .collect()
        })
        .unwrap_or_default();
    let mut blend_parameter_y = value
        .get("blend_parameter_y")
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as f32;
    let mut edited = false;

    ui.horizontal(|ui| {
        let label = if playing {
            workspace.t("desktop.anim.pause")
        } else {
            workspace.t("desktop.anim.play")
        };
        if ui.button(label).clicked() {
            playing = !playing;
            edited = true;
        }
        if ui
            .button("⏮")
            .on_hover_text(workspace.t("desktop.anim.restart"))
            .clicked()
        {
            time = 0.0;
            edited = true;
        }
        ui.label(RichText::new(clip_label(&clip_path)).size(11.0));
    });

    if duration > 0.0 {
        let response = ui.add(
            egui::Slider::new(&mut time, 0.0..=duration)
                .text(workspace.t("desktop.anim.time"))
                .suffix("s")
                .max_decimals(2),
        );
        if response.changed() {
            edited = true;
        }
        if let Some(keys) = state.clip_keys.get(&clip_path) {
            if let Some(key_time) = draw_key_ticks(ui, response.rect, keys, duration) {
                time = key_time;
                edited = true;
            }
        }
        if let Some(notifies) = state.clip_notifies.get(&clip_path) {
            draw_notify_markers(ui, response.rect, notifies, duration);
        }
        ui.label(
            RichText::new(workspace.tf(
                "desktop.anim.duration",
                &[("duration", &format!("{duration:.2}"))],
            ))
            .size(10.0)
            .weak(),
        );
    } else {
        let response = ui.add(
            egui::DragValue::new(&mut time)
                .speed(0.01)
                .range(0.0..=3600.0)
                .suffix("s"),
        );
        if response.changed() {
            edited = true;
        }
        ui.label(
            RichText::new(workspace.t("desktop.anim.durationUnknown"))
                .size(10.0)
                .weak(),
        );
    }

    ui.horizontal(|ui| {
        if ui
            .checkbox(&mut looped, workspace.t("desktop.anim.loop"))
            .changed()
        {
            edited = true;
        }
        if ui
            .add(
                egui::DragValue::new(&mut speed)
                    .speed(0.01)
                    .range(0.0..=8.0),
            )
            .changed()
        {
            edited = true;
        }
        ui.label(
            RichText::new(workspace.t("desktop.anim.speed"))
                .size(10.0)
                .weak(),
        );
    });

    if blend_2d_positions.len() >= 3 {
        ui.label(
            RichText::new(workspace.tf(
                "desktop.anim.blendParam2d",
                &[
                    ("x", &format!("{blend_parameter:.2}")),
                    ("y", &format!("{blend_parameter_y:.2}")),
                ],
            ))
            .size(10.0)
            .weak(),
        );
        if let Some(next) = draw_blend_pad(
            ui,
            &blend_2d_positions,
            [blend_parameter, blend_parameter_y],
        ) {
            blend_parameter = next[0];
            blend_parameter_y = next[1];
            edited = true;
        }
    } else if blend_positions.len() >= 2 {
        ui.label(
            RichText::new(workspace.tf(
                "desktop.anim.blendParam",
                &[("value", &format!("{blend_parameter:.2}"))],
            ))
            .size(10.0)
            .weak(),
        );
        if let Some(next) = draw_blend_axis(ui, &blend_positions, blend_parameter) {
            blend_parameter = next;
            edited = true;
        }
    }

    let machine_path = value
        .get("state_machine_path")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !machine_path.is_empty() {
        let state = value.get("state").and_then(Value::as_str).unwrap_or("");
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(workspace.t("desktop.anim.state"))
                    .size(10.0)
                    .weak(),
            );
            ui.label(
                RichText::new(if state.is_empty() {
                    workspace.t("desktop.anim.firstState")
                } else {
                    state.to_string()
                })
                .size(10.0)
                .strong(),
            );
            ui.label(RichText::new(clip_label(machine_path)).size(10.0).weak())
                .on_hover_text(machine_path);
        });
    }

    let anim_graph_path = value
        .get("anim_graph_path")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !anim_graph_path.is_empty() {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(workspace.t("desktop.anim.graph"))
                    .size(10.0)
                    .weak(),
            );
            ui.label(
                RichText::new(clip_label(anim_graph_path))
                    .size(10.0)
                    .strong(),
            )
            .on_hover_text(anim_graph_path);
        });
    }

    #[derive(Clone)]
    struct LayerRow {
        clip_path: String,
        weight: f32,
        additive: bool,
        bone_mask: String,
    }
    let mut layers: Vec<LayerRow> = value
        .get("layers")
        .and_then(Value::as_array)
        .map(|layers| {
            layers
                .iter()
                .map(|layer| LayerRow {
                    clip_path: layer
                        .get("clip_path")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    weight: layer.get("weight").and_then(Value::as_f64).unwrap_or(1.0) as f32,
                    additive: layer
                        .get("additive")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    bone_mask: layer
                        .get("bone_mask")
                        .and_then(Value::as_array)
                        .map(|names| {
                            names
                                .iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    let mut layers_edited = false;
    egui::CollapsingHeader::new(workspace.tf(
        "desktop.anim.layers",
        &[("count", &layers.len().to_string())],
    ))
    .id_salt("animation-layers")
    .show(ui, |ui| {
        let mut remove: Option<usize> = None;
        let mut add = false;
        for (index, layer) in layers.iter_mut().enumerate() {
            ui.push_id(index, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("#{}", index + 1)).strong());
                    if ui.small_button(workspace.t("common.remove")).clicked() {
                        remove = Some(index);
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(workspace.t("desktop.anim.clip"))
                            .size(10.0)
                            .weak(),
                    );
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut layer.clip_path)
                                .hint_text("clips/aim.ranim")
                                .desired_width(180.0),
                        )
                        .changed()
                    {
                        layers_edited = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(workspace.t("desktop.anim.weight"))
                            .size(10.0)
                            .weak(),
                    );
                    if ui
                        .add(egui::Slider::new(&mut layer.weight, 0.0..=1.0))
                        .changed()
                    {
                        layers_edited = true;
                    }
                    if ui
                        .checkbox(&mut layer.additive, workspace.t("desktop.anim.additive"))
                        .changed()
                    {
                        layers_edited = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(workspace.t("desktop.anim.bones"))
                            .size(10.0)
                            .weak(),
                    );
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut layer.bone_mask)
                                .hint_text(workspace.t("desktop.anim.emptyAllBones"))
                                .desired_width(180.0),
                        )
                        .changed()
                    {
                        layers_edited = true;
                    }
                });
            });
        }
        if let Some(index) = remove {
            layers.remove(index);
            layers_edited = true;
        }
        if ui.button(workspace.t("desktop.anim.addLayer")).clicked() {
            add = true;
        }
        if add {
            layers.push(LayerRow {
                clip_path: String::new(),
                weight: 1.0,
                additive: false,
                bone_mask: String::new(),
            });
            layers_edited = true;
        }
    });
    if layers_edited {
        let encoded: Vec<Value> = layers
            .iter()
            .map(|layer| {
                let bone_mask: Vec<Value> = layer
                    .bone_mask
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(|name| Value::String(name.to_string()))
                    .collect();
                json!({
                    "clip_path": layer.clip_path,
                    "weight": layer.weight as f64,
                    "additive": layer.additive,
                    "bone_mask": bone_mask,
                })
            })
            .collect();
        workspace.set_field(entity_id, component, "layers", Value::Array(encoded));
    }

    if edited {
        workspace.set_field(entity_id, component, "time", Value::from(time as f64));
        workspace.set_field(entity_id, component, "speed", Value::from(speed as f64));
        workspace.set_field(entity_id, component, "playing", Value::Bool(playing));
        workspace.set_field(entity_id, component, "looped", Value::Bool(looped));
        if blend_positions.len() >= 2 || blend_2d_positions.len() >= 3 {
            workspace.set_field(
                entity_id,
                component,
                "blend_parameter",
                Value::from(blend_parameter as f64),
            );
        }
        if blend_2d_positions.len() >= 3 {
            workspace.set_field(
                entity_id,
                component,
                "blend_parameter_y",
                Value::from(blend_parameter_y as f64),
            );
        }
    }
}

pub(super) fn skeleton_root_editor(
    ui: &mut Ui,
    workspace: &Workspace,
    state: &UiState,
    component: &str,
    value: &Value,
) {
    let skeleton_path = value
        .get("skeleton_path")
        .and_then(Value::as_str)
        .unwrap_or_default();
    ui.label(RichText::new(clip_label(skeleton_path)).size(11.0));
    let joints = state.skeleton_joints.get(skeleton_path);
    match joints {
        Some(joints) if !joints.is_empty() => {
            egui::CollapsingHeader::new(workspace.tf(
                "desktop.anim.bonesCount",
                &[("count", &joints.len().to_string())],
            ))
            .id_salt((component, skeleton_path))
            .show(ui, |ui| {
                for (index, joint) in joints.iter().enumerate() {
                    let depth = joint_depth(joints, index);
                    ui.label(
                        RichText::new(format!("{:indent$}{}", "", joint.name, indent = depth * 2))
                            .size(10.0)
                            .monospace(),
                    );
                }
            });
        }
        _ => {
            ui.label(
                RichText::new(workspace.t("desktop.anim.skeletonNotLoaded"))
                    .size(10.0)
                    .weak(),
            );
        }
    }
}

pub(super) fn runtime_node_summary(node: &prism_anim::GraphNodeSnapshot) -> String {
    let detail = if node.state_name.is_empty() {
        format!("{:.2}s", node.time)
    } else {
        format!("{} · {:.2}s", node.state_name, node.time)
    };
    format!(
        "{} [{:?}] w={:.2} {detail}",
        node.name, node.kind, node.weight
    )
}

pub(super) fn runtime_montage_summary(montage: &prism_anim::MontageSnapshot) -> String {
    let clip = montage.clip.as_deref().unwrap_or("-");
    let section = montage
        .section
        .as_deref()
        .map(|section| format!(" · {section}"))
        .unwrap_or_default();
    let warp = montage
        .warp_window
        .as_deref()
        .map(|window| {
            montage
                .warp_window_progress
                .map(|progress| format!(" · warp {window} {progress:.0}%"))
                .unwrap_or_else(|| format!(" · warp {window}"))
        })
        .unwrap_or_default();
    let time = montage
        .time
        .map(|time| format!("{time:.2}s"))
        .unwrap_or_else(|| "-".to_string());
    let mut flags = String::new();
    if montage.paused {
        flags.push_str(" paused");
    }
    if montage.finished {
        flags.push_str(" finished");
    }
    if montage.queued > 0 {
        flags.push_str(&format!(" +{} queued", montage.queued));
    }
    format!(
        "slot {} · {clip}{section}{warp} {time} w={:.2}{flags}",
        montage.slot, montage.weight
    )
}

#[derive(Clone, Copy)]
pub struct AnimRuntimeView<'a> {
    pub graph: &'a prism_engine::scene::systems::animator::AnimGraphDebug,

    pub events: Option<&'a prism_engine::scene::systems::animator::AnimEvents>,
}

pub(super) fn runtime_event_summary(
    event: &prism_engine::scene::systems::animator::AnimEvent,
) -> String {
    format!("• {} @ {:.2}s", event.name, event.time)
}

pub(super) fn draw_runtime_debug(
    ui: &mut Ui,
    workspace: &Workspace,
    runtime: Option<AnimRuntimeView<'_>>,
) {
    let Some(runtime) = runtime else {
        return;
    };
    let debug = runtime.graph;
    if debug.entities.is_empty() {
        ui.label(
            RichText::new(workspace.t("desktop.anim.runtimeEmpty"))
                .weak()
                .size(9.0),
        );
        return;
    }
    egui::CollapsingHeader::new(
        RichText::new(workspace.tf(
            "desktop.anim.runtimeEntities",
            &[("count", &debug.entities.len().to_string())],
        ))
        .color(Color32::from_rgb(120, 200, 160))
        .size(10.0),
    )
    .id_salt("anim-graph-runtime")
    .default_open(true)
    .show(ui, |ui| {
        let mut entities: Vec<_> = debug.entities.iter().collect();
        entities.sort_by_key(|(entity, _)| format!("{entity:?}"));
        for (entity, nodes) in entities {
            let path = debug.path(*entity).unwrap_or("?");
            ui.label(
                RichText::new(format!("{entity:?} · {path}"))
                    .weak()
                    .size(9.0),
            );
            for node in nodes {
                ui.label(RichText::new(runtime_node_summary(node)).size(9.0));
            }
            if let Some(montages) = debug.montages(*entity) {
                for montage in montages {
                    ui.label(RichText::new(runtime_montage_summary(montage)).size(9.0));
                }
            }
            if let Some(events) = runtime.events {
                for event in events.events.iter().filter(|event| event.entity == *entity) {
                    ui.label(
                        RichText::new(runtime_event_summary(event))
                            .color(Color32::from_rgb(110, 200, 255))
                            .size(9.0),
                    );
                }
            }
        }
    });
}

pub fn anim_graph(
    ui: &mut Ui,
    workspace: &mut Workspace,
    state: &mut UiState,
    runtime: Option<AnimRuntimeView<'_>>,
) {
    let Some(path) = state.asset_primary().cloned() else {
        ui.label(
            RichText::new(workspace.t("desktop.anim.selectAsset"))
                .weak()
                .size(11.0),
        );
        return;
    };
    let notify_asset = is_notify_sidecar(&path);
    let sync_marker_asset = is_sync_markers_sidecar(&path);
    let montage_asset = is_montages_sidecar(&path);
    let clip_asset = path.ends_with(".ranim");
    let graph_asset = path.ends_with(".animgraph.json");
    let machine_asset = path.ends_with(".statemachine.json");
    if !notify_asset
        && !sync_marker_asset
        && !montage_asset
        && !clip_asset
        && !graph_asset
        && !machine_asset
    {
        ui.label(
            RichText::new(workspace.t("desktop.anim.selectAsset"))
                .weak()
                .size(11.0),
        );
        return;
    }

    ui.label(
        RichText::new(path.rsplit('/').next().unwrap_or(&path))
            .strong()
            .size(13.0),
    );
    ui.label(RichText::new(&path).weak().size(10.0));
    draw_asset_edit_buttons(ui, workspace, state);
    ui.separator();
    if notify_asset {
        draw_notifies_asset(ui, workspace, state, &path);
        return;
    }
    if sync_marker_asset {
        draw_sync_markers_asset(ui, workspace, state, &path);
        return;
    }
    if montage_asset {
        draw_montages_asset(ui, workspace, state, &path);
        return;
    }
    if clip_asset {
        draw_clip_asset(ui, workspace, &path);
        return;
    }

    draw_runtime_debug(ui, workspace, runtime);
    ui.separator();

    let mut value = match workspace.read_asset_json(&path) {
        Ok(value) => value,
        Err(error) => {
            ui.label(
                RichText::new(workspace.tf(
                    "desktop.anim.readFailed",
                    &[("path", &path), ("error", &error.to_string())],
                ))
                .color(Color32::from_rgb(232, 96, 96)),
            );
            return;
        }
    };

    if graph_asset {
        match prism_editor_core::parse_anim_graph_view(&value) {
            Ok(graph) => {
                let validation = prism_editor_core::validate_anim_graph(&value);
                if validation.is_empty() {
                    ui.label(
                        RichText::new(workspace.t("desktop.anim.shapeOk"))
                            .weak()
                            .size(9.0),
                    );
                } else {
                    egui::CollapsingHeader::new(
                        RichText::new(workspace.tf(
                            "desktop.anim.shapeIssues",
                            &[("count", &validation.len().to_string())],
                        ))
                        .color(Color32::from_rgb(232, 160, 96))
                        .size(10.0),
                    )
                    .id_salt("anim-graph-shape-issues")
                    .default_open(true)
                    .show(ui, |ui| {
                        for issue in &validation {
                            ui.label(
                                RichText::new(issue)
                                    .color(Color32::from_rgb(232, 160, 96))
                                    .size(9.0),
                            );
                        }
                    });
                }
                let stale = state
                    .anim_graph_layout
                    .as_ref()
                    .map(|(cached, _)| cached != &path)
                    .unwrap_or(true);
                if stale {
                    let layout = workspace.read_anim_graph_layout(&path);
                    state.anim_graph_layout = Some((path.clone(), layout));
                }
                let mut changed = false;
                if let Some((_, layout)) = state.anim_graph_layout.as_mut() {
                    changed = draw_anim_graph_layout(ui, &graph, layout);
                }
                if changed {
                    ui.label(
                        RichText::new(workspace.t("desktop.anim.layoutModified"))
                            .weak()
                            .size(10.0),
                    );
                }
                if ui.button(workspace.t("desktop.anim.saveLayout")).clicked() {
                    if let Some((_, layout)) = state.anim_graph_layout.as_ref() {
                        state.status = match workspace.write_anim_graph_layout(&path, layout) {
                            Ok(()) => workspace.tf("desktop.anim.layoutSaved", &[("path", &path)]),
                            Err(error) => workspace.tf(
                                "desktop.anim.layoutSaveFailed",
                                &[("error", &error.to_string())],
                            ),
                        };
                    }
                }
                ui.separator();
                let before = value.clone();
                let result = draw_anim_graph_view(
                    ui,
                    workspace,
                    &graph,
                    &mut value,
                    &mut state.anim_graph_edit,
                );
                match result {
                    Ok(changed_now) => {
                        if changed_now {
                            state.status = match workspace.write_asset_json(&path, &value) {
                                Ok(()) => {
                                    workspace.record_asset_edit(&path, &before, &value);
                                    format!("saved nodes/links in {path}")
                                }
                                Err(error) => format!("save graph failed: {error}"),
                            };
                        }
                        if let Some((old_name, new_name)) =
                            state.anim_graph_edit.graph_parameter_rename.take()
                        {
                            state.status = match workspace
                                .rename_anim_graph_parameter(&path, &old_name, &new_name)
                            {
                                Ok(updated) if updated.is_empty() => {
                                    format!("renamed parameter '{old_name}' to '{new_name}'")
                                }
                                Ok(updated) => format!(
                                    "renamed parameter '{old_name}' to '{new_name}', updated {}",
                                    updated.join(", ")
                                ),
                                Err(error) => error,
                            };
                        }
                    }
                    Err(error) => {
                        state.anim_graph_edit.graph_parameter_rename = None;
                        state.status = format!("animation graph: {error}");
                    }
                }
            }
            Err(error) => {
                ui.label(RichText::new(error).color(Color32::from_rgb(232, 96, 96)));
            }
        }
    } else {
        match prism_editor_core::parse_state_machine_view(&value) {
            Ok(machine) => {
                let before = value.clone();
                match draw_state_machine_view(
                    ui,
                    workspace,
                    &machine,
                    &mut value,
                    &mut state.anim_graph_edit,
                    &mut state.anim_sm_condition,
                ) {
                    Ok(true) => {
                        state.status = match workspace.write_asset_json(&path, &value) {
                            Ok(()) => {
                                workspace.record_asset_edit(&path, &before, &value);
                                format!("saved state machine in {path}")
                            }
                            Err(error) => format!("save transitions failed: {error}"),
                        };
                    }
                    Ok(false) => {}
                    Err(error) => {
                        state.anim_graph_edit.sm_state_rename = None;
                        state.status = format!("state machine: {error}");
                    }
                }
                if let Some((index, new_name)) = state.anim_graph_edit.sm_state_rename.take() {
                    state.status =
                        match workspace.rename_state_machine_state(&path, index, &new_name) {
                            Ok(updated) if updated.is_empty() => {
                                format!("renamed state to '{new_name}'")
                            }
                            Ok(updated) => format!(
                                "renamed state to '{new_name}', updated {}",
                                updated.join(", ")
                            ),
                            Err(error) => format!("state machine: {error}"),
                        };
                }
            }
            Err(error) => {
                ui.label(RichText::new(error).color(Color32::from_rgb(232, 96, 96)));
            }
        }
    }
}

pub(super) fn draw_asset_edit_buttons(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                workspace.can_undo_asset_edit(),
                egui::Button::new(workspace.t("desktop.anim.undoEdit")),
            )
            .on_hover_text(workspace.t("desktop.anim.undoEditHint"))
            .clicked()
        {
            state.status = match workspace.undo_asset_edit() {
                Ok(Some(paths)) => {
                    workspace.tf("desktop.anim.undidEdit", &[("paths", &paths.join(", "))])
                }
                Ok(None) => workspace.t("desktop.anim.nothingToUndo"),
                Err(error) => error,
            };
        }
        if ui
            .add_enabled(
                workspace.can_redo_asset_edit(),
                egui::Button::new(workspace.t("desktop.anim.redoEdit")),
            )
            .on_hover_text(workspace.t("desktop.anim.redoEditHint"))
            .clicked()
        {
            state.status = match workspace.redo_asset_edit() {
                Ok(Some(paths)) => {
                    workspace.tf("desktop.anim.redidEdit", &[("paths", &paths.join(", "))])
                }
                Ok(None) => workspace.t("desktop.anim.nothingToRedo"),
                Err(error) => error,
            };
        }
    });
}

pub(super) fn is_notify_sidecar(path: &str) -> bool {
    path.to_ascii_lowercase()
        .ends_with(".skinning/notifies.json")
}

pub(super) fn is_sync_markers_sidecar(path: &str) -> bool {
    path.to_ascii_lowercase()
        .ends_with(".skinning/sync_markers.json")
}

pub(super) fn is_montages_sidecar(path: &str) -> bool {
    path.to_ascii_lowercase()
        .ends_with(".skinning/montages.json")
}

pub(super) fn draw_notifies_asset(
    ui: &mut Ui,
    workspace: &mut Workspace,
    state: &mut UiState,
    path: &str,
) {
    ui.label(
        RichText::new(path.rsplit('/').next().unwrap_or(path))
            .strong()
            .size(13.0),
    );
    ui.label(RichText::new(path).weak().size(10.0));
    ui.separator();

    let mut value = match workspace.read_asset_json(path) {
        Ok(value) => value,
        Err(error) => {
            ui.label(
                RichText::new(format!("read {path} failed: {error}"))
                    .color(Color32::from_rgb(232, 96, 96)),
            );
            return;
        }
    };
    let view = match prism_editor_core::parse_notifies_view(&value) {
        Ok(view) => view,
        Err(error) => {
            ui.label(RichText::new(error).color(Color32::from_rgb(232, 96, 96)));
            return;
        }
    };
    let before = value.clone();
    match draw_notifies_view(ui, workspace, &view, &mut value, &mut state.anim_graph_edit) {
        Ok(true) => {
            state.status = match workspace.write_asset_json(path, &value) {
                Ok(()) => {
                    workspace.record_asset_edit(path, &before, &value);
                    format!("saved notifies in {path}")
                }
                Err(error) => format!("save notifies failed: {error}"),
            };
        }
        Ok(false) => {}
        Err(error) => state.status = format!("notify file: {error}"),
    }
}

pub(super) const ANIMATOR_COMPONENT: &str = "prism_engine::scene::Animator";

pub(super) fn clip_runtime_path(asset_path: &str) -> &str {
    asset_path.strip_prefix("assets/").unwrap_or(asset_path)
}

pub(super) fn selected_animator_index(workspace: &Workspace) -> Option<usize> {
    workspace.selection.iter().rev().copied().find(|index| {
        workspace
            .document()
            .doc
            .entities
            .get(*index)
            .is_some_and(|entity| entity.components.contains_key(ANIMATOR_COMPONENT))
    })
}

pub(super) fn edit_animator(
    workspace: &mut Workspace,
    index: usize,
    label: &str,
    mutate: impl FnOnce(&mut Value),
) {
    workspace.edit(label, |document| {
        if let Some(animator) = document
            .doc
            .entities
            .get_mut(index)
            .and_then(|entity| entity.components.get_mut(ANIMATOR_COMPONENT))
        {
            mutate(animator);
        }
    });
}

pub(super) fn draw_clip_asset(ui: &mut Ui, workspace: &mut Workspace, path: &str) {
    let bytes = match workspace.read_asset_bytes(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            ui.label(
                RichText::new(format!("read {path} failed: {error}"))
                    .color(Color32::from_rgb(232, 96, 96)),
            );
            return;
        }
    };
    let Some(clip) = prism_anim::decode_ranim(&bytes) else {
        ui.label(
            RichText::new(workspace.t("desktop.anim.invalidClip"))
                .color(Color32::from_rgb(232, 96, 96)),
        );
        return;
    };
    let duration = clip.clip_duration().max(0.0);
    ui.label(RichText::new(&clip.name).strong().size(13.0));
    ui.label(RichText::new(path).weak().size(10.0));
    let flags = match (clip.looped, clip.additive) {
        (true, true) => workspace.t("desktop.anim.loopAdditive"),
        (true, false) => workspace.t("desktop.anim.looped"),
        (false, true) => workspace.t("desktop.anim.additiveOnly"),
        (false, false) => workspace.t("desktop.anim.oneShot"),
    };
    ui.label(
        RichText::new(workspace.tf(
            "desktop.anim.clipSummary",
            &[
                ("duration", &format!("{duration:.2}")),
                ("fps", &format!("{:.0}", clip.fps)),
                ("flags", &flags),
                ("tracks", &clip.tracks.len().to_string()),
                ("notifies", &clip.notifies.len().to_string()),
                ("sync", &clip.sync_markers.len().to_string()),
            ],
        ))
        .weak()
        .size(10.0),
    );

    let selected = selected_animator_index(workspace);
    let runtime_path = clip_runtime_path(path).to_string();
    let current_animator = selected.and_then(|index| {
        workspace.document().doc.entities[index]
            .components
            .get(ANIMATOR_COMPONENT)
            .cloned()
    });
    let matches = current_animator
        .as_ref()
        .and_then(|animator| animator.get("clip_path").and_then(Value::as_str))
        == Some(runtime_path.as_str());

    ui.separator();
    let width = ui.available_width().clamp(160.0, 560.0);
    let (track, painter) = ui.allocate_painter(Vec2::new(width, 44.0), egui::Sense::click());
    let rect = track.rect;
    painter.rect_filled(rect, 3.0, Color32::from_rgb(26, 28, 32));
    let inset = 8.0;
    let left = rect.left() + inset;
    let usable = (rect.width() - inset * 2.0).max(1.0);
    let y = rect.center().y;
    painter.line_segment(
        [egui::pos2(left, y), egui::pos2(left + usable, y)],
        egui::Stroke::new(1.0_f32, Color32::from_rgb(58, 63, 72)),
    );
    painter.text(
        egui::pos2(left, rect.bottom() - 1.0),
        egui::Align2::LEFT_BOTTOM,
        "0",
        egui::FontId::monospace(8.0),
        Color32::from_rgb(110, 116, 126),
    );
    painter.text(
        egui::pos2(left + usable, rect.bottom() - 1.0),
        egui::Align2::RIGHT_BOTTOM,
        format!("{duration:.2}s"),
        egui::FontId::monospace(8.0),
        Color32::from_rgb(110, 116, 126),
    );
    for notify in &clip.notifies {
        let x = prism_editor_core::timeline_x(notify.time, duration, left, usable);
        painter.circle_filled(egui::pos2(x, y), 3.5, Color32::from_rgb(214, 178, 74));
    }
    for phase in &clip.sync_markers {
        let x = prism_editor_core::timeline_x(phase * duration, duration, left, usable);
        painter.rect_filled(
            egui::Rect::from_center_size(egui::pos2(x, y - 7.0), Vec2::new(5.0, 5.0)),
            1.0,
            Color32::from_rgb(112, 196, 168),
        );
    }
    if let Some(pos) = track.hover_pos() {
        let mut closest: Option<(f32, String)> = None;
        for notify in &clip.notifies {
            let x = prism_editor_core::timeline_x(notify.time, duration, left, usable);
            let distance = (x - pos.x).abs();
            if distance <= 8.0 && closest.as_ref().is_none_or(|(best, _)| distance < *best) {
                closest = Some((distance, format!("{} @ {:.2}s", notify.name, notify.time)));
            }
        }
        for phase in &clip.sync_markers {
            let x = prism_editor_core::timeline_x(phase * duration, duration, left, usable);
            let distance = (x - pos.x).abs();
            if distance <= 8.0 && closest.as_ref().is_none_or(|(best, _)| distance < *best) {
                closest = Some((
                    distance,
                    workspace.tf(
                        "desktop.anim.syncHover",
                        &[("phase", &format!("{phase:.3}"))],
                    ),
                ));
            }
        }
        if let Some((_, text)) = closest {
            track.clone().on_hover_text(text);
        }
    }
    ui.label(
        RichText::new(workspace.t("desktop.anim.clipLegend"))
            .weak()
            .size(9.0),
    );

    if let Some(root) = clip.tracks.iter().min_by_key(|track| track.joint) {
        if let (Some(first), Some(last)) = (
            root.positions
                .iter()
                .min_by(|a, b| a.time.total_cmp(&b.time)),
            root.positions
                .iter()
                .max_by(|a, b| a.time.total_cmp(&b.time)),
        ) {
            let (delta, yaw) = prism_editor_core::root_motion_delta(first.value, last.value);
            ui.label(
                RichText::new(workspace.tf(
                    "desktop.anim.rootMotion",
                    &[
                        ("joint", &root.joint.to_string()),
                        ("x", &format!("{:.2}", delta[0])),
                        ("y", &format!("{:.2}", delta[1])),
                        ("z", &format!("{:.2}", delta[2])),
                        ("yaw", &format!("{yaw:.1}")),
                    ],
                ))
                .monospace()
                .size(10.0),
            );
        }
    }

    egui::CollapsingHeader::new(
        RichText::new(workspace.tf(
            "desktop.anim.tracks",
            &[("count", &clip.tracks.len().to_string())],
        ))
        .strong()
        .size(11.0),
    )
    .id_salt("clip-tracks")
    .show(ui, |ui| {
        for track in &clip.tracks {
            let first = track
                .positions
                .iter()
                .map(|key| key.time)
                .chain(track.rotations.iter().map(|key| key.time))
                .chain(track.scales.iter().map(|key| key.time))
                .filter(|time| time.is_finite())
                .fold(f32::INFINITY, f32::min);
            let last = track
                .positions
                .iter()
                .map(|key| key.time)
                .chain(track.rotations.iter().map(|key| key.time))
                .chain(track.scales.iter().map(|key| key.time))
                .filter(|time| time.is_finite())
                .fold(f32::NEG_INFINITY, f32::max);
            let range = if first.is_finite() && last.is_finite() {
                format!("{first:.2}..{last:.2}s")
            } else {
                workspace.t("desktop.anim.noKeys")
            };
            ui.label(
                RichText::new(workspace.tf(
                    "desktop.anim.trackLine",
                    &[
                        ("joint", &track.joint.to_string()),
                        ("pos", &track.positions.len().to_string()),
                        ("rot", &track.rotations.len().to_string()),
                        ("scale", &track.scales.len().to_string()),
                        ("range", &range),
                    ],
                ))
                .monospace()
                .size(9.0),
            );
        }
    });

    if !clip.notifies.is_empty() {
        egui::CollapsingHeader::new(
            RichText::new(workspace.tf(
                "desktop.anim.notifies",
                &[("count", &clip.notifies.len().to_string())],
            ))
            .strong()
            .size(11.0),
        )
        .id_salt("clip-notifies")
        .show(ui, |ui| {
            for notify in &clip.notifies {
                ui.label(
                    RichText::new(format!("{:.2}s · {}", notify.time, notify.name))
                        .monospace()
                        .size(9.0),
                );
            }
        });
    }

    ui.separator();
    ui.label(
        RichText::new(workspace.t("desktop.anim.selectedAnimator"))
            .strong()
            .size(11.0),
    );
    let Some(index) = selected else {
        ui.label(
            RichText::new(workspace.t("desktop.anim.selectAnimatorEntity"))
                .weak()
                .size(10.0),
        );
        return;
    };
    let Some(animator) = current_animator.as_ref() else {
        return;
    };
    if !matches {
        let current_clip = animator
            .get("clip_path")
            .and_then(Value::as_str)
            .unwrap_or("");
        ui.label(
            RichText::new(workspace.tf(
                "desktop.anim.currentClip",
                &[(
                    "clip",
                    if current_clip.is_empty() {
                        "-"
                    } else {
                        current_clip
                    },
                )],
            ))
            .weak()
            .size(10.0),
        );
        if ui.button(workspace.t("desktop.anim.useThisClip")).clicked() {
            edit_animator(
                workspace,
                index,
                &workspace.t("desktop.anim.undoUseClip"),
                |animator| {
                    animator["clip_path"] = json!(runtime_path.as_str());
                    animator["time"] = json!(0.0);
                    animator["playing"] = json!(true);
                    animator["looped"] = json!(clip.looped);
                    animator["state_machine_path"] = json!("");
                    animator["anim_graph_path"] = json!("");
                },
            );
            ui.label(
                RichText::new(workspace.t("desktop.anim.applied"))
                    .weak()
                    .size(9.0),
            );
        }
        return;
    }

    let mut speed = animator.get("speed").and_then(Value::as_f64).unwrap_or(1.0) as f32;
    let playing = animator
        .get("playing")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mut looped = animator
        .get("looped")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let time = animator.get("time").and_then(Value::as_f64).unwrap_or(0.0) as f32;
    ui.horizontal(|ui| {
        let play_label = if playing {
            workspace.t("desktop.anim.pauseShort")
        } else {
            workspace.t("desktop.anim.playShort")
        };
        if ui.button(play_label).clicked() {
            let next = !playing;
            edit_animator(
                workspace,
                index,
                &workspace.t("desktop.anim.undoTogglePlayback"),
                |animator| {
                    animator["playing"] = json!(next);
                },
            );
        }
        if ui
            .add(
                egui::DragValue::new(&mut speed)
                    .speed(0.01)
                    .range(-4.0..=4.0)
                    .prefix(workspace.t("desktop.anim.speedPrefix")),
            )
            .changed()
        {
            edit_animator(
                workspace,
                index,
                &workspace.t("desktop.anim.undoSetSpeed"),
                |animator| {
                    animator["speed"] = json!(speed);
                },
            );
        }
        if ui
            .checkbox(&mut looped, workspace.t("desktop.anim.loopLower"))
            .changed()
        {
            edit_animator(
                workspace,
                index,
                &workspace.t("desktop.anim.undoToggleLoop"),
                |animator| {
                    animator["looped"] = json!(looped);
                },
            );
        }
        ui.label(format!("t {time:.2}s"));
    });
    if matches && track.clicked() {
        if let Some(pos) = track.interact_pointer_pos() {
            let time = prism_editor_core::timeline_time_at(pos.x, left, usable, duration);
            edit_animator(
                workspace,
                index,
                &workspace.t("desktop.anim.undoScrub"),
                |animator| {
                    animator["time"] = json!(time);
                },
            );
        }
    }
    ui.label(
        RichText::new(workspace.t("desktop.anim.timelineHint"))
            .weak()
            .size(9.0),
    );
}

pub(super) fn draw_montages_asset(
    ui: &mut Ui,
    workspace: &mut Workspace,
    state: &mut UiState,
    path: &str,
) {
    let mut value = match workspace.read_asset_json(path) {
        Ok(value) => value,
        Err(error) => {
            ui.label(
                RichText::new(format!("read {path} failed: {error}"))
                    .color(Color32::from_rgb(232, 96, 96)),
            );
            return;
        }
    };
    let view = match prism_editor_core::parse_montages_view(&value) {
        Ok(view) => view,
        Err(error) => {
            ui.label(RichText::new(error).color(Color32::from_rgb(232, 96, 96)));
            return;
        }
    };
    let before = value.clone();
    match draw_montages_view(ui, workspace, &view, &mut value, &mut state.anim_graph_edit) {
        Ok(true) => {
            state.status = match workspace.write_asset_json(path, &value) {
                Ok(()) => {
                    workspace.record_asset_edit(path, &before, &value);
                    format!("saved montage sections in {path}")
                }
                Err(error) => format!("save montage sections failed: {error}"),
            };
        }
        Ok(false) => {}
        Err(error) => state.status = format!("montage file: {error}"),
    }
}

pub(super) fn draw_montage_warp_windows(
    ui: &mut Ui,
    workspace: &Workspace,
    clip: &prism_editor_core::ClipMontageView,
    document: &mut Value,
    edit: &mut AnimGraphEditState,
    span: f32,
) -> Result<bool, String> {
    let mut changed = false;
    let mut updates: Vec<(usize, String, f32, f32)> = Vec::new();
    let mut removals: Vec<usize> = Vec::new();

    ui.label(
        RichText::new(workspace.tf(
            "desktop.anim.warpWindows",
            &[("count", &clip.warp_windows.len().to_string())],
        ))
        .strong()
        .size(10.0),
    );
    if clip.warp_windows.is_empty() {
        ui.label(
            RichText::new(workspace.t("desktop.anim.noWarpWindows"))
                .weak()
                .size(9.0),
        );
    }
    let width = ui.available_width().clamp(160.0, 560.0);
    let (track, painter) = ui.allocate_painter(Vec2::new(width, 22.0), egui::Sense::hover());
    let rect = track.rect;
    painter.rect_filled(rect, 3.0, Color32::from_rgb(26, 28, 32));
    let inset = 6.0;
    let left = rect.left() + inset;
    let usable = (rect.width() - inset * 2.0).max(1.0);
    for window in &clip.warp_windows {
        let start = prism_editor_core::timeline_x(window.start, span, left, usable);
        let end = prism_editor_core::timeline_x(window.end, span, left, usable);
        let bar = egui::Rect::from_min_max(
            egui::pos2(start, rect.top() + 4.0),
            egui::pos2(end.max(start + 2.0), rect.bottom() - 4.0),
        );
        painter.rect_filled(bar, 2.0, Color32::from_rgb(214, 146, 74));
        if bar.width() > 28.0 {
            painter.text(
                bar.center(),
                egui::Align2::CENTER_CENTER,
                &window.name,
                egui::FontId::monospace(8.0),
                Color32::from_rgb(24, 20, 14),
            );
        }
    }
    painter.text(
        egui::pos2(left + usable, rect.bottom() - 1.0),
        egui::Align2::RIGHT_BOTTOM,
        format!("{span:.2}s"),
        egui::FontId::monospace(8.0),
        Color32::from_rgb(110, 116, 126),
    );

    for (index, window) in clip.warp_windows.iter().enumerate() {
        ui.horizontal(|ui| {
            let editing = matches!(
                &edit.montage_warp_name,
                Some((clip_path, edit_index, _))
                    if clip_path == &clip.clip && *edit_index == index
            );
            let mut name = if editing {
                edit.montage_warp_name
                    .as_ref()
                    .map(|(_, _, name)| name.clone())
                    .unwrap_or_else(|| window.name.clone())
            } else {
                window.name.clone()
            };
            let response = ui.add(
                egui::TextEdit::singleline(&mut name)
                    .desired_width(96.0)
                    .hint_text(workspace.t("desktop.anim.warpWindowHint")),
            );
            if response.has_focus() {
                edit.montage_warp_name = Some((clip.clip.clone(), index, name.clone()));
            }
            let mut start = window.start;
            let mut end = window.end;
            let start_changed = ui
                .add(
                    egui::DragValue::new(&mut start)
                        .speed(0.01)
                        .range(0.0..=span)
                        .prefix(workspace.t("desktop.anim.startPrefix")),
                )
                .changed();
            let end_changed = ui
                .add(
                    egui::DragValue::new(&mut end)
                        .speed(0.01)
                        .range(start..=span)
                        .prefix(workspace.t("desktop.anim.endPrefix")),
                )
                .changed();
            let name_changed = response.lost_focus() && name.trim() != window.name;
            if name_changed || start_changed || end_changed {
                let current_name = edit
                    .montage_warp_name
                    .as_ref()
                    .filter(|(clip_path, edit_index, _)| {
                        clip_path == &clip.clip && *edit_index == index
                    })
                    .map(|(_, _, name)| name.as_str())
                    .unwrap_or(&window.name);
                updates.push((index, current_name.to_string(), start, end));
            }
            if ui
                .button("×")
                .on_hover_text(workspace.t("desktop.anim.deleteWarpWindow"))
                .clicked()
            {
                removals.push(index);
            }
        });
    }

    let had_updates = !updates.is_empty();
    for (index, name, start, end) in updates {
        prism_editor_core::set_montage_warp_window(document, &clip.clip, index, &name, start, end)?;
        changed = true;
    }
    if had_updates {
        edit.montage_warp_name = None;
    }
    for index in removals.into_iter().rev() {
        prism_editor_core::remove_montage_warp_window(document, &clip.clip, index)?;
        changed = true;
    }

    let draft_matches = matches!(
        &edit.montage_new_warp,
        Some((clip_path, _, _, _)) if clip_path == &clip.clip
    );
    if draft_matches {
        let mut add = false;
        let mut cancel = false;
        {
            let (_, name, start, end) =
                edit.montage_new_warp.as_mut().expect("draft checked above");
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(name)
                        .desired_width(96.0)
                        .hint_text(workspace.t("desktop.anim.newWarpWindow")),
                );
                ui.add(
                    egui::DragValue::new(start)
                        .speed(0.01)
                        .range(0.0..=span)
                        .prefix(workspace.t("desktop.anim.startPrefix")),
                );
                ui.add(
                    egui::DragValue::new(end)
                        .speed(0.01)
                        .range(*start..=span)
                        .prefix(workspace.t("desktop.anim.endPrefix")),
                );
                add = ui.button(workspace.t("common.add")).clicked();
                cancel = ui.button(workspace.t("common.cancel")).clicked();
            });
        }
        if add {
            let (_, name, start, end) = edit.montage_new_warp.clone().expect("draft checked above");
            prism_editor_core::add_montage_warp_window(document, &clip.clip, &name, start, end)?;
            edit.montage_new_warp = None;
            changed = true;
        } else if cancel {
            edit.montage_new_warp = None;
        }
    } else if ui.button(workspace.t("desktop.anim.addWarp")).clicked() {
        edit.montage_new_warp = Some((clip.clip.clone(), String::new(), 0.0, span.min(0.5)));
    }

    Ok(changed)
}

pub(super) fn draw_montages_view(
    ui: &mut Ui,
    workspace: &Workspace,
    view: &prism_editor_core::MontagesView,
    document: &mut Value,
    edit: &mut AnimGraphEditState,
) -> Result<bool, String> {
    let mut changed = false;
    let mut error: Option<String> = None;
    ui.label(
        RichText::new(workspace.tf(
            "desktop.anim.montageRanges",
            &[
                ("sections", &view.section_count().to_string()),
                ("warps", &view.warp_window_count().to_string()),
            ],
        ))
        .strong()
        .size(11.0),
    );
    if view.is_empty() {
        ui.label(
            RichText::new(workspace.t("desktop.anim.noMontages"))
                .weak()
                .size(10.0),
        );
    }
    if edit.montage_new_section.is_none()
        && ui
            .button(workspace.t("desktop.anim.addClipSection"))
            .clicked()
    {
        edit.montage_new_section = Some((String::new(), String::new(), 0.0, 0.5));
    }

    for clip in &view.clips {
        ui.label(
            RichText::new(format!("{} ({})", clip.clip, clip.sections.len()))
                .strong()
                .size(10.0),
        );
        let span = clip
            .sections
            .iter()
            .map(|section| section.end)
            .chain(clip.warp_windows.iter().map(|window| window.end))
            .filter(|end| end.is_finite() && *end > 0.0)
            .fold(1.0_f32, f32::max)
            * 1.1;
        let width = ui.available_width().clamp(160.0, 560.0);
        let (track, painter) = ui.allocate_painter(Vec2::new(width, 28.0), egui::Sense::hover());
        let rect = track.rect;
        painter.rect_filled(rect, 3.0, Color32::from_rgb(26, 28, 32));
        let inset = 6.0;
        let left = rect.left() + inset;
        let usable = (rect.width() - inset * 2.0).max(1.0);
        for (index, section) in clip.sections.iter().enumerate() {
            let start = prism_editor_core::timeline_x(section.start, span, left, usable);
            let end = prism_editor_core::timeline_x(section.end, span, left, usable);
            let bar = egui::Rect::from_min_max(
                egui::pos2(start, rect.top() + 5.0),
                egui::pos2(end.max(start + 2.0), rect.bottom() - 5.0),
            );
            let color = if index % 2 == 0 {
                Color32::from_rgb(86, 142, 196)
            } else {
                Color32::from_rgb(112, 168, 124)
            };
            painter.rect_filled(bar, 2.0, color);
            if bar.width() > 28.0 {
                painter.text(
                    bar.center(),
                    egui::Align2::CENTER_CENTER,
                    &section.name,
                    egui::FontId::monospace(8.0),
                    Color32::from_rgb(18, 20, 24),
                );
            }
        }
        painter.text(
            egui::pos2(left + usable, rect.bottom() - 1.0),
            egui::Align2::RIGHT_BOTTOM,
            format!("{span:.2}s"),
            egui::FontId::monospace(8.0),
            Color32::from_rgb(110, 116, 126),
        );

        for (index, section) in clip.sections.iter().enumerate() {
            ui.horizontal(|ui| {
                let editing = matches!(
                    &edit.montage_section_name,
                    Some((clip_path, edit_index, _))
                        if clip_path == &clip.clip && *edit_index == index
                );
                let mut name = if editing {
                    edit.montage_section_name
                        .as_ref()
                        .map(|(_, _, name)| name.clone())
                        .unwrap_or_else(|| section.name.clone())
                } else {
                    section.name.clone()
                };
                let response = ui.add(
                    egui::TextEdit::singleline(&mut name)
                        .desired_width(96.0)
                        .hint_text(workspace.t("desktop.anim.sectionName")),
                );
                if response.has_focus() {
                    edit.montage_section_name = Some((clip.clip.clone(), index, name.clone()));
                }
                if response.lost_focus() && name.trim() != section.name {
                    if let Err(err) = prism_editor_core::set_montage_section(
                        document,
                        &clip.clip,
                        index,
                        &name,
                        section.start,
                        section.end,
                    ) {
                        error = Some(err);
                    } else {
                        edit.montage_section_name = None;
                        changed = true;
                    }
                }

                let mut start = section.start;
                let mut end = section.end;
                let start_changed = ui
                    .add(
                        egui::DragValue::new(&mut start)
                            .speed(0.01)
                            .range(0.0..=span)
                            .prefix(workspace.t("desktop.anim.startPrefix")),
                    )
                    .changed();
                let end_changed = ui
                    .add(
                        egui::DragValue::new(&mut end)
                            .speed(0.01)
                            .range(start..=span)
                            .prefix(workspace.t("desktop.anim.endPrefix")),
                    )
                    .changed();
                if start_changed || end_changed {
                    let current_name = edit
                        .montage_section_name
                        .as_ref()
                        .filter(|(clip_path, edit_index, _)| {
                            clip_path == &clip.clip && *edit_index == index
                        })
                        .map(|(_, _, name)| name.as_str())
                        .unwrap_or(&section.name);
                    if let Err(err) = prism_editor_core::set_montage_section(
                        document,
                        &clip.clip,
                        index,
                        current_name,
                        start,
                        end,
                    ) {
                        error = Some(err);
                    } else {
                        changed = true;
                    }
                }
                if ui
                    .button("×")
                    .on_hover_text(workspace.t("desktop.anim.deleteSection"))
                    .clicked()
                {
                    if let Err(err) =
                        prism_editor_core::remove_montage_section(document, &clip.clip, index)
                    {
                        error = Some(err);
                    } else {
                        edit.montage_section_name = None;
                        changed = true;
                    }
                }
            });
            if let Some(err) = error.take() {
                return Err(err);
            }
        }

        let draft_matches = matches!(
            &edit.montage_new_section,
            Some((clip_path, _, _, _)) if clip_path == &clip.clip
        );
        if draft_matches {
            let mut add = false;
            let mut cancel = false;
            {
                let (_, name, start, end) = edit
                    .montage_new_section
                    .as_mut()
                    .expect("draft checked above");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(name)
                            .desired_width(96.0)
                            .hint_text(workspace.t("desktop.anim.newSection")),
                    );
                    ui.add(
                        egui::DragValue::new(start)
                            .speed(0.01)
                            .range(0.0..=span)
                            .prefix(workspace.t("desktop.anim.startPrefix")),
                    );
                    ui.add(
                        egui::DragValue::new(end)
                            .speed(0.01)
                            .range(*start..=span)
                            .prefix(workspace.t("desktop.anim.endPrefix")),
                    );
                    add = ui.button(workspace.t("common.add")).clicked();
                    cancel = ui.button(workspace.t("common.cancel")).clicked();
                });
            }
            if add {
                let (_, name, start, end) = edit
                    .montage_new_section
                    .clone()
                    .expect("draft checked above");
                prism_editor_core::add_montage_section(document, &clip.clip, &name, start, end)?;
                edit.montage_new_section = None;
                changed = true;
            } else if cancel {
                edit.montage_new_section = None;
            }
        } else if ui.button(workspace.t("desktop.anim.addSection")).clicked() {
            edit.montage_new_section = Some((clip.clip.clone(), String::new(), 0.0, span.min(0.5)));
        }

        changed |= draw_montage_warp_windows(ui, workspace, clip, document, edit, span)?;
    }

    if let Some((clip, _, _, _)) = edit.montage_new_section.as_ref() {
        if !view.clips.iter().any(|entry| &entry.clip == clip) {
            ui.separator();
            let mut add = false;
            let mut cancel = false;
            {
                let (clip_name, name, start, end) = edit
                    .montage_new_section
                    .as_mut()
                    .expect("draft checked above");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(clip_name)
                            .desired_width(110.0)
                            .hint_text(workspace.t("desktop.anim.clipGlTFName")),
                    );
                    ui.add(
                        egui::TextEdit::singleline(name)
                            .desired_width(96.0)
                            .hint_text("section name"),
                    );
                    ui.add(
                        egui::DragValue::new(start)
                            .speed(0.01)
                            .range(0.0..=f32::MAX)
                            .prefix(workspace.t("desktop.anim.startPrefix")),
                    );
                    ui.add(
                        egui::DragValue::new(end)
                            .speed(0.01)
                            .range(*start..=f32::MAX)
                            .prefix(workspace.t("desktop.anim.endPrefix")),
                    );
                    add = ui.button(workspace.t("common.add")).clicked();
                    cancel = ui.button(workspace.t("common.cancel")).clicked();
                });
            }
            if add {
                let (clip_name, name, start, end) = edit
                    .montage_new_section
                    .clone()
                    .expect("draft checked above");
                prism_editor_core::add_montage_section(document, &clip_name, &name, start, end)?;
                edit.montage_new_section = None;
                changed = true;
            } else if cancel {
                edit.montage_new_section = None;
            }
        }
    }

    Ok(changed)
}

pub(super) fn draw_sync_markers_asset(
    ui: &mut Ui,
    workspace: &mut Workspace,
    state: &mut UiState,
    path: &str,
) {
    let mut value = match workspace.read_asset_json(path) {
        Ok(value) => value,
        Err(error) => {
            ui.label(
                RichText::new(format!("read {path} failed: {error}"))
                    .color(Color32::from_rgb(232, 96, 96)),
            );
            return;
        }
    };
    let view = match prism_editor_core::parse_sync_markers_view(&value) {
        Ok(view) => view,
        Err(error) => {
            ui.label(RichText::new(error).color(Color32::from_rgb(232, 96, 96)));
            return;
        }
    };
    let before = value.clone();
    match draw_sync_markers_view(ui, workspace, &view, &mut value, &mut state.anim_graph_edit) {
        Ok(true) => {
            state.status = match workspace.write_asset_json(path, &value) {
                Ok(()) => {
                    workspace.record_asset_edit(path, &before, &value);
                    format!("saved sync markers in {path}")
                }
                Err(error) => format!("save sync markers failed: {error}"),
            };
        }
        Ok(false) => {}
        Err(error) => state.status = format!("sync markers: {error}"),
    }
}

pub(super) fn draw_sync_markers_view(
    ui: &mut Ui,
    workspace: &Workspace,
    view: &prism_editor_core::SyncMarkersView,
    document: &mut Value,
    edit: &mut AnimGraphEditState,
) -> Result<bool, String> {
    let mut changed = false;
    ui.label(
        RichText::new(workspace.tf(
            "desktop.anim.syncMarkers",
            &[("count", &view.len().to_string())],
        ))
        .strong()
        .size(11.0),
    );
    if view.is_empty() {
        ui.label(
            RichText::new(workspace.t("desktop.anim.noSyncMarkers"))
                .weak()
                .size(10.0),
        );
    }

    let mut phase_edits = Vec::new();
    let mut removals = Vec::new();
    for clip in &view.clips {
        ui.label(
            RichText::new(format!("{} ({})", clip.clip, clip.phases.len()))
                .strong()
                .size(10.0),
        );
        let width = ui.available_width().clamp(140.0, 520.0);
        let (track, painter) =
            ui.allocate_painter(Vec2::new(width, 20.0), egui::Sense::click_and_drag());
        let rect = track.rect;
        painter.rect_filled(rect, 3.0, Color32::from_rgb(26, 28, 32));
        let inset = 5.0;
        let left = rect.left() + inset;
        let usable = (rect.width() - inset * 2.0).max(1.0);
        let y = rect.center().y;
        painter.line_segment(
            [egui::pos2(left, y), egui::pos2(left + usable, y)],
            egui::Stroke::new(1.0_f32, Color32::from_rgb(58, 63, 72)),
        );
        for tick in [0.25, 0.5, 0.75] {
            let x = left + tick * usable;
            painter.line_segment(
                [egui::pos2(x, y - 3.0), egui::pos2(x, y + 3.0)],
                egui::Stroke::new(1.0_f32, Color32::from_rgb(48, 52, 60)),
            );
        }
        painter.text(
            egui::pos2(left, rect.top() + 1.0),
            egui::Align2::LEFT_TOP,
            "0",
            egui::FontId::monospace(8.0),
            Color32::from_rgb(110, 116, 126),
        );
        painter.text(
            egui::pos2(left + usable, rect.top() + 1.0),
            egui::Align2::RIGHT_TOP,
            "1",
            egui::FontId::monospace(8.0),
            Color32::from_rgb(110, 116, 126),
        );
        let x_of = |phase: f32| left + phase.clamp(0.0, 1.0) * usable;
        if track.drag_started() {
            edit.sync_marker_drag = track
                .interact_pointer_pos()
                .and_then(|pos| {
                    prism_editor_core::nearest_sync_marker_index(
                        &clip.phases,
                        pos.x,
                        left,
                        usable,
                        8.0,
                    )
                })
                .map(|index| (clip.clip.clone(), index));
        }
        if track.dragged() {
            if let (Some(pos), Some((owner, index))) =
                (track.interact_pointer_pos(), edit.sync_marker_drag.as_ref())
            {
                if owner == &clip.clip && *index < clip.phases.len() {
                    phase_edits.push((
                        clip.clip.clone(),
                        *index,
                        prism_editor_core::sync_marker_phase_at(pos.x, left, usable),
                    ));
                }
            }
        }
        if track.drag_stopped() {
            edit.sync_marker_drag = None;
        }
        if track.clicked() {
            if let Some(pos) = track.interact_pointer_pos() {
                let hit = prism_editor_core::nearest_sync_marker_index(
                    &clip.phases,
                    pos.x,
                    left,
                    usable,
                    8.0,
                );
                if hit.is_none() {
                    edit.sync_marker_draft = Some((
                        clip.clip.clone(),
                        prism_editor_core::sync_marker_phase_at(pos.x, left, usable),
                    ));
                }
            }
        }
        let hovered = track.hover_pos().and_then(|pos| {
            prism_editor_core::nearest_sync_marker_index(&clip.phases, pos.x, left, usable, 8.0)
        });
        for (index, phase) in clip.phases.iter().enumerate() {
            let active = edit
                .sync_marker_drag
                .as_ref()
                .is_some_and(|(owner, slot)| owner == &clip.clip && *slot == index);
            let radius = if active || hovered == Some(index) {
                4.5
            } else {
                3.5
            };
            painter.circle_filled(
                egui::pos2(x_of(*phase), y),
                radius,
                Color32::from_rgb(112, 196, 168),
            );
        }
        if let Some(phase) = hovered.and_then(|index| clip.phases.get(index)) {
            track.clone().on_hover_text(workspace.tf(
                "desktop.anim.phaseDrag",
                &[("phase", &format!("{phase:.3}"))],
            ));
        } else if track.hovered() {
            track
                .clone()
                .on_hover_text(workspace.t("desktop.anim.clickAddSync"));
        }
        for (index, phase) in clip.phases.iter().enumerate() {
            ui.horizontal(|ui| {
                let mut phase = *phase;
                if ui
                    .add(
                        egui::DragValue::new(&mut phase)
                            .speed(0.001)
                            .range(0.0..=1.0)
                            .max_decimals(3)
                            .prefix(workspace.t("desktop.anim.phasePrefix")),
                    )
                    .changed()
                {
                    phase_edits.push((clip.clip.clone(), index, phase));
                }
                if ui.small_button("×").clicked() {
                    removals.push((clip.clip.clone(), index));
                }
            });
        }
    }

    ui.separator();
    let mut add = None;
    let mut cancel = false;
    if let Some(draft) = edit.sync_marker_draft.as_mut() {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut draft.0)
                    .hint_text(workspace.t("desktop.anim.clipGlTFName"))
                    .desired_width(120.0),
            );
            ui.add(
                egui::DragValue::new(&mut draft.1)
                    .speed(0.001)
                    .range(0.0..=1.0)
                    .max_decimals(3)
                    .prefix(workspace.t("desktop.anim.phasePrefix")),
            );
        });
        ui.horizontal(|ui| {
            if ui.button(workspace.t("desktop.anim.addMarker")).clicked() {
                add = Some(draft.clone());
            }
            if ui.button(workspace.t("common.cancel")).clicked() {
                cancel = true;
            }
        });
    } else if ui.button(workspace.t("desktop.anim.newMarker")).clicked() {
        edit.sync_marker_draft = Some((String::new(), 0.0));
    }

    for (clip, index, phase) in phase_edits {
        prism_editor_core::set_sync_marker(document, &clip, index, phase)?;
        changed = true;
    }
    for (clip, index) in removals {
        prism_editor_core::remove_sync_marker(document, &clip, index)?;
        changed = true;
    }
    if let Some((clip, phase)) = add {
        prism_editor_core::add_sync_marker(document, &clip, phase)?;
        edit.sync_marker_draft = None;
        changed = true;
    }
    if cancel {
        edit.sync_marker_draft = None;
    }
    Ok(changed)
}

pub(super) fn draw_notifies_view(
    ui: &mut Ui,
    workspace: &Workspace,
    view: &prism_editor_core::NotifiesView,
    document: &mut Value,
    edit: &mut AnimGraphEditState,
) -> Result<bool, String> {
    let mut changed = false;
    ui.label(
        RichText::new(workspace.tf(
            "desktop.anim.notifies",
            &[("count", &view.len().to_string())],
        ))
        .strong()
        .size(11.0),
    );
    if view.is_empty() {
        ui.label(
            RichText::new(workspace.t("desktop.anim.noNotifies"))
                .weak()
                .size(10.0),
        );
    }

    let mut time_edits = Vec::new();
    let mut name_edits = Vec::new();
    let mut removals = Vec::new();
    for clip in &view.clips {
        ui.label(
            RichText::new(format!("{} ({})", clip.clip, clip.entries.len()))
                .strong()
                .size(10.0),
        );
        let span = prism_editor_core::notify_timeline_span(&clip.entries);
        let width = ui.available_width().clamp(140.0, 520.0);
        let (track, painter) =
            ui.allocate_painter(Vec2::new(width, 20.0), egui::Sense::click_and_drag());
        let rect = track.rect;
        painter.rect_filled(rect, 3.0, Color32::from_rgb(26, 28, 32));
        let inset = 5.0;
        let left = rect.left() + inset;
        let usable = (rect.width() - inset * 2.0).max(1.0);
        let y = rect.center().y;
        painter.line_segment(
            [egui::pos2(left, y), egui::pos2(left + usable, y)],
            egui::Stroke::new(1.0_f32, Color32::from_rgb(58, 63, 72)),
        );
        painter.text(
            egui::pos2(left + usable, rect.top() + 1.0),
            egui::Align2::RIGHT_TOP,
            format!("{span:.1}s"),
            egui::FontId::monospace(8.0),
            Color32::from_rgb(110, 116, 126),
        );
        let x_of = |time: f32| left + (time / span).clamp(0.0, 1.0) * usable;
        if track.drag_started() {
            edit.notify_drag = track
                .interact_pointer_pos()
                .and_then(|pos| {
                    prism_editor_core::nearest_notify_index(
                        &clip.entries,
                        pos.x,
                        left,
                        usable,
                        span,
                        8.0,
                    )
                })
                .map(|index| (clip.clip.clone(), index));
        }
        if track.dragged() {
            if let (Some(pos), Some((owner, index))) =
                (track.interact_pointer_pos(), edit.notify_drag.as_ref())
            {
                if owner == &clip.clip {
                    if let Some(entry) = clip.entries.get(*index) {
                        let time = prism_editor_core::notify_time_at(pos.x, left, usable, span);
                        time_edits.push((clip.clip.clone(), *index, time, entry.name.clone()));
                    }
                }
            }
        }
        if track.drag_stopped() {
            edit.notify_drag = None;
        }
        if track.clicked() {
            if let Some(pos) = track.interact_pointer_pos() {
                let hit = prism_editor_core::nearest_notify_index(
                    &clip.entries,
                    pos.x,
                    left,
                    usable,
                    span,
                    8.0,
                );
                if hit.is_none() {
                    edit.notify_draft = Some((
                        clip.clip.clone(),
                        prism_editor_core::notify_time_at(pos.x, left, usable, span),
                        String::new(),
                    ));
                }
            }
        }
        let hovered = track.hover_pos().and_then(|pos| {
            prism_editor_core::nearest_notify_index(&clip.entries, pos.x, left, usable, span, 8.0)
        });
        for (index, entry) in clip.entries.iter().enumerate() {
            let active = edit
                .notify_drag
                .as_ref()
                .is_some_and(|(owner, slot)| owner == &clip.clip && *slot == index);
            let radius = if active || hovered == Some(index) {
                4.5
            } else {
                3.5
            };
            painter.circle_filled(
                egui::pos2(x_of(entry.time), y),
                radius,
                Color32::from_rgb(214, 178, 74),
            );
        }
        if let Some(entry) = hovered.and_then(|index| clip.entries.get(index)) {
            track.clone().on_hover_text(workspace.tf(
                "desktop.anim.notifyDrag",
                &[
                    ("name", &entry.name),
                    ("time", &format!("{:.2}", entry.time)),
                ],
            ));
        } else if track.hovered() {
            track
                .clone()
                .on_hover_text(workspace.t("desktop.anim.clickAddNotify"));
        }
        for (index, entry) in clip.entries.iter().enumerate() {
            ui.horizontal(|ui| {
                let mut time = entry.time;
                if ui
                    .add(
                        egui::DragValue::new(&mut time)
                            .speed(0.01)
                            .range(0.0..=60.0)
                            .prefix(workspace.t("desktop.anim.timePrefix")),
                    )
                    .changed()
                {
                    time_edits.push((clip.clip.clone(), index, time, entry.name.clone()));
                }
                let editing = edit
                    .notify_name
                    .as_ref()
                    .filter(|(owner, slot, _)| owner == &clip.clip && *slot == index);
                let mut text = editing
                    .map(|(_, _, text)| text.clone())
                    .unwrap_or_else(|| entry.name.clone());
                let response = ui.add(
                    egui::TextEdit::singleline(&mut text)
                        .hint_text("Footstep.L")
                        .desired_width(150.0),
                );
                if response.changed() {
                    edit.notify_name = Some((clip.clip.clone(), index, text));
                } else if response.lost_focus() {
                    if let Some((owner, slot, text)) = edit.notify_name.take() {
                        if owner == clip.clip && slot == index {
                            name_edits.push((owner, slot, text, entry.time));
                        }
                    }
                }
                if ui.small_button("×").clicked() {
                    removals.push((clip.clip.clone(), index));
                }
            });
        }
    }

    ui.separator();
    let mut add = None;
    let mut cancel = false;
    if let Some(draft) = edit.notify_draft.as_mut() {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut draft.0)
                    .hint_text("clip (glTF name)")
                    .desired_width(120.0),
            );
            ui.add(
                egui::DragValue::new(&mut draft.1)
                    .speed(0.01)
                    .range(0.0..=60.0)
                    .prefix(workspace.t("desktop.anim.timePrefix")),
            );
            ui.add(
                egui::TextEdit::singleline(&mut draft.2)
                    .hint_text("Footstep.L")
                    .desired_width(140.0),
            );
        });
        ui.horizontal(|ui| {
            if ui.button(workspace.t("desktop.anim.addNotify")).clicked() {
                add = Some(draft.clone());
            }
            if ui.button(workspace.t("common.cancel")).clicked() {
                cancel = true;
            }
        });
    } else if ui.button(workspace.t("desktop.anim.newNotify")).clicked() {
        edit.notify_draft = Some((String::new(), 0.0, String::new()));
    }

    for (clip, index, time, name) in time_edits {
        prism_editor_core::set_notify(document, &clip, index, time, &name)?;
        changed = true;
    }
    for (clip, index, name, time) in name_edits {
        prism_editor_core::set_notify(document, &clip, index, time, &name)?;
        changed = true;
    }
    for (clip, index) in removals {
        prism_editor_core::remove_notify(document, &clip, index)?;
        changed = true;
    }
    if let Some((clip, time, name)) = add {
        prism_editor_core::add_notify(document, &clip, time, &name)?;
        edit.notify_draft = None;
        changed = true;
    }
    if cancel {
        edit.notify_draft = None;
    }
    Ok(changed)
}

pub(super) fn draw_anim_graph_layout(
    ui: &mut Ui,
    graph: &prism_editor_core::AnimGraphView,
    layout: &mut prism_editor_core::AnimGraphLayout,
) -> bool {
    let width = ui.available_width().clamp(240.0, 560.0);
    let height = 300.0;
    let (response, painter) =
        ui.allocate_painter(Vec2::new(width, height), Sense::click_and_drag());
    painter.rect_filled(response.rect, 4.0, Color32::from_rgb(28, 30, 34));

    for (index, node) in graph.nodes.iter().enumerate() {
        if layout.position(&node.name).is_none() {
            let position = [
                40.0 + (index % 5) as f32 * 120.0,
                30.0 + (index / 5) as f32 * 70.0,
            ];
            layout.set_position(node.name.clone(), position);
        }
    }

    let origin = response.rect.min;
    let to_screen = |position: [f32; 2]| egui::pos2(origin.x + position[0], origin.y + position[1]);

    for node in &graph.nodes {
        let Some(target) = layout.position(&node.name) else {
            continue;
        };
        for input in &node.inputs {
            let Some(source) = layout.position(&input.node) else {
                continue;
            };
            painter.line_segment(
                [to_screen(source), to_screen(target)],
                egui::Stroke::new(1.0_f32, Color32::from_rgb(88, 96, 108)),
            );
        }
    }

    let mut changed = false;
    for node in &graph.nodes {
        let Some(position) = layout.position(&node.name) else {
            continue;
        };
        let center = to_screen(position);
        let rect = egui::Rect::from_center_size(center, Vec2::new(112.0, 34.0));
        let response = ui.interact(
            rect,
            ui.id().with(("anim-graph-layout", &node.name)),
            Sense::drag(),
        );
        if response.dragged() {
            let delta = response.drag_delta();
            let next = [
                (position[0] + delta.x).clamp(12.0, width - 12.0),
                (position[1] + delta.y).clamp(12.0, height - 12.0),
            ];
            layout.set_position(node.name.clone(), next);
            changed = true;
        }
        let fill = if node.is_output() {
            Color32::from_rgb(52, 74, 52)
        } else {
            Color32::from_rgb(48, 54, 64)
        };
        painter.rect_filled(rect, 4.0, fill);
        painter.rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(1.0_f32, Color32::from_rgb(120, 132, 150)),
            egui::StrokeKind::Inside,
        );
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            &node.name,
            egui::FontId::monospace(11.0),
            Color32::from_rgb(230, 234, 240),
        );
    }
    changed
}

#[derive(Debug, Default)]
pub struct AnimGraphEditState {
    pub new_node: (String, String),

    pub new_node_kind: String,

    pub mask: Option<(String, usize, String)>,

    pub sm_state_name: Option<(usize, String)>,
    pub sm_state_clip: Option<(usize, String)>,

    pub sm_new_state: Option<(String, String)>,

    pub sm_state_rename: Option<(usize, String)>,

    pub sm_condition_name: Option<(usize, usize, String)>,

    pub graph_node_clip: Option<(String, String)>,

    pub graph_node_name: Option<(String, String)>,

    pub graph_node_parameter: Option<(String, String)>,
    pub graph_node_parameter_y: Option<(String, String)>,

    pub graph_sample: Option<(String, String, f32, f32)>,

    pub graph_parameter: Option<(String, String, f32)>,

    pub graph_parameter_name: Option<(String, String)>,

    pub graph_parameter_rename: Option<(String, String)>,

    pub notify_name: Option<(String, usize, String)>,

    pub notify_draft: Option<(String, f32, String)>,

    pub notify_drag: Option<(String, usize)>,

    pub sync_marker_drag: Option<(String, usize)>,

    pub sync_marker_draft: Option<(String, f32)>,

    pub montage_section_name: Option<(String, usize, String)>,

    pub montage_new_section: Option<(String, String, f32, f32)>,

    pub montage_warp_name: Option<(String, usize, String)>,

    pub montage_new_warp: Option<(String, String, f32, f32)>,

    pub graph_sample_drag: Option<(String, usize)>,
}

pub(super) const GRAPH_NODE_KINDS: [(&str, &str); 6] = [
    ("clip_player", "desktop.anim.nodeClip"),
    ("blend_stack", "desktop.anim.nodeBlendStack"),
    ("slot", "desktop.anim.nodeSlot"),
    ("layered_blend", "desktop.anim.nodeLayeredBlend"),
    ("cached_pose", "desktop.anim.nodeCachedPose"),
    ("state_machine", "desktop.anim.nodeStateMachine"),
];

pub(super) fn graph_node_kind_key(kind: &str) -> &str {
    GRAPH_NODE_KINDS
        .iter()
        .find(|(value, _)| *value == kind)
        .map(|(_, key)| *key)
        .unwrap_or(kind)
}

pub(super) fn is_clip_player(kind: &str) -> bool {
    matches!(
        kind.to_ascii_lowercase().as_str(),
        "clip" | "clip_player" | "clipplayer"
    )
}

pub(super) fn is_blend_space(kind: &str) -> bool {
    matches!(
        kind.to_ascii_lowercase().as_str(),
        "blend_space" | "blend_space_1d" | "blendspace1d" | "blend_space_2d" | "blendspace2d"
    )
}

pub(super) fn is_blend_space_2d(kind: &str) -> bool {
    matches!(
        kind.to_ascii_lowercase().as_str(),
        "blend_space_2d" | "blendspace2d"
    )
}

pub(super) fn draw_anim_graph_view(
    ui: &mut Ui,
    workspace: &Workspace,
    graph: &prism_editor_core::AnimGraphView,
    document: &mut Value,
    edit: &mut AnimGraphEditState,
) -> Result<bool, String> {
    let mut changed = false;
    ui.label(
        RichText::new(workspace.tf(
            "desktop.anim.parameters",
            &[("count", &graph.parameters.len().to_string())],
        ))
        .strong()
        .size(11.0),
    );
    if graph.parameters.is_empty() {
        ui.label(
            RichText::new(workspace.t("desktop.anim.noParameters"))
                .weak()
                .size(10.0),
        );
    }
    let mut parameter_default_edits = Vec::new();
    let mut parameter_removals = Vec::new();
    for parameter in &graph.parameters {
        ui.horizontal(|ui| {
            let editing_name = edit
                .graph_parameter_name
                .as_ref()
                .filter(|(owner, _)| owner == &parameter.name);
            let mut name_text = editing_name
                .map(|(_, text)| text.clone())
                .unwrap_or_else(|| parameter.name.clone());
            let name_response = ui.add(
                egui::TextEdit::singleline(&mut name_text)
                    .desired_width(110.0)
                    .font(egui::TextStyle::Monospace),
            );
            if name_response.changed() {
                edit.graph_parameter_name = Some((parameter.name.clone(), name_text));
            } else if name_response.lost_focus() {
                if let Some((owner, text)) = edit.graph_parameter_name.take() {
                    if owner == parameter.name {
                        let new_name = text.trim().to_string();
                        if new_name != parameter.name {
                            edit.graph_parameter_rename = Some((parameter.name.clone(), new_name));
                        }
                    }
                }
            }
            ui.label(
                RichText::new(format!("· {}", parameter.kind))
                    .monospace()
                    .size(10.0),
            );
            let bool_like = !parameter.kind.eq_ignore_ascii_case("float");
            let mut value = parameter.default;
            let drag = egui::DragValue::new(&mut value).speed(0.01);
            let drag = if bool_like {
                drag.range(0.0..=1.0).max_decimals(0)
            } else {
                drag.range(-1000.0..=1000.0)
            };
            if ui.add(drag).changed() {
                parameter_default_edits.push((parameter.name.clone(), value));
            }
            if ui
                .small_button("×")
                .on_hover_text(workspace.t("desktop.anim.removeParameter"))
                .clicked()
            {
                parameter_removals.push(parameter.name.clone());
            }
        });
    }
    let mut add_parameter = None;
    let mut cancel_parameter = false;
    if let Some(draft) = edit.graph_parameter.as_mut() {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut draft.0)
                    .hint_text(workspace.t("desktop.anim.nameHint"))
                    .desired_width(110.0),
            );
            egui::ComboBox::from_id_salt("anim-graph-param-kind")
                .selected_text(&draft.1)
                .show_ui(ui, |ui| {
                    for (kind, key) in [
                        ("float", "desktop.anim.kindFloat"),
                        ("bool", "desktop.anim.kindBool"),
                        ("trigger", "desktop.anim.kindTrigger"),
                    ] {
                        if ui
                            .selectable_label(draft.1 == kind, workspace.t(key))
                            .clicked()
                        {
                            draft.1 = kind.to_string();
                        }
                    }
                });
            let bool_like = draft.1 != "float";
            let drag = egui::DragValue::new(&mut draft.2).speed(0.01);
            let drag = if bool_like {
                drag.range(0.0..=1.0).max_decimals(0)
            } else {
                drag.range(-1000.0..=1000.0)
            };
            ui.add(drag);
        });
        ui.horizontal(|ui| {
            if ui
                .button(workspace.t("desktop.anim.addParameter"))
                .clicked()
            {
                add_parameter = Some(draft.clone());
            }
            if ui.button(workspace.t("common.cancel")).clicked() {
                cancel_parameter = true;
            }
        });
    } else if ui
        .button(workspace.t("desktop.anim.newParameter"))
        .clicked()
    {
        edit.graph_parameter = Some((String::new(), "float".to_string(), 0.0));
    }
    for (name, value) in parameter_default_edits {
        prism_editor_core::set_parameter_default(document, &name, value)?;
        changed = true;
    }
    for name in parameter_removals {
        prism_editor_core::remove_parameter(document, &name)?;
        changed = true;
    }
    if let Some((name, kind, default)) = add_parameter {
        prism_editor_core::add_parameter(document, &name, &kind, default)?;
        edit.graph_parameter = None;
        changed = true;
    }
    if cancel_parameter {
        edit.graph_parameter = None;
    }
    ui.separator();

    ui.label(
        RichText::new(workspace.tf(
            "desktop.anim.nodes",
            &[("count", &graph.nodes.len().to_string())],
        ))
        .strong()
        .size(11.0),
    );
    let mut remove_node = None;
    let mut duplicate_node = None;
    let mut add_link = None;
    let mut remove_link = None;
    let mut weight_edits = Vec::new();
    let mut mask_edits = Vec::new();
    let mut node_clip_edits = Vec::new();
    let mut node_speed_edits = Vec::new();
    let mut node_weight_edits = Vec::new();
    let mut node_loop_edits = Vec::new();
    let mut node_name_edits = Vec::new();
    let mut parameter_edits = Vec::new();
    let mut position_edits = Vec::new();
    let mut sample_speed_edits = Vec::new();
    let mut sample_removals = Vec::new();
    let mut sample_adds = Vec::new();
    let mut sample_draft_cancel = false;
    for node in &graph.nodes {
        let kind = workspace.t(graph_node_kind_key(&node.kind));
        let title = if node.is_output() {
            workspace.tf(
                "desktop.anim.nodeTitleOutput",
                &[("name", &node.name), ("kind", &kind)],
            )
        } else {
            workspace.tf(
                "desktop.anim.nodeTitle",
                &[("name", &node.name), ("kind", &kind)],
            )
        };
        egui::CollapsingHeader::new(RichText::new(title).monospace().size(10.0))
            .id_salt(("anim-graph-node", &node.name))
            .show(ui, |ui| {
                if !node.is_output() {
                    ui.horizontal(|ui| {
                        if ui
                            .small_button(workspace.t("desktop.anim.duplicate"))
                            .on_hover_text(workspace.t("desktop.anim.duplicateNodeHint"))
                            .clicked()
                        {
                            duplicate_node = Some(node.name.clone());
                        }
                        if ui
                            .small_button(workspace.t("desktop.anim.removeNode"))
                            .clicked()
                        {
                            remove_node = Some(node.name.clone());
                        }
                    });
                }
                let editing_name = edit
                    .graph_node_name
                    .as_ref()
                    .filter(|(owner, _)| owner == &node.name);
                let mut name_text = editing_name
                    .map(|(_, text)| text.clone())
                    .unwrap_or_else(|| node.name.clone());
                let name_response = ui.add(
                    egui::TextEdit::singleline(&mut name_text)
                        .hint_text(workspace.t("desktop.anim.nodeName"))
                        .desired_width(140.0),
                );
                if name_response.changed() {
                    edit.graph_node_name = Some((node.name.clone(), name_text));
                } else if name_response.lost_focus() {
                    if let Some((owner, text)) = edit.graph_node_name.take() {
                        if owner == node.name {
                            node_name_edits.push((owner, text));
                        }
                    }
                }
                if !node.parameter.is_empty() {
                    ui.label(
                        RichText::new(
                            workspace
                                .tf("desktop.anim.parameterLine", &[("name", &node.parameter)]),
                        )
                        .weak()
                        .size(10.0),
                    );
                }
                if node.state_machine.is_empty() && node.kind.eq_ignore_ascii_case("state_machine")
                {
                    ui.label(
                        RichText::new(workspace.t("desktop.anim.stateMachineEmpty"))
                            .weak()
                            .size(10.0),
                    );
                }
                if is_clip_player(&node.kind) {
                    let editing = edit
                        .graph_node_clip
                        .as_ref()
                        .filter(|(owner, _)| owner == &node.name);
                    let mut text = editing
                        .map(|(_, text)| text.clone())
                        .unwrap_or_else(|| node.clips.first().cloned().unwrap_or_default());
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut text)
                            .hint_text("clip.ranim")
                            .desired_width(180.0),
                    );
                    if response.changed() {
                        edit.graph_node_clip = Some((node.name.clone(), text));
                    } else if response.lost_focus() {
                        if let Some((owner, text)) = edit.graph_node_clip.take() {
                            if owner == node.name {
                                node_clip_edits.push((node.name.clone(), text));
                            }
                        }
                    }
                } else if !node.clips.is_empty() {
                    ui.label(
                        RichText::new(workspace.tf(
                            "desktop.anim.clipsLine",
                            &[("clips", &node.clips.join(", "))],
                        ))
                        .weak()
                        .size(10.0),
                    );
                }
                ui.horizontal(|ui| {
                    let mut speed = node.speed;
                    if ui
                        .add(
                            egui::DragValue::new(&mut speed)
                                .speed(0.01)
                                .range(0.0..=5.0)
                                .prefix(workspace.t("desktop.anim.speedPrefix")),
                        )
                        .changed()
                    {
                        node_speed_edits.push((node.name.clone(), speed));
                    }
                    let mut weight = node.weight;
                    if ui
                        .add(
                            egui::DragValue::new(&mut weight)
                                .speed(0.01)
                                .range(0.0..=1.0)
                                .prefix(workspace.t("desktop.anim.weightPrefix")),
                        )
                        .changed()
                    {
                        node_weight_edits.push((node.name.clone(), weight));
                    }
                    let mut looped = node.looped;
                    if ui
                        .checkbox(&mut looped, workspace.t("desktop.anim.loopLower"))
                        .changed()
                    {
                        node_loop_edits.push((node.name.clone(), looped));
                    }
                });
                if is_blend_space(&node.kind) {
                    let two_d = is_blend_space_2d(&node.kind);
                    ui.horizontal(|ui| {
                        let editing_x = edit
                            .graph_node_parameter
                            .as_ref()
                            .filter(|(owner, _)| owner == &node.name);
                        let mut x = editing_x
                            .map(|(_, text)| text.clone())
                            .unwrap_or_else(|| node.parameter.clone());
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut x)
                                .hint_text(workspace.t("desktop.anim.parameterHint"))
                                .desired_width(100.0),
                        );
                        if response.changed() {
                            edit.graph_node_parameter = Some((node.name.clone(), x));
                        } else if response.lost_focus() {
                            if let Some((owner, text)) = edit.graph_node_parameter.take() {
                                if owner == node.name {
                                    parameter_edits.push((owner, text, node.parameter_y.clone()));
                                }
                            }
                        }
                        if two_d {
                            let editing_y = edit
                                .graph_node_parameter_y
                                .as_ref()
                                .filter(|(owner, _)| owner == &node.name);
                            let mut y = editing_y
                                .map(|(_, text)| text.clone())
                                .unwrap_or_else(|| node.parameter_y.clone());
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut y)
                                    .hint_text(workspace.t("desktop.anim.parameterYHint"))
                                    .desired_width(100.0),
                            );
                            if response.changed() {
                                edit.graph_node_parameter_y = Some((node.name.clone(), y));
                            } else if response.lost_focus() {
                                if let Some((owner, text)) = edit.graph_node_parameter_y.take() {
                                    if owner == node.name {
                                        parameter_edits.push((owner, node.parameter.clone(), text));
                                    }
                                }
                            }
                        }
                    });
                    for (index, clip) in node.clips.iter().enumerate() {
                        let position = node.positions.get(index).copied().unwrap_or([0.0, 0.0]);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("#{index} {clip}"))
                                    .monospace()
                                    .size(10.0),
                            );
                            let mut x = position[0];
                            if ui
                                .add(egui::DragValue::new(&mut x).speed(0.01).prefix("x "))
                                .changed()
                            {
                                position_edits.push((node.name.clone(), index, [x, position[1]]));
                            }
                            if two_d {
                                let mut y = position[1];
                                if ui
                                    .add(egui::DragValue::new(&mut y).speed(0.01).prefix("y "))
                                    .changed()
                                {
                                    position_edits.push((
                                        node.name.clone(),
                                        index,
                                        [position[0], y],
                                    ));
                                }
                            }
                            let mut speed = node.speeds.get(index).copied().unwrap_or(1.0);
                            if ui
                                .add(
                                    egui::DragValue::new(&mut speed)
                                        .speed(0.01)
                                        .range(0.0..=10.0)
                                        .prefix(workspace.t("desktop.anim.speedPrefix")),
                                )
                                .changed()
                            {
                                sample_speed_edits.push((node.name.clone(), index, speed));
                            }
                            if ui.small_button("×").clicked() {
                                sample_removals.push((node.name.clone(), index));
                            }
                        });
                    }
                    if two_d
                        && node.positions.len() >= 2
                        && node
                            .positions
                            .iter()
                            .all(|position| position[0].is_finite() && position[1].is_finite())
                    {
                        let positions = &node.positions;
                        let mut min_x =
                            positions.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min);
                        let mut max_x = positions
                            .iter()
                            .map(|p| p[0])
                            .fold(f32::NEG_INFINITY, f32::max);
                        let mut min_y =
                            positions.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
                        let mut max_y = positions
                            .iter()
                            .map(|p| p[1])
                            .fold(f32::NEG_INFINITY, f32::max);
                        if (max_x - min_x).abs() < 1e-3 {
                            min_x -= 0.5;
                            max_x += 0.5;
                        }
                        if (max_y - min_y).abs() < 1e-3 {
                            min_y -= 0.5;
                            max_y += 0.5;
                        }
                        let pad_x = (max_x - min_x) * 0.15;
                        let pad_y = (max_y - min_y) * 0.15;
                        min_x -= pad_x;
                        max_x += pad_x;
                        min_y -= pad_y;
                        max_y += pad_y;

                        let (canvas, painter) = ui.allocate_painter(
                            Vec2::new(180.0, 120.0),
                            egui::Sense::click_and_drag(),
                        );
                        let rect = canvas.rect;
                        painter.rect_filled(rect, 3.0, Color32::from_rgb(28, 30, 34));
                        let left = rect.left() + 8.0;
                        let width = rect.width() - 16.0;
                        let bottom = rect.bottom() - 8.0;
                        let height = rect.height() - 16.0;
                        let sx =
                            |x: f32| left + ((x - min_x) / (max_x - min_x)).clamp(0.0, 1.0) * width;
                        let sy = |y: f32| {
                            bottom - ((y - min_y) / (max_y - min_y)).clamp(0.0, 1.0) * height
                        };
                        let to_x = |px: f32| {
                            min_x + ((px - left) / width).clamp(0.0, 1.0) * (max_x - min_x)
                        };
                        let to_y = |py: f32| {
                            min_y + ((bottom - py) / height).clamp(0.0, 1.0) * (max_y - min_y)
                        };

                        if canvas.drag_started() {
                            edit.graph_sample_drag =
                                canvas.interact_pointer_pos().and_then(|pos| {
                                    positions
                                        .iter()
                                        .enumerate()
                                        .map(|(index, position)| {
                                            (
                                                index,
                                                (sx(position[0]) - pos.x)
                                                    .hypot(sy(position[1]) - pos.y),
                                            )
                                        })
                                        .filter(|(_, distance)| *distance <= 10.0)
                                        .min_by(|a, b| a.1.total_cmp(&b.1))
                                        .map(|(index, _)| (node.name.clone(), index))
                                });
                        }
                        if canvas.dragged() {
                            if let (Some(pos), Some((owner, index))) = (
                                canvas.interact_pointer_pos(),
                                edit.graph_sample_drag.as_ref(),
                            ) {
                                if owner == &node.name && *index < positions.len() {
                                    position_edits.push((
                                        node.name.clone(),
                                        *index,
                                        [to_x(pos.x), to_y(pos.y)],
                                    ));
                                }
                            }
                        }
                        if canvas.drag_stopped() {
                            edit.graph_sample_drag = None;
                        }
                        for (index, position) in positions.iter().enumerate() {
                            let active = edit
                                .graph_sample_drag
                                .as_ref()
                                .is_some_and(|(owner, slot)| owner == &node.name && *slot == index);
                            painter.circle_filled(
                                egui::pos2(sx(position[0]), sy(position[1])),
                                if active { 5.0 } else { 3.5 },
                                Color32::from_rgb(120, 170, 230),
                            );
                        }
                        if prism_editor_core::blend_space_2d_degenerate(positions) {
                            ui.label(
                                RichText::new(workspace.t("desktop.anim.degenerate"))
                                    .color(Color32::from_rgb(232, 160, 96))
                                    .size(9.0),
                            );
                        }
                        ui.label(
                            RichText::new(workspace.tf(
                                "desktop.anim.blendBounds",
                                &[
                                    ("min_x", &format!("{min_x:.2}")),
                                    ("max_x", &format!("{max_x:.2}")),
                                    ("min_y", &format!("{min_y:.2}")),
                                    ("max_y", &format!("{max_y:.2}")),
                                ],
                            ))
                            .weak()
                            .size(9.0),
                        );
                    }
                    let draft_here = edit
                        .graph_sample
                        .as_ref()
                        .is_some_and(|(owner, ..)| owner == &node.name);
                    if draft_here {
                        if let Some(draft) = edit.graph_sample.as_mut() {
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut draft.1)
                                        .hint_text("clip.ranim")
                                        .desired_width(120.0),
                                );
                                ui.add(egui::DragValue::new(&mut draft.2).speed(0.01).prefix("x "));
                                if two_d {
                                    ui.add(
                                        egui::DragValue::new(&mut draft.3).speed(0.01).prefix("y "),
                                    );
                                }
                                if ui.small_button(workspace.t("common.add")).clicked() {
                                    sample_adds.push(draft.clone());
                                }
                                if ui.small_button(workspace.t("common.cancel")).clicked() {
                                    sample_draft_cancel = true;
                                }
                            });
                        }
                    } else if ui
                        .small_button(workspace.t("desktop.anim.addSample"))
                        .clicked()
                    {
                        edit.graph_sample = Some((node.name.clone(), String::new(), 0.0, 0.0));
                    }
                    if node.clips.len() != node.positions.len() {
                        ui.label(
                            RichText::new(workspace.tf(
                                "desktop.anim.clipsVsPositions",
                                &[
                                    ("clips", &node.clips.len().to_string()),
                                    ("positions", &node.positions.len().to_string()),
                                ],
                            ))
                            .color(Color32::from_rgb(232, 160, 96))
                            .size(10.0),
                        );
                    }
                    if !node.speeds.is_empty() && node.speeds.len() != node.clips.len() {
                        ui.label(
                            RichText::new(workspace.tf(
                                "desktop.anim.clipsVsSpeeds",
                                &[
                                    ("clips", &node.clips.len().to_string()),
                                    ("speeds", &node.speeds.len().to_string()),
                                ],
                            ))
                            .color(Color32::from_rgb(232, 160, 96))
                            .size(10.0),
                        );
                    }
                }
                if !node.state_machine.is_empty() {
                    ui.label(
                        RichText::new(workspace.tf(
                            "desktop.anim.stateMachineLine",
                            &[("path", &node.state_machine)],
                        ))
                        .weak()
                        .size(10.0),
                    );
                }
                if !node.parameter.is_empty() {
                    ui.label(
                        RichText::new(format!("parameter: {}", node.parameter))
                            .weak()
                            .size(10.0),
                    );
                }
                for input in &node.inputs {
                    ui.label(
                        RichText::new(format!("← {}  × {}", input.node, input.weight))
                            .monospace()
                            .size(10.0),
                    );
                }

                let raw = document
                    .get_mut("nodes")
                    .and_then(Value::as_array_mut)
                    .and_then(|nodes| {
                        nodes.iter_mut().find(|value| {
                            value.get("name").and_then(Value::as_str) == Some(&node.name)
                        })
                    });
                if let Some(raw) = raw {
                    if let Some(inputs) = raw.get("inputs").and_then(Value::as_array) {
                        let mut remove = None;
                        for (index, input) in inputs.iter().enumerate() {
                            let (name, mut weight, mask) = match input {
                                Value::String(name) => (name.clone(), 1.0, Vec::new()),
                                Value::Object(map) => (
                                    map.get("node")
                                        .and_then(Value::as_str)
                                        .unwrap_or("?")
                                        .to_string(),
                                    map.get("weight").and_then(Value::as_f64).unwrap_or(1.0) as f32,
                                    map.get("bone_mask")
                                        .and_then(Value::as_array)
                                        .map(Vec::as_slice)
                                        .unwrap_or_default()
                                        .iter()
                                        .filter_map(Value::as_str)
                                        .map(str::to_string)
                                        .collect::<Vec<_>>(),
                                ),
                                _ => continue,
                            };
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(workspace.tf(
                                        "desktop.anim.linkLine",
                                        &[("index", &index.to_string()), ("name", &name)],
                                    ))
                                    .size(10.0),
                                );
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut weight)
                                            .speed(0.01)
                                            .range(0.0..=1.0)
                                            .prefix(workspace.t("desktop.anim.weightPrefix")),
                                    )
                                    .changed()
                                {
                                    weight_edits.push((node.name.clone(), index, weight));
                                }
                                if ui.small_button("×").clicked() {
                                    remove = Some(index);
                                }
                            });

                            let editing_mask =
                                edit.mask.as_ref().is_some_and(|(owner, slot, _)| {
                                    owner == &node.name && *slot == index
                                });
                            let mut text = if editing_mask {
                                edit.mask
                                    .as_ref()
                                    .map(|(_, _, text)| text.clone())
                                    .unwrap_or_default()
                            } else {
                                mask.join(", ")
                            };
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut text)
                                    .hint_text(workspace.t("desktop.anim.boneMaskHint"))
                                    .desired_width(220.0),
                            );
                            if response.changed() {
                                edit.mask = Some((node.name.clone(), index, text));
                            } else if response.lost_focus() {
                                if let Some((_, _, text)) = edit.mask.take() {
                                    let joints = text
                                        .split(',')
                                        .map(|joint| joint.trim().to_string())
                                        .filter(|joint| !joint.is_empty())
                                        .collect::<Vec<_>>();
                                    mask_edits.push((node.name.clone(), index, joints));
                                }
                            }
                        }
                        if let Some(index) = remove {
                            remove_link = Some((node.name.clone(), index));
                        }
                        ui.menu_button(workspace.t("desktop.anim.addLink"), |ui| {
                            for other in &graph.nodes {
                                if other.name == node.name
                                    || node.inputs.iter().any(|input| input.node == other.name)
                                {
                                    continue;
                                }
                                if ui.button(&other.name).clicked() {
                                    add_link = Some((node.name.clone(), other.name.clone()));
                                    ui.close();
                                }
                            }
                        });
                    } else if raw.get("input").is_some() {
                        ui.label(
                            RichText::new(workspace.t("desktop.anim.singleInput"))
                                .weak()
                                .size(10.0),
                        );
                    }
                }
            });
    }
    if let Some(node) = duplicate_node {
        prism_editor_core::duplicate_graph_node(document, &node)?;
        changed = true;
    }
    if let Some(node) = remove_node {
        prism_editor_core::remove_graph_node(document, &node)?;
        changed = true;
    }
    for (old, new) in node_name_edits {
        prism_editor_core::rename_graph_node(document, &old, &new)?;
        changed = true;
    }
    for (node, x, y) in parameter_edits {
        prism_editor_core::set_blend_space_parameter(document, &node, &x, &y)?;
        changed = true;
    }
    for (node, index, position) in position_edits {
        prism_editor_core::set_blend_space_position(document, &node, index, position)?;
        changed = true;
    }
    for (node, index, speed) in sample_speed_edits {
        prism_editor_core::set_blend_space_sample_speed(document, &node, index, speed)?;
        changed = true;
    }
    for (node, index) in sample_removals {
        prism_editor_core::remove_blend_space_sample(document, &node, index)?;
        changed = true;
    }
    for (node, clip, x, y) in sample_adds {
        prism_editor_core::add_blend_space_sample(document, &node, &clip, [x, y])?;
        edit.graph_sample = None;
        changed = true;
    }
    if sample_draft_cancel {
        edit.graph_sample = None;
    }
    for (node, clip) in node_clip_edits {
        prism_editor_core::set_graph_node_clip(document, &node, &clip)?;
        changed = true;
    }
    for (node, speed) in node_speed_edits {
        prism_editor_core::set_graph_node_speed(document, &node, speed)?;
        changed = true;
    }
    for (node, weight) in node_weight_edits {
        prism_editor_core::set_graph_node_weight(document, &node, weight)?;
        changed = true;
    }
    for (node, looped) in node_loop_edits {
        prism_editor_core::set_graph_node_looped(document, &node, looped)?;
        changed = true;
    }
    if let Some((target, source)) = add_link {
        prism_editor_core::add_graph_input(document, &target, &source)?;
        changed = true;
    }
    for (target, index, weight) in weight_edits {
        prism_editor_core::set_graph_input_weight(document, &target, index, weight)?;
        changed = true;
    }
    for (target, index, joints) in mask_edits {
        prism_editor_core::set_graph_input_bone_mask(document, &target, index, &joints)?;
        changed = true;
    }
    if let Some((target, index)) = remove_link {
        prism_editor_core::remove_graph_input(document, &target, index)?;
        changed = true;
    }

    ui.separator();
    ui.label(
        RichText::new(workspace.t("desktop.anim.addNode"))
            .strong()
            .size(11.0),
    );
    if edit.new_node_kind.is_empty() {
        edit.new_node_kind = "clip_player".to_string();
    }
    let source_hint = match edit.new_node_kind.as_str() {
        "clip_player" => "clip.ranim",
        "state_machine" => "anim/hero.rasm",
        _ => "desktop.anim.inputNode",
    };
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut edit.new_node.0)
                .hint_text("node name")
                .desired_width(96.0),
        );
        egui::ComboBox::from_id_salt("anim-graph-new-node-kind")
            .selected_text(workspace.t(graph_node_kind_key(&edit.new_node_kind)))
            .show_ui(ui, |ui| {
                for (kind, key) in GRAPH_NODE_KINDS {
                    let label = workspace.t(key);
                    ui.selectable_value(&mut edit.new_node_kind, kind.to_string(), label);
                }
            });
        ui.add(
            egui::TextEdit::singleline(&mut edit.new_node.1)
                .hint_text(if source_hint.starts_with("desktop.") {
                    workspace.t(source_hint)
                } else {
                    source_hint.to_string()
                })
                .desired_width(150.0),
        );
    });
    if ui.button(workspace.t("desktop.anim.newNode")).clicked() {
        prism_editor_core::add_graph_node(
            document,
            &edit.new_node.0,
            &edit.new_node_kind,
            &edit.new_node.1,
        )?;
        edit.new_node.0.clear();
        edit.new_node.1.clear();
        changed = true;
    }

    if let Some(output) = &graph.output {
        ui.separator();
        ui.label(
            RichText::new(workspace.tf("desktop.anim.output", &[("name", output)]))
                .strong()
                .size(11.0),
        );
    }
    Ok(changed)
}

#[derive(Debug, Clone)]
pub struct StateConditionDraft {
    pub transition: usize,
    pub kind: String,
    pub name: String,
    pub threshold: f32,
    pub value: bool,
}

pub(super) const STATE_CONDITION_KINDS: [(&str, &str); 4] = [
    ("float_greater", "float >"),
    ("float_less", "float <"),
    ("bool_is", "bool =="),
    ("trigger", "trigger"),
];

pub(super) fn condition_kind_label(kind: &str) -> &str {
    STATE_CONDITION_KINDS
        .iter()
        .find(|(value, _)| *value == kind)
        .map(|(_, label)| *label)
        .unwrap_or(kind)
}

pub(super) fn draw_state_machine_view(
    ui: &mut Ui,
    workspace: &Workspace,
    machine: &prism_editor_core::AnimStateMachineView,
    document: &mut Value,
    edit: &mut AnimGraphEditState,
    draft_slot: &mut Option<StateConditionDraft>,
) -> Result<bool, String> {
    let mut changed = false;
    ui.label(
        RichText::new(workspace.tf(
            "desktop.anim.states",
            &[("count", &machine.states.len().to_string())],
        ))
        .strong()
        .size(11.0),
    );
    ui.label(
        RichText::new(workspace.t("desktop.anim.statesHint"))
            .weak()
            .size(9.0),
    );
    let mut move_state = None;
    let mut state_rename = None;
    let mut clip_edits = Vec::new();
    let mut speed_edits = Vec::new();
    let mut loop_edits = Vec::new();
    let mut duplicate_states = Vec::new();
    let mut remove_states = Vec::new();
    let mut add_state_draft = None;
    let mut cancel_new_state = false;
    for (index, state) in machine.states.iter().enumerate() {
        ui.horizontal(|ui| {
            let mut name_text = edit
                .sm_state_name
                .as_ref()
                .filter(|(slot, _)| *slot == index)
                .map(|(_, text)| text.clone())
                .unwrap_or_else(|| state.name.clone());
            let name_response = ui.add(
                egui::TextEdit::singleline(&mut name_text)
                    .hint_text(workspace.t("desktop.anim.stateHint"))
                    .desired_width(80.0),
            );
            if name_response.changed() {
                edit.sm_state_name = Some((index, name_text));
            } else if name_response.lost_focus() {
                if let Some((slot, text)) = edit.sm_state_name.take() {
                    if slot == index {
                        state_rename = Some((index, text));
                    }
                }
            }

            let mut clip_text = edit
                .sm_state_clip
                .as_ref()
                .filter(|(slot, _)| *slot == index)
                .map(|(_, text)| text.clone())
                .unwrap_or_else(|| state.clip.clone());
            let clip_response = ui.add(
                egui::TextEdit::singleline(&mut clip_text)
                    .hint_text("clip.ranim")
                    .desired_width(130.0),
            );
            if clip_response.changed() {
                edit.sm_state_clip = Some((index, clip_text));
            } else if clip_response.lost_focus() {
                if let Some((slot, text)) = edit.sm_state_clip.take() {
                    if slot == index {
                        clip_edits.push((index, text));
                    }
                }
            }

            let mut speed = state.speed;
            if ui
                .add(
                    egui::DragValue::new(&mut speed)
                        .speed(0.01)
                        .range(0.0..=5.0)
                        .prefix(workspace.t("desktop.anim.speedPrefix")),
                )
                .changed()
            {
                speed_edits.push((index, speed));
            }
            let mut looped = state.looped;
            if ui.checkbox(&mut looped, "loop").changed() {
                loop_edits.push((index, looped));
            }
            let last = machine.states.len().saturating_sub(1);
            ui.add_enabled_ui(index > 0, |ui| {
                if ui
                    .small_button("↑")
                    .on_hover_text(workspace.t("desktop.anim.moveEarlierInitial"))
                    .clicked()
                {
                    move_state = Some((index, -1));
                }
            });
            ui.add_enabled_ui(index < last, |ui| {
                if ui
                    .small_button("↓")
                    .on_hover_text(workspace.t("desktop.anim.moveLater"))
                    .clicked()
                {
                    move_state = Some((index, 1));
                }
            });
            if ui
                .small_button("⧉")
                .on_hover_text(workspace.t("desktop.anim.duplicateState"))
                .clicked()
            {
                duplicate_states.push(state.name.clone());
            }
            ui.add_enabled_ui(machine.states.len() > 1, |ui| {
                if ui
                    .small_button("×")
                    .on_hover_text(workspace.t("desktop.anim.removeState"))
                    .clicked()
                {
                    remove_states.push(state.name.clone());
                }
            });
        });
    }
    if let Some((index, offset)) = move_state {
        prism_editor_core::move_state(document, index, offset)?;
        changed = true;
    }
    if state_rename.is_some() {
        edit.sm_state_rename = state_rename;
    }
    for (index, clip) in clip_edits {
        prism_editor_core::set_state_clip(document, index, &clip)?;
        changed = true;
    }
    for (index, speed) in speed_edits {
        prism_editor_core::set_state_speed(document, index, speed)?;
        changed = true;
    }
    for (index, looped) in loop_edits {
        prism_editor_core::set_state_looped(document, index, looped)?;
        changed = true;
    }
    for name in duplicate_states {
        prism_editor_core::duplicate_state(document, &name)?;
        changed = true;
    }
    for name in remove_states {
        prism_editor_core::remove_state(document, &name)?;
        changed = true;
    }

    if let Some(draft) = edit.sm_new_state.as_mut() {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut draft.0)
                    .hint_text("state")
                    .desired_width(80.0),
            );
            ui.add(
                egui::TextEdit::singleline(&mut draft.1)
                    .hint_text("clip.ranim")
                    .desired_width(130.0),
            );
        });
        ui.horizontal(|ui| {
            if ui.button(workspace.t("desktop.anim.addState")).clicked() {
                add_state_draft = Some(draft.clone());
            }
            if ui.button(workspace.t("common.cancel")).clicked() {
                cancel_new_state = true;
            }
        });
    } else if ui.button(workspace.t("desktop.anim.newState")).clicked() {
        edit.sm_new_state = Some((String::new(), String::new()));
    }
    if let Some((name, clip)) = add_state_draft {
        prism_editor_core::add_state(document, &name, &clip)?;
        edit.sm_new_state = None;
        changed = true;
    }
    if cancel_new_state {
        edit.sm_new_state = None;
    }

    ui.separator();
    ui.label(
        RichText::new(workspace.tf(
            "desktop.anim.transitions",
            &[("count", &machine.transitions.len().to_string())],
        ))
        .strong()
        .size(11.0),
    );
    ui.label(
        RichText::new(workspace.t("desktop.anim.transitionsHint"))
            .weak()
            .size(9.0),
    );
    let mut move_transition = None;
    let mut duplicate_transition = None;
    let mut remove = None;
    let mut timing_edits = Vec::new();
    let mut kind_edits = Vec::new();
    let mut condition_name_edits = Vec::new();
    let mut threshold_edits = Vec::new();
    let mut value_edits = Vec::new();
    let mut remove_condition = None;
    let mut add_condition = None;
    let mut cancel_draft = false;
    let mut add_transition = None;
    let mut retargets = Vec::new();
    for (index, transition) in machine.transitions.iter().enumerate() {
        let from = transition.from.as_deref().unwrap_or("*");
        let title = workspace.tf(
            "desktop.anim.transitionTitle",
            &[
                ("from", from),
                ("to", &transition.to),
                ("duration", &format!("{:.2}", transition.duration)),
                ("min", &format!("{:.2}", transition.min_state_fraction)),
            ],
        );
        egui::CollapsingHeader::new(RichText::new(title).monospace().size(10.0))
            .id_salt(("anim-sm-transition", from, &transition.to))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let last = machine.transitions.len().saturating_sub(1);
                    ui.add_enabled_ui(index > 0, |ui| {
                        if ui
                            .small_button("↑")
                            .on_hover_text(workspace.t("desktop.anim.moveEarlierPriority"))
                            .clicked()
                        {
                            move_transition = Some((index, -1));
                        }
                    });
                    ui.add_enabled_ui(index < last, |ui| {
                        if ui
                            .small_button("↓")
                            .on_hover_text(workspace.t("desktop.anim.moveLaterPriority"))
                            .clicked()
                        {
                            move_transition = Some((index, 1));
                        }
                    });
                    if ui
                        .small_button(workspace.t("desktop.anim.duplicate"))
                        .on_hover_text(workspace.t("desktop.anim.duplicateTransitionHint"))
                        .clicked()
                    {
                        duplicate_transition = Some(index);
                    }
                    if ui
                        .small_button(workspace.t("desktop.anim.removeTransition"))
                        .clicked()
                    {
                        remove = Some(index);
                    }
                });

                ui.horizontal(|ui| {
                    ui.label(workspace.t("desktop.anim.from"));
                    egui::ComboBox::from_id_salt(("anim-sm-from", index))
                        .selected_text(from)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(
                                    transition.from.is_none(),
                                    workspace.t("desktop.anim.any"),
                                )
                                .clicked()
                            {
                                retargets.push((index, None, transition.to.clone()));
                                ui.close();
                            }
                            for state in &machine.states {
                                let selected =
                                    transition.from.as_deref() == Some(state.name.as_str());
                                if ui.selectable_label(selected, &state.name).clicked() {
                                    retargets.push((
                                        index,
                                        Some(state.name.clone()),
                                        transition.to.clone(),
                                    ));
                                    ui.close();
                                }
                            }
                        });
                    ui.label(workspace.t("desktop.anim.to"));
                    egui::ComboBox::from_id_salt(("anim-sm-to", index))
                        .selected_text(&transition.to)
                        .show_ui(ui, |ui| {
                            for state in &machine.states {
                                let selected = transition.to == state.name;
                                if ui.selectable_label(selected, &state.name).clicked() {
                                    retargets.push((
                                        index,
                                        transition.from.clone(),
                                        state.name.clone(),
                                    ));
                                    ui.close();
                                }
                            }
                        });
                });

                let mut duration = transition.duration;
                let mut fraction = transition.min_state_fraction;
                let mut timing_changed = ui
                    .add(
                        egui::DragValue::new(&mut duration)
                            .speed(0.01)
                            .range(0.0..=5.0)
                            .prefix(workspace.t("desktop.anim.blendPrefix")),
                    )
                    .changed();
                timing_changed |= ui
                    .add(
                        egui::DragValue::new(&mut fraction)
                            .speed(0.01)
                            .range(0.0..=1.0)
                            .prefix(workspace.t("desktop.anim.minPrefix")),
                    )
                    .changed();
                if timing_changed {
                    timing_edits.push((index, duration, fraction));
                }

                if transition.conditions.is_empty() {
                    ui.label(
                        RichText::new(workspace.t("desktop.anim.noConditions"))
                            .weak()
                            .size(10.0),
                    );
                }
                for (condition_index, condition) in transition.conditions.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let mut kind = condition.kind.clone();
                        egui::ComboBox::from_id_salt((
                            "anim-sm-condition-kind-edit",
                            index,
                            condition_index,
                        ))
                        .selected_text(condition_kind_label(&condition.kind))
                        .show_ui(ui, |ui| {
                            for (value, label) in STATE_CONDITION_KINDS {
                                if ui
                                    .selectable_label(condition.kind.as_str() == value, label)
                                    .clicked()
                                {
                                    kind = value.to_string();
                                    ui.close();
                                }
                            }
                        });
                        if kind != condition.kind {
                            kind_edits.push((index, condition_index, kind));
                        }
                        let mut name_text = edit
                            .sm_condition_name
                            .as_ref()
                            .filter(|(transition, slot, _)| {
                                *transition == index && *slot == condition_index
                            })
                            .map(|(_, _, text)| text.clone())
                            .unwrap_or_else(|| condition.name.clone());
                        let name_response = ui.add(
                            egui::TextEdit::singleline(&mut name_text)
                                .hint_text("parameter")
                                .desired_width(90.0),
                        );
                        if name_response.changed() {
                            edit.sm_condition_name = Some((index, condition_index, name_text));
                        } else if name_response.lost_focus() {
                            if let Some((transition, slot, text)) = edit.sm_condition_name.take() {
                                if transition == index && slot == condition_index {
                                    condition_name_edits.push((index, condition_index, text));
                                }
                            }
                        }
                        match condition.kind.as_str() {
                            "float_greater" | "float_less" => {
                                let mut threshold = condition.threshold;
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut threshold)
                                            .speed(0.01)
                                            .range(-1000.0..=1000.0),
                                    )
                                    .changed()
                                {
                                    threshold_edits.push((index, condition_index, threshold));
                                }
                            }
                            "bool_is" => {
                                let mut value = condition.value;
                                if ui.checkbox(&mut value, "").changed() {
                                    value_edits.push((index, condition_index, value));
                                }
                            }
                            _ => {}
                        }
                        if ui.small_button("×").clicked() {
                            remove_condition = Some((index, condition_index));
                        }
                    });
                }

                if ui
                    .small_button(workspace.t("desktop.anim.addCondition"))
                    .clicked()
                {
                    *draft_slot = Some(StateConditionDraft {
                        transition: index,
                        kind: "float_greater".to_string(),
                        name: String::new(),
                        threshold: 0.0,
                        value: false,
                    });
                }
                if let Some(draft) = draft_slot
                    .as_mut()
                    .filter(|draft| draft.transition == index)
                {
                    ui.separator();
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt(("anim-sm-condition-kind", index))
                            .selected_text(condition_kind_label(&draft.kind))
                            .show_ui(ui, |ui| {
                                for (kind, label) in STATE_CONDITION_KINDS {
                                    ui.selectable_value(&mut draft.kind, kind.to_string(), label);
                                }
                            });
                        ui.add(
                            egui::TextEdit::singleline(&mut draft.name)
                                .hint_text("parameter")
                                .desired_width(110.0),
                        );
                        if draft.kind.starts_with("float") {
                            ui.add(
                                egui::DragValue::new(&mut draft.threshold)
                                    .speed(0.01)
                                    .range(-1000.0..=1000.0)
                                    .prefix(workspace.t("desktop.anim.thresholdPrefix")),
                            );
                        } else if draft.kind == "bool_is" {
                            ui.checkbox(&mut draft.value, workspace.t("desktop.anim.value"));
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui
                            .button(workspace.t("desktop.anim.addCondition"))
                            .clicked()
                        {
                            add_condition = Some(draft.clone());
                        }
                        if ui.button(workspace.t("common.cancel")).clicked() {
                            cancel_draft = true;
                        }
                    });
                }
            });
    }
    if let Some((index, offset)) = move_transition {
        prism_editor_core::move_state_transition(document, index, offset)?;
        changed = true;
    }
    if let Some(index) = duplicate_transition {
        prism_editor_core::duplicate_state_transition(document, index)?;
        changed = true;
    }
    for (index, from, to) in retargets {
        prism_editor_core::set_transition_states(document, index, from.as_deref(), &to)?;
        changed = true;
    }
    if let Some(index) = remove {
        prism_editor_core::remove_state_transition(document, index)?;
        changed = true;
    }
    for (index, duration, fraction) in timing_edits {
        prism_editor_core::set_transition_timing(document, index, duration, fraction)?;
        changed = true;
    }
    for (index, condition, kind) in kind_edits {
        prism_editor_core::set_condition_kind(document, index, condition, &kind)?;
        changed = true;
    }
    for (index, condition, name) in condition_name_edits {
        prism_editor_core::set_condition_name(document, index, condition, &name)?;
        changed = true;
    }
    for (index, condition, threshold) in threshold_edits {
        prism_editor_core::set_condition_threshold(document, index, condition, threshold)?;
        changed = true;
    }
    for (index, condition, value) in value_edits {
        prism_editor_core::set_condition_value(document, index, condition, value)?;
        changed = true;
    }
    if let Some((index, condition)) = remove_condition {
        prism_editor_core::remove_state_condition(document, index, condition)?;
        changed = true;
    }
    if let Some(draft) = add_condition {
        prism_editor_core::add_state_condition(
            document,
            draft.transition,
            &draft.kind,
            &draft.name,
            draft.threshold,
            draft.value,
        )?;
        *draft_slot = None;
        changed = true;
    }
    if cancel_draft {
        *draft_slot = None;
    }

    ui.menu_button(workspace.t("desktop.anim.addTransition"), |ui| {
        for from in
            std::iter::once(None).chain(machine.states.iter().map(|state| Some(&state.name)))
        {
            let label = from
                .map(String::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| workspace.t("desktop.anim.any"));
            let label = label.as_str();
            ui.menu_button(label, |ui| {
                for to in &machine.states {
                    if from.is_some_and(|from| from == &to.name) {
                        continue;
                    }
                    if ui.button(&to.name).clicked() {
                        add_transition = Some((from.map(String::as_str), to.name.clone()));
                        ui.close();
                    }
                }
            });
        }
    });
    if let Some((from, to)) = add_transition {
        prism_editor_core::add_state_transition(document, from.as_deref(), &to)?;
        changed = true;
    }
    Ok(changed)
}
