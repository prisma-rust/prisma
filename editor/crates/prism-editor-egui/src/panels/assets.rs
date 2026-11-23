//! Assets panel UI.

use super::material::open_material_asset;
use super::*;

pub(super) fn asset_folder_column(ui: &mut Ui, workspace: &Workspace, state: &mut UiState) {
    egui::ScrollArea::vertical()
        .id_salt("asset-folder-tree")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let selected = state.asset_folder == "assets";
            if ui
                .selectable_label(
                    selected,
                    format!("{} {}", icons::FOLDER, workspace.t("assets.title")),
                )
                .clicked()
            {
                navigate_assets(state, "assets");
            }
            let roots = workspace.tree.roots.clone();
            for node in &roots {
                folder_tree_row(ui, node, state, 1);
            }
        });
}

pub(super) fn folder_tree_row(
    ui: &mut Ui,
    node: &prism_editor_core::assets::AssetNode,
    state: &mut UiState,
    depth: usize,
) {
    if !node.is_dir {
        return;
    }
    let selected = state.asset_folder == node.path;
    let label = format!(
        "{}{} {}",
        "  ".repeat(depth.saturating_sub(1)),
        icons::FOLDER,
        node.name
    );
    if ui.selectable_label(selected, label).clicked() {
        navigate_assets(state, &node.path);
    }
    for child in &node.children {
        folder_tree_row(ui, child, state, depth + 1);
    }
}

pub(super) fn asset_grid(
    ui: &mut Ui,
    workspace: &mut Workspace,
    state: &mut UiState,
    folders: &[String],
    files: &[(String, AssetKind, u64)],
    visible: &[String],
    actions: &mut AssetActions,
) {
    let per_row = grid_columns(ui.available_width(), 92.0);
    egui::ScrollArea::vertical()
        .id_salt("asset-grid-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for chunk in folders.chunks(per_row) {
                ui.horizontal(|ui| {
                    for path in chunk {
                        let name = path.rsplit('/').next().unwrap_or(path);
                        let response = asset_grid_cell(
                            ui,
                            icons::FOLDER,
                            name,
                            state.asset_selected(path),
                            None,
                        );
                        if response.double_clicked() {
                            navigate_assets(state, path);
                        } else if response.clicked() {
                            state.select_asset_only(path.clone());
                        }
                        response.context_menu(|ui| {
                            if ui.button(workspace.t("menu.open")).clicked() {
                                navigate_assets(state, path);
                                ui.close();
                            }
                            if ui.button(workspace.t("common.rename")).clicked() {
                                actions.rename = Some(path.clone());
                                ui.close();
                            }
                            if ui.button(workspace.t("menu.copyPath")).clicked() {
                                ui.ctx().copy_text(path.clone());
                                state.status =
                                    workspace.tf("desktop.status.copied", &[("path", &path)]);
                                ui.close();
                            }
                            if ui.button(workspace.t("menu.showInExplorer")).clicked() {
                                state.status = match reveal_asset(workspace, path) {
                                    Ok(()) => format!("revealed {path}"),
                                    Err(error) => error,
                                };
                                ui.close();
                            }
                        });
                    }
                });
            }
            for chunk in files.chunks(per_row) {
                ui.horizontal(|ui| {
                    for (path, kind, _size) in chunk {
                        let name = path.rsplit('/').next().unwrap_or(path);
                        let selected = state.asset_selected(path);
                        let drag_id = egui::Id::new(("asset-grid-drag", path.as_str()));
                        let payload = widgets::AssetDrag { path: path.clone() };
                        let thumb = thumbnail_texture(ui, workspace, state, path, *kind);
                        let response = ui
                            .dnd_drag_source(drag_id, payload, |ui| {
                                asset_grid_cell(ui, asset_kind_icon(*kind), name, selected, thumb)
                            })
                            .inner;
                        if response.clicked() {
                            let modifiers = ui.input(|input| input.modifiers);
                            if modifiers.shift {
                                state.select_asset_range(visible, path);
                            } else if modifiers.ctrl || modifiers.command {
                                state.toggle_asset(path);
                            } else {
                                state.select_asset_only(path.clone());
                            }
                            workspace.selection.clear();
                        }
                        if response.double_clicked() {
                            actions.open = Some(path.clone());
                        }
                        let targets: Vec<String> = if selected && state.asset_selection.len() > 1 {
                            state.asset_selection.clone()
                        } else {
                            vec![path.clone()]
                        };
                        response.context_menu(|ui| {
                            if ui.button(workspace.t("menu.open")).clicked() {
                                actions.open = Some(path.clone());
                                ui.close();
                            }
                            if path.ends_with(".prefab.json")
                                && ui.button(workspace.t("menu.instantiate")).clicked()
                            {
                                actions.instantiate = Some(path.clone());
                                ui.close();
                            }
                            if ui.button(workspace.t("common.rename")).clicked() {
                                actions.rename = Some(path.clone());
                                ui.close();
                            }
                            if ui.button(workspace.t("assets.pingScene")).clicked() {
                                actions.ping = Some(path.clone());
                                ui.close();
                            }
                            if ui
                                .button(if targets.len() > 1 {
                                    workspace.tf(
                                        "assets.duplicateItems",
                                        &[("count", &targets.len().to_string())],
                                    )
                                } else {
                                    workspace.t("common.duplicate")
                                })
                                .clicked()
                            {
                                actions.duplicate = Some(targets.clone());
                                ui.close();
                            }
                            if ui.button(workspace.t("menu.copyPath")).clicked() {
                                ui.ctx().copy_text(targets.join("\n"));
                                state.status =
                                    workspace.tf("desktop.status.copied", &[("path", &path)]);
                                ui.close();
                            }
                            if ui.button(workspace.t("menu.showInExplorer")).clicked() {
                                state.status = match reveal_asset(workspace, path) {
                                    Ok(()) => format!("revealed {path}"),
                                    Err(error) => error,
                                };
                                ui.close();
                            }
                            if ui
                                .button(if targets.len() > 1 {
                                    workspace.tf(
                                        "assets.deleteItems",
                                        &[("count", &targets.len().to_string())],
                                    )
                                } else {
                                    workspace.t("common.delete")
                                })
                                .clicked()
                            {
                                actions.delete = Some(targets.clone());
                                ui.close();
                            }
                        });
                    }
                });
            }
            if files.is_empty() && folders.is_empty() {
                ui.label(
                    RichText::new(workspace.t("assets.emptyFolder"))
                        .weak()
                        .size(11.0),
                );
            }
        });
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AssetView {
    #[default]
    List,

    Grid,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AssetColumns {
    #[default]
    One,

    Two,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AssetSearch {
    pub kinds: Vec<AssetKind>,

    pub needles: Vec<String>,
}

impl AssetSearch {
    pub fn parse(text: &str) -> Self {
        let mut search = AssetSearch::default();
        for term in text.split_whitespace() {
            let lower = term.to_lowercase();
            if let Some(kind_text) = lower.strip_prefix("t:") {
                if let Some(kind) = asset_kind_by_token(kind_text) {
                    if !search.kinds.contains(&kind) {
                        search.kinds.push(kind);
                    }
                }
                continue;
            }
            let needle = lower.strip_prefix("dir:").unwrap_or(&lower);
            if !needle.is_empty() && !search.needles.iter().any(|known| known == needle) {
                search.needles.push(needle.to_string());
            }
        }
        search
    }

    pub fn is_active(&self) -> bool {
        !self.kinds.is_empty() || !self.needles.is_empty()
    }

    pub fn matches(&self, path: &str, kind: AssetKind) -> bool {
        if !self.kinds.is_empty() && !self.kinds.contains(&kind) {
            return false;
        }
        let lower = path.to_lowercase();
        self.needles.iter().all(|needle| lower.contains(needle))
    }
}

pub(super) fn asset_kind_by_token(token: &str) -> Option<AssetKind> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    AssetKind::all().into_iter().find(|kind| {
        kind.label().eq_ignore_ascii_case(token) || format!("{kind:?}").eq_ignore_ascii_case(token)
    })
}

pub(super) fn grid_columns(available_width: f32, cell_width: f32) -> usize {
    if !available_width.is_finite() || !cell_width.is_finite() || cell_width <= 0.0 {
        return 1;
    }
    ((available_width / cell_width).floor() as usize).max(1)
}

#[derive(Default)]
pub(super) struct AssetActions {
    open: Option<String>,
    delete: Option<Vec<String>>,
    duplicate: Option<Vec<String>>,
    rename: Option<String>,
    instantiate: Option<String>,
    commit_rename: Option<(String, String)>,
    move_to_folder: Option<(String, String)>,
    create_subfolder: Option<String>,

    ping: Option<String>,
}

pub(super) fn ping_asset(workspace: &mut Workspace, state: &mut UiState, path: &str) {
    let active = workspace.active_index();
    let target_guids = workspace.asset_guids(path);
    let mut found: Option<(usize, usize)> = None;
    for offset in 0..workspace.docs.len() {
        let index = (active + offset) % workspace.docs.len().max(1);
        let Some(doc) = workspace.docs.get(index) else {
            continue;
        };
        if let Some((entity, _, _)) = doc
            .doc
            .field_references_with_guids(path, &target_guids)
            .into_iter()
            .next()
        {
            found = Some((index, entity));
            break;
        }
    }
    let Some((doc, entity)) = found else {
        state.status = workspace.tf("desktop.status.noObjectReferences", &[("path", path)]);
        return;
    };
    if doc != active {
        workspace.set_active(doc);
    }

    state.clear_asset_selection();
    workspace.select(entity, false);
    workspace.frame_selection();

    reveal_in_hierarchy(workspace, state, entity);
    let name = workspace
        .document()
        .doc
        .entities
        .get(entity)
        .map(|entity| entity.display_name())
        .unwrap_or_else(|| format!("#{entity}"));
    state.status = workspace.tf("desktop.status.pinged", &[("path", path), ("name", &name)]);
}

pub(super) fn asset_grid_cell(
    ui: &mut Ui,
    icon: &str,
    name: &str,
    selected: bool,
    thumbnail: Option<(egui::TextureId, [u32; 2])>,
) -> egui::Response {
    let visuals = ui.visuals().clone();
    let cell = egui::vec2(84.0, 84.0);
    let (rect, response) = ui.allocate_exact_size(cell, egui::Sense::click());
    if selected {
        ui.painter().rect_filled(
            rect.shrink(2.0),
            egui::CornerRadius::same(4),
            visuals.selection.bg_fill,
        );
    } else if response.hovered() {
        ui.painter().rect_filled(
            rect.shrink(2.0),
            egui::CornerRadius::same(4),
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 12),
        );
    }

    if let Some((texture, [width, height])) = thumbnail {
        let bounds = egui::Rect::from_center_size(
            rect.center_top() + egui::vec2(0.0, 30.0),
            egui::vec2(52.0, 52.0),
        );
        let mut image_rect = fit_into(bounds, width, height);
        image_rect = image_rect.intersect(rect.shrink(2.0));
        ui.painter().image(
            texture,
            image_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        ui.painter().text(
            rect.center_top() + egui::vec2(0.0, 12.0),
            egui::Align2::CENTER_TOP,
            icon,
            egui::FontId::proportional(26.0),
            visuals.text_color(),
        );
    }
    let mut job = egui::text::LayoutJob::simple_singleline(
        truncate_label(name, 14),
        egui::FontId::proportional(10.5),
        visuals.text_color(),
    );
    job.wrap.max_width = rect.width() - 6.0;
    let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
    let size = galley.size();
    ui.painter().galley(
        rect.center_bottom() - egui::vec2(size.x * 0.5, size.y + 8.0),
        galley,
        visuals.text_color(),
    );
    response
}

pub(super) fn truncate_label(name: &str, max: usize) -> String {
    let count = name.chars().count();
    if count <= max {
        return name.to_string();
    }
    let head: String = name.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

pub fn assets(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    state.thumbnail_budget = 12;

    ui.horizontal(|ui| {
        let can_back = state.asset_history_pos > 0;
        let can_forward = state.asset_history_pos + 1 < state.asset_history.len();
        if ui
            .add_enabled(can_back, egui::Button::new(icons::CARET_LEFT))
            .clicked()
        {
            state.asset_history_pos -= 1;
            state.asset_folder = state.asset_history[state.asset_history_pos].clone();
        }
        if ui
            .add_enabled(can_forward, egui::Button::new(icons::CARET_RIGHT))
            .clicked()
        {
            state.asset_history_pos += 1;
            state.asset_folder = state.asset_history[state.asset_history_pos].clone();
        }
        let segments: Vec<String> = state
            .asset_folder
            .split('/')
            .map(|s| s.to_string())
            .collect();
        let mut target: Option<String> = None;
        for (index, segment) in segments.iter().enumerate() {
            if index > 0 {
                ui.label("/");
            }
            if ui.small_button(segment).clicked() {
                target = Some(segments[..=index].join("/"));
            }
        }
        if let Some(target) = target {
            navigate_assets(state, &target);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button(icons::ARROWS_CLOCKWISE).clicked() {
                workspace.refresh_project_files();
            }
        });
    });
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.asset_search)
                .hint_text(workspace.t("assets.searchPlaceholder"))
                .desired_width(170.0),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let two_columns = state.asset_columns == AssetColumns::Two;
            if ui.selectable_label(two_columns, icons::COLUMNS).clicked() {
                state.asset_columns = if two_columns {
                    AssetColumns::One
                } else {
                    AssetColumns::Two
                };
            }
            let grid = state.asset_view == AssetView::Grid;
            if ui.selectable_label(grid, icons::SQUARES_FOUR).clicked() {
                state.asset_view = if grid {
                    AssetView::List
                } else {
                    AssetView::Grid
                };
            }
            if ui.selectable_label(!grid, icons::LIST_BULLETS).clicked() {
                state.asset_view = AssetView::List;
            }
        });
    });

    ui.horizontal_wrapped(|ui| {
        for kind in AssetKind::all() {
            let enabled = workspace.asset_query.kinds.contains(&kind);
            if ui
                .selectable_label(enabled, asset_kind_icon(kind))
                .clicked()
            {
                workspace.toggle_asset_kind(kind);
            }
        }
    });

    ui.horizontal(|ui| {
        for (sort, label) in [
            (AssetSort::Name, workspace.t("assets.sortName")),
            (AssetSort::Kind, workspace.t("assets.sortKind")),
            (AssetSort::Size, workspace.t("assets.sortSize")),
        ] {
            let active = state.asset_sort == sort;
            let arrow = if !active {
                ""
            } else if state.asset_sort_desc {
                " ▼"
            } else {
                " ▲"
            };
            if ui
                .selectable_label(active, format!("{label}{arrow}"))
                .clicked()
            {
                if active {
                    state.asset_sort_desc = !state.asset_sort_desc;
                } else {
                    state.asset_sort = sort;
                    state.asset_sort_desc = false;
                }
            }
        }
    });
    ui.separator();

    let query = AssetSearch::parse(&state.asset_search);
    let folder = state.asset_folder.clone();
    let mut actions = AssetActions::default();
    let content = |ui: &mut Ui,
                   workspace: &mut Workspace,
                   state: &mut UiState,
                   actions: &mut AssetActions| {
        let all: Vec<(String, AssetKind, u64)> = workspace
            .filtered_assets()
            .into_iter()
            .map(|asset| (asset.path.clone(), asset.kind, asset.size_bytes))
            .collect();

        let mut folders: Vec<String> = Vec::new();
        let mut files: Vec<(String, AssetKind, u64)> = Vec::new();
        for (path, kind, size) in all {
            if query.is_active() {
                if query.matches(&path, kind) {
                    files.push((path, kind, size));
                }
                continue;
            }
            let Some(rest) = path.strip_prefix(&format!("{folder}/")) else {
                continue;
            };
            match rest.split_once('/') {
                Some((child, _)) => {
                    let child_path = format!("{folder}/{child}");
                    if !folders.contains(&child_path) {
                        folders.push(child_path);
                    }
                }
                None => files.push((path, kind, size)),
            }
        }
        folders.sort();
        sort_assets(&mut files, state.asset_sort, state.asset_sort_desc);

        let visible_paths: Vec<String> = files.iter().map(|(path, _, _)| path.clone()).collect();

        let typing = ui.ctx().memory(|memory| memory.focused()).is_some();
        let (select_all, escape) = ui.input(|input| {
            (
                !typing
                    && (input.modifiers.ctrl || input.modifiers.command)
                    && input.key_pressed(egui::Key::A),
                !typing && input.key_pressed(egui::Key::Escape),
            )
        });
        if select_all {
            state.asset_selection = visible_paths.clone();
            state.asset_anchor = visible_paths.first().cloned();
        }
        if escape && !state.asset_selection.is_empty() {
            state.clear_asset_selection();
        }

        if state.asset_view == AssetView::Grid {
            asset_grid(
                ui,
                workspace,
                state,
                &folders,
                &files,
                &visible_paths,
                actions,
            );
        } else {
            egui::ScrollArea::vertical()
                .id_salt("asset-scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if folder != "assets"
                        && ui
                            .selectable_label(false, format!("{} ..", icons::FOLDER))
                            .clicked()
                    {
                        {
                            let parent = folder.rsplit_once('/').map(|(p, _)| p.to_string());
                            if let Some(parent) = parent {
                                navigate_assets(state, &parent);
                            }
                        }
                    }
                    let colors = workspace.colors();
                    for path in &folders {
                        let selected = state.asset_selected(path);
                        let renaming =
                            matches!(&state.asset_rename, Some((target, _)) if target == path);
                        if renaming {
                            ui.horizontal(|ui| {
                                if let Some((_, buffer)) = state.asset_rename.as_mut() {
                                    let response = ui.add(
                                        egui::TextEdit::singleline(buffer)
                                            .desired_width(ui.available_width().max(80.0)),
                                    );
                                    if state.asset_rename_focus {
                                        response.request_focus();
                                        state.asset_rename_focus = false;
                                    }
                                    let (enter, escape) = ui.input(|input| {
                                        (
                                            input.key_pressed(egui::Key::Enter),
                                            input.key_pressed(egui::Key::Escape),
                                        )
                                    });
                                    let clicked_outside = response.lost_focus()
                                        && ui.input(|input| input.pointer.any_click());
                                    if escape {
                                        state.asset_rename = None;
                                        state.asset_rename_focus = false;
                                    } else if enter || clicked_outside {
                                        actions.commit_rename = state.asset_rename.take();
                                        state.asset_rename_focus = false;
                                    }
                                }
                            });
                            continue;
                        }
                        let label = format!(
                            "{} {}",
                            icons::FOLDER,
                            path.rsplit('/').next().unwrap_or(path)
                        );
                        let row = list_row(ui, &colors, selected, state.compact, &label, None);

                        if row.dnd_hover_payload::<widgets::AssetDrag>().is_some() {
                            ui.painter().rect_filled(
                                row.rect,
                                egui::CornerRadius::ZERO,
                                egui::Color32::from_rgba_unmultiplied(90, 170, 120, 40),
                            );
                        }
                        if let Some(payload) = row.dnd_release_payload::<widgets::AssetDrag>() {
                            actions.move_to_folder = Some((payload.path.clone(), path.clone()));
                        } else if row.double_clicked() {
                            navigate_assets(state, path);
                        } else if row.clicked() {
                            let modifiers = ui.input(|input| input.modifiers);
                            if modifiers.ctrl || modifiers.command {
                                state.toggle_asset(path);
                            } else {
                                state.select_asset_only(path.clone());
                            }
                        }
                        row.context_menu(|ui| {
                            if ui.button(workspace.t("menu.open")).clicked() {
                                navigate_assets(state, path);
                                ui.close();
                            }
                            if ui.button(workspace.t("common.rename")).clicked() {
                                actions.rename = Some(path.clone());
                                ui.close();
                            }
                            if ui.button(workspace.t("assets.newSubfolder")).clicked() {
                                actions.create_subfolder = Some(path.clone());
                                ui.close();
                            }
                            if ui.button(workspace.t("menu.copyPath")).clicked() {
                                ui.ctx().copy_text(path.clone());
                                state.status =
                                    workspace.tf("desktop.status.copied", &[("path", &path)]);
                                ui.close();
                            }
                            if ui.button(workspace.t("menu.showInExplorer")).clicked() {
                                state.status = match reveal_asset(workspace, path) {
                                    Ok(()) => format!("revealed {path}"),
                                    Err(error) => error,
                                };
                                ui.close();
                            }
                            ui.separator();
                            if ui.button(workspace.t("common.delete")).clicked() {
                                actions.delete = Some(vec![path.clone()]);
                                ui.close();
                            }
                        });
                    }
                    for (path, kind, _size) in &files {
                        let selected = state.asset_selected(path);
                        let renaming =
                            matches!(&state.asset_rename, Some((target, _)) if target == path);
                        if renaming {
                            ui.horizontal(|ui| {
                                if let Some((_, buffer)) = state.asset_rename.as_mut() {
                                    let response = ui.add(
                                        egui::TextEdit::singleline(buffer)
                                            .desired_width(ui.available_width().max(80.0)),
                                    );
                                    if state.asset_rename_focus {
                                        response.request_focus();
                                        state.asset_rename_focus = false;
                                    }
                                    let (enter, escape) = ui.input(|input| {
                                        (
                                            input.key_pressed(egui::Key::Enter),
                                            input.key_pressed(egui::Key::Escape),
                                        )
                                    });

                                    let clicked_outside = response.lost_focus()
                                        && ui.input(|input| input.pointer.any_click());
                                    if escape {
                                        state.asset_rename = None;
                                        state.asset_rename_focus = false;
                                    } else if enter || clicked_outside {
                                        actions.commit_rename = state.asset_rename.take();
                                        state.asset_rename_focus = false;
                                    }
                                }
                            });
                            continue;
                        }
                        let label = format!(
                            "{} {}",
                            asset_kind_icon(*kind),
                            path.rsplit('/').next().unwrap_or(path)
                        );

                        let targets: Vec<String> =
                            if state.asset_selected(path) && state.asset_selection.len() > 1 {
                                state.asset_selection.clone()
                            } else {
                                vec![path.clone()]
                            };
                        let drag_id = egui::Id::new(("asset-drag", path.as_str()));
                        let payload = widgets::AssetDrag { path: path.clone() };
                        let thumb = thumbnail_texture(ui, workspace, state, path, *kind);
                        let response = ui
                            .dnd_drag_source(drag_id, payload, |ui| {
                                list_row(
                                    ui,
                                    &workspace.colors(),
                                    selected,
                                    state.compact,
                                    &label,
                                    thumb,
                                )
                            })
                            .inner;
                        if response.clicked() {
                            let modifiers = ui.input(|input| input.modifiers);
                            if modifiers.shift {
                                state.select_asset_range(&visible_paths, path);
                            } else if modifiers.ctrl || modifiers.command {
                                state.toggle_asset(path);
                            } else {
                                state.select_asset_only(path.clone());
                            }

                            workspace.selection.clear();
                        }
                        if response.double_clicked() {
                            actions.open = Some(path.clone());
                        }
                        response.context_menu(|ui| {
                            if ui.button(workspace.t("menu.open")).clicked() {
                                actions.open = Some(path.clone());
                                ui.close();
                            }
                            if path.ends_with(".prefab.json")
                                && ui.button(workspace.t("menu.instantiate")).clicked()
                            {
                                actions.instantiate = Some(path.clone());
                                ui.close();
                            }
                            if ui.button(workspace.t("common.rename")).clicked() {
                                actions.rename = Some(path.clone());
                                ui.close();
                            }
                            if ui
                                .button(if targets.len() > 1 {
                                    workspace.tf(
                                        "assets.duplicateItems",
                                        &[("count", &targets.len().to_string())],
                                    )
                                } else {
                                    workspace.t("common.duplicate")
                                })
                                .clicked()
                            {
                                actions.duplicate = Some(targets.clone());
                                ui.close();
                            }
                            if ui.button(workspace.t("menu.copyPath")).clicked() {
                                let text = targets.join("\n");
                                ui.ctx().copy_text(text);
                                state.status = if targets.len() > 1 {
                                    format!("copied {} path(s)", targets.len())
                                } else {
                                    format!("copied {path}")
                                };
                                ui.close();
                            }
                            ui.separator();
                            if ui.button(workspace.t("assets.pingScene")).clicked() {
                                actions.ping = Some(path.clone());
                                ui.close();
                            }
                            if ui.button(workspace.t("menu.findRefsScene")).clicked() {
                                state.pending_find_references = Some((path.clone(), false));
                                ui.close();
                            }
                            if ui.button(workspace.t("menu.findRefsProject")).clicked() {
                                state.pending_find_references = Some((path.clone(), true));
                                ui.close();
                            }
                            ui.separator();
                            if ui.button(workspace.t("menu.showInExplorer")).clicked() {
                                state.status = match reveal_asset(workspace, path) {
                                    Ok(()) => format!("revealed {path}"),
                                    Err(error) => error,
                                };
                                ui.close();
                            }
                            if ui
                                .button(if targets.len() > 1 {
                                    workspace.tf(
                                        "assets.deleteItems",
                                        &[("count", &targets.len().to_string())],
                                    )
                                } else {
                                    workspace.t("common.delete")
                                })
                                .clicked()
                            {
                                actions.delete = Some(targets.clone());
                                ui.close();
                            }
                        });

                        if response.secondary_clicked() && !state.asset_selected(path) {
                            state.select_asset_only(path.clone());
                        }
                    }
                    if files.is_empty() && folders.is_empty() {
                        ui.label(
                            RichText::new(workspace.t("assets.emptyFolder"))
                                .weak()
                                .size(11.0),
                        );
                    }

                    let rest = ui.available_rect_before_wrap();
                    if rest.height() > 0.0 || rest.width() > 0.0 {
                        let blank = ui.interact(
                            rest,
                            egui::Id::new(("asset-blank", folder.as_str())),
                            egui::Sense::click(),
                        );
                        if blank.clicked() {
                            state.clear_asset_selection();
                        }
                    }
                });
        }
    };

    if state.asset_columns == AssetColumns::Two {
        let height = ui.available_height().max(120.0);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(150.0, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| asset_folder_column(ui, workspace, state),
            );
            ui.separator();
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| content(ui, workspace, state, &mut actions),
            );
        });
    } else {
        content(ui, workspace, state, &mut actions);
    }

    if let Some(path) = actions.ping.take() {
        ping_asset(workspace, state, &path);
    }
    if let Some((from, folder)) = actions.move_to_folder {
        match workspace.move_asset_to_folder(&from, Some(&folder)) {
            Ok(to) if to == from => {}
            Ok(to) => {
                state.select_asset_only(to.clone());
                state.status = workspace.tf("desktop.status.moved", &[("path", &to)]);
            }
            Err(error) => {
                state.status = workspace.tf(
                    "desktop.status.moveFailed",
                    &[("error", &error.to_string())],
                )
            }
        }
    }
    if let Some(path) = actions.open {
        if path.ends_with(".scene.json") || path.ends_with(".prefab.json") {
            if let Err(error) = workspace.open_project_file(&path) {
                state.status = workspace.tf(
                    "desktop.status.openFailed",
                    &[("path", &path), ("error", &error.to_string())],
                );
            }
        } else if prism_editor_core::project::is_json_editable_asset(&path) {
            open_material_asset(workspace, state, &path);
            workspace.show_panel("material");
            state.status = workspace.tf("desktop.status.opened", &[("path", &path)]);
        }
    }
    if let Some(path) = actions.instantiate {
        match workspace.instantiate_prefab_asset(&path, None) {
            Ok(count) => {
                state.status = workspace.tf(
                    "desktop.status.instantiated",
                    &[("count", &count.to_string())],
                )
            }
            Err(error) => {
                state.status = workspace.tf(
                    "desktop.status.instantiateFailed",
                    &[("error", &error.to_string())],
                )
            }
        }
    }
    if let Some(path) = actions.rename {
        state.select_asset_only(path.clone());
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        state.asset_rename = Some((path, name));
        state.asset_rename_focus = true;
    }
    if let Some((from, file_name)) = actions.commit_rename {
        let target = rename_target(&from, &file_name);
        let is_folder = workspace
            .project_root
            .as_ref()
            .map(|root| root.join(&from).is_dir())
            .unwrap_or(false);
        match target {
            Some(to) if to != from => {
                let result = if is_folder {
                    workspace.rename_folder(&from, &to).map(|(count, _)| count)
                } else {
                    workspace.rename_asset(&from, &to).map(|(count, _)| count)
                };
                match result {
                    Ok(count) => {
                        state.select_asset_only(to.clone());
                        state.status = workspace.tf(
                            "desktop.status.renamed",
                            &[("path", &to), ("count", &count.to_string())],
                        );
                    }
                    Err(error) => {
                        state.status = workspace.tf(
                            "desktop.status.renameFailed",
                            &[("error", &error.to_string())],
                        )
                    }
                }
            }
            Some(_) => {}
            None => state.status = workspace.t("desktop.status.invalidFileName"),
        }
    }
    if let Some(folder) = actions.create_subfolder {
        let title = workspace.t("desktop.prompt.newFolder");
        let hint = workspace.t("desktop.prompt.folderBody");
        state.prompt = Some(Prompt::new(
            PromptKind::NewFolder,
            &title,
            &hint,
            format!("{folder}/"),
        ));
    }
    if let Some(paths) = actions.delete {
        let mut assets = Vec::new();
        for path in paths {
            if is_document_path(&path) {
                if state.pending_doc_delete.is_none() {
                    state.pending_doc_delete = Some(path);
                }
            } else {
                assets.push(path);
            }
        }
        if !assets.is_empty() {
            request_delete_assets(workspace, state, assets);
        }
    }
    if let Some(paths) = actions.duplicate {
        let mut created = Vec::new();
        let mut failures = Vec::new();
        for path in &paths {
            match workspace.duplicate_asset(path) {
                Ok(copy) => created.push(copy),
                Err(error) => failures.push(format!("{path}: {error}")),
            }
        }
        state.status = if failures.is_empty() {
            match created.as_slice() {
                [one] => format!("duplicated → {one}"),
                _ => format!("duplicated {} asset(s)", created.len()),
            }
        } else {
            format!("duplicate failed: {}", failures.join("; "))
        };
        if let Some(last) = created.pop() {
            state.select_asset_only(last);
        }
    }
    asset_delete_confirm(ui, workspace, state);
}

pub fn request_delete_assets(workspace: &mut Workspace, state: &mut UiState, paths: Vec<String>) {
    if state.pending_asset_delete.is_some() || paths.is_empty() {
        return;
    }
    state.pending_asset_delete = Some(paths);
    let _ = workspace;
}

pub fn delete_asset_now(workspace: &mut Workspace, state: &mut UiState, path: &str) {
    let is_folder = workspace
        .project_root
        .as_ref()
        .map(|root| root.join(path).is_dir())
        .unwrap_or(false);
    let result = if is_folder {
        workspace.delete_folder(path)
    } else {
        workspace.delete_asset(path)
    };
    match result {
        Ok(()) => {
            state.forget_asset(path);
            state.status = workspace.tf("desktop.status.deleted", &[("path", &path)]);
        }
        Err(error) => {
            state.status = workspace.tf(
                "desktop.status.deleteFailed",
                &[("error", &error.to_string())],
            )
        }
    }
}

pub fn asset_delete_confirm(ui: &Ui, workspace: &mut Workspace, state: &mut UiState) {
    let Some(paths) = state.pending_asset_delete.clone() else {
        return;
    };
    let mut confirmed = false;
    let mut cancelled = false;
    egui::Window::new(workspace.t("assets.deleteAsset"))
        .collapsible(false)
        .resizable(false)
        .show(ui.ctx(), |ui| {
            match paths.as_slice() {
                [one] => {
                    ui.label(workspace.tf("assets.deletePermanent", &[("path", &one)]));
                }
                many => {
                    ui.label(
                        workspace.tf("assets.deleteMany", &[("count", &many.len().to_string())]),
                    );
                    for path in many.iter().take(10) {
                        ui.label(RichText::new(path).monospace().size(11.0));
                    }
                    if many.len() > 10 {
                        ui.label(RichText::new(format!("… and {} more", many.len() - 10)).weak());
                    }
                }
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(workspace.t("common.delete")).clicked() {
                    confirmed = true;
                }
                if ui.button(workspace.t("common.cancel")).clicked() {
                    cancelled = true;
                }
            });
        });
    if confirmed {
        apply_pending_asset_delete(workspace, state);
    } else if cancelled {
        state.pending_asset_delete = None;
    }
}

pub fn apply_pending_asset_delete(workspace: &mut Workspace, state: &mut UiState) {
    let Some(paths) = state.pending_asset_delete.take() else {
        return;
    };
    let mut deleted = 0usize;
    let mut failures = Vec::new();
    for path in &paths {
        let before = state.status.clone();
        delete_asset_now(workspace, state, path);
        if state.status.starts_with("deleted ") {
            deleted += 1;
        } else if state.status != before {
            failures.push(state.status.clone());
        }
    }
    state.status = if failures.is_empty() {
        format!("deleted {deleted} asset(s)")
    } else {
        format!("deleted {deleted}, failed: {}", failures.join("; "))
    };
}

pub(super) fn is_document_path(path: &str) -> bool {
    path.ends_with(".scene.json") || path.ends_with(".prefab.json")
}

pub fn doc_delete_confirm(ctx: &egui::Context, workspace: &mut Workspace, state: &mut UiState) {
    let Some(path) = state.pending_doc_delete.clone() else {
        return;
    };
    let mut decision = None;
    egui::Window::new(workspace.t("assets.deleteDoc"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label(workspace.tf("assets.confirmDelete", &[("path", &path)]));
            ui.label(
                RichText::new(
                    "The file, its open tab and its Build Settings entry are removed. This cannot be undone.",
                )
                .weak(),
            );
            ui.horizontal(|ui| {
                if ui.button(workspace.t("common.delete")).clicked() {
                    decision = Some(true);
                }
                if ui.button(workspace.t("common.cancel")).clicked() {
                    decision = Some(false);
                }
            });
        });
    match decision {
        Some(true) => apply_pending_doc_delete(workspace, state),
        Some(false) => state.pending_doc_delete = None,
        None => {}
    }
}

pub fn apply_pending_doc_delete(workspace: &mut Workspace, state: &mut UiState) {
    let Some(path) = state.pending_doc_delete.take() else {
        return;
    };
    match workspace.delete_document(&path) {
        Ok(()) => {
            state.forget_asset(&path);
            state.status = workspace.tf("desktop.status.deleted", &[("path", &path)]);
        }
        Err(error) => {
            state.status = workspace.tf(
                "desktop.status.deleteFailed",
                &[("error", &error.to_string())],
            )
        }
    }
}

pub(super) fn navigate_assets(state: &mut UiState, folder: &str) {
    if state.asset_folder == folder {
        return;
    }
    state.asset_folder = folder.to_string();
    state.asset_history.truncate(state.asset_history_pos + 1);
    state.asset_history.push(folder.to_string());
    state.asset_history_pos = state.asset_history.len() - 1;
}

pub fn docs(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    let mut activate: Option<usize> = None;
    let mut close: Option<usize> = None;
    let mut save: Option<usize> = None;
    let mut delete_doc: Option<String> = None;
    let count = workspace.docs.len();
    for index in 0..count {
        let title = workspace.docs[index].title_with_dirty();
        let dirty = title.ends_with(" *");
        let active = workspace.active_index() == index;
        let colors = workspace.colors();

        let (rect, row) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), row_height(state.compact)),
            egui::Sense::click(),
        );
        if ui.is_rect_visible(rect) {
            let bg = if active {
                Some(theme::color32(
                    &colors,
                    prism_editor_core::ThemeToken::Selection,
                ))
            } else if row.hovered() {
                Some(theme::color32(
                    &colors,
                    prism_editor_core::ThemeToken::RowHover,
                ))
            } else {
                None
            };
            if let Some(bg) = bg {
                ui.painter().rect_filled(rect, 0.0, bg);
            }
        }
        let text_color = theme::color32(
            &colors,
            if active {
                prism_editor_core::ThemeToken::OnAccent
            } else {
                prism_editor_core::ThemeToken::Text
            },
        );
        let mut row_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::left_to_right(Align::Center)),
        );
        row_ui.add_space(6.0);
        row_ui.label(RichText::new(title).size(12.0).color(text_color));
        if workspace.docs[index].externally_modified {
            row_ui
                .label(
                    RichText::new("⚠")
                        .size(11.0)
                        .color(egui::Color32::from_rgb(220, 170, 60)),
                )
                .on_hover_text(workspace.t("assets.externallyModified"));
        }
        row_ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if flat_icon(ui, icons::X).clicked() {
                close = Some(index);
            }
            if dirty && flat_icon(ui, icons::FLOPPY_DISK).clicked() {
                save = Some(index);
            }
        });
        if row.clicked() {
            activate = Some(index);
        }

        if let Some(path) = workspace.docs[index].path.clone() {
            row.context_menu(|ui| {
                if ui.button(workspace.t("common.delete")).clicked() {
                    delete_doc = Some(path);
                    ui.close();
                }
            });
        }
    }
    if let Some(index) = activate {
        workspace.set_active(index);
    }
    if let Some(index) = save {
        request_save_document(workspace, state, index);
    }
    if let Some(index) = close {
        request_close_document(workspace, state, index);
    }
    if let Some(path) = delete_doc {
        state.pending_doc_delete = Some(path);
    }
}

pub fn request_save_document(workspace: &mut Workspace, state: &mut UiState, index: usize) {
    match workspace.save_document(index) {
        Ok(path) => state.status = workspace.tf("desktop.status.saved", &[("path", &path)]),
        Err(_) if workspace.save_conflict(index).is_some() => {
            state.pending_save_conflict = Some(index);
            state.status = "File modified on disk".to_string();
        }
        Err(error) => {
            state.status = workspace.tf(
                "desktop.status.saveFailed",
                &[("error", &error.to_string())],
            )
        }
    }
}

pub fn request_close_document(workspace: &mut Workspace, state: &mut UiState, index: usize) {
    let dirty = workspace
        .docs
        .get(index)
        .map(|document| document.is_dirty())
        .unwrap_or(false);
    if dirty {
        state.pending_close = Some(CloseTarget::Document(index));
    } else {
        workspace.close_document(index);
    }
}

#[cfg(test)]
mod close_guard_tests {
    use super::*;

    #[test]
    fn dirty_document_close_is_deferred_until_confirmed() {
        let mut workspace = Workspace::sample();
        let mut state = UiState::new();
        workspace.add_entity("");
        let active = workspace.active_index();
        assert!(workspace.document().is_dirty());

        let count = workspace.docs.len();
        request_close_document(&mut workspace, &mut state, active);

        assert_eq!(workspace.docs.len(), count, "dirty document must stay open");
        assert_eq!(state.pending_close, Some(CloseTarget::Document(active)));
    }

    #[test]
    fn clean_document_close_is_not_deferred() {
        let mut workspace = Workspace::sample();
        let mut state = UiState::new();

        request_close_document(&mut workspace, &mut state, 0);

        assert!(state.pending_close.is_none());
    }
}

#[cfg(test)]
mod asset_browser_tests {
    use super::*;

    fn files() -> Vec<(String, AssetKind, u64)> {
        vec![
            ("assets/b.png".to_string(), AssetKind::Texture, 300),
            ("assets/a.json".to_string(), AssetKind::Data, 100),
            ("assets/c.png".to_string(), AssetKind::Texture, 200),
        ]
    }

    #[test]
    fn sorting_by_name_ignores_the_scan_order() {
        let mut list = files();
        sort_assets(&mut list, AssetSort::Name, false);
        let names: Vec<&str> = list.iter().map(|entry| entry.0.as_str()).collect();
        assert_eq!(
            names,
            ["assets/a.json", "assets/b.png", "assets/c.png"],
            "Sorted by filename ascending, independent of scan_assets order"
        );
        sort_assets(&mut list, AssetSort::Name, true);
        let names: Vec<&str> = list.iter().map(|entry| entry.0.as_str()).collect();
        assert_eq!(names, ["assets/c.png", "assets/b.png", "assets/a.json"]);
    }

    #[test]
    fn sorting_by_size_and_kind_breaks_ties_by_name() {
        let mut list = files();
        sort_assets(&mut list, AssetSort::Size, false);
        let order: Vec<u64> = list.iter().map(|entry| entry.2).collect();
        assert_eq!(order, [100, 200, 300]);

        sort_assets(&mut list, AssetSort::Kind, false);
        let names: Vec<&str> = list.iter().map(|entry| entry.0.as_str()).collect();
        assert_eq!(
            names,
            ["assets/b.png", "assets/c.png", "assets/a.json"],
            "Texture(1) before Data(7), by filename within same kind"
        );
    }

    fn paths(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn clicking_without_modifiers_replaces_the_selection() {
        let mut state = UiState::new();
        state.select_asset_only("assets/a.png");
        state.select_asset_only("assets/b.png");
        assert_eq!(state.asset_selection, paths(&["assets/b.png"]));
        assert_eq!(
            state.asset_primary().map(String::as_str),
            Some("assets/b.png")
        );
        assert_eq!(state.asset_anchor.as_deref(), Some("assets/b.png"));
    }

    #[test]
    fn ctrl_click_toggles_a_single_item() {
        let mut state = UiState::new();
        state.select_asset_only("assets/a.png");
        state.toggle_asset("assets/b.png");
        assert_eq!(
            state.asset_selection,
            paths(&["assets/a.png", "assets/b.png"])
        );
        state.toggle_asset("assets/a.png");
        assert_eq!(state.asset_selection, paths(&["assets/b.png"]));
        assert!(state.asset_selected("assets/b.png"));
    }

    #[test]
    fn shift_click_selects_the_range_from_the_anchor() {
        let visible = paths(&["a", "b", "c", "d", "e"]);
        let mut state = UiState::new();
        state.select_asset_only("b");
        state.select_asset_range(&visible, "d");
        assert_eq!(state.asset_selection, paths(&["b", "c", "d"]));

        state.select_asset_range(&visible, "a");
        assert_eq!(state.asset_selection, paths(&["a", "b"]));

        let mut state = UiState::new();
        state.select_asset_only("elsewhere/x");
        state.select_asset_range(&visible, "c");
        assert_eq!(state.asset_selection, paths(&["c"]));
    }

    #[test]
    fn forgetting_a_deleted_asset_keeps_the_rest_selected() {
        let mut state = UiState::new();
        state.asset_selection = paths(&["a", "b", "c"]);
        state.asset_anchor = Some("b".to_string());
        state.forget_asset("b");
        assert_eq!(state.asset_selection, paths(&["a", "c"]));
        assert_eq!(state.asset_anchor, None);
        state.clear_asset_selection();
        assert!(state.asset_selection.is_empty());
        assert!(state.asset_anchor.is_none());
    }

    #[test]
    fn rename_target_rejects_paths_and_keeps_the_folder() {
        assert_eq!(
            rename_target("assets/sprites/boat.png", " canoe.png "),
            Some("assets/sprites/canoe.png".to_string())
        );

        assert_eq!(rename_target("assets/boat.png", "sub/boat.png"), None);
        assert_eq!(rename_target("assets/boat.png", "   "), None);
        assert_eq!(rename_target("assets/boat.png", "a:b.png"), None);
    }
}
