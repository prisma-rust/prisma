//! MCP control server polling, UI snapshots, and automation actions.

use super::*;

pub(crate) struct PendingCapture {
    pub(crate) target: crate::screenshot::CaptureTarget,
    pub(crate) path: PathBuf,
    pub(crate) size: Option<[u32; 2]>,
    pub(crate) ready_at: u64,
    pub(crate) reply: std::sync::mpsc::Sender<serde_json::Value>,
}

pub(super) fn main_window_hwnd(window: &egui_host::winit::window::Window) -> Option<isize> {
    #[cfg(target_os = "windows")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let handle = window.window_handle().ok()?;
        match handle.as_raw() {
            RawWindowHandle::Win32(win32) => Some(win32.hwnd.get()),
            _ => None,
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = window;
        None
    }
}

pub(crate) enum InjectedInput {
    Key(prism_engine::input::KeyCode, bool),
    Mouse(prism_engine::input::MouseButton, bool),

    Cursor(f32, f32),

    Scroll(f64),
}

impl InjectedInput {
    pub(super) fn apply(self, input: &mut prism_engine::input::InputManager) {
        use prism_engine::input::ElementState;
        match self {
            Self::Key(key, pressed) => input.handle_keyboard(
                key,
                if pressed {
                    ElementState::Pressed
                } else {
                    ElementState::Released
                },
            ),
            Self::Mouse(button, pressed) => input.handle_mouse_button(
                button,
                if pressed {
                    ElementState::Pressed
                } else {
                    ElementState::Released
                },
            ),
            Self::Cursor(x, y) => input.set_mouse_position([x as f64, y as f64]),
            Self::Scroll(delta) => input.handle_scroll(delta),
        }
    }
}

pub(super) fn parse_offset(value: &serde_json::Value) -> Option<[f32; 2]> {
    let array = value.as_array()?;
    if array.len() != 2 {
        return None;
    }
    let x = array[0].as_f64()? as f32;
    let y = array[1].as_f64()? as f32;
    Some([x, y])
}

pub(super) fn parse_key(name: &str) -> Option<prism_engine::input::KeyCode> {
    use prism_engine::input::KeyCode;
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "w" => KeyCode::KeyW,
        "a" => KeyCode::KeyA,
        "s" => KeyCode::KeyS,
        "d" => KeyCode::KeyD,
        "q" => KeyCode::KeyQ,
        "e" => KeyCode::KeyE,
        "space" | " " => KeyCode::Space,
        "shift" | "shiftleft" => KeyCode::ShiftLeft,
        "shiftright" => KeyCode::ShiftRight,
        "ctrl" | "control" | "controlleft" => KeyCode::ControlLeft,
        "controlright" => KeyCode::ControlRight,
        "alt" | "altleft" => KeyCode::AltLeft,
        "altright" => KeyCode::AltRight,
        "escape" | "esc" => KeyCode::Escape,
        "tab" => KeyCode::Tab,
        "enter" | "return" => KeyCode::Enter,
        "up" | "arrowup" => KeyCode::ArrowUp,
        "down" | "arrowdown" => KeyCode::ArrowDown,
        "left" | "arrowleft" => KeyCode::ArrowLeft,
        "right" | "arrowright" => KeyCode::ArrowRight,
        "0" => KeyCode::Digit0,
        "1" => KeyCode::Digit1,
        "2" => KeyCode::Digit2,
        "3" => KeyCode::Digit3,
        "4" => KeyCode::Digit4,
        "5" => KeyCode::Digit5,
        "6" => KeyCode::Digit6,
        "7" => KeyCode::Digit7,
        "8" => KeyCode::Digit8,
        "9" => KeyCode::Digit9,
        _ => return None,
    })
}

pub(super) fn parse_mouse(name: &str) -> Option<prism_engine::input::MouseButton> {
    use prism_engine::input::MouseButton;
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "left" => MouseButton::Left,
        "right" => MouseButton::Right,
        "middle" => MouseButton::Middle,
        _ => return None,
    })
}

pub(super) fn scene_entity_visible(entity: &prism_editor_core::EntityJson) -> bool {
    match entity.components.get("prism_engine::scene::Active") {
        Some(serde_json::Value::Bool(value)) => *value,

        Some(value) => value
            .get("0")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
        None => true,
    }
}

pub(super) fn game_camera_frame(
    world: &prism_engine::ecs::World,
) -> Option<prism_editor_core::viewport_math::CameraFrame> {
    use prism_engine::scene::components::{Camera, LocalToWorld};
    use prism_engine::scene::systems::camera;

    let entity = camera::active_camera(world)?;
    let camera = world.get::<Camera>(entity)?;
    let matrix = world.get::<LocalToWorld>(entity)?.0;
    let eye = matrix.w_axis.truncate();
    let right = matrix.x_axis.truncate().normalize_or_zero();
    let up = matrix.y_axis.truncate().normalize_or_zero();
    let forward = -matrix.z_axis.truncate().normalize_or_zero();
    Some(prism_editor_core::viewport_math::CameraFrame {
        fov_y_degrees: 60.0,
        eye: eye.to_array(),
        right: right.to_array(),
        up: up.to_array(),
        forward: forward.to_array(),
        ortho_half_height: camera
            .orthographic
            .then(|| camera.orthographic_size.max(1e-4)),
    })
}

pub(super) fn game_view_objects(
    view: &ViewportState,
    rect: egui::Rect,
) -> Vec<crate::ui_tree::ViewObject> {
    use prism_engine::scene::components::{LocalToWorld, Name};
    use prism_engine::scene::runtime_components::SceneEntityId;
    use prism_engine::scene::systems::hierarchy::is_active_in_hierarchy;

    let world = view.host.world();
    let Some(frame) = game_camera_frame(world) else {
        return Vec::new();
    };
    let mut objects: Vec<crate::ui_tree::ViewObject> = world
        .query::<LocalToWorld>()
        .filter_map(|(entity, matrix)| {
            let name = world.get::<Name>(entity)?.0.clone();
            let world_pos = matrix.0.w_axis.truncate().to_array();
            let projected = prism_editor_core::viewport_math::project(
                world_pos,
                &frame,
                rect.width(),
                rect.height(),
            );
            let scene_id = world.get::<SceneEntityId>(entity).map(|id| id.0);
            Some(crate::ui_tree::ViewObject {
                node: match scene_id {
                    Some(id) => crate::ui_tree::view_object_node("gameView", id),
                    None => crate::ui_tree::view_object_node_ecs("gameView", entity.id()),
                },
                entity: scene_id,
                name,
                world: world_pos,
                screen: projected.map(|(x, y, _)| [x, y]),
                depth: projected.map(|(_, _, depth)| depth),
                visible: is_active_in_hierarchy(world, entity),
            })
        })
        .collect();
    objects.sort_by(|a, b| a.node.cmp(&b.node));
    objects
}

impl EditorApp {
    pub(super) fn sync_control_channel(&mut self) {
        let want = !self.control_cli_disabled && self.preferences.control_channel;
        if want == self.control_wanted {
            return;
        }
        self.control_wanted = want;
        if want {
            let dir = self
                .state_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            match crate::control::ControlServer::start(&dir) {
                Ok(server) => {
                    self.control = Some(server);
                    self.control_error = None;
                    self.state.status = self.t("desktop.status.controlEnabled");
                }
                Err(error) => {
                    log::warn!("control channel failed to start: {error:#}");
                    self.control_error = Some(format!("{error:#}"));
                }
            }
        } else {
            self.control = None;
            self.control_error = None;
            log::info!("control channel disabled");
            self.state.status = self.t("desktop.status.controlDisabled");
        }
    }

    pub(super) fn poll_control(&mut self, ctx: &egui::Context) {
        self.sync_control_channel();
        if let Some(control) = self.control.as_ref() {
            for request in control.drain() {
                if let Some(response) =
                    self.handle_command(ctx, &request.command, &request.args, request.reply.clone())
                {
                    let _ = request.reply.send(response);
                }
            }
        }

        let due: Vec<usize> = self
            .pending_captures
            .iter()
            .enumerate()
            .filter(|(_, pending)| self.frame_count >= pending.ready_at)
            .map(|(index, _)| index)
            .collect();
        for index in due.into_iter().rev() {
            let pending = self.pending_captures.remove(index);
            match pending.target {
                crate::screenshot::CaptureTarget::Window => {
                    let Some(hwnd) = self.hwnd else {
                        let _ = pending.reply.send(crate::control::failed(
                            crate::control::Kind::Denied,
                            "window capture unavailable on this platform",
                        ));
                        continue;
                    };
                    let path = pending.path.clone();
                    let size = pending.size;
                    let reply = pending.reply;
                    std::thread::Builder::new()
                        .name("editor-window-capture".to_owned())
                        .spawn(move || {
                            let response =
                                match crate::screenshot::save_window_png(hwnd, &path, size) {
                                    Ok(path) => serde_json::json!({
                                        "ok": true,
                                        "path": path.display().to_string(),
                                        "target": "window",
                                    }),
                                    Err(error) => crate::control::failed(
                                        crate::control::Kind::Internal,
                                        format!("{error:#}"),
                                    ),
                                };
                            let _ = reply.send(response);
                        })
                        .ok();
                    self.state.status = self.tf(
                        "desktop.status.screenshot",
                        &[("path", &pending.path.display().to_string())],
                    );
                }
                target => {
                    let response = match self.capture(target, &pending.path, pending.size) {
                        Ok(path) => serde_json::json!({
                            "ok": true,
                            "path": path.display().to_string(),
                            "target": target.name(),
                        }),
                        Err(error) => crate::control::failed(
                            crate::control::Kind::Internal,
                            format!("{error:#}"),
                        ),
                    };
                    let _ = pending.reply.send(response);
                }
            }
        }
        if !self.pending_captures.is_empty() {
            ctx.request_repaint();
        }
    }

    pub(super) fn dialogs(&self) -> Vec<crate::ui_tree::Dialog> {
        use crate::ui_tree::Dialog;
        let mut out = Vec::new();

        if let Some(indices) = &self.state.pending_entity_delete {
            let message = if indices.len() == 1 {
                self.t("desktop.dialog.deleteEntity")
            } else {
                self.tf(
                    "desktop.dialog.deleteEntities",
                    &[("count", &indices.len().to_string())],
                )
            };
            out.push(
                Dialog::new(
                    "delete-entities",
                    self.t("desktop.dialog.deleteTitle"),
                    message,
                )
                .button("delete", &self.t("common.delete"))
                .button("cancel", &self.t("common.cancel")),
            );
        }

        if let Some(path) = &self.state.pending_doc_delete {
            out.push(
                Dialog::new(
                    "delete-document",
                    self.t("desktop.dialog.deleteDocumentTitle"),
                    self.tf("desktop.dialog.deleteDocument", &[("path", path)]),
                )
                .button("delete", &self.t("common.delete"))
                .button("cancel", &self.t("common.cancel")),
            );
        }

        if let Some(paths) = &self.state.pending_asset_delete {
            let message = match paths.as_slice() {
                [one] => self.tf("desktop.dialog.deleteAsset", &[("path", one)]),
                many => self.tf(
                    "desktop.dialog.deleteAssets",
                    &[("count", &many.len().to_string())],
                ),
            };
            out.push(
                Dialog::new(
                    "delete-assets",
                    self.t("desktop.dialog.deleteAssetTitle"),
                    message,
                )
                .button("delete", &self.t("common.delete"))
                .button("cancel", &self.t("common.cancel")),
            );
        }

        if let Some(index) = self.state.pending_save_conflict {
            let (title, path) = self
                .workspace
                .docs
                .get(index)
                .map(|document| (document.title(), document.path.clone().unwrap_or_default()))
                .unwrap_or_else(|| (self.t("desktop.dialog.document"), String::new()));
            out.push(
                Dialog::new(
                    "save-conflict",
                    self.t("desktop.dialog.saveConflictTitle"),
                    self.tf(
                        "desktop.dialog.saveConflict",
                        &[("title", &title), ("path", &path)],
                    ),
                )
                .button("overwrite", &self.t("desktop.dialog.overwrite"))
                .button("reload", &self.t("material.reload"))
                .button("cancel", &self.t("common.cancel")),
            );
        }

        if let Some(target) = self.state.pending_close {
            let (message, detail) = match target {
                panels::CloseTarget::Document(index) => {
                    let title = self
                        .workspace
                        .docs
                        .get(index)
                        .map(|document| document.title())
                        .unwrap_or_else(|| self.t("desktop.dialog.document"));
                    (
                        self.tf("desktop.dialog.unsavedDoc", &[("title", &title)]),
                        self.t("desktop.dialog.closeAnyway"),
                    )
                }
                panels::CloseTarget::App => {
                    let count = self
                        .workspace
                        .docs
                        .iter()
                        .filter(|document| document.is_dirty())
                        .count();
                    (
                        self.tf(
                            "desktop.dialog.unsavedDocs",
                            &[("count", &count.to_string())],
                        ),
                        self.t("desktop.dialog.quitAnyway"),
                    )
                }
            };
            out.push(
                Dialog::new(
                    "close",
                    self.t("desktop.dialog.unsavedTitle"),
                    format!("{message} {detail}"),
                )
                .button("save", &self.t("common.save"))
                .button("discard", &self.t("desktop.dialog.dontSave"))
                .button("cancel", &self.t("common.cancel")),
            );
        }

        if let Some(confirm) = &self.state.confirm {
            out.push(
                Dialog::new("confirm", confirm.title.clone(), confirm.message.clone())
                    .button("ok", &confirm.ok)
                    .button("cancel", &self.t("common.cancel")),
            );
        }

        if let Some(prompt) = &self.state.prompt {
            out.push(
                Dialog::new(
                    "prompt",
                    prompt.title.clone(),
                    self.tf(
                        "desktop.dialog.promptValue",
                        &[("hint", &prompt.hint), ("value", &prompt.value)],
                    ),
                )
                .button("ok", &self.t("common.ok"))
                .button("cancel", &self.t("common.cancel"))
                .value(prompt.value.clone()),
            );
        }

        out
    }

    pub(super) fn viewport_objects(&self) -> Vec<crate::ui_tree::ViewObjects> {
        use crate::ui_tree::{ViewObject, ViewObjects};
        use prism_editor_core::viewport_math as vm;

        let mut out = Vec::new();

        if let Some(rect) = self.panel_rects.get("sceneView").copied() {
            let frame = vm::camera_frame(&self.workspace.camera);
            let doc = &self.workspace.document().doc;
            let objects = (0..doc.entities.len())
                .map(|index| {
                    let entity = &doc.entities[index];
                    let world = vm::world_position(doc, index);
                    let projected = vm::project(world, &frame, rect.width(), rect.height());
                    ViewObject {
                        node: crate::ui_tree::view_object_node(
                            "sceneView",
                            entity.id.unwrap_or(index as u32),
                        ),
                        entity: Some(entity.id.unwrap_or(index as u32)),
                        name: entity.name.clone().unwrap_or_default(),
                        world,
                        screen: projected.map(|(x, y, _)| [x, y]),
                        depth: projected.map(|(_, _, depth)| depth),
                        visible: scene_entity_visible(entity),
                    }
                })
                .collect();
            out.push(ViewObjects {
                tab: "sceneView",
                objects,
            });
        }

        if let (Some(view), Some(rect)) = (
            self.game_view.as_ref(),
            self.panel_rects.get("gameView").copied(),
        ) {
            out.push(ViewObjects {
                tab: "gameView",
                objects: game_view_objects(view, rect),
            });
        }

        out
    }

    pub(super) fn ui_click_view_object(
        &mut self,
        tab: &str,
        node: &str,
        offset: Option<[f32; 2]>,
    ) -> Result<serde_json::Value, crate::control::ControlError> {
        let objects = self.viewport_objects();
        let view = objects.iter().find(|view| view.tab == tab).ok_or_else(|| {
            crate::control::ControlError::not_ready(format!("viewport '{tab}' is not open"))
        })?;
        let object = view
            .objects
            .iter()
            .find(|object| object.node == node)
            .ok_or_else(|| format!("'{node}' is not in view '{tab}'"))?;
        let screen = object
            .screen
            .ok_or_else(|| format!("'{node}' is behind the camera"))?;
        let [dx, dy] = offset.unwrap_or([0.0, 0.0]);
        self.viewport_click(tab, (screen[0] + dx, screen[1] + dy))
    }

    pub(super) fn ui_click_panel_offset(
        &mut self,
        node: &str,
        offset: [f32; 2],
    ) -> Result<serde_json::Value, crate::control::ControlError> {
        let tab = node
            .strip_prefix("panel:")
            .ok_or_else(|| format!("offset clicks need a viewport panel, got '{node}'"))?;
        if tab != "sceneView" && tab != "gameView" {
            return Err(format!(
                "panel '{tab}' has no viewport to click in (only sceneView / gameView)"
            )
            .into());
        }
        self.viewport_click(tab, (offset[0], offset[1]))
    }

    pub(super) fn viewport_click(
        &mut self,
        tab: &str,
        local: (f32, f32),
    ) -> Result<serde_json::Value, crate::control::ControlError> {
        let rect = self.panel_rects.get(tab).copied().ok_or_else(|| {
            crate::control::ControlError::not_ready(format!(
                "viewport '{tab}' was not laid out yet"
            ))
        })?;
        if !local.0.is_finite() || !local.1.is_finite() {
            return Err("offset must be finite".into());
        }
        match tab {
            "sceneView" => {
                if local.0 < 0.0
                    || local.1 < 0.0
                    || local.0 > rect.width()
                    || local.1 > rect.height()
                {
                    return Err(format!(
                        "({:.1},{:.1}) is outside the scene view ({}x{})",
                        local.0,
                        local.1,
                        rect.width(),
                        rect.height()
                    )
                    .into());
                }
                let frame = prism_editor_core::viewport_math::camera_frame(&self.workspace.camera);
                let hit = {
                    let host = self.scene_view.as_ref().map(|view| &view.host);
                    crate::viewport::pick(&self.workspace, host, local, rect.size(), &frame)
                };
                let hit_node = hit.map(|index| {
                    crate::ui_tree::view_object_node(
                        tab,
                        self.workspace.document().doc.entities[index]
                            .id
                            .unwrap_or(index as u32),
                    )
                });
                match hit {
                    Some(index) => self.workspace.select(index, false),
                    None => self.workspace.clear_selection(),
                }
                Ok(json!({
                    "anchor": "top-left",
                    "at": [local.0, local.1],
                    "hit": hit_node,
                    "action": match hit {
                        Some(_) => "select",
                        None => "clear-selection",
                    },
                }))
            }
            "gameView" => {
                let cursor = self.game_graphic_cursor(local)?;
                self.pending_input
                    .push_back(InjectedInput::Cursor(cursor.0, cursor.1));
                self.pending_input.push_back(InjectedInput::Mouse(
                    prism_engine::input::MouseButton::Left,
                    true,
                ));
                self.pending_input.push_back(InjectedInput::Mouse(
                    prism_engine::input::MouseButton::Left,
                    false,
                ));
                Ok(json!({
                    "anchor": "top-left",
                    "at": [local.0, local.1],
                    "cursor": [cursor.0, cursor.1],
                    "queued": 3,
                }))
            }
            other => Err(format!("unknown viewport '{other}'").into()),
        }
    }

    pub(super) fn scene_rect(&self) -> Result<egui::Rect, String> {
        self.panel_rects
            .get("sceneView")
            .copied()
            .ok_or_else(|| "scene view was not laid out yet".to_string())
    }

    pub(super) fn viewport_point(
        &self,
        args: &serde_json::Value,
    ) -> Result<(String, (f32, f32), &'static str), crate::control::ControlError> {
        if let Some(node) = args.get("node").and_then(serde_json::Value::as_str) {
            let offset = args
                .get("offset")
                .and_then(parse_offset)
                .unwrap_or([0.0, 0.0]);
            if let Some(tab) = node.strip_prefix("panel:") {
                if !self.panel_rects.contains_key(tab) {
                    return Err(crate::control::ControlError::not_ready(format!(
                        "panel '{tab}' was not laid out yet"
                    )));
                }
                return Ok((tab.to_string(), (offset[0], offset[1]), "top-left"));
            }
            let tab = crate::ui_tree::parse_view_object_node(node)
                .ok_or_else(|| format!("'{node}' is not a viewport object node"))?;
            let objects = self.viewport_objects();
            let view = objects.iter().find(|view| view.tab == tab).ok_or_else(|| {
                crate::control::ControlError::not_ready(format!("viewport '{tab}' is not open"))
            })?;
            let object = view
                .objects
                .iter()
                .find(|object| object.node == node)
                .ok_or_else(|| format!("'{node}' is not in view '{tab}'"))?;
            let screen = object
                .screen
                .ok_or_else(|| format!("'{node}' is behind the camera"))?;
            return Ok((
                tab.to_string(),
                (screen[0] + offset[0], screen[1] + offset[1]),
                "center",
            ));
        }
        let tab = args
            .get("tab")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("sceneView")
            .to_string();
        let x = args.get("x").and_then(serde_json::Value::as_f64);
        let y = args.get("y").and_then(serde_json::Value::as_f64);
        match (x, y) {
            (Some(x), Some(y)) => Ok((tab, (x as f32, y as f32), "top-left")),
            _ => Err("need node+offset or x/y".into()),
        }
    }

    pub(super) fn game_graphic_cursor(
        &self,
        local: (f32, f32),
    ) -> Result<(f32, f32), crate::control::ControlError> {
        let rect = self.panel_rects.get("gameView").copied().ok_or_else(|| {
            crate::control::ControlError::not_ready("game view was not laid out yet")
        })?;
        let view = self
            .game_view
            .as_ref()
            .ok_or_else(|| crate::control::ControlError::not_ready("game view is not open"))?;
        let extent = view.host.size();
        let image = crate::viewport::letterbox(rect, extent);
        let position = egui::pos2(rect.min.x + local.0, rect.min.y + local.1);
        let cursor = crate::viewport::game_cursor(image, position, extent);
        Ok((cursor[0], cursor[1]))
    }

    pub(super) fn ui_view_move(
        &mut self,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, crate::control::ControlError> {
        let (tab, local, anchor) = self.viewport_point(args)?;
        match tab.as_str() {
            "sceneView" => {
                let rect = self.scene_rect()?;
                if local.0 < 0.0
                    || local.1 < 0.0
                    || local.0 > rect.width()
                    || local.1 > rect.height()
                {
                    return Err(format!(
                        "({:.1},{:.1}) is outside the scene view ({}x{})",
                        local.0,
                        local.1,
                        rect.width(),
                        rect.height()
                    )
                    .into());
                }
                let frame = prism_editor_core::viewport_math::camera_frame(&self.workspace.camera);
                let hit = {
                    let host = self.scene_view.as_ref().map(|view| &view.host);
                    crate::viewport::pick(&self.workspace, host, local, rect.size(), &frame)
                };
                let hit_node = hit.map(|index| {
                    crate::ui_tree::view_object_node(
                        "sceneView",
                        self.workspace.document().doc.entities[index]
                            .id
                            .unwrap_or(index as u32),
                    )
                });
                let gizmo_axis = self
                    .scene_view
                    .as_ref()
                    .and_then(|view| view.gizmo_hit_at(&self.workspace, &frame, rect, local))
                    .and_then(|(handle, _, _)| match handle {
                        prism_editor_core::viewport_math::GizmoHandle::Axis(axis) => Some(axis),
                        _ => None,
                    });
                Ok(json!({
                    "at": [local.0, local.1],
                    "anchor": anchor,
                    "hit": hit_node,
                    "gizmo_axis": gizmo_axis,
                }))
            }
            "gameView" => {
                let cursor = self.game_graphic_cursor(local)?;
                self.pending_input
                    .push_back(InjectedInput::Cursor(cursor.0, cursor.1));
                Ok(json!({
                    "at": [local.0, local.1],
                    "anchor": anchor,
                    "cursor": [cursor.0, cursor.1],
                    "queued": 1,
                }))
            }
            other => Err(format!("'{other}' is not a viewport panel").into()),
        }
    }

    pub(super) fn ui_view_wheel(
        &mut self,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, crate::control::ControlError> {
        let (tab, local, anchor) = self.viewport_point(args)?;
        let delta = args
            .get("delta")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(120.0) as f32;
        if !delta.is_finite() {
            return Err("delta must be finite".into());
        }
        match tab.as_str() {
            "sceneView" => {
                let rect = self.scene_rect()?;
                let frame = prism_editor_core::viewport_math::camera_frame(&self.workspace.camera);
                let (origin, direction) = prism_editor_core::viewport_math::ray_from_screen(
                    local.0,
                    local.1,
                    &frame,
                    rect.width(),
                    rect.height(),
                );
                let distance = self.workspace.camera.distance;
                let anchor_point = [
                    origin[0] + direction[0] * distance,
                    origin[1] + direction[1] * distance,
                    origin[2] + direction[2] * distance,
                ];
                self.workspace
                    .camera
                    .zoom_towards(delta, Some(anchor_point));
                if let Some(view) = self.scene_view.as_mut() {
                    view.mark_dirty();
                }
                Ok(json!({
                    "at": [local.0, local.1],
                    "anchor": anchor,
                    "delta": delta,
                    "distance": self.workspace.camera.distance,
                    "action": "zoom",
                }))
            }
            "gameView" => {
                let cursor = self.game_graphic_cursor(local)?;
                self.pending_input
                    .push_back(InjectedInput::Cursor(cursor.0, cursor.1));
                self.pending_input
                    .push_back(InjectedInput::Scroll(delta as f64));
                Ok(json!({
                    "at": [local.0, local.1],
                    "anchor": anchor,
                    "delta": delta,
                    "cursor": [cursor.0, cursor.1],
                    "queued": 2,
                    "action": "scroll",
                }))
            }
            other => Err(format!("'{other}' is not a viewport panel").into()),
        }
    }

    pub(super) fn drag_point(
        &self,
        args: &serde_json::Value,
        key: &str,
    ) -> Result<(String, (f32, f32), &'static str), crate::control::ControlError> {
        let value = args
            .get(key)
            .filter(|value| value.is_object())
            .ok_or_else(|| format!("drag needs '{key}' as {{node,offset}} or {{x,y}}"))?;
        self.viewport_point(value)
    }

    pub(super) fn ui_view_drag(
        &mut self,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, crate::control::ControlError> {
        let (from_tab, from, _) = self.drag_point(args, "from")?;
        let (to_tab, to, _) = self.drag_point(args, "to")?;
        if from_tab != to_tab {
            return Err(
                format!("from/to must be in the same view ('{from_tab}' vs '{to_tab}')").into(),
            );
        }
        let steps = args
            .get("steps")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(8)
            .clamp(1, 120) as u32;
        match from_tab.as_str() {
            "sceneView" => self.drag_scene(from, to, steps, args),
            "gameView" => self.drag_game(from, to, steps),
            other => Err(format!("'{other}' is not a viewport panel").into()),
        }
    }

    pub(super) fn drag_scene(
        &mut self,
        from: (f32, f32),
        to: (f32, f32),
        steps: u32,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, crate::control::ControlError> {
        let rect = self.scene_rect()?;
        for (name, point) in [("from", from), ("to", to)] {
            if point.0 < 0.0 || point.1 < 0.0 || point.0 > rect.width() || point.1 > rect.height() {
                return Err(format!(
                    "{name} ({:.1},{:.1}) is outside the scene view ({}x{})",
                    point.0,
                    point.1,
                    rect.width(),
                    rect.height()
                )
                .into());
            }
        }
        let frame = prism_editor_core::viewport_math::camera_frame(&self.workspace.camera);
        let orbit = args.get("mode").and_then(serde_json::Value::as_str) == Some("orbit")
            || args.get("orbit").and_then(serde_json::Value::as_bool) == Some(true);
        let ctrl = args.get("ctrl").and_then(serde_json::Value::as_bool) == Some(true);
        let Some(view) = self.scene_view.as_mut() else {
            return Err(crate::control::ControlError::not_ready(
                "scene view is not open",
            ));
        };
        if !orbit && view.begin_gizmo_drag(&mut self.workspace, &frame, rect, from) {
            let mut changed = false;
            let mut previous = from;
            for current in crate::viewport::drag_samples(from, to, steps) {
                changed |= view.gizmo_drag_step(
                    &mut self.workspace,
                    &frame,
                    rect,
                    current,
                    (current.0 - previous.0, current.1 - previous.1),
                    ctrl,
                );
                previous = current;
            }
            view.finish_gizmo_drag(&mut self.workspace);
            return Ok(json!({
                "from": [from.0, from.1],
                "to": [to.0, to.1],
                "anchor": "top-left",
                "steps": steps,
                "changed": changed,
                "action": "gizmo",
            }));
        }
        if orbit {
            self.workspace.camera.orbit(to.0 - from.0, to.1 - from.1);
            view.mark_dirty();
            return Ok(json!({
                "from": [from.0, from.1],
                "to": [to.0, to.1],
                "anchor": "top-left",
                "steps": steps,
                "action": "orbit",
            }));
        }
        let hits = prism_editor_core::viewport_math::box_select(
            prism_editor_core::viewport_math::local_box(from, to, (0.0, 0.0)),
            &frame,
            rect.width(),
            rect.height(),
            &crate::viewport::proxy_transforms(&self.workspace),
        );
        let nodes: Vec<String> = hits
            .iter()
            .map(|index| {
                crate::ui_tree::view_object_node(
                    "sceneView",
                    self.workspace.document().doc.entities[*index]
                        .id
                        .unwrap_or(*index as u32),
                )
            })
            .collect();
        self.workspace.select_many(&hits, false);
        Ok(json!({
            "from": [from.0, from.1],
            "to": [to.0, to.1],
            "anchor": "top-left",
            "steps": steps,
            "selected": nodes,
            "count": hits.len(),
            "action": "box-select",
        }))
    }

    pub(super) fn drag_game(
        &mut self,
        from: (f32, f32),
        to: (f32, f32),
        steps: u32,
    ) -> Result<serde_json::Value, crate::control::ControlError> {
        let start = self.game_graphic_cursor(from)?;
        let end = self.game_graphic_cursor(to)?;
        self.pending_input
            .push_back(InjectedInput::Cursor(start.0, start.1));
        self.pending_input.push_back(InjectedInput::Mouse(
            prism_engine::input::MouseButton::Left,
            true,
        ));
        for cursor in crate::viewport::drag_samples(start, end, steps) {
            self.pending_input
                .push_back(InjectedInput::Cursor(cursor.0, cursor.1));
        }
        self.pending_input.push_back(InjectedInput::Mouse(
            prism_engine::input::MouseButton::Left,
            false,
        ));
        Ok(json!({
            "from": [from.0, from.1],
            "to": [to.0, to.1],
            "anchor": "top-left",
            "cursor": [[start.0, start.1], [end.0, end.1]],
            "steps": steps,
            "queued": steps + 3,
            "action": "game-pointer",
        }))
    }

    pub(super) fn ui_click(&mut self, node: &str) -> Result<String, String> {
        if let Some(name) = node.strip_prefix("command:") {
            let command = crate::ui_tree::command_by_name(name)
                .ok_or_else(|| format!("unknown command '{name}'"))?;
            self.apply_command(command);
            return Ok(format!("command {name}"));
        }
        if let Some(rest) = node.strip_prefix("doc:") {
            let index = rest
                .parse::<usize>()
                .map_err(|_| format!("bad document index '{rest}'"))?;
            if index >= self.workspace.docs.len() {
                return Err(format!("document {index} does not exist"));
            }
            self.workspace.set_active(index);
            return Ok(format!("activate document {index}"));
        }
        if let Some(rest) = node.strip_prefix("entity:") {
            let id = rest
                .parse::<u32>()
                .map_err(|_| format!("bad entity id '{rest}'"))?;
            let index = self
                .workspace
                .document()
                .doc
                .index_of_id(id)
                .ok_or_else(|| format!("entity {id} not found"))?;
            self.workspace.select(index, false);
            return Ok(format!("select entity {id}"));
        }
        if let Some(path) = node.strip_prefix("asset:") {
            self.state.select_asset_only(path.to_owned());
            return Ok(format!("select asset {path}"));
        }

        if let Some(path) = node.strip_prefix("open:") {
            self.workspace.open_path(path);
            self.state.status = self.tf("desktop.status.opening", &[("path", &path)]);
            return Ok(format!("open {path}"));
        }
        if let Some(tab) = node.strip_prefix("panel:") {
            let leaf = self
                .dock_leaves
                .iter()
                .find(|(_, _, tabs)| tabs.iter().any(|name| name == tab))
                .map(|(leaf, _, _)| leaf.clone());
            match leaf {
                Some(leaf) => self.workspace.activate_panel(&leaf, tab),
                None => self.workspace.toggle_panel(tab),
            }
            return Ok(format!("open panel {tab}"));
        }
        if node == "search:open" {
            self.palette.open(&self.workspace);
            return Ok("open search".to_string());
        }
        if let Some(rest) = node.strip_prefix("resolution:") {
            let target = if rest == "match" {
                GameResolution::MatchWindow
            } else if let Some(ratio) = rest.strip_prefix("fit:") {
                let (width, height) = ratio
                    .split_once('x')
                    .ok_or_else(|| format!("bad aspect '{ratio}' (want fit:16x9)"))?;
                GameResolution::Aspect {
                    width: width
                        .parse()
                        .map_err(|_| format!("bad aspect width '{width}'"))?,
                    height: height
                        .parse()
                        .map_err(|_| format!("bad aspect height '{height}'"))?,
                }
            } else {
                let (width, height) = rest
                    .split_once('x')
                    .ok_or_else(|| format!("bad resolution '{rest}' (want 1920x1080)"))?;
                GameResolution::Fixed {
                    width: width
                        .parse()
                        .map_err(|_| format!("bad resolution width '{width}'"))?,
                    height: height
                        .parse()
                        .map_err(|_| format!("bad resolution height '{height}'"))?,
                }
            };
            self.ensure_view(ViewKind::Game);
            if let Some(view) = self.game_view.as_mut() {
                view.target = target;
                view.mark_dirty();
            }
            return Ok(format!("game resolution {}", target.label()));
        }
        Err(format!(
            "unsupported node '{node}' (command:… / panel:… / doc:… / entity:… / asset:… / dialog:… / search:open / resolution:<WxH>|fit:<WxH>|match)"
        ))
    }

    pub(super) fn ui_click_dialog(
        &mut self,
        ctx: &egui::Context,
        node: &str,
    ) -> Result<serde_json::Value, String> {
        let (id, action) = crate::ui_tree::parse_dialog_node(node)
            .ok_or_else(|| format!("bad dialog node '{node}'"))?;
        let action = action.ok_or_else(|| format!("'{node}' 是对话框，不是按钮"))?;
        let label = match (id, action) {
            ("delete-entities", "delete") => {
                panels::apply_pending_entity_delete(&mut self.workspace, &mut self.state);
                "delete entities"
            }
            ("delete-entities", "cancel") => {
                self.state.pending_entity_delete = None;
                "cancel delete entities"
            }
            ("delete-document", "delete") => {
                panels::apply_pending_doc_delete(&mut self.workspace, &mut self.state);
                "delete document"
            }
            ("delete-document", "cancel") => {
                self.state.pending_doc_delete = None;
                "cancel delete document"
            }
            ("delete-assets", "delete") => {
                panels::apply_pending_asset_delete(&mut self.workspace, &mut self.state);
                "delete assets"
            }
            ("delete-assets", "cancel") => {
                self.state.pending_asset_delete = None;
                "cancel delete assets"
            }
            ("save-conflict", "overwrite") => {
                let index = self
                    .state
                    .pending_save_conflict
                    .ok_or_else(|| "no save conflict".to_string())?;
                self.apply_save_conflict_choice(index, SaveConflictChoice::Overwrite);
                "overwrite"
            }
            ("save-conflict", "reload") => {
                let index = self
                    .state
                    .pending_save_conflict
                    .ok_or_else(|| "no save conflict".to_string())?;
                self.apply_save_conflict_choice(index, SaveConflictChoice::Reload);
                "reload"
            }
            ("save-conflict", "cancel") => {
                self.state.pending_save_conflict = None;
                "cancel save conflict"
            }
            ("close", "save") => {
                let target = self
                    .state
                    .pending_close
                    .ok_or_else(|| "no pending close".to_string())?;
                self.apply_close_decision(ctx, target, CloseDecision::Save);
                "save and close"
            }
            ("close", "discard") => {
                let target = self
                    .state
                    .pending_close
                    .ok_or_else(|| "no pending close".to_string())?;
                self.apply_close_decision(ctx, target, CloseDecision::Discard);
                "discard and close"
            }
            ("close", "cancel") => {
                self.state.pending_close = None;
                "cancel close"
            }
            ("confirm", "ok") => {
                self.accept_confirm();
                "confirm"
            }
            ("confirm", "cancel") => {
                self.state.confirm = None;
                "cancel confirm"
            }
            ("prompt", "ok") => {
                let Some(prompt) = self.state.prompt.clone() else {
                    return Err("no prompt".into());
                };
                self.commit_prompt(prompt.kind, prompt.value.trim().to_string())?;
                "prompt ok"
            }
            ("prompt", "cancel") => {
                self.state.prompt = None;
                "cancel prompt"
            }
            _ => return Err(format!("unknown dialog button '{node}'")),
        };
        Ok(json!({
            "dialog": id,
            "button": action,
            "action": label,
            "pending_dialog": self.dialogs().first().map(|dialog| dialog.node()),
        }))
    }

    pub(super) fn run_palette_action(&mut self, action: prism_editor_core::palette::PaletteAction) {
        use prism_editor_core::palette::PaletteAction;
        match action {
            PaletteAction::RunCommand(command) => self.apply_command(command),
            PaletteAction::OpenDocument(index) => {
                self.workspace.set_active(index);
                self.state.status = self.tf(
                    "desktop.status.opened",
                    &[("path", &self.workspace.document().title())],
                );
            }
            PaletteAction::SelectEntity(index) => {
                let name = self.workspace.document().doc.entities[index]
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("entity {index}"));
                self.workspace.select(index, false);
                self.workspace.frame_selection();
                self.mark_scene_dirty();
                self.state.status = self.tf("desktop.status.selectedEntity", &[("name", &name)]);
            }
            PaletteAction::OpenAsset(path) => {
                if path.ends_with(".scene.json") || path.ends_with(".prefab.json") {
                    if let Err(error) = self.workspace.open_project_file(&path) {
                        self.state.status = self.tf(
                            "desktop.status.openFailed",
                            &[("path", &path), ("error", &error.to_string())],
                        );
                    } else {
                        self.state.status = self.tf("desktop.status.opened", &[("path", &path)]);
                    }
                    return;
                }

                if let Some((folder, _)) = path.rsplit_once('/') {
                    let mut prefix = String::new();
                    for segment in folder.split('/') {
                        if !prefix.is_empty() {
                            prefix.push('/');
                        }
                        prefix.push_str(segment);
                        self.workspace.expanded_assets.insert(prefix.clone());
                    }
                }
                self.state.select_asset_only(path.clone());
                self.state.status = self.tf("desktop.status.selectedAsset", &[("path", &path)]);
            }
        }
    }

    pub(super) fn ui_set(
        &mut self,
        node: &str,
        value: serde_json::Value,
    ) -> Result<String, String> {
        if let Some(("prompt", None)) = crate::ui_tree::parse_dialog_node(node) {
            let text = value
                .as_str()
                .ok_or_else(|| "dialog:prompt 需要字符串 value".to_string())?;
            let Some(prompt) = self.state.prompt.as_mut() else {
                return Err("no prompt is open".into());
            };
            prompt.value = text.to_string();
            return Ok("set prompt value".to_string());
        }
        let rest = node.strip_prefix("field:").ok_or_else(|| {
            format!("unsupported node '{node}' (field:<entity>:<component>:<path>)")
        })?;

        let (entity, rest) = rest
            .split_once(':')
            .ok_or_else(|| "field node needs an entity id".to_string())?;
        let entity = entity
            .parse::<u32>()
            .map_err(|_| "field node needs an entity id".to_string())?;
        let (component, path) = rest
            .rsplit_once(':')
            .ok_or_else(|| "field node needs <component>:<path>".to_string())?;
        let component = component.to_owned();
        let path = path.to_owned();

        let current = self
            .workspace
            .document()
            .doc
            .index_of_id(entity)
            .and_then(|index| {
                let doc = &self.workspace.document().doc;
                let entity = doc.entities.get(index)?;
                if component == "Transform" {
                    Some(serde_json::to_value(&entity.transform).unwrap_or_default())
                } else {
                    entity.components.get(&component).cloned()
                }
            })
            .ok_or_else(|| format!("entity {entity} 没有组件 '{component}'"))?;
        if prism_editor_core::schema::get_at(&current, &path).is_none() {
            let mut available = Vec::new();
            crate::ui_tree::flatten_paths(&current, "", &mut |path, _| {
                available.push(path.to_string())
            });
            available.truncate(24);
            return Err(format!(
                "'{component}' 里没有字段 '{path}'；可用：{}",
                available.join(", ")
            ));
        }
        if component == "Transform" {
            let (field, axis) = split_axis(&path).ok_or_else(|| {
                format!("Transform 只支持 position/rotation/scale 的 [轴] 写法，收到 '{path}'")
            })?;
            let axes: Vec<(usize, f64)> = match value.as_array() {
                Some(items) => items
                    .iter()
                    .enumerate()
                    .filter_map(|(axis, item)| item.as_f64().map(|value| (axis, value)))
                    .collect(),
                None => {
                    let scalar = value
                        .as_f64()
                        .ok_or_else(|| format!("Transform.{field}[{axis}] 需要数字"))?;
                    vec![(axis, scalar)]
                }
            };
            for (axis, value) in axes {
                self.workspace
                    .set_transform_axis(entity, field, axis, value as f32);
            }
            return Ok(format!("set Transform.{path}"));
        }
        self.workspace.set_field(entity, &component, &path, value);
        Ok(format!("set {component}.{path}"))
    }

    pub(super) fn capture_dir(&self) -> PathBuf {
        self.state_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub(super) fn control_port_file(&self) -> PathBuf {
        self.capture_dir().join("control.port")
    }

    pub(super) fn capture(
        &mut self,
        target: crate::screenshot::CaptureTarget,
        path: &Path,
        size: Option<[u32; 2]>,
    ) -> anyhow::Result<PathBuf> {
        match target {
            crate::screenshot::CaptureTarget::Window => {
                let hwnd = self.hwnd.ok_or_else(|| {
                    anyhow::anyhow!("window capture unavailable on this platform")
                })?;
                let path = crate::screenshot::save_window_png(hwnd, path, size)?;
                self.state.status = self.tf(
                    "desktop.status.screenshot",
                    &[("path", &path.display().to_string())],
                );
                self.workspace.log(
                    prism_editor_core::LogSource::Editor,
                    prism_editor_core::LogLevel::Info,
                    format!("screenshot saved: {}", path.display()),
                );
                Ok(path)
            }
            crate::screenshot::CaptureTarget::Scene | crate::screenshot::CaptureTarget::Game => {
                let bridge = ImageBridge {
                    registry: self.bridge.registry.clone(),
                    sampler: self.bridge.sampler,
                };
                let Some(view) = self.view_for(target) else {
                    anyhow::bail!("view not available (GPU init failed?)");
                };
                view.host.request_capture(path.to_path_buf());
                view.mark_dirty();
                let render_error = view.render_frame(&bridge);
                if view.error.is_some() {
                    anyhow::bail!(
                        "view render failed: {}",
                        view.error.clone().unwrap_or_default()
                    );
                }
                let _ = render_error;
                if !path.exists() {
                    anyhow::bail!("viewport capture wrote nothing (no frame?)");
                }
                self.state.status = self.tf(
                    "desktop.status.screenshot",
                    &[("path", &path.display().to_string())],
                );
                self.workspace.log(
                    prism_editor_core::LogSource::Editor,
                    prism_editor_core::LogLevel::Info,
                    format!("viewport screenshot: {}", path.display()),
                );
                Ok(path.to_path_buf())
            }
        }
    }

    pub(super) fn view_for(
        &mut self,
        target: crate::screenshot::CaptureTarget,
    ) -> Option<&mut ViewportState> {
        let kind = match target {
            crate::screenshot::CaptureTarget::Scene => ViewKind::Scene,
            _ => ViewKind::Game,
        };
        if !self.ensure_view(kind) {
            return None;
        }
        match kind {
            ViewKind::Scene => self.scene_view.as_mut(),
            ViewKind::Game => self.game_view.as_mut(),
        }
    }

    pub(super) fn capture_window_now(&mut self) {
        let path = crate::screenshot::default_path(
            &self.capture_dir(),
            crate::screenshot::CaptureTarget::Window,
        );
        let (reply, _unused) = std::sync::mpsc::channel();
        self.pending_captures.push(PendingCapture {
            target: crate::screenshot::CaptureTarget::Window,
            path: path.clone(),
            size: None,
            ready_at: self.frame_count + 1,
            reply,
        });
        self.state.status = self.tf(
            "desktop.status.screenshot",
            &[("path", &path.display().to_string())],
        );
    }
}
