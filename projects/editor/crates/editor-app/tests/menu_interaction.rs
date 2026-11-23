//! Behaviour the menu bar owes every desktop app: hovering another trigger
//! while a menu is open switches to it, and clicking outside closes everything.

use glam::Vec2;
use prism_ui::menu::MenuNodeKey;
use prism_ui::prelude::*;

use editor_app::EditorUiState;

const VIEWPORT: Vec2 = Vec2::new(1280.0, 720.0);

fn rect(state: &EditorUiState, key: NodeKey) -> Rect {
    let id = state.tree.node_by_key(key).expect("node exists");
    state.tree.node(id).unwrap().computed_rect
}

fn step(state: &mut EditorUiState, input: UiInput) {
    state.build_and_step(VIEWPORT, &input, 0.016);
}

fn click(state: &mut EditorUiState, at: Vec2) {
    step(
        state,
        UiInput {
            pointer_pos: at,
            primary_down: true,
            primary_just_pressed: true,
            ..Default::default()
        },
    );
    step(
        state,
        UiInput {
            pointer_pos: at,
            primary_just_released: true,
            ..Default::default()
        },
    );
    step(state, UiInput::default());
}

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

fn open_menu(state: &mut EditorUiState, id: &str) {
    let r = rect(state, MenuNodeKey::trigger_key(id));
    click(state, Vec2::new(r.x + r.w * 0.5, r.y + r.h * 0.5));
}

#[test]
fn hovering_another_trigger_switches_the_open_menu() {
    let mut state = EditorUiState::default();
    step(&mut state, UiInput::default());
    open_menu(&mut state, "file");
    assert_eq!(state.menu.open(), Some("file"));

    hover(&mut state, MenuNodeKey::trigger_key("edit"));
    assert_eq!(
        state.menu.open(),
        Some("edit"),
        "hovering another trigger must switch to it while a menu is open"
    );

    hover(&mut state, MenuNodeKey::trigger_key("view"));
    assert_eq!(state.menu.open(), Some("view"));
    assert!(
        state
            .tree
            .node_by_key(("menu", "dropdown", "view").into_node_key())
            .is_some(),
        "the switched menu must actually be mounted"
    );
}

#[test]
fn hovering_a_trigger_does_not_open_a_closed_menu() {
    let mut state = EditorUiState::default();
    step(&mut state, UiInput::default());
    assert_eq!(state.menu.open(), None);
    hover(&mut state, MenuNodeKey::trigger_key("view"));
    assert_eq!(
        state.menu.open(),
        None,
        "hover alone must not open a menu; that stays a click"
    );
}

#[test]
fn clicking_empty_space_closes_the_open_menu() {
    let mut state = EditorUiState::default();
    step(&mut state, UiInput::default());
    open_menu(&mut state, "view");
    assert_eq!(state.menu.open(), Some("view"));

    click(&mut state, Vec2::new(900.0, 600.0));
    assert_eq!(
        state.menu.open(),
        None,
        "an outside click must close the menu"
    );
    assert!(
        state
            .tree
            .node_by_key(("menu", "dropdown", "view").into_node_key())
            .is_none(),
        "the closed menu must stop being mounted"
    );
}

#[test]
fn moving_from_a_row_into_its_submenu_keeps_the_branch_open() {
    let mut state = EditorUiState::default();
    step(&mut state, UiInput::default());
    open_menu(&mut state, "view");

    let row: NodeKey = ("menu", "submenu", "view", "Panels").into_node_key();
    hover(&mut state, row);
    assert_eq!(state.menu.submenu_branch(), ["Panels"].as_slice());

    let row_rect = rect(&state, row);
    let item = rect(
        &state,
        MenuNodeKey::item_key("view/Panels", "view.panel.hierarchy"),
    );
    let from = Vec2::new(row_rect.x + row_rect.w - 4.0, row_rect.y + row_rect.h * 0.5);
    let to = Vec2::new(item.x + item.w * 0.5, item.y + item.h * 0.5);
    let steps = ((to - from).length().ceil() as i32).max(1);
    for i in 1..=steps {
        let p = from + (to - from) * (i as f32 / steps as f32);
        step(
            &mut state,
            UiInput {
                pointer_pos: p,
                ..Default::default()
            },
        );
        assert_eq!(
            state.menu.submenu_branch(),
            ["Panels"].as_slice(),
            "the branch collapsed at {p:?} while moving into its own panel"
        );
    }
}
