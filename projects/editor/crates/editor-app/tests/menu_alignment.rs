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

#[test]
fn a_nested_panel_starts_at_the_corner_of_the_row_that_opened_it() {
    let mut state = EditorUiState::default();
    step(&mut state, UiInput::default());
    click(&mut state, MenuNodeKey::trigger_key("file"));

    // Level 2: the "New" row inside the File panel.
    hover(&mut state, row_key("file", "New"));
    let row = rect(&state, row_key("file", "New"));
    let panel = rect(&state, panel_key("file/New"));
    let first = rect(&state, MenuNodeKey::item_key("file/New", "file.new.scene"));

    assert!(
        (panel.x - (row.x + row.w)).abs() < 0.5,
        "the panel's left edge must start at the row's right edge: \
         panel={panel:?} row={row:?}"
    );
    assert!(
        (panel.y - row.y).abs() < 0.5,
        "the panel's top must sit on the row's top: panel={panel:?} row={row:?}"
    );
    let _ = first;
}

#[test]
fn a_third_level_panel_aligns_the_same_way() {
    let mut state = EditorUiState::default();
    step(&mut state, UiInput::default());
    click(&mut state, MenuNodeKey::trigger_key("file"));

    hover(&mut state, row_key("file", "New"));
    hover(&mut state, row_key("file/New", "From Template"));

    let row = rect(&state, row_key("file/New", "From Template"));
    let panel = rect(&state, panel_key("file/New/From Template"));
    let first = rect(
        &state,
        MenuNodeKey::item_key("file/New/From Template", "file.new.template.empty3d"),
    );

    assert!(
        (panel.x - (row.x + row.w)).abs() < 0.5,
        "level 3's left edge must start at its row's right edge: \
         panel={panel:?} row={row:?}"
    );
    assert!(
        (panel.y - row.y).abs() < 0.5,
        "level 3's top must sit on its row's top: panel={panel:?} row={row:?}"
    );
    let _ = first;
}
