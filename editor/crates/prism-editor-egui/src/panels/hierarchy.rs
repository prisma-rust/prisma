//! Hierarchy panel UI.

use super::*;

pub(super) enum TreeAction {
    Select(usize, bool),
    Toggle(usize),
    ToggleExpand(usize),
    ToggleActive(usize),
    ToggleHidden(usize),
    Delete(usize),
    Duplicate(usize),
    CommitRename(usize, String),
    BeginRename(usize, String),

    Frame(usize),

    PreviewRename(usize, String),

    CancelRename,

    ClearSelection,
}

pub(super) fn search_matches(
    doc: &SceneDoc,
    filter: &str,
    mode: HierarchySearchMode,
) -> std::collections::HashSet<usize> {
    let mut keep = std::collections::HashSet::new();
    if filter.is_empty() {
        return keep;
    }
    for (index, entity) in doc.entities.iter().enumerate() {
        if mode.matches(entity, filter) {
            keep.extend(doc.path_of(index));
        }
    }
    keep
}

pub(super) fn hierarchy_indices(workspace: &Workspace, state: &mut UiState) -> Vec<usize> {
    let Some(ids) = state.hierarchy_lock.clone() else {
        return workspace.selection.clone();
    };
    let resolved: Vec<usize> = ids
        .iter()
        .filter_map(|id| workspace.document().doc.index_of_id(*id))
        .collect();
    if resolved.is_empty() {
        state.hierarchy_lock = None;
        return workspace.selection.clone();
    }
    resolved
}

pub(super) fn toggle_hierarchy_lock(workspace: &Workspace, state: &mut UiState) {
    if state.hierarchy_lock.is_some() {
        state.hierarchy_lock = None;
        return;
    }
    state.hierarchy_lock = Some(
        workspace
            .selection
            .iter()
            .filter_map(|index| workspace.document().doc.id_of_index(*index))
            .collect(),
    );
}

pub(super) fn sync_hierarchy_lock(workspace: &Workspace, state: &mut UiState) {
    if state.hierarchy_lock.is_some() {
        state.hierarchy_lock = Some(
            workspace
                .selection
                .iter()
                .filter_map(|index| workspace.document().doc.id_of_index(*index))
                .collect(),
        );
    }
}

pub(crate) fn reveal_in_hierarchy(workspace: &mut Workspace, state: &mut UiState, index: usize) {
    state.hierarchy_search.clear();

    state.hierarchy_lock = None;
    let ids: Vec<u32> = workspace
        .document()
        .doc
        .path_of(index)
        .into_iter()
        .filter_map(|path_index| workspace.document().doc.id_of_index(path_index))
        .collect();
    workspace.expanded.extend(ids);
    state.hierarchy_reveal = workspace.document().doc.id_of_index(index);
}

#[expect(
    dead_code,
    reason = "create and show_all responses are returned alongside tested toolbar widgets"
)]
pub(super) struct HierarchyToolbar {
    pub(super) search: egui::Response,
    pub(super) mode: egui::Response,
    pub(super) create: egui::Response,
    pub(super) show_all: Option<egui::Response>,
    pub(super) isolate: egui::Response,
    pub(super) lock: egui::Response,
    pub(super) find: egui::Response,
}

/// Hierarchy toolbar: search / search mode / create / visibility / lock / find.
/// Returns the widget responses so interaction tests can click the real
/// controls without duplicating the layout.
pub(super) fn hierarchy_toolbar(
    ui: &mut Ui,
    workspace: &mut Workspace,
    state: &mut UiState,
) -> HierarchyToolbar {
    let mut toolbar = None;
    ui.horizontal(|ui| {
        let search = ui.add(
            egui::TextEdit::singleline(&mut state.hierarchy_search)
                .hint_text(
                    if state.hierarchy_search_mode == HierarchySearchMode::Component {
                        workspace.t("hierarchy.filterType")
                    } else {
                        workspace.t("hierarchy.filterPlaceholder")
                    },
                )
                .desired_width(110.0),
        );
        let mode = egui::ComboBox::from_id_salt("hierarchy-search-mode")
            .width(56.0)
            .selected_text(match state.hierarchy_search_mode {
                HierarchySearchMode::Name => workspace.t("desktop.hierarchy.byName"),
                HierarchySearchMode::Component => workspace.t("desktop.hierarchy.byType"),
            })
            .show_ui(ui, |ui| {
                for mode in [HierarchySearchMode::Name, HierarchySearchMode::Component] {
                    let label = match mode {
                        HierarchySearchMode::Name => workspace.t("desktop.hierarchy.byName"),
                        HierarchySearchMode::Component => workspace.t("desktop.hierarchy.byType"),
                    };
                    ui.selectable_value(&mut state.hierarchy_search_mode, mode, label);
                }
            })
            .response;
        let presets: Vec<(String, String)> = workspace
            .presets
            .iter()
            .map(|preset| (preset.id.clone(), preset.label.clone()))
            .collect();
        let create = if presets.is_empty() {
            let create = ui.button(workspace.t("hierarchy.createMenu"));
            if create.clicked() {
                workspace.add_entity("");
            }
            create
        } else {
            let current = state.preset_choice.min(presets.len().saturating_sub(1));
            ui.menu_button(workspace.t("hierarchy.createMenu"), |ui| {
                for (index, (id, label)) in presets.iter().enumerate() {
                    if ui.selectable_label(index == current, label).clicked() {
                        state.preset_choice = index;
                        workspace.add_entity(id);
                        ui.close();
                    }
                }
            })
            .response
        };
        let visibility_changed =
            !workspace.document().hidden_entities.is_empty() || workspace.isolate;
        let show_all = visibility_changed.then(|| {
            let response = ui.button(format!(
                "{} {}",
                icons::EYE,
                workspace.t("hierarchy.showAll")
            ));
            if response.clicked() {
                workspace.show_all_entities();
            }
            response
        });
        let isolate = ui.add_enabled(
            workspace.isolate || !workspace.selection.is_empty(),
            egui::Button::new(format!(
                "{} {}",
                icons::TARGET,
                workspace.t("hierarchy.isolate")
            ))
            .selected(workspace.isolate),
        );
        if isolate.clicked() {
            workspace.set_isolate(!workspace.isolate);
        }

        let hierarchy_locked = state.hierarchy_lock.is_some();
        let lock = ui.add_enabled(
            hierarchy_locked || !workspace.selection.is_empty(),
            egui::Button::new(if hierarchy_locked {
                icons::LOCK_SIMPLE
            } else {
                icons::LOCK_SIMPLE_OPEN
            })
            .selected(hierarchy_locked),
        );
        let lock = if hierarchy_locked {
            lock.on_hover_text(workspace.t("hierarchy.unlock"))
        } else {
            lock.on_hover_text(workspace.t("hierarchy.lock"))
        };
        if lock.clicked() {
            toggle_hierarchy_lock(workspace, state);
        }
        let find = ui.add_enabled(
            workspace.primary_selection().is_some(),
            egui::Button::new(icons::CROSSHAIR),
        );
        let find = find.on_hover_text(workspace.t("hierarchy.findSelected"));
        if find.clicked() {
            if let Some(index) = workspace.primary_selection() {
                reveal_in_hierarchy(workspace, state, index);
            }
        }
        toolbar = Some(HierarchyToolbar {
            search,
            mode,
            create,
            show_all,
            isolate,
            lock,
            find,
        });
    });
    toolbar.expect("horizontal layout always runs")
}

/// Apply one hierarchy row click (also used by the interaction tests).
/// `extend` is Shift; Ctrl/Cmd uses [`apply_hierarchy_toggle`] instead. The
/// range walks `visible_rows`, so collapsed/filtered entities never sneak in.
pub(super) fn apply_hierarchy_select(
    workspace: &mut Workspace,
    state: &mut UiState,
    index: usize,
    extend: bool,
    visible_rows: &[usize],
) {
    if extend {
        let anchor = state
            .hierarchy_anchor
            .filter(|anchor| visible_rows.contains(anchor))
            .or_else(|| workspace.primary_selection());
        match anchor {
            Some(anchor) => workspace.select_visible_range(visible_rows, anchor, index),
            None => {
                workspace.select(index, false);
                state.hierarchy_anchor = Some(index);
            }
        }
    } else {
        workspace.select(index, false);
        state.hierarchy_anchor = Some(index);
    }
}

pub(super) fn entity_semantic_icon(entity: &prism_editor_core::EntityJson) -> &'static str {
    if entity.prefab_link().is_some() {
        icons::PACKAGE
    } else if entity.has_component("prism_engine::scene::Camera") {
        icons::CAMERA
    } else if entity.has_component("prism_engine::scene::DirectionalLight")
        || entity.has_component("prism_engine::scene::PointLight")
        || entity.has_component("prism_engine::scene::SpotLight")
        || entity.has_component("prism_engine::scene::Light")
    {
        icons::LIGHTBULB
    } else if entity.has_component("prism_engine::scene::MeshRenderer")
        || entity.has_component("prism_engine::scene::SkinnedMeshRenderer")
    {
        icons::CUBE
    } else if entity.has_component("prism_engine::scene::SpriteRenderer")
        || entity.has_component("prism_engine::scene::Tilemap")
    {
        icons::IMAGE
    } else if entity.has_component("prism_engine::ui::Style")
        || entity.has_component("prism_engine::ui::UiText")
    {
        icons::SQUARES_FOUR
    } else if entity.has_component("prism_engine::scene::AudioSource")
        || entity.has_component("prism_engine::scene::AudioListener")
    {
        icons::MUSIC_NOTES
    } else {
        icons::CIRCLE
    }
}

/// Ctrl/Cmd-click one hierarchy row: add or remove it from the selection.
pub(super) fn apply_hierarchy_toggle(workspace: &mut Workspace, state: &mut UiState, index: usize) {
    state.hierarchy_anchor = Some(index);
    if workspace.selection.contains(&index) {
        let remaining: Vec<usize> = workspace
            .selection
            .iter()
            .copied()
            .filter(|selected| *selected != index)
            .collect();
        workspace.select_many(&remaining, false);
    } else {
        workspace.select_many(&[index], true);
    }
}

pub fn hierarchy(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    let _ = hierarchy_toolbar(ui, workspace, state);
    ui.separator();

    let filter = state.hierarchy_search.to_lowercase();
    let searching = !filter.is_empty();

    let search = search_matches(
        &workspace.document().doc,
        &filter,
        state.hierarchy_search_mode,
    );
    if searching {
        let ids: Vec<u32> = {
            let doc = &workspace.document().doc;
            search
                .iter()
                .filter_map(|index| doc.id_of_index(*index))
                .collect()
        };
        workspace.expanded.extend(ids);
    }
    let expanded = workspace.expanded.clone();
    let rows = workspace.document().doc.visible_rows(&expanded);

    let display_selection = hierarchy_indices(workspace, state);

    let hidden_ids: std::collections::HashSet<u32> =
        workspace.hidden_entity_ids().into_iter().collect();

    let visible_rows: Vec<usize> = rows
        .iter()
        .filter(|(index, _)| !searching || search.contains(index))
        .map(|(index, _)| *index)
        .collect();

    let broken: std::collections::HashSet<usize> = workspace
        .broken_references()
        .into_iter()
        .map(|(entity, _, _)| entity)
        .collect();
    let mut action: Option<TreeAction> = None;

    let mut asset_drop: Option<(usize, String)> = None;
    let mut hovered_drop_target = false;

    let mut row_context_open = false;

    let area_rect = egui::ScrollArea::vertical()
        .id_salt("hierarchy-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let area_rect = ui.max_rect();
            for &(index, _id) in &rows {
                let doc = &workspace.document().doc;
                let Some(entity) = doc.entities.get(index) else {
                    continue;
                };
                let name = entity.display_name();
                if searching && !search.contains(&index) {
                    continue;
                }
                let depth = doc.path_of(index).len().saturating_sub(1);
                let selected = display_selection.contains(&index);
                let expandable = !doc.children(index).is_empty();
                let expanded_here = entity
                    .id
                    .map(|id| workspace.expanded.contains(&id))
                    .unwrap_or(false);
                let active = entity.active();
                let renaming = matches!(&state.rename, Some((target, _)) if *target == index);

                let (row_rect, row) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), tree_row_height(state.compact)),
                    egui::Sense::click_and_drag(),
                );
                if state.hierarchy_reveal.is_some() && state.hierarchy_reveal == entity.id {
                    row.scroll_to_me(Some(Align::Center));
                    state.hierarchy_reveal = None;
                }
                if ui.is_rect_visible(row_rect) {
                    use prism_editor_core::ThemeToken;
                    let colors = workspace.colors();
                    let bg = if selected {
                        Some(theme::color32(&colors, ThemeToken::Selection))
                    } else if row.hovered() {
                        Some(theme::color32(&colors, ThemeToken::RowHover))
                    } else {
                        None
                    };
                    if let Some(bg) = bg {
                        ui.painter().rect_filled(row_rect, 0.0, bg);
                    }
                    if selected {
                        ui.painter().rect_filled(
                            egui::Rect::from_min_max(
                                row_rect.left_top(),
                                egui::pos2(row_rect.left() + 2.0, row_rect.bottom()),
                            ),
                            0.0,
                            theme::color32(&colors, ThemeToken::Accent),
                        );
                    }
                    if depth > 0 {
                        let guide_color = theme::color32(&colors, ThemeToken::BorderSubtle);
                        let guide_stroke = egui::Stroke::new(1.0_f32, guide_color);
                        for level in 0..depth {
                            let gx = row_rect.left() + 12.0 + level as f32 * 12.0;
                            ui.painter().line_segment(
                                [
                                    egui::pos2(gx, row_rect.top()),
                                    egui::pos2(gx, row_rect.bottom()),
                                ],
                                guide_stroke,
                            );
                        }
                        let leaf_x = row_rect.left() + 12.0 + (depth - 1) as f32 * 12.0;
                        let mid_y = row_rect.center().y;
                        ui.painter().line_segment(
                            [egui::pos2(leaf_x, mid_y), egui::pos2(leaf_x + 6.0, mid_y)],
                            guide_stroke,
                        );
                    }
                }
                let mut row_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(row_rect)
                        .layout(Layout::left_to_right(Align::Center)),
                );
                row_ui.add_space(6.0 + depth as f32 * 12.0);
                if expandable {
                    let arrow = if expanded_here {
                        icons::CARET_DOWN
                    } else {
                        icons::CARET_RIGHT
                    };
                    if flat_icon(&mut row_ui, arrow).clicked() {
                        action = Some(TreeAction::ToggleExpand(index));
                    }
                } else {
                    row_ui.add_space(16.0);
                }

                let ancestor_active = doc
                    .path_of(index)
                    .iter()
                    .rev()
                    .skip(1)
                    .all(|ancestor| doc.entities.get(*ancestor).is_some_and(|e| e.active()));

                let active_icon = icons::POWER;
                let response = flat_icon_dimmed(
                    &mut row_ui,
                    active_icon,
                    !active || (active && !ancestor_active),
                );
                let response = if active && !ancestor_active {
                    response
                } else {
                    response.on_hover_text(workspace.t("hierarchy.toggleActive"))
                };
                if response.clicked() {
                    action = Some(TreeAction::ToggleActive(index));
                }
                let hidden = entity.id.is_some_and(|id| hidden_ids.contains(&id));
                let visibility_icon = if hidden {
                    icons::EYE_CLOSED
                } else {
                    icons::EYE
                };
                let visibility = flat_icon_dimmed(&mut row_ui, visibility_icon, hidden);
                let visibility = if workspace.isolate && hidden {
                    visibility
                } else if hidden {
                    visibility.on_hover_text(workspace.t("hierarchy.showInScene"))
                } else {
                    visibility.on_hover_text(workspace.t("hierarchy.hideInScene"))
                };
                if visibility.clicked() && !(workspace.isolate && hidden) {
                    action = Some(TreeAction::ToggleHidden(index));
                }
                if renaming {
                    if let Some((_, buffer)) = state.rename.as_mut() {
                        let response = row_ui.add(
                            egui::TextEdit::singleline(buffer)
                                .desired_width(row_ui.available_width().max(60.0)),
                        );
                        if state.rename_focus {
                            response.request_focus();
                            state.rename_focus = false;
                        }
                        let (enter, escape, clicked) = ui.input(|input| {
                            (
                                input.key_pressed(egui::Key::Enter),
                                input.key_pressed(egui::Key::Escape),
                                input.pointer.any_click(),
                            )
                        });

                        let clicked_outside = clicked
                            && ui
                                .input(|input| input.pointer.interact_pos())
                                .map(|pos| !row_rect.contains(pos))
                                .unwrap_or(false);
                        if response.changed() {
                            action = Some(TreeAction::PreviewRename(index, buffer.clone()));
                        }
                        if escape {
                            action = Some(TreeAction::CancelRename);
                            state.rename_focus = false;
                        } else if enter || clicked_outside || response.lost_focus() {
                            action = Some(TreeAction::CommitRename(index, buffer.clone()));
                        }
                    }
                } else {
                    use prism_editor_core::ThemeToken;
                    let colors = workspace.colors();
                    let is_prefab = entity.prefab_link().is_some();
                    let effectively_active = active && ancestor_active;
                    let text_color = if selected {
                        theme::color32(&colors, ThemeToken::OnAccent)
                    } else if !effectively_active {
                        theme::color32(&colors, ThemeToken::Muted)
                    } else if is_prefab {
                        theme::color32(&colors, ThemeToken::AccentAlt)
                    } else {
                        theme::color32(&colors, ThemeToken::Text)
                    };

                    let entity_icon = entity_semantic_icon(entity);
                    let icon_color = if selected {
                        theme::color32(&colors, ThemeToken::OnAccent)
                    } else if !effectively_active {
                        theme::color32(&colors, ThemeToken::MutedDim)
                    } else if is_prefab {
                        theme::color32(&colors, ThemeToken::AccentAlt)
                    } else if entity_icon == icons::LIGHTBULB {
                        theme::color32(&colors, ThemeToken::Warning)
                    } else {
                        theme::color32(&colors, ThemeToken::TextSoft)
                    };
                    row_ui.label(RichText::new(entity_icon).size(11.0).color(icon_color));

                    let text_pos = row_ui.cursor().min;
                    let font = egui::FontId::proportional(12.0);
                    row_ui.painter().text(
                        egui::pos2(text_pos.x, row_ui.max_rect().center().y),
                        egui::Align2::LEFT_CENTER,
                        &name,
                        font.clone(),
                        text_color,
                    );
                    if broken.contains(&index) {
                        let width = row_ui
                            .painter()
                            .layout_no_wrap(name.clone(), font, text_color)
                            .size()
                            .x;
                        let center =
                            egui::pos2(text_pos.x + width + 9.0, row_ui.max_rect().center().y);
                        let badge = egui::Color32::from_rgb(226, 96, 96);
                        row_ui.painter().circle_filled(center, 5.0, badge);
                        row_ui.painter().text(
                            center,
                            egui::Align2::CENTER_CENTER,
                            "!",
                            egui::FontId::proportional(9.0),
                            egui::Color32::WHITE,
                        );
                    }
                }

                if !renaming {
                    if row.clicked() {
                        let modifiers = ui.input(|i| i.modifiers);

                        if modifiers.ctrl || modifiers.command {
                            action = Some(TreeAction::Toggle(index));
                        } else {
                            action = Some(TreeAction::Select(index, modifiers.shift));
                        }
                    }
                    if row.double_clicked() {
                        action = Some(TreeAction::Frame(index));
                    }
                }

                if row.drag_started_by(egui::PointerButton::Primary) {
                    state.dragging = Some(index);
                }
                if state.dragging.is_some() && state.dragging != Some(index) && row.hovered() {
                    let from = state.dragging.expect("dragging checked above");
                    let invalid = from == index
                        || workspace.document().doc.descendants(from).contains(&index);
                    hovered_drop_target = true;
                    let painter = ui.painter();
                    if invalid {
                        state.drop_target = None;
                        painter.rect_stroke(
                            row.rect,
                            egui::CornerRadius::ZERO,
                            egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(220, 90, 90)),
                            egui::StrokeKind::Inside,
                        );
                    } else {
                        let pointer_y = ui
                            .input(|input| input.pointer.interact_pos())
                            .map(|pos| pos.y)
                            .unwrap_or(row.rect.center().y);
                        let ratio = (pointer_y - row.rect.top()) / row.rect.height().max(1.0);
                        let zone = if ratio < 0.25 {
                            DropZone::Before
                        } else if ratio > 0.75 {
                            DropZone::After
                        } else {
                            DropZone::Child
                        };
                        state.drop_target = Some((index, zone));
                        match zone {
                            DropZone::Before => painter.line_segment(
                                [row.rect.left_top(), row.rect.right_top()],
                                egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(90, 160, 240)),
                            ),
                            DropZone::After => painter.line_segment(
                                [row.rect.left_bottom(), row.rect.right_bottom()],
                                egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(90, 160, 240)),
                            ),
                            DropZone::Child => painter.rect_stroke(
                                row.rect,
                                egui::CornerRadius::ZERO,
                                egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(90, 160, 240)),
                                egui::StrokeKind::Inside,
                            ),
                        };
                    }
                }

                if row.dnd_hover_payload::<widgets::AssetDrag>().is_some() {
                    ui.painter().rect_filled(
                        row.rect,
                        egui::CornerRadius::ZERO,
                        egui::Color32::from_rgba_unmultiplied(90, 170, 120, 40),
                    );
                }
                if let Some(payload) = row.dnd_release_payload::<widgets::AssetDrag>() {
                    if payload.path.ends_with(".prefab.json") {
                        asset_drop = Some((index, payload.path.clone()));
                    }
                }

                if row.secondary_clicked() {
                    row_context_open = true;
                    if workspace.select_for_context(index) {
                        sync_hierarchy_lock(workspace, state);
                    }
                }

                row.context_menu(|ui| {
                    let hidden_now = hidden;
                    let visibility_label = if hidden_now {
                        workspace.t("hierarchy.showInScene")
                    } else {
                        workspace.t("hierarchy.hideInScene")
                    };
                    if ui.button(visibility_label).clicked() {
                        if workspace.isolate && hidden_now {
                            workspace.show_all_entities();
                        } else {
                            workspace.set_entity_hidden(index, !hidden_now);
                        }
                        ui.close();
                    }
                    if ui
                        .add_enabled(
                            !workspace.selection.is_empty(),
                            egui::Button::new(workspace.t("hierarchy.isolateSelection")),
                        )
                        .clicked()
                    {
                        workspace.set_isolate(true);
                        ui.close();
                    }
                    if ui.button(workspace.t("hierarchy.showAll")).clicked() {
                        workspace.show_all_entities();
                        ui.close();
                    }
                    ui.separator();
                    if ui.button(workspace.t("common.rename")).clicked() {
                        action = Some(TreeAction::BeginRename(index, name.clone()));
                        ui.close();
                    }
                    if ui.button(workspace.t("common.duplicate")).clicked() {
                        action = Some(TreeAction::Duplicate(index));
                        ui.close();
                    }
                    if ui.button(workspace.t("common.delete")).clicked() {
                        action = Some(TreeAction::Delete(index));
                        ui.close();
                    }
                    if ui.button(workspace.t("menu.selectChildren")).clicked() {
                        let children = workspace.document().doc.descendants(index);
                        workspace.select_many(&children, false);
                        sync_hierarchy_lock(workspace, state);
                        ui.close();
                    }
                    if ui
                        .button(workspace.t("hierarchy.setFirstSibling"))
                        .clicked()
                    {
                        let _ = workspace.document_mut().doc.move_entity(index, 0);
                        ui.close();
                    }
                    if ui.button(workspace.t("hierarchy.setLastSibling")).clicked() {
                        let last = workspace.document().doc.entities.len().saturating_sub(1);
                        let _ = workspace.document_mut().doc.move_entity(index, last);
                        ui.close();
                    }
                    if ui.button(workspace.t("hierarchy.moveToRoot")).clicked() {
                        workspace.reparent(index, None, None);
                        ui.close();
                    }
                    if let Some(parent) = workspace.primary_selection() {
                        if parent != index
                            && ui
                                .button(workspace.t("hierarchy.parentToSelection"))
                                .clicked()
                        {
                            workspace.reparent(index, Some(parent), None);
                            ui.close();
                        }
                    }
                });
            }
            area_rect
        })
        .inner;

    if state.dragging.is_some() && !hovered_drop_target {
        state.drop_target = None;
    }

    if !row_context_open
        && ui.input(|input| input.pointer.secondary_clicked())
        && ui
            .input(|input| input.pointer.interact_pos())
            .is_some_and(|pos| area_rect.contains(pos))
    {
        action = Some(TreeAction::ClearSelection);
    }

    // Hierarchy keeps a lightweight ghost under the pointer while dragging.
    // It makes the operation legible even when the pointer is between rows or over
    // the empty tail of the scroll area.
    if let (Some(from), Some(pointer)) = (
        state.dragging,
        ui.input(|input| input.pointer.interact_pos()),
    ) {
        let label = workspace
            .document()
            .doc
            .entities
            .get(from)
            .map(|entity| entity.display_name())
            .unwrap_or_else(|| workspace.t("common.entity"));
        let ghost = egui::Rect::from_min_size(
            pointer + egui::vec2(12.0, 12.0),
            egui::vec2((label.len() as f32 * 7.0 + 30.0).clamp(96.0, 260.0), 22.0),
        );
        let painter = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("hierarchy-drag-preview"),
        ));
        painter.rect_filled(
            ghost,
            egui::CornerRadius::same(3),
            egui::Color32::from_rgba_unmultiplied(35, 42, 54, 235),
        );
        painter.rect_stroke(
            ghost,
            egui::CornerRadius::same(3),
            egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(100, 170, 245)),
            egui::StrokeKind::Inside,
        );
        painter.text(
            ghost.left_center() + egui::vec2(8.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(11.0),
            egui::Color32::WHITE,
        );
    }

    if ui.input(|input| input.pointer.button_released(egui::PointerButton::Primary)) {
        if let (Some(from), Some((to, zone))) = (state.dragging.take(), state.drop_target.take()) {
            if from != to {
                let parent_of_to = workspace.document().doc.entities[to]
                    .parent
                    .and_then(|id| workspace.document().doc.index_of_id(id));

                let group = workspace.selection.len() > 1 && workspace.selection.contains(&from);
                if group {
                    let moved = match zone {
                        DropZone::Child => workspace.reparent_selection(Some(to)),
                        DropZone::Before => workspace.reparent_selection_at(to, true),
                        DropZone::After => workspace.reparent_selection_at(to, false),
                    };
                    if moved > 0 {
                        state.status = workspace.tf(
                            "desktop.status.movedEntities",
                            &[("count", &moved.to_string())],
                        );
                    }
                    state.drop_target = None;
                    return;
                }
                match zone {
                    DropZone::Child => workspace.reparent(from, Some(to), None),
                    DropZone::Before => {
                        let mut position = to;
                        if from < position {
                            position = position.saturating_sub(1);
                        }
                        workspace.reparent(from, parent_of_to, Some(position));
                    }
                    DropZone::After => {
                        let mut position = to.saturating_add(1);
                        if from < position {
                            position = position.saturating_sub(1);
                        }
                        workspace.reparent(from, parent_of_to, Some(position));
                    }
                }
            }
        }
        state.drop_target = None;
    }

    if let Some((parent, prefab)) = asset_drop {
        match workspace.instantiate_prefab_asset(&prefab, Some(parent)) {
            Ok(count) => {
                state.status = workspace.tf(
                    "desktop.status.instantiatedFromPrefab",
                    &[("count", &count.to_string()), ("path", &prefab)],
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

    let typing = ui.ctx().memory(|memory| memory.focused()).is_some();
    let rename_key = ui.input(|input| {
        input.key_pressed(egui::Key::F2)
            || (cfg!(target_os = "macos") && input.key_pressed(egui::Key::Enter))
    });
    if !typing && rename_key && state.rename.is_none() {
        if let Some(index) = workspace.primary_selection() {
            if let Some(entity) = workspace.document().doc.entities.get(index) {
                let name = entity.display_name();
                action = Some(TreeAction::BeginRename(index, name));
            }
        }
    }

    if let Some(action) = action {
        let selection_before = workspace.selection.clone();
        match action {
            TreeAction::Select(index, extend) => {
                apply_hierarchy_select(workspace, state, index, extend, &visible_rows);
            }
            TreeAction::Toggle(index) => {
                apply_hierarchy_toggle(workspace, state, index);
            }
            TreeAction::ToggleExpand(index) => workspace.toggle_expanded(index),
            TreeAction::ToggleActive(index) => {
                let active = workspace.document().doc.entities[index].active();
                workspace.set_active_entity(index, !active);
            }
            TreeAction::ToggleHidden(index) => {
                let hidden = workspace.is_entity_hidden(index);
                workspace.set_entity_hidden(index, !hidden);
            }
            TreeAction::Delete(index) => {
                workspace.select(index, false);
                if state.confirm_delete {
                    state.pending_entity_delete = Some(vec![index]);
                } else {
                    workspace.delete_selection();
                }
                state.rename = None;
                state.rename_base = None;
            }
            TreeAction::Duplicate(index) => {
                workspace.select(index, false);
                workspace.duplicate_selection();
            }
            TreeAction::BeginRename(index, name) => {
                state.rename_base =
                    Some((workspace.document().doc.clone(), workspace.document().dirty));
                state.rename = Some((index, name));
                state.rename_focus = true;
            }
            TreeAction::PreviewRename(index, name) => {
                workspace.preview_rename(index, &name);
            }
            TreeAction::CancelRename => {
                if let Some((base, was_dirty)) = state.rename_base.take() {
                    workspace.cancel_rename(base, was_dirty);
                }
                state.rename = None;
            }
            TreeAction::Frame(index) => {
                workspace.select(index, false);
                workspace.frame_selection();
            }
            TreeAction::ClearSelection => workspace.select_many(&[], false),
            TreeAction::CommitRename(index, name) => {
                let name = name.trim().to_string();
                match state.rename_base.take() {
                    Some((base, was_dirty)) => {
                        if name.is_empty() {
                            workspace.cancel_rename(base, was_dirty);
                        } else {
                            workspace.preview_rename(index, &name);
                            workspace.finish_rename(base, was_dirty);
                        }
                    }
                    None => {
                        if !name.is_empty() {
                            workspace.rename_entity(index, &name);
                        }
                    }
                }
                state.rename = None;
            }
        }
        if workspace.selection != selection_before {
            sync_hierarchy_lock(workspace, state);
        }
    }
}

#[cfg(test)]
mod hierarchy_search_tests {
    use super::*;

    #[test]
    fn search_keeps_ancestors_of_matches() {
        let mut doc = SceneDoc {
            version: 3,
            entities: Vec::new(),
            ..Default::default()
        };
        let parent = doc.add_named_child(None, "Boat");
        let child = doc.add_named_child(Some(parent), "Rod");
        doc.add_named_child(None, "Water");

        let keep = search_matches(&doc, "rod", HierarchySearchMode::Name);
        assert!(keep.contains(&child));
        assert!(keep.contains(&parent), "collapsed parent must stay visible");
        assert_eq!(keep.len(), 2);

        assert!(search_matches(&doc, "", HierarchySearchMode::Name).is_empty());
        assert!(search_matches(&doc, "missing", HierarchySearchMode::Name).is_empty());
    }

    #[test]
    fn component_search_matches_the_short_type_name_and_keeps_ancestors() {
        let mut doc = SceneDoc::default();
        let parent = doc.add_named_child(None, "Lamp");
        let _child = doc.add_named_child(Some(parent), "Bulb");
        doc.entities[parent].components.insert(
            "prism_engine::scene::PointLight".to_string(),
            serde_json::json!({ "intensity": 2.0 }),
        );

        let keep = search_matches(&doc, "pointlight", HierarchySearchMode::Component);
        assert!(keep.contains(&parent));
        assert_eq!(
            keep.len(),
            1,
            "ancestor chain only fills ancestors of hit item, does not expand descendants"
        );

        assert!(search_matches(&doc, "pointlight", HierarchySearchMode::Name).is_empty());
        assert!(search_matches(&doc, "lamp", HierarchySearchMode::Component).is_empty());
    }
}
