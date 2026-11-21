//! Layout and draw-order tests for the menu widgets.
//!
//! These assert real geometry rather than pixels: a dropdown that hangs in the
//! wrong place, or draws under a panel, is a defect the eye catches long before
//! any snapshot diff.

use glam::Vec2;

use super::*;
use crate::tree::{UiInput, UiTree};

/// Build one frame of a two-menu bar.
fn frame_with_open_menu(state: &mut MenuState, open: Option<&str>) -> (UiTree, Vec2) {
    reset_key_registry();
    if let Some(id) = open {
        state.open_menu(id);
    }
    let theme = Theme::dark();
    let viewport = Vec2::new(1280.0, 720.0);
    let mut tree = UiTree::new();
    tree.begin_frame();
    {
        let mut ctx = UiContext::new(&mut tree);
        let root = ctx.mount(
            crate::elements::col()
                .size(Length::Px(viewport.x), Length::Px(viewport.y))
                .bg(theme.bg),
        );
        let bar = MenuBar::new(theme, viewport).menus(vec![
            Menu::new("file", "File")
                .item(MenuEntry::item_with_shortcut("Save", "file.save", "Ctrl+S"))
                .item(MenuEntry::separator())
                .item(MenuEntry::item_with_shortcut("Exit", "file.exit", "Alt+F4")),
            Menu::new("edit", "Edit").item(MenuEntry::item_with_shortcut(
                "Undo",
                "edit.undo",
                "Ctrl+Z",
            )),
        ]);
        ctx.with_parent(root, |ctx| bar.mount(ctx, state));
    }
    tree.compute_layout(viewport);
    (tree, viewport)
}

fn rect_of(tree: &UiTree, key: NodeKey) -> crate::style::Rect {
    let id = tree.node_by_key(key).expect("node exists");
    tree.node(id).expect("node alive").computed_rect
}

#[test]
fn the_bar_sits_at_the_top_and_its_triggers_are_inside_it() {
    let mut state = MenuState::default();
    let (tree, viewport) = frame_with_open_menu(&mut state, None);

    let bar = rect_of(&tree, MENU_BAR_KEY.into_node_key());
    assert!(bar.y.abs() < 0.5, "menu bar must start at the top: {bar:?}");
    assert!(
        bar.w > 0.0 && bar.h > 0.0,
        "menu bar must have a size: {bar:?}"
    );
    assert!(bar.y + bar.h <= viewport.y);

    for id in ["file", "edit"] {
        let trigger = rect_of(&tree, MenuNodeKey::trigger_key(id));
        assert!(
            trigger.y >= bar.y - 0.5 && trigger.y + trigger.h <= bar.y + bar.h + 0.5,
            "trigger {id} must sit inside the bar: {trigger:?} vs {bar:?}"
        );
        assert!(trigger.w > 0.0, "trigger {id} must be visible: {trigger:?}");
    }

    let file = rect_of(&tree, MenuNodeKey::trigger_key("file"));
    let edit = rect_of(&tree, MenuNodeKey::trigger_key("edit"));
    assert!(file.x < edit.x, "File must come before Edit");
}

#[test]
fn an_open_menu_places_its_dropdown_under_the_trigger() {
    let mut state = MenuState::default();
    let (tree, viewport) = frame_with_open_menu(&mut state, Some("file"));

    let trigger = rect_of(&tree, MenuNodeKey::trigger_key("file"));
    let panel = rect_of(&tree, ("menu", "dropdown", "file").into_node_key());

    assert!(
        panel.w > 0.0 && panel.h > 0.0,
        "dropdown must have a size: {panel:?}"
    );
    assert!(
        (panel.y - (trigger.y + trigger.h)).abs() <= Theme::dark().menu_dropdown_offset + 0.5,
        "dropdown must hang under the trigger: panel={panel:?} trigger={trigger:?}"
    );
    assert!(
        (panel.x - trigger.x).abs() < 0.5,
        "dropdown must align with the trigger left edge"
    );
    assert!(
        panel.x + panel.w <= viewport.x,
        "dropdown must stay on screen"
    );
    assert!(
        panel.y + panel.h <= viewport.y,
        "dropdown must stay on screen"
    );

    let save = rect_of(&tree, MenuNodeKey::item_key("file", "file.save"));
    let exit = rect_of(&tree, MenuNodeKey::item_key("file", "file.exit"));
    assert!(
        save.y > panel.y,
        "first item must be inside the panel: panel={panel:?} save={save:?}"
    );
    assert!(exit.y > save.y, "items must stack downward");
    assert!(exit.y + exit.h <= panel.y + panel.h + 0.5);
}

#[test]
fn a_closed_menu_does_not_mount_its_dropdown() {
    let mut state = MenuState::default();
    let (tree, _) = frame_with_open_menu(&mut state, None);
    assert!(
        tree.node_by_key(("menu", "dropdown", "file").into_node_key())
            .is_none(),
        "a closed menu must not mount its dropdown"
    );
}

#[test]
fn the_dropdown_draws_above_the_bar() {
    let mut state = MenuState::default();
    let (tree, _) = frame_with_open_menu(&mut state, Some("file"));
    let mut draw = crate::draw::UiDrawList::default();
    tree.build_draw_list(&mut draw);

    let panel_text = draw
        .commands
        .iter()
        .position(|cmd| {
            matches!(
                cmd,
                crate::draw::UiDrawCommand::Text { content, .. } if content == "Save"
            )
        })
        .expect("menu item text must be drawn");
    let bar_text = draw
        .commands
        .iter()
        .position(|cmd| {
            matches!(
                cmd,
                crate::draw::UiDrawCommand::Text { content, .. } if content == "File"
            )
        })
        .expect("menu bar trigger text must be drawn");
    assert!(
        panel_text > bar_text,
        "dropdown content must be emitted after the bar"
    );
}

#[test]
fn a_click_inside_the_dropdown_hits_an_item() {
    let mut state = MenuState::default();
    let (tree, _) = frame_with_open_menu(&mut state, Some("file"));
    let save = rect_of(&tree, MenuNodeKey::item_key("file", "file.save"));
    let point = Vec2::new(save.x + save.w * 0.5, save.y + save.h * 0.5);
    let hit = tree
        .hit_test(point)
        .expect("a point inside an item must hit");
    let key = tree.node(hit).expect("hit node alive").key;
    assert_eq!(key, MenuNodeKey::item_key("file", "file.save"));
}
#[test]
fn hovering_an_item_marks_it_hovered_for_the_next_frame() {
    let mut state = MenuState::default();
    let (mut tree, _) = frame_with_open_menu(&mut state, Some("file"));

    let save = rect_of(&tree, MenuNodeKey::item_key("file", "file.save"));
    let point = Vec2::new(save.x + save.w * 0.5, save.y + save.h * 0.5);
    tree.process_input(&UiInput {
        pointer_pos: point,
        ..Default::default()
    });

    assert!(
        tree.is_hovered(MenuNodeKey::item_key("file", "file.save")),
        "the item under the pointer must report hover"
    );
    assert!(
        !tree.is_hovered(MenuNodeKey::item_key("file", "file.exit")),
        "a sibling must not report hover"
    );
}

#[test]
fn a_hovered_submenu_row_opens_its_panel() {
    // Multi-level behaviour is covered in `menu_nested_tests`; this keeps the
    // single-level contract: hovering the row mounts exactly one panel beside it.
    let theme = Theme::dark();
    let viewport = Vec2::new(1280.0, 720.0);
    let mut state = MenuState::default();
    state.open_menu("file");
    reset_key_registry();

    let menus = vec![Menu::new("file", "File").items(vec![
        MenuEntry::item("Save", "file.save"),
        MenuEntry::submenu("Export", vec![MenuEntry::item("glTF", "file.export.gltf")]),
    ])];

    let mut tree = UiTree::new();
    let mut mount = |tree: &mut UiTree, state: &MenuState| {
        tree.begin_frame();
        {
            let mut ctx = UiContext::new(tree);
            let root = ctx.mount(
                crate::elements::col()
                    .size(Length::Px(viewport.x), Length::Px(viewport.y))
                    .bg(theme.bg),
            );
            ctx.with_parent(root, |ctx| {
                MenuBar::new(theme, viewport)
                    .menus(menus.clone())
                    .mount(ctx, state);
            });
        }
        tree.compute_layout(viewport);
    };

    mount(&mut tree, &state);

    let row_key = ("menu", "submenu", "file", "Export").into_node_key();
    let row = rect_of(&tree, row_key);
    tree.process_input(&UiInput {
        pointer_pos: Vec2::new(row.x + row.w * 0.5, row.y + row.h * 0.5),
        ..Default::default()
    });
    assert!(tree.is_hovered(row_key), "submenu row must be hovered");

    // Resolve after input, while this frame's nodes still exist.
    state.sync_hover(&tree, |id| {
        menus
            .iter()
            .find(|menu| menu.id == id)
            .map(|menu| menu.entries.clone())
    });
    assert_eq!(state.submenu_branch(), ["Export".to_string()]);

    mount(&mut tree, &state);
    let sub_panel = rect_of(&tree, ("menu", "dropdown", "file/Export").into_node_key());
    assert!(
        sub_panel.w > 0.0 && sub_panel.h > 0.0,
        "hovering the submenu row must mount its panel: {sub_panel:?}"
    );
}
