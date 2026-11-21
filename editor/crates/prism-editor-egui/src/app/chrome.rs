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
    pub(super) fn t(&self, key: &str) -> String {
        self.workspace.t(key)
    }

    pub(super) fn tf(&self, key: &str, params: &[(&str, &str)]) -> String {
        self.workspace.i18n.t(key, params)
    }

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
            ui.menu_button(self.t("menu.file"), |ui| {
                self.menu_item(ui, &self.t("menu.newScene"), "Ctrl+N", Command::NewScene);
                let scene_templates: Vec<(String, String)> = self
                    .workspace
                    .templates
                    .iter()
                    .filter(|template| template.kind == prism_editor_core::DocKind::Scene)
                    .map(|template| (template.id.clone(), template.label.clone()))
                    .collect();
                if !scene_templates.is_empty() {
                    ui.menu_button(self.t("menu.newSceneFromTemplate"), |ui| {
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
                self.menu_item(ui, &self.t("menu.newPrefab"), "", Command::NewPrefab);
                ui.separator();
                self.menu_item(
                    ui,
                    &self.t("menu.openDocument"),
                    "Ctrl+O",
                    Command::OpenDocument,
                );
                let docs: Vec<(usize, String, bool)> = self
                    .workspace
                    .docs
                    .iter()
                    .enumerate()
                    .map(|(index, doc)| {
                        (index, doc.title(), index == self.workspace.active_index())
                    })
                    .collect();
                ui.menu_button(self.t("menu.recentScenes"), |ui| {
                    for (index, title, active) in &docs {
                        if ui.selectable_label(*active, title).clicked() {
                            self.workspace.set_active(*index);
                            self.synced_doc_state = None;
                            ui.close();
                        }
                    }
                });
                self.menu_item(ui, &self.t("menu.save"), "Ctrl+S", Command::Save);
                self.menu_item(ui, &self.t("menu.saveAs"), "Ctrl+Shift+S", Command::SaveAs);
                self.menu_do(ui, &self.t("menu.saveProject"), |app| {
                    let _ = app.save_all_documents();
                });
                self.menu_item(ui, &self.t("menu.reloadScene"), "", Command::ReloadDocument);
                ui.separator();
                self.menu_item(ui, &self.t("menu.buildSettings"), "", Command::ToggleBuild);
                if ui
                    .add_enabled(
                        !self.build_job_running(),
                        egui::Button::new(self.t("menu.buildScenePack")),
                    )
                    .clicked()
                {
                    self.start_build(crate::build::BuildKind::ScenePack);
                    ui.close();
                }
                if ui
                    .add_enabled(
                        !self.build_job_running(),
                        egui::Button::new(self.t("menu.cookActiveScene")),
                    )
                    .clicked()
                {
                    self.cook_active_scene();
                    ui.close();
                }
                if ui
                    .add_enabled(
                        !self.build_job_running(),
                        egui::Button::new(self.t("menu.buildPlayer")),
                    )
                    .clicked()
                {
                    self.start_build(crate::build::BuildKind::Player);
                    ui.close();
                }
                if ui
                    .add_enabled(
                        !self.build_job_running(),
                        egui::Button::new(format!("{}  (Ctrl+B)", self.t("menu.buildAndRun"))),
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
                        egui::Button::new(format!("{}  (Ctrl+R)", self.t("menu.compileModule"))),
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
                        egui::Checkbox::new(&mut auto_refresh, self.t("menu.autoRefreshModule")),
                    )
                    .changed()
                {
                    self.set_project_auto_refresh(auto_refresh);
                }
                if ui
                    .add_enabled(
                        !self.build_job_running(),
                        egui::Button::new(self.t("menu.cleanBuild")),
                    )
                    .clicked()
                {
                    self.state.confirm = Some(panels::Confirm::new(
                        panels::ConfirmKind::CleanPlayer,
                        &self.t("menu.cleanBuild"),
                        self.t("desktop.confirm.cleanBuildBody"),
                        &self.t("desktop.confirm.cleanPlayer"),
                    ));
                    ui.close();
                }
                if ui
                    .add_enabled(
                        !self.build_job_running(),
                        egui::Button::new(self.t("menu.cleanEngineCache")),
                    )
                    .clicked()
                {
                    self.state.confirm = Some(panels::Confirm::new(
                        panels::ConfirmKind::CleanEngine,
                        &self.t("menu.cleanEngineCache"),
                        self.t("desktop.confirm.cleanEngineBody"),
                        &self.t("desktop.confirm.cleanEngine"),
                    ));
                    ui.close();
                }
                if self.build_job_running() && ui.button(self.t("menu.cancelBuild")).clicked() {
                    self.cancel_build();
                    ui.close();
                }
                ui.separator();
                let quit_ctx = ui.ctx().clone();
                self.menu_do(ui, &self.t("menu.exit"), |app| app.request_quit(&quit_ctx));
            });

            // ---- Edit ----
            ui.menu_button(self.t("menu.edit"), |ui| {
                self.menu_do(ui, &format!("{} (Ctrl+K)", self.t("menu.search")), |app| {
                    app.palette.open(&app.workspace)
                });
                ui.separator();
                self.menu_item(ui, &self.t("menu.undo"), "Ctrl+Z", Command::Undo);
                self.menu_item(ui, &self.t("menu.redo"), "Ctrl+Y", Command::Redo);
                ui.separator();
                self.menu_item(ui, &self.t("hierarchy.cut"), "Ctrl+X", Command::CutEntity);
                self.menu_item(ui, &self.t("hierarchy.copy"), "Ctrl+C", Command::CopyEntity);
                self.menu_item(
                    ui,
                    &self.t("hierarchy.paste"),
                    "Ctrl+V",
                    Command::PasteEntity,
                );
                self.menu_item(
                    ui,
                    &self.t("common.duplicate"),
                    "Ctrl+D",
                    Command::DuplicateEntity,
                );
                self.menu_item(
                    ui,
                    &self.t("common.delete"),
                    "Del",
                    Command::DeleteSelection,
                );
                ui.separator();
                self.menu_item(ui, &self.t("common.all"), "Ctrl+A", Command::SelectAll);
                self.menu_do(ui, &self.t("menu.deselectAll"), |app| {
                    app.workspace.clear_selection()
                });
                self.menu_do(ui, &self.t("menu.invertSelection"), |app| {
                    app.invert_selection()
                });
                self.menu_do(ui, &self.t("menu.selectChildren"), |app| {
                    app.select_children_of_selection()
                });
                self.menu_do(ui, &self.t("menu.selectPrefabRoot"), |app| {
                    app.select_prefab_root()
                });
                ui.separator();
                self.menu_do(ui, &self.t("menu.selectionBack"), |app| {
                    app.navigate_selection_history(-1)
                });
                self.menu_do(ui, &self.t("menu.selectionForward"), |app| {
                    app.navigate_selection_history(1)
                });
                self.menu_do(ui, &self.t("menu.findInHierarchy"), |app| {
                    app.find_in_hierarchy()
                });
                ui.separator();
                ui.menu_button(self.t("menu.copyTransform"), |ui| {
                    for field in ["position", "rotation", "scale"] {
                        if ui.button(field).clicked() {
                            self.copy_transform(field);
                            ui.close();
                        }
                    }
                });
                ui.menu_button(self.t("menu.pasteTransform"), |ui| {
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
                    &self.t("menu.pasteComponent"),
                    "Ctrl+Shift+V",
                    Command::PasteComponent,
                );
                self.menu_item(
                    ui,
                    &self.t("menu.clearComponentClipboard"),
                    "",
                    Command::ClearComponentClipboard,
                );
                ui.separator();
                self.menu_item(ui, &self.t("common.run"), "Ctrl+P", Command::Play);
                self.menu_item(ui, &self.t("common.pause"), "Ctrl+Shift+P", Command::Pause);
                self.menu_item(ui, &self.t("common.step"), "Ctrl+Alt+P", Command::Step);
                ui.separator();
                self.menu_item(
                    ui,
                    &self.t("menu.projectSettings"),
                    "",
                    Command::ProjectSettings,
                );
                self.menu_item(ui, &self.t("menu.preferences"), "", Command::Preferences);
                self.menu_item(ui, &self.t("menu.theme"), "", Command::ThemePanel);
            });

            // ---- Assets ----
            ui.menu_button(self.t("menu.assets"), |ui| {
                ui.menu_button(self.t("menu.create"), |ui| {
                    self.menu_item(ui, &self.t("menu.scene"), "", Command::NewScene);
                    self.menu_item(ui, &self.t("menu.prefab"), "", Command::NewPrefab);
                    let can_variant = self.has_prefab_instance();
                    if ui
                        .add_enabled(can_variant, egui::Button::new(self.t("menu.prefabVariant")))
                        .clicked()
                    {
                        let title = self.t("desktop.prompt.prefabVariant");
                        let hint = self.t("desktop.prompt.prefabVariantBody");
                        let value = self.t("desktop.prompt.variantDefault");
                        self.open_prompt(panels::PromptKind::PrefabVariant, &title, &hint, value);
                        ui.close();
                    }
                    let folder_title = self.t("desktop.prompt.newFolder");
                    let folder_body = self.t("desktop.prompt.folderBody");
                    self.menu_do(ui, &self.t("menu.folder"), move |app| {
                        app.open_prompt(
                            panels::PromptKind::NewFolder,
                            &folder_title,
                            &folder_body,
                            "assets/",
                        )
                    });
                });
                let selected = self.state.asset_primary().cloned();
                if let Some(path) = selected.as_deref() {
                    self.menu_do(ui, &self.t("menu.open"), |app| {
                        if let Err(error) = app.workspace.open_project_file(path) {
                            app.state.status = app.tf(
                                "desktop.status.openFailed",
                                &[("path", &path), ("error", &error.to_string())],
                            );
                        }
                    });
                    let selected_path = path.to_string();
                    self.menu_do(ui, &self.t("menu.showInExplorer"), |app| {
                        app.state.status =
                            match panels::reveal_asset(&app.workspace, &selected_path) {
                                Ok(()) => {
                                    app.tf("desktop.status.revealed", &[("path", &selected_path)])
                                }
                                Err(error) => error,
                            };
                    });
                    let ctx_copy = ui.ctx().clone();
                    let copy_path = selected_path.clone();
                    self.menu_do(ui, &self.t("menu.copyPath"), move |app| {
                        ctx_copy.copy_text(copy_path.clone());
                        app.state.status = app.tf("desktop.status.copied", &[("path", &copy_path)]);
                    });
                    self.menu_do(ui, &self.t("menu.duplicate"), |app| {
                        match app.workspace.duplicate_asset(path) {
                            Ok(copy) => {
                                app.state.status =
                                    app.tf("desktop.status.duplicated", &[("path", &copy)])
                            }
                            Err(error) => {
                                app.state.status = app.tf(
                                    "desktop.status.duplicateFailed",
                                    &[("error", &error.to_string())],
                                )
                            }
                        }
                    });
                    self.menu_do(ui, &self.t("common.delete"), |app| {
                        match app.workspace.delete_asset(path) {
                            Ok(()) => {
                                app.state.forget_asset(path);
                                app.state.status =
                                    app.tf("desktop.status.deleted", &[("path", &path)]);
                            }
                            Err(error) => {
                                app.state.status = app.tf(
                                    "desktop.status.deleteFailed",
                                    &[("error", &error.to_string())],
                                )
                            }
                        }
                    });
                    if path.ends_with(".prefab.json") {
                        self.menu_do(ui, &self.t("menu.instantiate"), |app| {
                            match app.workspace.instantiate_prefab_asset(path, None) {
                                Ok(count) => {
                                    app.state.status = app.tf(
                                        "desktop.status.instantiated",
                                        &[("count", &count.to_string())],
                                    );
                                    app.synced_doc_state = None;
                                }
                                Err(error) => {
                                    app.state.status = app.tf(
                                        "desktop.status.instantiateFailed",
                                        &[("error", &error.to_string())],
                                    )
                                }
                            }
                        });
                    }
                } else {
                    ui.add_enabled(false, egui::Button::new(self.t("menu.open")));
                    ui.add_enabled(false, egui::Button::new(self.t("menu.showInExplorer")));
                    ui.add_enabled(false, egui::Button::new(self.t("menu.duplicate")));
                    ui.add_enabled(false, egui::Button::new(self.t("common.delete")));
                }
                ui.separator();
                self.menu_do(ui, &self.t("menu.findRefsScene"), |app| {
                    if let Some(path) = app.state.asset_primary().cloned() {
                        app.find_references(&path, false);
                    }
                });
                self.menu_do(ui, &self.t("menu.findRefsProject"), |app| {
                    if let Some(path) = app.state.asset_primary().cloned() {
                        app.find_references(&path, true);
                    }
                });
                ui.separator();
                self.menu_do(ui, &self.t("menu.refresh"), |app| {
                    app.workspace.refresh_project_files();
                    app.state.status = app.t("desktop.status.assetsRefreshed");
                });
            });

            // ---- GameObject ----
            ui.menu_button(self.t("menu.gameObject"), |ui| {
                self.menu_do(ui, &self.t("hierarchy.createEmpty"), |app| {
                    app.workspace.add_entity("")
                });
                self.menu_do(ui, &self.t("hierarchy.createEmptyChild"), |app| {
                    app.create_empty_child()
                });
                let empty_parent = self.t("hierarchy.createEmptyParent");
                self.menu_do(ui, &empty_parent, |app| app.create_empty_parent());
                ui.separator();
                ui.menu_button(self.t("menu.createOther"), |ui| {
                    ui.menu_button(self.t("hierarchy.object3D"), |ui| {
                        for (id, key) in [
                            ("cube", "desktop.preset.cube"),
                            ("sphere", "desktop.preset.sphere"),
                            ("cylinder", "desktop.preset.cylinder"),
                            ("capsule", "desktop.preset.capsule"),
                            ("quad", "desktop.preset.quad"),
                            ("plane", "desktop.preset.plane"),
                        ] {
                            let known = self.workspace.presets.iter().any(|p| p.id == id);
                            if ui
                                .add_enabled(known, egui::Button::new(self.t(key)))
                                .clicked()
                            {
                                self.workspace.add_entity(id);
                                ui.close();
                            }
                        }
                    });
                    ui.menu_button(self.t("menu.rendering"), |ui| {
                        for (id, key) in [
                            ("mesh", "desktop.preset.mesh"),
                            ("skybox", "desktop.preset.skybox"),
                        ] {
                            let known = self.workspace.presets.iter().any(|p| p.id == id);
                            if ui
                                .add_enabled(known, egui::Button::new(self.t(key)))
                                .clicked()
                            {
                                self.workspace.add_entity(id);
                                ui.close();
                            }
                        }
                    });
                    ui.menu_button(self.t("hierarchy.light"), |ui| {
                        for (id, key) in [
                            ("sun", "desktop.preset.sun"),
                            ("point_light", "desktop.preset.pointLight"),
                            ("spot_light", "desktop.preset.spotLight"),
                        ] {
                            let known = self.workspace.presets.iter().any(|p| p.id == id);
                            if ui
                                .add_enabled(known, egui::Button::new(self.t(key)))
                                .clicked()
                            {
                                self.workspace.add_entity(id);
                                ui.close();
                            }
                        }
                    });
                    ui.menu_button(self.t("hierarchy.camera"), |ui| {
                        let known = self.workspace.presets.iter().any(|p| p.id == "camera");
                        if ui
                            .add_enabled(known, egui::Button::new(self.t("hierarchy.camera")))
                            .clicked()
                        {
                            self.workspace.add_entity("camera");
                            ui.close();
                        }
                    });
                    ui.menu_button(self.t("hierarchy.ui"), |ui| {
                        let known = self.workspace.presets.iter().any(|p| p.id == "ui");
                        if ui
                            .add_enabled(known, egui::Button::new(self.t("hierarchy.uiElement")))
                            .clicked()
                        {
                            self.workspace.add_entity("ui");
                            ui.close();
                        }
                    });
                });
                ui.separator();
                self.menu_do(ui, &self.t("menu.moveToView"), |app| {
                    if app.workspace.align_selection_to_view() {
                        app.state.status = app.t("desktop.status.movedToView");
                        app.mark_scene_dirty();
                    } else {
                        app.state.status = app.t("desktop.status.moveToViewNeedsSelection");
                    }
                });
                self.menu_do(ui, &self.t("menu.alignViewToSelected"), |app| {
                    if app.workspace.align_view_to_selection() {
                        app.mark_scene_dirty();
                    }
                });
                self.menu_do(ui, &self.t("menu.snapToGround"), |app| {
                    let count = app.workspace.snap_selection_to_ground();
                    app.state.status =
                        app.tf("desktop.status.snapped", &[("count", &count.to_string())]);
                    app.mark_scene_dirty();
                });
            });

            // ---- Component ----
            ui.menu_button(self.t("inspector.components"), |ui| {
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
                    ui.label(self.t("menu.noComponents"));
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
            ui.menu_button(self.t("menu.window"), |ui| {
                ui.menu_button(self.t("menu.general"), |ui| {
                    for (label, command) in [
                        (self.t("menu.sceneView"), Command::ToggleSceneView),
                        (self.t("menu.gameView"), Command::ToggleGameView),
                        (self.t("menu.hierarchy"), Command::ToggleHierarchy),
                        (self.t("menu.assets"), Command::ToggleAssets),
                        (self.t("menu.inspector"), Command::ToggleInspector),
                        (self.t("menu.log"), Command::ToggleLog),
                        (self.t("menu.docs"), Command::ToggleDocs),
                        (self.t("menu.history"), Command::ToggleHistory),
                        (self.t("menu.renderGraph"), Command::ToggleRenderGraph),
                        (self.t("menu.buildPanel"), Command::ToggleBuild),
                        (self.t("menu.tools"), Command::ToggleTools),
                        (self.t("menu.projectSettings"), Command::ProjectSettings),
                        (self.t("menu.preferences"), Command::Preferences),
                        (self.t("menu.theme"), Command::ThemePanel),
                    ] {
                        self.menu_item(ui, &label, "", command);
                    }
                    if ui.button(self.t("menu.problems")).clicked() {
                        self.workspace.show_panel("problems");
                        ui.close();
                    }
                    if ui.button(self.t("menu.prefabOverrides")).clicked() {
                        self.workspace.show_panel("prefabOverrides");
                        ui.close();
                    }
                    let mut maximize_on_play = self.workspace.maximize_on_play;
                    if ui
                        .checkbox(&mut maximize_on_play, self.t("menu.maximizeOnPlay"))
                        .changed()
                    {
                        self.workspace.maximize_on_play = maximize_on_play;
                    }
                    if ui.button(self.t("menu.material")).clicked() {
                        self.workspace.show_panel("material");
                        ui.close();
                    }
                    if ui.button(self.t("menu.animationGraph")).clicked() {
                        self.workspace.show_panel("animGraph");
                        ui.close();
                    }
                    if ui.button(self.t("menu.gaspDebug")).clicked() {
                        self.workspace.show_panel("gaspDebug");
                        ui.close();
                    }
                });
                ui.separator();
                self.menu_do(ui, &self.t("menu.captureWindow"), |app| {
                    app.capture_window_now()
                });
                ui.menu_button(self.t("menu.layouts"), |ui| {
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
                            if ui.button(self.t("menu.load")).clicked() {
                                let value = self.named_layouts.get(&name).cloned();
                                if let Some(value) = value {
                                    self.apply_layout_json(&value);
                                    self.state.status =
                                        self.tf("desktop.status.layoutLoaded", &[("name", &name)]);
                                }
                                ui.close();
                            }
                            if ui.button(self.t("common.delete")).clicked() {
                                self.named_layouts.remove(&name);
                                self.write_named_layouts();
                                ui.close();
                            }
                        });
                    }
                    if !self.named_layouts.is_empty() {
                        ui.separator();
                    }
                    let save_title = self.t("menu.saveLayout");
                    let save_hint = self.t("desktop.prompt.layoutName");
                    let save_default = self.t("desktop.prompt.layoutDefault");
                    self.menu_do(ui, &self.t("menu.saveLayout"), move |app| {
                        app.open_prompt(
                            panels::PromptKind::LayoutName,
                            &save_title,
                            &save_hint,
                            save_default,
                        )
                    });
                    let save_file_title = self.t("menu.saveLayoutFile");
                    let save_file_hint = self.t("desktop.prompt.outputJson");
                    self.menu_do(ui, &self.t("menu.saveLayoutFile"), move |app| {
                        app.open_prompt(
                            panels::PromptKind::LayoutFileSave,
                            &save_file_title,
                            &save_file_hint,
                            "target/editor-egui/layout.json",
                        )
                    });
                    let load_title = self.t("desktop.prompt.loadLayoutFile");
                    let load_body = self.t("desktop.prompt.loadLayoutBody");
                    self.menu_do(ui, &self.t("menu.loadLayoutFile"), move |app| {
                        app.open_prompt(
                            panels::PromptKind::LayoutFileLoad,
                            &load_title,
                            &load_body,
                            "target/editor-egui/layout.json",
                        )
                    });
                    self.menu_do(ui, &self.t("menu.resetAllLayouts"), |app| {
                        app.named_layouts.clear();
                        app.write_named_layouts();
                        app.workspace.set_layout_preset("default");
                        app.state.status = app.t("desktop.status.layoutsReset");
                    });
                });
                ui.separator();
                self.menu_item(ui, &self.t("menu.resetLayout"), "", Command::ResetLayout);
            });

            // ---- Help ----
            ui.menu_button(self.t("menu.help"), |ui| {
                self.menu_item(ui, &self.t("menu.shortcuts"), "", Command::Shortcuts);
                self.menu_item(ui, &self.t("menu.about"), "", Command::About);
            });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
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
                        .small_button(RichText::new(self.t("common.cancel")).size(10.0))
                        .clicked()
                    {
                        self.cancel_build();
                    }
                    ui.label(
                        RichText::new(self.tf("desktop.status.building", &[("label", &label)]))
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
                        |ui: &mut egui::Ui, glyph: &str, enabled: bool, tint: Option<Color32>| {
                            let mut text = RichText::new(glyph).size(13.0);
                            if enabled {
                                if let Some(c) = tint {
                                    text = text.color(c);
                                }
                            }
                            ui.add_enabled(enabled, egui::Button::new(text).frame(true))
                                .clicked()
                        };
                    let play_tint = if playing && !paused {
                        Some(theme::color32(&colors, ThemeToken::Success))
                    } else {
                        None
                    };
                    if transport_btn(ui, icons::PLAY, !playing, play_tint) {
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
                        playing,
                        pause_tint,
                    ) {
                        app.apply_command(Command::Pause);
                    }
                    if transport_btn(ui, icons::SKIP_FORWARD, playing, None) {
                        app.apply_command(Command::Step);
                    }
                    let stop_tint = if playing {
                        Some(theme::color32(&colors, ThemeToken::DangerSoft))
                    } else {
                        None
                    };
                    if transport_btn(ui, icons::STOP, playing, stop_tint) {
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
                            .clicked()
                        {
                            let prefab_index = self.workspace.active_index();
                            if self.workspace.return_from_prefab(prefab_index) {
                                self.state.status = self.tf(
                                    "desktop.status.returnedTo",
                                    &[("scene", &origin.scene_title), ("entity", &prefab_title)],
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
                    if ui.small_button(save_icon).clicked() {
                        self.apply_command(Command::Save);
                    }
                    if ui
                        .add_enabled(
                            self.workspace.can_undo(),
                            egui::Button::new(icons::ARROW_COUNTER_CLOCKWISE).small(),
                        )
                        .clicked()
                    {
                        self.apply_command(Command::Undo);
                    }
                    if ui
                        .add_enabled(
                            self.workspace.can_redo(),
                            egui::Button::new(icons::ARROW_CLOCKWISE).small(),
                        )
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
                            for (tool, icon, key) in [
                                (
                                    crate::viewport::GizmoTool::Translate,
                                    icons::ARROWS_OUT_CARDINAL,
                                    "W",
                                ),
                                (
                                    crate::viewport::GizmoTool::Rotate,
                                    icons::ARROWS_CLOCKWISE,
                                    "E",
                                ),
                                (crate::viewport::GizmoTool::Scale, icons::ARROWS_OUT, "R"),
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
                                        RichText::new(format!(
                                            "{} {}",
                                            icons::SPARKLE,
                                            self.t("toolbar.sculpt")
                                        ))
                                        .size(11.0)
                                        .color(sculpt_fg),
                                    )
                                    .fill(sculpt_fill),
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
                        .small_button(format!(
                            "{space_icon} {}",
                            match space {
                                prism_editor_core::viewport_math::GizmoSpace::World =>
                                    self.t("toolbar.global"),
                                prism_editor_core::viewport_math::GizmoSpace::Local =>
                                    self.t("toolbar.local"),
                            }
                        ))
                        .clicked()
                    {
                        self.workspace.toggle_gizmo_space();
                        self.mark_scene_dirty();
                    }

                    let pivot =
                        self.workspace.pivot_mode == prism_editor_core::workspace::PivotMode::Pivot;
                    let pivot_label = if pivot {
                        format!("{} {}", icons::CROSSHAIR, self.t("toolbar.pivot"))
                    } else {
                        format!("{} {}", icons::TARGET, self.t("toolbar.center"))
                    };
                    if ui.selectable_label(pivot, pivot_label).clicked() {
                        self.workspace.pivot_mode = if pivot {
                            prism_editor_core::workspace::PivotMode::Center
                        } else {
                            prism_editor_core::workspace::PivotMode::Pivot
                        };
                        self.mark_scene_dirty();
                    }

                    let snap = self.workspace.snap;
                    if ui
                        .selectable_label(
                            snap.enabled,
                            format!("{} {}", icons::MAGNET, self.t("toolbar.snap")),
                        )
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
                            .small_button(format!(
                                "{} {}",
                                icons::MAGNIFYING_GLASS,
                                self.t("desktop.search")
                            ))
                            .clicked()
                        {
                            self.palette.open(&self.workspace);
                        }

                        let active_preset = self.workspace.layout_preset.clone();
                        egui::ComboBox::from_id_salt("toolbar-layout")
                            .width(86.0)
                            .selected_text(format!(
                                "{} {}",
                                icons::LAYOUT,
                                self.t(&format!("desktop.layout.{active_preset}"))
                            ))
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
                                egui::Button::new(format!(
                                    "{} {}",
                                    icons::HAMMER,
                                    self.t("desktop.build")
                                ))
                                .small(),
                            )
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
                                            RichText::new(format!(
                                                "{} {}",
                                                icons::CIRCLE,
                                                self.t("desktop.compile")
                                            ))
                                            .small()
                                            .color(dot_color),
                                        ),
                                    )
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
                let label = self.tf(
                    "desktop.status.documentsFailed",
                    &[("count", &failures.len().to_string())],
                );
                ui.label(
                    RichText::new(label)
                        .size(11.0)
                        .strong()
                        .color(Color32::from_rgb(232, 96, 96)),
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let text = if self.playing {
                    if self.paused {
                        self.t("desktop.status.paused")
                    } else {
                        self.t("desktop.status.playing")
                    }
                } else {
                    self.t("desktop.status.edit")
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
                    RichText::new(self.tf(
                        "desktop.status.counts",
                        &[
                            ("entities", &entities.to_string()),
                            ("selected", &selected.to_string()),
                        ],
                    ))
                    .size(10.5)
                    .weak(),
                );
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
                    if ui.button(self.t("common.cancel")).clicked() {
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
                commit = ui.button(self.t("common.ok")).clicked()
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
                .map(|()| {
                    self.tf(
                        "desktop.status.layoutWritten",
                        &[("path", &path.display().to_string())],
                    )
                })
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
        egui::Window::new(self.t("desktop.dialog.fileChanged"))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(self.tf("desktop.dialog.fileChangedMsg", &[("title", &title)]));
                ui.label(egui::RichText::new(path).weak().small());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button(self.t("common.overwrite")).clicked() {
                        choice = Some(SaveConflictChoice::Overwrite);
                    }
                    if ui.button(self.t("desktop.dialog.reloadFromDisk")).clicked() {
                        choice = Some(SaveConflictChoice::Reload);
                    }
                    if ui.button(self.t("common.cancel")).clicked() {
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
                Ok(path) => {
                    self.state.status = self.tf("desktop.status.savedOverwrote", &[("path", &path)])
                }
                Err(error) => {
                    self.state.status = self.tf(
                        "desktop.status.saveFailed",
                        &[("error", &error.to_string())],
                    )
                }
            },
            SaveConflictChoice::Reload => match self.workspace.reload_document_from_disk(index) {
                Ok(()) => self.state.status = self.t("desktop.status.reloadedFromDisk"),
                Err(error) => {
                    self.state.status = self.tf(
                        "desktop.status.reloadFailed",
                        &[("error", &error.to_string())],
                    )
                }
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
                    self.tf("desktop.dialog.unsavedDoc", &[("title", &title)]),
                    String::new(),
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
                        "desktop.dialog.unsavedApp",
                        &[("count", &count.to_string())],
                    ),
                    String::new(),
                )
            }
        };

        let mut open = true;
        let mut decision: Option<CloseDecision> = None;
        egui::Window::new(self.t("desktop.dialog.unsavedTitle"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(message);
                if !detail.is_empty() {
                    ui.label(egui::RichText::new(detail).weak());
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button(self.t("common.save")).clicked() {
                        decision = Some(CloseDecision::Save);
                    }
                    if ui.button(self.t("common.dontSave")).clicked() {
                        decision = Some(CloseDecision::Discard);
                    }
                    if ui.button(self.t("common.cancel")).clicked() {
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
        egui::Window::new(self.t("desktop.dialog.recoveredTitle"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(self.tf(
                    "desktop.dialog.recoveredMsg",
                    &[("count", &count.to_string())],
                ));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button(self.t("desktop.dialog.recover")).clicked() {
                        recover = Some(true);
                    }
                    if ui.button(self.t("common.discard")).clicked() {
                        recover = Some(false);
                    }
                });
            });

        match recover {
            Some(true) => match self.workspace.recover_unsaved() {
                Ok(count) => {
                    self.state.pending_recovery = 0;
                    self.state.status =
                        self.tf("desktop.status.recovered", &[("count", &count.to_string())]);
                    // Reload engine from the recovered document on the next sync pass.
                    self.synced_doc_state = None;
                    self.mark_scene_dirty();
                }
                Err(error) => {
                    self.state.status = self.tf(
                        "desktop.status.recoverFailed",
                        &[("error", &error.to_string())],
                    );
                }
            },
            Some(false) => match self.workspace.discard_recovery() {
                Ok(()) => {
                    self.state.pending_recovery = 0;
                    self.state.status = self.t("desktop.status.discardedRecovery");
                }
                Err(error) => {
                    self.state.status = self.tf(
                        "desktop.status.discardFailed",
                        &[("error", &error.to_string())],
                    );
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
                Err(error) => {
                    self.state.status = self.tf(
                        "desktop.status.saveFailed",
                        &[("error", &error.to_string())],
                    )
                }
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
                    self.state.status = self.tf("desktop.status.saved", &[("path", &path)]);
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
