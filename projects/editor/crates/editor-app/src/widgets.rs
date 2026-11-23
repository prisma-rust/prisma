//! Reusable editor UI building blocks that consume [`Theme`] and [`IconSet`] tokens.
//!
//! Panels compose these widgets instead of hand-coding spacing, colors, or borders.

use prism_ui::prelude::*;

use crate::commands::EditorCommand;
use crate::state::GizmoMode;

/// Standard panel container with surface background, padding, and muted uppercase header.
pub fn panel_shell(header: impl Into<String>, pad: f32, gap: f32) -> ElementBox {
    col()
        .size(Length::Percent(100.0), Length::Percent(100.0))
        .pad(pad)
        .gap(gap)
        .bg(UiPalette::SURFACE_1)
        .child(muted_label(header))
}

/// Standard filter/search text input used across Hierarchy, Assets, and Console panels.
pub fn search_bar(
    key: impl IntoNodeKey,
    query: impl Into<String>,
    placeholder: impl Into<String>,
) -> ElementBox {
    text_field(key, query, placeholder)
}

/// Standard selectable row for an entity in the scene hierarchy.
pub fn entity_row(id: u32, name: impl Into<String>, selected: bool) -> ElementBox {
    toggle_button(("entity", id), name, selected)
}

/// Collapsible section card with a toggleable header and surface border.
pub fn collapsible_section(
    key: impl IntoNodeKey,
    title: impl Into<String>,
    expanded: bool,
    children: Vec<ElementBox>,
) -> ElementBox {
    let prefix = if expanded { "v " } else { "> " };
    let mut section = col()
        .pad(8.0)
        .gap(6.0)
        .bg(UiPalette::SURFACE_2)
        .border(1.0, UiPalette::BORDER)
        .radius(4.0)
        .child(toggle_button(
            key,
            format!("{prefix}{}", title.into()),
            expanded,
        ));
    if expanded {
        for child in children {
            section = section.child(child);
        }
    }
    section
}

/// Compact asset reference slot showing the assigned path or `"<None>"`.
pub fn asset_picker(
    key: impl IntoNodeKey,
    field_label: impl Into<String>,
    current_asset: Option<&str>,
    theme: &Theme,
) -> ElementBox {
    let display = current_asset.filter(|s| !s.is_empty()).unwrap_or("<None>");
    row()
        .gap(8.0)
        .align(Align::Center)
        .child(muted_label(field_label).width(Length::Px(120.0)))
        .child(
            button(key, display)
                .grow(1.0)
                .bg(theme.panel_alt)
                .border(1.0, theme.border),
        )
}

/// Modal confirmation card with Confirm and Cancel action buttons.
pub fn confirm_dialog(
    key_prefix: &str,
    title: impl Into<String>,
    message: impl Into<String>,
    confirm_label: impl Into<String>,
    cancel_label: impl Into<String>,
    theme: &Theme,
) -> ElementBox {
    col()
        .pad(14.0)
        .gap(10.0)
        .bg(theme.panel)
        .border(1.0, theme.border)
        .radius(8.0)
        .child(label(title))
        .child(muted_label(message))
        .child(
            row()
                .gap(8.0)
                .justify(Justify::End)
                .child(button((key_prefix, "cancel").into_node_key(), cancel_label))
                .child(
                    button((key_prefix, "confirm").into_node_key(), confirm_label).bg(theme.accent),
                ),
        )
}

/// Icon + label button styled for the editor top toolbar.
pub fn toolbar_button_icon(
    icons: &IconSet,
    theme: &Theme,
    key: impl IntoNodeKey,
    icon: IconKind,
    label_text: &str,
) -> ElementBox {
    row()
        .key(key)
        .padding(Edges::symmetric(9.0, 5.0))
        .gap(5.0)
        .align(Align::Center)
        .justify(Justify::Center)
        .bg(theme.panel_alt)
        .hover_bg(theme.row_hover)
        .border(1.0, theme.border)
        .focus_border(theme.accent)
        .radius(6.0)
        .focusable(0)
        .clickable()
        .child(icon_view(icons, icon, 13.0, theme.text))
        .child(prism_ui::elements::text(
            theme.text,
            theme.font_size,
            label_text,
        ))
}

/// Icon + label toggle button styled for the editor top toolbar.
pub fn toolbar_toggle_icon(
    icons: &IconSet,
    theme: &Theme,
    key: impl IntoNodeKey,
    icon: IconKind,
    label_text: &str,
    active: bool,
) -> ElementBox {
    let base_bg = if active {
        theme.accent
    } else {
        theme.panel_alt
    };
    let fg = if active { theme.on_accent } else { theme.text };
    row()
        .key(key)
        .padding(Edges::symmetric(9.0, 5.0))
        .gap(5.0)
        .align(Align::Center)
        .justify(Justify::Center)
        .bg(base_bg)
        .hover_bg(if active {
            theme.accent
        } else {
            theme.row_hover
        })
        .border(1.0, if active { theme.accent } else { theme.border })
        .focus_border(theme.accent)
        .radius(6.0)
        .focusable(0)
        .toggleable(active)
        .child(icon_view(icons, icon, 13.0, fg))
        .child(prism_ui::elements::text(fg, theme.font_size, label_text))
}

/// Build the top editor toolbar (gizmo selectors, play toggle, undo/redo).
pub fn editor_toolbar(
    icons: &IconSet,
    theme: &Theme,
    gizmo: GizmoMode,
    playing: bool,
    can_undo: bool,
    can_redo: bool,
) -> ElementBox {
    row()
        .height(Length::Px(36.0))
        .padding(Edges::symmetric(10.0, 4.0))
        .gap(6.0)
        .align(Align::Center)
        .bg(theme.panel)
        .child(prism_ui::elements::text(
            theme.text_strong,
            theme.font_size,
            "Prisma Editor",
        ))
        .child(divider(true))
        .child(toolbar_toggle_icon(
            icons,
            theme,
            "gizmo:translate",
            IconKind::Move,
            "W",
            gizmo == GizmoMode::Translate,
        ))
        .child(toolbar_toggle_icon(
            icons,
            theme,
            "gizmo:rotate",
            IconKind::Rotate,
            "E",
            gizmo == GizmoMode::Rotate,
        ))
        .child(toolbar_toggle_icon(
            icons,
            theme,
            "gizmo:scale",
            IconKind::Scale,
            "R",
            gizmo == GizmoMode::Scale,
        ))
        .child(spacer())
        .child(toolbar_toggle_icon(
            icons,
            theme,
            "toolbar:play",
            if playing {
                IconKind::Stop
            } else {
                IconKind::Play
            },
            if playing { "Stop" } else { "Play" },
            playing,
        ))
        .child(toolbar_button_icon(
            icons,
            theme,
            "toolbar:undo",
            IconKind::Undo,
            if can_undo { "Undo" } else { "Undo (0)" },
        ))
        .child(toolbar_button_icon(
            icons,
            theme,
            "toolbar:redo",
            IconKind::Redo,
            if can_redo { "Redo" } else { "Redo (0)" },
        ))
}

/// Translate toolbar widget actions into [`EditorCommand`]s.
pub fn toolbar_action_to_command(action: &UiAction) -> Option<EditorCommand> {
    match action {
        UiAction::Toggled { key, value } => {
            if *key == "toolbar:play".into_node_key() {
                Some(EditorCommand::SetPlaying(*value))
            } else if *key == "gizmo:translate".into_node_key() {
                Some(EditorCommand::SetGizmoMode(GizmoMode::Translate))
            } else if *key == "gizmo:rotate".into_node_key() {
                Some(EditorCommand::SetGizmoMode(GizmoMode::Rotate))
            } else if *key == "gizmo:scale".into_node_key() {
                Some(EditorCommand::SetGizmoMode(GizmoMode::Scale))
            } else {
                None
            }
        }
        UiAction::Clicked(key) => {
            if *key == "toolbar:undo".into_node_key() {
                Some(EditorCommand::Undo)
            } else if *key == "toolbar:redo".into_node_key() {
                Some(EditorCommand::Redo)
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec2;

    use super::*;

    #[test]
    fn collapsible_section_hides_children_when_collapsed() {
        let mut tree = UiTree::new();
        tree.begin_frame();
        {
            let mut ui = UiContext::new(&mut tree);
            ui.mount(collapsible_section(
                "sec:test",
                "Transform",
                false,
                vec![label("hidden_child")],
            ));
        }
        tree.compute_layout(Vec2::new(300.0, 200.0));
        let mut list = UiDrawList::default();
        tree.build_draw_list(&mut list);
        assert!(list
            .commands
            .iter()
            .any(|c| matches!(c, UiDrawCommand::Text { content, .. } if content == "> Transform")));
        assert!(!list.commands.iter().any(
            |c| matches!(c, UiDrawCommand::Text { content, .. } if content == "hidden_child")
        ));
    }

    #[test]
    fn asset_picker_and_confirm_dialog_mount_expected_keys() {
        let theme = Theme::dark();
        let mut tree = UiTree::new();
        tree.begin_frame();
        {
            let mut ui = UiContext::new(&mut tree);
            ui.mount(
                col()
                    .child(asset_picker("pick:mesh", "Mesh", None, &theme))
                    .child(confirm_dialog(
                        "del",
                        "Delete Entity",
                        "Are you sure?",
                        "Delete",
                        "Cancel",
                        &theme,
                    )),
            );
        }
        tree.compute_layout(Vec2::new(400.0, 300.0));
        assert!(tree.node_by_key("pick:mesh".into_node_key()).is_some());
        assert!(tree
            .node_by_key(("del", "confirm").into_node_key())
            .is_some());
        assert!(tree
            .node_by_key(("del", "cancel").into_node_key())
            .is_some());
    }
}
