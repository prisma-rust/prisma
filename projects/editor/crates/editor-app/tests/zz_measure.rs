//! Temporary measurement of real editor menu geometry.

use glam::Vec2;
use prism_ui::menu::MenuNodeKey;
use prism_ui::prelude::*;

use editor_app::EditorUiState;

fn row_key(scope: &str, label: &str) -> NodeKey {
    ("menu", "submenu", scope, label).into_node_key()
}

fn panel_key(scope: &str) -> NodeKey {
    ("menu", "dropdown", scope).into_node_key()
}

fn r(state: &EditorUiState, key: NodeKey) -> Rect {
    let id = state.tree.node_by_key(key).expect("node exists");
    state.tree.node(id).unwrap().computed_rect
}

#[test]
fn measure_real_editor() {
    let viewport = Vec2::new(1280.0, 720.0);
    let mut state = EditorUiState::default();
    state.build_and_step(viewport, &UiInput::default(), 0.016);

    // Open File.
    let t = r(&state, MenuNodeKey::trigger_key("file"));
    let c = Vec2::new(t.x + t.w * 0.5, t.y + t.h * 0.5);
    for input in [
        UiInput {
            pointer_pos: c,
            primary_down: true,
            primary_just_pressed: true,
            ..Default::default()
        },
        UiInput {
            pointer_pos: c,
            primary_just_released: true,
            ..Default::default()
        },
        UiInput::default(),
    ] {
        state.build_and_step(viewport, &input, 0.016);
    }

    let file_panel = r(&state, panel_key("file"));
    let new_row = r(&state, row_key("file", "New"));
    println!("File panel = {file_panel:?}");
    println!("New row    = {new_row:?}");

    // Hover "New".
    let c = Vec2::new(new_row.x + new_row.w * 0.5, new_row.y + new_row.h * 0.5);
    for input in [
        UiInput {
            pointer_pos: c,
            ..Default::default()
        },
        UiInput {
            pointer_pos: c,
            ..Default::default()
        },
    ] {
        state.build_and_step(viewport, &input, 0.016);
    }

    let l2 = r(&state, panel_key("file/New"));
    let first = r(&state, MenuNodeKey::item_key("file/New", "file.new.scene"));
    println!("L2 panel   = {l2:?}");
    println!("L2 item[0] = {first:?}");
    println!(
        "panel.x - (row.x+row.w) = {}",
        l2.x - (new_row.x + new_row.w)
    );
    println!("panel.y - row.y = {}", l2.y - new_row.y);
    println!("item.y  - row.y = {}", first.y - new_row.y);
}
