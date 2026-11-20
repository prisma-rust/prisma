//! Material panel UI.

use super::*;

pub fn open_material_asset(workspace: &Workspace, state: &mut UiState, path: &str) {
    match workspace.read_asset_json(path) {
        Ok(value) => {
            state.material_asset = Some(path.to_string());
            state.material_saved = Some(value.clone());
            state.material_value = Some(value);
            state.material_error = None;
        }
        Err(error) => {
            state.material_asset = Some(path.to_string());
            state.material_value = None;
            state.material_saved = None;
            state.material_error = Some(workspace.tf(
                "material.readFailed",
                &[("path", path), ("error", &error.to_string())],
            ));
        }
    }
}

pub fn material(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    let dirty = state.material_saved.as_ref() != state.material_value.as_ref();
    if !dirty {
        if let Some(path) = state.asset_primary().cloned() {
            if prism_editor_core::project::is_json_editable_asset(&path)
                && state.material_asset.as_deref() != Some(path.as_str())
            {
                open_material_asset(workspace, state, &path);
            }
        }
    }
    let Some(value) = state.material_value.clone() else {
        ui.vertical_centered(|ui| {
            ui.add_space(24.0);
            ui.label(RichText::new(icons::FILE).size(22.0).color(theme::color32(
                &workspace.colors(),
                prism_editor_core::ThemeToken::MutedDim,
            )));
            ui.label(
                RichText::new(workspace.t("material.empty"))
                    .weak()
                    .size(11.0),
            );
        });
        if let Some(error) = state.material_error.clone() {
            ui.label(RichText::new(error).color(Color32::from_rgb(232, 96, 96)));
        }
        return;
    };
    let path = state.material_asset.clone().unwrap_or_default();
    let dirty = state.material_saved.as_ref() != Some(&value);

    ui.horizontal(|ui| {
        ui.label(RichText::new(&path).size(11.0));
        if dirty {
            ui.label(
                RichText::new(workspace.t("material.unsaved"))
                    .size(11.0)
                    .color(Color32::from_rgb(226, 178, 84)),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(dirty, egui::Button::new(workspace.t("common.save")))
                .clicked()
            {
                match workspace.write_asset_json(&path, &value) {
                    Ok(()) => {
                        state.material_saved = Some(value.clone());
                        state.material_error = None;
                        state.status = format!("saved {path}");
                    }
                    Err(error) => {
                        state.material_error = Some(
                            workspace.tf("material.saveFailed", &[("error", &error.to_string())]),
                        )
                    }
                }
            }
            if ui
                .add_enabled(dirty, egui::Button::new(workspace.t("inspector.revert")))
                .clicked()
            {
                state.material_value = state.material_saved.clone();
            }
            if ui.button(workspace.t("material.reload")).clicked() {
                open_material_asset(workspace, state, &path);
            }
        });
    });
    if let Some(error) = state.material_error.clone() {
        ui.label(RichText::new(error).color(Color32::from_rgb(232, 96, 96)));
    }
    ui.separator();

    let Some(mut value) = state.material_value.clone() else {
        return;
    };
    egui::ScrollArea::vertical()
        .id_salt("material-fields")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if let Some(map) = value.as_object_mut() {
                let mut keys: Vec<String> = map.keys().cloned().collect();
                keys.sort();
                egui::Grid::new("material-grid")
                    .num_columns(2)
                    .spacing([10.0, 4.0])
                    .show(ui, |ui| {
                        for key in keys {
                            ui.label(RichText::new(&key).size(11.0));
                            let slot = map.get_mut(&key).expect("key came from the map");
                            material_field(ui, workspace, &key, slot);
                            ui.end_row();
                        }
                    });
            } else {
                ui.label(RichText::new(workspace.t("material.notObject")).weak());
                ui.label(RichText::new(value.to_string()).monospace().size(10.0));
            }
        });
    state.material_value = Some(value);
}

pub(super) fn material_field(
    ui: &mut Ui,
    workspace: &Workspace,
    key: &str,
    value: &mut serde_json::Value,
) {
    match value {
        serde_json::Value::Bool(flag) => {
            ui.checkbox(flag, "");
        }
        serde_json::Value::Number(number) => {
            let mut item = number.as_f64().unwrap_or(0.0);
            let speed = if key.contains("rough") || key.contains("metal") {
                0.005
            } else {
                0.01
            };
            if ui
                .add(
                    egui::DragValue::new(&mut item)
                        .speed(speed)
                        .max_decimals(4)
                        .custom_parser(crate::widgets::number_parser),
                )
                .changed()
            {
                *value = serde_json::json!(item);
            }
        }
        serde_json::Value::String(text) => {
            ui.add(egui::TextEdit::singleline(text).desired_width(220.0));
        }
        serde_json::Value::Array(items) => {
            let numbers: Option<Vec<f64>> = items.iter().map(|item| item.as_f64()).collect();
            match numbers {
                Some(channels) if channels.len() == 4 && key.contains("color") => {
                    let mut rgba = [
                        channels[0] as f32,
                        channels[1] as f32,
                        channels[2] as f32,
                        channels[3] as f32,
                    ];
                    if ui.color_edit_button_rgba_unmultiplied(&mut rgba).changed() {
                        *value = serde_json::json!([rgba[0], rgba[1], rgba[2], rgba[3]]);
                    }
                }
                Some(channels) => {
                    let mut channels = channels;
                    let mut changed = false;
                    ui.horizontal(|ui| {
                        for channel in channels.iter_mut() {
                            if ui
                                .add(
                                    egui::DragValue::new(channel)
                                        .speed(0.01)
                                        .max_decimals(4)
                                        .custom_parser(crate::widgets::number_parser),
                                )
                                .changed()
                            {
                                changed = true;
                            }
                        }
                    });
                    if changed {
                        *value = serde_json::json!(channels);
                    }
                }
                None => {
                    ui.label(
                        RichText::new(value.to_string())
                            .monospace()
                            .size(10.0)
                            .weak(),
                    );
                }
            }
        }
        serde_json::Value::Object(_) => {
            ui.label(
                RichText::new(value.to_string())
                    .monospace()
                    .size(10.0)
                    .weak(),
            );
        }
        serde_json::Value::Null => {
            ui.label(
                RichText::new(workspace.t("material.null"))
                    .weak()
                    .size(11.0),
            );
        }
    }
}
