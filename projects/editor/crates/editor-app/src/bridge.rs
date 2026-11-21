//! Bridge between `prism_ui` and `prism_engine` (`InputManager`, `ScreenSize`, `UiDrawList`).

use glam::Vec2;
use prism_engine::ecs::World;
use prism_engine::input::{InputManager, KeyCode, MouseButton};
use prism_engine::ui::components::TextAlign as EngineTextAlign;
use prism_engine::ui::ScreenSize;
use prism_engine::ui::{UiDrawList as EngineUiDrawList, UiQuad as EngineUiQuad, UiTextCmd};
use prism_ui::prelude::*;

/// Read live screen dimensions from `World`.
pub fn screen_size(world: &World) -> Vec2 {
    world
        .get_resource::<ScreenSize>()
        .map(|s| Vec2::new(s.width, s.height))
        .unwrap_or(Vec2::new(1280.0, 720.0))
}

/// Sample `prism_engine::input::InputManager` into a `prism_ui::UiInput` frame snapshot.
pub fn read_engine_input(world: &World, time_seconds: f64) -> UiInput {
    let Some(input) = world.get_resource::<InputManager>() else {
        return UiInput {
            time_seconds,
            ..Default::default()
        };
    };
    let [mx, my] = input.mouse_position();
    UiInput {
        pointer_pos: Vec2::new(mx as f32, my as f32),
        primary_down: input.mouse_held(MouseButton::Left),
        primary_just_pressed: input.mouse_just_pressed(MouseButton::Left),
        primary_just_released: input.mouse_just_released(MouseButton::Left),
        scroll_delta: Vec2::new(0.0, input.scroll_delta() as f32),
        shift_held: input.key_held(KeyCode::ShiftLeft) || input.key_held(KeyCode::ShiftRight),
        ctrl_held: input.key_held(KeyCode::ControlLeft) || input.key_held(KeyCode::ControlRight),
        text_committed: String::new(),
        backspace_pressed: false,
        escape_pressed: input.key_just_pressed(KeyCode::Escape),
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
            } => {
                if *border_width > 0.0 && border_color.a > 0.0 {
                    dst.push_quad(EngineUiQuad {
                        rect: rect.to_array(),
                        color: border_color.to_array(),
                        border_radius: *radius,
                        texture: None,
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
                        layer: 0,
                        order,
                    });
                } else {
                    dst.push_quad(EngineUiQuad {
                        rect: rect.to_array(),
                        color: color.to_array(),
                        border_radius: *radius,
                        texture: texture.map(|t| t.0),
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
            UiDrawCommand::BackdropBlur {
                rect, radius, tint, ..
            } => {
                // Fallback quad until Render Graph Dual-Kawase backdrop pass is wired.
                dst.push_quad(EngineUiQuad {
                    rect: rect.to_array(),
                    color: tint.with_alpha(tint.a.max(0.72)).to_array(),
                    border_radius: *radius,
                    texture: None,
                    layer: 1,
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

pub fn submit_draw_list(world: &mut World, src: &UiDrawList) {
    let engine_list = convert_to_engine_draw_list(src);
    world.insert_resource(engine_list);
}
