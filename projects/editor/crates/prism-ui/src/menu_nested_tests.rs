//! Geometry of multi-level menus: each nested panel opens beside its row.
//!
//! Depth is a layout concern, so these assert rects rather than pixels: a
//! third-level panel that lands on top of its parent is unusable no matter how
//! it is coloured.

use glam::Vec2;

use super::*;
use crate::tree::{UiInput, UiTree};

/// A four-level chain: File > New > From Template > Samples.
fn deep_menus() -> Vec<Menu> {
    vec![Menu::new("file", "File").items(vec![
        MenuEntry::item("Save", "file.save"),
        MenuEntry::submenu(
            "New",
            vec![
                MenuEntry::item("Scene", "file.new.scene"),
                MenuEntry::submenu(
                    "From Template",
                    vec![
                        MenuEntry::item("Empty 3D", "file.new.tpl.empty3d"),
                        MenuEntry::submenu(
                            "Samples",
                            vec![
                                MenuEntry::item("Cube", "file.new.sample.cube"),
                                MenuEntry::item("Voxel", "file.new.sample.voxel"),
                            ],
                        ),
                    ],
                ),
            ],
        ),
    ])]
}

fn build(tree: &mut UiTree, state: &MenuState, viewport: Vec2) {
    let theme = Theme::dark();
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
                .menus(deep_menus())
                .mount(ctx, state);
        });
    }
    tree.compute_layout(viewport);
}

/// Advance one frame with no hover: rebuild so pending state lands on screen.
fn settle(tree: &mut UiTree, state: &mut MenuState, viewport: Vec2) {
    build(tree, state, viewport);
}

/// Hover a row, resolve the branch, then rebuild — the sequence a real frame runs.
fn hover_frame(tree: &mut UiTree, state: &mut MenuState, viewport: Vec2, scope: &str, label: &str) {
    let key = submenu_row_key(scope, label);
    let id = tree.node_by_key(key).expect("submenu row exists");
    let rect = tree.node(id).unwrap().computed_rect;
    tree.process_input(&UiInput {
        pointer_pos: Vec2::new(rect.x + rect.w * 0.5, rect.y + rect.h * 0.5),
        ..Default::default()
    });
    let menus = deep_menus();
    state.sync_hover(tree, |id| {
        menus
            .iter()
            .find(|menu| menu.id == id)
            .map(|menu| menu.entries.clone())
    });
    build(tree, state, viewport);
}

/// Hover a plain item inside a panel and resolve, without rebuilding.
fn hover_item(tree: &mut UiTree, state: &mut MenuState, scope: &str, command: &str) {
    let key = MenuNodeKey::item_key(scope, command);
    let id = tree.node_by_key(key).expect("item exists");
    let rect = tree.node(id).unwrap().computed_rect;
    tree.process_input(&UiInput {
        pointer_pos: Vec2::new(rect.x + rect.w * 0.5, rect.y + rect.h * 0.5),
        ..Default::default()
    });
    let menus = deep_menus();
    state.sync_hover(tree, |id| {
        menus
            .iter()
            .find(|menu| menu.id == id)
            .map(|menu| menu.entries.clone())
    });
}

fn rect(tree: &UiTree, key: NodeKey) -> crate::style::Rect {
    let id = tree.node_by_key(key).expect("node exists");
    tree.node(id).unwrap().computed_rect
}

fn panel(scope: &str) -> NodeKey {
    ("menu", "dropdown", scope).into_node_key()
}

#[test]
fn nested_panels_open_to_the_right_of_their_row() {
    let viewport = Vec2::new(1280.0, 720.0);
    let mut state = MenuState::default();
    state.open_menu("file");
    let mut tree = UiTree::new();

    settle(&mut tree, &mut state, viewport);
    assert!(tree.node_by_key(panel("file")).is_some());

    // Level 2: "New" row spawns a panel whose first item must sit level with it.
    hover_frame(&mut tree, &mut state, viewport, "file", "New");
    let row2 = rect(&tree, submenu_row_key("file", "New"));
    let l2 = rect(&tree, panel("file/New"));
    assert!(
        l2.x >= row2.x + row2.w - 0.5,
        "level 2 must open at its row's right edge: panel={l2:?} row={row2:?}"
    );
    assert!(
        (l2.y - row2.y).abs() < 0.5,
        "level 2's top must sit on its row's top: panel={l2:?} row={row2:?}"
    );

    // Level 3.
    hover_frame(&mut tree, &mut state, viewport, "file/New", "From Template");
    let row3 = rect(&tree, submenu_row_key("file/New", "From Template"));
    let l3 = rect(&tree, panel("file/New/From Template"));
    assert!(
        l3.x >= row3.x + row3.w - 0.5,
        "level 3 must open at its row's right edge: panel={l3:?} row={row3:?}"
    );
    assert!(
        (l3.y - row3.y).abs() < 0.5,
        "level 3's top must sit on its row's top: panel={l3:?} row={row3:?}"
    );

    // Level 4.
    hover_frame(
        &mut tree,
        &mut state,
        viewport,
        "file/New/From Template",
        "Samples",
    );
    let row4 = rect(&tree, submenu_row_key("file/New/From Template", "Samples"));
    let l4 = rect(&tree, panel("file/New/From Template/Samples"));
    assert!(
        l4.x >= row4.x + row4.w - 0.5,
        "level 4 must open at its row's right edge: panel={l4:?} row={row4:?}"
    );
    assert!(
        (l4.y - row4.y).abs() < 0.5,
        "level 4's top must sit on its row's top: panel={l4:?} row={row4:?}"
    );

    for (name, p) in [("l2", l2), ("l3", l3), ("l4", l4)] {
        assert!(
            p.x >= 0.0 && p.x + p.w <= viewport.x + 0.5,
            "{name} must stay on screen: {p:?}"
        );
        assert!(
            p.h < viewport.y * 0.5,
            "{name} must size to its items: {p:?}"
        );
    }
}

#[test]
fn a_panel_that_would_leave_the_window_flips_left() {
    let viewport = Vec2::new(420.0, 720.0);
    let mut state = MenuState::default();
    state.open_menu("file");
    let mut tree = UiTree::new();

    settle(&mut tree, &mut state, viewport);
    hover_frame(&mut tree, &mut state, viewport, "file", "New");
    hover_frame(&mut tree, &mut state, viewport, "file/New", "From Template");

    let l3 = rect(&tree, panel("file/New/From Template"));
    assert!(
        l3.x + l3.w <= viewport.x + 0.5,
        "a panel that would overflow must flip: {l3:?}"
    );
}

#[test]
fn hovering_a_deeper_item_keeps_the_branch_open() {
    let viewport = Vec2::new(1280.0, 720.0);
    let mut state = MenuState::default();
    state.open_menu("file");
    let mut tree = UiTree::new();

    settle(&mut tree, &mut state, viewport);
    hover_frame(&mut tree, &mut state, viewport, "file", "New");

    // The pointer moves onto an item inside the level-2 panel; the branch must
    // survive or the panel would vanish the moment the pointer left its row.
    hover_item(&mut tree, &mut state, "file/New", "file.new.scene");
    assert_eq!(
        state.submenu_branch(),
        ["New".to_string()],
        "hovering a child item must keep its panel open"
    );
}

#[test]
fn hovering_a_plain_row_closes_the_branch() {
    let viewport = Vec2::new(1280.0, 720.0);
    let mut state = MenuState::default();
    state.open_menu("file");
    let mut tree = UiTree::new();

    settle(&mut tree, &mut state, viewport);
    hover_frame(&mut tree, &mut state, viewport, "file", "New");
    assert!(!state.submenu_branch().is_empty());

    hover_item(&mut tree, &mut state, "file", "file.save");
    assert!(
        state.submenu_branch().is_empty(),
        "hovering a plain row must collapse the branch"
    );
}
