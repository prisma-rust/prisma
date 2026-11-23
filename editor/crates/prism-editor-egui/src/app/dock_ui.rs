//! Dock split tree, tab bar, overlay, and floating panels.

use super::*;

pub(super) const TAB_BAR_HEIGHT: f32 = 26.0;

pub(super) const TAB_OVERFLOW_WIDTH: f32 = 22.0;

pub(super) fn panel_icon(tab: &str) -> &'static str {
    match tab {
        "sceneView" => icons::CUBE,
        "gameView" => icons::GAME_CONTROLLER,
        "hierarchy" => icons::TREE_STRUCTURE,
        "inspector" => icons::SLIDERS_HORIZONTAL,
        "assets" => icons::FOLDER,
        "docs" => icons::FILE_TEXT,
        "log" => icons::TERMINAL_WINDOW,
        "history" => icons::CLOCK_COUNTER_CLOCKWISE,
        "renderGraph" => icons::FILM_SLATE,
        "projectSettings" => icons::GEAR,
        "preferences" => icons::SLIDERS,
        "theme" => icons::PALETTE,
        "build" => icons::HAMMER,
        "problems" => icons::WARNING,
        "prefabOverrides" => icons::PACKAGE,
        "material" => icons::PALETTE,
        "animGraph" => icons::FILM_STRIP,
        "gaspDebug" => icons::BUG,
        "tools" => icons::WRENCH,
        "shortcuts" => icons::KEYBOARD,
        "about" => icons::INFO,
        _ => icons::FILE,
    }
}

pub(super) fn dock_tab_width(ui: &egui::Ui, title: &str, closable: bool) -> f32 {
    let text = ui.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(
                title.to_owned(),
                egui::FontId::proportional(12.0),
                Color32::PLACEHOLDER,
            )
            .size()
            .x
    });
    text + 36.0 + if closable { 16.0 } else { 0.0 }
}

pub(super) fn visible_tab_range(widths: &[f32], available: f32, active: usize) -> (usize, usize) {
    if widths.is_empty() {
        return (0, 0);
    }
    let active = active.min(widths.len() - 1);
    let mut end = 0;
    let mut used = 0.0;
    while end < widths.len() && used + widths[end] <= available {
        used += widths[end];
        end += 1;
    }

    if end == 0 {
        end = 1;
    }
    if active < end {
        return (0, end);
    }

    let mut start = active;
    let mut used = widths[active];
    while start > 0 && used + widths[start - 1] <= available {
        used += widths[start - 1];
        start -= 1;
    }
    (start, active + 1)
}

pub(super) fn tab_bar_needs_overflow(needed: f32, available: f32) -> bool {
    needed > available - 1.0
}

pub(super) struct DockTabOutcome {
    pub response: egui::Response,
    pub closed: bool,
}

pub(super) fn dock_tab(
    ui: &mut egui::Ui,
    tab_id: &str,
    title: &str,
    active: bool,
    closable: bool,
    colors: &prism_editor_core::ThemeColors,
) -> DockTabOutcome {
    use prism_editor_core::ThemeToken;
    let text_color = theme::color32(
        colors,
        if active {
            ThemeToken::TextStrong
        } else {
            ThemeToken::Muted
        },
    );
    let icon_color = theme::color32(
        colors,
        if active {
            ThemeToken::AccentAlt
        } else {
            ThemeToken::Muted
        },
    );
    let icon_galley = ui.painter().layout_no_wrap(
        panel_icon(tab_id).to_owned(),
        egui::FontId::proportional(12.0),
        icon_color,
    );
    let galley = ui.painter().layout_no_wrap(
        title.to_owned(),
        egui::FontId::proportional(12.0),
        text_color,
    );
    let close_space = if closable { 16.0 } else { 0.0 };
    let pad_x = 8.0_f32;
    let icon_gap = 5.0_f32;
    let size = Vec2::new(
        pad_x * 2.0 + icon_galley.size().x + icon_gap + galley.size().x + close_space,
        TAB_BAR_HEIGHT,
    );
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let close_rect = Rect::from_center_size(
        egui::pos2(rect.right() - 10.0, rect.center().y),
        Vec2::splat(14.0),
    );
    let show_close = closable && (active || response.hovered() || ui.rect_contains_pointer(rect));
    let mut closed = false;
    if show_close {
        let close_res = ui.interact(close_rect, close_res_id(ui, tab_id), Sense::click());
        if close_res.clicked() {
            closed = true;
        }
        if ui.is_rect_visible(rect) {}
        let _ = close_res;
    }

    if ui.is_rect_visible(rect) {
        let paintable = ui.painter();
        let bg = if active {
            theme::panel_fill(colors)
        } else if response.hovered() || response.dragged() {
            theme::color32(colors, ThemeToken::RowHover)
        } else {
            Color32::TRANSPARENT
        };
        if bg != Color32::TRANSPARENT {
            paintable.rect_filled(rect, 0.0, bg);
        }

        paintable.rect_filled(
            Rect::from_min_max(
                egui::pos2(rect.right() - 1.0, rect.top()),
                egui::pos2(rect.right(), rect.bottom()),
            ),
            0.0,
            theme::border(colors),
        );
        if active {
            paintable.rect_filled(
                Rect::from_min_max(
                    egui::pos2(rect.left(), rect.top()),
                    egui::pos2(rect.right() - 1.0, rect.top() + 2.0),
                ),
                0.0,
                theme::color32(colors, ThemeToken::Accent),
            );
        }
        let icon_x = rect.left() + pad_x;
        paintable.galley(
            egui::pos2(icon_x, rect.center().y - icon_galley.size().y * 0.5),
            icon_galley,
            icon_color,
        );
        let text_x = icon_x + 14.0;
        paintable.galley(
            egui::pos2(text_x, rect.center().y - galley.size().y * 0.5),
            galley,
            text_color,
        );
        if show_close {
            let close_hovered = ui.rect_contains_pointer(close_rect);
            if close_hovered {
                paintable.rect_filled(
                    close_rect,
                    egui::CornerRadius::same(2),
                    theme::color32(colors, ThemeToken::MenuHover),
                );
            }
            let x_color = theme::color32(
                colors,
                if close_hovered {
                    ThemeToken::TextStrong
                } else {
                    ThemeToken::Muted
                },
            );
            let x_galley = paintable.layout_no_wrap(
                icons::X.to_owned(),
                egui::FontId::proportional(10.0),
                x_color,
            );
            paintable.galley(
                close_rect.center() - x_galley.size() * 0.5,
                x_galley,
                x_color,
            );
        }
    }
    DockTabOutcome { response, closed }
}

fn close_res_id(ui: &egui::Ui, tab_id: &str) -> egui::Id {
    ui.id().with(("tab-close", tab_id))
}

pub(super) fn dock_compass_zones(rect: Rect) -> [(prism_editor_core::dock::DockZone, Rect); 5] {
    use prism_editor_core::dock::DockZone;
    let c = rect.center();
    let pad = Vec2::splat(28.0);
    let step = 32.0;
    [
        (DockZone::Center, Rect::from_center_size(c, pad)),
        (
            DockZone::Left,
            Rect::from_center_size(c + egui::vec2(-step, 0.0), pad),
        ),
        (
            DockZone::Right,
            Rect::from_center_size(c + egui::vec2(step, 0.0), pad),
        ),
        (
            DockZone::Top,
            Rect::from_center_size(c + egui::vec2(0.0, -step), pad),
        ),
        (
            DockZone::Bottom,
            Rect::from_center_size(c + egui::vec2(0.0, step), pad),
        ),
    ]
}

pub(super) fn dock_zone(rect: Rect, pos: egui::Pos2) -> prism_editor_core::dock::DockZone {
    let fx = (pos.x - rect.left()) / rect.width().max(1.0);
    let fy = (pos.y - rect.top()) / rect.height().max(1.0);
    if fx < 0.25 {
        prism_editor_core::dock::DockZone::Left
    } else if fx > 0.75 {
        prism_editor_core::dock::DockZone::Right
    } else if fy < 0.25 {
        prism_editor_core::dock::DockZone::Top
    } else if fy > 0.75 {
        prism_editor_core::dock::DockZone::Bottom
    } else {
        prism_editor_core::dock::DockZone::Center
    }
}

pub(super) fn scene_overlay_rect(viewport: Rect, collapsed: bool) -> Rect {
    let width: f32 = if collapsed { 30.0 } else { 640.0 };
    let available = (viewport.width() - 16.0).max(30.0).min(viewport.width());
    let height = 28.0_f32.min((viewport.height() - 10.0).max(16.0));
    Rect::from_min_size(
        viewport.min + egui::vec2(8.0, 8.0),
        egui::vec2(width.min(available), height),
    )
}

impl EditorApp {
    pub(super) fn dock_tree(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();

        if let Some(tab) = self.workspace.maximized_tab.clone() {
            match prism_editor_core::dock::find_leaf(&self.workspace.dock, &tab) {
                Some(DockNode::Leaf { id, tabs, .. }) => {
                    let (id, tabs) = (id.clone(), tabs.clone());
                    self.dock_leaf(ui, rect, &id, &tabs, &tab);
                    return;
                }
                _ => self.workspace.restore_maximized(),
            }
        }
        let dock = self.workspace.dock.clone();
        self.dock_node(ui, rect, &dock);
    }

    pub(super) fn dock_node(&mut self, ui: &mut egui::Ui, rect: Rect, node: &DockNode) {
        match node {
            DockNode::Leaf { id, tabs, active } => {
                self.dock_leaf(ui, rect, id, tabs, active);
            }
            DockNode::Split {
                id,
                dir,
                ratio,
                a,
                b,
            } => {
                let splitter = 5.0;
                match dir {
                    DockDirection::Row => {
                        let usable = (rect.width() - splitter).max(2.0);

                        let a_w = split_side_length(usable, *ratio, 60.0);
                        let a_rect = Rect::from_min_size(rect.min, Vec2::new(a_w, rect.height()));
                        let split_rect = Rect::from_min_size(
                            egui::pos2(rect.min.x + a_w, rect.min.y),
                            Vec2::new(splitter, rect.height()),
                        );
                        let b_rect = Rect::from_min_size(
                            egui::pos2(split_rect.max.x, rect.min.y),
                            Vec2::new(usable - a_w, rect.height()),
                        );
                        self.splitter(
                            ui,
                            split_rect,
                            CursorIcon::ResizeHorizontal,
                            id,
                            usable,
                            true,
                        );
                        self.dock_node(ui, a_rect, a);
                        self.dock_node(ui, b_rect, b);
                    }
                    DockDirection::Column => {
                        let usable = (rect.height() - splitter).max(2.0);
                        let a_h = split_side_length(usable, *ratio, 50.0);
                        let a_rect = Rect::from_min_size(rect.min, Vec2::new(rect.width(), a_h));
                        let split_rect = Rect::from_min_size(
                            egui::pos2(rect.min.x, rect.min.y + a_h),
                            Vec2::new(rect.width(), splitter),
                        );
                        let b_rect = Rect::from_min_size(
                            egui::pos2(rect.min.x, split_rect.max.y),
                            Vec2::new(rect.width(), usable - a_h),
                        );
                        self.splitter(
                            ui,
                            split_rect,
                            CursorIcon::ResizeVertical,
                            id,
                            usable,
                            false,
                        );
                        self.dock_node(ui, a_rect, a);
                        self.dock_node(ui, b_rect, b);
                    }
                }
            }
        }
    }

    pub(super) fn splitter(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        cursor: CursorIcon,
        split_id: &str,
        usable: f32,
        horizontal: bool,
    ) {
        let response = ui.interact(
            rect,
            ui.id().with(("splitter", split_id)),
            Sense::click_and_drag(),
        );
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(cursor);
        }

        let colors = self.workspace.colors();
        let color = if response.dragged() {
            theme::color32(&colors, prism_editor_core::ThemeToken::Accent)
        } else if response.hovered() {
            theme::color32(&colors, prism_editor_core::ThemeToken::BorderStrong)
        } else {
            theme::border(&colors)
        };
        ui.painter().rect_filled(rect, 0.0, color);
        if response.dragged() {
            let delta = ui.input(|i| i.pointer.delta());
            let delta = if horizontal { delta.x } else { delta.y };
            let current = self.split_ratio(split_id).unwrap_or(0.5);
            let next = (current + delta / usable.max(1.0)).clamp(0.12, 0.88);
            self.set_split_ratio(split_id, next);
        }
    }

    pub(super) fn split_ratio(&self, split_id: &str) -> Option<f32> {
        pub(super) fn walk(node: &DockNode, id: &str) -> Option<f32> {
            match node {
                DockNode::Leaf { .. } => None,
                DockNode::Split {
                    id: node_id,
                    ratio,
                    a,
                    b,
                    ..
                } => {
                    if node_id == id {
                        Some(*ratio)
                    } else {
                        walk(a, id).or_else(|| walk(b, id))
                    }
                }
            }
        }
        walk(&self.workspace.dock, split_id)
    }

    pub(super) fn set_split_ratio(&mut self, split_id: &str, ratio: f32) {
        self.workspace.dock =
            prism_editor_core::dock::set_ratio(&self.workspace.dock, split_id, ratio, 0.12, 0.88);
    }

    pub(super) fn dock_leaf(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        leaf_id: &str,
        tabs: &[String],
        active: &str,
    ) {
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::top_down(Align::Min)),
        );
        child.set_clip_rect(rect);
        self.dock_leaves.retain(|(id, _, _)| id != leaf_id);
        self.dock_leaves
            .push((leaf_id.to_owned(), active.to_owned(), tabs.to_vec()));

        let colors = self.workspace.colors();
        let sunken = theme::color32(&colors, prism_editor_core::ThemeToken::BgSunken);
        let border = theme::border(&colors);
        child
            .painter()
            .rect_filled(rect, 0.0, theme::panel_fill(&colors));
        child.painter().rect_stroke(
            rect,
            egui::CornerRadius::ZERO,
            Stroke::new(1.0_f32, border),
            egui::StrokeKind::Inside,
        );

        let mut activate: Option<String> = None;
        let mut float: Option<String> = None;
        let mut begin_drag: Option<String> = None;
        let mut maximize: Option<String> = None;
        let mut close: Option<String> = None;
        let mut close_others: Option<String> = None;
        let mut add_tab: Option<String> = None;
        let (bar, _) = child.allocate_exact_size(
            Vec2::new(child.available_width(), TAB_BAR_HEIGHT),
            egui::Sense::hover(),
        );
        child.painter().rect_filled(bar, 0.0, sunken);
        child.painter().rect_filled(
            Rect::from_min_max(
                egui::pos2(bar.left(), bar.bottom() - 1.0),
                egui::pos2(bar.right(), bar.bottom()),
            ),
            0.0,
            border,
        );

        let closable = tabs.len() > 1;
        let widths: Vec<f32> = tabs
            .iter()
            .map(|tab| dock_tab_width(&child, &self.workspace.panel_title(tab), closable))
            .collect();
        let needed: f32 = widths.iter().sum();
        let overflow = tab_bar_needs_overflow(needed, bar.width());
        let tabs_rect = if overflow {
            Rect::from_min_max(
                bar.min,
                egui::pos2(bar.right() - TAB_OVERFLOW_WIDTH, bar.bottom()),
            )
        } else {
            bar
        };
        let active_index = tabs.iter().position(|tab| tab == active).unwrap_or(0);
        let (visible_from, visible_to) =
            visible_tab_range(&widths, tabs_rect.width(), active_index);
        let mut bar_ui = child.new_child(
            egui::UiBuilder::new()
                .max_rect(tabs_rect)
                .layout(Layout::left_to_right(Align::Center)),
        );
        bar_ui.set_clip_rect(tabs_rect);
        let mut overflow_activate: Option<String> = None;

        let addable: Vec<(String, String)> = self
            .workspace
            .known_panels()
            .into_iter()
            .filter(|id| !tabs.iter().any(|tab| tab == id))
            .map(|id| {
                let title = self.workspace.panel_title(&id);
                (id, title)
            })
            .collect();
        let maximized = self.workspace.maximized_tab.clone();
        let bar_ui = &mut bar_ui;
        for tab in &tabs[visible_from..visible_to] {
            let selected = tab == active;
            let title = self.workspace.panel_title(tab);
            let tab_outcome = dock_tab(bar_ui, tab, &title, selected, closable, &colors);
            if tab_outcome.closed {
                close = Some(tab.clone());
                continue;
            }
            let response = tab_outcome.response;
            if response.clicked() {
                activate = Some(tab.clone());
                self.state.focused_panel = Some(tab.clone());
            }

            if response.double_clicked() {
                maximize = Some(tab.clone());
            }
            if response.drag_started() {
                begin_drag = Some(tab.clone());
            }
            response.context_menu(|ui| {
                let is_maximized = maximized.as_deref() == Some(tab.as_str());
                if ui
                    .button(if is_maximized {
                        self.t("desktop.dock.restore")
                    } else {
                        self.t("desktop.dock.maximize")
                    })
                    .clicked()
                {
                    maximize = Some(tab.clone());
                    ui.close();
                }
                if tabs.len() > 1 && ui.button(self.t("desktop.dock.float")).clicked() {
                    float = Some(tab.clone());
                    ui.close();
                }
                ui.separator();
                if tabs.len() > 1 && ui.button(self.t("desktop.dock.closeTab")).clicked() {
                    close = Some(tab.clone());
                    ui.close();
                }
                if tabs.len() > 1 && ui.button(self.t("desktop.dock.closeOthers")).clicked() {
                    close_others = Some(tab.clone());
                    ui.close();
                }
                if !addable.is_empty() {
                    ui.menu_button(self.t("desktop.dock.addTab"), |ui| {
                        for (id, title) in &addable {
                            if ui.button(title).clicked() {
                                add_tab = Some(id.clone());
                                ui.close();
                            }
                        }
                    });
                }
            });
        }
        if overflow {
            let button_rect = Rect::from_min_max(
                egui::pos2(bar.right() - TAB_OVERFLOW_WIDTH + 2.0, bar.top() + 2.0),
                egui::pos2(bar.right() - 2.0, bar.bottom() - 3.0),
            );
            let mut button_ui = child.new_child(
                egui::UiBuilder::new()
                    .max_rect(button_rect)
                    .layout(Layout::left_to_right(Align::Center)),
            );
            button_ui.set_clip_rect(button_rect);
            button_ui.menu_button(icons::CARET_DOWN, |ui| {
                for tab in tabs {
                    let title = self.workspace.panel_title(tab);
                    if ui.selectable_label(tab == active, title).clicked() {
                        overflow_activate = Some(tab.clone());
                        ui.close();
                    }
                }
            });
        }
        if let Some(tab) = overflow_activate {
            activate = Some(tab);
        }

        let body = child.available_rect_before_wrap();
        let mut body_ui = child.new_child(
            egui::UiBuilder::new()
                .max_rect(body)
                .layout(Layout::top_down(Align::Min)),
        );
        body_ui.set_clip_rect(body);
        self.panel_body(&mut body_ui, active, body);

        if let Some(tab) = begin_drag {
            self.state.tab_drag = Some((tab, leaf_id.to_string()));
        }

        if let Some((drag_tab, source)) = self.state.tab_drag.clone() {
            let pointer = ui.input(|input| input.pointer.hover_pos());
            if let Some(pos) = pointer {
                if rect.contains(pos) {
                    use prism_editor_core::dock::DockZone;
                    use prism_editor_core::ThemeToken;
                    let show_compass = rect.width() >= 140.0 && rect.height() >= 140.0;
                    let compass = dock_compass_zones(rect);
                    let compass_hit = if show_compass {
                        compass
                            .iter()
                            .find(|(_, pad)| pad.contains(pos))
                            .map(|(z, _)| *z)
                    } else {
                        None
                    };
                    let zone = compass_hit.unwrap_or_else(|| dock_zone(rect, pos));
                    if zone != DockZone::Center || source != leaf_id {
                        self.state.tab_drop = Some((leaf_id.to_string(), zone, 0.5));
                        let preview = match zone {
                            DockZone::Center => rect.shrink(4.0),
                            DockZone::Left => Rect::from_min_max(
                                rect.min,
                                egui::pos2(rect.center().x, rect.max.y),
                            ),
                            DockZone::Right => Rect::from_min_max(
                                egui::pos2(rect.center().x, rect.min.y),
                                rect.max,
                            ),
                            DockZone::Top => Rect::from_min_max(
                                rect.min,
                                egui::pos2(rect.max.x, rect.center().y),
                            ),
                            DockZone::Bottom => Rect::from_min_max(
                                egui::pos2(rect.min.x, rect.center().y),
                                rect.max,
                            ),
                        };
                        let accent = theme::color32(&colors, ThemeToken::Accent);
                        let fill =
                            Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 44);
                        ui.painter()
                            .rect_filled(preview, egui::CornerRadius::same(2), fill);
                        ui.painter().rect_stroke(
                            preview,
                            egui::CornerRadius::same(2),
                            egui::Stroke::new(2.0_f32, accent),
                            egui::StrokeKind::Inside,
                        );
                    }
                    if show_compass {
                        let painter = ui.painter();
                        let accent = theme::color32(&colors, ThemeToken::Accent);
                        let pad_bg = theme::color32(&colors, ThemeToken::PanelRaised);
                        let pad_border = theme::color32(&colors, ThemeToken::BorderStrong);
                        let active_fill = theme::color32(&colors, ThemeToken::AccentSolid);
                        let on_accent = theme::color32(&colors, ThemeToken::OnAccent);
                        let text_soft = theme::color32(&colors, ThemeToken::TextSoft);
                        for (z, pad_rect) in compass {
                            if z == DockZone::Center && source == leaf_id {
                                continue;
                            }
                            let selected = z == zone;
                            let bg = if selected {
                                active_fill
                            } else {
                                Color32::from_rgba_unmultiplied(
                                    pad_bg.r(),
                                    pad_bg.g(),
                                    pad_bg.b(),
                                    230,
                                )
                            };
                            let fg = if selected { on_accent } else { text_soft };
                            let stroke = if selected { accent } else { pad_border };
                            painter.rect_filled(pad_rect, egui::CornerRadius::same(4), bg);
                            painter.rect_stroke(
                                pad_rect,
                                egui::CornerRadius::same(4),
                                Stroke::new(1.0_f32, stroke),
                                egui::StrokeKind::Inside,
                            );
                            let glyph = match z {
                                DockZone::Center => icons::SQUARES_FOUR,
                                DockZone::Left => icons::CARET_LEFT,
                                DockZone::Right => icons::CARET_RIGHT,
                                DockZone::Top => icons::CARET_UP,
                                DockZone::Bottom => icons::CARET_DOWN,
                            };
                            painter.text(
                                pad_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                glyph,
                                egui::FontId::proportional(13.0),
                                fg,
                            );
                        }
                    }
                    let _ = &drag_tab;
                }
            }
        }

        if let Some(tab) = activate {
            self.workspace.activate_panel(leaf_id, &tab);
        }
        if let Some(tab) = float {
            self.workspace.float_panel(&tab, 260.0, 160.0);
        }
        if let Some(tab) = maximize {
            self.workspace.toggle_maximized(&tab);
        }
        if let Some(tab) = close {
            self.workspace.close_panel(&tab);
        }
        if let Some(tab) = close_others {
            self.workspace.close_other_panels(&tab);
        }
        if let Some(tab) = add_tab {
            self.workspace.add_tab_to_leaf(leaf_id, &tab);
        }
    }

    pub(super) fn finish_tab_drag(&mut self, ctx: &egui::Context) {
        if !ctx.input(|input| input.pointer.any_released()) {
            self.state.tab_drop = None;
            return;
        }
        let Some((tab, source)) = self.state.tab_drag.take() else {
            self.state.tab_drop = None;
            return;
        };
        match self.state.tab_drop.take() {
            Some((target, zone, ratio)) => {
                if target != source || zone != prism_editor_core::dock::DockZone::Center {
                    let dock = prism_editor_core::dock::insert_panel(
                        &self.workspace.dock,
                        &target,
                        &tab,
                        zone,
                        ratio,
                        None,
                    );
                    self.workspace.dock = dock;
                }
            }
            None => {
                if let Some(pos) = ctx.input(|input| input.pointer.interact_pos()) {
                    self.workspace.float_panel(&tab, pos.x, pos.y);
                }
            }
        }
    }

    pub(super) fn panel_body(&mut self, ui: &mut egui::Ui, tab: &str, rect: Rect) {
        self.panel_rects.insert(tab.to_owned(), rect);

        if ui.rect_contains_pointer(rect) && ui.input(|input| input.pointer.any_click()) {
            self.state.focused_panel = Some(tab.to_owned());
        }

        let no_viewport = std::env::var("PRISMA_EDITOR_NO_VIEWPORT").is_ok();
        match tab {
            "sceneView" if no_viewport => {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new(self.t("desktop.viewport.disabled")).weak())
                });
            }
            "gameView" if no_viewport => {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new(self.t("desktop.viewport.disabled")).weak())
                });
            }
            "sceneView" => self.show_viewport(ui, ViewKind::Scene, rect),
            "gameView" => self.show_viewport(ui, ViewKind::Game, rect),
            "hierarchy" => panels::hierarchy(ui, &mut self.workspace, &mut self.state),
            "inspector" => panels::inspector(ui, &mut self.workspace, &mut self.state),
            "assets" => {
                panels::assets(ui, &mut self.workspace, &mut self.state);
                if let Some((path, project_wide)) = self.state.pending_find_references.take() {
                    self.find_references(&path, project_wide);
                }
            }
            "docs" => panels::docs(ui, &mut self.workspace, &mut self.state),
            "log" => panels::log(ui, &mut self.workspace, &mut self.state),
            "history" => panels::history(ui, &mut self.workspace, &mut self.state),
            "renderGraph" => {
                refresh_render_report(self);
                let was_paused = self.state.render_graph_paused;
                if was_paused && self.state.render_graph_frozen.is_none() {
                    self.state.render_graph_frozen = self.render_report.clone();
                }
                let report = if was_paused {
                    self.state.render_graph_frozen.as_deref()
                } else {
                    self.render_report.as_deref()
                };
                let mut paused = was_paused;
                let mut settings = self.render_settings.clone();
                if panels::render_graph(
                    ui,
                    &self.workspace,
                    report,
                    &mut settings,
                    &mut self.state.viewport_scale,
                    &mut paused,
                    &mut self.state.render_graph_call,
                    &mut self.state.render_graph_budget_ms,
                ) {
                    self.apply_render_settings(settings);
                }
                if paused != was_paused {
                    self.state.render_graph_paused = paused;
                    self.state.render_graph_frozen = if paused {
                        self.render_report.clone()
                    } else {
                        None
                    };
                }
            }
            "projectSettings" => panels::project_settings(
                ui,
                &self.workspace,
                self.project_root.as_deref(),
                &mut self.state,
            ),
            "preferences" => panels::preferences(
                ui,
                &mut self.preferences,
                &mut self.workspace,
                &mut self.state,
                self.control_cli_disabled,
            ),
            "theme" => panels::theme(ui, &mut self.workspace),
            "build" => {
                panels::build(ui, &mut self.workspace, &mut self.state);
                if let Some(kind) = self.state.pending_build.take() {
                    self.start_build(kind);
                }
            }
            "problems" => {
                let module = self
                    .game_view
                    .as_ref()
                    .map(|view| view.host.project_diagnostics().to_vec())
                    .unwrap_or_default();
                panels::problems(
                    ui,
                    &self.workspace,
                    &module,
                    &self.build_diagnostics,
                    &mut self.state,
                );
            }
            "prefabOverrides" => panels::prefab_overrides(ui, &mut self.workspace, &mut self.state),
            "material" => panels::material(ui, &mut self.workspace, &mut self.state),
            "animGraph" => {
                let runtime = self.game_view.as_ref().and_then(|view| {
                    let world = view.host.world();
                    let graph = world
                        .get_resource::<prism_engine::scene::systems::animator::AnimGraphDebug>()?;
                    Some(panels::AnimRuntimeView {
                        graph,
                        events: world
                            .get_resource::<prism_engine::scene::systems::animator::AnimEvents>(),
                    })
                });
                panels::anim_graph(ui, &mut self.workspace, &mut self.state, runtime)
            }
            "gaspDebug" => {
                let runtime_graph = self.game_view.as_ref().and_then(|view| {
                    view.host
                        .world()
                        .get_resource::<prism_engine::scene::systems::animator::AnimGraphDebug>()
                });
                crate::gasp_debug::gasp_debug_panel(
                    ui,
                    &self.workspace,
                    &mut self.state.gasp_debug,
                    runtime_graph,
                );
            }
            "tools" => {
                panels::tools(ui, &mut self.workspace, &mut self.state);
                if let Some(kind) = self.state.pending_build.take() {
                    self.start_build(kind);
                }
            }
            "shortcuts" => {
                panels::shortcuts(ui, &self.workspace, &mut self.preferences, &mut self.state)
            }
            "about" => panels::about(ui, &self.workspace),
            other => {
                ui.label(
                    RichText::new(self.tf("desktop.status.panelMissing", &[("panel", other)]))
                        .weak(),
                );
            }
        }
    }

    fn prt_mode_label(&self, mode: prism_editor_core::PrtDisplayMode) -> String {
        self.t(match mode {
            prism_editor_core::PrtDisplayMode::ShSpheres => "desktop.viewport.prtSpheres",
            prism_editor_core::PrtDisplayMode::MeanIrradiance => "desktop.viewport.prtMean",
            prism_editor_core::PrtDisplayMode::DominantDirection => "desktop.viewport.prtDirection",
            prism_editor_core::PrtDisplayMode::GridBoundsOnly => "desktop.viewport.prtBounds",
        })
    }

    pub(super) fn scene_overlay(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let collapsed = self.state.scene_overlay_collapsed;
        let bar = scene_overlay_rect(rect, collapsed);
        let mut settings = self.render_settings.clone();
        let mut changed = false;
        let mut skybox_changed = false;
        let mut camera_changed = false;
        let mut mark_dirty = false;
        let mut collapsed_now = collapsed;

        let mut fov = self.workspace.camera.fov_y_degrees;
        let mut near = self.workspace.camera.near;
        let mut far = self.workspace.camera.far;
        let mut frame_sel = false;
        let mut reset_cam = false;

        let colors = self.workspace.colors();
        let border_strong = theme::color32(&colors, prism_editor_core::ThemeToken::BorderStrong);
        ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(18, 20, 26, 224))
                .stroke(egui::Stroke::new(1.0_f32, border_strong))
                .corner_radius(egui::CornerRadius::same(4))
                .inner_margin(egui::Margin::symmetric(6, 2))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.horizontal(|ui| {
                        if ui
                            .small_button(if collapsed_now {
                                icons::CARET_RIGHT
                            } else {
                                icons::CARET_DOWN
                            })
                            .clicked()
                        {
                            collapsed_now = !collapsed_now;
                        }
                        if collapsed_now {
                            return;
                        }
                        if panels::scene_render_mode_combo(ui, &self.workspace, &mut settings) {
                            changed = true;
                        }
                        if ui
                            .selectable_label(
                                self.workspace.camera.orthographic,
                                self.t("desktop.viewport.ortho"),
                            )
                            .clicked()
                        {
                            self.workspace.camera.toggle_2d();
                            camera_changed = true;
                        }
                        if ui
                            .selectable_label(settings.show_grid, self.t("desktop.viewport.grid"))
                            .clicked()
                        {
                            settings.show_grid = !settings.show_grid;
                            changed = true;
                        }
                        if ui
                            .selectable_label(
                                self.workspace.show_skybox,
                                self.t("desktop.viewport.sky"),
                            )
                            .clicked()
                        {
                            self.workspace.show_skybox = !self.workspace.show_skybox;
                            skybox_changed = true;
                        }
                        if ui
                            .selectable_label(
                                self.workspace.show_selection_outline,
                                self.t("desktop.viewport.outline"),
                            )
                            .clicked()
                        {
                            self.workspace.show_selection_outline =
                                !self.workspace.show_selection_outline;
                            mark_dirty = true;
                        }
                        if ui
                            .selectable_label(
                                self.workspace.show_skeleton,
                                self.t("desktop.viewport.bones"),
                            )
                            .clicked()
                        {
                            self.workspace.show_skeleton = !self.workspace.show_skeleton;
                        }
                        let raw_ui = self
                            .scene_view
                            .as_ref()
                            .is_some_and(|view| view.raw_ui_edit);
                        if ui
                            .selectable_label(raw_ui, self.t("desktop.viewport.uiRaw"))
                            .clicked()
                        {
                            if let Some(view) = self.scene_view.as_mut() {
                                view.raw_ui_edit = !raw_ui;
                            }
                        }
                        if ui
                            .selectable_label(
                                self.workspace.prt_gizmos.enabled,
                                self.t("desktop.viewport.probes"),
                            )
                            .clicked()
                        {
                            self.workspace.prt_gizmos.enabled = !self.workspace.prt_gizmos.enabled;
                        }

                        ui.menu_button(self.t("desktop.viewport.viewMenu"), |ui| {
                            ui.label(
                                egui::RichText::new(self.t("desktop.viewport.editorCamera"))
                                    .strong()
                                    .size(11.0),
                            );
                            if ui
                                .add(
                                    egui::Slider::new(&mut fov, 20.0..=110.0)
                                        .text(self.t("desktop.viewport.fov")),
                                )
                                .changed()
                            {
                                camera_changed = true;
                            }
                            if ui
                                .add(
                                    egui::DragValue::new(&mut near)
                                        .speed(0.01)
                                        .range(0.001..=100.0)
                                        .prefix(self.t("desktop.viewport.near")),
                                )
                                .changed()
                            {
                                camera_changed = true;
                            }
                            if ui
                                .add(
                                    egui::DragValue::new(&mut far)
                                        .speed(1.0)
                                        .range(1.0..=100_000.0)
                                        .prefix(self.t("desktop.viewport.far")),
                                )
                                .changed()
                            {
                                camera_changed = true;
                            }
                            ui.separator();
                            let label = self.t("desktop.viewport.cameraIcons");
                            ui.checkbox(&mut self.workspace.show_camera_icons, label);
                            let label = self.t("desktop.viewport.lightIcons");
                            ui.checkbox(&mut self.workspace.show_light_icons, label);
                            let label = self.t("desktop.viewport.colliderGizmos");
                            ui.checkbox(&mut self.workspace.show_collider_gizmos, label);
                            let label = self.t("desktop.viewport.audioGizmos");
                            ui.checkbox(&mut self.workspace.show_audio_gizmos, label);
                            ui.separator();
                            ui.label(
                                egui::RichText::new(self.t("desktop.viewport.prtVolume"))
                                    .strong()
                                    .size(11.0),
                            );
                            let show_probes = self.t("desktop.viewport.showProbes");
                            ui.checkbox(&mut self.workspace.prt_gizmos.enabled, show_probes);
                            if self.workspace.prt_gizmos.enabled {
                                ui.horizontal(|ui| {
                                    ui.label(self.t("desktop.viewport.mode"));
                                    egui::ComboBox::from_id_salt("view-prt-mode")
                                        .selected_text(
                                            self.prt_mode_label(self.workspace.prt_gizmos.mode),
                                        )
                                        .width(128.0)
                                        .show_ui(ui, |ui| {
                                            for mode in prism_editor_core::PrtDisplayMode::ALL {
                                                let label = self.prt_mode_label(mode);
                                                ui.selectable_value(
                                                    &mut self.workspace.prt_gizmos.mode,
                                                    mode,
                                                    label,
                                                );
                                            }
                                        });
                                });
                                let radius_prefix = self.t("desktop.viewport.probeRadiusPrefix");
                                let exposure_prefix = self.t("desktop.viewport.exposurePrefix");
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::DragValue::new(
                                            &mut self.workspace.prt_gizmos.probe_radius,
                                        )
                                        .speed(0.01)
                                        .range(0.04..=2.0)
                                        .prefix(radius_prefix),
                                    );
                                    ui.add(
                                        egui::DragValue::new(
                                            &mut self.workspace.prt_gizmos.exposure,
                                        )
                                        .speed(0.05)
                                        .range(0.05..=16.0)
                                        .prefix(exposure_prefix),
                                    );
                                });
                                let label = self.t("desktop.viewport.gridBounds");
                                ui.checkbox(&mut self.workspace.prt_gizmos.show_grid_bounds, label);
                                let label = self.t("desktop.viewport.tethers");
                                ui.checkbox(
                                    &mut self.workspace.prt_gizmos.show_trilinear_tethers,
                                    label,
                                );
                                if let Some(layers) = self
                                    .scene_view
                                    .as_ref()
                                    .and_then(|v| v.cached_probe_volume())
                                    .map(|v| v.dims[1])
                                {
                                    ui.horizontal(|ui| {
                                        let mut sliced =
                                            self.workspace.prt_gizmos.slice_y.is_some();
                                        if ui
                                            .checkbox(
                                                &mut sliced,
                                                self.t("desktop.viewport.ySlice"),
                                            )
                                            .changed()
                                        {
                                            self.workspace.prt_gizmos.slice_y =
                                                if sliced { Some(0) } else { None };
                                        }
                                        if let Some(ref mut layer) =
                                            self.workspace.prt_gizmos.slice_y
                                        {
                                            let top = layers.saturating_sub(1);
                                            ui.add(
                                                egui::DragValue::new(layer)
                                                    .range(0..=top)
                                                    .speed(0.1),
                                            );
                                        }
                                    });
                                }
                            }
                            ui.separator();
                            let snap_prefix = self.t("desktop.viewport.snapStep");
                            ui.add(
                                egui::DragValue::new(&mut self.workspace.snap.translate)
                                    .speed(0.01)
                                    .range(0.01..=100.0)
                                    .prefix(snap_prefix),
                            );
                        });

                        if ui.small_button(icons::CROSSHAIR).clicked() {
                            frame_sel = true;
                        }
                        if ui.small_button(icons::ARROWS_CLOCKWISE).clicked() {
                            reset_cam = true;
                        }
                    });
                });
        });

        if collapsed_now != collapsed {
            self.state.scene_overlay_collapsed = collapsed_now;
        }
        if changed {
            self.apply_render_settings(settings);
        }
        if skybox_changed {
            let on = self.workspace.show_skybox;
            if let Some(view) = self.scene_view.as_mut() {
                view.host.set_show_skybox(on);
                view.mark_dirty();
            }
            self.mark_scene_dirty();
        }
        if camera_changed {
            self.workspace.camera.set_fov_degrees(fov);
            self.workspace.camera.set_clip_planes(near, far);
            self.mark_scene_dirty();
            if let Some(view) = self.scene_view.as_mut() {
                view.mark_dirty();
            }
        } else if mark_dirty {
            self.mark_scene_dirty();
            if let Some(view) = self.scene_view.as_mut() {
                view.mark_dirty();
            }
        }
        if frame_sel {
            self.apply_command(Command::FrameSelection);
            if let Some(view) = self.scene_view.as_mut() {
                view.mark_dirty();
            }
        }
        if reset_cam {
            self.apply_command(Command::ResetSceneCamera);
            self.mark_scene_dirty();
        }
    }

    pub(super) fn show_viewport(&mut self, ui: &mut egui::Ui, kind: ViewKind, rect: Rect) {
        let _ = rect;
        if !self.ensure_view(kind) {
            ui.label(
                RichText::new(self.t("desktop.viewport.unavailable"))
                    .color(Color32::from_rgb(232, 96, 96)),
            );
            return;
        }

        if kind == ViewKind::Scene {
            let selection_hash = selection_hash(&self.workspace);
            if selection_hash != self.selection_hash {
                self.selection_hash = selection_hash;
                if let Some(view) = self.scene_view.as_mut() {
                    view.mark_dirty();
                }
            }
            let camera = self.workspace.camera;
            let selected: Vec<usize> = self.workspace.selection.iter().copied().collect();
            let outline = self.workspace.show_selection_outline;
            let hidden = self.workspace.hidden_entity_ids();
            let visibility_hash = {
                let mut hash = 0xcbf2_9ce4_8422_2325u64 ^ u64::from(self.workspace.isolate);
                for id in &hidden {
                    hash ^= u64::from(*id);
                    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                }
                hash
            };
            if visibility_hash != self.visibility_hash {
                self.visibility_hash = visibility_hash;
                if let Some(view) = self.scene_view.as_mut() {
                    view.mark_dirty();
                }
            }
            if let Some(view) = self.scene_view.as_mut() {
                view.host.place_editor_camera(&camera);

                view.host.set_selection_outline(&selected, outline);

                view.host.set_scene_hidden(&hidden);
            }
        }

        let playing = self.playing;
        let game_keyboard_focused = self.state.focused_panel.as_deref() == Some("gameView")
            && !ui.ctx().wants_keyboard_input();

        let mut document_changed = false;
        {
            let bridge = &self.bridge;
            let playing = playing;
            let workspace = &mut self.workspace;
            let view = match kind {
                ViewKind::Scene => self.scene_view.as_mut(),
                ViewKind::Game => self.game_view.as_mut(),
            };
            if let Some(view) = view {
                if kind == ViewKind::Game {
                    view.game_keyboard_focused = game_keyboard_focused;
                }
                let rerender = playing && kind == ViewKind::Game;
                let response =
                    view.show(ui, workspace, bridge, rerender, self.state.viewport_scale);
                document_changed = response.document_changed;
            }
        }
        if document_changed {
            self.state.status = self.t("desktop.status.movedSelection");
        }
        if kind == ViewKind::Scene {
            self.scene_overlay(ui, rect);
        }
    }

    pub(super) fn floating_panels(&mut self, ctx: &egui::Context) {
        let mut to_close: Vec<String> = Vec::new();
        for index in 0..self.workspace.floating.len() {
            let Some(panel) = self.workspace.floating.get(index).cloned() else {
                continue;
            };
            let mut open = true;
            let mut titled = false;
            let title = self.workspace.panel_title(&panel.tab);
            let shown = egui::Window::new(title)
                .id(egui::Id::new(("float", &panel.tab)))
                .open(&mut open)
                .default_pos(egui::pos2(panel.x, panel.y))
                .default_size(egui::vec2(panel.width.max(260.0), panel.height.max(180.0)))
                .resizable(true)
                .show(ctx, |ui| {
                    titled = true;

                    let rect = ui.available_rect_before_wrap();
                    self.panel_body(ui, &panel.tab, rect);
                });
            let _ = titled;

            if let Some(shown) = shown {
                let rect = shown.response.rect;
                let moved =
                    (rect.min.x - panel.x).abs() > 0.5 || (rect.min.y - panel.y).abs() > 0.5;
                let resized = (rect.width() - panel.width).abs() > 0.5
                    || (rect.height() - panel.height).abs() > 0.5;
                if moved || resized {
                    if let Some(slot) = self.workspace.floating.get_mut(index) {
                        slot.x = rect.min.x;
                        slot.y = rect.min.y;
                        slot.width = rect.width();
                        slot.height = rect.height();
                    }
                }
            }
            if !open {
                to_close.push(panel.tab.clone());
            }
        }
        for tab in to_close {
            self.workspace.close_floating(&tab);
        }
    }
}
