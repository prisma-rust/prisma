//! UI Input System — mouse hit test + Interaction component update
//!
//! Each frame reads mouse state from `UiInputState` resource,
//! does hit test against `Node + ComputedLayout`, updates `Interaction` component.
//!
//! # Hit test rules
//!
//! - Elements with `Style::visible == false` do not participate (consistent with `ui_render_system`
//!   skipping invisible elements when collecting quads).
//! - Entities with `Active(false)` do not participate, parent disable excludes whole child tree (consistent with draw).
//! - On overlap only **topmost** element gets `hovered / clicked / pressed`, click at hit point
//!   consumed, will not "pass through" to background panel.
//! - "Topmost" by UI hierarchy depth: child drawn above parent, so larger depth first; same depth
//!   smaller area more specific, first hit; depth and area both equal take later entity in query order
//!   (`ui_render_system` also pushes draw list by query order, later on top).

use crate::ui::components::*;
use crate::ui::is_ui_active;
use prism_ecs::{Entity, World};

/// Mouse / touch input state — copied by Engine each frame from `InputManager` into World.
///
/// This is the **only** pointer source host UI can read: `InputManager` is not a
/// World resource, and `InputSnapshot` is the cross-ABI bitmap for project modules
/// (no cursor mapping, no wheel). Every pointer quantity UI needs is here.
#[derive(Clone, Debug, Default)]
pub struct UiInputState {
    /// Cursor in UI layout space (pixels, top-left origin), mapped by `UiScreenMapping`.
    pub cursor_pos: [f32; 2],
    /// Whether left button pressed this frame (press edge).
    pub left_clicked: bool,
    /// Whether left button currently held.
    pub left_held: bool,
    /// Whether left button released this frame (release edge). Click holds on **release**.
    pub left_released: bool,
    /// Wheel delta this frame in lines; positive y scrolls up.
    pub scroll_delta: [f32; 2],
    /// Shift held, the fine-grained tier of drag-to-scrub.
    pub shift_held: bool,
    /// Ctrl held, the snapping tier of drag-to-scrub.
    pub ctrl_held: bool,
    /// List of keys newly pressed this frame (deduplicated, for any-key skip etc).
    pub pressed_keys: Vec<crate::input::KeyCode>,
}

/// Left-button press target (#528).
///
/// Persistent resource: on press records hit entity, on release only "same entity" counts as click.
/// So pressing outside button then releasing on button will not misfire, and vice versa dragging out then releasing also not counted.
#[derive(Clone, Copy, Debug, Default)]
pub struct UiPressState {
    pub entity: Option<Entity>,
}

/// Keyboard focus (#528): most recently clicked button, `Enter` / `Space` submits it.
#[derive(Clone, Copy, Debug, Default)]
pub struct UiFocus {
    pub entity: Option<Entity>,
}

/// UI Input System — runs each frame, does hit test and updates Interaction component.
///
/// Must run after `ui::layout` (needs ComputedLayout ready).
///
/// Click semantics consistent with Unity `Clickable`: press inside element, **and release inside element**
/// sets `clicked`; `pressed` means pointer still held on press target. Elements with `Button` and
/// `interactable = false` only update `hovered`, do not respond to press / click.
/// With keyboard focus (most recently clicked button), `Enter` / `Space` also sets `clicked`.
pub fn ui_input_system(world: &mut World) {
    let state = match world.get_resource::<UiInputState>() {
        Some(s) => s.clone(),
        None => return,
    };

    // Step zero: `Button` is authorable interactive element, scene need not explicitly attach
    // `Interaction` (that is runtime state); fill it here so author only writes `Button`.
    let buttons: Vec<Entity> = world.query::<Button>().map(|(entity, _)| entity).collect();
    for entity in buttons {
        if world.get::<Interaction>(entity).is_none() {
            world.insert(entity, Interaction::default());
        }
    }

    // Step one: clear all previous-frame state.
    for (_, interaction) in world.query_mut::<Interaction>() {
        interaction.clicked = false;
        interaction.hovered = false;
        interaction.pressed = false;
    }

    let hit = topmost_hit(world, state.cursor_pos);
    let mut press = world.remove_resource::<UiPressState>().unwrap_or_default();
    let mut focus = world.remove_resource::<UiFocus>().unwrap_or_default();

    // Step two: record target on press; click = release on same target.
    if state.left_clicked {
        press.entity = hit;
        if hit.and_then(|entity| world.get::<Button>(entity)).is_some() {
            focus.entity = hit;
        }
    }

    // Step three: write state only to topmost element.
    if let Some(entity) = hit {
        let interactable = world
            .get::<Button>(entity)
            .map(|button| button.interactable)
            .unwrap_or(true);
        if let Some(interaction) = world.get_mut::<Interaction>(entity) {
            interaction.hovered = true;
            if interactable {
                interaction.pressed =
                    press.entity == Some(entity) && (state.left_held || state.left_clicked);
                interaction.clicked = press.entity == Some(entity) && state.left_released;
            }
        }
    }

    // Step four: keyboard activation. Focused element need not be under cursor (cursor may have moved away).
    let activate = state.pressed_keys.iter().any(|key| {
        matches!(
            key,
            crate::input::KeyCode::Enter | crate::input::KeyCode::Space
        )
    });
    if activate {
        if let Some(entity) = focus.entity {
            let interactable = world
                .get::<Button>(entity)
                .map(|button| button.interactable)
                .unwrap_or(true);
            if interactable {
                if let Some(interaction) = world.get_mut::<Interaction>(entity) {
                    interaction.clicked = true;
                }
            }
        }
    }

    // After release invalidate press target (regardless of release inside / outside element).
    if state.left_released {
        press.entity = None;
    }
    world.insert_resource(press);
    world.insert_resource(focus);
}

/// Topmost interactive UI entity under cursor (`None` = no hit).
fn topmost_hit(world: &World, cursor: [f32; 2]) -> Option<Entity> {
    let mut best: Option<(Entity, u32, f32)> = None;

    for (entity, _node, _interaction) in world.query2::<Node, Interaction>() {
        // Elements disabled by `Active(false)` (including parent disable) do not participate.
        if !is_ui_active(world, entity) {
            continue;
        }
        // Invisible elements neither drawn nor clickable.
        if !world
            .get::<Style>(entity)
            .map(|style| style.visible)
            .unwrap_or(true)
        {
            continue;
        }

        let Some(layout) = world.get::<ComputedLayout>(entity) else {
            continue;
        };
        if !rect_contains(layout.rect, cursor) {
            continue;
        }

        let depth = ui_depth(world, entity);
        let area = layout.rect[2].max(0.0) * layout.rect[3].max(0.0);
        // Larger depth first; same depth smaller area first; fully equal keep later traversed
        // (later into draw list = drawn on top).
        let better = best.is_none_or(|(_, best_depth, best_area)| {
            depth > best_depth || (depth == best_depth && area <= best_area)
        });
        if better {
            best = Some((entity, depth, area));
        }
    }

    best.map(|(entity, _, _)| entity)
}

/// Whether screen-space rect `[left, top, width, height]` contains cursor (pixel coords, y downward).
fn rect_contains(rect: [f32; 4], cursor: [f32; 2]) -> bool {
    let [left, top, width, height] = rect;
    cursor[0] >= left && cursor[0] <= left + width && cursor[1] >= top && cursor[1] <= top + height
}

/// UI hierarchy depth (root = 0). Hierarchy only recognizes scene `Parent`.
///
/// Max 64 levels: dirty-data cycle will not hang a frame.
fn ui_depth(world: &World, entity: Entity) -> u32 {
    const MAX_DEPTH: u32 = 64;
    let mut depth = 0;
    let mut current = entity;
    while depth < MAX_DEPTH {
        let Some(parent) = world
            .get::<prism_scene::components::Parent>(current)
            .map(|parent| parent.0)
        else {
            break;
        };
        depth += 1;
        current = parent;
    }
    depth
}

#[cfg(test)]
#[path = "input_tests.rs"]
mod tests;
