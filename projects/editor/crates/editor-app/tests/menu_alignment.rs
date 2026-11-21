//! Nested panels must align to the row that opened them, measured on the real
//! editor composition rather than a hand-built fixture: the editor mounts the
//! menu bar inside a column of panels, and that is where alignment regressed.

use glam::Vec2;
use prism_ui::menu::MenuNodeKey;
use prism_ui::prelude::*;

use editor_app::EditorUiState;

const VIEWPORT: Vec2 = Vec2::new(1280.0, 720.0);

fn rect(state: &EditorUiState, key: NodeKey) -> Rect {
    let id = state.tree.node_by_key(key).expect("node exists");
    state.tree.node(id).unwrap().computed_rect
}

fn row_key(scope: &str, label: &str) -> NodeKey {
    ("menu", "submenu", scope, label).into_node_key()
}

fn panel_key(scope: &str) -> NodeKey {
    ("menu", "dropdown", scope).into_node_key()
}

fn step(state: &mut EditorUiState, input: UiInput) {
    state.build_and_step(VIEWPORT, &input, 0.016);
}

/// Click a node's centre.
fn click(state: &mut EditorUiState, key: NodeKey) {
    let r = rect(state, key);
    let c = Vec2::new(r.x + r.w * 0.5, r.y + r.h * 0.5);
    step(
        state,
        UiInput {
            pointer_pos: c,
            primary_down: true,
            primary_just_pressed: true,
            ..Default::default()
        },
    );
    step(
        state,
        UiInput {
            pointer_pos: c,
            primary_just_released: true,
            ..Default::default()
        },
    );
    step(state, UiInput::default());
}

/// Park the pointer on a node's centre and let the next frames settle.
fn hover(state: &mut EditorUiState, key: NodeKey) {
    let r = rect(state, key);
    let c = Vec2::new(r.x + r.w * 0.5, r.y + r.h * 0.5);
    for _ in 0..3 {
        step(
            state,
            UiInput {
                pointer_pos: c,
                ..Default::default()
            },
        );
    }
}

/// Inset of a panel's first item from the panel's own top-left corner.
fn inset(state: &EditorUiState, panel: NodeKey, first_item: NodeKey) -> Vec2 {
    let p = rect(state, panel);
    let f = rect(state, first_item);
    Vec2::new(f.x - p.x, f.y - p.y)
}

/// Every dropdown, whatever its depth, insets its items by the theme's panel
/// padding. A nested panel that drops the inset sits flush against its frame.
fn assert_shared_inset(state: &EditorUiState, panel: NodeKey, first_item: NodeKey) {
    let pad = state.theme.menu_dropdown_padding;
    let got = inset(state, panel, first_item);
    assert!(
        (got.x - pad.left).abs() < 0.5 && (got.y - pad.top).abs() < 0.5,
        "a dropdown insets its items by the theme padding ({pad:?}), not {got:?}"
    );
}

/// A nested panel's first item is level with the row that opened it, and its
/// frame starts one inset above that row so the shared padding stays intact.
fn assert_level_with_row(
    state: &EditorUiState,
    scope: &str,
    row: NodeKey,
    first_item_command: &str,
) {
    let pad = state.theme.menu_dropdown_padding;
    let row_rect = rect(state, row);
    let panel = rect(state, panel_key(scope));
    let first = rect(state, MenuNodeKey::item_key(scope, first_item_command));
    assert!(
        (first.y - row_rect.y).abs() < 0.5,
        "the first item must sit on the row's top: first={first:?} row={row_rect:?}"
    );
    assert!(
        (panel.y - (row_rect.y - pad.top)).abs() < 0.5,
        "the frame sits one inset above the row so the item lands on it: \
         panel={panel:?} row={row_rect:?} inset={:?}",
        pad.top
    );
    assert!(
        (panel.x - (row_rect.x + row_rect.w)).abs() < 0.5,
        "the frame starts at the row's right edge: panel={panel:?} row={row_rect:?}"
    );
    assert_shared_inset(
        state,
        panel_key(scope),
        MenuNodeKey::item_key(scope, first_item_command),
    );
}

#[test]
fn a_nested_panel_puts_its_first_item_level_with_the_row_that_opened_it() {
    let mut state = EditorUiState::default();
    step(&mut state, UiInput::default());
    click(&mut state, MenuNodeKey::trigger_key("file"));

    assert_shared_inset(&state, panel_key("file"), row_key("file", "New"));

    // Level 2: the "New" row inside the File panel.
    hover(&mut state, row_key("file", "New"));
    assert_level_with_row(&state, "file/New", row_key("file", "New"), "file.new.scene");
}

#[test]
fn a_third_level_panel_aligns_the_same_way() {
    let mut state = EditorUiState::default();
    step(&mut state, UiInput::default());
    click(&mut state, MenuNodeKey::trigger_key("file"));

    hover(&mut state, row_key("file", "New"));
    hover(&mut state, row_key("file/New", "From Template"));
    assert_level_with_row(
        &state,
        "file/New/From Template",
        row_key("file/New", "From Template"),
        "file.new.template.empty3d",
    );
}
