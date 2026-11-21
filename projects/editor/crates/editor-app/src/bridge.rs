//! Bridge between `prism_ui` and `prism_engine` (`UiInputState`, `ScreenSize`, `UiDrawList`).

use glam::Vec2;
use prism_engine::ecs::World;
use prism_engine::ui::components::TextAlign as EngineTextAlign;
use prism_engine::ui::{ScreenSize, UiInputState};
use prism_engine::ui::{UiDrawList as EngineUiDrawList, UiQuad as EngineUiQuad, UiTextCmd};
use prism_ui::prelude::*;

/// Read live screen dimensions from `World`.
pub fn screen_size(world: &World) -> Vec2 {
    world
        .get_resource::<ScreenSize>()
        .map(|s| Vec2::new(s.width, s.height))
        .unwrap_or(Vec2::new(1280.0, 720.0))
}

/// Sample the engine's per-frame `UiInputState` into a `prism_ui::UiInput` snapshot.
///
/// `UiInputState` is the engine's published pointer state: it already carries the
/// `UiScreenMapping` conversion into layout space plus wheel and modifier state.
/// `InputManager` is NOT a `World` resource — it lives on `App` and is only folded
/// into `InputSnapshot` for gameplay — so reading it here returned `default()` every
/// frame: a cursor parked at (0,0) with no buttons, i.e. no editor interaction at all.
pub fn read_engine_input(world: &World, time_seconds: f64) -> UiInput {
    let Some(input) = world.get_resource::<UiInputState>() else {
        return UiInput {
            time_seconds,
            ..Default::default()
        };
    };
    let [mx, my] = input.cursor_pos;
    UiInput {
        pointer_pos: Vec2::new(mx, my),
        primary_down: input.left_held,
        primary_just_pressed: input.left_clicked,
        primary_just_released: input.left_released,
        scroll_delta: Vec2::new(input.scroll_delta[0], input.scroll_delta[1]),
        shift_held: input.shift_held,
        ctrl_held: input.ctrl_held,
        text_committed: String::new(),
        backspace_pressed: false,
        escape_pressed: input
            .pressed_keys
            .contains(&prism_engine::input::KeyCode::Escape),
        time_seconds,
    }
}

/// Convert `prism_ui::UiDrawList` into `prism_engine::ui::render::UiDrawList` and submit to `World`.
pub fn convert_to_engine_draw_list(src: &UiDrawList) -> EngineUiDrawList {
    let mut dst = EngineUiDrawList::default();
    let mut order: u32 = 0;

    for cmd in &src.commands {
        match cmd {
            UiDrawCommand::Quad {
                rect,
                color,
                border_color,
                border_width,
                radius,
                texture,
                material,
            } => {
                // A surface whose material samples the backdrop asks the renderer
                // for its filter pass; the tint stays the shader's own colour.
                let backdrop_sigma = material
                    .as_ref()
                    .map_or(0.0, |material| material.backdrop_sigma);
                if *border_width > 0.0 && border_color.a > 0.0 {
                    dst.push_quad(EngineUiQuad {
                        rect: rect.to_array(),
                        color: border_color.to_array(),
                        border_radius: *radius,
                        texture: None,
                        backdrop_sigma: 0.0,
                        layer: 0,
                        order,
                    });
                    order += 1;
                    let inset = *border_width;
                    let inner = [
                        rect.x + inset,
                        rect.y + inset,
                        (rect.w - inset * 2.0).max(0.0),
                        (rect.h - inset * 2.0).max(0.0),
                    ];
                    dst.push_quad(EngineUiQuad {
                        rect: inner,
                        color: color.to_array(),
                        border_radius: (*radius - inset).max(0.0),
                        texture: texture.map(|t| t.0),
                        backdrop_sigma,
                        layer: 0,
                        order,
                    });
                } else {
                    dst.push_quad(EngineUiQuad {
                        rect: rect.to_array(),
                        color: color.to_array(),
                        border_radius: *radius,
                        texture: texture.map(|t| t.0),
                        backdrop_sigma,
                        layer: 0,
                        order,
                    });
                }
                order += 1;
            }
            UiDrawCommand::Text {
                rect,
                content,
                font_size,
                color,
                align,
                wrap,
            } => {
                dst.push_text(UiTextCmd {
                    rect: rect.to_array(),
                    content: content.clone(),
                    font_size: *font_size,
                    color: color.to_array(),
                    alignment: match align {
                        TextAlign::Left => EngineTextAlign::Left,
                        TextAlign::Center => EngineTextAlign::Center,
                        TextAlign::Right => EngineTextAlign::Right,
                    },
                    font: String::new(),
                    wrap: *wrap,
                    layer: 0,
                    order,
                });
                order += 1;
            }
            UiDrawCommand::DropShadow {
                rect,
                radius,
                color,
                offset,
                ..
            } => {
                dst.push_quad(EngineUiQuad {
                    rect: [rect.x + offset.x, rect.y + offset.y, rect.w, rect.h],
                    color: color.to_array(),
                    border_radius: *radius,
                    texture: None,
                    backdrop_sigma: 0.0,
                    layer: 0,
                    order,
                });
                order += 1;
            }
            UiDrawCommand::Line { .. } | UiDrawCommand::PushClip(_) | UiDrawCommand::PopClip => {}
        }
    }

    dst
}

/// Append the editor's draw list to this frame's engine [`UiDrawList`].
///
/// The engine rebuilds `UiDrawList` in `ui::render` from ECS UI entities, and the
/// editor system runs after it (a system added after `runtime_initialize` is
/// appended to the schedule). Replacing the resource outright deleted whatever the
/// engine had just produced, so ECS UI entities — the ones this editor is meant to
/// migrate onto — never reached the renderer at all.
///
/// Appending keeps both: engine commands stay in their layers, editor commands go
/// on top in their own layer so editor chrome cannot be drawn under a scene's UI.
/// The renderer sorts on `layer`, so nothing here needs a re-sort.
pub fn submit_draw_list(world: &mut World, src: &UiDrawList) {
    let editor_list = convert_to_engine_draw_list(src);
    let mut engine_list = world
        .remove_resource::<EngineUiDrawList>()
        .unwrap_or_default();
    engine_list.items.extend(editor_list.items);
    world.insert_resource(engine_list);
}
