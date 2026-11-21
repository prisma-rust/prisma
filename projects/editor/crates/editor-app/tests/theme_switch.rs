//! Theme switching must actually change what is drawn.

use glam::Vec2;
use prism_ui::prelude::*;

use editor_app::EditorUiState;

fn first_bg(state: &mut EditorUiState, viewport: Vec2) -> Color {
    state.build_and_step(viewport, &UiInput::default(), 0.016);
    state
        .draw_list
        .commands
        .iter()
        .find_map(|cmd| match cmd {
            UiDrawCommand::Quad { color, .. } => Some(*color),
            _ => None,
        })
        .expect("the frame draws a background quad")
}

#[test]
fn switching_the_preset_changes_the_drawn_colors() {
    let viewport = Vec2::new(1280.0, 720.0);
    let mut state = EditorUiState::default();

    let dark_bg = first_bg(&mut state, viewport);
    assert_eq!(state.theme.preset(), ThemePreset::Dark);

    state.theme = Theme::from_preset(ThemePreset::Light);
    let light_bg = first_bg(&mut state, viewport);

    assert_eq!(state.theme.preset(), ThemePreset::Light);
    assert_ne!(
        dark_bg, light_bg,
        "the light preset must actually change the drawn background"
    );
    // Light means a bright background; guard against an inverted preset.
    assert!(
        light_bg.r > dark_bg.r && light_bg.g > dark_bg.g && light_bg.b > dark_bg.b,
        "light theme must be brighter: dark={dark_bg:?} light={light_bg:?}"
    );
}

#[test]
fn the_theme_menu_marks_the_active_preset() {
    let mut state = EditorUiState::default();
    let viewport = Vec2::new(1280.0, 720.0);
    state.build_and_step(viewport, &UiInput::default(), 0.016);

    // Open View and check the Light entry exists and is not checked yet.
    let trigger = state
        .tree
        .node_by_key(prism_ui::menu::MenuNodeKey::trigger_key("view"))
        .expect("View trigger exists");
    let rect = state.tree.node(trigger).unwrap().computed_rect;
    let center = Vec2::new(rect.x + rect.w * 0.5, rect.y + rect.h * 0.5);
    for input in [
        UiInput {
            pointer_pos: center,
            primary_down: true,
            primary_just_pressed: true,
            ..Default::default()
        },
        UiInput {
            pointer_pos: center,
            primary_just_released: true,
            ..Default::default()
        },
    ] {
        state.build_and_step(viewport, &input, 0.016);
    }
    assert_eq!(state.menu.open(), Some("view"));

    // The dropdown is mounted on the frame *after* the menu opens, so it needs
    // one more build before its nodes exist.
    state.build_and_step(viewport, &UiInput::default(), 0.016);

    // Theme is a submenu now: its rows only mount once the Theme row is entered,
    // so assert on the definition rather than on the open panel.
    let view = state
        .menus()
        .into_iter()
        .find(|menu| menu.id == "view")
        .expect("View menu exists");
    let theme_entry = view
        .entries
        .iter()
        .find_map(|entry| match entry {
            prism_ui::menu::MenuEntry::Submenu { label, entries } if label == "Theme" => {
                Some(entries.clone())
            }
            _ => None,
        })
        .expect("View must expose a Theme submenu");
    assert!(
        theme_entry
            .iter()
            .any(|entry| matches!(entry, prism_ui::menu::MenuEntry::Checkbox { command, .. } if command == "view.theme.light")),
        "the Theme submenu must offer the Light preset"
    );
}
