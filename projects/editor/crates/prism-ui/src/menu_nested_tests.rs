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

/// Hover the row with this label inside the panel scoped to `scope`.
fn hover_row(tree: &mut UiTree, scope: &str, label: &str) {
    let key = submenu_row_key(scope, label);
    let id = tree.node_by_key(key).expect("submenu row exists");
    let rect = tree.node(id).unwrap().computed_rect;
    let point = Vec2::new(rect.x + rect.w * 0.5, rect.y + rect.h * 0.5);
    tree.process_input(&UiInput {
        pointer_pos: point,
        ..Default::default()
    });
}

/// Hover a row and rebuild, so the next frame's branch resolution sees it.
fn hover_and_rebuild(
    tree: &mut UiTree,
    state: &MenuState,
    scope: &str,
    label: &str,
    viewport: Vec2,
) {
    hover_row(tree, scope, label);
    build(tree, state, viewport);
}

fn rect(tree: &UiTree, key: NodeKey) -> crate::style::Rect {
    let id = tree.node_by_key(key).expect("node exists");
    tree.node(id).unwrap().computed_rect
}

#[test]
fn nested_panels_open_to_the_right_of_their_row() {
    let viewport = Vec2::new(1280.0, 720.0);
    let mut state = MenuState::default();
    state.open_menu("file");
    let mut tree = UiTree::new();
    build(&mut tree, &state, viewport);

    // Open level 2 by hovering "New".
    hover_row(&mut tree, "file", "New");
    build(&mut tree, &state, viewport);
    let l2 = rect(&tree, ("menu", "dropdown", "file/New").into_node_key());
    let row2 = rect(&tree, submenu_row_key("file", "New"));
    assert!(l2.w > 0.0 && l2.h > 0.0, "level 2 must mount: {l2:?}");
    assert!(
        l2.x >= row2.x + row2.w - 1.0,
        "level 2 must open beside its row, not over it: panel={l2:?} row={row2:?}"
    );

    // Level 3.
    hover_row(&mut tree, "file/New", "From Template");
    build(&mut tree, &state, viewport);
    let l3 = rect(
        &tree,
        ("menu", "dropdown", "file/New/From Template").into_node_key(),
    );
    let row3 = rect(&tree, submenu_row_key("file/New", "From Template"));
    assert!(l3.w > 0.0, "level 3 must mount: {l3:?}");
    assert!(
        l3.x >= row3.x + row3.w - 1.0,
        "level 3 must open beside its row: panel={l3:?} row={row3:?}"
    );

    // Level 4.
    hover_row(&mut tree, "file/New/From Template", "Samples");
    build(&mut tree, &state, viewport);
    let l4 = rect(
        &tree,
        ("menu", "dropdown", "file/New/From Template/Samples").into_node_key(),
    );
    let row4 = rect(&tree, submenu_row_key("file/New/From Template", "Samples"));
    assert!(l4.w > 0.0, "level 4 must mount: {l4:?}");
    assert!(
        l4.x >= row4.x + row4.w - 1.0,
        "level 4 must open beside its row: panel={l4:?} row={row4:?}"
    );

    // Every level stays on screen and none is full height.
    for (name, panel) in [("l2", l2), ("l3", l3), ("l4", l4)] {
        assert!(
            panel.x >= 0.0 && panel.x + panel.w <= viewport.x + 0.5,
            "{name} must stay on screen: {panel:?}"
        );
        assert!(
            panel.h < viewport.y * 0.5,
            "{name} must size to its items: {panel:?}"
        );
    }
}

#[test]
fn a_panel_that_would_leave_the_window_flips_left() {
    // A narrow window forces the third level past the right edge.
    let viewport = Vec2::new(420.0, 720.0);
    let mut state = MenuState::default();
    state.open_menu("file");
    let mut tree = UiTree::new();
    build(&mut tree, &state, viewport);

    hover_row(&mut tree, "file", "New");
    build(&mut tree, &state, viewport);
    hover_row(&mut tree, "file/New", "From Template");
    build(&mut tree, &state, viewport);

    let l3 = rect(
        &tree,
        ("menu", "dropdown", "file/New/From Template").into_node_key(),
    );
    assert!(
        l3.x + l3.w <= viewport.x + 0.5,
        "a panel that would overflow must flip to the other side: {l3:?}"
    );
}

#[test]
fn moving_off_a_submenu_row_closes_the_deeper_levels() {
    let viewport = Vec2::new(1280.0, 720.0);
    let mut state = MenuState::default();
    state.open_menu("file");
    let mut tree = UiTree::new();
    build(&mut tree, &state, viewport);

    hover_row(&mut tree, "file", "New");
    build(&mut tree, &state, viewport);
    assert!(tree
        .node_by_key(("menu", "dropdown", "file/New").into_node_key())
        .is_some());

    // Hover a plain item in the first panel: the branch must collapse.
    let save = rect(&tree, MenuNodeKey::item_key("file", "file.save"));
    tree.process_input(&UiInput {
        pointer_pos: Vec2::new(save.x + save.w * 0.5, save.y + save.h * 0.5),
        ..Default::default()
    });
    build(&mut tree, &state, viewport);
    assert!(
        tree.node_by_key(("menu", "dropdown", "file/New").into_node_key())
            .is_none(),
        "hovering a plain row must close the submenu branch"
    );
}
