//! Inspector panel UI.

use super::anim_graph::{animator_editor, skeleton_root_editor, CLIP_TRACKS};
use super::assets::ping_asset;
use super::material::open_material_asset;
use super::*;

pub(super) fn inspector_indices(workspace: &Workspace, state: &mut UiState) -> Vec<usize> {
    let Some(ids) = state.inspector_lock.clone() else {
        return workspace.selection.clone();
    };
    let resolved: Vec<usize> = ids
        .iter()
        .filter_map(|id| workspace.document().doc.index_of_id(*id))
        .collect();
    if resolved.is_empty() {
        state.inspector_lock = None;
        return workspace.selection.clone();
    }
    resolved
}

pub(super) fn toggle_inspector_lock(workspace: &Workspace, state: &mut UiState) {
    if state.inspector_lock.is_some() {
        state.inspector_lock = None;
        return;
    }
    let ids = workspace
        .selection
        .iter()
        .filter_map(|index| workspace.document().doc.id_of_index(*index))
        .collect();
    state.inspector_lock = Some(ids);
}

pub fn play_mode_tint(paused: bool) -> egui::Color32 {
    if paused {
        egui::Color32::from_rgb(230, 180, 80)
    } else {
        egui::Color32::from_rgb(96, 200, 120)
    }
}

pub fn play_mode_banner(playing: bool) -> Option<&'static str> {
    playing.then_some("Play mode — changes made now are discarded when Play stops.")
}

pub fn inspector(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    let interaction_id = ui.make_persistent_id("inspector-drag-interaction");
    let was_dragging = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(interaction_id))
        .unwrap_or(false);
    let panel_rect = ui.max_rect();
    let pointer_pressed = ui.input(|i| i.pointer.primary_pressed());
    let pointer_down = ui.input(|i| i.pointer.primary_down());
    let pointer_released = ui.input(|i| i.pointer.any_released());
    let in_panel_or_popup = ui.rect_contains_pointer(panel_rect)
        || (was_dragging && pointer_down)
        || egui::Popup::is_any_open(ui.ctx());

    if pointer_pressed && pointer_down && in_panel_or_popup {
        workspace.begin_interaction();
        ui.ctx().data_mut(|d| d.insert_temp(interaction_id, true));
    } else if was_dragging && pointer_down {
        workspace.begin_interaction();
    }

    if let Some(banner) = play_mode_banner(workspace.playing && workspace.has_play_snapshot()) {
        let tint = play_mode_tint(workspace.paused);
        egui::Frame::new()
            .fill(tint.gamma_multiply(0.16))
            .inner_margin(egui::Margin::symmetric(6, 4))
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(format!("{} {banner}", icons::INFO))
                        .size(11.0)
                        .color(tint),
                );
            });
        ui.add_space(4.0);
    }

    let lock_row_shown = !workspace.selection.is_empty() || state.inspector_lock.is_some();
    if lock_row_shown {
        ui.horizontal(|ui| {
            let locked = state.inspector_lock.is_some();
            let icon = if locked {
                icons::LOCK_SIMPLE
            } else {
                icons::LOCK_SIMPLE_OPEN
            };
            let hint = if locked {
                workspace.t("desktop.inspector.unlock")
            } else {
                workspace.t("desktop.inspector.lock")
            };
            let can_lock = locked || !workspace.selection.is_empty();
            if ui
                .add_enabled(can_lock, egui::Button::new(icon).small())
                .on_hover_text(hint)
                .clicked()
            {
                toggle_inspector_lock(workspace, state);
            }
            if locked {
                ui.label(
                    RichText::new(workspace.t("inspector.locked"))
                        .size(10.5)
                        .weak(),
                );
            }
        });
    }

    let indices = inspector_indices(workspace, state);
    if indices.is_empty() {
        if !state.asset_selection.is_empty() {
            asset_inspector(ui, workspace, state);
        } else {
            ui.vertical_centered(|ui| {
                ui.add_space(24.0);
                ui.label(
                    RichText::new(icons::CURSOR_CLICK)
                        .size(22.0)
                        .color(theme::color32(
                            &workspace.colors(),
                            prism_editor_core::ThemeToken::MutedDim,
                        )),
                );
                ui.label(
                    RichText::new(workspace.t("common.selectEntity"))
                        .weak()
                        .size(11.0),
                );
            });
        }
    } else if indices.len() > 1 {
        inspector_multi(ui, workspace, state, &indices);
    } else {
        inspector_single(ui, workspace, state, indices[0]);
    }

    if pointer_released || !pointer_down {
        if was_dragging || pointer_released {
            workspace.end_interaction();
        }
        ui.ctx().data_mut(|d| d.insert_temp(interaction_id, false));
    }
}

#[cfg(test)]
mod asset_inspector_tests {
    use super::*;
    use prism_editor_core::{AssetInfo, Workspace};

    fn render(workspace: &mut Workspace, state: &mut UiState) {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| inspector(ui, workspace, state));
        });
    }

    fn workspace_with_asset(path: &str) -> Workspace {
        let mut workspace = Workspace::sample();
        workspace.assets.push(AssetInfo {
            path: path.to_string(),
            size_bytes: 2048,
            kind: prism_editor_core::AssetKind::Texture,
            importer: "ImageImporter".to_string(),
            guid: None,
        });
        workspace
    }

    #[test]
    fn an_asset_selection_takes_over_the_inspector() {
        let path = "assets/sprites/fish.png";
        let mut workspace = workspace_with_asset(path);
        let mut state = UiState::default();
        state.asset_selection = vec![path.to_string()];

        render(&mut workspace, &mut state);

        assert!(workspace.selection.is_empty());
        assert!(
            workspace.undo_history().is_empty(),
            "只读的资产检查器不该产生撤销步"
        );
        assert!(!workspace.document().is_dirty());
    }

    #[test]
    fn an_entity_selection_keeps_the_entity_inspector() {
        let mut workspace = workspace_with_asset("assets/sprites/fish.png");
        workspace.selection = vec![0];
        let before = workspace.document().doc.clone();
        let mut state = UiState::default();
        state.asset_selection = vec!["assets/sprites/fish.png".to_string()];

        render(&mut workspace, &mut state);

        assert_eq!(workspace.document().doc, before);
        assert_eq!(workspace.selection, vec![0]);
    }

    #[test]
    fn import_settings_sidecar_round_trips() {
        let dir = std::env::temp_dir().join(format!("prism-import-ui-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("assets/sprites")).unwrap();
        let sidecar = dir.join(import_settings_path("assets/sprites/boat.png"));
        assert_eq!(read_import_settings(&sidecar), (0, true));

        std::fs::write(&sidecar, import_settings_json(512, false)).unwrap();
        assert_eq!(read_import_settings(&sidecar), (512, false));

        std::fs::write(&sidecar, r#"{"maxSize": 256}"#).unwrap();
        assert_eq!(read_import_settings(&sidecar), (256, true));

        std::fs::write(&sidecar, "not json").unwrap();
        assert_eq!(read_import_settings(&sidecar), (0, true));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unknown_asset_renders_a_placeholder() {
        let mut workspace = Workspace::sample();
        let mut state = UiState::default();
        state.asset_selection = vec!["assets/missing.png".to_string()];

        render(&mut workspace, &mut state);

        assert!(workspace.undo_history().is_empty());
    }
}

pub(super) fn asset_inspector(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    let Some(path) = state.asset_primary().cloned() else {
        return;
    };

    let stale = state
        .asset_details
        .as_ref()
        .map(|(cached, _)| cached != &path)
        .unwrap_or(true);
    let expired = state
        .asset_details_at
        .map(|at| at.elapsed() >= std::time::Duration::from_millis(500))
        .unwrap_or(true);
    if stale || expired {
        let details = workspace.asset_details(&path);
        state.asset_details = details.map(|details| (path.clone(), details));
        state.asset_details_at = Some(std::time::Instant::now());
    }
    let details = state
        .asset_details
        .as_ref()
        .filter(|(cached, _)| cached == &path)
        .map(|(_, details)| details.clone());

    ui.horizontal(|ui| {
        ui.label(
            RichText::new(path.rsplit('/').next().unwrap_or(&path))
                .strong()
                .size(13.0),
        );
        if state.asset_selection.len() > 1 {
            ui.label(
                RichText::new(workspace.tf(
                    "inspector.moreSelected",
                    &[("count", &(state.asset_selection.len() - 1).to_string())],
                ))
                .weak()
                .size(11.0),
            );
        }
    });
    ui.label(RichText::new(&path).weak().size(10.0));
    ui.separator();

    let Some(details) = details else {
        ui.label(RichText::new(workspace.t("inspector.notIndexed")).weak());
        return;
    };

    let mut row = |label: &str, value: String| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).weak().size(11.0));
            ui.label(RichText::new(value).size(11.0));
        });
    };
    row(
        &workspace.t("inspector.assetType"),
        details.asset.kind.label().to_string(),
    );
    row(
        &workspace.t("inspector.assetSize"),
        format_bytes(details.asset.size_bytes),
    );
    if !details.asset.importer.is_empty() {
        row(
            &workspace.t("inspector.importer"),
            details.asset.importer.clone(),
        );
    }
    row("GUID", details.guid.clone());
    row(
        &workspace.t("inspector.references"),
        details.references.len().to_string(),
    );
    ui.horizontal(|ui| {
        if ui.button(workspace.t("assets.pingScene")).clicked() {
            ping_asset(workspace, state, &path);
        }
    });

    ui.separator();
    ui.label(
        RichText::new(workspace.t("inspector.preview"))
            .strong()
            .size(11.0),
    );
    match &details.preview {
        AssetPreview::Text { text, truncated } => {
            let lines: Vec<&str> = text.lines().take(10).collect();
            ui.add(
                egui::Label::new(
                    RichText::new(lines.join(
                        "
",
                    ))
                    .monospace()
                    .size(10.0),
                )
                .wrap(),
            );
            if *truncated {
                ui.label(
                    RichText::new(workspace.t("inspector.truncated"))
                        .weak()
                        .size(10.0),
                );
            }
        }
        AssetPreview::Image {
            width,
            height,
            bytes,
        } => {
            let size = match (width, height) {
                (Some(width), Some(height)) => format!("{width} × {height}"),
                _ => workspace.t("inspector.unknownSize"),
            };
            ui.label(RichText::new(format!("{size} · {}", format_bytes(*bytes))).size(11.0));

            if let Some((texture, [thumb_width, thumb_height])) =
                thumbnail_texture(ui, workspace, state, &path, details.asset.kind)
            {
                let bounds = egui::vec2(160.0, 160.0);
                let (rect, _) = ui.allocate_exact_size(bounds, egui::Sense::hover());
                ui.painter().rect_filled(rect, 0.0, Color32::from_gray(24));
                let image_rect = fit_into(rect, thumb_width, thumb_height);
                ui.painter().image(
                    texture,
                    image_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
        }
        AssetPreview::Document => {
            ui.label(
                RichText::new(workspace.t("inspector.sceneDoc"))
                    .weak()
                    .size(11.0),
            );
        }
        AssetPreview::None => {
            ui.label(
                RichText::new(workspace.t("inspector.noPreview"))
                    .weak()
                    .size(11.0),
            );
        }
    }

    if details.asset.kind == AssetKind::Audio {
        ui.add_space(4.0);
        let playing = state.audio_preview_path.as_deref() == Some(path.as_str());
        ui.horizontal(|ui| {
            let label = if playing {
                workspace.t("common.stop")
            } else {
                workspace.t("common.play")
            };
            if ui.button(label).clicked() {
                state.pending_audio_preview = Some(if playing {
                    AudioPreviewAction::Stop
                } else {
                    AudioPreviewAction::Play(path.clone())
                });
            }
            if playing {
                ui.label(
                    RichText::new(workspace.t("inspector.playing"))
                        .weak()
                        .size(10.0),
                );
            }
        });
    }

    if !details.references.is_empty() {
        ui.separator();
        ui.label(
            RichText::new(workspace.t("inspector.usedBy"))
                .strong()
                .size(11.0),
        );
        for reference in details.references.iter().take(12) {
            ui.label(
                RichText::new(format!(
                    "{} · {} · {}",
                    reference.document, reference.entity_name, reference.field
                ))
                .weak()
                .size(10.0),
            );
        }
        if details.references.len() > 12 {
            ui.label(
                RichText::new(workspace.tf(
                    "inspector.more",
                    &[("count", &(details.references.len() - 12).to_string())],
                ))
                .weak()
                .size(10.0),
            );
        }
    }

    if path.ends_with(".bin") {
        prpv_asset_section(ui, workspace, &path);
    }

    if details.asset.kind == AssetKind::Texture {
        texture_import_settings(ui, workspace, state, &path);
    }

    ui.separator();
    ui.horizontal(|ui| {
        if ui.button(workspace.t("menu.copyPath")).clicked() {
            ui.ctx().copy_text(path.clone());
            state.status = workspace.tf("desktop.status.copied", &[("path", &path)]);
        }
        let openable = path.ends_with(".scene.json")
            || path.ends_with(".prefab.json")
            || prism_editor_core::project::is_json_editable_asset(&path);
        if openable && ui.button(workspace.t("menu.open")).clicked() {
            if path.ends_with(".scene.json") || path.ends_with(".prefab.json") {
                match workspace.open_project_file(&path) {
                    Ok(()) => {
                        state.status = workspace.tf("desktop.status.opened", &[("path", &path)])
                    }
                    Err(error) => {
                        state.status = workspace.tf(
                            "desktop.status.openFailed",
                            &[("path", &path), ("error", &error.to_string())],
                        )
                    }
                }
            } else {
                open_material_asset(workspace, state, &path);
                workspace.show_panel("material");
                state.status = workspace.tf("desktop.status.opened", &[("path", &path)]);
            }
        }
        if ui.button(workspace.t("menu.showInExplorer")).clicked() {
            state.status = match reveal_asset(workspace, &path) {
                Ok(()) => format!("revealed {path}"),
                Err(error) => error,
            };
        }
    });
}

pub(super) fn add_component_menu_for(
    ui: &mut Ui,
    workspace: &mut Workspace,
    state: &mut UiState,
    indices: &[usize],
) {
    ui.menu_button(workspace.t("inspector.addComponent"), |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.add_component_query)
                .hint_text(workspace.t("common.search")),
        );
        let query = state.add_component_query.to_lowercase();

        let attachable: std::collections::HashSet<String> = workspace
            .component_types_for(indices)
            .into_iter()
            .map(|component| component.name.clone())
            .collect();
        let categories: Vec<(String, Vec<(String, String)>)> = workspace
            .schema
            .component_menu()
            .into_iter()
            .map(|(category, items)| {
                (
                    category.to_string(),
                    items
                        .into_iter()
                        .filter(|item| attachable.contains(&item.name))
                        .filter(|item| {
                            query.is_empty() || item.name.to_lowercase().contains(&query)
                        })
                        .map(|item| (item.name.clone(), item.display_name().to_string()))
                        .collect(),
                )
            })
            .filter(|(_, items): &(String, Vec<(String, String)>)| !items.is_empty())
            .collect();
        let mut picked = None;
        if categories.is_empty() {
            ui.label(RichText::new(workspace.t("inspector.noMoreComponents")).weak());
        }
        for (category, items) in categories {
            ui.menu_button(category, |ui| {
                for (name, label) in items {
                    if ui.button(label).clicked() {
                        picked = Some(name);
                        ui.close();
                    }
                }
            });
        }
        if let Some(name) = picked {
            workspace.add_component_multi(indices, &name);
        }
    });
}

pub(super) enum TransformEdit {
    Axis {
        field: &'static str,
        axis: usize,
        value: f32,
    },

    Vector {
        field: &'static str,
        values: [f32; 3],
    },

    Rotation([f32; 4]),
}

pub(super) struct TransformLabels {
    pub position: String,
    pub rotation: String,
    pub scale: String,
    pub copy: String,
    pub paste: String,
    pub reset: String,
}

impl TransformLabels {
    pub fn new(workspace: &Workspace) -> Self {
        Self {
            position: workspace.t("inspector.position"),
            rotation: workspace.t("inspector.rotation"),
            scale: workspace.t("inspector.scale"),
            copy: workspace.t("common.copy"),
            paste: workspace.t("common.paste"),
            reset: workspace.t("common.reset"),
        }
    }
}

pub(super) fn transform_rows(
    ui: &mut Ui,
    labels: &TransformLabels,
    state: &mut UiState,
    transform: &prism_editor_core::TransformJson,

    mixed: Option<&[[bool; 4]; 3]>,
    mut write: impl FnMut(TransformEdit),
) {
    let mixed_at = |row: usize, axis: usize| mixed.map(|mixed| mixed[row][axis]).unwrap_or(false);
    let marker = |ui: &mut Ui, axis: usize, row: usize| {
        if mixed_at(row, axis) {
            ui.label(RichText::new("—").weak());
        }
    };

    let label_w = 64.0_f32;
    let axis_labels = ["X", "Y", "Z"];

    let mut position = transform.position;
    let mut position_row: Option<[f32; 3]> = None;
    ui.horizontal(|ui| {
        let mut row_delta = 0.0f32;
        if widgets::aligned_scrubbable_label(ui, &labels.position, label_w, &mut row_delta, 0.01) {
            for axis in 0..3 {
                position[axis] += row_delta;
            }
            position_row = Some(position);
        }
        for axis in 0..3 {
            if widgets::scrubbable_axis_badge(
                ui,
                axis_labels[axis],
                axis,
                &mut position[axis],
                0.01,
            ) {
                write(TransformEdit::Axis {
                    field: "position",
                    axis,
                    value: position[axis],
                });
            }
            let response = ui.add(
                egui::DragValue::new(&mut position[axis])
                    .speed(0.01)
                    .max_decimals(3)
                    .custom_parser(widgets::number_parser),
            );
            transform_row_menu(
                labels,
                &response,
                TransformRow::Position,
                transform,
                &mut write,
            );
            if response.changed() {
                write(TransformEdit::Axis {
                    field: "position",
                    axis,
                    value: position[axis],
                });
            }
            marker(ui, axis, 0);
        }
        if ui
            .small_button(RichText::new(icons::ARROW_COUNTER_CLOCKWISE).size(10.0))
            .clicked()
        {
            apply_transform_row(
                TransformRow::Position,
                &TransformRow::Position.reset(),
                &mut write,
            );
        }
    });
    if let Some(values) = position_row {
        write(TransformEdit::Vector {
            field: "position",
            values,
        });
    }

    let mut euler = prism_editor_core::viewport_math::quat_to_euler_degrees(transform.rotation);
    let mut rotation_row = false;
    ui.horizontal(|ui| {
        let mut row_delta = 0.0f32;
        if widgets::aligned_scrubbable_label(ui, &labels.rotation, label_w, &mut row_delta, 0.5) {
            for value in euler.iter_mut() {
                *value += row_delta;
            }
            rotation_row = true;
        }
        for axis in 0..3 {
            let mut changed =
                widgets::scrubbable_axis_badge(ui, axis_labels[axis], axis, &mut euler[axis], 0.5);
            let response = ui.add(
                egui::DragValue::new(&mut euler[axis])
                    .speed(0.5)
                    .max_decimals(2)
                    .suffix("°")
                    .custom_parser(widgets::number_parser),
            );
            transform_row_menu(
                labels,
                &response,
                TransformRow::Rotation,
                transform,
                &mut write,
            );
            if response.changed() {
                changed = true;
            }
            if changed {
                write(TransformEdit::Rotation(
                    prism_editor_core::viewport_math::euler_degrees_to_quat(euler),
                ));
                break;
            }
            marker(ui, axis, 1);
        }
        if ui
            .small_button(RichText::new(icons::ARROW_COUNTER_CLOCKWISE).size(10.0))
            .clicked()
        {
            apply_transform_row(
                TransformRow::Rotation,
                &TransformRow::Rotation.reset(),
                &mut write,
            );
        }
    });
    if rotation_row {
        write(TransformEdit::Rotation(
            prism_editor_core::viewport_math::euler_degrees_to_quat(euler),
        ));
    }

    let mut scale = transform.scale;
    let mut scale_row: Option<[f32; 3]> = None;
    ui.horizontal(|ui| {
        let mut row_delta = 0.0f32;
        if widgets::aligned_scrubbable_label(ui, &labels.scale, label_w, &mut row_delta, 0.01) {
            for axis in 0..3 {
                scale[axis] += row_delta;
            }
            scale_row = Some(scale);
        }
        for axis in 0..3 {
            if widgets::scrubbable_axis_badge(ui, axis_labels[axis], axis, &mut scale[axis], 0.01) {
                write(TransformEdit::Axis {
                    field: "scale",
                    axis,
                    value: scale[axis],
                });
            }
            let response = ui.add(
                egui::DragValue::new(&mut scale[axis])
                    .speed(0.01)
                    .max_decimals(3)
                    .custom_parser(widgets::number_parser),
            );
            transform_row_menu(
                labels,
                &response,
                TransformRow::Scale,
                transform,
                &mut write,
            );
            if response.changed() {
                write(TransformEdit::Axis {
                    field: "scale",
                    axis,
                    value: scale[axis],
                });
            }
            marker(ui, axis, 2);
        }
        if ui
            .small_button(RichText::new(icons::ARROW_COUNTER_CLOCKWISE).size(10.0))
            .clicked()
        {
            apply_transform_row(
                TransformRow::Scale,
                &TransformRow::Scale.reset(),
                &mut write,
            );
        }
    });
    if let Some(values) = scale_row {
        write(TransformEdit::Vector {
            field: "scale",
            values,
        });
    }
    let _ = state;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TransformRow {
    Position,
    Rotation,
    Scale,
}

impl TransformRow {
    pub(super) fn reset(self) -> Vec<f32> {
        match self {
            Self::Position => vec![0.0, 0.0, 0.0],
            Self::Scale => vec![1.0, 1.0, 1.0],
            Self::Rotation => vec![0.0, 0.0, 0.0, 1.0],
        }
    }

    fn from_transform(self, transform: &prism_editor_core::TransformJson) -> Vec<f32> {
        match self {
            Self::Position => transform.position.to_vec(),
            Self::Scale => transform.scale.to_vec(),
            Self::Rotation => transform.rotation.to_vec(),
        }
    }
}

pub(super) fn transform_row_menu(
    labels: &TransformLabels,
    response: &egui::Response,
    row: TransformRow,
    transform: &prism_editor_core::TransformJson,
    write: &mut impl FnMut(TransformEdit),
) {
    const CLIPBOARD: &str = "prism-transform-row-clipboard";
    let id = egui::Id::new(CLIPBOARD);
    response.context_menu(|ui| {
        if ui.button(labels.copy.as_str()).clicked() {
            let values = row.from_transform(transform);
            ui.ctx().copy_text(format!("{values:?}"));
            ui.data_mut(|data| data.insert_temp(id, (row, values)));
            ui.close();
        }
        let clipboard = ui.data(|data| data.get_temp::<(TransformRow, Vec<f32>)>(id));
        let compatible = clipboard
            .as_ref()
            .map(|(kind, _)| *kind == row)
            .unwrap_or(false);
        if ui
            .add_enabled(compatible, egui::Button::new(labels.paste.as_str()))
            .clicked()
        {
            if let Some((_, values)) = clipboard {
                apply_transform_row(row, &values, &mut *write);
            }
            ui.close();
        }
        if ui.button(labels.reset.as_str()).clicked() {
            apply_transform_row(row, &row.reset(), &mut *write);
            ui.close();
        }
    });
}

pub(super) fn apply_transform_row(
    row: TransformRow,
    values: &[f32],
    write: &mut impl FnMut(TransformEdit),
) {
    match row {
        TransformRow::Position if values.len() == 3 => write(TransformEdit::Vector {
            field: "position",
            values: [values[0], values[1], values[2]],
        }),
        TransformRow::Scale if values.len() == 3 => write(TransformEdit::Vector {
            field: "scale",
            values: [values[0], values[1], values[2]],
        }),
        TransformRow::Rotation if values.len() == 4 => write(TransformEdit::Rotation([
            values[0], values[1], values[2], values[3],
        ])),
        _ => {}
    }
}

pub(super) const TILEMAP_COMPONENT: &str = "prism_engine::scene::Tilemap";

pub(super) fn next_tile(current: u32, palette: u32) -> u32 {
    let palette = palette.max(1);
    if current >= palette {
        0
    } else {
        current + 1
    }
}

pub(super) fn tilemap_tiles(value: &serde_json::Value) -> Vec<u32> {
    value
        .get("tiles")
        .and_then(|tiles| tiles.as_array())
        .map(|tiles| {
            tiles
                .iter()
                .filter_map(|tile| tile.as_u64())
                .map(|tile| tile as u32)
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn tilemap_columns(value: &serde_json::Value) -> usize {
    value
        .get("columns")
        .and_then(|columns| columns.as_u64())
        .unwrap_or(0) as usize
}

pub(super) fn write_tiles(workspace: &mut Workspace, entity_id: u32, tiles: &[u32]) {
    workspace.set_field(
        entity_id,
        TILEMAP_COMPONENT,
        "tiles",
        serde_json::json!(tiles),
    );
}

pub(super) fn tilemap_editor(
    ui: &mut Ui,
    workspace: &mut Workspace,
    state: &mut UiState,
    entity_id: u32,
    value: &serde_json::Value,
) {
    let mut tiles = tilemap_tiles(value);
    let authored_columns = tilemap_columns(value);
    let columns = if authored_columns > 0 {
        authored_columns
    } else {
        tiles.len().max(1)
    };
    let rows = tiles.len().div_ceil(columns.max(1));

    ui.horizontal(|ui| {
        ui.label(
            RichText::new(workspace.t("desktop.inspector.tilemap"))
                .strong()
                .size(12.0),
        );
        ui.label(
            RichText::new(format!("{columns} x {rows}"))
                .size(10.0)
                .weak(),
        );
        if value
            .get("tileset_path")
            .and_then(|path| path.as_str())
            .is_some_and(|path| path.is_empty())
        {
            ui.label(
                RichText::new(workspace.t("inspector.noTileset"))
                    .size(10.0)
                    .color(Color32::from_rgb(226, 168, 64)),
            );
        }
    });

    ui.horizontal(|ui| {
        ui.label(RichText::new(workspace.t("inspector.columns")).size(10.5));
        let mut edited_columns = columns as u32;
        if ui
            .add(egui::DragValue::new(&mut edited_columns).range(1..=256))
            .changed()
        {
            workspace.set_field(
                entity_id,
                TILEMAP_COMPONENT,
                "columns",
                serde_json::json!(edited_columns),
            );
        }

        ui.label(RichText::new(workspace.t("inspector.rows")).size(10.5));
        let mut edited_rows = rows as u32;
        if ui
            .add(egui::DragValue::new(&mut edited_rows).range(1..=256))
            .changed()
        {
            tiles.resize(edited_rows as usize * columns.max(1), 0);
            write_tiles(workspace, entity_id, &tiles);
        }

        ui.separator();
        ui.label(RichText::new(workspace.t("inspector.palette")).size(10.5));
        ui.add(egui::DragValue::new(&mut state.tilemap_palette).range(1..=64));
        if ui.small_button(workspace.t("common.clear")).clicked() {
            for tile in &mut tiles {
                *tile = 0;
            }
            write_tiles(workspace, entity_id, &tiles);
        }
    });

    let side = 18.0;
    egui::ScrollArea::vertical()
        .id_salt("tilemap-grid")
        .max_height(240.0)
        .show(ui, |ui| {
            for row in 0..rows {
                ui.horizontal(|ui| {
                    for column in 0..columns {
                        let index = row * columns + column;
                        let Some(current) = tiles.get(index).copied() else {
                            continue;
                        };
                        let label = if current == 0 {
                            "\u{b7}".to_string()
                        } else {
                            current.to_string()
                        };
                        let response = ui.add_sized(
                            [side, side],
                            egui::Button::new(RichText::new(label).size(10.0)),
                        );
                        if response.clicked_by(egui::PointerButton::Secondary) {
                            tiles[index] = 0;
                            write_tiles(workspace, entity_id, &tiles);
                        } else if response.clicked() {
                            tiles[index] = next_tile(current, state.tilemap_palette);
                            write_tiles(workspace, entity_id, &tiles);
                        }
                    }
                });
            }
        });
}

pub(super) fn inspector_single(
    ui: &mut Ui,
    workspace: &mut Workspace,
    state: &mut UiState,
    index: usize,
) {
    if index >= workspace.document().doc.entities.len() {
        workspace.clear_selection();
        return;
    }
    let entity_id = workspace.document().doc.id_of_index(index).unwrap_or(0);
    let name = workspace.document().doc.entities[index].display_name();
    let prefab = workspace.document().doc.entities[index]
        .prefab_link()
        .map(|link| link.to_string());

    let colors = workspace.colors();
    let entity_ref = &workspace.document().doc.entities[index];
    let entity_icon = super::hierarchy::entity_semantic_icon(entity_ref);
    let mut entity_active = entity_ref.active();
    let active_was = entity_active;

    if state.name_edit.as_ref().map(|(target, _)| *target) != Some(index) {
        state.name_edit = Some((index, name.clone()));
    }
    let mut buffer = state
        .name_edit
        .take()
        .map(|(_, buffer)| buffer)
        .unwrap_or_else(|| name.clone());
    egui::Frame::new()
        .fill(theme::color32(
            &colors,
            prism_editor_core::ThemeToken::PanelAlt,
        ))
        .stroke(egui::Stroke::new(
            1.0_f32,
            theme::color32(&colors, prism_editor_core::ThemeToken::Border),
        ))
        .corner_radius(egui::CornerRadius::same(3))
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.checkbox(&mut entity_active, "").changed() && entity_active != active_was {
                    workspace.set_active_entity(index, entity_active);
                }
                ui.label(RichText::new(entity_icon).size(13.0).color(theme::color32(
                    &colors,
                    prism_editor_core::ThemeToken::AccentAlt,
                )));
                let id_badge_w = 42.0_f32;
                let edit_w = (ui.available_width() - id_badge_w).max(60.0);
                let response =
                    ui.add_sized([edit_w, 20.0], egui::TextEdit::singleline(&mut buffer));
                let (enter, escape) = ui.input(|input| {
                    (
                        input.key_pressed(egui::Key::Enter),
                        input.key_pressed(egui::Key::Escape),
                    )
                });
                if response.has_focus() && escape {
                    buffer = name.clone();
                    response.surrender_focus();
                } else if (response.has_focus() && enter) || response.lost_focus() {
                    let trimmed = buffer.trim().to_string();
                    if !trimmed.is_empty() && trimmed != name {
                        workspace.rename_entity(index, &trimmed);
                        buffer = trimmed;
                    } else {
                        buffer = name.clone();
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("#{entity_id}"))
                            .monospace()
                            .weak()
                            .size(10.0),
                    );
                });
            });
        });
    state.name_edit = Some((index, buffer));
    ui.add_space(4.0);

    let broken = workspace.missing_references(index);
    if !broken.is_empty() {
        for (field, value) in broken {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(workspace.t("inspector.missing"))
                        .size(10.5)
                        .strong()
                        .color(Color32::from_rgb(232, 96, 96)),
                );
                ui.label(RichText::new(field).monospace().size(10.0));
                ui.label(RichText::new(value).monospace().size(10.0).weak());
            });
        }
        ui.separator();
    }

    let is_local_light = [
        "prism_engine::scene::Light",
        "prism_engine::scene::PointLight",
        "prism_engine::scene::SpotLight",
    ]
    .iter()
    .any(|name| workspace.document().doc.entities[index].has_component(name));
    if is_local_light {
        if state.dropped_light_ids.contains(&entity_id) {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(workspace.t("inspector.notRendered"))
                        .size(10.5)
                        .strong()
                        .color(Color32::from_rgb(226, 168, 64)),
                );
            });
            ui.separator();
        } else if state.lights_total > state.lights_kept {
            ui.label(
                RichText::new(format!(
                    "{} of {} local lights are drawn (the rest are dropped)",
                    state.lights_kept, state.lights_total
                ))
                .size(10.5)
                .weak(),
            );
            ui.separator();
        }
    }

    match &prefab {
        Some(link) => {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(workspace.t("inspector.prefab"))
                        .weak()
                        .size(10.0),
                );
                ui.label(RichText::new(link).monospace().size(10.0));
            });
            ui.horizontal(|ui| {
                if ui
                    .small_button(workspace.t("inspector.openPrefab"))
                    .clicked()
                {
                    let scene_index = workspace.active_index();
                    match workspace.open_prefab_from_instance(scene_index, entity_id) {
                        Ok(()) => {
                            state.status = workspace.tf("desktop.status.opened", &[("path", link)])
                        }
                        Err(error) => {
                            state.status = workspace.tf(
                                "desktop.status.openPrefabFailed",
                                &[("error", &error.to_string())],
                            )
                        }
                    }
                }
                if ui
                    .small_button(workspace.t("inspector.revertPrefab"))
                    .clicked()
                {
                    match workspace.revert_prefab_instance(entity_id) {
                        Ok(_) => state.status = workspace.t("desktop.status.revertedPrefab"),
                        Err(error) => {
                            state.status = workspace.tf(
                                "desktop.status.revertFailed",
                                &[("error", &error.to_string())],
                            )
                        }
                    }
                }
                if ui.small_button(workspace.t("inspector.syncAll")).clicked() {
                    match workspace.sync_prefab_instances(link) {
                        Ok(count) => {
                            state.status = workspace.tf(
                                "desktop.status.syncedInstances",
                                &[("count", &count.to_string())],
                            )
                        }
                        Err(error) => {
                            state.status = workspace.tf(
                                "desktop.status.syncFailed",
                                &[("error", &error.to_string())],
                            )
                        }
                    }
                }
                if ui
                    .small_button(workspace.t("inspector.applyToPrefab"))
                    .clicked()
                {
                    match apply_to_prefab(workspace, index, link) {
                        Ok(count) => {
                            state.status = workspace.tf(
                                "desktop.status.appliedToPrefab",
                                &[("count", &count.to_string()), ("path", link)],
                            );
                            let _ = workspace.sync_prefab_instances(link);
                        }
                        Err(error) => {
                            state.status = workspace.tf(
                                "desktop.status.applyFailed",
                                &[("error", &error.to_string())],
                            )
                        }
                    }
                }
            });
        }
        None => {
            if ui
                .small_button(workspace.t("inspector.saveAsPrefab"))
                .clicked()
            {
                state.prompt = Some(Prompt::new(
                    PromptKind::SavePrefab,
                    &workspace.t("desktop.inspector.savePrefabTitle"),
                    &workspace.t("desktop.inspector.savePrefabHint"),
                    name.clone(),
                ));
            }
        }
    }
    ui.separator();

    let transform = workspace.document().doc.entities[index].transform.clone();
    let tf_open_id = ui.make_persistent_id("inspector-transform-open");
    let mut tf_open = ui.data(|d| d.get_temp::<bool>(tf_open_id)).unwrap_or(true);
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::symmetric(8, 5))
        .corner_radius(egui::CornerRadius::same(3))
        .show(ui, |ui| {
            egui::Frame::new()
                .fill(ui.visuals().faint_bg_color)
                .inner_margin(egui::Margin::symmetric(6, 3))
                .corner_radius(egui::CornerRadius::same(3))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .small_button(if tf_open {
                                icons::CARET_DOWN
                            } else {
                                icons::CARET_RIGHT
                            })
                            .clicked()
                        {
                            tf_open = !tf_open;
                            ui.data_mut(|d| d.insert_temp(tf_open_id, tf_open));
                        }
                        ui.label(RichText::new(icons::ARROWS_OUT_CARDINAL).size(11.5));
                        ui.label(
                            RichText::new(workspace.t("inspector.transform"))
                                .strong()
                                .size(12.0),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button(icons::ARROW_COUNTER_CLOCKWISE).clicked() {
                                workspace.set_transform_vector(
                                    entity_id,
                                    "position",
                                    [0.0, 0.0, 0.0],
                                );
                                workspace.set_transform_rotation(entity_id, [0.0, 0.0, 0.0, 1.0]);
                                workspace.set_transform_vector(entity_id, "scale", [1.0, 1.0, 1.0]);
                            }
                        });
                    });
                });
            if tf_open {
                ui.add_space(4.0);
                let labels = TransformLabels::new(workspace);
                transform_rows(ui, &labels, state, &transform, None, |edit| match edit {
                    TransformEdit::Axis { field, axis, value } => {
                        workspace.set_transform_axis(entity_id, field, axis, value);
                    }
                    TransformEdit::Vector { field, values } => {
                        workspace.set_transform_vector(entity_id, field, values);
                    }
                    TransformEdit::Rotation(rotation) => {
                        workspace.set_transform_rotation(entity_id, rotation);
                    }
                });
            }
        });
    ui.add_space(4.0);

    if let Some(style) = workspace.document().doc.entities[index]
        .component(STYLE_COMPONENT)
        .cloned()
    {
        if let Some(layout) = style.get("layout") {
            layout_editor(ui, workspace, entity_id, layout);
            ui.separator();
        }
    }

    if let Some(tilemap) = workspace.document().doc.entities[index]
        .component(TILEMAP_COMPONENT)
        .cloned()
    {
        tilemap_editor(ui, workspace, state, entity_id, &tilemap);
        ui.separator();
    }

    ui.horizontal(|ui| {
        ui.label(
            RichText::new(workspace.t("inspector.components"))
                .strong()
                .size(12.0),
        );
        ui.add(
            egui::TextEdit::singleline(&mut state.inspector_search)
                .hint_text(workspace.t("common.filter"))
                .desired_width(80.0),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            add_component_menu_for(ui, workspace, state, &[index]);
        });
    });

    let components: Vec<(String, Value)> = workspace.document().doc.entities[index]
        .components
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    let filter = state.inspector_search.to_lowercase();
    let asset_paths = workspace.asset_paths();
    let doc_dir = workspace.document_dir();
    let registry = workspace.schema.clone();
    let mut actions: Vec<widgets::CardAction> = Vec::new();
    for (name, value) in components {
        if name == "prism_engine::scene::Active" {
            continue;
        }
        if !filter.is_empty() && !name.to_lowercase().contains(&filter) {
            continue;
        }
        let Some(schema) = workspace.schema.component(&name).cloned() else {
            component_debug_editor(ui, workspace, entity_id, &name, &value);
            continue;
        };
        let presets = workspace.component_preset_names(&schema.name);
        let i18n = workspace.i18n.clone();
        let mut set = |field: &str, value: Value| {
            workspace.set_field(entity_id, &schema.name, field, value);
        };
        widgets::component_card_with_assets(
            ui,
            &registry,
            &schema,
            &value,
            &mut actions,
            &mut set,
            &asset_paths,
            &doc_dir,
            &presets,
            &i18n,
        );
        if name == "prism_engine::scene::LineRenderer" {
            line_renderer_editor(ui, workspace, entity_id, &schema.name, &value);
        } else if name == "prism_engine::scene::SpriteRenderer" {
            sprite_preview(ui, workspace, state, &value);
        } else if name == "prism_engine::scene::SpriteAnimation" {
            sprite_animation_editor(ui, workspace, index, entity_id, &value);
        } else if name == "prism_engine::scene::Animator" {
            animator_editor(ui, workspace, state, entity_id, &schema.name, &value);
        } else if name == "prism_engine::scene::SkeletonRoot" {
            skeleton_root_editor(ui, workspace, state, &schema.name, &value);
        } else if name == "prism_engine::scene::KeyframeAnimation" {
            keyframe_animation_editor(ui, workspace, entity_id, &schema.name, &value);
        } else if name == "prism_engine::scene::Terrain" {
            terrain_editor(ui, workspace, &value);
        } else if name == "prism_engine::scene::GiConfig" {
            gi_config_editor(ui, workspace, state, &value);
        }
    }
    for action in actions {
        match action {
            widgets::CardAction::Copy(name) => workspace.copy_component_at(index, &name),
            widgets::CardAction::Paste(name) => {
                let applied = workspace.paste_component_values(&[index], &name);
                state.status = if applied == 0 {
                    format!("nothing to paste into {name}")
                } else {
                    format!("pasted values into {name}")
                };
            }
            widgets::CardAction::Remove(name) => workspace.remove_component(&name),
            widgets::CardAction::MoveUp(name) => {
                workspace.move_component_at(index, &name, -1);
            }
            widgets::CardAction::MoveDown(name) => {
                workspace.move_component_at(index, &name, 1);
            }
            widgets::CardAction::SavePreset {
                component,
                name,
                value,
            } => {
                workspace.save_component_preset(&component, &name, value);
                state.status = workspace.tf(
                    "desktop.status.savedPreset",
                    &[("name", &name), ("component", &component)],
                );
            }
            widgets::CardAction::ApplyPreset { component, name } => {
                match workspace.component_preset_value(&component, &name) {
                    Some(value) => {
                        workspace.set_field(entity_id, &component, "", value);
                        state.status =
                            workspace.tf("desktop.status.appliedPreset", &[("name", &name)]);
                    }
                    None => {
                        state.status =
                            workspace.tf("desktop.status.presetNotFound", &[("name", &name)])
                    }
                }
            }
        }
    }
}

pub(super) fn cached_prpv_header(
    ui: &Ui,
    path: &std::path::Path,
) -> Option<Result<PrtHeaderInfo, String>> {
    use std::io::Read;
    let mtime = std::fs::metadata(path).ok().and_then(|m| m.modified().ok());
    let key = ui.make_persistent_id(("prpv-header-cache", path));
    if let Some((cached_mtime, cached_res)) = ui
        .ctx()
        .data(|d| d.get_temp::<(Option<std::time::SystemTime>, Result<PrtHeaderInfo, String>)>(key))
    {
        if cached_mtime == mtime {
            return Some(cached_res);
        }
    }
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = [0u8; 116];
    let res = match file.read_exact(&mut buf) {
        Ok(()) => PrtProbeVolume::parse_header(&buf),
        Err(e) => Err(format!("read failed: {e}")),
    };
    ui.ctx()
        .data_mut(|d| d.insert_temp(key, (mtime, res.clone())));
    Some(res)
}

pub(super) fn resolve_gi_volume_on_disk(
    workspace: &Workspace,
    declared: Option<&str>,
) -> (String, Option<std::path::PathBuf>) {
    let rel = declared
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("assets/gi/probe_volume.bin")
        .to_string();
    let p = std::path::PathBuf::from(&rel);
    if p.is_absolute() {
        return (rel, p.is_file().then_some(p));
    }
    let mut candidates = Vec::new();
    if let Some(root) = workspace.project_root.as_ref() {
        candidates.push(root.join(&p));
        candidates.push(root.join("assets").join(&p));
    }
    candidates.push(p);
    let found = candidates.into_iter().find(|c| c.is_file());
    (rel, found)
}

pub(super) fn prt_probe_summary_rows(ui: &mut Ui, info: &PrtHeaderInfo) {
    let probes = info.dims[0] as usize * info.dims[1] as usize * info.dims[2] as usize;
    ui.label(
        RichText::new(format!(
            "Grid: {}x{}x{} ({probes} probes) · scene: {}",
            info.dims[0],
            info.dims[1],
            info.dims[2],
            if info.scene_name.is_empty() {
                "<unspecified>"
            } else {
                info.scene_name.as_str()
            }
        ))
        .size(11.0),
    );
    ui.label(
        RichText::new(format!(
            "Origin: [{:.1}, {:.1}, {:.1}] · Spacing: [{:.2}, {:.2}, {:.2}]",
            info.origin[0],
            info.origin[1],
            info.origin[2],
            info.spacing[0],
            info.spacing[1],
            info.spacing[2],
        ))
        .weak()
        .size(10.5),
    );
    if info.global_hit_ratio >= 0.0 {
        ui.label(
            RichText::new(format!(
                "Global hit ratio: {:.1}%",
                info.global_hit_ratio * 100.0
            ))
            .weak()
            .size(10.5),
        );
    }
}

pub(super) fn prt_preview_controls(
    ui: &mut Ui,
    workspace: &mut Workspace,
    max_y_layers: Option<u32>,
) {
    let label = workspace.t("desktop.viewport.showProbes");
    ui.checkbox(&mut workspace.prt_gizmos.enabled, label);
    if !workspace.prt_gizmos.enabled {
        return;
    }
    ui.horizontal(|ui| {
        ui.label(workspace.t("desktop.viewport.mode"));
        egui::ComboBox::from_id_salt(("prt-display-mode", ui.id()))
            .selected_text(workspace.t(match workspace.prt_gizmos.mode {
                prism_editor_core::PrtDisplayMode::ShSpheres => "desktop.viewport.prtSpheres",
                prism_editor_core::PrtDisplayMode::MeanIrradiance => "desktop.viewport.prtMean",
                prism_editor_core::PrtDisplayMode::DominantDirection => {
                    "desktop.viewport.prtDirection"
                }
                prism_editor_core::PrtDisplayMode::GridBoundsOnly => "desktop.viewport.prtBounds",
            }))
            .width(136.0)
            .show_ui(ui, |ui| {
                for mode in PrtDisplayMode::ALL {
                    let label = workspace.t(match mode {
                        prism_editor_core::PrtDisplayMode::ShSpheres => {
                            "desktop.viewport.prtSpheres"
                        }
                        prism_editor_core::PrtDisplayMode::MeanIrradiance => {
                            "desktop.viewport.prtMean"
                        }
                        prism_editor_core::PrtDisplayMode::DominantDirection => {
                            "desktop.viewport.prtDirection"
                        }
                        prism_editor_core::PrtDisplayMode::GridBoundsOnly => {
                            "desktop.viewport.prtBounds"
                        }
                    });
                    ui.selectable_value(&mut workspace.prt_gizmos.mode, mode, label);
                }
            });
    });
    ui.horizontal(|ui| {
        ui.label(workspace.t("desktop.render.radius"));
        ui.add(
            egui::DragValue::new(&mut workspace.prt_gizmos.probe_radius)
                .speed(0.01)
                .range(0.04..=2.0),
        );
        ui.label(workspace.t("desktop.render.exposure"));
        ui.add(
            egui::DragValue::new(&mut workspace.prt_gizmos.exposure)
                .speed(0.05)
                .range(0.05..=16.0),
        );
    });
    ui.horizontal(|ui| {
        let bounds = workspace.t("desktop.viewport.prtBounds");
        ui.checkbox(&mut workspace.prt_gizmos.show_grid_bounds, bounds);
        let tethers = workspace.t("desktop.viewport.tethers");
        ui.checkbox(&mut workspace.prt_gizmos.show_trilinear_tethers, tethers);
    });
    if let Some(layers) = max_y_layers {
        ui.horizontal(|ui| {
            let mut sliced = workspace.prt_gizmos.slice_y.is_some();
            if ui
                .checkbox(&mut sliced, workspace.t("desktop.viewport.ySlice"))
                .changed()
            {
                workspace.prt_gizmos.slice_y = if sliced { Some(0) } else { None };
            }
            if let Some(ref mut layer) = workspace.prt_gizmos.slice_y {
                let top = layers.saturating_sub(1);
                ui.add(egui::DragValue::new(layer).range(0..=top).speed(0.1));
                ui.label(RichText::new(format!("/ {top}")).weak().size(10.5));
            }
        });
    }
}

pub(super) fn prpv_asset_section(ui: &mut Ui, workspace: &mut Workspace, rel_path: &str) {
    let Some(root) = workspace.project_root.as_ref() else {
        return;
    };
    let abs = root.join(rel_path);
    let Some(Ok(info)) = cached_prpv_header(ui, &abs) else {
        return;
    };
    ui.separator();
    ui.label(
        RichText::new(workspace.t("inspector.prpv"))
            .strong()
            .size(11.0),
    );
    prt_probe_summary_rows(ui, &info);
    prt_preview_controls(ui, workspace, Some(info.dims[1]));
}

pub(super) fn gi_config_editor(
    ui: &mut Ui,
    workspace: &mut Workspace,
    state: &mut UiState,
    value: &Value,
) {
    let declared = value.get("volume").and_then(Value::as_str);
    let (rel_path, disk_path) = resolve_gi_volume_on_disk(workspace, declared);
    let mut y_layers = None;

    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::symmetric(8, 5))
        .corner_radius(egui::CornerRadius::same(2))
        .show(ui, |ui| {
            ui.label(
                RichText::new(workspace.t("inspector.prtPreview"))
                    .strong()
                    .size(11.5),
            );
            match disk_path {
                Some(ref abs) => match cached_prpv_header(ui, abs) {
                    Some(Ok(info)) => {
                        y_layers = Some(info.dims[1]);
                        prt_probe_summary_rows(ui, &info);
                    }
                    Some(Err(err)) => {
                        ui.label(
                            RichText::new(workspace.tf(
                                "inspector.invalidPrpv",
                                &[("path", &rel_path), ("error", &err.to_string())],
                            ))
                            .color(Color32::from_rgb(232, 96, 96))
                            .size(10.5),
                        );
                    }
                    None => {}
                },
                None => {
                    ui.label(
                        RichText::new(
                            workspace.tf("inspector.noProbeFile", &[("path", &rel_path)]),
                        )
                        .weak()
                        .size(10.5),
                    );
                }
            }
            ui.add_space(2.0);
            prt_preview_controls(ui, workspace, y_layers);
            ui.add_space(2.0);
            ui.add_enabled_ui(!state.build_running, |ui| {
                if ui.button(workspace.t("inspector.bakeGi")).clicked() {
                    state.pending_build = Some(crate::build::BuildKind::BakeGi {
                        output: rel_path,
                        rays: state.offline.gi_rays,
                        bounces: state.offline.gi_bounces,
                    });
                }
            });
        });
}

pub(super) fn line_renderer_editor(
    ui: &mut Ui,
    workspace: &mut Workspace,
    entity_id: u32,
    component: &str,
    value: &Value,
) {
    let Some(points) = value.get("points").and_then(Value::as_array) else {
        return;
    };
    ui.label(
        RichText::new(workspace.t("inspector.positions"))
            .strong()
            .size(11.0),
    );
    let mut updated = points.clone();
    let mut action = None;
    for (index, point) in points.iter().enumerate() {
        let mut coords = [0.0f64; 3];
        for (axis, value) in coords.iter_mut().enumerate() {
            *value = point.get(axis).and_then(Value::as_f64).unwrap_or_default();
        }
        ui.horizontal(|ui| {
            ui.label(format!("{index}"));
            let mut changed = false;
            for coord in &mut coords {
                changed |= ui.add(egui::DragValue::new(coord).speed(0.05)).changed();
            }
            if changed {
                updated[index] = json!([coords[0], coords[1], coords[2]]);
            }
            if index + 1 < points.len() && ui.small_button("+").clicked() {
                action = Some(LinePointAction::Subdivide(index));
            }
            if ui.small_button("x").clicked() {
                action = Some(LinePointAction::Remove(index));
            }
        });
    }
    if ui
        .small_button(workspace.t("inspector.addPosition"))
        .clicked()
    {
        action = Some(LinePointAction::Add);
    }
    if let Some(action) = action {
        apply_line_point_action(&mut updated, action);
    }
    if updated.as_slice() != points {
        workspace.set_field(entity_id, component, "points", Value::Array(updated));
    }
}

#[derive(Clone, Copy)]
pub(super) enum LinePointAction {
    Add,
    Remove(usize),
    Subdivide(usize),
}

pub(super) fn apply_line_point_action(points: &mut Vec<Value>, action: LinePointAction) {
    match action {
        LinePointAction::Add => {
            let next = points
                .last()
                .and_then(Value::as_array)
                .map(|point| {
                    [0, 1, 2]
                        .map(|axis| point.get(axis).and_then(Value::as_f64).unwrap_or_default())
                })
                .unwrap_or([0.0, 0.0, 0.0]);
            points.push(json!(next));
        }
        LinePointAction::Remove(index) => {
            if index < points.len() {
                points.remove(index);
            }
        }
        LinePointAction::Subdivide(index) => {
            let Some(a) = points.get(index).and_then(Value::as_array) else {
                return;
            };
            let Some(b) = points.get(index + 1).and_then(Value::as_array) else {
                return;
            };
            let midpoint = (0..3)
                .map(|axis| {
                    (a.get(axis).and_then(Value::as_f64).unwrap_or_default()
                        + b.get(axis).and_then(Value::as_f64).unwrap_or_default())
                        * 0.5
                })
                .collect::<Vec<_>>();
            points.insert(index + 1, json!(midpoint));
        }
    }
}

pub fn sprite_frame_grid(
    source: [u32; 2],
    frame_size: [u32; 2],
    sheet_columns: u32,
) -> Option<(u32, u32)> {
    if frame_size[0] == 0 || frame_size[1] == 0 {
        return None;
    }
    if source[0] < frame_size[0] || source[1] < frame_size[1] {
        return None;
    }
    if source[0] % frame_size[0] != 0 || source[1] % frame_size[1] != 0 {
        return None;
    }
    let rows = source[1] / frame_size[1];
    let columns = if sheet_columns > 0 {
        sheet_columns
    } else {
        source[0] / frame_size[0]
    };
    (columns > 0 && rows > 0).then_some((columns, rows))
}

pub(super) fn sprite_preview(
    ui: &mut Ui,
    workspace: &Workspace,
    state: &mut UiState,
    renderer: &Value,
) {
    let path = renderer
        .get("sprite_path")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if path.is_empty() {
        return;
    }
    let frame_size = renderer
        .get("frame_size")
        .and_then(Value::as_array)
        .and_then(|size| {
            let width = size.first().and_then(Value::as_u64)? as u32;
            let height = size.get(1).and_then(Value::as_u64)? as u32;
            Some([width, height])
        })
        .unwrap_or([0, 0]);
    let sheet_columns = renderer
        .get("sheet_columns")
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    let sheet_row = renderer
        .get("sheet_row")
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    let frame_index = renderer
        .get("frame_index")
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;

    ui.label(
        RichText::new(workspace.t("inspector.spritePreview"))
            .strong()
            .size(11.0),
    );
    let Some(image) = thumbnail_image(ui, workspace, state, path, AssetKind::Texture) else {
        ui.label(
            RichText::new(workspace.tf("inspector.noPreviewFor", &[("path", path)]))
                .weak()
                .size(10.0),
        );
        return;
    };

    let (response, painter) = ui.allocate_painter(egui::vec2(160.0, 160.0), egui::Sense::hover());
    let bounds = response.rect.shrink(2.0);
    let image_rect = fit_into(bounds, image.size[0], image.size[1]);
    painter.image(
        image.texture,
        image_rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );

    let Some((columns, rows)) = sprite_frame_grid(image.source, frame_size, sheet_columns) else {
        let (source_width, source_height) = (image.source[0], image.source[1]);
        ui.label(
            RichText::new(if frame_size == [0, 0] {
                workspace.tf(
                    "inspector.wholeTexture",
                    &[
                        ("width", &source_width.to_string()),
                        ("height", &source_height.to_string()),
                    ],
                )
            } else {
                workspace.tf(
                    "inspector.badFrameSize",
                    &[
                        ("frame_w", &frame_size[0].to_string()),
                        ("frame_h", &frame_size[1].to_string()),
                        ("width", &source_width.to_string()),
                        ("height", &source_height.to_string()),
                    ],
                )
            })
            .size(10.0)
            .color(ui.visuals().warn_fg_color),
        );
        return;
    };

    let cell = egui::vec2(
        image_rect.width() / columns as f32,
        image_rect.height() / rows as f32,
    );
    let grid = egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(70));
    for column in 1..columns {
        let x = image_rect.left() + cell.x * column as f32;
        painter.line_segment(
            [
                egui::pos2(x, image_rect.top()),
                egui::pos2(x, image_rect.bottom()),
            ],
            grid,
        );
    }
    for row in 1..rows {
        let y = image_rect.top() + cell.y * row as f32;
        painter.line_segment(
            [
                egui::pos2(image_rect.left(), y),
                egui::pos2(image_rect.right(), y),
            ],
            grid,
        );
    }

    let column = frame_index % columns;
    let row = (sheet_row + frame_index / columns).min(rows - 1);
    let highlight = egui::Rect::from_min_size(
        image_rect.min + egui::vec2(cell.x * column as f32, cell.y * row as f32),
        cell,
    );
    painter.rect_stroke(
        highlight,
        0.0,
        egui::Stroke::new(2.0_f32, ui.visuals().selection.bg_fill),
        egui::StrokeKind::Inside,
    );
    let total = columns * rows;
    ui.label(
        RichText::new(workspace.tf(
            "inspector.frameInfo",
            &[
                ("frame", &(row * columns + column + 1).to_string()),
                ("total", &total.to_string()),
                ("columns", &columns.to_string()),
                ("rows", &rows.to_string()),
                ("width", &frame_size[0].to_string()),
                ("height", &frame_size[1].to_string()),
            ],
        ))
        .size(10.0)
        .weak(),
    );
}

pub(super) fn sprite_animation_editor(
    ui: &mut Ui,
    workspace: &mut Workspace,
    index: usize,
    entity_id: u32,
    value: &Value,
) {
    let frames = value
        .get("frames")
        .and_then(Value::as_u64)
        .unwrap_or_default() as u32;
    let playing = value
        .get("playing")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let current = workspace
        .document()
        .doc
        .entities
        .get(index)
        .and_then(|entity| entity.components.get("prism_engine::scene::SpriteRenderer"))
        .and_then(|renderer| renderer.get("frame_index"))
        .and_then(Value::as_u64)
        .unwrap_or_default() as u32;
    let max_frame = frames.saturating_sub(1);
    let current = current.min(max_frame);

    ui.label(
        RichText::new(workspace.t("inspector.spriteSheet"))
            .strong()
            .size(11.0),
    );
    let mut target_frame = None;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(frames > 0 && !playing, egui::Button::new(icons::SKIP_BACK))
            .clicked()
        {
            target_frame = Some(0);
        }
        if ui
            .add_enabled(frames > 0 && !playing, egui::Button::new(icons::CARET_LEFT))
            .clicked()
        {
            target_frame = Some(current.saturating_sub(1));
        }
        ui.label(if frames == 0 {
            workspace.t("inspector.noFrames")
        } else {
            workspace.t("inspector.frame")
        });
        let mut selected = current;
        if ui
            .add_enabled(
                frames > 0 && !playing,
                egui::DragValue::new(&mut selected).range(0..=max_frame),
            )
            .changed()
        {
            target_frame = Some(selected);
        }
        ui.label(format!("/ {frames}"));
        if ui
            .add_enabled(
                frames > 0 && !playing,
                egui::Button::new(icons::CARET_RIGHT),
            )
            .clicked()
        {
            target_frame = Some(current.saturating_add(1).min(max_frame));
        }
        if ui
            .add_enabled(
                frames > 0 && !playing,
                egui::Button::new(icons::SKIP_FORWARD),
            )
            .clicked()
        {
            target_frame = Some(max_frame);
        }
    });
    if playing {
        ui.label(
            RichText::new(workspace.t("inspector.pauseToPreview"))
                .weak()
                .size(10.0),
        );
    }
    if let Some(frame) = target_frame {
        set_sprite_frame(workspace, entity_id, frame);
    }
}

pub(super) fn set_sprite_frame(workspace: &mut Workspace, entity_id: u32, frame: u32) {
    workspace.set_field(
        entity_id,
        "prism_engine::scene::SpriteRenderer",
        "frame_index",
        json!(frame),
    );
}

pub(super) fn joint_depth(joints: &[SkeletonJointRow], index: usize) -> usize {
    let mut depth = 0usize;
    let mut current = joints.get(index).and_then(|joint| joint.parent);
    let mut guard = 0usize;
    while let Some(parent) = current {
        guard += 1;
        if guard > joints.len() {
            break;
        }
        depth += 1;
        current = joints.get(parent).and_then(|joint| joint.parent);
    }
    depth
}

pub(super) fn clip_label(path: &str) -> String {
    if path.is_empty() {
        "(none)".to_string()
    } else {
        path.rsplit('/').next().unwrap_or(path).to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainHeightmapStats {
    pub cells: usize,
    pub samples: usize,
    pub min: f32,
    pub max: f32,
}

pub fn terrain_heightmap_stats(
    bytes: &[u8],
    resolution: u32,
) -> Result<TerrainHeightmapStats, String> {
    if bytes.len() % 4 != 0 {
        return Err(format!(
            "{} bytes is not a multiple of 4 (f32 samples)",
            bytes.len()
        ));
    }
    let samples = bytes.len() / 4;
    let cells = if resolution == 0 {
        let cells = (samples as f64).sqrt() as usize;
        if cells < 2 || cells * cells != samples {
            return Err(format!(
                "expected a square heightmap, got {samples} samples"
            ));
        }
        cells
    } else {
        let cells = resolution.clamp(2, 1024) as usize;
        if samples != cells * cells {
            return Err(format!(
                "expected {} samples for {cells}×{cells} cells, got {samples}",
                cells * cells
            ));
        }
        cells
    };
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for chunk in bytes.chunks_exact(4) {
        let raw = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let value = if raw.is_finite() { raw } else { 0.0 };
        min = min.min(value);
        max = max.max(value);
    }
    Ok(TerrainHeightmapStats {
        cells,
        samples,
        min,
        max,
    })
}

pub(super) fn terrain_sculpt_controls(ui: &mut Ui, workspace: &Workspace) {
    ui.separator();
    ui.label(
        RichText::new(workspace.t("inspector.sculptBrush"))
            .strong()
            .size(11.0),
    );
    let mut brush = terrain_brush_settings(ui);
    ui.horizontal(|ui| {
        ui.label(workspace.t("desktop.render.mode"));
        for (mode, label) in [
            (SculptMode::Raise, workspace.t("inspector.sculptRaise")),
            (SculptMode::Lower, workspace.t("inspector.sculptLower")),
            (SculptMode::Smooth, workspace.t("inspector.sculptSmooth")),
            (SculptMode::Flatten, workspace.t("inspector.sculptFlatten")),
        ] {
            ui.selectable_value(&mut brush.mode, mode, label);
        }
    });
    ui.add(
        egui::Slider::new(&mut brush.radius, 0.01..=1.0).text(workspace.t("desktop.render.radius")),
    );
    ui.add(
        egui::Slider::new(&mut brush.strength, 0.01..=100.0)
            .logarithmic(true)
            .text(workspace.t("inspector.strength")),
    );
    set_terrain_brush_settings(ui, brush);
}

pub(super) fn terrain_editor(ui: &mut Ui, workspace: &Workspace, value: &Value) {
    let heightmap = value
        .get("heightmap")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let resolution = value.get("resolution").and_then(Value::as_u64).unwrap_or(0) as u32;
    let height_scale = value
        .get("height_scale")
        .and_then(Value::as_f64)
        .unwrap_or(1.0) as f32;
    let height_scale = if height_scale.is_finite() {
        height_scale
    } else {
        1.0
    };

    ui.separator();
    ui.label(
        RichText::new(workspace.t("desktop.tools.heightmap"))
            .strong()
            .size(11.0),
    );
    if heightmap.is_empty() {
        ui.label(
            RichText::new(workspace.t("inspector.noHeightmap"))
                .weak()
                .size(10.0),
        );
        return;
    }
    ui.label(RichText::new(heightmap).monospace().size(10.0));

    let Some(root) = workspace.project_root.as_ref() else {
        ui.label(
            RichText::new(workspace.t("inspector.noHeightmapRoot"))
                .weak()
                .size(10.0),
        );
        return;
    };

    let relative = heightmap.strip_prefix("assets/").unwrap_or(heightmap);
    let path = root.join("assets").join(relative);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            ui.colored_label(
                Color32::from_rgb(220, 90, 90),
                format!("cannot read {}: {error}", path.display()),
            );
            return;
        }
    };

    match terrain_heightmap_stats(&bytes, resolution) {
        Ok(stats) => {
            ui.label(format!(
                "{} × {} cells ({} samples)",
                stats.cells, stats.cells, stats.samples
            ));
            ui.label(format!(
                "height {:.2} .. {:.2} raw → {:.2} .. {:.2} m",
                stats.min,
                stats.max,
                stats.min * height_scale,
                stats.max * height_scale
            ));
            if let Some(size) = value.get("size").and_then(Value::as_array) {
                if size.len() >= 2 {
                    let x = size[0].as_f64().unwrap_or(0.0);
                    let z = size[1].as_f64().unwrap_or(0.0);
                    ui.label(workspace.tf(
                        "inspector.worldSize",
                        &[("x", &format!("{x:.1}")), ("z", &format!("{z:.1}"))],
                    ));
                }
            }
            terrain_sculpt_controls(ui, workspace);
        }
        Err(error) => {
            ui.colored_label(
                Color32::from_rgb(220, 90, 90),
                format!("heightmap: {error}"),
            );
        }
    }
}

pub(super) fn keyframe_animation_editor(
    ui: &mut Ui,
    workspace: &mut Workspace,
    entity_id: u32,
    component: &str,
    value: &Value,
) {
    let mut longest = 0.0f32;
    for (field, _, _) in CLIP_TRACKS {
        for key in value
            .get(field)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            longest = longest.max(time_of(key) as f32);
        }
    }
    let duration = value
        .get("duration")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    let length_label = if duration > 0.0 {
        format!("{duration:.2}s")
    } else {
        format!("{longest:.2}s (auto)")
    };
    ui.label(
        RichText::new(workspace.tf("inspector.keyframesLength", &[("duration", &length_label)]))
            .strong()
            .size(11.0),
    );

    for (field, label, identity) in CLIP_TRACKS {
        let keys = value
            .get(field)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if let Some(updated) = keyframe_track_editor(ui, workspace, label, field, keys, identity) {
            workspace.set_field(entity_id, component, field, Value::Array(updated));
        }
    }
}

pub(super) fn keyframe_track_editor(
    ui: &mut Ui,
    workspace: &Workspace,
    label: &str,
    field: &str,
    keys: &[Value],
    identity: [f32; 3],
) -> Option<Vec<Value>> {
    let mut updated = keys.to_vec();
    let mut remove = None;
    let mut dirty = false;
    egui::CollapsingHeader::new(format!("{label} ({})", keys.len()))
        .id_salt(("clip-track", field))
        .default_open(true)
        .show(ui, |ui| {
            for (index, key) in keys.iter().enumerate() {
                let mut time = time_of(key) as f32;
                let mut coords = identity;
                for (axis, coord) in coords.iter_mut().enumerate() {
                    *coord = key
                        .get("value")
                        .and_then(|value| value.get(axis))
                        .and_then(Value::as_f64)
                        .unwrap_or(identity[axis] as f64) as f32;
                }
                let (changed, remove_clicked) = ui
                    .horizontal(|ui| {
                        let mut changed = ui
                            .add(
                                egui::DragValue::new(&mut time)
                                    .speed(0.01)
                                    .range(0.0..=3600.0)
                                    .suffix("s"),
                            )
                            .changed();
                        for (axis, coord) in coords.iter_mut().enumerate() {
                            changed |= ui
                                .add(
                                    egui::DragValue::new(coord)
                                        .speed(0.01)
                                        .prefix(["X ", "Y ", "Z "][axis]),
                                )
                                .changed();
                        }
                        let remove_clicked = ui
                            .small_button("x")
                            .on_hover_text(workspace.t("inspector.removeKey"))
                            .clicked();
                        (changed, remove_clicked)
                    })
                    .inner;
                if remove_clicked {
                    remove = Some(index);
                }

                if changed {
                    updated[index] = json!({ "time": time, "value": coords });
                    dirty = true;
                }
            }
            ui.horizontal(|ui| {
                if ui
                    .small_button("+ Key")
                    .on_hover_text(workspace.t("inspector.appendKeyHint"))
                    .clicked()
                {
                    append_key(&mut updated, identity);
                    dirty = true;
                }
            });
        });
    if let Some(index) = remove {
        if index < updated.len() {
            updated.remove(index);
            dirty = true;
        }
    }
    if !dirty {
        return None;
    }

    sort_key_values(&mut updated);
    Some(updated)
}

pub(super) fn append_key(keys: &mut Vec<Value>, identity: [f32; 3]) {
    let last = keys.last();
    let time = last.map(|key| time_of(key) + 1.0).unwrap_or_default();
    let value = last
        .and_then(|key| key.get("value"))
        .cloned()
        .unwrap_or_else(|| json!(identity));
    keys.push(json!({ "time": time, "value": value }));
}

pub(super) fn sort_key_values(keys: &mut [Value]) {
    keys.sort_by(|a, b| time_of(a).total_cmp(&time_of(b)));
}

pub(super) fn time_of(key: &Value) -> f64 {
    key.get("time").and_then(Value::as_f64).unwrap_or_default()
}

#[cfg(test)]
mod keyframe_editor_tests {
    use super::super::anim_graph::{
        blend_axis_offsets, blend_axis_range, blend_axis_value_at, blend_pad_bounds,
        blend_pad_value_at, clip_runtime_path, key_tick_hit, key_tick_offsets, notify_markers,
    };
    use super::*;

    #[test]
    fn notify_markers_keep_their_names_and_drop_out_of_range_ones() {
        let notifies = vec![
            (0.0, "Footstep.L".to_string()),
            (0.5, "Footstep.R".to_string()),
            (9.0, "too late".to_string()),
            (f32::NAN, "broken".to_string()),
        ];
        let markers = notify_markers(&notifies, 2.0, 200.0);
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[0], (0.0, "Footstep.L".to_string()));
        assert_eq!(markers[1], (50.0, "Footstep.R".to_string()));

        assert!(notify_markers(&notifies, 0.0, 200.0).is_empty());
        assert!(notify_markers(&notifies, 2.0, 0.0).is_empty());
        assert!(notify_markers(&[], 2.0, 200.0).is_empty());
    }

    #[test]
    fn blend_pad_bounds_cover_finite_samples_and_expand_degenerate_axes() {
        let (min, max) = blend_pad_bounds(&[[0.0, 0.0], [10.0, 4.0]]).expect("bounds");
        assert_eq!(min, [0.0, 0.0]);
        assert_eq!(max, [10.0, 4.0]);

        let (min, max) = blend_pad_bounds(&[[0.0, 2.0], [10.0, 2.0]]).expect("bounds");
        assert_eq!(min[1], 1.0);
        assert_eq!(max[1], 3.0);

        let (min, max) = blend_pad_bounds(&[[f32::NAN, 0.0], [1.0, 2.0]]).expect("bounds");

        assert_eq!(min, [0.0, 1.0]);
        assert_eq!(max, [2.0, 3.0]);
        assert!(blend_pad_bounds(&[]).is_none());
        assert!(blend_pad_bounds(&[[f32::INFINITY, 0.0]]).is_none());
    }

    #[test]
    fn blend_pad_clicks_map_back_to_the_parameter_plane() {
        let min = [0.0, 0.0];
        let max = [10.0, 20.0];
        assert_eq!(
            blend_pad_value_at(0.0, 0.0, 100.0, 100.0, min, max),
            Some([0.0, 20.0])
        );
        assert_eq!(
            blend_pad_value_at(100.0, 100.0, 100.0, 100.0, min, max),
            Some([10.0, 0.0])
        );
        assert_eq!(
            blend_pad_value_at(50.0, 50.0, 100.0, 100.0, min, max),
            Some([5.0, 10.0])
        );

        assert_eq!(
            blend_pad_value_at(-40.0, 400.0, 100.0, 100.0, min, max),
            Some([0.0, 0.0])
        );
        assert_eq!(blend_pad_value_at(0.0, 0.0, 0.0, 100.0, min, max), None);
        assert_eq!(
            blend_pad_value_at(f32::NAN, 0.0, 100.0, 100.0, min, max),
            None
        );
    }

    #[test]
    fn blend_axis_offsets_span_the_sample_range() {
        let offsets = blend_axis_offsets(&[0.0, 0.5, 1.0], 200.0);
        assert_eq!(offsets, vec![0.0, 100.0, 200.0]);

        let offsets = blend_axis_offsets(&[2.0, -2.0], 100.0);
        assert_eq!(offsets, vec![100.0, 0.0]);
    }

    #[test]
    fn blend_axis_offsets_handle_degenerate_and_broken_input() {
        assert_eq!(blend_axis_offsets(&[1.0, 1.0], 100.0), vec![50.0, 50.0]);

        assert_eq!(
            blend_axis_offsets(&[f32::NAN, 0.0, 1.0], 100.0),
            vec![0.0, 100.0]
        );
        assert!(blend_axis_offsets(&[0.0, 1.0], 0.0).is_empty());
        assert!(blend_axis_offsets(&[], 100.0).is_empty());
        assert!(blend_axis_range(&[f32::NAN]).is_none());
        assert_eq!(blend_axis_range(&[3.0, -1.0, 2.0]), Some((-1.0, 3.0)));
    }

    #[test]
    fn clicking_the_blend_axis_maps_back_into_the_parameter_range() {
        assert_eq!(blend_axis_value_at(0.0, 200.0, 0.0, 2.0), Some(0.0));
        assert_eq!(blend_axis_value_at(100.0, 200.0, 0.0, 2.0), Some(1.0));
        assert_eq!(blend_axis_value_at(200.0, 200.0, 0.0, 2.0), Some(2.0));

        assert_eq!(blend_axis_value_at(-50.0, 200.0, 0.0, 2.0), Some(0.0));
        assert_eq!(blend_axis_value_at(999.0, 200.0, 0.0, 2.0), Some(2.0));

        assert!(blend_axis_value_at(f32::NAN, 200.0, 0.0, 2.0).is_none());
        assert!(blend_axis_value_at(10.0, 0.0, 0.0, 2.0).is_none());
        assert!(blend_axis_value_at(10.0, 200.0, f32::NAN, 2.0).is_none());
    }

    #[test]
    fn key_ticks_map_time_to_the_track_width() {
        let offsets = key_tick_offsets(&[0.0, 0.5, 2.0], 2.0, 200.0);
        assert_eq!(offsets, vec![0.0, 50.0, 200.0]);
    }

    #[test]
    fn clicking_a_tick_snaps_to_that_keyframes_time() {
        let times = [0.0, 0.5, 2.0];
        assert_eq!(key_tick_hit(&times, 2.0, 200.0, 50.0, 4.0), Some(0.5));
        assert_eq!(key_tick_hit(&times, 2.0, 200.0, 0.0, 4.0), Some(0.0));
        assert_eq!(key_tick_hit(&times, 2.0, 200.0, 199.0, 4.0), Some(2.0));

        assert_eq!(key_tick_hit(&times, 2.0, 200.0, 120.0, 4.0), None);
    }

    #[test]
    fn the_nearest_tick_wins_when_two_are_close() {
        assert_eq!(
            key_tick_hit(&[1.0, 1.02], 2.0, 200.0, 101.5, 4.0),
            Some(1.02)
        );
        assert_eq!(
            key_tick_hit(&[1.0, 1.02], 2.0, 200.0, 100.4, 4.0),
            Some(1.0)
        );
    }

    #[test]
    fn the_hit_tolerance_is_inclusive_and_broken_inputs_are_ignored() {
        assert_eq!(key_tick_hit(&[0.5], 2.0, 200.0, 54.0, 4.0), Some(0.5));
        assert_eq!(key_tick_hit(&[0.5], 2.0, 200.0, 54.5, 4.0), None);

        assert_eq!(key_tick_hit(&[0.5], 0.0, 200.0, 50.0, 4.0), None);
        assert_eq!(key_tick_hit(&[0.5], 2.0, 0.0, 50.0, 4.0), None);
        assert_eq!(key_tick_hit(&[0.5], 2.0, 200.0, f32::NAN, 4.0), None);
        assert_eq!(key_tick_hit(&[0.5], 2.0, 200.0, 50.0, -1.0), None);

        assert_eq!(key_tick_hit(&[3.0, f32::NAN], 2.0, 200.0, 200.0, 4.0), None);
    }

    #[test]
    fn key_ticks_drop_out_of_range_and_broken_values() {
        let offsets = key_tick_offsets(&[-0.5, 1.0, 3.0, f32::NAN], 2.0, 100.0);
        assert_eq!(offsets, vec![50.0], "只留 0..=duration 的有限值");
        assert!(
            key_tick_offsets(&[0.5], 0.0, 100.0).is_empty(),
            "未知时长不画刻度"
        );
        assert!(
            key_tick_offsets(&[0.5], 2.0, 0.0).is_empty(),
            "零宽度不画刻度"
        );
        assert!(key_tick_offsets(&[0.5], f32::NAN, 100.0).is_empty());
    }

    #[test]
    fn the_animator_editor_renders_without_writing_when_nothing_is_touched() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::sample();
        let mut state = UiState::new();
        state
            .clip_durations
            .insert("char/walk.ranim".to_string(), 1.5);
        let value = json!({
            "clip_path": "char/walk.ranim",
            "time": 0.25,
            "speed": 1.0,
            "looped": true,
            "playing": true,
        });
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                animator_editor(
                    ui,
                    &mut workspace,
                    &state,
                    1,
                    "prism_engine::scene::Animator",
                    &value,
                );
            });
        });

        let animator = workspace.document().doc.entities[1]
            .components
            .get("prism_engine::scene::Animator");
        assert!(animator.is_none(), "只是画控件不该凭空写出一个组件");
    }

    #[test]
    fn a_scaffolded_bone_tree_indents_children() {
        let joints = vec![
            SkeletonJointRow {
                name: "hip".into(),
                parent: None,
            },
            SkeletonJointRow {
                name: "knee".into(),
                parent: Some(0),
            },
            SkeletonJointRow {
                name: "foot".into(),
                parent: Some(1),
            },
        ];
        assert_eq!(joint_depth(&joints, 0), 0);
        assert_eq!(joint_depth(&joints, 2), 2);
    }

    #[test]
    fn a_cyclic_bone_chain_does_not_hang_the_depth_calculation() {
        let joints = vec![
            SkeletonJointRow {
                name: "a".into(),
                parent: Some(1),
            },
            SkeletonJointRow {
                name: "b".into(),
                parent: Some(0),
            },
        ];
        let depth = joint_depth(&joints, 0);
        assert!(depth <= joints.len(), "depth {depth} must stay bounded");
    }

    #[test]
    fn clip_labels_show_the_file_name_only() {
        assert_eq!(clip_label(""), "(none)");
        assert_eq!(clip_label("assets/char/walk.ranim"), "walk.ranim");
        assert_eq!(clip_label("walk.ranim"), "walk.ranim");
    }

    #[test]
    fn clip_runtime_paths_strip_only_the_assets_prefix() {
        assert_eq!(
            clip_runtime_path("assets/char/walk.ranim"),
            "char/walk.ranim"
        );
        assert_eq!(clip_runtime_path("char/walk.ranim"), "char/walk.ranim");
        assert_eq!(clip_runtime_path("walk.ranim"), "walk.ranim");
    }

    #[test]
    fn appending_a_key_lands_one_second_after_the_last_one() {
        let mut keys = vec![json!({"time": 0.0, "value": [0.0, 0.0, 0.0]})];
        append_key(&mut keys, [1.0, 1.0, 1.0]);
        assert_eq!(keys[1], json!({"time": 1.0, "value": [0.0, 0.0, 0.0]}));
    }

    #[test]
    fn the_first_key_of_an_empty_track_uses_the_identity_value() {
        let mut keys: Vec<Value> = Vec::new();
        append_key(&mut keys, [1.0, 1.0, 1.0]);
        assert_eq!(keys[0], json!({"time": 0.0, "value": [1.0, 1.0, 1.0]}));
    }

    #[test]
    fn the_track_rows_render_without_writing_when_nothing_is_touched() {
        let ctx = egui::Context::default();
        let keys = vec![
            json!({"time": 0.0, "value": [0.0, 0.0, 0.0]}),
            json!({"time": 1.0, "value": [0.0, 2.0, 0.0]}),
        ];
        let mut written = None;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                written = keyframe_track_editor(
                    ui,
                    &Workspace::sample(),
                    "Position",
                    "position",
                    &keys,
                    [0.0; 3],
                );
            });
        });
        assert!(
            written.is_none(),
            "only user edits may write the track back"
        );
    }

    #[test]
    fn sorting_keyframes_keeps_equal_times_in_insertion_order() {
        let mut keys = vec![
            json!({"time": 2.0, "value": [2.0, 0.0, 0.0]}),
            json!({"time": 0.0, "value": [0.0, 0.0, 0.0]}),
            json!({"time": 2.0, "value": [7.0, 0.0, 0.0]}),
        ];
        sort_key_values(&mut keys);
        let order: Vec<f64> = keys
            .iter()
            .map(|key| key["value"][0].as_f64().unwrap())
            .collect();
        assert_eq!(order, vec![0.0, 2.0, 7.0]);
    }
}

#[cfg(test)]
mod line_renderer_tests {
    use super::*;

    #[test]
    fn subdivide_inserts_midpoint_between_adjacent_positions() {
        let mut points = vec![json!([0.0, 2.0, 4.0]), json!([2.0, 4.0, 8.0])];
        apply_line_point_action(&mut points, LinePointAction::Subdivide(0));
        assert_eq!(
            Value::Array(points),
            json!([[0.0, 2.0, 4.0], [1.0, 3.0, 6.0], [2.0, 4.0, 8.0]])
        );
    }

    #[test]
    fn remove_and_add_positions_preserve_existing_points() {
        let mut points = vec![json!([1.0, 2.0, 3.0]), json!([4.0, 5.0, 6.0])];
        apply_line_point_action(&mut points, LinePointAction::Remove(0));
        apply_line_point_action(&mut points, LinePointAction::Add);
        assert_eq!(
            Value::Array(points),
            json!([[4.0, 5.0, 6.0], [4.0, 5.0, 6.0]])
        );
    }
}

pub(super) fn mixed_transform_axes(workspace: &Workspace, indices: &[usize]) -> [[bool; 4]; 3] {
    let mut mixed = [[false; 4]; 3];
    let Some(first) = indices.first().copied() else {
        return mixed;
    };
    let base = workspace.document().doc.entities[first].transform;
    let base_basis = prism_editor_core::viewport_math::basis_from_quat(base.rotation);
    for index in indices.iter().skip(1) {
        let transform = workspace.document().doc.entities[*index].transform;
        for axis in 0..3 {
            if (base.position[axis] - transform.position[axis]).abs() > 1e-5 {
                mixed[0][axis] = true;
            }
            if (base.scale[axis] - transform.scale[axis]).abs() > 1e-5 {
                mixed[2][axis] = true;
            }
        }

        let basis = prism_editor_core::viewport_math::basis_from_quat(transform.rotation);
        let rotated = (0..3).any(|column| {
            (0..3).any(|row| (base_basis[column][row] - basis[column][row]).abs() > 1e-5)
        });
        if rotated {
            for axis in 0..3 {
                mixed[1][axis] = true;
            }
        }
    }
    mixed
}

pub(super) fn inspector_multi(
    ui: &mut Ui,
    workspace: &mut Workspace,
    state: &mut UiState,
    indices: &[usize],
) {
    let ids: Vec<u32> = indices
        .iter()
        .filter_map(|index| workspace.document().doc.id_of_index(*index))
        .collect();

    let active = workspace
        .primary_selection()
        .filter(|index| indices.contains(index))
        .unwrap_or(indices[0]);
    let active_name = workspace
        .document()
        .doc
        .entities
        .get(active)
        .and_then(|entity| entity.name.clone())
        .unwrap_or_else(|| format!("entity {active}"));
    ui.label(
        RichText::new(workspace.tf(
            "inspector.multiSelected",
            &[
                ("count", &indices.len().to_string()),
                ("name", &active_name),
            ],
        ))
        .strong()
        .size(12.0),
    );
    ui.separator();

    let first = active;
    let transform = workspace.document().doc.entities[first].transform.clone();
    let mixed = mixed_transform_axes(workspace, indices);
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::symmetric(8, 5))
        .corner_radius(egui::CornerRadius::same(3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(icons::ARROWS_OUT_CARDINAL).size(11.5));
                ui.label(
                    RichText::new(workspace.t("inspector.transform"))
                        .strong()
                        .size(12.0),
                );
            });
            ui.add_space(4.0);
            let labels = TransformLabels::new(workspace);
            transform_rows(
                ui,
                &labels,
                state,
                &transform,
                Some(&mixed),
                |edit| match edit {
                    TransformEdit::Axis { field, axis, value } => {
                        workspace.set_transform_axis_many(&ids, field, axis, value);
                    }
                    TransformEdit::Vector { field, values } => {
                        workspace.set_transform_vector_many(&ids, field, values);
                    }
                    TransformEdit::Rotation(rotation) => {
                        workspace.set_transform_rotation_many(&ids, rotation);
                    }
                },
            );
        });
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.label(
            RichText::new(workspace.t("inspector.commonComponents"))
                .strong()
                .size(12.0),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            add_component_menu_for(ui, workspace, state, indices);
        });
    });

    let common = workspace.common_components(indices);
    let values: Vec<(String, Value)> = common
        .iter()
        .filter(|component| component.name != "prism_engine::scene::Active")
        .filter_map(|component| {
            let value = workspace
                .document()
                .doc
                .entities
                .get(first)?
                .component(&component.name)?
                .clone();
            Some((component.name.clone(), value))
        })
        .collect();
    let asset_paths = workspace.asset_paths();
    let doc_dir = workspace.document_dir();
    let registry = workspace.schema.clone();
    let mut actions: Vec<widgets::CardAction> = Vec::new();
    for (name, value) in values {
        let Some(schema) = workspace.schema.component(&name).cloned() else {
            continue;
        };
        let presets = workspace.component_preset_names(&schema.name);
        let i18n = workspace.i18n.clone();
        let mut set = |field: &str, value: Value| {
            workspace.set_field_multi(&ids, &schema.name, field, value);
        };
        widgets::component_card_with_assets(
            ui,
            &registry,
            &schema,
            &value,
            &mut actions,
            &mut set,
            &asset_paths,
            &doc_dir,
            &presets,
            &i18n,
        );
    }
    for action in actions {
        match action {
            widgets::CardAction::Copy(name) => {
                if let Some(index) = workspace.primary_selection() {
                    workspace.copy_component_at(index, &name);
                }
            }
            widgets::CardAction::Paste(name) => {
                let applied = workspace.paste_component_values(indices, &name);
                state.status = if applied == 0 {
                    format!("nothing to paste into {name}")
                } else {
                    format!("pasted values into {name} ({applied} objects)")
                };
            }
            widgets::CardAction::Remove(name) => {
                workspace.remove_component_multi(indices, &name);
            }

            widgets::CardAction::MoveUp(name) => {
                for index in indices {
                    workspace.move_component_at(*index, &name, -1);
                }
            }
            widgets::CardAction::MoveDown(name) => {
                for index in indices {
                    workspace.move_component_at(*index, &name, 1);
                }
            }
            widgets::CardAction::SavePreset {
                component,
                name,
                value,
            } => {
                workspace.save_component_preset(&component, &name, value);
                state.status = workspace.tf(
                    "desktop.status.savedPreset",
                    &[("name", &name), ("component", &component)],
                );
            }
            widgets::CardAction::ApplyPreset { component, name } => {
                match workspace.component_preset_value(&component, &name) {
                    Some(value) => {
                        let applied = workspace.set_field_multi(&ids, &component, "", value);
                        state.status = workspace.tf(
                            "desktop.status.appliedPresetCount",
                            &[("name", &name), ("count", &applied.to_string())],
                        );
                    }
                    None => {
                        state.status =
                            workspace.tf("desktop.status.presetNotFound", &[("name", &name)])
                    }
                }
            }
        }
    }
}

pub(super) fn component_debug_editor(
    ui: &mut Ui,
    workspace: &mut Workspace,
    entity_id: u32,
    name: &str,
    value: &Value,
) {
    let id = egui::Id::new(("component-debug-json", entity_id, name));
    let mut buffer = ui
        .data(|data| data.get_temp::<String>(id))
        .unwrap_or_else(|| serde_json::to_string_pretty(value).unwrap_or_default());

    ui.horizontal(|ui| {
        ui.label(
            RichText::new(workspace.tf("inspector.rawComponent", &[("name", name)]))
                .weak()
                .size(11.0),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button(workspace.t("material.reload")).clicked() {
                buffer = serde_json::to_string_pretty(value).unwrap_or_default();
            }
        });
    });
    ui.add(
        egui::TextEdit::multiline(&mut buffer)
            .code_editor()
            .desired_rows(5)
            .desired_width(f32::INFINITY),
    );

    let parsed = serde_json::from_str::<Value>(&buffer);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                parsed.is_ok(),
                egui::Button::new(workspace.t("inspector.applyJson")),
            )
            .clicked()
        {
            if let Ok(parsed) = &parsed {
                workspace.set_field(entity_id, name, "", parsed.clone());
                ui.data_mut(|data| data.remove::<String>(id));
            }
        }
        match &parsed {
            Ok(_) => {
                ui.label(
                    RichText::new(workspace.t("inspector.validJson"))
                        .size(10.0)
                        .weak(),
                );
            }
            Err(error) => {
                ui.label(
                    RichText::new(error.to_string())
                        .size(10.0)
                        .color(Color32::from_rgb(232, 96, 96)),
                );
            }
        }
    });

    if parsed.is_ok() {
        ui.data_mut(|data| data.insert_temp(id, buffer));
    }
}

pub(super) fn apply_to_prefab(
    workspace: &mut Workspace,
    index: usize,
    prefab: &str,
) -> std::io::Result<usize> {
    let root = workspace
        .project_root
        .clone()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "没有绑定项目根"))?;
    let doc = workspace.document().doc.extract_subtree(&[index]);
    let count = doc.entities.len();
    prism_editor_core::project::write_document(&root, prefab, &doc)?;
    Ok(count)
}

pub fn prefab_overrides(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    let Some(root) = workspace.primary_selection() else {
        ui.label(
            RichText::new(workspace.t("inspector.selectPrefabRoot"))
                .weak()
                .size(11.0),
        );
        return;
    };
    let linked = workspace
        .document()
        .doc
        .entities
        .get(root)
        .and_then(|entity| entity.prefab_link())
        .map(|path| path.to_string());
    let Some(prefab_path) = linked else {
        ui.label(
            RichText::new(workspace.t("inspector.notPrefabInstance"))
                .weak()
                .size(11.0),
        );
        return;
    };

    let overrides = match workspace.prefab_overrides(root) {
        Ok(list) => list,
        Err(error) => {
            ui.label(
                RichText::new(workspace.tf(
                    "inspector.cannotRead",
                    &[("path", &prefab_path), ("error", &error.to_string())],
                ))
                .weak(),
            );
            return;
        }
    };

    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{prefab_path}")).size(11.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button(workspace.t("inspector.unpack")).clicked() {
                let removed = workspace.unpack_prefab(root);
                state.status = workspace.tf(
                    "desktop.status.unpacked",
                    &[("count", &removed.to_string())],
                );
            }
        });
    });
    ui.separator();

    if overrides.is_empty() {
        ui.label(
            RichText::new(workspace.t("inspector.noOverrides"))
                .weak()
                .size(11.0),
        );
        return;
    }
    ui.label(
        RichText::new(workspace.tf(
            "inspector.overrideCount",
            &[("count", &overrides.len().to_string())],
        ))
        .size(11.0)
        .weak(),
    );

    let mut revert: Option<prism_editor_core::workspace::PrefabOverride> = None;
    let mut apply: Option<prism_editor_core::workspace::PrefabOverride> = None;
    egui::ScrollArea::vertical()
        .id_salt("prefab-overrides")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for entry in &overrides {
                ui.horizontal(|ui| {
                    let field = if entry.field.is_empty() {
                        entry.component.clone()
                    } else {
                        format!("{}.{}", entry.component, entry.field)
                    };
                    let label = format!("{} · {field}", entry.entity_name);
                    ui.label(RichText::new(label).size(11.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(workspace.t("common.apply")).clicked() {
                            apply = Some(entry.clone());
                        }
                        if ui.small_button(workspace.t("inspector.revert")).clicked() {
                            revert = Some(entry.clone());
                        }
                    });
                });
            }
        });

    if let Some(entry) = revert {
        if workspace.revert_prefab_override(&entry) {
            state.status = workspace.tf(
                "desktop.status.revertedOverride",
                &[
                    ("entity", &entry.entity_name),
                    (
                        "field",
                        &if entry.field.is_empty() {
                            entry.component.clone()
                        } else {
                            format!("{}.{}", entry.component, entry.field)
                        },
                    ),
                ],
            );
        }
    }
    if let Some(entry) = apply {
        match workspace.apply_prefab_override(root, &entry) {
            Ok(()) => {
                state.status = workspace.tf(
                    "desktop.status.appliedOverride",
                    &[
                        ("entity", &entry.entity_name),
                        (
                            "field",
                            &if entry.field.is_empty() {
                                entry.component.clone()
                            } else {
                                format!("{}.{}", entry.component, entry.field)
                            },
                        ),
                    ],
                );
            }
            Err(error) => {
                state.status = workspace.tf(
                    "desktop.status.applyFailed",
                    &[("error", &error.to_string())],
                )
            }
        }
    }
}

pub(super) const STYLE_COMPONENT: &str = "prism_engine::ui::Style";

pub(super) fn layout_editor(
    ui: &mut Ui,
    workspace: &mut Workspace,
    entity_id: u32,
    layout: &Value,
) {
    let direction = layout
        .get("direction")
        .and_then(Value::as_str)
        .unwrap_or("None")
        .to_string();
    ui.label(
        RichText::new(workspace.t("inspector.layoutGroup"))
            .strong()
            .size(12.0),
    );
    egui::Grid::new(format!("layout-group-{entity_id}"))
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label(RichText::new(workspace.t("inspector.direction")).size(11.0));
            egui::ComboBox::from_id_salt(format!("layout-direction-{entity_id}"))
                .selected_text(direction.as_str())
                .show_ui(ui, |ui| {
                    for option in ["None", "Row", "Column"] {
                        if ui.selectable_label(direction == option, option).clicked() {
                            workspace.set_field(
                                entity_id,
                                STYLE_COMPONENT,
                                "layout.direction",
                                json!(option),
                            );
                        }
                    }
                });
            ui.end_row();

            if direction != "None" {
                ui.label(RichText::new(workspace.t("inspector.spacing")).size(11.0));
                let mut spacing =
                    layout.get("spacing").and_then(Value::as_f64).unwrap_or(0.0) as f32;
                if ui
                    .add(
                        egui::DragValue::new(&mut spacing)
                            .speed(0.5)
                            .range(0.0..=512.0),
                    )
                    .changed()
                {
                    workspace.set_field(
                        entity_id,
                        STYLE_COMPONENT,
                        "layout.spacing",
                        json!(spacing),
                    );
                }
                ui.end_row();

                ui.label(RichText::new(workspace.t("inspector.padding")).size(11.0));
                ui.horizontal(|ui| {
                    for (key, label) in
                        [("left", "L"), ("right", "R"), ("top", "T"), ("bottom", "B")]
                    {
                        let mut value = layout
                            .get("padding")
                            .and_then(|padding| padding.get(key))
                            .and_then(Value::as_f64)
                            .unwrap_or(0.0) as f32;
                        ui.label(RichText::new(label).size(10.0).weak());
                        if ui
                            .add(
                                egui::DragValue::new(&mut value)
                                    .speed(0.5)
                                    .range(0.0..=1024.0),
                            )
                            .changed()
                        {
                            workspace.set_field(
                                entity_id,
                                STYLE_COMPONENT,
                                &format!("layout.padding.{key}"),
                                json!(value),
                            );
                        }
                    }
                });
                ui.end_row();

                ui.label(RichText::new(workspace.t("inspector.childAlign")).size(11.0));
                let align = layout
                    .get("child_align")
                    .and_then(Value::as_str)
                    .unwrap_or("Start")
                    .to_string();
                egui::ComboBox::from_id_salt(format!("layout-align-{entity_id}"))
                    .selected_text(align.as_str())
                    .show_ui(ui, |ui| {
                        for option in ["Start", "Center", "End", "Stretch"] {
                            if ui.selectable_label(align == option, option).clicked() {
                                workspace.set_field(
                                    entity_id,
                                    STYLE_COMPONENT,
                                    "layout.child_align",
                                    json!(option),
                                );
                            }
                        }
                    });
                ui.end_row();

                ui.label(RichText::new(workspace.t("inspector.fitContent")).size(11.0));
                let mut fit = layout
                    .get("fit_content")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if ui.checkbox(&mut fit, "").changed() {
                    workspace.set_field(
                        entity_id,
                        STYLE_COMPONENT,
                        "layout.fit_content",
                        json!(fit),
                    );
                }
                ui.end_row();
            }
        });
}

pub(super) fn import_settings_path(asset: &str) -> String {
    format!("{asset}.import.json")
}

pub(super) fn read_import_settings(path: &std::path::Path) -> (u32, bool) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (0, true);
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return (0, true);
    };
    let max_size = value.get("maxSize").and_then(Value::as_u64).unwrap_or(0) as u32;
    let mips = value
        .get("generateMips")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    (max_size, mips)
}

pub(super) fn import_settings_json(max_size: u32, generate_mips: bool) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "maxSize": max_size,
        "generateMips": generate_mips,
    }))
    .unwrap_or_default()
}

pub(super) fn texture_import_settings(
    ui: &mut Ui,
    workspace: &Workspace,
    state: &mut UiState,
    asset: &str,
) {
    let Some(root) = workspace.project_root.clone() else {
        return;
    };
    let sidecar = root.join(import_settings_path(asset));
    let id = egui::Id::new(("import-settings", asset));
    let mut buffer = ui
        .data(|data| data.get_temp::<(u32, bool)>(id))
        .unwrap_or_else(|| read_import_settings(&sidecar));

    ui.separator();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(workspace.t("inspector.importSettings"))
                .strong()
                .size(11.0),
        );
    });
    ui.horizontal(|ui| {
        ui.label(workspace.t("inspector.maxSize"));
        ui.add(
            egui::DragValue::new(&mut buffer.0)
                .range(0..=16384)
                .speed(16.0),
        );
        ui.checkbox(&mut buffer.1, workspace.t("inspector.mips"));
    });
    ui.horizontal(|ui| {
        if ui.button(workspace.t("common.apply")).clicked() {
            state.status = match std::fs::write(&sidecar, import_settings_json(buffer.0, buffer.1))
            {
                Ok(()) => format!("saved {}", sidecar.display()),
                Err(error) => format!("save import settings failed: {error}"),
            };
            ui.data_mut(|data| data.insert_temp(id, buffer));
        }
        if ui.button(workspace.t("inspector.revert")).clicked() {
            buffer = read_import_settings(&sidecar);
            ui.data_mut(|data| data.insert_temp(id, buffer));
            state.status =
                workspace.tf("desktop.status.reloadedImportSettings", &[("path", &asset)]);
        }
        if ui.button(workspace.t("inspector.reimport")).clicked() {
            state.pending_build = Some(crate::build::BuildKind::ScenePack);
            state.status = workspace.t("desktop.status.rebuildingPack");
        }
    });
    if !sidecar.is_file() {}
}

#[cfg(test)]
mod terrain_stats_tests {
    use super::*;

    fn raw(values: &[f32]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }

    #[test]
    fn infers_a_square_heightmap_and_reports_the_range() {
        let stats =
            terrain_heightmap_stats(&raw(&[-1.5, 2.0, 0.0, 3.0]), 0).expect("2×2 heightmap");
        assert_eq!(stats.cells, 2);
        assert_eq!(stats.samples, 4);
        assert_eq!(stats.min, -1.5);
        assert_eq!(stats.max, 3.0);
    }

    #[test]
    fn explicit_resolution_must_match_the_sample_count() {
        let error = terrain_heightmap_stats(&raw(&[0.0; 4]), 3).expect_err("3×3 needs 9 samples");
        assert!(error.contains("expected 9 samples"), "{error}");
    }

    #[test]
    fn rejects_unaligned_non_square_and_empty_files() {
        assert!(terrain_heightmap_stats(&[0, 1, 2], 0).is_err());
        assert!(terrain_heightmap_stats(&raw(&[0.0; 5]), 0).is_err());
        assert!(terrain_heightmap_stats(&[], 0).is_err());
    }

    #[test]
    fn non_finite_heights_are_treated_as_zero() {
        let stats = terrain_heightmap_stats(&raw(&[f32::NAN, 1.0, -2.0, f32::INFINITY]), 0)
            .expect("2×2 heightmap");
        assert_eq!(stats.min, -2.0);
        assert_eq!(stats.max, 1.0);
    }
}

#[cfg(test)]
mod sprite_preview_tests {
    use super::sprite_frame_grid;

    #[test]
    fn a_single_row_sheet_derives_its_columns_from_the_texture() {
        // 960×16 sheet with 64×16 frames = 15 cells in one row.
        assert_eq!(sprite_frame_grid([960, 16], [64, 16], 0), Some((15, 1)));
    }

    #[test]
    fn a_multi_row_atlas_counts_rows_from_the_height() {
        assert_eq!(sprite_frame_grid([256, 128], [64, 32], 0), Some((4, 4)));
    }

    #[test]
    fn an_explicit_column_count_wins_over_the_derived_one() {
        assert_eq!(sprite_frame_grid([256, 128], [32, 32], 4), Some((4, 4)));
    }

    #[test]
    fn a_whole_texture_frame_has_no_grid() {
        assert_eq!(sprite_frame_grid([960, 16], [0, 0], 0), None);
    }

    #[test]
    fn a_frame_size_that_does_not_tile_the_sheet_is_rejected() {
        // 100 is not a multiple of 30, so the last column would be clipped.
        assert_eq!(sprite_frame_grid([100, 50], [30, 25], 0), None);
        assert_eq!(sprite_frame_grid([100, 50], [25, 25], 0), Some((4, 2)));
    }

    #[test]
    fn a_frame_larger_than_the_texture_is_rejected() {
        assert_eq!(sprite_frame_grid([32, 32], [64, 64], 0), None);
    }
}
