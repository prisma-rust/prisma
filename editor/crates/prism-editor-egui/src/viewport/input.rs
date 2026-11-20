//! Viewport camera panning, entity picking proxies, and Game view input forwarding.

use super::*;

pub(super) fn pan_camera(
    camera: &mut prism_editor_core::workspace::OrbitCamera,
    dx: f32,
    dy: f32,
    viewport_height: f32,
) {
    let frame = vm::camera_frame(camera);
    let world_per_px = if let Some(half_h) = frame.ortho_half_height {
        (half_h * 2.0) / viewport_height.max(1.0)
    } else {
        let half_fov = (frame.fov_y_degrees.clamp(10.0, 170.0) * 0.5).to_radians();
        (2.0 * camera.distance.max(0.05) * half_fov.tan()) / viewport_height.max(1.0)
    };
    for axis in 0..3 {
        camera.target[axis] += (-dx * frame.right[axis] + dy * frame.up[axis]) * world_per_px;
    }
}

pub(super) fn snap_active(enabled: bool, modifier: bool) -> bool {
    enabled != modifier
}

/// Resolve the pointer-drag target. Order is the contract: UI handles before
/// world gizmos, and box selection only when neither was hit.
pub(crate) fn scene_drag_intent(
    has_ui_handle: bool,
    has_gizmo: bool,
    dragging: bool,
) -> SceneDragIntent {
    if has_ui_handle {
        SceneDragIntent::UiAnchor
    } else if has_gizmo {
        SceneDragIntent::Gizmo
    } else if dragging {
        SceneDragIntent::BoxSelect
    } else {
        SceneDragIntent::None
    }
}

pub(super) fn drag_translate_step(
    snap: prism_editor_core::workspace::SnapSettings,
    snapping: bool,
    start_axis_value: f32,
    accum: f32,
    applied: f32,
) -> f32 {
    let desired = if snapping {
        snap.snapped_translate(start_axis_value + accum) - start_axis_value
    } else {
        accum
    };
    desired - applied
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PickCycle {
    pub(super) point: (f32, f32),
    pub(super) candidates: Vec<usize>,
    pub(super) index: usize,
}

pub(super) fn next_pick(
    cycle: Option<&PickCycle>,
    point: (f32, f32),
    candidates: Vec<usize>,
) -> Option<(usize, PickCycle)> {
    if candidates.is_empty() {
        return None;
    }
    let repeated = cycle
        .map(|cycle| {
            (cycle.point.0 - point.0).abs() <= 4.0
                && (cycle.point.1 - point.1).abs() <= 4.0
                && cycle.candidates == candidates
        })
        .unwrap_or(false);
    let index = if repeated {
        (cycle.map(|cycle| cycle.index).unwrap_or(0) + 1) % candidates.len()
    } else {
        0
    };
    Some((
        candidates[index],
        PickCycle {
            point,
            candidates,
            index,
        },
    ))
}

pub fn viewport_extent(width: f32, height: f32, ppp: f32, scale: f32) -> (u32, u32) {
    let scale = scale.clamp(0.25, 2.0);
    (
        ((width * ppp * scale).round() as u32).clamp(1, 4096),
        ((height * ppp * scale).round() as u32).clamp(1, 4096),
    )
}

pub(crate) fn game_cursor(rect: Rect, position: egui::Pos2, extent: (u32, u32)) -> [f32; 2] {
    let width = rect.width().max(1.0);
    let height = rect.height().max(1.0);
    [
        ((position.x - rect.min.x) / width) * extent.0 as f32,
        ((position.y - rect.min.y) / height) * extent.1 as f32,
    ]
}

pub fn game_resolution_presets(project: Option<(u32, u32)>) -> Vec<GameResolution> {
    let mut out = vec![GameResolution::MatchWindow];
    if let Some((width, height)) = project.filter(|(w, h)| *w > 0 && *h > 0) {
        out.push(GameResolution::Fixed { width, height });
    }
    out.extend_from_slice(GAME_ASPECT_PRESETS);
    for preset in GAME_RESOLUTION_PRESETS.iter().skip(1) {
        if !out.contains(preset) {
            out.push(*preset);
        }
    }
    out
}

pub(crate) fn drag_samples(from: (f32, f32), to: (f32, f32), steps: u32) -> Vec<(f32, f32)> {
    let steps = steps.max(1);
    (1..=steps)
        .map(|step| {
            let t = step as f32 / steps as f32;
            (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)
        })
        .collect()
}

pub fn letterbox(outer: Rect, extent: (u32, u32)) -> Rect {
    if extent.0 == 0 || extent.1 == 0 || outer.width() <= 0.0 || outer.height() <= 0.0 {
        return outer;
    }
    let target = extent.0 as f32 / extent.1 as f32;
    let panel = outer.width() / outer.height();
    if panel > target {
        let width = outer.height() * target;
        Rect::from_center_size(outer.center(), Vec2::new(width, outer.height()))
    } else {
        let height = outer.width() / target;
        Rect::from_center_size(outer.center(), Vec2::new(outer.width(), height))
    }
}

pub(super) fn sprite_bounds(
    doc: &prism_editor_core::SceneDoc,
    index: usize,
    host: Option<&EngineHost>,
) -> Option<vm::PickBounds> {
    const SPRITE: &str = "prism_engine::scene::SpriteRenderer";
    let sprite = doc.entities.get(index)?.component(SPRITE)?;
    let path = sprite.get("sprite_path")?.as_str()?;
    if path.is_empty() {
        return None;
    }
    let frame = sprite.get("frame_size");
    let mut frame_w = frame
        .and_then(|value| value.get(0))
        .and_then(|value| value.as_f64())
        .unwrap_or(0.0) as f32;
    let mut frame_h = frame
        .and_then(|value| value.get(1))
        .and_then(|value| value.as_f64())
        .unwrap_or(0.0) as f32;
    if frame_w <= 0.0 || frame_h <= 0.0 {
        let size = host.and_then(|host| host.sprite_texture_size(index))?;
        frame_w = size[0];
        frame_h = size[1];
    }
    if frame_w <= 0.0 || frame_h <= 0.0 {
        return None;
    }
    let ppu = sprite
        .get("pixels_per_unit")
        .and_then(|value| value.as_f64())
        .map(|value| value as f32)
        .filter(|value| *value > 0.0)
        .unwrap_or(100.0);
    let pivot = sprite.get("pivot");
    let pivot_x = pivot
        .and_then(|value| value.get(0))
        .and_then(|value| value.as_f64())
        .unwrap_or(0.5) as f32;
    let pivot_y = pivot
        .and_then(|value| value.get(1))
        .and_then(|value| value.as_f64())
        .unwrap_or(0.5) as f32;
    let width = frame_w / ppu;
    let height = frame_h / ppu;
    Some(vm::PickBounds {
        min: [-pivot_x * width, -pivot_y * height, -0.05],
        max: [(1.0 - pivot_x) * width, (1.0 - pivot_y) * height, 0.05],
    })
}

pub(super) fn collider_bounds(
    doc: &prism_editor_core::SceneDoc,
    index: usize,
) -> Option<vm::PickBounds> {
    const COLLIDER: &str = "prism_engine::scene::Collider";
    let collider = doc.entities.get(index)?.component(COLLIDER)?;
    let number = |value: Option<&serde_json::Value>, default: f32| {
        value
            .and_then(serde_json::Value::as_f64)
            .map(|value| value as f32)
            .unwrap_or(default)
    };
    let center = [0usize, 1, 2].map(|axis| {
        number(
            collider.get("center").and_then(|center| center.get(axis)),
            0.0,
        )
    });
    let shape = collider
        .get("shape")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Box");
    let (half_x, half_y, half_z) = match shape {
        "Sphere" => {
            let radius = number(collider.get("radius"), 0.5).max(1.0e-4);
            (radius, radius, radius)
        }
        "Capsule" => {
            let radius = number(collider.get("radius"), 0.5).max(1.0e-4);
            let half_height = number(collider.get("half_height"), 0.5).max(0.0);
            (radius, half_height + radius, radius)
        }
        _ => {
            let half = collider.get("half_extents");
            (
                number(half.and_then(|value| value.get(0)), 0.5).max(1.0e-4),
                number(half.and_then(|value| value.get(1)), 0.5).max(1.0e-4),
                number(half.and_then(|value| value.get(2)), 0.5).max(1.0e-4),
            )
        }
    };
    Some(vm::PickBounds {
        min: [center[0] - half_x, center[1] - half_y, center[2] - half_z],
        max: [center[0] + half_x, center[1] + half_y, center[2] + half_z],
    })
}

pub(crate) fn bounds_table(
    workspace: &Workspace,
    host: Option<&EngineHost>,
) -> Vec<Option<vm::PickBounds>> {
    let doc = &workspace.document().doc;
    (0..doc.entities.len())
        .map(|index| {
            collider_bounds(doc, index)
                .or_else(|| sprite_bounds(doc, index, host))
                .or_else(|| {
                    host.and_then(|host| host.mesh_bounds(index))
                        .map(|(min, max)| vm::PickBounds { min, max })
                })
        })
        .collect()
}

fn entity_active_in_hierarchy(doc: &prism_editor_core::SceneDoc, mut index: usize) -> bool {
    let mut depth = 0;
    while depth <= 64 {
        let Some(entity) = doc.entities.get(index) else {
            return false;
        };
        if !entity.active() {
            return false;
        }
        let Some(parent_id) = entity.parent else {
            return true;
        };
        let Some(parent_idx) = doc.index_of_id(parent_id) else {
            return true;
        };
        index = parent_idx;
        depth += 1;
    }
    true
}

pub(super) fn proxies(
    workspace: &Workspace,
    host: Option<&EngineHost>,
) -> Vec<(usize, TransformJson, Option<vm::PickBounds>)> {
    let doc = &workspace.document().doc;
    let hidden: std::collections::HashSet<u32> =
        workspace.hidden_entity_ids().into_iter().collect();
    let bounds = bounds_table(workspace, host);
    (0..doc.entities.len())
        .filter(|index| {
            entity_active_in_hierarchy(doc, *index)
                && !doc
                    .id_of_index(*index)
                    .is_some_and(|id| hidden.contains(&id))
        })
        .map(|index| {
            let bounds = bounds.get(index).copied().flatten();
            (
                index,
                TransformJson {
                    position: vm::world_position(doc, index),
                    rotation: vm::world_rotation(doc, index),
                    scale: vm::world_scale(doc, index),
                },
                bounds,
            )
        })
        .collect()
}

pub(crate) fn proxy_transforms(workspace: &Workspace) -> Vec<(usize, TransformJson)> {
    proxies(workspace, None)
        .into_iter()
        .map(|(index, transform, _)| (index, transform))
        .collect()
}

pub(crate) fn pick(
    workspace: &Workspace,
    host: Option<&EngineHost>,
    point: (f32, f32),
    size: egui::Vec2,
    frame: &vm::CameraFrame,
) -> Option<usize> {
    pick_candidates(workspace, host, point, size, frame)
        .into_iter()
        .next()
}

pub(crate) fn pick_candidates(
    workspace: &Workspace,
    host: Option<&EngineHost>,
    point: (f32, f32),
    size: egui::Vec2,
    frame: &vm::CameraFrame,
) -> Vec<usize> {
    vm::pick_entities_with_bounds(point, frame, size.x, size.y, &proxies(workspace, host))
}

pub(super) fn engine_key(key: egui::Key) -> Option<KeyCode> {
    use egui::Key as EguiKey;
    use prism_engine::input::KeyCode as EngineKey;
    Some(match key {
        EguiKey::W => EngineKey::KeyW,
        EguiKey::A => EngineKey::KeyA,
        EguiKey::S => EngineKey::KeyS,
        EguiKey::D => EngineKey::KeyD,
        EguiKey::Q => EngineKey::KeyQ,
        EguiKey::E => EngineKey::KeyE,
        EguiKey::T => EngineKey::KeyT,
        EguiKey::B => EngineKey::Other(b'B' as u32),
        EguiKey::C => EngineKey::Other(b'C' as u32),
        EguiKey::F => EngineKey::Other(b'F' as u32),
        EguiKey::G => EngineKey::Other(b'G' as u32),
        EguiKey::H => EngineKey::Other(b'H' as u32),
        EguiKey::I => EngineKey::Other(b'I' as u32),
        EguiKey::J => EngineKey::Other(b'J' as u32),
        EguiKey::K => EngineKey::Other(b'K' as u32),
        EguiKey::L => EngineKey::Other(b'L' as u32),
        EguiKey::M => EngineKey::Other(b'M' as u32),
        EguiKey::N => EngineKey::Other(b'N' as u32),
        EguiKey::O => EngineKey::Other(b'O' as u32),
        EguiKey::P => EngineKey::Other(b'P' as u32),
        EguiKey::R => EngineKey::Other(b'R' as u32),
        EguiKey::U => EngineKey::Other(b'U' as u32),
        EguiKey::V => EngineKey::Other(b'V' as u32),
        EguiKey::X => EngineKey::Other(b'X' as u32),
        EguiKey::Y => EngineKey::Other(b'Y' as u32),
        EguiKey::Z => EngineKey::Other(b'Z' as u32),
        EguiKey::Space => EngineKey::Space,
        EguiKey::Escape => EngineKey::Escape,
        EguiKey::Tab => EngineKey::Tab,
        EguiKey::Enter => EngineKey::Enter,
        EguiKey::Backspace => EngineKey::Other(0x08),
        EguiKey::PageUp => EngineKey::Other(0x21),
        EguiKey::PageDown => EngineKey::Other(0x22),
        EguiKey::End => EngineKey::Other(0x23),
        EguiKey::Home => EngineKey::Other(0x24),
        EguiKey::Delete => EngineKey::Other(0x2E),
        EguiKey::Semicolon => EngineKey::Other(0xBA),
        EguiKey::Equals => EngineKey::Other(0xBB),
        EguiKey::Comma => EngineKey::Other(0xBC),
        EguiKey::Minus => EngineKey::Other(0xBD),
        EguiKey::Period => EngineKey::Other(0xBE),
        EguiKey::Slash => EngineKey::Other(0xBF),
        EguiKey::Backtick => EngineKey::Other(0xC0),
        EguiKey::F3 => EngineKey::F3,
        EguiKey::ArrowUp => EngineKey::ArrowUp,
        EguiKey::ArrowDown => EngineKey::ArrowDown,
        EguiKey::ArrowLeft => EngineKey::ArrowLeft,
        EguiKey::ArrowRight => EngineKey::ArrowRight,
        EguiKey::Num0 => EngineKey::Digit0,
        EguiKey::Num1 => EngineKey::Digit1,
        EguiKey::Num2 => EngineKey::Digit2,
        EguiKey::Num3 => EngineKey::Digit3,
        EguiKey::Num4 => EngineKey::Digit4,
        EguiKey::Num5 => EngineKey::Digit5,
        EguiKey::Num6 => EngineKey::Digit6,
        EguiKey::Num7 => EngineKey::Digit7,
        EguiKey::Num8 => EngineKey::Digit8,
        EguiKey::Num9 => EngineKey::Digit9,
        _ => return None,
    })
}

impl ViewportState {
    pub(super) fn forward_game_input(
        &mut self,
        ui: &egui::Ui,
        image_rect: Rect,
        response: &egui::Response,
        keyboard_active: bool,
    ) {
        if self.kind() != ViewKind::Game || !self.host.is_simulating() {
            self.reset_forwarded_input();
            return;
        }

        let allow_keys = keyboard_active && !ui.ctx().wants_keyboard_input();
        let (
            mut keys_down,
            modifiers,
            hover_pos,
            primary_down,
            secondary_down,
            middle_down,
            scroll_y,
        ) = ui.input(|input| {
            let keys = if allow_keys {
                input
                    .keys_down
                    .iter()
                    .filter_map(|key| engine_key(*key))
                    .collect::<HashSet<_>>()
            } else {
                HashSet::new()
            };
            (
                keys,
                input.modifiers,
                input.pointer.hover_pos(),
                input.pointer.primary_down(),
                input.pointer.secondary_down(),
                input.pointer.middle_down(),
                input.raw_scroll_delta.y,
            )
        });

        if allow_keys {
            if modifiers.shift {
                keys_down.insert(KeyCode::ShiftLeft);
            }
            if modifiers.ctrl {
                keys_down.insert(KeyCode::ControlLeft);
            }
            if modifiers.alt {
                keys_down.insert(KeyCode::AltLeft);
            }
        }

        let dragging =
            self.forwarded_left_down || self.forwarded_right_down || self.forwarded_middle_down;
        let pointer_in_image = hover_pos.is_some_and(|pos| image_rect.contains(pos));
        if let Some(position) = hover_pos {
            if pointer_in_image || dragging {
                let local = game_cursor(image_rect, position, self.extent);
                let pos = [local[0] as f64, local[1] as f64];
                if self.forwarded_cursor_inside || dragging {
                    self.host.input_mut().handle_mouse_move(pos);
                } else {
                    self.host.input_mut().set_mouse_position(pos);
                }
                self.forwarded_cursor_inside = pointer_in_image;
            } else {
                self.forwarded_cursor_inside = false;
            }
        } else {
            self.forwarded_cursor_inside = false;
        }

        if response.hovered() && pointer_in_image && scroll_y.abs() > 0.0 {
            self.host.input_mut().handle_scroll(scroll_y as f64);
        }

        let hovered = response.hovered();
        for (button, raw_down, tracked) in [
            (
                MouseButton::Left,
                primary_down,
                &mut self.forwarded_left_down,
            ),
            (
                MouseButton::Right,
                secondary_down,
                &mut self.forwarded_right_down,
            ),
            (
                MouseButton::Middle,
                middle_down,
                &mut self.forwarded_middle_down,
            ),
        ] {
            let held = if *tracked {
                raw_down
            } else {
                raw_down && hovered && pointer_in_image
            };
            if held != *tracked {
                let state = if held {
                    ElementState::Pressed
                } else {
                    ElementState::Released
                };
                self.host.input_mut().handle_mouse_button(button, state);
                *tracked = held;
            }
        }

        let pressed: Vec<KeyCode> = keys_down
            .difference(&self.forwarded_keys)
            .copied()
            .collect();
        let released: Vec<KeyCode> = self
            .forwarded_keys
            .difference(&keys_down)
            .copied()
            .collect();
        for key in pressed {
            self.host
                .input_mut()
                .handle_keyboard(key, ElementState::Pressed);
        }
        for key in released {
            self.host
                .input_mut()
                .handle_keyboard(key, ElementState::Released);
        }
        self.forwarded_keys = keys_down;
    }
}
