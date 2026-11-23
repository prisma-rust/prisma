//! Declarative top-level menu bar definition and pure command mapping.

use prism_ui::prelude::*;

use crate::commands::{EditorCommand, LayoutPreset, UndoStack};
use crate::panels::{PanelRegistry, PanelViewCtx};
use crate::windows::TOOL_WINDOWS;

/// Build the editor's top-level menu bar from read-only state and registered panels.
pub fn build_menus(
    ctx: &PanelViewCtx,
    panels: &PanelRegistry,
    layout: &WindowLayout,
    undo: &UndoStack,
) -> Vec<Menu> {
    let playing = ctx.session.playing;
    let can_undo = undo.can_undo();
    let can_redo = undo.can_redo();
    let mut panel_entries: Vec<MenuEntry> = panels
        .iter()
        .map(|p| {
            let win_id = WindowId::from(p.id());
            MenuEntry::checkbox(
                ctx.i18n.t(p.title_key()),
                format!("view.panel.{}", p.id()),
                layout.is_window_open(&win_id),
            )
        })
        .collect();
    for (id, title_key) in TOOL_WINDOWS {
        panel_entries.push(MenuEntry::checkbox(
            ctx.i18n.t(title_key),
            format!("view.panel.{id}"),
            layout.is_window_open(&WindowId::from(*id)),
        ));
    }
    panel_entries.push(MenuEntry::separator());
    panel_entries.push(MenuEntry::submenu(
        "Layouts",
        vec![
            MenuEntry::item("Default", "view.layout.default"),
            MenuEntry::item("Wide Viewport", "view.layout.wide"),
            MenuEntry::item("Tall Panels", "view.layout.tall"),
            MenuEntry::separator(),
            MenuEntry::submenu(
                "Saved",
                vec![
                    MenuEntry::item("My Layout", "view.layout.saved.mine"),
                    MenuEntry::item("Manage...", "view.layout.saved.manage"),
                ],
            ),
        ],
    ));

    let mut window_menu = Menu::new("window", ctx.i18n.t("menu.window"));
    for p in panels.iter() {
        let win_id = WindowId::from(p.id());
        window_menu = window_menu.item(MenuEntry::checkbox(
            ctx.i18n.t(p.title_key()),
            format!("window.{}", p.id()),
            layout.is_window_open(&win_id),
        ));
    }
    for (id, title_key) in [("console", "window.console"), ("game", "window.game")] {
        window_menu = window_menu.item(MenuEntry::checkbox(
            ctx.i18n.t(title_key),
            format!("window.{id}"),
            layout.is_window_open(&WindowId::from(id)),
        ));
    }
    window_menu = window_menu
        .item(MenuEntry::separator())
        .item(MenuEntry::submenu(
            "Analysis & Rendering",
            ["profiler", "lighting", "audio_mixer", "animation"]
                .iter()
                .map(|id| {
                    MenuEntry::checkbox(
                        ctx.i18n.t(&format!("window.{id}")),
                        format!("window.{id}"),
                        layout.is_window_open(&WindowId::from(*id)),
                    )
                })
                .collect(),
        ))
        .item(MenuEntry::submenu(
            "Project & Packages",
            ["package_manager", "project_settings", "build_settings"]
                .iter()
                .map(|id| {
                    MenuEntry::checkbox(
                        ctx.i18n.t(&format!("window.{id}")),
                        format!("window.{id}"),
                        layout.is_window_open(&WindowId::from(*id)),
                    )
                })
                .collect(),
        ))
        .item(MenuEntry::separator())
        .item(MenuEntry::item(
            ctx.i18n.t("menu.reset_layout"),
            "window.reset_layout",
        ));
    vec![
        Menu::new("file", "File")
            .item(MenuEntry::submenu(
                "New",
                vec![
                    MenuEntry::item_with_shortcut("Scene", "file.new.scene", "Ctrl+N"),
                    MenuEntry::item_with_shortcut("Prefab", "file.new.prefab", ""),
                    MenuEntry::separator(),
                    MenuEntry::submenu(
                        "From Template",
                        vec![
                            MenuEntry::item("Empty 3D", "file.new.template.empty3d"),
                            MenuEntry::item("Empty 2D", "file.new.template.empty2d"),
                            MenuEntry::separator(),
                            MenuEntry::submenu(
                                "Samples",
                                vec![
                                    MenuEntry::item("Spinning Cube", "file.new.sample.cube"),
                                    MenuEntry::item("Voxel World", "file.new.sample.voxel"),
                                    MenuEntry::item("Sprite Playground", "file.new.sample.sprites"),
                                ],
                            ),
                        ],
                    ),
                ],
            ))
            .item(MenuEntry::submenu(
                "Open Recent",
                vec![
                    MenuEntry::item("scenes/main.scene.json", "file.recent.main"),
                    MenuEntry::item("scenes/arena.scene.json", "file.recent.arena"),
                    MenuEntry::separator(),
                    MenuEntry::item("Clear List", "file.recent.clear"),
                ],
            ))
            .item(MenuEntry::item_with_shortcut(
                "Open Scene",
                "file.open",
                "Ctrl+O",
            ))
            .item(MenuEntry::separator())
            .item(MenuEntry::item_with_shortcut("Save", "file.save", "Ctrl+S"))
            .item(MenuEntry::item_with_shortcut(
                "Save As",
                "file.save_as",
                "Ctrl+Shift+S",
            ))
            .item(MenuEntry::separator())
            .item(MenuEntry::submenu(
                "Build",
                vec![
                    MenuEntry::item("Scene Package", "file.build.scene"),
                    MenuEntry::item("Player", "file.build.player"),
                    MenuEntry::separator(),
                    MenuEntry::submenu(
                        "Configuration",
                        vec![
                            MenuEntry::item("Debug", "file.build.config.debug"),
                            MenuEntry::item("Release", "file.build.config.release"),
                            MenuEntry::separator(),
                            MenuEntry::submenu(
                                "Advanced",
                                vec![
                                    MenuEntry::checkbox("Strip Symbols", "file.build.strip", true),
                                    MenuEntry::checkbox("LTO", "file.build.lto", false),
                                    MenuEntry::item("Edit Profiles...", "file.build.profiles"),
                                ],
                            ),
                        ],
                    ),
                ],
            ))
            .item(MenuEntry::separator())
            .item(MenuEntry::item_with_shortcut("Exit", "file.exit", "Alt+F4")),
        Menu::new("edit", "Edit")
            .item(undo_entry("Undo", "edit.undo", "Ctrl+Z", can_undo))
            .item(undo_entry("Redo", "edit.redo", "Ctrl+Y", can_redo))
            .item(MenuEntry::separator())
            .item(MenuEntry::item_with_shortcut(
                "Duplicate",
                "edit.duplicate",
                "Ctrl+D",
            ))
            .item(MenuEntry::item_with_shortcut(
                "Delete",
                "edit.delete",
                "Del",
            ))
            .item(MenuEntry::separator())
            .item(MenuEntry::item_with_shortcut(
                "Select All",
                "edit.select_all",
                "Ctrl+A",
            )),
        Menu::new("view", "View")
            .item(MenuEntry::submenu("Panels", panel_entries))
            .item(MenuEntry::checkbox(
                "Show Grid",
                "view.grid",
                ctx.session.show_grid,
            ))
            .item(MenuEntry::checkbox(
                "Show Gizmos",
                "view.gizmos",
                ctx.session.show_gizmos,
            ))
            .item(MenuEntry::separator())
            .item(MenuEntry::submenu(
                ctx.i18n.t("menu.theme"),
                ThemePreset::ALL
                    .iter()
                    .map(|preset| {
                        MenuEntry::checkbox(
                            preset.label(),
                            format!("view.theme.{}", preset.id()),
                            ctx.theme.preset() == *preset,
                        )
                    })
                    .collect(),
            ))
            .item(MenuEntry::submenu(
                ctx.i18n.t("menu.language"),
                Locale::ALL
                    .iter()
                    .map(|loc| {
                        MenuEntry::checkbox(
                            loc.label(),
                            format!("view.lang.{}", loc.id()),
                            ctx.i18n.locale == *loc,
                        )
                    })
                    .collect(),
            ))
            .item(MenuEntry::separator())
            .item(MenuEntry::item_with_shortcut(
                ctx.i18n.t("menu.reset_layout"),
                "view.reset_layout",
                "",
            )),
        Menu::new("scene", ctx.i18n.t("menu.scene"))
            .item(MenuEntry::item_with_shortcut(
                if playing {
                    ctx.i18n.t("toolbar.stop")
                } else {
                    ctx.i18n.t("toolbar.play")
                },
                "scene.toggle_play",
                "Ctrl+P",
            ))
            .item(MenuEntry::separator())
            .item(MenuEntry::item_with_shortcut(
                "Gizmo: Translate",
                "scene.gizmo_translate",
                "W",
            ))
            .item(MenuEntry::item_with_shortcut(
                "Gizmo: Rotate",
                "scene.gizmo_rotate",
                "E",
            ))
            .item(MenuEntry::item_with_shortcut(
                "Gizmo: Scale",
                "scene.gizmo_scale",
                "R",
            )),
        window_menu,
        Menu::new("help", ctx.i18n.t("menu.help"))
            .item(MenuEntry::item("Documentation", "help.docs"))
            .item(MenuEntry::item(ctx.i18n.t("menu.about"), "help.about")),
    ]
}

/// Translate [`MenuAction`]s into [`EditorCommand`]s without mutating editor state directly.
pub fn menu_actions_to_commands(actions: Vec<MenuAction>) -> Vec<EditorCommand> {
    let mut cmds = Vec::new();
    for action in actions {
        match action {
            MenuAction::Command(command) => match command.as_str() {
                "help.about" => cmds.push(EditorCommand::OpenAbout),
                "window.reset_layout" | "view.reset_layout" | "view.layout.default" => {
                    cmds.push(EditorCommand::ResetLayout);
                }
                "view.layout.wide" => cmds.push(EditorCommand::SetLayout(LayoutPreset::Wide)),
                "view.layout.tall" => cmds.push(EditorCommand::SetLayout(LayoutPreset::Tall)),
                "file.exit" => cmds.push(EditorCommand::RequestExit),
                "file.build" | "file.build.profiles" => {
                    cmds.push(EditorCommand::ToggleWindow(WindowId::from(
                        "build_settings",
                    )));
                }
                "edit.select_all" => cmds.push(EditorCommand::SelectAll),
                "edit.duplicate" => cmds.push(EditorCommand::DuplicateSelected),
                "edit.delete" => cmds.push(EditorCommand::DeleteSelected),
                cmd_str if cmd_str.starts_with("window.") => {
                    let id = &cmd_str["window.".len()..];
                    cmds.push(EditorCommand::ToggleWindow(WindowId::from(id)));
                }
                cmd_str if cmd_str.starts_with("view.panel.") => {
                    let id = &cmd_str["view.panel.".len()..];
                    cmds.push(EditorCommand::ToggleWindow(WindowId::from(id)));
                }
                "view.lang.en-US" => cmds.push(EditorCommand::SetLocale(Locale::EnUs)),
                "view.lang.zh-CN" => cmds.push(EditorCommand::SetLocale(Locale::ZhCn)),
                "view.grid" => cmds.push(EditorCommand::ToggleGrid),
                "view.gizmos" => cmds.push(EditorCommand::ToggleGizmos),
                cmd_str if cmd_str.starts_with("view.theme.") => {
                    let id = &cmd_str["view.theme.".len()..];
                    if let Some(preset) = ThemePreset::ALL.iter().find(|p| p.id() == id) {
                        cmds.push(EditorCommand::SetTheme(*preset));
                    }
                }
                "scene.toggle_play" => cmds.push(EditorCommand::TogglePlay),
                "scene.gizmo_translate" => {
                    cmds.push(EditorCommand::SetGizmoMode(
                        crate::state::GizmoMode::Translate,
                    ));
                }
                "scene.gizmo_rotate" => {
                    cmds.push(EditorCommand::SetGizmoMode(crate::state::GizmoMode::Rotate));
                }
                "scene.gizmo_scale" => {
                    cmds.push(EditorCommand::SetGizmoMode(crate::state::GizmoMode::Scale));
                }
                "edit.undo" => cmds.push(EditorCommand::Undo),
                "edit.redo" => cmds.push(EditorCommand::Redo),
                other => log::debug!("editor: unhandled menu command '{other}'"),
            },
            MenuAction::Toggled { command, .. } => {
                log::debug!("editor: menu toggle '{command}'");
            }
            MenuAction::OpenChanged(open) => {
                log::trace!("editor: open menu {open:?}");
            }
        }
    }
    cmds
}

fn undo_entry(label: &str, command: &str, shortcut: &str, enabled: bool) -> MenuEntry {
    let entry = MenuEntry::item_with_shortcut(label, command, shortcut);
    if enabled {
        entry
    } else {
        entry.disabled()
    }
}
