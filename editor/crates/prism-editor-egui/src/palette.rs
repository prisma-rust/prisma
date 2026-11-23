//! Global search popup (`Ctrl+K`).
//!
//! Data layer in [`prism_editor_core::palette`] (entry collection + sort, pure function);
//! here only responsible for: centered input box, result list, ↑↓/Enter/Esc, and passing selected
//! [`PaletteAction`] back to `App` to execute.

use egui::Key;
use prism_editor_core::palette::{self, PaletteAction, PaletteEntry};
use prism_editor_core::workspace::Workspace;

pub struct Palette {
    open: bool,
    query: String,
    selected: usize,
    entries: Vec<PaletteEntry>,
    /// Index into `entries`, sorted by current query.
    order: Vec<usize>,
    focus_pending: bool,
}

impl Default for Palette {
    fn default() -> Self {
        Self::new()
    }
}

impl Palette {
    pub fn new() -> Self {
        Self {
            open: false,
            query: String::new(),
            selected: 0,
            entries: Vec::new(),
            order: Vec::new(),
            focus_pending: false,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open and rebuild entry table based on current workspace (entities / assets / documents change).
    pub fn open(&mut self, workspace: &Workspace) {
        self.entries = palette::entries(workspace);
        self.query.clear();
        self.order = palette::search("", &self.entries);
        self.selected = 0;
        self.open = true;
        self.focus_pending = true;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.selected = 0;
    }

    pub fn toggle(&mut self, workspace: &Workspace) {
        if self.open {
            self.close();
        } else {
            self.open(workspace);
        }
    }

    /// Draw one frame; returns the action of confirmed result (popup closes immediately).
    pub fn show(&mut self, ctx: &egui::Context, workspace: &Workspace) -> Option<PaletteAction> {
        if !self.open {
            return None;
        }

        let entries = &self.entries;
        let mut order = std::mem::take(&mut self.order);
        let mut selected = self.selected;
        let mut query = std::mem::take(&mut self.query);
        let mut focus_pending = self.focus_pending;
        let mut confirmed = false;
        let mut close = false;

        egui::Window::new("global-search")
            .id(egui::Id::new("global-search"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .fixed_size([560.0, 360.0])
            .anchor(egui::Align2::CENTER_TOP, [0.0, 96.0])
            .show(ctx, |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut query)
                        .hint_text(workspace.t("desktop.palette.placeholder"))
                        .desired_width(f32::INFINITY),
                );
                if focus_pending {
                    response.request_focus();
                    focus_pending = false;
                }
                if response.changed() {
                    order = palette::search(&query, entries);
                    selected = 0;
                }
                ui.separator();

                let total = order.len();
                if total == 0 {
                    ui.label(egui::RichText::new(workspace.t("desktop.palette.noMatches")).weak());
                } else {
                    if ui.input(|input| input.key_pressed(Key::ArrowDown)) {
                        selected = (selected + 1) % total;
                    }
                    if ui.input(|input| input.key_pressed(Key::ArrowUp)) {
                        selected = selected.checked_sub(1).unwrap_or(total - 1);
                    }
                    selected = selected.min(total - 1);
                    egui::ScrollArea::vertical()
                        .max_height(300.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for (row, &index) in order.iter().enumerate() {
                                let entry = &entries[index];
                                let text = if entry.detail.is_empty() {
                                    format!(
                                        "[{}] {}",
                                        workspace.t(kind_key(entry.kind)),
                                        entry.title
                                    )
                                } else {
                                    format!(
                                        "[{}] {}  —  {}",
                                        workspace.t(kind_key(entry.kind)),
                                        entry.title,
                                        entry.detail
                                    )
                                };
                                if ui.selectable_label(row == selected, text).clicked() {
                                    selected = row;
                                    confirmed = true;
                                }
                            }
                        });
                }
                if ui.input(|input| input.key_pressed(Key::Escape)) {
                    close = true;
                }
                if ui.input(|input| input.key_pressed(Key::Enter)) {
                    confirmed = true;
                }
            });

        let action = if confirmed {
            order
                .get(selected)
                .map(|&index| entries[index].action.clone())
        } else {
            None
        };

        self.order = order;
        self.selected = selected;
        self.query = query;
        self.focus_pending = focus_pending;
        if close || action.is_some() {
            self.close();
        }
        action
    }
}

fn kind_key(kind: palette::PaletteKind) -> &'static str {
    match kind {
        palette::PaletteKind::Command => "desktop.palette.kindCommand",
        palette::PaletteKind::Document => "desktop.palette.kindDocument",
        palette::PaletteKind::Entity => "desktop.palette.kindEntity",
        palette::PaletteKind::Asset => "desktop.palette.kindAsset",
    }
}
