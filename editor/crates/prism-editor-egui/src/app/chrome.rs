//! Editor menu bar, toolbar, status bar, and modal dialogs.

use super::*;

pub(super) enum CloseDecision {
    Save,
    Discard,
    Cancel,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SaveConflictChoice {
    Overwrite,
    Reload,
    Cancel,
}

impl EditorApp {
    pub(super) fn menu_item(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        shortcut: &str,
        command: Command,
    ) {
        let enabled = self.workspace.is_enabled(command);
        let text = if shortcut.is_empty() {
            label.to_owned()
        } else {
            format!("{label}\t{shortcut}")
        };
        if ui.add_enabled(enabled, egui::Button::new(text)).clicked() {
            self.apply_command(command);
            ui.close();
        }
    }

    pub(super) fn request_quit(&mut self, ctx: &egui::Context) {
        if self
            .workspace
            .docs
            .iter()
            .any(|document| document.is_dirty())
        {
            self.state.pending_close = Some(panels::CloseTarget::App);
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    pub(super) fn menu_do(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        action: impl FnOnce(&mut Self),
    ) {
        if ui.button(label).clicked() {
            action(self);
            ui.close();
        }
    }

    pub(super) fn open_prompt(
        &mut self,
        kind: panels::PromptKind,
        title: &str,
        hint: &str,
        value: impl Into<String>,
    ) {
        self.state.prompt = Some(panels::Prompt::new(kind, title, hint, value));
    }

    pub(super) fn menu_bar(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        let colors = self.workspace.colors();
        let text_soft = theme::color32(&colors, prism_editor_core::ThemeToken::TextSoft);
        let text_strong = theme::color32(&colors, prism_editor_core::ThemeToken::TextStrong);
        let menu_hover = theme::color32(&colors, prism_editor_core::ThemeToken::MenuHover);
        let menu_active = theme::color32(&colors, prism_editor_core::ThemeToken::MenuActive);
        let style = ui.style_mut();
        style.spacing.button_padding = egui::vec2(7.0, 1.0);
        let widgets = &mut style.visuals.widgets;
        widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
        widgets.inactive.bg_fill = Color32::TRANSPARENT;
        widgets.inactive.bg_stroke = Stroke::NONE;
        widgets.inactive.fg_stroke = Stroke::new(1.0_f32, text_soft);
        widgets.hovered.weak_bg_fill = menu_hover;
        widgets.hovered.bg_fill = menu_hover;
        widgets.hovered.bg_stroke = Stroke::NONE;
        widgets.hovered.fg_stroke = Stroke::new(1.0_f32, text_strong);
        widgets.open.weak_bg_fill = menu_active;
        widgets.open.bg_fill = menu_active;
        widgets.open.bg_stroke = Stroke::NONE;
        widgets.open.fg_stroke = Stroke::new(1.0_f32, text_strong);

        egui::MenuBar::new().ui(ui, |ui| {
            // ---- File ----
            ui.menu_button("File", |ui| {
                self.menu_item(ui, "New Scene", "Ctrl+N", Command::NewScene);
                let scene_templates: Vec<(String, String)> = self
                    .workspace
                    .templates
                    .iter()
                    .filter(|template| template.kind == prism_editor_core::DocKind::Scene)
                    .map(|template| (template.id.clone(), template.label.clone()))
                    .collect();
                if !scene_templates.is_empty() {
                    ui.menu_button("New Scene From Template", |ui| {
                        for (id, label) in &scene_templates {
                            if ui.button(label).clicked() {
                                self.workspace
                                    .new_document(prism_editor_core::DocKind::Scene, Some(id));
                                self.synced_doc_state = None;
                                ui.close();
                            }
                        }
                    });
                }
                self.menu_item(ui, "New Prefab", "", Command::NewPrefab);
                ui.separator();
                self.menu_item(ui, "Open Scene…", "Ctrl+O", Command::OpenDocument);
                let docs: Vec<(usize, String, bool)> = self
                    .workspace
                    .docs
                    .iter()
                    .enumerate()
                    .map(|(index, doc)| {
                        (index, doc.title(), index == self.workspace.active_index())
                    })
                    .collect();
                ui.menu_button("Recent Scenes", |ui| {
                    for (index, title, active) in &docs {
                        if ui.selectable_label(*active, title).clicked() {
                            self.workspace.set_active(*index);
                            self.synced_doc_state = None;
                            ui.close();
                        }
                    }
                });
                self.menu_item(ui, "Save", "Ctrl+S", Command::Save);
                self.menu_item(ui, "Save As…", "Ctrl+Shift+S", Command::SaveAs);
                self.menu_do(ui, "Save Project", |app| {
                    let _ = app.save_all_documents();
                });
                self.menu_item(ui, "Reload Scene", "", Command::ReloadDocument);
                ui.separator();
                self.menu_item(ui, "Build Settings…", "", Command::ToggleBuild);
                if ui
                    .add_enabled(
                        !self.build_job_running(),
                        egui::Button::new("Build Scene Pack"),
                    )
                    .clicked()
                {
                    self.start_build(crate::build::BuildKind::ScenePack);
                    ui.close();
                }
                if ui
                    .add_enabled(
                        !self.build_job_running(),
                        egui::Button::new("Cook Active Scene"),
                    )
                    .clicked()
                {
                    self.cook_active_scene();
                    ui.close();
                }
                if ui
                    .add_enabled(!self.build_job_running(), egui::Button::new("Build Player"))
                    .clicked()
                {
                    self.start_build(crate::build::BuildKind::Player);
                    ui.close();
                }
                if ui
                    .add_enabled(
                        !self.build_job_running(),
                        egui::Button::new("Build And Run  (Ctrl+B)"),
                    )
                    .clicked()
                {
                    self.start_build(crate::build::BuildKind::PlayerRun);
                    ui.close();
                }
                let module_available = self
                    .project_module_status()
                    .is_some_and(|status| status.available);
                if ui
                    .add_enabled(
                        module_available,
                        egui::Button::new("Compile Project Module  (Ctrl+R)"),
                    )
                    .clicked()
                {
                    self.refresh_project_module();
                    ui.close();
                }
                let mut auto_refresh = self.project_auto_refresh();
                if ui
                    .add_enabled(
                        module_available,
                        egui::Checkbox::new(&mut auto_refresh, "Auto Refresh Project Module"),
                    )
                    .changed()
                {
                    self.set_project_auto_refresh(auto_refresh);
                }
                if ui
                    .add_enabled(!self.build_job_running(), egui::Button::new("Clean Build"))
                    .clicked()
                {
                    self.state.confirm = Some(panels::Confirm::new(
                        panels::ConfirmKind::CleanPlayer,
                        "Clean Build",
                        "清理玩家工程（projects/template）的编译产物，下次 Build 会全量重编。\n\
                         不会动仓库根的引擎 target。",
                        "Clean Player",
                    ));
                    ui.close();
                }
                if ui
                    .add_enabled(
                        !self.build_job_running(),
                        egui::Button::new("Clean Engine Cache…"),
                    )
                    .clicked()
                {
                    self.state.confirm = Some(panels::Confirm::new(
                        panels::ConfirmKind::CleanEngine,
                        "Clean Engine Cache",
                        "清理仓库根 workspace 的 target/（引擎 + 全部 crate 的编译产物，\n\
                         通常几个 GB，重编需要十几分钟）。",
                        "Clean Engine",
                    ));
                    ui.close();
                }
                if self.build_job_running() && ui.button("Cancel Build").clicked() {
                    self.cancel_build();
                    ui.close();
                }
                ui.separator();
                let quit_ctx = ui.ctx().clone();
                self.menu_do(ui, "Exit", |app| app.request_quit(&quit_ctx));
            });

            // ---- Edit ----
            ui.menu_button("Edit", |ui| {
                self.menu_do(ui, "Search… (Ctrl+K)", |app| {
                    app.palette.open(&app.workspace)
                });
                ui.separator();
                self.menu_item(ui, "Undo", "Ctrl+Z", Command::Undo);
                self.menu_item(ui, "Redo", "Ctrl+Y", Command::Redo);
                ui.separator();
                self.menu_item(ui, "Cut", "Ctrl+X", Command::CutEntity);
                self.menu_item(ui, "Copy", "Ctrl+C", Command::CopyEntity);
                self.menu_item(ui, "Paste", "Ctrl+V", Command::PasteEntity);
                self.menu_item(ui, "Duplicate", "Ctrl+D", Command::DuplicateEntity);
                self.menu_item(ui, "Delete", "Del", Command::DeleteSelection);
                ui.separator();
                self.menu_item(ui, "Select All", "Ctrl+A", Command::SelectAll);
                self.menu_do(ui, "Deselect All", |app| app.workspace.clear_selection());
                self.menu_do(ui, "Invert Selection", |app| app.invert_selection());
                self.menu_do(ui, "Select Children", |app| {
                    app.select_children_of_selection()
                });
                self.menu_do(ui, "Select Prefab Root", |app| app.select_prefab_root());
                ui.separator();
                self.menu_do(ui, "Selection Back", |app| {
                    app.navigate_selection_history(-1)
                });
                self.menu_do(ui, "Selection Forward", |app| {
                    app.navigate_selection_history(1)
                });
                self.menu_do(ui, "Find in Hierarchy", |app| app.find_in_hierarchy());
                ui.separator();
                ui.menu_button("Copy Transform", |ui| {
                    for field in ["position", "rotation", "scale"] {
                        if ui.button(field).clicked() {
                            self.copy_transform(field);
                            ui.close();
                        }
                    }
                });
                ui.menu_button("Paste Transform", |ui| {
                    for field in ["position", "rotation", "scale"] {
                        if ui.button(field).clicked() {
                            self.paste_transform(field);
                            ui.close();
                        }
                    }
                });
                ui.separator();
                self.menu_item(
                    ui,
                    "Paste Component",
                    "Ctrl+Shift+V",
                    Command::PasteComponent,
                );
                self.menu_item(
                    ui,
                    "Clear Component Clipboard",
                    "",
                    Command::ClearComponentClipboard,
                );
                ui.separator();
                self.menu_item(ui, "Play", "Ctrl+P", Command::Play);
                self.menu_item(ui, "Pause", "Ctrl+Shift+P", Command::Pause);
                self.menu_item(ui, "Step", "Ctrl+Alt+P", Command::Step);
                ui.separator();
                self.menu_item(ui, "Project Settings…", "", Command::ProjectSettings);
                self.menu_item(ui, "Preferences…", "", Command::Preferences);
                self.menu_item(ui, "Theme…", "", Command::ThemePanel);
            });

            // ---- Assets ----
            ui.menu_button("Assets", |ui| {
                ui.menu_button("Create", |ui| {
                    self.menu_item(ui, "Scene", "", Command::NewScene);
                    self.menu_item(ui, "Prefab", "", Command::NewPrefab);
                    let can_variant = self.has_prefab_instance();
                    if ui
                        .add_enabled(can_variant, egui::Button::new("Prefab Variant…"))
                        .clicked()
                    {
                        self.open_prompt(
                            panels::PromptKind::PrefabVariant,
                            "Create Prefab Variant",
                            "变体文件名（不含目录，写到 assets/prefabs/）",
                            "Variant",
                        );
                        ui.close();
                    }
                    self.menu_do(ui, "Folder…", |app| {
                        app.open_prompt(
                            panels::PromptKind::NewFolder,
                            "New Folder",
                            "assets/ 下的相对目录，例如 prefabs/enemies",
                            "assets/",
                        )
                    });
                });
                let selected = self.state.asset_primary().cloned();
                if let Some(path) = selected.as_deref() {
                    self.menu_do(ui, "Open", |app| {
                        if let Err(error) = app.workspace.open_project_file(path) {
                            app.state.status = format!("open {path} failed: {error}");
                        }
                    });
                    let selected_path = path.to_string();
                    self.menu_do(ui, "Show in Explorer", |app| {
                        app.state.status =
                            match panels::reveal_asset(&app.workspace, &selected_path) {
                                Ok(()) => format!("revealed {selected_path}"),
                                Err(error) => error,
                            };
                    });
                    let ctx_copy = ui.ctx().clone();
                    let copy_path = selected_path.clone();
                    self.menu_do(ui, "Copy Path", move |app| {
                        ctx_copy.copy_text(copy_path.clone());
                        app.state.status = format!("copied {copy_path}");
                    });
                    self.menu_do(ui, "Duplicate", |app| {
                        match app.workspace.duplicate_asset(path) {
                            Ok(copy) => app.state.status = format!("duplicated → {copy}"),
                            Err(error) => app.state.status = format!("duplicate failed: {error}"),
                        }
                    });
                    self.menu_do(ui, "Delete", |app| match app.workspace.delete_asset(path) {
                        Ok(()) => {
                            app.state.forget_asset(path);
                            app.state.status = format!("deleted {path}");
                        }
                        Err(error) => app.state.status = format!("delete failed: {error}"),
                    });
                    if path.ends_with(".prefab.json") {
                        self.menu_do(ui, "Instantiate In Scene", |app| {
                            match app.workspace.instantiate_prefab_asset(path, None) {
                                Ok(count) => {
                                    app.state.status = format!("instantiated {count} entities");
                                    app.synced_doc_state = None;
                                }
                                Err(error) => {
                                    app.state.status = format!("instantiate failed: {error}")
                                }
                            }
                        });
                    }
                } else {
                    ui.add_enabled(false, egui::Button::new("Open"));
                    ui.add_enabled(false, egui::Button::new("Show in Explorer"));
                    ui.add_enabled(false, egui::Button::new("Duplicate"));
                    ui.add_enabled(false, egui::Button::new("Delete"));
                }
                ui.separator();
                self.menu_do(ui, "Find References In Scene", |app| {
                    if let Some(path) = app.state.asset_primary().cloned() {
                        app.find_references(&path, false);
                    }
                });
                self.menu_do(ui, "Find References In Project", |app| {
                    if let Some(path) = app.state.asset_primary().cloned() {
                        app.find_references(&path, true);
                    }
                });
                ui.separator();
                self.menu_do(ui, "Refresh", |app| {
                    app.workspace.refresh_project_files();
                    app.state.status = "assets refreshed".to_string();
                });
            });

            // ---- GameObject ----
            ui.menu_button("GameObject", |ui| {
                self.menu_do(ui, "Create Empty", |app| app.workspace.add_entity(""));
                self.menu_do(ui, "Create Empty Child", |app| app.create_empty_child());
                self.menu_do(ui, "Create Empty Parent", |app| app.create_empty_parent());
                ui.separator();
                ui.menu_button("Create Other", |ui| {
                    ui.menu_button("3D Object", |ui| {
                        for (id, label) in [
                            ("cube", "Cube"),
                            ("sphere", "Sphere"),
                            ("cylinder", "Cylinder"),
                            ("capsule", "Capsule"),
                            ("quad", "Quad"),
                            ("plane", "Plane"),
                        ] {
                            let known = self.workspace.presets.iter().any(|p| p.id == id);
                            if ui.add_enabled(known, egui::Button::new(label)).clicked() {
                                self.workspace.add_entity(id);
                                ui.close();
                            }
                        }
                    });
                    ui.menu_button("Rendering", |ui| {
                        for (id, label) in [("mesh", "Mesh Renderer"), ("skybox", "Skybox")] {
                            let known = self.workspace.presets.iter().any(|p| p.id == id);
                            if ui.add_enabled(known, egui::Button::new(label)).clicked() {
                                self.workspace.add_entity(id);
                                ui.close();
                            }
                        }
                    });
                    ui.menu_button("Light", |ui| {
                        for (id, label) in [
                            ("sun", "Directional Light"),
                            ("point_light", "Point Light"),
                            ("spot_light", "Spot Light"),
                        ] {
                            let known = self.workspace.presets.iter().any(|p| p.id == id);
                            if ui.add_enabled(known, egui::Button::new(label)).clicked() {
                                self.workspace.add_entity(id);
                                ui.close();
                            }
                        }
                    });
                    ui.menu_button("Camera", |ui| {
                        let known = self.workspace.presets.iter().any(|p| p.id == "camera");
                        if ui.add_enabled(known, egui::Button::new("Camera")).clicked() {
                            self.workspace.add_entity("camera");
                            ui.close();
                        }
                    });
                    ui.menu_button("UI", |ui| {
                        let known = self.workspace.presets.iter().any(|p| p.id == "ui");
                        if ui
                            .add_enabled(known, egui::Button::new("UI Element"))
                            .clicked()
                        {
                            self.workspace.add_entity("ui");
                            ui.close();
                        }
                    });
                });
                ui.separator();
                self.menu_do(ui, "Move To View", |app| {
                    if app.workspace.align_selection_to_view() {
                        app.state.status = "moved selection to view".to_string();
                        app.mark_scene_dirty();
                    } else {
                        app.state.status = "Move To View needs a selection".to_string();
                    }
                });
                self.menu_do(ui, "Align View to Selected", |app| {
                    if app.workspace.align_view_to_selection() {
                        app.mark_scene_dirty();
                    }
                });
                self.menu_do(ui, "Snap to Ground", |app| {
                    let count = app.workspace.snap_selection_to_ground();
                    app.state.status = format!("snapped {count} entities");
                    app.mark_scene_dirty();
                });
            });

            // ---- Component ----
            ui.menu_button("Component", |ui| {
                let attachable: std::collections::HashSet<String> = self
                    .workspace
                    .component_types()
                    .into_iter()
                    .map(|component| component.name.clone())
                    .collect();
                let categories: Vec<(String, Vec<(String, String)>)> = self
                    .workspace
                    .schema
                    .component_menu()
                    .into_iter()
                    .map(|(category, items)| {
                        (
                            category.to_string(),
                            items
                                .into_iter()
                                .filter(|item| attachable.contains(&item.name))
                                .map(|item| (item.name.clone(), item.display_name().to_string()))
                                .collect(),
                        )
                    })
                    .filter(|(_, items): &(String, Vec<(String, String)>)| !items.is_empty())
                    .collect();
                if categories.is_empty() {
                    ui.label("no more components");
                }
                for (category, items) in categories {
                    ui.menu_button(category, |ui| {
                        for (name, label) in &items {
                            if ui.button(label).clicked() {
                                self.workspace.add_component(name);
                                ui.close();
                            }
                        }
                    });
                }
            });

            // ---- Window ----
            ui.menu_button("Window", |ui| {
                ui.menu_button("General", |ui| {
                    for (label, command) in [
                        ("Scene", Command::ToggleSceneView),
                        ("Game", Command::ToggleGameView),
                        ("Hierarchy", Command::ToggleHierarchy),
                        ("Project", Command::ToggleAssets),
                        ("Inspector", Command::ToggleInspector),
                        ("Console", Command::ToggleLog),
                        ("Documents", Command::ToggleDocs),
                        ("History", Command::ToggleHistory),
                        ("Render Graph", Command::ToggleRenderGraph),
                        ("Build Settings", Command::ToggleBuild),
                        ("Offline Tools", Command::ToggleTools),
                        ("Project Settings", Command::ProjectSettings),
                        ("Preferences", Command::Preferences),
                        ("Theme", Command::ThemePanel),
                    ] {
                        self.menu_item(ui, label, "", command);
                    }
                    if ui.button("Problems").clicked() {
                        self.workspace.show_panel("problems");
                        ui.close();
                    }
                    if ui.button("Prefab Overrides").clicked() {
                        self.workspace.show_panel("prefabOverrides");
                        ui.close();
                    }
                    let mut maximize_on_play = self.workspace.maximize_on_play;
                    if ui
                        .checkbox(&mut maximize_on_play, "Maximize Game View on Play")
                        .on_hover_text(
                            "Enter Play with the Game view maximized (Unity maximizeOnPlay)",
                        )
                        .changed()
                    {
                        self.workspace.maximize_on_play = maximize_on_play;
                    }
                    if ui.button("Material").clicked() {
                        self.workspace.show_panel("material");
                        ui.close();
                    }
                    if ui.button("Animation Graph").clicked() {
                        self.workspace.show_panel("animGraph");
                        ui.close();
                    }
                    if ui.button("GASP Debug").clicked() {
                        self.workspace.show_panel("gaspDebug");
                        ui.close();
                    }
                });
                ui.separator();
                self.menu_do(ui, "Capture Window", |app| app.capture_window_now());
                ui.menu_button("Layouts", |ui| {
                    for (id, label) in prism_editor_core::workspace::layout_preset_ids() {
                        let active = self.workspace.layout_preset == id;
                        if ui.selectable_label(active, label).clicked() {
                            self.workspace.set_layout_preset(id);
                            ui.close();
                        }
                    }
                    ui.separator();
                    let names: Vec<String> = self.named_layouts.keys().cloned().collect();
                    for name in names {
                        ui.menu_button(&name, |ui| {
                            if ui.button("Load").clicked() {
                                let value = self.named_layouts.get(&name).cloned();
                                if let Some(value) = value {
                                    self.apply_layout_json(&value);
                                    self.state.status = format!("layout loaded: {name}");
                                }
                                ui.close();
                            }
                            if ui.button("Delete").clicked() {
                                self.named_layouts.remove(&name);
                                self.write_named_layouts();
                                ui.close();
                            }
                        });
                    }
                    if !self.named_layouts.is_empty() {
                        ui.separator();
                    }
                    self.menu_do(ui, "Save Layout…", |app| {
                        app.open_prompt(
                            panels::PromptKind::LayoutName,
                            "Save Layout",
                            "布局名（写进 target/editor-egui/layouts.json）",
                            "My Layout",
                        )
                    });
                    self.menu_do(ui, "Save Layout to File…", |app| {
                        app.open_prompt(
                            panels::PromptKind::LayoutFileSave,
                            "Save Layout to File",
                            "输出 JSON 路径",
                            "target/editor-egui/layout.json",
                        )
                    });
                    self.menu_do(ui, "Load Layout from File…", |app| {
                        app.open_prompt(
                            panels::PromptKind::LayoutFileLoad,
                            "Load Layout from File",
                            "读取 JSON 路径（v4 {version,dock,floating}）",
                            "target/editor-egui/layout.json",
                        )
                    });
                    self.menu_do(ui, "Reset All Layouts", |app| {
                        app.named_layouts.clear();
                        app.write_named_layouts();
                        app.workspace.set_layout_preset("default");
                        app.state.status = "layouts reset".to_string();
                    });
                });
                ui.separator();
                self.menu_item(ui, "Reset Layout", "", Command::ResetLayout);
            });

            // ---- Help ----
            ui.menu_button("Help", |ui| {
                self.menu_item(ui, "Shortcuts", "", Command::Shortcuts);
                self.menu_do(ui, "Editor Guide", |app| {
                    app.state.status = "docs/editor-egui.md".to_string();
                });
                self.menu_item(ui, "About Prisma", "", Command::About);
            });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let colors = self.workspace.colors();
                let accent = theme::color32(&colors, prism_editor_core::ThemeToken::Accent);
                ui.label(
                    RichText::new("zero-copy viewports")
                        .size(10.0)
                        .color(accent),
                );
                if let Some(view) = &self.scene_view {
                    ui.label(
                        RichText::new(format!("{:.1} ms", view.last_frame_ms))
                            .size(10.0)
                            .weak(),
                    );
                }
                let build_label = self
                    .build
                    .as_ref()
                    .filter(|job| job.running())
                    .map(|job| job.label().to_string());
                if let Some(label) = build_label {
                    if ui
                        .small_button(RichText::new("Cancel").size(10.0))
                        .on_hover_text("Stop the running cargo build")
                        .clicked()
                    {
                        self.cancel_build();
                    }
                    ui.label(
                        RichText::new(format!("building: {label}"))
                            .size(10.0)
                            .color(egui::Color32::from_rgb(224, 176, 80)),
                    );
                }
            });
        });
    }

    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui) {
        use prism_editor_core::ThemeToken;
        let colors = self.workspace.colors();
        let sunken = theme::color32(&colors, ThemeToken::BgSunken);
        let border = theme::color32(&colors, ThemeToken::Border);
        let border_strong = theme::color32(&colors, ThemeToken::BorderStrong);
        let selection = theme::color32(&colors, ThemeToken::Selection);
        let accent = theme::color32(&colors, ThemeToken::Accent);
        let text_strong = theme::color32(&colors, ThemeToken::TextStrong);
        let muted = theme::color32(&colors, ThemeToken::Muted);
        let full = ui.max_rect();

        let playing = self.playing;
        let paused = self.paused;
        let island_w = 132.0_f32;
        let wide_layout = full.width() >= 1040.0;
        let center_min_x = (full.center().x - island_w * 0.5).round();
        let center_rect = Rect::from_min_max(
            egui::pos2(center_min_x, full.min.y),
            egui::pos2(center_min_x + island_w, full.max.y),
        );
        let left_rect = if wide_layout {
            Rect::from_min_max(
                full.min,
                egui::pos2(
                    (center_rect.min.x - 10.0).max(full.min.x + 200.0),
                    full.max.y,
                ),
            )
        } else {
            full
        };

        let render_transport = |ui: &mut egui::Ui, app: &mut Self| {
            let island_stroke = if playing {
                Stroke::new(1.0_f32, panels::play_mode_tint(paused))
            } else {
                Stroke::new(1.0_f32, border_strong)
            };
            let island_fill = if playing {
                if paused {
                    theme::color32(&colors, ThemeToken::WarningBorder).gamma_multiply(0.45)
                } else {
                    theme::color32(&colors, ThemeToken::SuccessBg)
                }
            } else {
                sunken
            };
            egui::Frame::new()
                .fill(island_fill)
                .stroke(island_stroke)
                .corner_radius(egui::CornerRadius::same(4))
                .inner_margin(egui::Margin::symmetric(5, 1))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let transport_btn =
                        |ui: &mut egui::Ui,
                         glyph: &str,
                         tip: &str,
                         enabled: bool,
                         tint: Option<Color32>| {
                            let mut text = RichText::new(glyph).size(13.0);
                            if enabled {
                                if let Some(c) = tint {
                                    text = text.color(c);
                                }
                            }
                            ui.add_enabled(enabled, egui::Button::new(text).frame(true))
                                .on_hover_text(tip)
                                .clicked()
                        };
                    let play_tint = if playing && !paused {
                        Some(theme::color32(&colors, ThemeToken::Success))
                    } else {
                        None
                    };
                    if transport_btn(ui, icons::PLAY, "Play (Ctrl+P)", !playing, play_tint) {
                        app.apply_command(Command::Play);
                    }
                    let pause_tint = if paused {
                        Some(theme::color32(&colors, ThemeToken::Warning))
                    } else {
                        None
                    };
                    if transport_btn(
                        ui,
                        if paused { icons::PLAY } else { icons::PAUSE },
                        if paused { "Resume" } else { "Pause" },
                        playing,
                        pause_tint,
                    ) {
                        app.apply_command(Command::Pause);
                    }
                    if transport_btn(ui, icons::SKIP_FORWARD, "Step Frame", playing, None) {
                        app.apply_command(Command::Step);
                    }
                    let stop_tint = if playing {
                        Some(theme::color32(&colors, ThemeToken::DangerSoft))
                    } else {
                        None
                    };
                    if transport_btn(ui, icons::STOP, "Stop", playing, stop_tint) {
                        app.apply_command(Command::Stop);
                    }
                });
        };

        let left_end_x = ui
            .scope_builder(egui::UiBuilder::new().max_rect(left_rect), |ui| {
                ui.horizontal(|ui| {
                    if let Some(origin) = self.workspace.document().prefab_origin.clone() {
                        let prefab_title = self.workspace.document().title();
                        if ui
                            .small_button(format!("{} {}", icons::ARROW_LEFT, origin.scene_title))
                            .on_hover_text("Back to the scene this prefab instance came from")
                            .clicked()
                        {
                            let prefab_index = self.workspace.active_index();
                            if self.workspace.return_from_prefab(prefab_index) {
                                self.state.status = format!(
                                    "returned to {} · selected {}",
                                    origin.scene_title, prefab_title
                                );
                            }
                        }
                        ui.separator();
                    }

                    let dirty = self.workspace.document().dirty;
                    let save_icon = if dirty {
                        RichText::new(icons::FLOPPY_DISK).color(accent)
                    } else {
                        RichText::new(icons::FLOPPY_DISK).color(muted)
                    };
                    if ui
                        .small_button(save_icon)
                        .on_hover_text("Save Active Document (Ctrl+S)")
                        .clicked()
                    {
                        self.apply_command(Command::Save);
                    }
                    if ui
                        .add_enabled(
                            self.workspace.can_undo(),
                            egui::Button::new(icons::ARROW_COUNTER_CLOCKWISE).small(),
                        )
                        .on_hover_text("Undo (Ctrl+Z)")
                        .clicked()
                    {
                        self.apply_command(Command::Undo);
                    }
                    if ui
                        .add_enabled(
                            self.workspace.can_redo(),
                            egui::Button::new(icons::ARROW_CLOCKWISE).small(),
                        )
                        .on_hover_text("Redo (Ctrl+Y)")
                        .clicked()
                    {
                        self.apply_command(Command::Redo);
                    }

                    ui.separator();

                    // Segmented gizmo tool pill group matching UE5 / Unity 6 tool strips.
                    egui::Frame::new()
                        .fill(sunken)
                        .stroke(Stroke::new(1.0_f32, border))
                        .corner_radius(egui::CornerRadius::same(3))
                        .inner_margin(egui::Margin::symmetric(2, 1))
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 1.0;
                            for (tool, icon, key, tip) in [
                                (
                                    crate::viewport::GizmoTool::Translate,
                                    icons::ARROWS_OUT_CARDINAL,
                                    "W",
                                    "Move Tool (W)",
                                ),
                                (
                                    crate::viewport::GizmoTool::Rotate,
                                    icons::ARROWS_CLOCKWISE,
                                    "E",
                                    "Rotate Tool (E)",
                                ),
                                (
                                    crate::viewport::GizmoTool::Scale,
                                    icons::ARROWS_OUT,
                                    "R",
                                    "Scale Tool (R)",
                                ),
                            ] {
                                let active = self
                                    .scene_view
                                    .as_ref()
                                    .is_some_and(|view| view.tool == tool && !view.sculpt);
                                let fill = if active {
                                    selection
                                } else {
                                    Color32::TRANSPARENT
                                };
                                let fg = if active { text_strong } else { muted };
                                if ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new(format!("{icon} {key}"))
                                                .size(11.0)
                                                .color(fg),
                                        )
                                        .fill(fill)
                                        .frame(true),
                                    )
                                    .on_hover_text(tip)
                                    .clicked()
                                {
                                    self.set_gizmo_tool(tool);
                                    if let Some(view) = self.scene_view.as_mut() {
                                        view.sculpt = false;
                                    }
                                }
                            }
                            let sculpting = self.scene_view.as_ref().is_some_and(|v| v.sculpt);
                            let sculpt_fill = if sculpting {
                                selection
                            } else {
                                Color32::TRANSPARENT
                            };
                            let sculpt_fg = if sculpting { text_strong } else { muted };
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(format!("{} Sculpt", icons::SPARKLE))
                                            .size(11.0)
                                            .color(sculpt_fg),
                                    )
                                    .fill(sculpt_fill),
                                )
                                .on_hover_text(
                                    "Paint Terrain heightmap: drag in Scene view. \
                                     Brush settings live in the Terrain inspector.",
                                )
                                .clicked()
                            {
                                if let Some(view) = self.scene_view.as_mut() {
                                    view.sculpt = !view.sculpt;
                                    view.dirty = true;
                                    self.state.status = if view.sculpt {
                                        "Sculpt: drag on the selected terrain".to_string()
                                    } else {
                                        "Sculpt disabled".to_string()
                                    };
                                }
                            }
                        });

                    let space = self.workspace.gizmo_space;
                    let space_icon = match space {
                        prism_editor_core::viewport_math::GizmoSpace::World => icons::GLOBE,
                        prism_editor_core::viewport_math::GizmoSpace::Local => icons::CUBE,
                    };
                    if ui
                        .small_button(format!("{space_icon} {space:?}"))
                        .on_hover_text("Toggle World / Local gizmo space (X)")
                        .clicked()
                    {
                        self.workspace.toggle_gizmo_space();
                        self.mark_scene_dirty();
                    }

                    let pivot =
                        self.workspace.pivot_mode == prism_editor_core::workspace::PivotMode::Pivot;
                    let pivot_label = if pivot {
                        format!("{} Pivot", icons::CROSSHAIR)
                    } else {
                        format!("{} Center", icons::TARGET)
                    };
                    if ui
                        .selectable_label(pivot, pivot_label)
                        .on_hover_text(
                            "Rotate / scale each object around its own pivot \
                             (off = selection center, group transform)",
                        )
                        .clicked()
                    {
                        self.workspace.pivot_mode = if pivot {
                            prism_editor_core::workspace::PivotMode::Center
                        } else {
                            prism_editor_core::workspace::PivotMode::Pivot
                        };
                        self.mark_scene_dirty();
                    }

                    let snap = self.workspace.snap;
                    if ui
                        .selectable_label(snap.enabled, format!("{} Snap", icons::MAGNET))
                        .on_hover_text(format!(
                            "Snap gizmo drags: {} units / {}° / x{} — hold Ctrl to invert",
                            snap.translate, snap.rotate_degrees, snap.scale
                        ))
                        .clicked()
                    {
                        self.workspace.snap.enabled = !snap.enabled;
                    }

                    if !wide_layout {
                        ui.separator();
                        render_transport(ui, self);
                    }
                    ui.next_widget_position().x
                })
                .inner
            })
            .inner;

        if wide_layout {
            let safe_center_rect = if left_end_x + 6.0 > center_rect.min.x {
                Rect::from_min_max(
                    egui::pos2(left_end_x + 6.0, center_rect.min.y),
                    egui::pos2(left_end_x + 6.0 + island_w, center_rect.max.y),
                )
            } else {
                center_rect
            };
            ui.scope_builder(egui::UiBuilder::new().max_rect(safe_center_rect), |ui| {
                ui.centered_and_justified(|ui| {
                    render_transport(ui, self);
                });
            });

            let right_rect = Rect::from_min_max(
                egui::pos2((safe_center_rect.max.x + 8.0).min(full.max.x), full.min.y),
                full.max,
            );
            if right_rect.width() > 60.0 {
                ui.scope_builder(egui::UiBuilder::new().max_rect(right_rect), |ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .small_button(format!("{} Search", icons::MAGNIFYING_GLASS))
                            .on_hover_text("Command Palette & Search (Ctrl+K)")
                            .clicked()
                        {
                            self.palette.open(&self.workspace);
                        }

                        let active_preset = self.workspace.layout_preset.clone();
                        egui::ComboBox::from_id_salt("toolbar-layout")
                            .width(86.0)
                            .selected_text(format!("{} {}", icons::LAYOUT, active_preset))
                            .show_ui(ui, |ui| {
                                for (id, label) in prism_editor_core::workspace::layout_preset_ids()
                                {
                                    if ui.selectable_label(active_preset == id, label).clicked() {
                                        self.workspace.set_layout_preset(id);
                                    }
                                }
                            });

                        let docs: Vec<String> = self
                            .workspace
                            .docs
                            .iter()
                            .map(|document| document.title_with_dirty())
                            .collect();
                        let active = self.workspace.active_index();
                        let current = docs.get(active).cloned().unwrap_or_default();
                        egui::ComboBox::from_id_salt("toolbar-docs")
                            .width(140.0)
                            .selected_text(current)
                            .show_ui(ui, |ui| {
                                for (index, title) in docs.iter().enumerate() {
                                    if ui.selectable_label(index == active, title).clicked() {
                                        self.workspace.set_active(index);
                                        self.synced_doc_state = None;
                                    }
                                }
                            });

                        if ui
                            .add_enabled(
                                !self.build_job_running(),
                                egui::Button::new(format!("{} Build", icons::HAMMER)).small(),
                            )
                            .on_hover_text("Build and Run Player (Ctrl+B)")
                            .clicked()
                        {
                            self.start_build(crate::build::BuildKind::PlayerRun);
                        }

                        if let Some(status) = self.project_module_status() {
                            if status.available {
                                let dot_color = match status.state {
                                    ModuleState::Ready => {
                                        theme::color32(&colors, ThemeToken::Success)
                                    }
                                    ModuleState::Pending => {
                                        theme::color32(&colors, ThemeToken::Warning)
                                    }
                                    ModuleState::Compiling => accent,
                                    ModuleState::Failed => {
                                        theme::color32(&colors, ThemeToken::Danger)
                                    }
                                    ModuleState::Unavailable => muted,
                                };
                                if ui
                                    .add_enabled(
                                        !status.building,
                                        egui::Button::new(
                                            RichText::new(format!("{} Compile", icons::CIRCLE))
                                                .small()
                                                .color(dot_color),
                                        ),
                                    )
                                    .on_hover_text("Compile & Reload Project Module (Ctrl+R)")
                                    .clicked()
                                {
                                    self.refresh_project_module();
                                }
                            }
                        }
                    });
                });
            }
        }
    }

    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&self.state.status).size(11.0));
            if !self.workspace.status.is_empty() {
                ui.label(RichText::new(&self.workspace.status).size(11.0).weak());
            }

            let failures = self.workspace.project_load_failures();
            if !failures.is_empty() {
                let label = if failures.len() == 1 {
                    "1 document failed to load".to_string()
                } else {
                    format!("{} documents failed to load", failures.len())
                };
                let hint = format!(
                    "{}\n\nFix the file and restart the editor. The editor shows an empty scene \
                     instead of the sample fixture.",
                    failures.join("\n")
                );
                ui.label(
                    RichText::new(label)
                        .size(11.0)
                        .strong()
                        .color(Color32::from_rgb(232, 96, 96)),
                )
                .on_hover_text(hint);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(status) = self.project_module_status() {
                    if status.available {
                        if ui
                            .add_enabled(
                                !status.building,
                                egui::Button::new("Compile (Ctrl+R)").small(),
                            )
                            .on_hover_text(
                                "Recompile and reload the project module \
                                 (whole-module reload: project state resets)",
                            )
                            .clicked()
                        {
                            self.refresh_project_module();
                        }
                        let color = match status.state {
                            ModuleState::Ready => Color32::from_rgb(120, 200, 120),
                            ModuleState::Pending => Color32::from_rgb(220, 190, 100),
                            ModuleState::Compiling => Color32::from_rgb(120, 170, 230),
                            ModuleState::Failed => Color32::from_rgb(232, 96, 96),
                            ModuleState::Unavailable => Color32::from_gray(140),
                        };
                        let label = if status.problems > 0 {
                            format!("{} · {} problems", status.state.label(), status.problems)
                        } else {
                            status.state.label().to_string()
                        };
                        let hint = status.error.as_deref().unwrap_or(
                            "Project module build state (diagnostics live in the Problems panel)",
                        );
                        ui.label(RichText::new(label).size(10.0).color(color))
                            .on_hover_text(hint);
                    }
                }
                let text = if self.playing {
                    if self.paused {
                        "PAUSED"
                    } else {
                        "PLAYING"
                    }
                } else {
                    "EDIT"
                };
                let color = if self.playing {
                    panels::play_mode_tint(self.paused)
                } else {
                    Color32::from_gray(150)
                };
                let mut label = RichText::new(text).size(10.0).color(color).strong();
                if self.playing {
                    label = label.background_color(color.gamma_multiply(0.22));
                }
                ui.label(label);
                let selected = self.workspace.selection.len();
                let entities = self.workspace.document().doc.entities.len();
                ui.label(
                    RichText::new(format!("{entities} entities · {selected} selected"))
                        .size(10.5)
                        .weak(),
                );
                if let Some(root) = &self.project_root {
                    ui.label(RichText::new(root.display().to_string()).size(10.0).weak());
                } else {
                    ui.label(RichText::new("no project (fixture only)").size(10.0).weak());
                }
            });
        });
    }

    pub(super) fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(confirm) = self.state.confirm.clone() else {
            return;
        };
        let mut open = true;
        let mut accepted = false;
        let mut cancelled = false;
        egui::Window::new(&confirm.title)
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(&confirm.message);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(confirm.ok.as_str()).clicked() {
                        accepted = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancelled = true;
                    }
                });
            });
        if accepted {
            self.accept_confirm();
        } else if cancelled || !open {
            self.state.confirm = None;
        }
    }

    pub(super) fn accept_confirm(&mut self) {
        let Some(confirm) = self.state.confirm.take() else {
            return;
        };
        match confirm.kind {
            panels::ConfirmKind::CleanPlayer => self.start_build(crate::build::BuildKind::Clean),
            panels::ConfirmKind::CleanEngine => {
                self.start_build(crate::build::BuildKind::CleanEngine)
            }
        }
    }

    pub(super) fn prompt_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut prompt) = self.state.prompt.clone() else {
            return;
        };
        let mut open = true;
        let mut commit = false;
        egui::Window::new(&prompt.title)
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(&prompt.hint);
                let response =
                    ui.add(egui::TextEdit::singleline(&mut prompt.value).desired_width(380.0));
                if !response.has_focus() && !response.lost_focus() {
                    response.request_focus();
                }
                commit = ui.button("OK").clicked()
                    || ui.input(|input| input.key_pressed(egui::Key::Enter));
            });

        if commit {
            let value = prompt.value.trim().to_string();
            let _ = self.commit_prompt(prompt.kind, value);
        } else if !open {
            self.state.prompt = None;
        } else {
            self.state.prompt = Some(prompt);
        }
    }

    pub(super) fn commit_prompt(
        &mut self,
        kind: panels::PromptKind,
        value: String,
    ) -> Result<(), String> {
        let result = match kind {
            panels::PromptKind::SaveAs => self
                .workspace
                .save_active_as(&value)
                .map(|path| format!("saved {path}")),
            panels::PromptKind::NewFolder => self
                .workspace
                .create_asset_folder(&value)
                .map(|path| format!("created {path}")),
            panels::PromptKind::SavePrefab => self
                .workspace
                .save_selection_as_prefab_at(Some("assets/prefabs"), &value)
                .map(|path| format!("saved prefab {path}")),
            panels::PromptKind::PrefabVariant => self.create_prefab_variant(&value),
            panels::PromptKind::LayoutName => {
                self.save_named_layout(&value);
                Ok(format!("layout saved: {value}"))
            }
            panels::PromptKind::LayoutFileSave => {
                let path = PathBuf::from(&value);
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                std::fs::write(
                    &path,
                    serde_json::to_string_pretty(&self.current_layout_json()).unwrap_or_default(),
                )
                .map(|()| format!("layout written: {}", path.display()))
            }
            panels::PromptKind::LayoutFileLoad => {
                let text = std::fs::read_to_string(&value);
                match text {
                    Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
                        Ok(json) => {
                            self.apply_layout_json(&json);
                            Ok(format!("layout loaded: {value}"))
                        }
                        Err(error) => Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("invalid layout json: {error}"),
                        )),
                    },
                    Err(error) => Err(error),
                }
            }
        };
        match result {
            Ok(message) => {
                self.state.status = message;
                self.synced_doc_state = None;
                self.state.prompt = None;
                Ok(())
            }
            Err(error) => {
                self.state.status = format!("{error}");
                Err(error.to_string())
            }
        }
    }

    pub(super) fn save_conflict_dialog(&mut self, ctx: &egui::Context) {
        let Some(index) = self.state.pending_save_conflict else {
            return;
        };
        let Some(document) = self.workspace.docs.get(index) else {
            self.state.pending_save_conflict = None;
            return;
        };
        let title = document.title();
        let path = document.path.clone().unwrap_or_default();

        let mut choice: Option<SaveConflictChoice> = None;
        egui::Window::new("File Changed on Disk")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!("{title} was modified outside the editor."));
                ui.label(egui::RichText::new(path).weak().small());
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Overwriting discards the other copy.").weak());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Overwrite").clicked() {
                        choice = Some(SaveConflictChoice::Overwrite);
                    }
                    if ui.button("Reload from Disk").clicked() {
                        choice = Some(SaveConflictChoice::Reload);
                    }
                    if ui.button("Cancel").clicked() {
                        choice = Some(SaveConflictChoice::Cancel);
                    }
                });
            });

        if let Some(choice) = choice {
            self.apply_save_conflict_choice(index, choice);
        }
    }

    pub(super) fn apply_save_conflict_choice(&mut self, index: usize, choice: SaveConflictChoice) {
        self.state.pending_save_conflict = None;
        match choice {
            SaveConflictChoice::Overwrite => match self.workspace.save_document_forced(index) {
                Ok(path) => self.state.status = format!("saved {path} (overwrote)"),
                Err(error) => self.state.status = format!("save failed: {error}"),
            },
            SaveConflictChoice::Reload => match self.workspace.reload_document_from_disk(index) {
                Ok(()) => self.state.status = "reloaded from disk".to_string(),
                Err(error) => self.state.status = format!("reload failed: {error}"),
            },
            SaveConflictChoice::Cancel => {}
        }
    }

    pub(super) fn close_confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(target) = self.state.pending_close else {
            return;
        };
        let (message, detail) = match target {
            panels::CloseTarget::Document(index) => {
                let title = self
                    .workspace
                    .docs
                    .get(index)
                    .map(|document| document.title())
                    .unwrap_or_else(|| "document".to_string());
                (
                    format!("{title} has unsaved changes."),
                    "Close it anyway?".to_string(),
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
                    format!("{count} document(s) have unsaved changes."),
                    "Quit anyway?".to_string(),
                )
            }
        };

        let mut open = true;
        let mut decision: Option<CloseDecision> = None;
        egui::Window::new("Unsaved Changes")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(message);
                ui.label(egui::RichText::new(detail).weak());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        decision = Some(CloseDecision::Save);
                    }
                    if ui.button("Don't Save").clicked() {
                        decision = Some(CloseDecision::Discard);
                    }
                    if ui.button("Cancel").clicked() {
                        decision = Some(CloseDecision::Cancel);
                    }
                });
            });

        match decision {
            Some(decision) => self.apply_close_decision(ctx, target, decision),
            None => {
                if !open {
                    self.state.pending_close = None;
                }
            }
        }
    }

    pub(super) fn recovery_dialog(&mut self, ctx: &egui::Context) {
        let count = self.state.pending_recovery;
        if count == 0 {
            return;
        }
        let mut recover = None;
        egui::Window::new("Unsaved Changes Recovered")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(format!(
                    "{count} document(s) from the previous session were not saved."
                ));
                ui.label(
                    egui::RichText::new(
                        "Recover them as unsaved documents, or discard the snapshot?",
                    )
                    .weak(),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Recover").clicked() {
                        recover = Some(true);
                    }
                    if ui.button("Discard").clicked() {
                        recover = Some(false);
                    }
                });
            });

        match recover {
            Some(true) => match self.workspace.recover_unsaved() {
                Ok(count) => {
                    self.state.pending_recovery = 0;
                    self.state.status = format!("recovered {count} unsaved document(s)");
                    // Reload engine from the recovered document on the next sync pass.
                    self.synced_doc_state = None;
                    self.mark_scene_dirty();
                }
                Err(error) => {
                    self.state.status = format!("recover failed: {error}");
                }
            },
            Some(false) => match self.workspace.discard_recovery() {
                Ok(()) => {
                    self.state.pending_recovery = 0;
                    self.state.status = "discarded the recovery snapshot".to_string();
                }
                Err(error) => {
                    self.state.status = format!("discard failed: {error}");
                }
            },
            None => {}
        }
    }

    pub(super) fn apply_close_decision(
        &mut self,
        ctx: &egui::Context,
        target: panels::CloseTarget,
        decision: CloseDecision,
    ) {
        match decision {
            CloseDecision::Save => match self.save_before_close(target) {
                Ok(()) => self.finish_close(ctx, target),
                Err(error) => self.state.status = format!("save failed: {error}"),
            },
            CloseDecision::Discard => self.finish_close(ctx, target),
            CloseDecision::Cancel => self.state.pending_close = None,
        }
    }

    pub(super) fn save_before_close(&mut self, target: panels::CloseTarget) -> Result<(), String> {
        match target {
            panels::CloseTarget::Document(index) => self
                .workspace
                .save_document(index)
                .map(|path| {
                    self.state.status = format!("saved {path}");
                })
                .map_err(|error| error.to_string()),
            panels::CloseTarget::App => {
                let mut failure: Option<String> = None;
                for index in 0..self.workspace.docs.len() {
                    if !self.workspace.docs[index].is_dirty() {
                        continue;
                    }
                    if let Err(error) = self.workspace.save_document(index) {
                        failure = Some(error.to_string());
                    }
                }
                failure.map_or(Ok(()), Err)
            }
        }
    }

    pub(super) fn finish_close(&mut self, ctx: &egui::Context, target: panels::CloseTarget) {
        self.state.pending_close = None;
        match target {
            panels::CloseTarget::Document(index) => self.workspace.close_document(index),
            panels::CloseTarget::App => {
                self.autosave_state();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
}
