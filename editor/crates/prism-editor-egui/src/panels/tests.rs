//! Panel UI smoke and interaction tests.

use super::anim_graph::*;
use super::assets::*;
use super::console::*;
use super::hierarchy::*;
use super::inspector::*;
use super::render_graph::*;
use super::*;

#[cfg(test)]
mod interaction_tests {
    use super::*;
    use crate::test_harness::EguiHarness;
    use std::cell::RefCell;

    struct ToolbarScene {
        workspace: Workspace,
        state: UiState,
        toolbar: Option<HierarchyToolbar>,
    }

    impl ToolbarScene {
        fn new() -> Self {
            let mut workspace = Workspace::sample();
            workspace.select(1, false);
            Self {
                workspace,
                state: UiState::new(),
                toolbar: None,
            }
        }

        fn draw(&mut self, ctx: &egui::Context) {
            let mut toolbar = None;
            egui::CentralPanel::default().show(ctx, |ui| {
                toolbar = Some(hierarchy_toolbar(ui, &mut self.workspace, &mut self.state));
            });
            self.toolbar = toolbar;
        }

        fn toolbar(&self) -> &HierarchyToolbar {
            self.toolbar.as_ref().expect("draw a frame first")
        }
    }

    #[test]
    fn hierarchy_toolbar_search_isolate_lock_and_find_are_interactive() {
        let mut harness = EguiHarness::with_size(1000.0, 220.0);
        let scene = RefCell::new(ToolbarScene::new());
        let mut draw = |ctx: &egui::Context| scene.borrow_mut().draw(ctx);

        harness.frame(&mut draw);
        let search = scene.borrow().toolbar().search.rect;
        harness.click(search.center(), &mut draw);
        harness.text("cube", &mut draw);
        assert_eq!(scene.borrow().state.hierarchy_search, "cube");
        assert!(search_matches(
            &scene.borrow().workspace.document().doc,
            "cube",
            scene.borrow().state.hierarchy_search_mode
        )
        .contains(&1));

        // Isolate appears before the lock/find buttons, so re-read their rects
        // after every state-changing click.
        let isolate = scene.borrow().toolbar().isolate.rect;
        harness.click(isolate.center(), &mut draw);
        assert!(
            scene.borrow().workspace.isolate,
            "isolate button toggles the workspace"
        );
        // The new Show All button shifts the following controls; lay out one
        // more frame before reading their rects.
        harness.frame(&mut draw);

        let lock = scene.borrow().toolbar().lock.rect;
        harness.click(lock.center(), &mut draw);
        let cube_id = scene
            .borrow()
            .workspace
            .document()
            .doc
            .id_of_index(1)
            .expect("cube id");
        assert_eq!(scene.borrow().state.hierarchy_lock, Some(vec![cube_id]));

        let find = scene.borrow().toolbar().find.rect;
        harness.click(find.center(), &mut draw);
        assert!(
            scene.borrow().state.hierarchy_search.is_empty(),
            "find must clear the search that filters the target out"
        );
        assert!(
            scene.borrow().state.hierarchy_lock.is_none(),
            "find must release the old locked selection"
        );
        assert_eq!(scene.borrow().state.hierarchy_reveal, Some(cube_id));
    }

    #[test]
    fn hierarchy_search_mode_combo_switches_to_type() {
        let mut harness = EguiHarness::with_size(1000.0, 220.0);
        let scene = RefCell::new(ToolbarScene::new());
        let mut draw = |ctx: &egui::Context| scene.borrow_mut().draw(ctx);

        harness.frame(&mut draw);
        let mode = scene.borrow().toolbar().mode.rect;
        harness.click(mode.center(), &mut draw);
        // The popup opens below the combo: Name first, Type second.
        harness.click(egui::pos2(mode.center().x, mode.bottom() + 33.0), &mut draw);
        assert_eq!(
            scene.borrow().state.hierarchy_search_mode,
            HierarchySearchMode::Component
        );
    }

    #[test]
    fn shift_click_uses_visible_rows_and_lock_keeps_the_snapshot() {
        let mut workspace = Workspace::sample();
        let mut state = UiState::new();
        // Tree order is camera, cube, cube/Detail, light.
        let visible = vec![0, 1, 3, 2];

        apply_hierarchy_select(&mut workspace, &mut state, 1, false, &visible);
        assert_eq!(workspace.selection, vec![1]);
        apply_hierarchy_select(&mut workspace, &mut state, 2, true, &visible);
        assert_eq!(
            workspace.selection,
            vec![1, 2, 3],
            "Shift range walks visible rows and includes the nested Detail"
        );

        let locked_ids: Vec<u32> = workspace
            .selection
            .iter()
            .filter_map(|index| workspace.document().doc.id_of_index(*index))
            .collect();
        toggle_hierarchy_lock(&workspace, &mut state);
        assert_eq!(state.hierarchy_lock, Some(locked_ids));

        // A Scene/Project selection change must not move the locked highlight.
        workspace.select(0, false);
        assert_eq!(hierarchy_indices(&workspace, &mut state), vec![1, 2, 3]);

        // Clicking inside the hierarchy re-syncs the lock to the new selection.
        apply_hierarchy_select(&mut workspace, &mut state, 0, false, &visible);
        sync_hierarchy_lock(&workspace, &mut state);
        let camera_id = workspace.document().doc.id_of_index(0).expect("camera id");
        assert_eq!(state.hierarchy_lock, Some(vec![camera_id]));
    }
}

#[cfg(test)]
mod window_smoke_tests {
    use super::*;

    fn run_frame(mut draw: impl FnMut(&mut egui::Ui, &mut Workspace, &mut UiState)) {
        run_frame_with(
            Workspace::sample(),
            UiState::new(),
            |ui, workspace, state| draw(ui, workspace, state),
        );
    }

    fn run_frame_with(
        mut workspace: Workspace,
        mut state: UiState,
        mut draw: impl FnMut(&mut egui::Ui, &mut Workspace, &mut UiState),
    ) {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                draw(ui, &mut workspace, &mut state);
            });
        });
    }

    #[test]
    fn collapse_merges_identical_log_lines_and_keeps_time_order() {
        let line = |seq: u64, level: LogLevel, text: &str| prism_editor_core::LogLine {
            source: LogSource::Engine,
            level,
            text: text.to_string(),
            seq,
        };
        let lines = vec![
            line(0, LogLevel::Info, "a"),
            line(1, LogLevel::Warn, "b"),
            line(2, LogLevel::Info, "a"),
        ];
        assert_eq!(log_rows(&lines, false), vec![(0, 1), (1, 1), (2, 1)]);
        assert_eq!(log_rows(&lines, true), vec![(0, 2), (1, 1)]);

        let mut other = line(3, LogLevel::Info, "a");
        other.source = LogSource::Game;
        let mut lines = lines;
        lines.push(other);
        assert_eq!(log_rows(&lines, true), vec![(0, 2), (1, 1), (3, 1)]);
    }

    #[test]
    fn log_location_finds_an_existing_file_with_line_and_column() {
        let temp = std::env::temp_dir().join("prism-log-location-test.rs");
        std::fs::write(&temp, "// x").unwrap();
        let text = format!("thread panicked at {}:12:3", temp.display());
        let found = log_location(&text).expect("location");
        assert_eq!(found.0, temp.display().to_string());
        assert_eq!(found.1, Some(12));
        assert_eq!(found.2, Some(3));

        let only_file = format!("see {}", temp.display());
        assert_eq!(log_location(&only_file).map(|found| found.1), Some(None));
        assert!(log_location("no path here").is_none());
        let _ = std::fs::remove_file(&temp);
    }

    #[test]
    fn tilemap_cells_cycle_through_the_palette() {
        assert_eq!(next_tile(0, 4), 1);
        assert_eq!(next_tile(3, 4), 4);
        assert_eq!(next_tile(4, 4), 0);
        assert_eq!(
            next_tile(9, 4),
            0,
            "Out-of-range old value also returns empty"
        );
        assert_eq!(next_tile(0, 0), 1, "Palette at least 1");

        let value = serde_json::json!({ "columns": 3, "tiles": [0, 2] });
        assert_eq!(tilemap_columns(&value), 3);
        assert_eq!(tilemap_tiles(&value), vec![0, 2]);
        assert!(tilemap_tiles(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn a_tilemap_entity_renders_the_grid_editor() {
        let mut workspace = Workspace::sample();
        let index = workspace.document_mut().doc.add_named_child(None, "Ground");
        let id = workspace.document().doc.id_of_index(index).unwrap();
        workspace.document_mut().doc.entities[index]
            .components
            .insert(
                TILEMAP_COMPONENT.to_string(),
                serde_json::json!({ "columns": 2, "tiles": [1, 0, 2, 0] }),
            );
        workspace.selection = vec![index];
        assert_eq!(workspace.document().doc.id_of_index(index), Some(id));

        run_frame_with(workspace, UiState::new(), |ui, workspace, state| {
            inspector(ui, workspace, state)
        });
    }

    #[test]
    fn asset_search_supports_type_and_path_filters() {
        let query = AssetSearch::parse("t:texture sprites");
        assert_eq!(query.kinds, vec![AssetKind::Texture]);
        assert_eq!(query.needles, vec!["sprites".to_string()]);
        assert!(query.matches("assets/sprites/boat.png", AssetKind::Texture));
        assert!(!query.matches("assets/sprites/boat.png", AssetKind::Scene));
        assert!(!query.matches("assets/models/boat.gltf", AssetKind::Texture));

        assert_eq!(
            AssetSearch::parse("t:材质").kinds,
            vec![AssetKind::Material],
            "Chinese tags also recognized"
        );
        assert_eq!(
            AssetSearch::parse("t:Material").kinds,
            vec![AssetKind::Material]
        );

        let both = AssetSearch::parse("sprites boat");
        assert!(both.matches("assets/sprites/boat.png", AssetKind::Texture));
        assert!(!both.matches("assets/sprites/water.png", AssetKind::Texture));
        assert!(AssetSearch::parse("zz:foo").matches("assets/zz:foo/x.png", AssetKind::Other));

        assert!(!AssetSearch::parse("   ").is_active());
    }

    #[test]
    fn the_grid_wraps_cells_by_the_available_width() {
        assert_eq!(grid_columns(400.0, 92.0), 4);
        assert_eq!(
            grid_columns(90.0, 92.0),
            1,
            "Narrow panel at least one column"
        );
        assert_eq!(grid_columns(0.0, 92.0), 1);
        assert_eq!(grid_columns(f32::NAN, 92.0), 1);
        assert_eq!(truncate_label("short.png", 14), "short.png");
        assert_eq!(truncate_label("a-very-long-asset-name.png", 6), "a-ver…");
    }

    #[test]
    fn grid_and_two_column_modes_render() {
        for (view, columns) in [
            (AssetView::Grid, AssetColumns::One),
            (AssetView::Grid, AssetColumns::Two),
            (AssetView::List, AssetColumns::Two),
        ] {
            let workspace = Workspace::sample();
            let mut state = UiState::new();
            state.asset_view = view;
            state.asset_columns = columns;
            run_frame_with(workspace, state, |ui, workspace, state| {
                assets(ui, workspace, state);
            });
        }
    }

    #[test]
    fn ping_selects_the_object_that_references_the_asset() {
        let mut workspace = Workspace::sample();
        let mut state = UiState::new();
        workspace.document_mut().doc.entities[0].components.insert(
            "prism_engine::scene::MeshRenderer".to_string(),
            serde_json::json!({ "mesh_path": "assets/models/probe.gltf" }),
        );
        state.asset_selection = vec!["assets/models/probe.gltf".to_string()];

        ping_asset(&mut workspace, &mut state, "assets/models/probe.gltf");

        assert_eq!(
            workspace.selection,
            vec![0],
            "ping should select entity referencing it"
        );
        assert!(
            state.asset_selection.is_empty(),
            "Exiting asset inspector after selecting entity"
        );
        assert!(state.status.contains("已定位"), "status: {}", state.status);

        workspace.clear_selection();
        ping_asset(&mut workspace, &mut state, "assets/models/missing.gltf");
        assert!(workspace.selection.is_empty());
        assert!(state.status.contains("没有对象引用"));
    }

    #[test]
    fn transform_row_actions_emit_the_expected_edits() {
        let mut edits = Vec::new();
        apply_transform_row(TransformRow::Position, &[1.0, 2.0, 3.0], &mut |edit| {
            edits.push(edit);
        });
        apply_transform_row(TransformRow::Rotation, &[0.0, 0.0, 0.0, 1.0], &mut |edit| {
            edits.push(edit);
        });
        apply_transform_row(TransformRow::Scale, &[2.0, 2.0, 2.0], &mut |edit| {
            edits.push(edit);
        });

        apply_transform_row(TransformRow::Rotation, &[1.0, 0.0], &mut |edit| {
            edits.push(edit);
        });
        assert_eq!(edits.len(), 3);
        assert!(matches!(
            &edits[0],
            TransformEdit::Vector { field: "position", values } if *values == [1.0, 2.0, 3.0]
        ));
        assert!(matches!(&edits[1], TransformEdit::Rotation(_)));
        assert!(matches!(
            &edits[2],
            TransformEdit::Vector { field: "scale", .. }
        ));

        assert_eq!(TransformRow::Position.reset(), vec![0.0, 0.0, 0.0]);
        assert_eq!(TransformRow::Rotation.reset(), vec![0.0, 0.0, 0.0, 1.0]);
        assert_eq!(TransformRow::Scale.reset(), vec![1.0, 1.0, 1.0]);
    }

    #[test]
    fn a_component_without_schema_gets_an_editable_json_fallback() {
        let mut workspace = Workspace::sample();
        workspace.document_mut().doc.entities[0].components.insert(
            "test::Custom".to_string(),
            serde_json::json!({"speed": 3.0}),
        );
        workspace.select(0, false);
        let mut state = UiState::new();
        let before = workspace.document().doc.to_json_string();

        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                inspector(ui, &mut workspace, &mut state);
            });
        });

        assert_eq!(workspace.document().doc.to_json_string(), before);
        assert!(workspace.undo_history().is_empty());
    }

    #[test]
    fn locking_the_inspector_pins_entity_ids() {
        let mut workspace = Workspace::sample();
        let mut state = UiState::new();
        workspace.select(1, false);
        toggle_inspector_lock(&workspace, &mut state);
        assert!(state.inspector_lock.is_some());
        assert_eq!(inspector_indices(&workspace, &mut state), vec![1]);

        workspace.select(0, false);
        assert_eq!(
            inspector_indices(&workspace, &mut state),
            vec![1],
            "Ignore new selection while locked"
        );

        toggle_inspector_lock(&workspace, &mut state);
        assert!(state.inspector_lock.is_none());
        assert_eq!(inspector_indices(&workspace, &mut state), vec![0]);
    }

    #[test]
    fn locking_the_hierarchy_pins_the_highlighted_rows() {
        let mut workspace = Workspace::sample();
        let mut state = UiState::new();
        workspace.select(1, false);
        toggle_hierarchy_lock(&workspace, &mut state);
        assert!(state.hierarchy_lock.is_some());
        assert_eq!(hierarchy_indices(&workspace, &mut state), vec![1]);

        workspace.select(0, false);
        assert_eq!(
            hierarchy_indices(&workspace, &mut state),
            vec![1],
            "External selection does not change locked hierarchy highlight"
        );

        sync_hierarchy_lock(&workspace, &mut state);
        assert_eq!(
            hierarchy_indices(&workspace, &mut state),
            vec![0],
            "Internal hierarchy click updates locked snapshot"
        );

        toggle_hierarchy_lock(&workspace, &mut state);
        assert!(state.hierarchy_lock.is_none());
        assert_eq!(hierarchy_indices(&workspace, &mut state), vec![0]);
    }

    #[test]
    fn deleting_the_hierarchy_lock_target_releases_the_lock() {
        let mut workspace = Workspace::sample();
        let mut state = UiState::new();
        workspace.select(1, false);
        toggle_hierarchy_lock(&workspace, &mut state);
        workspace.delete_selection();
        let indices = hierarchy_indices(&workspace, &mut state);
        assert!(
            state.hierarchy_lock.is_none(),
            "Auto-unlock when locked object deleted"
        );
        assert_eq!(indices, workspace.selection);
    }

    #[test]
    fn reveal_expands_ancestors_and_clears_filters() {
        let mut workspace = Workspace::sample();
        let mut state = UiState::new();
        let detail = 3;
        let detail_id = workspace.document().doc.id_of_index(detail).unwrap();
        let cube_id = workspace.document().doc.id_of_index(1).unwrap();
        state.hierarchy_search = "not-a-match".to_string();
        state.hierarchy_lock = Some(vec![detail_id]);
        workspace.expanded.clear();

        reveal_in_hierarchy(&mut workspace, &mut state, detail);

        assert!(state.hierarchy_search.is_empty());
        assert!(state.hierarchy_lock.is_none());
        assert!(
            workspace.expanded.contains(&cube_id),
            "Parent should be expanded"
        );
        assert!(
            workspace.expanded.contains(&detail_id),
            "Target itself also expandable"
        );
        assert_eq!(state.hierarchy_reveal, Some(detail_id));
    }

    #[test]
    fn deleting_the_locked_entity_releases_the_lock() {
        let mut workspace = Workspace::sample();
        let mut state = UiState::new();
        workspace.select(1, false);
        toggle_inspector_lock(&workspace, &mut state);
        let locked_id = workspace.document().doc.id_of_index(1).expect("entity id");

        workspace.document_mut().doc.entities.remove(1);
        assert!(workspace.document().doc.index_of_id(locked_id).is_none());
        let indices = inspector_indices(&workspace, &mut state);
        assert!(
            state.inspector_lock.is_none(),
            "Auto-unlock when locked object deleted"
        );
        assert_eq!(
            indices,
            workspace.selection.clone(),
            "Fall back to document selection"
        );
    }

    #[test]
    fn play_mode_banner_only_appears_while_playing() {
        assert_eq!(play_mode_banner(false), None);
        let banner = play_mode_banner(true).expect("playing banner");
        assert!(banner.contains("discarded"), "{banner}");

        assert_ne!(play_mode_tint(false), play_mode_tint(true));
    }

    #[test]
    fn the_inspector_shows_the_play_banner_without_editing_the_document() {
        let mut workspace = Workspace::sample();
        workspace.play();
        let mut state = UiState::new();
        let before = workspace.document().doc.to_json_string();

        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                inspector(ui, &mut workspace, &mut state);
            });
        });

        assert_eq!(
            workspace.document().doc.to_json_string(),
            before,
            "Only shows hint, must not touch document"
        );
        assert!(
            workspace.undo_history().is_empty(),
            "Must not produce undo steps either"
        );

        workspace.stop();
        assert_eq!(
            play_mode_banner(workspace.playing && workspace.has_play_snapshot()),
            None
        );
    }

    #[test]
    fn runtime_debug_lines_name_states_and_montage_slots() {
        let node = prism_anim::GraphNodeSnapshot {
            name: "sm".into(),
            kind: prism_anim::GraphNodeKind::StateMachine,
            time: 0.5,
            state: 1,
            blending: false,
            blend_weight: 1.0,
            cached: false,
            weight: 1.0,
            state_name: "walk".into(),
            state_clip: "walk.ranim".into(),
        };
        let line = runtime_node_summary(&node);
        assert!(line.contains("sm"), "{line}");
        assert!(line.contains("StateMachine"), "{line}");
        assert!(line.contains("walk"), "{line}");
        assert!(line.contains("0.50s"), "{line}");

        let montage = prism_anim::MontageSnapshot {
            slot: "hit".into(),
            clip: Some("hit.anim".into()),
            section: Some("Impact".into()),
            warp_window: Some("VaultWarp".into()),
            warp_window_progress: Some(0.5),
            time: Some(0.25),
            weight: 0.5,
            playing: true,
            paused: true,
            finished: false,
            queued: 2,
        };
        let line = runtime_montage_summary(&montage);
        assert!(line.contains("hit"), "{line}");
        assert!(line.contains("0.25s"), "{line}");
        assert!(line.contains("paused"), "{line}");
        assert!(line.contains("+2 queued"), "{line}");
    }

    #[test]
    fn runtime_event_lines_show_name_and_time() {
        let event = prism_engine::scene::systems::animator::AnimEvent {
            entity: prism_engine::ecs::Entity::from_raw(7, 0),
            name: "footstep".into(),
            time: 0.3,
        };
        let line = runtime_event_summary(&event);
        assert!(line.contains("footstep"), "{line}");
        assert!(line.contains("0.30s"), "{line}");
    }

    #[test]
    fn per_pass_budget_splits_the_frame_budget_and_handles_zero() {
        assert_eq!(per_pass_budget_ms_with_total(0, FRAME_BUDGET_MS), 0.0);
        assert!((per_pass_budget_ms_with_total(1, FRAME_BUDGET_MS) - FRAME_BUDGET_MS).abs() < 1e-6);
        assert!(
            (per_pass_budget_ms_with_total(4, FRAME_BUDGET_MS) - FRAME_BUDGET_MS / 4.0).abs()
                < 1e-6
        );
        assert!(
            per_pass_budget_ms_with_total(8, FRAME_BUDGET_MS)
                < per_pass_budget_ms_with_total(4, FRAME_BUDGET_MS)
        );

        assert_eq!(per_pass_budget_ms_with_total(0, 8.0), 0.0);
        assert!((per_pass_budget_ms_with_total(4, 8.0) - 2.0).abs() < 1e-6);
        assert_eq!(
            per_pass_budget_ms_with_total(4, 0.0),
            0.0,
            "Illegal budget must not produce NaN/Inf shares"
        );
        assert_eq!(per_pass_budget_ms_with_total(4, f32::NAN), 0.0);
    }

    #[test]
    fn stepped_call_moves_between_draw_calls_without_wrapping() {
        assert_eq!(stepped_call(None, 0, 1), None);
        assert_eq!(stepped_call(Some(0), 0, 1), None);

        assert_eq!(stepped_call(None, 3, 1), Some(0));
        assert_eq!(stepped_call(None, 3, -1), Some(2));
        assert_eq!(stepped_call(Some(0), 3, -1), Some(0));
        assert_eq!(stepped_call(Some(2), 3, 1), Some(2));
        assert_eq!(stepped_call(Some(1), 3, 1), Some(2));

        assert_eq!(stepped_call(Some(9), 2, 0), Some(1));
    }

    #[test]
    fn every_editor_panel_builds_a_frame() {
        run_frame(|ui, workspace, state| hierarchy(ui, workspace, state));
        run_frame(|ui, workspace, state| anim_graph(ui, workspace, state, None));
        run_frame(|ui, workspace, state| inspector(ui, workspace, state));
        run_frame(|ui, workspace, state| assets(ui, workspace, state));
        run_frame(|ui, workspace, state| docs(ui, workspace, state));
        run_frame(|ui, workspace, state| log(ui, workspace, state));
        run_frame(|ui, workspace, state| history(ui, workspace, state));
        run_frame(|ui, _workspace, _state| {
            let mut settings = prism_engine::render_settings::RenderSettings::default();
            let mut viewport_scale = 1.0;
            let _ = render_graph(
                ui,
                _workspace,
                None,
                &mut settings,
                &mut viewport_scale,
                &mut false,
                &mut None,
                &mut None,
            );
        });
        let report = r#"{"generatedAtMs":1,"frame":2,"passes":[],"resources":[],
            "stats":{"textures":3,"textureBytes":1024,"meshes":2,"meshBytes":2048,
                     "materials":4,"materialBytes":384},
            "drawCalls":[
              {"pass":"forward","label":"mesh #0 · material 0","instances":1,
               "triangles":12,"visible":true,
               "state":"queue 2000 · cull Back · z-write true · One/Zero Add"},
              {"pass":"sprite","label":"SrcAlpha / OneMinusSrcAlpha","instances":4,
               "triangles":8,"visible":false}
            ]}"#;
        run_frame(|ui, _workspace, _state| {
            let mut settings = prism_engine::render_settings::RenderSettings::default();
            let mut viewport_scale = 1.0;
            let _ = render_graph(
                ui,
                _workspace,
                Some(report),
                &mut settings,
                &mut viewport_scale,
                &mut true,
                &mut Some(1),
                &mut None,
            );
        });
        run_frame(|ui, workspace, state| project_settings(ui, workspace, None, state));
        run_frame(|ui, workspace, state| {
            let mut prefs = Preferences::default();
            preferences(ui, &mut prefs, workspace, state, false);
        });
        run_frame(|ui, workspace, _state| theme(ui, workspace));
        run_frame(|ui, workspace, state| build(ui, workspace, state));
        run_frame(|ui, workspace, state| {
            let mut prefs = Preferences::default();
            shortcuts(ui, workspace, &mut prefs, state);
        });
        run_frame(|ui, workspace, _state| about(ui, workspace));
        run_frame(|ui, workspace, state| tools(ui, workspace, state));
    }

    #[test]
    fn boolean_window_builds_and_can_be_closed() {
        let ctx = egui::Context::default();
        let mut open = true;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            boolean_window(ctx, &mut open, "Test", |ui| {
                ui.label("content");
            });
        });
        assert!(open);
    }

    #[test]
    fn confirmation_actions_can_run_without_the_dialog_widget() {
        let mut workspace = Workspace::sample();
        let mut state = UiState::new();
        let before = workspace.document().doc.entities.len();
        state.pending_entity_delete = Some(vec![0]);
        apply_pending_entity_delete(&mut workspace, &mut state);
        assert!(workspace.document().doc.entities.len() < before);
        assert!(state.pending_entity_delete.is_none());
    }

    #[test]
    fn the_layout_group_editor_renders_without_a_panic() {
        for direction in ["None", "Row", "Column"] {
            let layout = serde_json::json!({
                "direction": direction,
                "spacing": 8,
                "padding": {"left": 4, "right": 4, "top": 2, "bottom": 2},
                "child_align": "Stretch",
                "fit_content": true
            });
            let ctx = egui::Context::default();
            let mut workspace = Workspace::sample();
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    layout_editor(ui, &mut workspace, 1, &layout);
                });
            });
        }
    }

    #[test]
    fn the_layout_editor_writes_the_same_json_paths_the_system_reads() {
        let mut workspace = Workspace::sample();
        let index = workspace.document_mut().doc.add_named_child(None, "Panel");
        let id = workspace.document().doc.id_of_index(index).unwrap();
        workspace.document_mut().doc.entities[index]
            .components
            .insert(
                STYLE_COMPONENT.to_string(),
                serde_json::json!({
                    "layout": {
                        "direction": "None",
                        "spacing": 0,
                        "padding": {"left": 0, "right": 0, "top": 0, "bottom": 0},
                        "child_align": "Start",
                        "fit_content": false
                    }
                }),
            );
        workspace.set_field(
            id,
            STYLE_COMPONENT,
            "layout.direction",
            serde_json::json!("Column"),
        );
        workspace.set_field(
            id,
            STYLE_COMPONENT,
            "layout.padding.left",
            serde_json::json!(12),
        );
        let layout =
            &workspace.document().doc.entities[index].components[STYLE_COMPONENT]["layout"];
        assert_eq!(layout["direction"], serde_json::json!("Column"));
        assert_eq!(layout["padding"]["left"].as_f64(), Some(12.0));
    }

    #[test]
    fn panel_row_metrics_follow_compact_preference() {
        assert_eq!(tree_row_height(true), 15.0);
        assert_eq!(tree_row_height(false), 18.0);
        assert_eq!(row_height(true), 16.0);
        assert_eq!(row_height(false), 20.0);
    }

    #[test]
    fn mixed_transform_axes_flags_only_the_differing_components() {
        let mut workspace = Workspace::sample();
        workspace.clear_selection();
        workspace.document_mut().doc.entities[0].transform.position = [1.0, 2.0, 3.0];
        workspace.document_mut().doc.entities[1].transform.position = [1.0, 5.0, 3.0];
        let mixed = mixed_transform_axes(&workspace, &[0, 1]);
        assert!(!mixed[0][0]);
        assert!(mixed[0][1], "Only Y differs");
        assert!(!mixed[0][2]);
        assert!(
            mixed[2].iter().all(|flag| !flag),
            "Not flagged when scale same"
        );

        let single = mixed_transform_axes(&workspace, &[0]);
        assert!(single.iter().all(|row| row.iter().all(|flag| !flag)));
    }

    #[test]
    fn mixed_transform_axes_ignores_the_quaternion_double_cover() {
        let mut workspace = Workspace::sample();
        workspace.document_mut().doc.entities[0].transform.rotation =
            prism_editor_core::viewport_math::euler_degrees_to_quat([0.0, 45.0, 0.0]);

        workspace.document_mut().doc.entities[1].transform.rotation =
            prism_editor_core::viewport_math::euler_degrees_to_quat([0.0, 45.0, 0.0])
                .map(|value| -value);
        let mixed = mixed_transform_axes(&workspace, &[0, 1]);
        assert!(mixed[1].iter().all(|flag| !flag), "{mixed:?}");
    }

    #[test]
    fn inspector_drag_brackets_multi_frame_edits_into_single_undo() {
        let mut workspace = Workspace::sample();
        workspace.selection = vec![0];
        let id = workspace.document().doc.id_of_index(0).unwrap();
        let mut state = UiState::new();
        let ctx = egui::Context::default();

        // Frame 1: pointer press inside inspector panel initiates interaction bracketing.
        let mut frame1 = egui::RawInput::default();
        frame1
            .events
            .push(egui::Event::PointerMoved(egui::pos2(80.0, 80.0)));
        frame1.events.push(egui::Event::PointerButton {
            pos: egui::pos2(80.0, 80.0),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx.run(frame1, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| inspector(ui, &mut workspace, &mut state));
        });
        assert!(workspace.document().undo.interaction_active());
        workspace.set_transform_axis(id, "position", 0, 1.5);

        // Frame 2: held drag keeps interaction open across edits (even outside panel rect).
        let mut frame2 = egui::RawInput::default();
        frame2
            .events
            .push(egui::Event::PointerMoved(egui::pos2(900.0, 80.0)));
        let _ = ctx.run(frame2, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| inspector(ui, &mut workspace, &mut state));
        });
        assert!(workspace.document().undo.interaction_active());
        workspace.set_transform_axis(id, "position", 0, 3.0);
        assert_eq!(workspace.undo_history().len(), 1);

        // Frame 3: pointer release commits interaction so the next edit is a distinct step.
        let mut frame3 = egui::RawInput::default();
        frame3.events.push(egui::Event::PointerButton {
            pos: egui::pos2(900.0, 80.0),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx.run(frame3, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| inspector(ui, &mut workspace, &mut state));
        });
        assert!(!workspace.document().undo.interaction_active());
        workspace.set_transform_axis(id, "position", 0, 4.0);
        assert_eq!(workspace.undo_history().len(), 2);
    }

    #[test]
    fn hierarchy_semantic_icons_reflect_entity_components() {
        let mut workspace = Workspace::sample();
        let cam = workspace.document_mut().doc.add_named_child(None, "Cam");
        workspace.document_mut().doc.entities[cam]
            .components
            .insert(
                "prism_engine::scene::Camera".to_string(),
                serde_json::json!({}),
            );
        let light = workspace.document_mut().doc.add_named_child(None, "Sun");
        workspace.document_mut().doc.entities[light]
            .components
            .insert(
                "prism_engine::scene::PointLight".to_string(),
                serde_json::json!({}),
            );
        let empty = workspace.document_mut().doc.add_named_child(None, "Empty");
        assert_eq!(
            hierarchy::entity_semantic_icon(&workspace.document().doc.entities[cam]),
            icons::CAMERA
        );
        assert_eq!(
            hierarchy::entity_semantic_icon(&workspace.document().doc.entities[light]),
            icons::LIGHTBULB
        );
        assert_eq!(
            hierarchy::entity_semantic_icon(&workspace.document().doc.entities[empty]),
            icons::CIRCLE
        );
    }
}
