//! Frame-level tests for the editor UI composition.

use glam::Vec2;
use prism_ui::prelude::*;

use editor_app::EditorUiState;

fn draw_texts(state: &mut EditorUiState, viewport: Vec2) -> Vec<String> {
    state.build_and_step(viewport, &UiInput::default(), 0.016);
    state
        .draw_list
        .commands
        .iter()
        .filter_map(|cmd| match cmd {
            UiDrawCommand::Text { content, .. } => Some(content.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_frame_draws_the_menu_bar_and_the_panels() {
    let mut state = EditorUiState::default();
    state.i18n.locale = Locale::EnUs;
    let texts = draw_texts(&mut state, Vec2::new(1280.0, 720.0));

    for expected in ["File", "Edit", "View", "Scene", "Help"] {
        assert!(
            texts.iter().any(|t| t == expected),
            "menu bar must draw '{expected}'; drew: {texts:?}"
        );
    }
    for expected in ["HIERARCHY", "ASSETS"] {
        assert!(
            texts.iter().any(|t| t == expected),
            "panel must draw '{expected}'"
        );
    }
}

#[test]
fn opening_a_menu_draws_its_items_on_top() {
    let mut state = EditorUiState::default();
    let viewport = Vec2::new(1280.0, 720.0);
    state.build_and_step(viewport, &UiInput::default(), 0.016);

    // Click the "File" trigger.
    let trigger = state
        .tree
        .node_by_key(prism_ui::menu::MenuNodeKey::trigger_key("file"))
        .expect("File trigger exists");
    let rect = state.tree.node(trigger).unwrap().computed_rect;
    let center = Vec2::new(rect.x + rect.w * 0.5, rect.y + rect.h * 0.5);

    state.build_and_step(
        viewport,
        &UiInput {
            pointer_pos: center,
            primary_down: true,
            primary_just_pressed: true,
            ..Default::default()
        },
        0.016,
    );
    // A menu opens on click, which needs the release edge.
    state.build_and_step(
        viewport,
        &UiInput {
            pointer_pos: center,
            primary_just_released: true,
            ..Default::default()
        },
        0.016,
    );
    let texts = draw_texts(&mut state, viewport);

    assert_eq!(state.menu.open(), Some("file"), "File menu must open");
    // "New" and "Open Recent" are submenu rows; flat entries sit beside them.
    for expected in ["New", "Open Recent", "Save", "Exit"] {
        assert!(
            texts.iter().any(|t| t == expected),
            "open File menu must draw '{expected}'; drew: {texts:?}"
        );
    }
}
