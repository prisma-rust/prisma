//! Bridge between `prism_ui` and the engine (`UiInputState`, `ScreenSize`, draw packets).

use glam::Vec2;
use prism_engine::ecs::World;
use prism_engine::ui::{ScreenSize, UiInputState};
use prism_ui::prelude::*;
use prism_ui_draw::{UiQuad, UiTextInput};

/// Read live screen dimensions from `World`.
pub fn screen_size(world: &World) -> Vec2 {
    world
        .get_resource::<ScreenSize>()
        .map(|s| Vec2::new(s.width, s.height))
        .unwrap_or(Vec2::new(1280.0, 720.0))
}

/// Sample the engine per-frame `UiInputState` into a `prism_ui::UiInput` snapshot.
///
/// `UiInputState` is the engine published pointer state: it already carries the
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

/// Translate one widget draw command into packet quads.
///
/// A border is its own quad under the fill: the packet has no border field, and
/// the engine merge sorts by push order, so the fill lands on top unchanged.
/// `PushClip` / `PopClip` / `Line` stay untranslated until a consumer needs them.
fn push_command(out: &mut prism_ui_draw::UiOverlayInput, cmd: &UiDrawCommand) {
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
            // for its filter pass; the tint stays the shader own colour.
            let backdrop_sigma = material
                .as_ref()
                .map_or(0.0, |material| material.backdrop_sigma);
            let texture = texture.map(|t| t.0);
            if *border_width > 0.0 && border_color.a > 0.0 {
                let inset = *border_width;
                let inner = [
                    rect.x + inset,
                    rect.y + inset,
                    (rect.w - inset * 2.0).max(0.0),
                    (rect.h - inset * 2.0).max(0.0),
                ];
                out.push_quad(UiQuad {
                    rect: rect.to_array(),
                    color: border_color.to_array(),
                    border_radius: *radius,
                    texture: None,
                    backdrop_sigma,
                });
                out.push_quad(UiQuad {
                    rect: inner,
                    color: color.to_array(),
                    border_radius: (*radius - inset).max(0.0),
                    texture,
                    backdrop_sigma,
                });
            } else {
                out.push_quad(UiQuad {
                    rect: rect.to_array(),
                    color: color.to_array(),
                    border_radius: *radius,
                    texture,
                    backdrop_sigma,
                });
            }
        }
        UiDrawCommand::Text {
            rect,
            content,
            font_size,
            color,
            align,
            wrap,
            outline_width,
            outline_color,
            shadow_offset,
            shadow_color,
        } => {
            out.push_text(UiTextInput {
                rect: rect.to_array(),
                content: content.clone(),
                font_size: *font_size,
                color: color.to_array(),
                alignment: match align {
                    TextAlign::Left => prism_ui_draw::UiTextAlign::Left,
                    TextAlign::Center => prism_ui_draw::UiTextAlign::Center,
                    TextAlign::Right => prism_ui_draw::UiTextAlign::Right,
                },
                font: None,
                wrap: *wrap,
                outline_width: *outline_width,
                outline_color: outline_color.to_array(),
                shadow_offset: shadow_offset.to_array(),
                shadow_color: shadow_color.to_array(),
            });
        }
        UiDrawCommand::DropShadow {
            rect,
            radius,
            color,
            offset,
            ..
        } => {
            out.push_quad(UiQuad {
                rect: [rect.x + offset.x, rect.y + offset.y, rect.w, rect.h],
                color: color.to_array(),
                border_radius: *radius,
                texture: None,
                backdrop_sigma: 0.0,
            });
        }
        UiDrawCommand::Line { .. } | UiDrawCommand::PushClip(_) | UiDrawCommand::PopClip => {}
    }
}

/// Flatten the widget draw list into the packet the engine merges and the
/// renderer rasterizes.
pub fn to_ui_overlay(src: &UiDrawList) -> prism_ui_draw::UiOverlayInput {
    let mut out = prism_ui_draw::UiOverlayInput::default();
    for cmd in &src.commands {
        push_command(&mut out, cmd);
    }
    out
}

/// The layer every editor chrome command lands on: the engine merges
/// externals into this same layer, so both channels sort together and the
/// packet's interleaved quad/text sequence survives.
const EDITOR_LAYER: i32 = prism_engine::ui::EXTERNAL_LAYER;

/// Hand the editor draw packet to the engine.
///
/// The engine rebuilds `UiDrawList` in `ui::render` from ECS UI entities, and the
/// editor system runs before it, so the merge in `merge_external_quads` keeps both:
/// engine commands stay in their layers, editor commands go on top in
/// `EDITOR_LAYER` so editor chrome cannot be drawn under a scene UI.
///
/// Quads ride the `ExternalUiQuads` channel; text has no external channel and
/// joins the engine's own draw list directly. Both carry a shared increasing
/// `order` within `EDITOR_LAYER`, so replaying `packet.order` reproduces the
/// widget tree's exact quad/text sequence — a scrim still covers earlier text.
pub fn submit_draw_list(world: &mut World, src: &UiDrawList) {
    let packet = to_ui_overlay(src);
    let mut engine_list = world
        .remove_resource::<prism_engine::ui::UiDrawList>()
        .unwrap_or_default();
    for (idx, cmd) in packet.order.into_iter().enumerate() {
        let order = idx as u32;
        match cmd {
            prism_ui_draw::UiOverlayDraw::Quad(index) => {
                if let Some(quad) = packet.quads.get(index as usize) {
                    engine_list.push_quad(prism_engine::ui::UiQuad {
                        rect: quad.rect,
                        color: quad.color,
                        border_radius: quad.border_radius,
                        texture: quad.texture,
                        backdrop_sigma: quad.backdrop_sigma,
                        layer: EDITOR_LAYER,
                        order,
                    });
                }
            }
            prism_ui_draw::UiOverlayDraw::Text(index) => {
                if let Some(text) = packet.texts.get(index as usize) {
                    engine_list.push_text(prism_engine::ui::UiTextCmd {
                        rect: text.rect,
                        content: text.content.clone(),
                        font_size: text.font_size,
                        color: text.color,
                        alignment: match text.alignment {
                            prism_ui_draw::UiTextAlign::Left => {
                                prism_engine::ui::components::TextAlign::Left
                            }
                            prism_ui_draw::UiTextAlign::Center => {
                                prism_engine::ui::components::TextAlign::Center
                            }
                            prism_ui_draw::UiTextAlign::Right => {
                                prism_engine::ui::components::TextAlign::Right
                            }
                        },
                        font: String::new(),
                        wrap: text.wrap,
                        outline_width: text.outline_width,
                        outline_color: text.outline_color,
                        shadow_offset: text.shadow_offset,
                        shadow_color: text.shadow_color,
                        layer: EDITOR_LAYER,
                        order,
                    });
                }
            }
        }
    }
    world.insert_resource(engine_list);
}
