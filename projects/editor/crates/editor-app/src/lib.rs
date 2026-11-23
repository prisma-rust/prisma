//! Prisma self-hosted editor ("editor as a game") entry & modular workspace.

#![deny(warnings)]

pub mod about;
pub mod bridge;
pub mod commands;
pub mod i18n;
pub mod inspector;
pub mod menus;
pub mod panels;
pub mod state;
pub mod widgets;
pub mod windows;

use glam::Vec2;
use prism_app::Subsystem;
use prism_ui::prelude::*;

use crate::about::{about_default_rect, build_about_body_localized, ABOUT_CLOSE_KEY};
use crate::commands::{EditorCommand, LayoutPreset, ReversibleCommand, UndoStack};
use crate::i18n::EditorI18n;
use crate::menus::{build_menus, menu_actions_to_commands};
use crate::panels::{EditorPanel, EditorPlugin, PanelRegistry, PanelViewCtx};
use crate::state::{EditorDocument, EditorSession};
use crate::widgets::{editor_toolbar, toolbar_action_to_command};

pub use crate::state::{EditorEntity, GizmoMode};

/// Retained editor coordinator hosted as a resource inside `prism-app` / `prism-engine`.
pub struct EditorUiState {
    pub doc: EditorDocument,
    pub session: EditorSession,
    pub undo: UndoStack,
    pub panels: PanelRegistry,
    pub tree: UiTree,
    pub draw_list: UiDrawList,
    /// Design tokens every widget reads; switching it restyles the whole editor.
    pub theme: Theme,
    /// Which menu is open. Editor policy, so it lives here rather than in the UI kernel.
    pub menu: MenuState,
    /// Whether the About dialog is on screen.
    pub about_open: bool,
    /// Unity-style docking tree and floating container windows.
    pub window_layout: WindowLayout,
    /// Uploaded Lucide SVG icon bindless textures.
    pub icons: IconSet,
    /// Config-driven JSON localization (`locales/*.json`).
    pub i18n: EditorI18n,
    /// Last known viewport size for centring newly opened floating windows.
    last_viewport: Vec2,
    /// Cursor the UI wants this frame, forwarded to the platform layer.
    pub desired_cursor: CursorHint,
    /// Set when the user asks to quit; the host app leaves the event loop.
    pub exit_requested: bool,
    /// Input signature of the last processed frame, used to skip idle rebuilds.
    last_frame_sig: Option<u64>,
    /// Whether any UI animation is still running.
    has_active_animations: bool,
}

impl Default for EditorUiState {
    fn default() -> Self {
        let panels = PanelRegistry::default();
        let i18n = EditorI18n::default();
        let window_layout = windows::build_layout(
            LayoutPreset::Default,
            &panels,
            &i18n,
            ABOUT_CLOSE_KEY.into_node_key(),
        );
        Self {
            doc: EditorDocument::default(),
            session: EditorSession::default(),
            undo: UndoStack::default(),
            panels,
            tree: UiTree::new(),
            draw_list: UiDrawList::default(),
            theme: Theme::dark(),
            menu: MenuState::default(),
            about_open: false,
            window_layout,
            icons: IconSet::default(),
            i18n,
            last_viewport: Vec2::new(1280.0, 720.0),
            desired_cursor: CursorHint::Default,
            exit_requested: false,
            last_frame_sig: None,
            has_active_animations: false,
        }
    }
}

impl EditorUiState {
    fn view_ctx(&self, viewport_size: Vec2) -> PanelViewCtx<'_> {
        PanelViewCtx {
            doc: &self.doc,
            session: &self.session,
            theme: &self.theme,
            i18n: &self.i18n,
            icons: &self.icons,
            viewport_size,
        }
    }

    /// Register a plugin and refresh window metadata/layout.
    pub fn register_plugin(&mut self, plugin: &dyn EditorPlugin) {
        self.panels.install_plugin(plugin);
        self.panels
            .register_windows(&mut self.window_layout, &self.i18n);
    }

    /// Register a standalone panel and refresh window metadata.
    pub fn register_panel(&mut self, panel: Box<dyn EditorPanel>) {
        self.panels.register(panel);
        self.panels
            .register_windows(&mut self.window_layout, &self.i18n);
    }

    /// Build the editor's top-level menu bar definition.
    pub fn menus(&self) -> Vec<Menu> {
        let ctx = self.view_ctx(self.last_viewport);
        build_menus(&ctx, &self.panels, &self.window_layout, &self.undo)
    }

    /// Apply menu commands triggered this frame by translating them to [`EditorCommand`]s.
    pub fn apply_menu_actions(&mut self, actions: Vec<MenuAction>) {
        let cmds = menu_actions_to_commands(actions);
        self.apply_commands(cmds);
    }

    /// Central command dispatcher: the single mutator of editor state.
    pub fn apply_commands(&mut self, cmds: Vec<EditorCommand>) {
        for cmd in cmds {
            match cmd {
                EditorCommand::Doc(doc_cmd) => {
                    doc_cmd.apply(&mut self.doc);
                    self.undo.push(doc_cmd);
                }
                EditorCommand::PreviewField {
                    entity_id,
                    component,
                    field,
                    axis,
                    value,
                } => {
                    self.doc
                        .apply_field_value(entity_id, &component, &field, axis, value);
                }
                EditorCommand::Undo => {
                    if let Some(doc_cmd) = self.undo.pop_undo() {
                        doc_cmd.revert(&mut self.doc);
                    }
                }
                EditorCommand::Redo => {
                    if let Some(doc_cmd) = self.undo.pop_redo() {
                        doc_cmd.apply(&mut self.doc);
                    }
                }
                EditorCommand::SelectEntity(id) => self.doc.selected_entity = id,
                EditorCommand::SetEntityExpanded { id, expanded } => {
                    if expanded {
                        self.doc.expanded.insert(id);
                    } else {
                        self.doc.expanded.remove(&id);
                    }
                }
                EditorCommand::SetSearchQuery(query) => self.session.search_query = query,
                EditorCommand::SetGizmoMode(mode) => self.session.gizmo_mode = mode,
                EditorCommand::SetPlaying(playing) => self.session.playing = playing,
                EditorCommand::TogglePlay => self.session.playing = !self.session.playing,
                EditorCommand::ToggleGrid => self.session.show_grid = !self.session.show_grid,
                EditorCommand::ToggleGizmos => {
                    self.session.show_gizmos = !self.session.show_gizmos;
                }
                EditorCommand::SetTheme(preset) => self.theme = Theme::from_preset(preset),
                EditorCommand::SetLocale(locale) => {
                    self.i18n.locale = locale;
                    self.sync_all_window_titles();
                }
                EditorCommand::ToggleWindow(id) => self.window_layout.get_window(&id),
                EditorCommand::OpenAbout => {
                    let rect = about_default_rect(&self.theme, self.last_viewport);
                    self.window_layout
                        .show_floating(&"about".into(), rect, true);
                    self.about_open = true;
                }
                EditorCommand::CloseAbout => {
                    self.window_layout.close_window(&"about".into());
                    self.about_open = false;
                }
                EditorCommand::ResetLayout => self.set_layout(LayoutPreset::Default),
                EditorCommand::SetLayout(preset) => self.set_layout(preset),
                EditorCommand::RequestExit => self.exit_requested = true,
                EditorCommand::SelectAll => {
                    if let Some(first) = self.doc.entities.first() {
                        self.doc.selected_entity = first.id;
                    }
                }
                EditorCommand::DuplicateSelected => {
                    if let Some(src) = self.doc.selected_entity_ref().cloned() {
                        let mut dup = src;
                        dup.id = self.doc.next_entity_id();
                        dup.name = format!("{} (Copy)", dup.name);
                        self.doc.selected_entity = dup.id;
                        self.doc.entities.push(dup);
                        self.doc.dirty = true;
                    }
                }
                EditorCommand::DeleteSelected => {
                    let id = self.doc.selected_entity;
                    if id != 1 {
                        self.doc.entities.retain(|e| e.id != id);
                        self.doc.selected_entity = self.doc.entities.first().map_or(0, |e| e.id);
                        self.doc.dirty = true;
                    }
                }
            }
        }
    }

    fn set_layout(&mut self, preset: LayoutPreset) {
        let icons = self.window_layout.icons.clone();
        self.window_layout = windows::build_layout(
            preset,
            &self.panels,
            &self.i18n,
            ABOUT_CLOSE_KEY.into_node_key(),
        );
        self.window_layout.icons = icons;
        self.sync_all_window_titles();
    }

    fn sync_all_window_titles(&mut self) {
        self.panels
            .sync_window_titles(&mut self.window_layout, &self.i18n);
        windows::sync_tool_window_titles(&mut self.window_layout, &self.i18n);
        if let Some(meta) = self.window_layout.windows.get_mut(&WindowId::from("about")) {
            meta.title = self.i18n.t("window.about").to_string();
        }
    }

    /// Build one frame of the self-hosted editor UI and process interaction actions.
    pub fn build_and_step(
        &mut self,
        viewport_size: Vec2,
        input: &UiInput,
        dt: f32,
    ) -> Vec<UiAction> {
        self.last_viewport = viewport_size;
        self.tree.begin_frame();

        let theme = self.theme;
        let menu_state = self.menu.clone();
        let menus = self.menus();
        let workspace_top = theme.menu_bar_height + 37.0;
        let workspace = Rect::new(
            0.0,
            workspace_top,
            viewport_size.x,
            (viewport_size.y - workspace_top).max(0.0),
        );

        {
            let view_ctx = PanelViewCtx {
                doc: &self.doc,
                session: &self.session,
                theme: &theme,
                i18n: &self.i18n,
                icons: &self.icons,
                viewport_size,
            };
            let toolbar = editor_toolbar(
                &self.icons,
                &theme,
                self.session.gizmo_mode,
                self.session.playing,
                self.undo.can_undo(),
                self.undo.can_redo(),
            );
            let mut ui = UiContext::new(&mut self.tree);
            let root = ui.mount(
                col()
                    .size(Length::Px(viewport_size.x), Length::Px(viewport_size.y))
                    .bg(theme.bg),
            );
            ui.with_parent(root, |ui| {
                MenuBar::new(theme, viewport_size)
                    .icons(self.icons.clone())
                    .menus(menus)
                    .mount(ui, &menu_state);
            });
            let overlay = ui.tree.overlay_root(viewport_size);
            ui.with_parent(root, |ui| {
                ui.mount(
                    col()
                        .width(Length::Percent(100.0))
                        .grow(1.0)
                        .child(toolbar)
                        .child(divider(false)),
                );
                let i18n = &self.i18n;
                self.window_layout
                    .mount(ui, overlay, &theme, workspace, &mut |win_id, ui| {
                        if win_id.as_str() == "about" {
                            ui.mount(build_about_body_localized(&theme, i18n));
                        } else if !self.panels.mount_panel(win_id.as_str(), &view_ctx, ui) {
                            if let Some(body) =
                                windows::build_tool_window(win_id.as_str(), &theme, i18n)
                            {
                                ui.mount(body);
                            }
                        }
                    });
            });
        }

        {
            let _layout = prism_engine::util::timing::span("editor::ui_layout");
            self.tree.compute_layout(viewport_size);
        }
        let actions = {
            let _input = prism_engine::util::timing::span("editor::ui_input");
            self.tree.process_input(input)
        };
        if self.menu.open().is_none() {
            self.window_layout.step_input(workspace, input, &actions);
        }
        self.about_open = self.window_layout.is_window_open(&"about".into());
        self.desired_cursor =
            if self.menu.open().is_some() || self.window_layout.has_open_context_menu() {
                self.tree.desired_cursor()
            } else {
                let dock_cursor = self.window_layout.desired_cursor(input.pointer_pos);
                if dock_cursor != CursorHint::Default {
                    dock_cursor
                } else {
                    self.tree.desired_cursor()
                }
            };

        {
            let definitions = self.menus();
            self.menu.sync_hover(&self.tree, |id| {
                definitions
                    .iter()
                    .find(|menu| menu.id == id)
                    .map(|menu| menu.entries.clone())
            });
        }

        let menu_actions = self.menu.handle_actions(&actions);
        let mut cmds = menu_actions_to_commands(menu_actions);
        {
            let view_ctx = PanelViewCtx {
                doc: &self.doc,
                session: &self.session,
                theme: &self.theme,
                i18n: &self.i18n,
                icons: &self.icons,
                viewport_size,
            };
            for action in &actions {
                if let UiAction::Clicked(key) = action {
                    if *key == ABOUT_CLOSE_KEY.into_node_key() {
                        cmds.push(EditorCommand::CloseAbout);
                        continue;
                    }
                }
                if let Some(cmd) = toolbar_action_to_command(action) {
                    cmds.push(cmd);
                    continue;
                }
                self.panels.handle_action(action, &view_ctx, &mut cmds);
            }
        }
        self.apply_commands(cmds);

        self.has_active_animations = self.tree.step_animations(dt);
        {
            let mut draw = prism_engine::util::timing::span("editor::ui_draw_list");
            self.tree.default_text_shadow =
                Some((theme.text_shadow_offset, theme.text_shadow_color));
            self.tree.build_draw_list(&mut self.draw_list);
            draw.detail(format!("commands={}", self.draw_list.commands.len()));
        }
        actions
    }
}

fn sync_uploaded_icons(world: &prism_engine::ecs::World, state: &mut EditorUiState) {
    let Some(uploaded) = world.get_resource::<prism_engine::ui::UploadedUiImages>() else {
        return;
    };
    let Ok(slots) = uploaded.slots.lock() else {
        return;
    };
    for kind in IconKind::ALL {
        if state.icons.get(kind).is_none() {
            if let Some(&slot) = slots.get(kind.key()) {
                let tex = UiTextureId(slot);
                state.icons.insert(kind, tex);
                state.window_layout.icons.insert(kind, tex);
            }
        }
    }
}

fn frame_input_signature(screen: Vec2, input: &UiInput, state: &EditorUiState) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    screen.x.to_bits().hash(&mut h);
    screen.y.to_bits().hash(&mut h);
    input.pointer_pos.x.to_bits().hash(&mut h);
    input.pointer_pos.y.to_bits().hash(&mut h);
    input.primary_down.hash(&mut h);
    input.primary_just_pressed.hash(&mut h);
    input.primary_just_released.hash(&mut h);
    input.secondary_just_pressed.hash(&mut h);
    input.scroll_delta.x.to_bits().hash(&mut h);
    input.scroll_delta.y.to_bits().hash(&mut h);
    input.shift_held.hash(&mut h);
    input.ctrl_held.hash(&mut h);
    input.text_committed.hash(&mut h);
    input.backspace_pressed.hash(&mut h);
    input.escape_pressed.hash(&mut h);
    state.session.playing.hash(&mut h);
    state.icons.get(IconKind::Menu).is_some().hash(&mut h);
    h.finish()
}

fn editor_system(world: &mut prism_engine::ecs::World, dt: f32) {
    let _span = prism_engine::util::timing::span("editor::tick");
    let mut state = world.remove_resource::<EditorUiState>().unwrap_or_default();
    sync_uploaded_icons(world, &mut state);
    state.session.elapsed_time += dt as f64;
    let screen = bridge::screen_size(world);
    let input = bridge::read_engine_input(world, state.session.elapsed_time);
    let sig = frame_input_signature(screen, &input, &state);
    let can_skip = state.last_frame_sig == Some(sig)
        && !state.has_active_animations
        && !state.draw_list.commands.is_empty();
    if !can_skip {
        state.build_and_step(screen, &input, dt);
        state.last_frame_sig = Some(sig);
    }
    bridge::submit_draw_list(world, &state.draw_list);
    bridge::submit_cursor(world, state.desired_cursor);
    world.insert_resource(prism_engine::ui::HostUiActive(
        state.session.playing || state.has_active_animations || !can_skip,
    ));
    if state.exit_requested {
        bridge::request_exit(world);
    }
    world.insert_resource(state);
}

/// Build the editor application.
///
/// Uses [`prism_app::App::hosted`] rather than [`prism_app::app`]: the editor owns
/// its own documents and loads them through [`prism_app::App::load_scene_json_file`].
pub fn build_app() -> prism_app::App {
    let _span = prism_engine::util::timing::span("editor::build_app");
    let mut app = prism_app::App::hosted(prism_app::load_config())
        .with_pointer_lock_policy(prism_app::PointerLockPolicy::Manual)
        .with_idle_throttling(true)
        .with_subsystem(Subsystem::Render);
    {
        let _state = prism_engine::util::timing::span("editor::state_default");
        let world = app.engine_mut().world_mut();
        world.insert_resource(EditorUiState::default());

        let mut uploads = prism_engine::ui::PendingUiImageUploads::default();
        for kind in IconKind::ALL {
            uploads.items.push(prism_engine::ui::PendingUiImageUpload {
                key: kind.key().to_string(),
                width: 48,
                height: 48,
                pixels: rasterize_lucide_svg(kind.lucide_svg(), 48),
            });
        }
        world.insert_resource(uploads);
        world.insert_resource(prism_engine::ui::UploadedUiImages::default());
    }
    app.add_system("editor::tick", editor_system);
    app
}

#[cfg(test)]
mod tests;
