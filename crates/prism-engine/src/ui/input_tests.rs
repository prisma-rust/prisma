//! `ui_input_system` hit-test contract: visibility, hierarchy occlusion, click consumption, press / release semantics.

use super::{ui_input_system, UiFocus, UiInputState};
use crate::ui::components::*;
use prism_ecs::{Entity, World};
use prism_scene::components::Parent;
use prism_scene::HierarchyHelper;

/// Create an interactive UI element (`Node + ComputedLayout + Style + Interaction`).
fn spawn_ui(world: &mut World, rect: [f32; 4], visible: bool) -> Entity {
    let entity = world.spawn();
    world.insert(entity, Node {});
    world.insert(entity, ComputedLayout::from_rect(rect));
    world.insert(
        entity,
        Style {
            visible,
            ..Default::default()
        },
    );
    world.insert(entity, Interaction::default());
    entity
}

fn input(world: &mut World, cursor: [f32; 2], clicked: bool, held: bool) {
    world.insert_resource(UiInputState {
        cursor_pos: cursor,
        left_clicked: clicked,
        left_held: held,
        left_released: false,
        ..UiInputState::default()
    });
}

/// Press left button at `cursor`.
fn press(world: &mut World, cursor: [f32; 2]) {
    input(world, cursor, true, true);
    ui_input_system(world);
}

/// Keep held (no press / release edge).
fn hold(world: &mut World, cursor: [f32; 2]) {
    input(world, cursor, false, true);
    ui_input_system(world);
}

/// Release left button.
fn release(world: &mut World, cursor: [f32; 2]) {
    world.insert_resource(UiInputState {
        cursor_pos: cursor,
        left_clicked: false,
        left_held: false,
        left_released: true,
        ..UiInputState::default()
    });
    ui_input_system(world);
}

/// Press and release at same position (one complete click).
fn click(world: &mut World, cursor: [f32; 2]) {
    press(world, cursor);
    release(world, cursor);
}

fn idle(world: &mut World, cursor: [f32; 2]) {
    input(world, cursor, false, false);
    ui_input_system(world);
}

fn state_of(world: &World, entity: Entity) -> Interaction {
    *world
        .get::<Interaction>(entity)
        .expect("entity has Interaction")
}

#[test]
fn click_is_consumed_by_the_top_most_element() {
    let mut world = World::new();
    let panel = spawn_ui(&mut world, [0.0, 0.0, 800.0, 600.0], true);
    let button = spawn_ui(&mut world, [350.0, 275.0, 100.0, 50.0], true);
    HierarchyHelper::reparent(&mut world, button, Some(panel));

    press(&mut world, [400.0, 300.0]);
    let pressed = state_of(&world, button);
    assert!(pressed.hovered, "Cursor inside button");
    assert!(pressed.pressed, "pressed set when pressed");
    assert!(!pressed.clicked, "clicked only true on release");

    release(&mut world, [400.0, 300.0]);
    let button_state = state_of(&world, button);
    assert!(button_state.hovered);
    assert!(
        button_state.clicked,
        "Press and release on same element = click"
    );
    assert!(!button_state.pressed, "Not pressed after release");

    let panel_state = state_of(&world, panel);
    assert!(
        !panel_state.hovered,
        "Background panel should not be hovered at same time"
    );
    assert!(
        !panel_state.clicked,
        "Click consumed by topmost, not bubbled to panel"
    );
    assert!(!panel_state.pressed);
}

#[test]
fn releasing_outside_the_button_is_not_a_click() {
    let mut world = World::new();
    let button = spawn_ui(&mut world, [100.0, 100.0, 100.0, 40.0], true);

    press(&mut world, [120.0, 110.0]);
    assert!(state_of(&world, button).pressed);
    release(&mut world, [500.0, 500.0]);

    let state = state_of(&world, button);
    assert!(!state.clicked, "Dragging out then releasing not a click");
    assert!(!state.hovered);
}

#[test]
fn pressing_outside_then_releasing_inside_is_not_a_click() {
    let mut world = World::new();
    let button = spawn_ui(&mut world, [100.0, 100.0, 100.0, 40.0], true);

    press(&mut world, [500.0, 500.0]);
    release(&mut world, [120.0, 110.0]);

    let state = state_of(&world, button);
    assert!(state.hovered, "Cursor actually on button at release");
    assert!(
        !state.clicked,
        "Press target was not it, no click triggered"
    );
}

#[test]
fn disabled_button_hovers_but_never_clicks() {
    let mut world = World::new();
    let button = spawn_ui(&mut world, [100.0, 100.0, 100.0, 40.0], true);
    world.insert(
        button,
        Button {
            interactable: false,
            ..Default::default()
        },
    );

    press(&mut world, [120.0, 110.0]);
    release(&mut world, [120.0, 110.0]);

    let state = state_of(&world, button);
    assert!(
        state.hovered,
        "Non-interactive still shows hover appearance"
    );
    assert!(!state.pressed);
    assert!(!state.clicked);
}

#[test]
fn enter_activates_the_focused_button() {
    let mut world = World::new();
    let button = spawn_ui(&mut world, [100.0, 100.0, 100.0, 40.0], true);
    world.insert(button, Button::default());

    click(&mut world, [120.0, 110.0]);
    assert_eq!(
        world.get_resource::<UiFocus>().and_then(|f| f.entity),
        Some(button),
        "Button gets keyboard focus after click"
    );

    idle(&mut world, [500.0, 500.0]);
    world.insert_resource(UiInputState {
        cursor_pos: [500.0, 500.0],
        left_clicked: false,
        left_held: false,
        left_released: false,
        pressed_keys: vec![crate::input::KeyCode::Enter],
        ..UiInputState::default()
    });
    ui_input_system(&mut world);
    assert!(
        state_of(&world, button).clicked,
        "Enter on focused button equals click"
    );

    idle(&mut world, [500.0, 500.0]);
    world.insert_resource(UiInputState {
        cursor_pos: [500.0, 500.0],
        left_clicked: false,
        left_held: false,
        left_released: false,
        pressed_keys: vec![crate::input::KeyCode::Space],
        ..UiInputState::default()
    });
    ui_input_system(&mut world);
    assert!(state_of(&world, button).clicked);
}

#[test]
fn a_button_without_an_interaction_component_still_works() {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, Node {});
    world.insert(entity, ComputedLayout::from_rect([0.0, 0.0, 100.0, 40.0]));
    world.insert(entity, Style::default());
    world.insert(entity, Button::default());
    assert!(world.get::<Interaction>(entity).is_none());

    click(&mut world, [10.0, 10.0]);
    assert!(world.get::<Interaction>(entity).is_some());
    assert!(state_of(&world, entity).clicked);
}

#[test]
fn hidden_element_is_not_hittable() {
    let mut world = World::new();
    let hidden = spawn_ui(&mut world, [0.0, 0.0, 800.0, 600.0], false);

    click(&mut world, [400.0, 300.0]);

    let state = state_of(&world, hidden);
    assert!(!state.hovered, "`visible: false` element cannot be hovered");
    assert!(!state.clicked, "`visible: false` element cannot be clicked");
    assert!(!state.pressed);
}

#[test]
fn at_equal_depth_the_smaller_element_wins() {
    let mut world = World::new();
    let big = spawn_ui(&mut world, [0.0, 0.0, 400.0, 400.0], true);
    let small = spawn_ui(&mut world, [100.0, 100.0, 50.0, 50.0], true);

    click(&mut world, [120.0, 120.0]);

    assert!(state_of(&world, small).clicked);
    assert!(!state_of(&world, big).hovered);
}

#[test]
fn state_is_cleared_when_the_cursor_leaves() {
    let mut world = World::new();
    let button = spawn_ui(&mut world, [10.0, 10.0, 100.0, 40.0], true);

    click(&mut world, [20.0, 20.0]);
    assert!(state_of(&world, button).clicked);

    idle(&mut world, [500.0, 500.0]);
    let state = state_of(&world, button);
    assert!(!state.hovered);
    assert!(!state.clicked);
    assert!(!state.pressed);
}

#[test]
fn cyclic_parent_chain_does_not_hang() {
    let mut world = World::new();
    let a = spawn_ui(&mut world, [0.0, 0.0, 100.0, 100.0], true);
    let b = spawn_ui(&mut world, [0.0, 0.0, 100.0, 100.0], true);
    world.insert(a, Parent(b));
    world.insert(b, Parent(a));

    idle(&mut world, [10.0, 10.0]);

    let hovered = u8::from(state_of(&world, a).hovered) + u8::from(state_of(&world, b).hovered);
    assert_eq!(hovered, 1, "With cycle should still hit only one element");
}

#[test]
fn holding_keeps_pressed_until_release() {
    let mut world = World::new();
    let button = spawn_ui(&mut world, [10.0, 10.0, 100.0, 40.0], true);

    press(&mut world, [20.0, 20.0]);
    assert!(state_of(&world, button).pressed);
    hold(&mut world, [20.0, 20.0]);
    let state = state_of(&world, button);
    assert!(state.pressed, "pressed stays true while held");
    assert!(!state.clicked, "clicked only valid for one frame");
}
