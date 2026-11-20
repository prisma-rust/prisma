//! Settings And Tools panel UI.

use super::inspector::prt_preview_controls;
use super::*;

pub fn project_settings(ui: &mut Ui, root: Option<&std::path::Path>, state: &mut UiState) {
    let Some(root) = root else {
        ui.label(RichText::new("Open a project directory first.").weak());
        return;
    };
    let same_root = state
        .settings_cache
        .as_ref()
        .is_some_and(|cache| cache.root() == root);
    let mut cache = if same_root {
        state.settings_cache.take().unwrap()
    } else {
        SettingsCache::load(root)
    };
    cache.refresh();
    let mut query = std::mem::take(&mut state.settings_search);

    let mut apply = false;
    let mut revert = false;
    let mut reset_all = false;
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut query)
                .hint_text("Search settings…")
                .desired_width(170.0),
        );
        if cache.is_dirty() {
            ui.label(
                RichText::new("● unsaved")
                    .size(11.0)
                    .color(Color32::from_rgb(226, 178, 84)),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            apply = ui
                .add_enabled(cache.is_dirty(), egui::Button::new("Apply"))
                .on_hover_text("Write assets/settings.ini")
                .clicked();
            revert = ui
                .add_enabled(cache.is_dirty(), egui::Button::new("Revert"))
                .clicked();
            reset_all = ui
                .button("Reset all")
                .on_hover_text("Fill every field with the built-in default")
                .clicked();
        });
    });
    if cache.external_change() {
        ui.label(
            RichText::new(
                "settings.ini changed on disk while you were editing — Revert reloads it.",
            )
            .size(11.0)
            .color(Color32::from_rgb(226, 178, 84)),
        );
    }
    if let Some(error) = cache.error.clone() {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(error)
                    .size(11.0)
                    .color(Color32::from_rgb(232, 96, 96)),
            );
            if ui.small_button("Reset to defaults").clicked() {
                reset_all = true;
            }
            if ui.small_button("Reload").clicked() {
                cache.reload_from_disk();
            }
        });
    }
    if apply {
        match cache.apply() {
            Ok(()) => state.status = "assets/settings.ini saved".to_string(),
            Err(error) => state.status = format!("settings save failed: {error}"),
        }
    }
    if revert {
        cache.revert();
        state.status = "settings reverted".to_string();
    }
    if reset_all {
        cache.reset_all();
    }
    ui.separator();

    let fields = matching_fields(&query);
    if fields.is_empty() {
        ui.label(RichText::new("no matching setting").weak());
    }
    egui::ScrollArea::vertical()
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for section in prism_editor_core::settings::SETTINGS_SECTIONS {
                let rows: Vec<_> = fields
                    .iter()
                    .filter(|field| field.section == *section)
                    .collect();
                if rows.is_empty() {
                    continue;
                }
                ui.horizontal(|ui| {
                    ui.label(RichText::new(*section).strong().size(12.0));
                    if ui
                        .small_button("Reset")
                        .on_hover_text(format!("Restore [{section}] to its defaults"))
                        .clicked()
                    {
                        cache.reset_section(section);
                    }
                });
                egui::Grid::new(format!("settings-{section}"))
                    .num_columns(2)
                    .spacing([10.0, 4.0])
                    .show(ui, |ui| {
                        for field in rows {
                            ui.label(field.label);
                            settings_field(ui, &mut cache.settings, field);
                            ui.end_row();
                        }
                    });
                ui.separator();
            }
        });

    state.settings_search = query;
    state.settings_cache = Some(cache);
}

pub(super) fn settings_field(
    ui: &mut Ui,
    settings: &mut ProjectSettings,
    field: &prism_editor_core::settings::SettingsField,
) {
    match (field.section, field.key) {
        ("app", "company") => {
            ui.text_edit_singleline(&mut settings.app.company);
        }
        ("app", "name") => {
            ui.text_edit_singleline(&mut settings.app.name);
        }
        ("app", "version") => {
            ui.text_edit_singleline(&mut settings.app.version);
        }
        ("window", "title") => {
            ui.text_edit_singleline(&mut settings.window.title);
        }
        ("window", "width") => {
            ui.add(
                egui::DragValue::new(&mut settings.window.width)
                    .speed(1.0)
                    .range(320..=16_384),
            );
        }
        ("window", "height") => {
            ui.add(
                egui::DragValue::new(&mut settings.window.height)
                    .speed(1.0)
                    .range(240..=16_384),
            );
        }
        ("window", "resizable") => {
            ui.checkbox(&mut settings.window.resizable, "");
        }
        ("window", "fullscreen") => {
            ui.checkbox(&mut settings.window.fullscreen, "");
        }
        ("window", "vsync") => {
            ui.checkbox(&mut settings.window.vsync, "");
        }
        ("build", "target") => {
            settings_choice(ui, field.key, &mut settings.build.target, TARGET_CHOICES);
        }
        ("build", "profile") => {
            settings_choice(ui, field.key, &mut settings.build.profile, PROFILE_CHOICES);
        }
        ("build", "output_dir") => {
            ui.text_edit_singleline(&mut settings.build.output_dir);
        }
        ("build", "run_after") => {
            ui.checkbox(&mut settings.build.run_after, "");
        }
        _ => {
            ui.label(RichText::new("?").weak());
        }
    }
}

pub(super) fn settings_choice(ui: &mut Ui, id: &str, value: &mut String, choices: &[&str]) {
    egui::ComboBox::from_id_salt(("settings-choice", id))
        .selected_text(value.clone())
        .width(140.0)
        .show_ui(ui, |ui| {
            for choice in choices {
                if ui.selectable_label(value == choice, *choice).clicked() {
                    *value = (*choice).to_string();
                }
            }
            if !choices.contains(&value.as_str()) {
                ui.label(RichText::new(value.clone()).weak());
            }
        });
}

pub fn preferences(
    ui: &mut Ui,
    preferences: &mut Preferences,
    workspace: &mut Workspace,
    state: &mut UiState,
    control_cli_disabled: bool,
) {
    ui.checkbox(&mut preferences.autosave, "Auto save layout");
    ui.checkbox(&mut preferences.confirm_delete, "Confirm before delete");
    ui.checkbox(&mut preferences.compact, "Compact layout");
    ui.add_enabled(
        !control_cli_disabled,
        egui::Checkbox::new(
            &mut preferences.control_channel,
            "Enable local control channel (MCP / automation)",
        ),
    )
    .on_hover_text(
        "Listen on 127.0.0.1 and write target/editor-egui/control.port so the MCP \
         server can drive the editor; turning it off stops the listener immediately \
         (#1008). --no-control disables it for the whole process.",
    );
    if control_cli_disabled {
        ui.label(
            RichText::new("Control channel disabled by --no-control")
                .weak()
                .small(),
        );
    }
    ui.checkbox(
        &mut preferences.hot_reload_assets,
        "Rebuild scene pack when loose assets change",
    )
    .on_hover_text(
        "Watch assets/ (textures, meshes, HDR, fonts…) and rerun Build Scene Pack \
         after a change, so Play picks it up without a restart (#495)",
    );
    ui.horizontal(|ui| {
        ui.label("Max log lines");
        let response =
            ui.add(egui::DragValue::new(&mut preferences.max_log_lines).range(500.0..=20_000.0));
        if response.changed() {
            workspace
                .log
                .set_capacity(preferences.max_log_lines as usize);
        }
    });
    ui.horizontal(|ui| {
        ui.label("Language");
        let locale = workspace.i18n.locale();
        if ui
            .selectable_label(locale == Locale::EnUs, "English")
            .clicked()
        {
            workspace.set_locale(Locale::EnUs);
            state.status = "locale: en-US".to_string();
        }
        if ui
            .selectable_label(locale == Locale::ZhCn, "简体中文")
            .clicked()
        {
            workspace.set_locale(Locale::ZhCn);
            state.status = "locale: zh-CN".to_string();
        }
    });
}

pub fn entity_delete_confirm(ctx: &egui::Context, workspace: &mut Workspace, state: &mut UiState) {
    let Some(indices) = state.pending_entity_delete.clone() else {
        return;
    };
    let mut decision: Option<bool> = None;
    egui::Window::new("Delete")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            let count = indices.len();
            if count == 1 {
                ui.label("Delete the selected entity and its children?");
            } else {
                ui.label(format!(
                    "Delete {count} selected entities and their children?"
                ));
            }
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    decision = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    decision = Some(false);
                }
            });
        });
    match decision {
        Some(true) => apply_pending_entity_delete(workspace, state),
        Some(false) => state.pending_entity_delete = None,
        None => {}
    }
}

pub fn apply_pending_entity_delete(workspace: &mut Workspace, state: &mut UiState) {
    let Some(indices) = state.pending_entity_delete.take() else {
        return;
    };
    workspace.select_many(&indices, false);
    workspace.delete_selection();
}

pub fn theme(ui: &mut Ui, workspace: &mut Workspace) {
    let current = workspace.theme.preset.clone();
    for preset in THEME_PRESETS {
        let selected = preset.id == current;
        if ui.selectable_label(selected, preset.label).clicked() {
            workspace.set_theme_preset(preset.id);
        }
    }
}

pub fn build(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    ui.heading("Scenes in build");
    let entries: Vec<(String, bool)> = workspace
        .scene_manifest
        .scenes
        .iter()
        .map(|entry| (entry.path.clone(), entry.enabled))
        .collect();
    let mut changed = false;
    for (index, (path, enabled)) in entries.iter().enumerate() {
        ui.horizontal(|ui| {
            let mut on = *enabled;
            if ui.checkbox(&mut on, "").changed() {
                workspace.set_scene_enabled(index, on);
                changed = true;
            }
            ui.label(RichText::new(path).size(11.0));
            if ui.small_button(icons::ARROW_UP).clicked() {
                workspace.move_scene_entry(index, -1);
                changed = true;
            }
            if ui.small_button(icons::ARROW_DOWN).clicked() {
                workspace.move_scene_entry(index, 1);
                changed = true;
            }
            if ui.small_button(icons::X).clicked() {
                workspace.remove_scene_entry(index);
                changed = true;
            }
        });
    }
    if changed {
        if let Some(root) = workspace.project_root.clone() {
            if let Err(error) = workspace.scene_manifest.write(&root) {
                state.status = format!("scenes.json save failed: {error}");
            }
        }
    }

    // ---- Add scenes that exist on disk but are missing from the manifest ----
    let available = workspace.available_scene_documents();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Add").size(11.0));
        ui.add_enabled_ui(!available.is_empty() && !state.build_running, |ui| {
            egui::ComboBox::from_id_salt("build-add-scene")
                .selected_text(if available.is_empty() {
                    "all scenes in build"
                } else {
                    "scene…"
                })
                .show_ui(ui, |ui| {
                    for path in &available {
                        if ui.selectable_label(false, path).clicked() {
                            if workspace.add_scene_document(path).is_some() {
                                state.status = format!("added {path} to build");
                            }
                        }
                    }
                });
        });
    });

    // ---- The active document may not be part of the build yet ----
    if let Some(path) = workspace.document().path.clone() {
        let relative = path.trim_start_matches("assets/").to_string();
        if relative.ends_with(".scene.json")
            && !workspace.scene_manifest.contains_document(&relative)
        {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("This scene is not in the build")
                        .weak()
                        .size(11.0),
                );
                if ui
                    .small_button("+ Add to build")
                    .on_hover_text("Append this document to assets/scenes.toml")
                    .clicked()
                    && workspace.add_scene_document(&relative).is_some()
                {
                    state.status = format!("added {relative} to build");
                }
            });
        }
    }

    ui.separator();
    ui.horizontal(|ui| {
        ui.add_enabled_ui(!state.build_running, |ui| {
            if ui.button("Build").clicked() {
                state.pending_build = Some(crate::build::BuildKind::ScenePack);
            }
            if ui.button("Build And Run").clicked() {
                state.pending_build = Some(crate::build::BuildKind::PlayerRun);
            }
        });
        if state.build_running {
            ui.label(RichText::new("building…").weak().size(11.0));
        }
    });
    ui.label(
        RichText::new(
            "Building runs `cargo build` + `prism-build-pipeline build-scenes` on the host; \
             the editor never links the cooker.",
        )
        .weak()
        .size(11.0),
    );
}

pub fn shortcuts(ui: &mut Ui, preferences: &mut Preferences, state: &mut UiState) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new("Click a key to rebind it. Esc cancels; Reset restores the default.")
                .weak()
                .size(11.0),
        );
        if ui.button("Reset all").clicked() {
            preferences.shortcut_overrides.clear();
            state.shortcut_capture = None;
            state.status = "shortcuts reset to defaults".to_string();
        }
    });

    if let Some(command) = state.shortcut_capture {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("Press a new key for {command:?}"))
                    .strong()
                    .color(Color32::from_rgb(255, 180, 80)),
            );
            if ui.small_button("Cancel").clicked() {
                state.shortcut_capture = None;
            }
        });
        if let Some(captured) = captured_shortcut(ui) {
            match captured {
                CapturedShortcut::Cancel => state.shortcut_capture = None,
                CapturedShortcut::Bound(keys) => match prism_editor_core::KeyChord::parse(&keys) {
                    Some(chord) => {
                        match prism_editor_core::conflicting_command(
                            &preferences.shortcut_overrides,
                            command,
                            &chord,
                        ) {
                            Some(other) => {
                                state.status = format!("{keys} is already bound to {other:?}");
                            }
                            None => {
                                preferences
                                    .shortcut_overrides
                                    .insert(command.id().to_string(), keys.clone());
                                state.status = format!("bound {command:?} to {keys}");
                                state.shortcut_capture = None;
                            }
                        }
                    }
                    None => state.status = format!("cannot bind {keys}"),
                },
            }
        }
        ui.separator();
    }

    egui::ScrollArea::vertical()
        .id_salt("shortcuts-scroll")
        .show(ui, |ui| {
            egui::Grid::new("shortcuts-grid")
                .num_columns(3)
                .striped(true)
                .show(ui, |ui| {
                    for entry in prism_editor_core::SHORTCUTS {
                        let capturing = state.shortcut_capture == Some(entry.command);
                        let overridden = preferences
                            .shortcut_overrides
                            .contains_key(entry.command.id());
                        let keys = prism_editor_core::effective_keys(
                            entry.command,
                            &preferences.shortcut_overrides,
                        )
                        .unwrap_or_else(|| entry.keys.to_string());
                        let text = if capturing {
                            "Press…".to_string()
                        } else {
                            keys
                        };
                        let mut rich = RichText::new(text);
                        if overridden || capturing {
                            rich = rich.strong();
                        }
                        if ui
                            .add(egui::Button::new(rich))
                            .on_hover_text("Click, then press the new key")
                            .clicked()
                        {
                            state.shortcut_capture = Some(entry.command);
                        }
                        ui.label(format!("{:?}", entry.command));
                        if overridden {
                            if ui.small_button("Reset").clicked() {
                                preferences.shortcut_overrides.remove(entry.command.id());
                                if state.shortcut_capture == Some(entry.command) {
                                    state.shortcut_capture = None;
                                }
                            }
                        } else {
                            ui.label("");
                        }
                        ui.end_row();
                    }
                });
        });
}

pub(super) enum CapturedShortcut {
    Cancel,
    Bound(String),
}

pub(super) fn captured_shortcut(ui: &Ui) -> Option<CapturedShortcut> {
    ui.input(|input| {
        input.events.iter().find_map(|event| match event {
            egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } => {
                if *key == egui::Key::Escape {
                    Some(CapturedShortcut::Cancel)
                } else {
                    let chord = prism_editor_core::KeyChord::from_event(
                        modifiers.ctrl || modifiers.command,
                        modifiers.shift,
                        modifiers.alt,
                        key.name(),
                    );
                    Some(CapturedShortcut::Bound(chord.display()))
                }
            }
            _ => None,
        })
    })
}

pub fn about(ui: &mut Ui) {
    ui.heading("Prisma Editor");
    ui.label("egui + ash Vulkan editor with zero-copy Scene / Game viewports.");
    ui.separator();
    ui.label(
        RichText::new(
            "Scene View renders the edited document with an editor camera; \
             Game View uses the document camera and runs the simulation on Play.",
        )
        .weak()
        .size(11.0),
    );
}

pub fn tools(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    ui.heading("Offline tools");
    ui.label(
        RichText::new(
            "Runs `cargo run -p prism-build-pipeline -- …` on the host. \
             Output streams into the Console; the editor never links the cooker.",
        )
        .weak()
        .size(11.0),
    );
    if state.build_running {
        ui.label(
            RichText::new("a task is already running…")
                .weak()
                .size(11.0),
        );
    }
    ui.separator();

    egui::CollapsingHeader::new("GI Bake")
        .default_open(true)
        .show(ui, |ui| {
            ui.label(
                RichText::new("Offline probe-volume bake (GPU ray query, multi-bounce).")
                    .weak()
                    .size(10.5),
            );
            ui.horizontal(|ui| {
                ui.label("rays");
                ui.add(
                    egui::DragValue::new(&mut state.offline.gi_rays)
                        .speed(4.0)
                        .range(1..=4096),
                )
                .on_hover_text("Rays per probe");
                ui.label("bounces");
                ui.add(
                    egui::DragValue::new(&mut state.offline.gi_bounces)
                        .speed(0.2)
                        .range(1..=16),
                )
                .on_hover_text("Max bounces");
            });
            ui.horizontal(|ui| {
                ui.label("output");
                ui.add(
                    egui::TextEdit::singleline(&mut state.offline.gi_output)
                        .desired_width(ui.available_width().max(120.0)),
                );
            });
            let (output, rays, bounces) = (
                state.offline.gi_output.trim().to_string(),
                state.offline.gi_rays,
                state.offline.gi_bounces,
            );
            ui.add_enabled_ui(!state.build_running, |ui| {
                if ui.button("Bake GI").clicked() {
                    state.pending_build = Some(crate::build::BuildKind::BakeGi {
                        output,
                        rays,
                        bounces,
                    });
                }
            });
            ui.add_space(3.0);
            prt_preview_controls(ui, workspace, None);
        });

    egui::CollapsingHeader::new("Heightmap")
        .default_open(true)
        .show(ui, |ui| {
            ui.label(
                RichText::new("Procedural fbm terrain + hydraulic / thermal erosion (raw f32).")
                    .weak()
                    .size(10.5),
            );
            ui.horizontal(|ui| {
                ui.label("size");
                ui.add(
                    egui::DragValue::new(&mut state.offline.heightmap_width)
                        .speed(8.0)
                        .range(16..=8192),
                );
                ui.label("×");
                ui.add(
                    egui::DragValue::new(&mut state.offline.heightmap_height)
                        .speed(8.0)
                        .range(16..=8192),
                );
            });
            ui.horizontal(|ui| {
                ui.label("seed");
                ui.add(
                    egui::DragValue::new(&mut state.offline.heightmap_seed)
                        .speed(1.0)
                        .range(0..=u64::MAX),
                );
                ui.label("erosion");
                ui.add(
                    egui::DragValue::new(&mut state.offline.heightmap_iterations)
                        .speed(4.0)
                        .range(0..=10_000),
                )
                .on_hover_text("Erosion iterations");
            });
            ui.horizontal(|ui| {
                ui.label("sea level");
                ui.add(
                    egui::DragValue::new(&mut state.offline.heightmap_sea_level)
                        .speed(0.01)
                        .range(0.0..=1.0)
                        .max_decimals(2),
                )
                .on_hover_text("0 = all land, 1 = all ocean (0.7 ≈ Earth)");
            });
            ui.horizontal(|ui| {
                ui.label("output");
                ui.add(
                    egui::TextEdit::singleline(&mut state.offline.heightmap_output)
                        .desired_width(ui.available_width().max(120.0)),
                );
            });
            let (output, width, height, seed, iterations, sea_level) = (
                state.offline.heightmap_output.trim().to_string(),
                state.offline.heightmap_width,
                state.offline.heightmap_height,
                state.offline.heightmap_seed,
                state.offline.heightmap_iterations,
                state.offline.heightmap_sea_level,
            );
            ui.add_enabled_ui(!state.build_running, |ui| {
                if ui.button("Generate Heightmap").clicked() {
                    state.pending_build = Some(crate::build::BuildKind::Heightmap {
                        output,
                        width,
                        height,
                        seed,
                        iterations,
                        sea_level,
                    });
                }
            });
        });

    egui::CollapsingHeader::new("Cook / Build Scenes")
        .default_open(true)
        .show(ui, |ui| {
            ui.label(
                RichText::new("Cook every scene in `assets/scenes.json` into the runtime `.pak`.")
                    .weak()
                    .size(10.5),
            );
            ui.horizontal(|ui| {
                ui.label("output");
                ui.add(
                    egui::TextEdit::singleline(&mut state.offline.pack_output)
                        .desired_width(ui.available_width().max(120.0)),
                );
            });
            ui.horizontal(|ui| {
                ui.label("compression");
                ui.add(
                    egui::DragValue::new(&mut state.offline.pack_compression)
                        .speed(0.2)
                        .range(0..=22),
                )
                .on_hover_text("zstd level (0 = store)");
            });
            let (output, compression) = (
                state.offline.pack_output.trim().to_string(),
                state.offline.pack_compression,
            );
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!state.build_running, |ui| {
                    if ui.button("Build Scenes").clicked() {
                        state.pending_build = Some(crate::build::BuildKind::ScenePackCustom {
                            output,
                            compression,
                        });
                    }

                    let cook_path = workspace
                        .project_root
                        .as_ref()
                        .zip(workspace.document().path.as_ref())
                        .map(|(root, path)| root.join(path));
                    if ui
                        .add_enabled(cook_path.is_some(), egui::Button::new("Cook active scene"))
                        .on_hover_text("Single scene → RSCN (assets/cooked)")
                        .clicked()
                    {
                        if let Some(path) = cook_path {
                            state.pending_build = Some(crate::build::BuildKind::Cook(path));
                        }
                    }
                });
            });
        });
}

pub fn boolean_window(
    ctx: &egui::Context,
    open: &mut bool,
    title: &str,
    body: impl FnOnce(&mut Ui),
) {
    if !*open {
        return;
    }
    let mut visible = true;
    egui::Window::new(title)
        .open(&mut visible)
        .resizable(true)
        .default_width(420.0)
        .show(ctx, |ui| body(ui));
    if !visible {
        *open = false;
    }
}
