//! Reproduces the reported defect: a submenu panel covers the whole window.

use glam::Vec2;

use super::*;
use crate::tree::{UiInput, UiTree};

fn frame(tree: &mut UiTree, state: &MenuState, menus: Vec<Menu>, theme: Theme, viewport: Vec2) {
    tree.begin_frame();
    {
        let mut ctx = UiContext::new(tree);
        let root = ctx.mount(
            crate::elements::col()
                .size(Length::Px(viewport.x), Length::Px(viewport.y))
                .bg(theme.bg),
        );
        ctx.with_parent(root, |ctx| {
            MenuBar::new(theme, viewport).menus(menus).mount(ctx, state);
        });
    }
    tree.compute_layout(viewport);
}

#[test]
fn a_submenu_panel_is_not_full_screen() {
    let theme = Theme::dark();
    let viewport = Vec2::new(1280.0, 720.0);
    let mut state = MenuState::default();
    state.open_menu("file");

    let menus = || {
        vec![Menu::new("file", "File").items(vec![
            MenuEntry::item("Save", "file.save"),
            MenuEntry::Submenu {
                label: "Export".into(),
                entries: vec![MenuEntry::item("glTF", "file.export.gltf")],
            },
        ])]
    };

    let mut tree = UiTree::new();
    frame(&mut tree, &state, menus(), theme, viewport);

    // Hover the submenu row.
    let row_key = ("menu", "submenu", "file", "Export").into_node_key();
    let row_id = tree.node_by_key(row_key).expect("submenu row exists");
    let row = tree.node(row_id).unwrap().computed_rect;
    let point = Vec2::new(row.x + row.w * 0.5, row.y + row.h * 0.5);
    tree.process_input(&UiInput {
        pointer_pos: point,
        ..Default::default()
    });

    // Next frame mounts the submenu panel.
    frame(&mut tree, &state, menus(), theme, viewport);

    let panel_key = ("menu", "dropdown", "file/Export").into_node_key();
    let panel_id = tree.node_by_key(panel_key).expect("submenu panel mounted");
    let panel = tree.node(panel_id).unwrap().computed_rect;

    assert!(
        panel.h < viewport.y * 0.5,
        "a submenu panel must size to its items, not the window: {panel:?}"
    );
    assert!(
        panel.w < viewport.x * 0.5,
        "a submenu panel must size to its items, not the window: {panel:?}"
    );
}

#[test]
fn a_dropdown_panel_is_not_full_screen() {
    let theme = Theme::dark();
    let viewport = Vec2::new(1280.0, 720.0);
    let mut state = MenuState::default();
    state.open_menu("file");
    let menus = vec![Menu::new("file", "File")
        .item(MenuEntry::item_with_shortcut("Save", "file.save", "Ctrl+S"))
        .item(MenuEntry::separator())
        .item(MenuEntry::item_with_shortcut("Exit", "file.exit", "Alt+F4"))];

    let mut tree = UiTree::new();
    frame(&mut tree, &state, menus, theme, viewport);

    let panel_id = tree
        .node_by_key(("menu", "dropdown", "file").into_node_key())
        .expect("dropdown mounted");
    let panel = tree.node(panel_id).unwrap().computed_rect;
    assert!(
        panel.h < viewport.y * 0.5,
        "a dropdown must size to its items, not the window: {panel:?}"
    );
}
