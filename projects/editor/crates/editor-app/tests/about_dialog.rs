//! The About dialog: opened from the menu, drawn as frosted glass, closable.

use glam::Vec2;
use prism_ui::prelude::*;

use editor_app::about::{ABOUT_CLOSE_KEY, ABOUT_GLASS_SIGMA, ABOUT_PANEL_KEY};
use editor_app::EditorUiState;

const VIEWPORT: Vec2 = Vec2::new(1280.0, 720.0);

fn step(state: &mut EditorUiState, input: UiInput) {
    state.build_and_step(VIEWPORT, &input, 0.016);
}

fn center_of(state: &mut EditorUiState, key: NodeKey) -> Vec2 {
    let id = state.tree.node_by_key(key).expect("node exists");
    let rect = state.tree.node(id).unwrap().computed_rect;
    Vec2::new(rect.x + rect.w * 0.5, rect.y + rect.h * 0.5)
}

fn click(state: &mut EditorUiState, key: NodeKey) {
    let center = center_of(state, key);
    step(
        state,
        UiInput {
            pointer_pos: center,
            primary_down: true,
            primary_just_pressed: true,
            ..Default::default()
        },
    );
    step(
        state,
        UiInput {
            pointer_pos: center,
            primary_just_released: true,
            ..Default::default()
        },
    );
    // The dialog mounts on the frame after the command is handled.
    step(state, UiInput::default());
}

fn open_about(state: &mut EditorUiState) {
    step(state, UiInput::default());
    click(state, MenuNodeKey::trigger_key("help"));
    click(state, MenuNodeKey::item_key("help", "help.about"));
}

#[test]
fn the_help_menu_opens_the_about_dialog() {
    let mut state = EditorUiState::default();
    step(&mut state, UiInput::default());
    assert!(!state.about_open);

    open_about(&mut state);
    assert!(state.about_open, "Help > About must open the dialog");
    assert!(
        state
            .tree
            .node_by_key(ABOUT_PANEL_KEY.into_node_key())
            .is_some(),
        "the dialog panel must be mounted"
    );
}

#[test]
fn the_dialog_paints_a_glass_surface_over_the_editor() {
    let mut state = EditorUiState::default();
    open_about(&mut state);

    let glass = state.draw_list.commands.iter().find_map(|cmd| match cmd {
        UiDrawCommand::Quad {
            material: Some(material),
            ..
        } if material.needs_backdrop() => Some(material.clone()),
        _ => None,
    });
    let material = glass.expect("the About dialog must paint a backdrop-sampling surface");
    assert_eq!(material.shader, ShaderRef::FrostedGlass);
    assert_eq!(material.backdrop_sigma, ABOUT_GLASS_SIGMA);
}

#[test]
fn the_glass_surface_reaches_the_engine_draw_list() {
    let mut state = EditorUiState::default();
    open_about(&mut state);

    let packet = editor_app::bridge::to_ui_overlay(&state.draw_list);
    let sigma = packet
        .quads
        .iter()
        .map(|quad| quad.backdrop_sigma)
        .fold(0.0_f32, f32::max);
    assert_eq!(
        sigma, ABOUT_GLASS_SIGMA,
        "the material's backdrop radius must survive the bridge"
    );
}

#[test]
fn the_close_button_dismisses_the_dialog() {
    let mut state = EditorUiState::default();
    open_about(&mut state);
    assert!(state.about_open);

    click(&mut state, ABOUT_CLOSE_KEY.into_node_key());
    assert!(!state.about_open, "Close must dismiss the dialog");
    assert!(
        state
            .tree
            .node_by_key(ABOUT_PANEL_KEY.into_node_key())
            .is_none(),
        "a dismissed dialog must unmount"
    );
}

#[test]
fn the_dialog_follows_the_theme() {
    let mut state = EditorUiState::default();
    state.theme = Theme::from_preset(ThemePreset::Light);
    open_about(&mut state);

    let light_sigma = state
        .draw_list
        .commands
        .iter()
        .filter_map(|cmd| match cmd {
            UiDrawCommand::Quad {
                material: Some(material),
                ..
            } => Some(material.backdrop_sigma),
            _ => None,
        })
        .fold(0.0_f32, f32::max);
    assert_eq!(light_sigma, ABOUT_GLASS_SIGMA);
    assert_eq!(state.theme.preset(), ThemePreset::Light);
}
