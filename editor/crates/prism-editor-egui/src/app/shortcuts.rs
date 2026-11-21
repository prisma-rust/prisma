//! Keyboard shortcut handling and editor command dispatch.

use super::*;

pub(super) fn editor_shortcuts(
    overrides: &std::collections::BTreeMap<String, String>,
) -> Vec<(egui::KeyboardShortcut, Command)> {
    use egui::{KeyboardShortcut, Modifiers};
    let mut shortcuts: Vec<(KeyboardShortcut, Command)> =
        prism_editor_core::effective_bindings(overrides)
            .into_iter()
            .filter_map(|(command, chord)| {
                let key = key_for(&chord.key)?;
                let mut modifiers = Modifiers::NONE;
                if chord.ctrl {
                    modifiers |= Modifiers::CTRL;
                }
                if chord.shift {
                    modifiers |= Modifiers::SHIFT;
                }
                if chord.alt {
                    modifiers |= Modifiers::ALT;
                }
                Some((KeyboardShortcut::new(modifiers, key), command))
            })
            .collect();
    shortcuts.sort_by_key(|(shortcut, _)| {
        std::cmp::Reverse(
            u8::from(shortcut.modifiers.ctrl)
                + u8::from(shortcut.modifiers.shift)
                + u8::from(shortcut.modifiers.alt),
        )
    });
    shortcuts
}

pub(super) fn key_for(name: &str) -> Option<egui::Key> {
    let canonical = match name {
        "del" => "delete",
        "ins" => "insert",
        "esc" => "escape",
        "return" => "enter",
        other => other,
    };
    egui::Key::ALL
        .iter()
        .copied()
        .find(|key| key.name().eq_ignore_ascii_case(canonical))
}

pub(super) fn consume_shortcut_exact(
    input: &mut egui::InputState,
    shortcut: &egui::KeyboardShortcut,
) -> bool {
    let index = input.events.iter().position(|event| match event {
        egui::Event::Key {
            key,
            pressed: true,
            repeat,
            modifiers,
            ..
        } => {
            *key == shortcut.logical_key && !*repeat && modifiers.matches_exact(shortcut.modifiers)
        }
        _ => false,
    });
    match index {
        Some(index) => {
            input.events.remove(index);
            true
        }
        None => false,
    }
}

pub(super) fn plain_shortcut(command: Command) -> bool {
    matches!(
        command,
        Command::SelectAll | Command::FrameSelection | Command::DeleteSelection
    )
}

pub(super) fn split_axis(path: &str) -> Option<(&str, usize)> {
    match path.split_once('[') {
        Some((field, rest)) => {
            let axis = rest.trim_end_matches(']').parse::<usize>().ok()?;
            Some((field, axis))
        }
        None => match path {
            "position" | "rotation" | "scale" => Some((path, 0)),
            _ => None,
        },
    }
}

impl EditorApp {
    pub(super) fn apply_command(&mut self, command: Command) {
        if command == Command::DeleteSelection
            && self.state.confirm_delete
            && !self.workspace.selection.is_empty()
        {
            self.state.pending_entity_delete = Some(self.workspace.selection.clone());
            return;
        }
        if command == Command::SaveAs {
            let kind = self.workspace.document().kind;
            let title = prism_editor_core::project::ensure_document_suffix(
                kind,
                &self.workspace.document().title(),
            );
            self.open_prompt(
                panels::PromptKind::SaveAs,
                "Save As",
                "assets/ 下的相对路径（.scene.json / .prefab.json）",
                title,
            );
            return;
        }
        let was_playing = self.workspace.playing;
        let was_paused = self.workspace.paused;
        let pre_command_doc = self.doc_sync_key();
        // Console Clear on Play runs before Play starts so initial runtime logs stay visible.
        if command == Command::Play && !was_playing && self.state.log_clear_on_play {
            self.workspace.log.clear();
        }
        self.workspace.run(command);

        if command == Command::Play && !was_playing {
            self.playing = true;
            self.paused = false;
            self.play_start_doc_state = Some(pre_command_doc);
            // Write current preview snapshot so Game view starts from clean disk state.
            self.write_preview_scene();
            self.ensure_view(ViewKind::Game);
            let mut play_note = None;
            if let Some(view) = self.game_view.as_mut() {
                if let Err(error) = view.host.load_scene(&self.preview_scene) {
                    view.error = Some(format!("{error}"));
                }

                let status = view.host.project_module_status();
                match &status {
                    Some(status) if status.loaded && status.dirty => {
                        if !status.building {
                            view.host.refresh_project_module();
                        }
                        play_note = Some(
                            "Play: compiling pending module changes — project state resets on reload"
                                .to_string(),
                        );
                    }
                    Some(status) if status.loaded => {}
                    Some(status) if status.available => {
                        if !status.building {
                            view.host.refresh_project_module();
                        }
                        play_note = Some(if status.building {
                            "Play: project module is compiling — engine systems only for now"
                                .to_string()
                        } else {
                            "Play: compiling project module — engine systems only until it loads"
                                .to_string()
                        });
                    }
                    _ => {
                        play_note = Some(
                            "Play: project has no preview-module — running engine systems only"
                                .to_string(),
                        );
                    }
                }
                view.host.set_simulate(true);
                view.host.set_paused(false);
                view.mark_dirty();
            }
            if let Some(note) = &play_note {
                self.state.status = note.clone();
                self.workspace.log(
                    prism_editor_core::LogSource::System,
                    prism_editor_core::LogLevel::Warn,
                    note.clone(),
                );
            }
            if let Some(view) = self.scene_view.as_mut() {
                view.mark_dirty();
            }
        }
        if command == Command::Stop && was_playing {
            self.playing = false;
            self.paused = false;
            // Play edits were discarded by `workspace.stop()`; flush snapshot before reloading.
            let discarded = self
                .play_start_doc_state
                .take()
                .is_some_and(|start| start != pre_command_doc);
            self.write_preview_scene();
            if discarded {
                self.state.status = self.t("desktop.status.stoppedPlayDiscarded");
            }
            // Stop must clear the live runtime immediately; do not wait for sync_document.
            if let Some(view) = self.game_view.as_mut() {
                if let Err(error) = view.host.load_scene(&self.preview_scene) {
                    view.error = Some(format!("{error}"));
                }
                view.host.set_simulate(false);
                view.host.set_paused(false);
                view.reset_forwarded_input();
                view.mark_dirty();
            }
            if discarded {
                if let Some(view) = self.scene_view.as_mut() {
                    if let Err(error) = view.host.load_scene(&self.preview_scene) {
                        view.error = Some(format!("{error}"));
                    }
                    view.mark_dirty();
                }
            } else if let Some(view) = self.scene_view.as_mut() {
                view.mark_dirty();
            }
            self.synced_doc_state = Some(self.doc_sync_key());
        }
        if command == Command::Pause || command == Command::Step {
            self.paused = self.workspace.paused;
        }
        if command == Command::Step {
            if let Some(view) = self.game_view.as_mut() {
                view.host.step();
                view.mark_dirty();
            }
        }

        if self.playing != was_playing || self.paused != was_paused {
            if let Some(view) = self.game_view.as_mut() {
                view.host.set_simulate(self.playing);
                view.host.set_paused(self.paused);
            }
        }
    }

    pub(super) fn handle_command(
        &mut self,
        ctx: &egui::Context,
        command: &str,
        args: &serde_json::Value,
        reply: std::sync::mpsc::Sender<serde_json::Value>,
    ) -> Option<serde_json::Value> {
        use serde_json::json;
        let direct = match command {
            "ping" => json!({
                "ok": true,
                "pid": std::process::id(),
                "frames": self.frame_count,
                "fps": self.game_view.as_ref().map(|view| view.fps).unwrap_or(0.0),
                "scene": self.workspace.document().path.clone(),
            }),
            "status" => json!({
                "ok": true,
                "pid": std::process::id(),
                "frames": self.frame_count,
                "fps": self.game_view.as_ref().map(|view| view.fps).unwrap_or(0.0),
                "playing": self.playing,
                "paused": self.paused,
                "project_module": self
                    .game_view
                    .as_ref()
                    .and_then(|view| view.host.project_module_status())
                    .map(|status| {
                        json!({
                            "available": status.available,
                            "loaded": status.loaded,
                            "building": status.building,
                        })
                    }),
                "scene": self.workspace.document().path.clone(),
                "dirty": self.workspace.document().is_dirty(),
                "project_root": self
                    .project_root
                    .as_ref()
                    .map(|path| path.display().to_string()),
                "selection": self.workspace.selection.clone(),
                "status": self.state.status.clone(),
                "control": {
                    "enabled": self.control.is_some(),
                    "cli_disabled": self.control_cli_disabled,
                    "port_file": self.control_port_file().display().to_string(),
                    "error": self.control_error.clone(),
                },
                "project_module": self.project_module_json(),
            }),
            "logs" | "log.dump" => {
                let since = args
                    .get("since")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let limit = args
                    .get("limit")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(200) as usize;
                let lines: Vec<serde_json::Value> = self
                    .workspace
                    .log
                    .all()
                    .iter()
                    .filter(|line| line.seq >= since)
                    .take(limit)
                    .map(|line| {
                        json!({
                            "seq": line.seq,
                            "source": format!("{:?}", line.source),
                            "level": format!("{:?}", line.level),
                            "text": line.text,
                        })
                    })
                    .collect();
                let next = self
                    .workspace
                    .log
                    .all()
                    .back()
                    .map(|line| line.seq + 1)
                    .unwrap_or(since);
                json!({ "ok": true, "lines": lines, "next": next })
            }
            "input" | "input.send" => {
                let state = args
                    .get("state")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("press");
                let pressed = match state {
                    "press" | "down" | "true" => true,
                    "release" | "up" | "false" => false,
                    other => {
                        return Some(crate::control::failed(
                            crate::control::Kind::BadRequest,
                            format!("state 只能是 press / release，收到 '{other}'"),
                        ))
                    }
                };
                if args.get("click").and_then(serde_json::Value::as_bool) == Some(true) {
                    self.pending_input.push_back(InjectedInput::Mouse(
                        prism_engine::input::MouseButton::Left,
                        true,
                    ));
                    self.pending_input.push_back(InjectedInput::Mouse(
                        prism_engine::input::MouseButton::Left,
                        false,
                    ));
                    return Some(json!({
                        "ok": true,
                        "queued": 2,
                        "pending": self.pending_input.len(),
                    }));
                }
                if let Some(button) = args.get("mouse").and_then(serde_json::Value::as_str) {
                    let button = parse_mouse(button);
                    let Some(button) = button else {
                        return Some(crate::control::failed(
                            crate::control::Kind::BadRequest,
                            "mouse 只能是 left / right / middle",
                        ));
                    };
                    self.pending_input
                        .push_back(InjectedInput::Mouse(button, pressed));
                } else if let Some(key) = args.get("key").and_then(serde_json::Value::as_str) {
                    let Some(key) = parse_key(key) else {
                        return Some(crate::control::failed(
                            crate::control::Kind::BadRequest,
                            format!("不认识的按键 '{key}'（可用：w/a/s/d/q/e/space/shift/ctrl/alt/escape/tab/enter/方向键/0-9）"),
                        ));
                    };
                    self.pending_input
                        .push_back(InjectedInput::Key(key, pressed));
                } else {
                    return Some(crate::control::failed(
                        crate::control::Kind::BadRequest,
                        "需要 key / mouse / click 之一",
                    ));
                }
                json!({ "ok": true, "queued": 1, "pending": self.pending_input.len() })
            }
            "play" | "play.start" => {
                self.apply_command(Command::Play);
                json!({ "ok": true, "playing": self.playing })
            }
            "stop" | "play.stop" => {
                self.apply_command(Command::Stop);
                json!({ "ok": true, "playing": self.playing })
            }
            "pause" => {
                self.apply_command(Command::Pause);
                json!({ "ok": true, "paused": self.paused, "playing": self.playing })
            }
            "capture" => {
                let target = args
                    .get("target")
                    .and_then(serde_json::Value::as_str)
                    .and_then(crate::screenshot::CaptureTarget::parse)
                    .unwrap_or(crate::screenshot::CaptureTarget::Window);
                let size = args
                    .get("size")
                    .and_then(serde_json::Value::as_str)
                    .and_then(crate::screenshot::parse_size);
                let path = match args.get("path").and_then(serde_json::Value::as_str) {
                    Some(value) => std::path::PathBuf::from(value),
                    None => crate::screenshot::default_path(&self.capture_dir(), target),
                };

                const STABLE_FRAME: u64 = 3;
                let ready_at = (self.frame_count + 1).max(STABLE_FRAME);
                self.pending_captures.push(PendingCapture {
                    target,
                    path,
                    size,
                    ready_at,
                    reply,
                });
                ctx.request_repaint();
                return None;
            }
            "ui.snapshot" | "ui_snapshot" => {
                let logs: Vec<String> = self
                    .workspace
                    .log
                    .visible_lines()
                    .iter()
                    .map(|line| format!("[{:?}] {}", line.level, line.text))
                    .collect();
                let viewports = self.viewport_objects();
                let dialogs = self.dialogs();
                let input = crate::ui_tree::SnapshotInput {
                    workspace: &self.workspace,
                    playing: self.playing,
                    paused: self.paused,
                    status: &self.state.status,
                    frames: self.frame_count,
                    fps: self.game_view.as_ref().map(|view| view.fps).unwrap_or(0.0),
                    gizmo_tool: self.scene_view.as_ref().map(|view| view.tool.label()),
                    // Keep the control protocol locale-independent. The toolbar may use
                    // localized labels, but MCP clients need stable enum-like values.
                    gizmo_space: match self.workspace.gizmo_space {
                        prism_editor_core::viewport_math::GizmoSpace::World => "World",
                        prism_editor_core::viewport_math::GizmoSpace::Local => "Local",
                    },
                    rects: &self.panel_rects,
                    leaves: &self.dock_leaves,
                    logs: &logs,
                    viewports: &viewports,
                    dialogs: &dialogs,
                };
                crate::ui_tree::snapshot(&input)
            }
            "ui.click" | "ui_click" => {
                let node = args
                    .get("node")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                let offset = args.get("offset").and_then(parse_offset);
                let clicked = match crate::ui_tree::parse_view_object_node(node) {
                    Some(tab) => self.ui_click_view_object(tab, node, offset),
                    None if crate::ui_tree::parse_dialog_node(node).is_some() => self
                        .ui_click_dialog(ctx, node)
                        .map_err(crate::control::ControlError::from),
                    None => match offset {
                        Some(offset) => self.ui_click_panel_offset(node, offset),
                        None => self
                            .ui_click(node)
                            .map(|message| json!({ "node": node, "action": message }))
                            .map_err(crate::control::ControlError::from),
                    },
                };
                match clicked {
                    Ok(mut value) => {
                        value["ok"] = json!(true);

                        value["pending_dialog"] = self
                            .dialogs()
                            .first()
                            .map(|dialog| json!(dialog.node()))
                            .unwrap_or(serde_json::Value::Null);
                        value
                    }
                    Err(error) => {
                        let mut value = crate::control::ControlError::from(error).into_json();
                        value["node"] = json!(node);
                        value
                    }
                }
            }
            "ui.set" | "ui_set" => {
                let node = args
                    .get("node")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                let value = args
                    .get("value")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                match self.ui_set(node, value) {
                    Ok(message) => json!({ "ok": true, "node": node, "action": message }),
                    Err(error) => {
                        let mut value = crate::control::ControlError::from(error).into_json();
                        value["node"] = json!(node);
                        value
                    }
                }
            }
            "ui.move" | "ui_move" | "ui.hover" => match self.ui_view_move(args) {
                Ok(mut value) => {
                    value["ok"] = json!(true);
                    value
                }
                Err(error) => error.into_json(),
            },
            "ui.wheel" | "ui_wheel" => match self.ui_view_wheel(args) {
                Ok(mut value) => {
                    value["ok"] = json!(true);
                    value
                }
                Err(error) => error.into_json(),
            },
            "ui.drag" | "ui_drag" => match self.ui_view_drag(args) {
                Ok(mut value) => {
                    value["ok"] = json!(true);
                    value
                }
                Err(error) => error.into_json(),
            },
            "quit.confirm" | "quit_confirm" => {
                let dirty: Vec<String> = self
                    .workspace
                    .docs
                    .iter()
                    .filter(|document| document.is_dirty())
                    .map(|document| document.path.clone().unwrap_or_else(|| document.title()))
                    .collect();
                self.request_quit(ctx);
                json!({
                    "ok": true,
                    "closing": dirty.is_empty(),
                    "pending": (!dirty.is_empty()).then_some("unsaved"),
                    "dirty_documents": dirty,
                })
            }
            "quit.cancel" | "quit_cancel" => {
                self.state.pending_close = None;
                json!({ "ok": true })
            }
            "quit" => {
                self.force_quit = true;
                self.request_quit(ctx);
                json!({ "ok": true })
            }
            other => crate::control::failed(
                crate::control::Kind::BadRequest,
                format!("unknown cmd: {other}"),
            ),
        };
        Some(direct)
    }

    pub(super) fn save_all_documents(&mut self) -> Result<usize, String> {
        let count = self.workspace.docs.len();
        let mut failures = Vec::new();
        let mut saved = 0usize;
        for index in 0..count {
            if self.workspace.docs[index].is_dirty() {
                match self.workspace.save_document(index) {
                    Ok(_) => saved += 1,
                    Err(error) if self.workspace.save_conflict(index).is_some() => {
                        self.state.pending_save_conflict.get_or_insert(index);
                        failures.push(error.to_string());
                    }
                    Err(error) => failures.push(error.to_string()),
                }
            }
        }
        if failures.is_empty() {
            self.state.status = if saved == 0 {
                self.t("desktop.status.nothingToSave")
            } else {
                self.tf("desktop.status.savedDocs", &[("count", &saved.to_string())])
            };
            Ok(saved)
        } else {
            let message = failures.join("; ");
            self.state.status = self.tf("desktop.status.saveFailed", &[("error", &message)]);
            Err(message)
        }
    }

    pub(super) fn request_asset_delete(&mut self) {
        if self.state.asset_selection.is_empty() {
            self.state.status = self.t("desktop.status.noAssetSelected");
            return;
        }
        let paths = self.state.asset_selection.clone();
        if self.preferences.confirm_delete {
            panels::request_delete_assets(&mut self.workspace, &mut self.state, paths);
        } else {
            let mut deleted = 0usize;
            for path in &paths {
                panels::delete_asset_now(&mut self.workspace, &mut self.state, path);
                deleted += 1;
            }
            self.state.status = self.tf(
                "desktop.status.deletedAssets",
                &[("count", &deleted.to_string())],
            );
        }
    }

    pub(super) fn duplicate_selected_asset(&mut self) {
        if self.state.asset_selection.is_empty() {
            self.state.status = self.t("desktop.status.noAssetSelected");
            return;
        }
        let mut created = Vec::new();
        let mut failures = Vec::new();
        for path in self.state.asset_selection.clone() {
            match self.workspace.duplicate_asset(&path) {
                Ok(copy) => created.push(copy),
                Err(error) => failures.push(format!("{path}: {error}")),
            }
        }
        self.state.status = if failures.is_empty() {
            self.tf(
                "desktop.status.duplicatedAssets",
                &[("count", &created.len().to_string())],
            )
        } else {
            self.tf(
                "desktop.status.duplicateAssetsFailed",
                &[("error", &failures.join("; "))],
            )
        };
        if let Some(last) = created.last() {
            self.state.select_asset_only(last.clone());
        }
    }

    pub(super) fn rename_selected_asset_prompt(&mut self) {
        let Some(path) = self.state.asset_primary().cloned() else {
            self.state.status = self.t("desktop.status.noAssetSelected");
            return;
        };
        if self.state.asset_selection.len() > 1 {
            self.state.status = self.t("desktop.status.renameOneAtATime");
        }
        if path.ends_with(".scene.json") || path.ends_with(".prefab.json") {
            self.state.status = self.t("desktop.status.sceneRenameViaSaveAs");
            return;
        }
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        self.state.asset_rename = Some((path, name));
        self.state.asset_rename_focus = true;
    }

    pub(super) fn create_empty_parent(&mut self) {
        let parents: Vec<usize> = self.workspace.selection.clone();
        if parents.is_empty() {
            self.create_empty_child();
            return;
        }
        let before = self.workspace.document().doc.entities.len();
        self.workspace.add_entity("");
        let created = self
            .workspace
            .document()
            .doc
            .entities
            .len()
            .saturating_sub(1);
        if created >= before {
            for index in parents {
                self.workspace.reparent(index, Some(created), None);
            }
            self.workspace.select(created, false);
        }
    }

    pub(super) fn invert_selection(&mut self) {
        let total = self.workspace.document().doc.entities.len();
        let inverted: Vec<usize> = (0..total)
            .filter(|index| !self.workspace.selection.contains(index))
            .collect();
        self.workspace.select_many(&inverted, false);
    }

    pub(super) fn select_children_of_selection(&mut self) {
        let indices = self.workspace.selection.clone();
        let mut children = Vec::new();
        for index in indices {
            children.extend(self.workspace.document().doc.descendants(index));
        }
        if !children.is_empty() {
            self.workspace.select_many(&children, false);
        }
    }

    pub(super) fn select_prefab_root(&mut self) {
        let Some(mut index) = self.workspace.primary_selection() else {
            return;
        };
        let doc = &self.workspace.document().doc;
        let Some(link) = doc.entities[index]
            .prefab_link()
            .map(|link| link.to_string())
        else {
            self.state.status = self.t("inspector.notPrefabInstance");
            return;
        };
        while let Some(parent) = doc.entities[index].parent {
            let Some(parent_index) = doc.index_of_id(parent) else {
                break;
            };
            if doc.entities[parent_index].prefab_link() != Some(link.as_str()) {
                break;
            }
            index = parent_index;
        }
        self.workspace.select(index, false);
    }

    pub(super) fn current_selection_snapshot(&self) -> SelectionSnapshot {
        SelectionSnapshot::new(
            self.workspace.active_index(),
            self.workspace.selection.clone(),
            self.workspace.selection_active,
        )
    }

    pub(super) fn track_selection_history(&mut self) {
        self.selection_history
            .observe(self.current_selection_snapshot());
    }

    pub(super) fn find_in_hierarchy(&mut self) {
        let Some(index) = self.workspace.primary_selection() else {
            return;
        };
        panels::reveal_in_hierarchy(&mut self.workspace, &mut self.state, index);
    }

    pub(super) fn navigate_selection_history(&mut self, delta: isize) {
        let Some(entry) = self.selection_history.navigate(delta) else {
            return;
        };
        if self.workspace.active_index() != entry.document
            && entry.document < self.workspace.docs.len()
        {
            self.workspace.set_active(entry.document);
        }
        self.workspace.select_many(&entry.selection, false);
        self.workspace.selection_active = entry
            .active
            .filter(|index| entry.selection.contains(index))
            .or_else(|| entry.selection.last().copied());
        let expanded: Vec<u32> = self
            .workspace
            .document()
            .doc
            .entities
            .iter()
            .enumerate()
            .filter(|(index, _)| entry.selection.contains(index))
            .flat_map(|(index, _)| self.workspace.document().doc.path_of(index))
            .filter_map(|index| self.workspace.document().doc.id_of_index(index))
            .collect();
        self.workspace.expanded.extend(expanded);
        self.state.status = self.tf(
            "desktop.status.selectionHistory",
            &[
                (
                    "index",
                    &(self.selection_history.position() + 1).to_string(),
                ),
                ("total", &self.selection_history.len().to_string()),
            ],
        );
    }

    pub(super) fn copy_transform(&mut self, field: &str) {
        let Some(index) = self.workspace.primary_selection() else {
            return;
        };
        let transform = self.workspace.document().doc.entities[index].transform;
        let values = match field {
            "position" => transform.position.to_vec(),
            "rotation" => transform.rotation.to_vec(),
            "scale" => transform.scale.to_vec(),
            _ => return,
        };
        self.state.transform_clip = Some((field.to_string(), values));
        self.state.status = self.tf("desktop.status.copiedField", &[("field", field)]);
    }

    pub(super) fn paste_transform(&mut self, field: &str) {
        let Some((clipboard_field, values)) = self.state.transform_clip.clone() else {
            self.state.status = self.t("desktop.status.clipboardEmpty");
            return;
        };
        if clipboard_field != field {
            self.state.status = self.tf(
                "desktop.status.clipboardMismatch",
                &[("held", &clipboard_field), ("field", field)],
            );
            return;
        }
        let ids: Vec<u32> = self
            .workspace
            .selection
            .iter()
            .filter_map(|index| self.workspace.document().doc.id_of_index(*index))
            .collect();
        if ids.is_empty() {
            return;
        }
        for entity in &ids {
            for (axis, value) in values.iter().enumerate() {
                self.workspace
                    .set_transform_axis(*entity, field, axis, *value);
            }
        }
        self.state.status = self.tf(
            "desktop.status.pastedField",
            &[("field", field), ("count", &ids.len().to_string())],
        );
    }

    pub(super) fn find_references(&mut self, path: &str, project_wide: bool) {
        let mut hits = Vec::new();
        let docs = if project_wide {
            self.workspace.docs.len()
        } else {
            self.workspace.active_index() + 1
        };
        let start = if project_wide {
            0
        } else {
            self.workspace.active_index()
        };
        let target_guids = self.workspace.asset_guids(path);
        for index in start..docs {
            let Some(doc) = self.workspace.docs.get(index) else {
                continue;
            };
            for (entity, component, field) in
                doc.doc.field_references_with_guids(path, &target_guids)
            {
                let name = doc
                    .doc
                    .entities
                    .get(entity)
                    .map(|entity| entity.display_name())
                    .unwrap_or_else(|| format!("#{entity}"));
                hits.push(format!("{}: {name} · {component}.{field}", doc.title()));
            }
        }
        if hits.is_empty() {
            self.state.status = self.tf("desktop.status.noReferences", &[("path", path)]);
        } else {
            self.state.status = format!("{} reference(s) to {path}", hits.len());
        }
        for hit in hits {
            self.workspace.log(
                prism_editor_core::LogSource::Editor,
                prism_editor_core::LogLevel::Info,
                hit,
            );
        }
    }

    pub(super) fn create_empty_child(&mut self) {
        let parent = self.workspace.primary_selection();
        let before = self.workspace.document().doc.entities.len();
        self.workspace.add_entity("");
        let created = self
            .workspace
            .document()
            .doc
            .entities
            .len()
            .saturating_sub(1);
        if created >= before {
            self.workspace.reparent(created, parent, None);
        }
    }

    pub(super) fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};

        if self.state.shortcut_capture.is_some() {
            if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
                self.state.shortcut_capture = None;
            }
            return;
        }

        if ctx.input_mut(|input| {
            input.consume_shortcut(&KeyboardShortcut::new(Modifiers::CTRL, Key::K))
        }) {
            self.palette.toggle(&self.workspace);
        }

        if self.palette.is_open() {
            return;
        }
        if self
            .scene_view
            .as_ref()
            .is_some_and(|view| view.flying_camera())
            || (self.state.focused_panel.as_deref() == Some("sceneView")
                && ctx.input(|input| input.pointer.button_down(egui::PointerButton::Secondary)))
        {
            return;
        }
        if self.playing && self.state.focused_panel.as_deref() == Some("gameView") {
            return;
        }
        let shortcuts = editor_shortcuts(&self.preferences.shortcut_overrides);

        let wants_keyboard = ctx.memory(|memory| memory.focused()).is_some();

        if !wants_keyboard {
            if ctx.input(|input| input.key_pressed(egui::Key::Space) && input.modifiers.shift) {
                if self.workspace.maximized_tab.is_some() {
                    self.workspace.restore_maximized();
                } else if let Some(tab) = self.state.focused_panel.clone() {
                    self.workspace.toggle_maximized(&tab);
                }
            }
            use crate::viewport::GizmoTool;
            if ctx.input(|input| input.key_pressed(egui::Key::W)) {
                self.set_gizmo_tool(GizmoTool::Translate);
            }
            if ctx.input(|input| input.key_pressed(egui::Key::E)) {
                self.set_gizmo_tool(GizmoTool::Rotate);
            }
            if ctx.input(|input| input.key_pressed(egui::Key::R)) {
                self.set_gizmo_tool(GizmoTool::Scale);
            }
            if ctx.input(|input| input.key_pressed(egui::Key::X)) {
                self.workspace.toggle_gizmo_space();
                self.mark_scene_dirty();
            }
            if ctx.input(|input| input.key_pressed(egui::Key::G)) {
                let mut settings = self.render_settings.clone();
                settings.show_grid = !settings.show_grid;
                self.apply_render_settings(settings);
            }
            if ctx.input(|input| input.key_pressed(egui::Key::S)) {
                self.workspace.toggle_skybox();
                let on = self.workspace.show_skybox;
                if let Some(view) = self.scene_view.as_mut() {
                    view.host.set_show_skybox(on);
                    view.mark_dirty();
                }
            }

            if ctx.input_mut(|input| {
                input.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::CTRL,
                    egui::Key::B,
                ))
            }) {
                if !self.build_job_running() {
                    self.start_build(crate::build::BuildKind::PlayerRun);
                }
            }

            if ctx.input_mut(|input| {
                input.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::CTRL,
                    egui::Key::R,
                ))
            }) {
                self.refresh_project_module();
            }
            if ctx.input_mut(|input| {
                input.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::CTRL,
                    egui::Key::I,
                ))
            }) {
                self.invert_selection();
            }
        }

        let focus = self.state.focused_panel.clone().unwrap_or_default();
        let assets_focused = focus == "assets";
        let inspector_focused = focus == "inspector";

        for (shortcut, command) in shortcuts {
            if wants_keyboard && plain_shortcut(command) {
                continue;
            }
            if ctx.input_mut(|input| consume_shortcut_exact(input, &shortcut)) {
                if assets_focused {
                    match command {
                        Command::DeleteSelection => {
                            self.request_asset_delete();
                            continue;
                        }
                        Command::DuplicateEntity => {
                            self.duplicate_selected_asset();
                            continue;
                        }

                        Command::CopyEntity
                        | Command::CutEntity
                        | Command::PasteEntity
                        | Command::SelectAll => continue,
                        _ => {}
                    }
                }
                if inspector_focused
                    && matches!(
                        command,
                        Command::DeleteSelection
                            | Command::DuplicateEntity
                            | Command::CopyEntity
                            | Command::CutEntity
                            | Command::PasteEntity
                    )
                {
                    continue;
                }
                self.apply_command(command);
                if matches!(command, Command::FrameSelection | Command::ResetSceneCamera) {
                    if let Some(view) = self.scene_view.as_mut() {
                        view.mark_dirty();
                    }
                }
            }
        }

        if !wants_keyboard && ctx.input(|input| input.key_pressed(egui::Key::F2)) {
            if assets_focused {
                self.rename_selected_asset_prompt();
            } else if let Some(index) = self.workspace.primary_selection() {
                let name = self.workspace.document().doc.entities[index].display_name();
                self.state.rename = Some((index, name));
                self.state.rename_focus = true;
            }
        }
    }

    pub(super) fn create_prefab_variant(&mut self, name: &str) -> std::io::Result<String> {
        let root_dir = self
            .project_root
            .clone()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no project root"))?;
        let Some(index) = self.workspace.selection.iter().copied().find(|index| {
            self.workspace
                .document()
                .doc
                .entities
                .get(*index)
                .is_some_and(|entity| entity.prefab_link().is_some())
        }) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "selection has no prefab instance",
            ));
        };
        let name = name.trim().trim_end_matches(".prefab.json");
        let stem = if name.is_empty() { "Variant" } else { name };
        let doc = self
            .workspace
            .document()
            .doc
            .extract_subtree_preserving_links(&[index]);
        let mut relative = format!("assets/prefabs/{stem}.prefab.json");
        let mut suffix = 2;
        while root_dir.join(&relative).exists() {
            relative = format!("assets/prefabs/{stem}-{suffix}.prefab.json");
            suffix += 1;
        }
        prism_editor_core::project::write_document(&root_dir, &relative, &doc)?;
        self.workspace.refresh_project_files();

        if let Some(entity) = self.workspace.document().doc.id_of_index(index) {
            self.workspace.set_field(
                entity,
                "prism_engine::scene::PrefabInstance",
                "prefab",
                serde_json::Value::String(relative.clone()),
            );
        }
        self.workspace.log(
            prism_editor_core::LogSource::Editor,
            prism_editor_core::LogLevel::Info,
            format!("prefab variant created: {relative}"),
        );
        self.synced_doc_state = None;
        Ok(relative)
    }

    pub(super) fn has_prefab_instance(&self) -> bool {
        let doc = &self.workspace.document().doc;
        self.workspace.selection.iter().any(|index| {
            doc.entities
                .get(*index)
                .is_some_and(|entity| entity.prefab_link().is_some())
        })
    }

    pub(super) fn set_gizmo_tool(&mut self, tool: crate::viewport::GizmoTool) {
        self.ensure_view(ViewKind::Scene);
        if let Some(view) = self.scene_view.as_mut() {
            view.tool = tool;
            view.sculpt = false;
            view.mark_dirty();
        }
    }

    pub(super) fn mark_scene_dirty(&mut self) {
        if let Some(view) = self.scene_view.as_mut() {
            view.mark_dirty();
        }
    }
}
