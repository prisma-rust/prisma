//! Unit tests for editor workspace state.

use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_empty_path_replaces_the_whole_component_in_one_step() {
        let mut workspace = Workspace::sample();
        let id = workspace.document().doc.id_of_index(0).unwrap();
        workspace.document_mut().doc.entities[0]
            .components
            .insert("test::Rig".to_string(), serde_json::json!({"speed": 9.0}));
        let undo_before = workspace.undo_history().len();

        workspace.set_field(id, "test::Rig", "", serde_json::json!({"speed": 1.0}));

        assert_eq!(
            workspace.document().doc.entities[0].components["test::Rig"],
            serde_json::json!({"speed": 1.0})
        );
        assert_eq!(workspace.undo_history().len(), undo_before + 1);
        workspace.undo();
        assert_eq!(
            workspace.document().doc.entities[0].components["test::Rig"]["speed"],
            serde_json::json!(9.0)
        );
    }

    #[test]
    fn dragging_a_point_anchor_preserves_the_visual_rect() {
        let mut workspace = Workspace::sample();
        workspace.document_mut().doc.entities[0].components.insert(
            "prism_engine::ui::Style".to_string(),
            json!({
                "width": 100.0,
                "height": 50.0,
                "anchors": {"min_x": 0.5, "min_y": 0.5, "max_x": 0.5, "max_y": 0.5},
                "pivot": {"x": 0.5, "y": 0.5},
                "margin": {"left": 0.0, "right": 0.0, "top": 0.0, "bottom": 0.0}
            }),
        );
        let undo_before = workspace.undo_history().len();

        assert!(workspace.set_ui_anchors_preserving_rect(
            0,
            [0.25, 0.25, 0.25, 0.25],
            [0.0, 0.0, 1000.0, 500.0],
            [450.0, 225.0, 100.0, 50.0],
        ));

        let style = &workspace.document().doc.entities[0].components["prism_engine::ui::Style"];
        assert_eq!(style["anchors"]["min_x"], json!(0.25));
        assert_eq!(style["anchors"]["max_x"], json!(0.25));
        assert_eq!(style["margin"]["left"], json!(250.0));
        assert_eq!(style["margin"]["top"], json!(125.0));
        assert_eq!(style["margin"]["right"], json!(0.0));
        assert_eq!(style["margin"]["bottom"], json!(0.0));
        assert_eq!(workspace.undo_history().len(), undo_before + 1);

        workspace.undo();
        let style = &workspace.document().doc.entities[0].components["prism_engine::ui::Style"];
        assert_eq!(style["anchors"]["min_x"], json!(0.5));
        assert_eq!(style["margin"]["left"], json!(0.0));
    }

    #[test]
    fn dragging_to_a_stretched_anchor_uses_margins_for_both_edges() {
        let mut workspace = Workspace::sample();
        workspace.document_mut().doc.entities[0].components.insert(
            "prism_engine::ui::Style".to_string(),
            json!({
                "width": 100.0,
                "height": 50.0,
                "anchors": {"min_x": 0.5, "min_y": 0.5, "max_x": 0.5, "max_y": 0.5},
                "pivot": {"x": 0.5, "y": 0.5}
            }),
        );

        assert!(workspace.set_ui_anchors_preserving_rect(
            0,
            [0.25, 0.25, 0.75, 0.75],
            [0.0, 0.0, 1000.0, 500.0],
            [450.0, 225.0, 100.0, 50.0],
        ));

        let style = &workspace.document().doc.entities[0].components["prism_engine::ui::Style"];
        assert_eq!(style["margin"]["left"], json!(200.0));
        assert_eq!(style["margin"]["right"], json!(200.0));
        assert_eq!(style["margin"]["top"], json!(100.0));
        assert_eq!(style["margin"]["bottom"], json!(100.0));
        assert_eq!(
            style["width"],
            json!(100.0),
            "explicit width not overridden by anchor"
        );
    }

    #[test]
    fn point_anchor_writes_a_width_when_the_style_had_none() {
        let mut workspace = Workspace::sample();
        workspace.document_mut().doc.entities[0].components.insert(
            "prism_engine::ui::Style".to_string(),
            json!({
                "anchors": {"min_x": 0.5, "min_y": 0.5, "max_x": 0.5, "max_y": 0.5}
            }),
        );

        assert!(workspace.set_ui_anchors_preserving_rect(
            0,
            [0.5, 0.5, 0.5, 0.5],
            [0.0, 0.0, 1000.0, 500.0],
            [450.0, 225.0, 100.0, 50.0],
        ));

        let style = &workspace.document().doc.entities[0].components["prism_engine::ui::Style"];
        assert_eq!(style["width"], json!(100.0));
        assert_eq!(style["height"], json!(50.0));
        assert_eq!(style["margin"]["left"], json!(0.0));
        assert_eq!(style["margin"]["top"], json!(0.0));
    }

    #[test]
    fn raw_anchor_edit_leaves_margin_and_size_untouched() {
        let mut workspace = Workspace::sample();
        workspace.document_mut().doc.entities[0].components.insert(
            "prism_engine::ui::Style".to_string(),
            json!({
                "width": 100.0,
                "height": 50.0,
                "anchors": {"min_x": 0.5, "min_y": 0.5, "max_x": 0.5, "max_y": 0.5},
                "margin": {"left": 12.0, "right": 0.0, "top": 8.0, "bottom": 0.0}
            }),
        );
        let undo_before = workspace.undo_history().len();

        assert!(workspace.set_ui_anchors_raw(0, [0.25, 0.25, 0.75, 0.75]));

        let style = &workspace.document().doc.entities[0].components["prism_engine::ui::Style"];
        assert_eq!(style["anchors"]["min_x"], json!(0.25));
        assert_eq!(style["anchors"]["max_x"], json!(0.75));
        assert_eq!(style["margin"]["left"], json!(12.0));
        assert_eq!(style["margin"]["top"], json!(8.0));
        assert_eq!(style["width"], json!(100.0));
        assert_eq!(style["height"], json!(50.0));
        assert_eq!(workspace.undo_history().len(), undo_before + 1);
    }

    #[test]
    fn pasting_component_values_overwrites_matching_components_in_one_step() {
        let mut workspace = Workspace::sample();
        workspace.document_mut().doc.entities[0]
            .components
            .insert("test::Rig".to_string(), serde_json::json!({"speed": 1.0}));
        workspace.document_mut().doc.entities[1]
            .components
            .insert("test::Rig".to_string(), serde_json::json!({"speed": 2.0}));

        workspace.copy_component_at(0, "test::Rig");
        let undo_before = workspace.undo_history().len();
        assert_eq!(
            workspace.paste_component_values(&[1], "test::Rig"),
            1,
            "only same-type components override"
        );
        assert_eq!(
            workspace.document().doc.entities[1].components["test::Rig"]["speed"],
            serde_json::json!(1.0)
        );
        assert_eq!(workspace.undo_history().len(), undo_before + 1);

        assert_eq!(workspace.paste_component_values(&[0], "test::Other"), 0);
        assert_eq!(workspace.paste_component_values(&[2], "test::Rig"), 0);
    }

    #[test]
    fn maximize_on_play_restores_the_previous_layout() {
        let mut workspace = Workspace::sample();
        workspace.maximize_on_play = true;
        assert!(workspace.maximized_tab.is_none());

        workspace.play();
        assert_eq!(workspace.maximized_tab.as_deref(), Some("gameView"));

        workspace.stop();
        assert!(
            workspace.maximized_tab.is_none(),
            "no panel maximized before entering Play, Stop must return to none maximized"
        );

        workspace.maximized_tab = Some("sceneView".to_string());
        workspace.play();
        assert_eq!(workspace.maximized_tab.as_deref(), Some("gameView"));
        workspace.stop();
        assert_eq!(workspace.maximized_tab.as_deref(), Some("sceneView"));
    }

    #[test]
    fn maximize_on_play_off_leaves_the_layout_alone() {
        let mut workspace = Workspace::sample();
        workspace.maximized_tab = Some("hierarchy".to_string());
        workspace.play();
        assert_eq!(workspace.maximized_tab.as_deref(), Some("hierarchy"));
        workspace.stop();
        assert_eq!(workspace.maximized_tab.as_deref(), Some("hierarchy"));
    }

    #[test]
    fn camera_fov_and_clip_planes_are_clamped() {
        let mut camera = OrbitCamera::default();
        assert!((camera.fov_y_degrees - 60.0).abs() < 1e-6);
        camera.set_fov_degrees(5.0);
        assert!((camera.fov_y_degrees - 10.0).abs() < 1e-6);
        camera.set_fov_degrees(400.0);
        assert!((camera.fov_y_degrees - 170.0).abs() < 1e-6);
        let before = camera.fov_y_degrees;
        camera.set_fov_degrees(f32::NAN);
        assert_eq!(camera.fov_y_degrees, before, "non-finite value ignored");

        camera.set_clip_planes(0.0, 100.0);
        assert!((camera.near - 0.1).abs() < 1e-6, "illegal near unchanged");
        camera.set_clip_planes(1.0, 0.5);
        assert!((camera.near - 1.0).abs() < 1e-6);
        assert!(
            (camera.far - 1.01).abs() < 1e-6,
            "far at least 1cm larger than near"
        );
        camera.set_clip_planes(2.0, 500.0);
        assert!((camera.near - 2.0).abs() < 1e-6);
        assert!((camera.far - 500.0).abs() < 1e-6);
    }

    #[test]
    fn focus_distance_follows_the_configured_fov() {
        let mut wide = OrbitCamera::default();
        wide.set_fov_degrees(90.0);
        wide.focus([0.0, 0.0, 0.0], 1.0);
        let mut narrow = OrbitCamera::default();
        narrow.set_fov_degrees(30.0);
        narrow.focus([0.0, 0.0, 0.0], 1.0);
        assert!(
            narrow.distance > wide.distance,
            "30° {} should be farther than 90° {}",
            narrow.distance,
            wide.distance
        );
        assert!((wide.distance - 1.2 / 45.0f32.to_radians().sin()).abs() < 1e-3);
    }

    #[test]
    fn aligning_the_camera_to_an_axis_puts_the_eye_on_that_side() {
        let mut camera = OrbitCamera::default();
        camera.target = [1.0, 2.0, 3.0];
        for (axis, sign) in [
            (0, 1.0),
            (0, -1.0),
            (1, 1.0),
            (1, -1.0),
            (2, 1.0),
            (2, -1.0),
        ] {
            camera.align_to_axis(axis, sign);
            let eye = camera.eye();
            let offset = [
                eye[0] - camera.target[0],
                eye[1] - camera.target[1],
                eye[2] - camera.target[2],
            ];
            let length =
                (offset[0] * offset[0] + offset[1] * offset[1] + offset[2] * offset[2]).sqrt();
            let expected = camera.distance;
            assert!(
                (length - expected).abs() < 1e-3,
                "axis {axis} sign {sign}: eye distance {} != {}",
                length,
                expected
            );
            for other in 0..3 {
                let want = if other == axis { sign } else { 0.0 };
                let normalized = offset[other] / length;
                assert!(
                    (normalized - want).abs() < 1e-3,
                    "axis {axis} sign {sign} component {other}: {normalized} != {want}"
                );
            }
        }

        let before = camera;
        camera.align_to_axis(9, 1.0);
        assert_eq!(camera, before);
    }

    #[test]
    fn component_presets_replace_by_name_and_keep_values() {
        let mut workspace = Workspace::sample();
        workspace.save_component_preset("a::B", "hero", serde_json::json!({"x": 1}));
        workspace.save_component_preset("a::C", "hero", serde_json::json!({"y": 2}));
        workspace.save_component_preset("a::B", "hero", serde_json::json!({"x": 3}));

        assert_eq!(
            workspace.component_preset_names("a::B"),
            vec!["hero".to_string()]
        );
        assert_eq!(
            workspace.component_preset_value("a::B", "hero"),
            Some(serde_json::json!({"x": 3}))
        );
        assert_eq!(
            workspace.component_preset_value("a::C", "hero"),
            Some(serde_json::json!({"y": 2}))
        );
        assert_eq!(workspace.component_preset_value("a::B", "missing"), None);

        workspace.save_component_preset("a::B", "   ", serde_json::json!({}));
        assert_eq!(workspace.component_preset_names("a::B").len(), 1);
    }

    #[test]
    fn orbit_applies_horizontal_and_vertical_deltas_independently() {
        let mut camera = OrbitCamera::default();
        let (yaw, pitch) = (camera.yaw, camera.pitch);
        camera.orbit(10.0, 0.0);
        assert!((camera.yaw - (yaw - 0.1)).abs() < 1e-6);
        assert_eq!(camera.pitch, pitch);

        camera = OrbitCamera::default();
        let (yaw, pitch) = (camera.yaw, camera.pitch);
        camera.orbit(0.0, 10.0);
        assert_eq!(camera.yaw, yaw);
        assert!((camera.pitch - (pitch + 0.1)).abs() < 1e-6);

        camera = OrbitCamera::default();
        let (yaw, pitch) = (camera.yaw, camera.pitch);
        camera.orbit(10.0, -20.0);
        assert!((camera.yaw - (yaw - 0.1)).abs() < 1e-6);
        assert!((camera.pitch - (pitch - 0.2)).abs() < 1e-6);
    }

    #[test]
    fn the_2d_view_locks_the_camera_to_the_xy_plane() {
        let mut camera = OrbitCamera::default();
        camera.orbit(30.0, 20.0);
        assert!(camera.yaw.abs() > 1e-6 || camera.pitch.abs() > 1e-6);

        camera.set_2d(true);
        assert!(camera.orthographic);
        assert_eq!((camera.yaw, camera.pitch), (0.0, 0.0));

        camera.orbit(50.0, 50.0);
        camera.look_in_place(50.0, 50.0);
        assert_eq!(
            (camera.yaw, camera.pitch),
            (0.0, 0.0),
            "2D view should not be rotated"
        );

        let eye = camera.eye();
        assert!((eye[2] - camera.distance).abs() < 1e-5);
        assert!(camera.ortho_half_height().unwrap() > 0.0);

        camera.toggle_2d();
        assert!(!camera.orthographic);
        assert!(camera.ortho_half_height().is_none());
        camera.orbit(50.0, 0.0);
        assert!(camera.yaw.abs() > 1e-6, "orbit restored after exiting 2D");
    }

    fn anchor_point(camera: &OrbitCamera, x: f32, y: f32, width: f32, height: f32) -> [f32; 3] {
        let frame = crate::viewport_math::camera_frame(camera);
        let (origin, direction) =
            crate::viewport_math::ray_from_screen(x, y, &frame, width, height);
        let distance = camera.distance;
        [
            origin[0] + direction[0] * distance,
            origin[1] + direction[1] * distance,
            origin[2] + direction[2] * distance,
        ]
    }

    fn screen_of(camera: &OrbitCamera, point: [f32; 3], width: f32, height: f32) -> (f32, f32) {
        let frame = crate::viewport_math::camera_frame(camera);
        let (x, y, _depth) =
            crate::viewport_math::project(point, &frame, width, height).expect("on screen");
        (x, y)
    }

    #[test]
    fn zooming_towards_the_cursor_keeps_the_anchor_on_screen() {
        let mut camera = OrbitCamera {
            target: [1.0, 0.5, -2.0],
            yaw: 0.4,
            pitch: 0.3,
            distance: 14.0,
            ..OrbitCamera::default()
        };
        let before = camera;
        let anchor = anchor_point(&camera, 640.0, 180.0, 800.0, 600.0);
        let (x0, y0) = screen_of(&camera, anchor, 800.0, 600.0);

        camera.zoom_towards(240.0, Some(anchor));

        assert!(camera.distance < before.distance, "wheel forward zooms in");

        let percentage = 1.0 - camera.distance / before.distance;
        for axis in 0..3 {
            let expected = before.target[axis] + (anchor[axis] - before.target[axis]) * percentage;
            assert!((camera.target[axis] - expected).abs() < 1e-4);
        }

        let (x1, y1) = screen_of(&camera, anchor, 800.0, 600.0);
        assert!(
            (x1 - x0).abs() < 0.05 && (y1 - y0).abs() < 0.05,
            "anchor moved on screen: ({x0}, {y0}) -> ({x1}, {y1})"
        );
    }

    #[test]
    fn zooming_towards_the_cursor_works_in_the_2d_view() {
        let mut camera = OrbitCamera::default();
        camera.set_2d(true);
        let before = camera;
        let anchor = anchor_point(&camera, 720.0, 120.0, 800.0, 600.0);
        let (x0, y0) = screen_of(&camera, anchor, 800.0, 600.0);

        camera.zoom_towards(300.0, Some(anchor));

        assert!(camera.distance < before.distance);
        let (x1, y1) = screen_of(&camera, anchor, 800.0, 600.0);
        assert!(
            (x1 - x0).abs() < 0.05 && (y1 - y0).abs() < 0.05,
            "2D anchor moved on screen: ({x0}, {y0}) -> ({x1}, {y1})"
        );
    }

    #[test]
    fn zooming_without_a_cursor_keeps_the_target() {
        let mut camera = OrbitCamera::default();
        let before = camera;
        camera.zoom_towards(200.0, None);
        assert_eq!(camera.target, before.target);
        assert!(camera.distance < before.distance);

        let mut camera = OrbitCamera::default();
        camera.zoom_towards(200.0, Some([f32::NAN, 0.0, 0.0]));
        assert_eq!(camera.target, before.target);
    }

    #[test]
    fn zooming_towards_the_cursor_respects_the_distance_limits() {
        let mut camera = OrbitCamera {
            distance: 500.0,
            ..OrbitCamera::default()
        };

        camera.zoom_towards(-500.0, Some([10.0, 10.0, 0.0]));
        assert_eq!(camera.distance, 500.0);
        assert_eq!(camera.target, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn focus_fits_the_radius_in_the_ortho_size() {
        let mut camera = OrbitCamera::default();
        camera.set_2d(true);
        camera.focus([1.0, 2.0, 0.0], 5.0);
        assert_eq!(camera.target, [1.0, 2.0, 0.0]);
        assert!((camera.distance - 6.0).abs() < 1e-4);
    }

    #[test]
    fn orbit_keeps_pitch_inside_pole_limits() {
        let mut camera = OrbitCamera::default();
        camera.orbit(0.0, 1000.0);
        assert_eq!(camera.pitch, 1.5);
        camera.orbit(0.0, -1000.0);
        assert_eq!(camera.pitch, -1.5);
    }

    #[test]
    fn look_in_place_rotates_view_without_moving_eye() {
        let mut camera = OrbitCamera::default();
        let eye = camera.eye();
        camera.look_in_place(10.0, -5.0);
        let next_eye = camera.eye();
        for axis in 0..3 {
            assert!((next_eye[axis] - eye[axis]).abs() < 1e-5);
        }
        assert!((camera.yaw + 0.1).abs() < 1e-6);
        assert!((camera.pitch + 0.05).abs() < 1e-6);
    }

    #[test]
    fn fly_move_translates_eye_and_target_together() {
        let mut camera = OrbitCamera::default();
        let eye = camera.eye();
        let target = camera.target;
        camera.fly_move(1.0, 0.0, 0.0, 1.0);
        let next_eye = camera.eye();
        let delta = [
            camera.target[0] - target[0],
            camera.target[1] - target[1],
            camera.target[2] - target[2],
        ];
        assert!(delta[2] < 0.0);
        for axis in 0..3 {
            assert!((next_eye[axis] - eye[axis] - delta[axis]).abs() < 1e-5);
        }
    }

    #[test]
    fn reset_scene_camera_restores_default_orbit() {
        let mut workspace = Workspace::sample();
        workspace.camera.target = [9.0, -4.0, 2.0];
        workspace.camera.yaw = -1.2;
        workspace.camera.pitch = 1.1;
        workspace.camera.distance = 42.0;
        workspace.run(Command::ResetSceneCamera);
        assert_eq!(workspace.camera, OrbitCamera::default());
        assert_eq!(workspace.camera.yaw, 0.0);
        assert_eq!(workspace.camera.pitch, 0.0);
    }

    /// A workspace built from [`fixture`] with its schema snapshot installed.
    /// Production never reads a schema from the fixture: the host installs the
    /// live projection from `prism-editor-api`. Tests keep a tiny explicit
    /// schema so they can assert schema-driven behaviour without linking it.
    fn fixture_workspace() -> Workspace {
        let value = fixture();
        let mut workspace = Workspace::load_fixture(&value);
        workspace.set_schema(fixture_schema(&value));
        workspace
    }

    fn fixture_schema(value: &Value) -> Schema {
        value
            .get("schema")
            .and_then(Schema::from_value)
            .unwrap_or_default()
    }

    fn fixture() -> Value {
        json!({
            "schema": { "types": [
                {
                    "name": "prism_engine::scene::Camera",
                    "typeId": "c",
                    "version": 1,
                    "isComponent": true,
                    "defaultJson": { "fov_y_degrees": 60.0 },
                    "fields": [
                        { "name": "fov_y_degrees", "kind": "f32", "structType": null, "inner": null,
                          "arrayLen": null, "variants": null, "readOnly": false }
                    ]
                }
            ]},
            "presets": [
                { "id": "camera", "label": "camera", "name": "Camera",
                  "components": ["prism_engine::scene::Camera"],
                  "overrides": {}, "position": [0.0, 0.0, 10.0], "rotation": [0.0, 0.0, 0.0, 1.0] },
                { "id": "empty", "label": "empty entity", "name": "entity",
                  "overrides": {}, "position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0] }
            ],
            "assetInfos": [
                { "path": "assets/scenes/a.scene.json", "sizeBytes": 10, "kind": "scene", "importer": "TextScriptImporter" }
            ],
            "docs": {
                "assets/scenes/a.scene.json": {
                    "version": 3,
                    "entities": [
                        { "id": 1, "name": "Camera", "parent": null,
                          "transform": {"position":[0,1,-10],"rotation":[0,0,0,1],"scale":[1,1,1]},
                          "components": { "prism_engine::scene::Camera": { "fov_y_degrees": 60.0 } } },
                        { "id": 2, "name": "Cube", "parent": 1,
                          "transform": {"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                          "components": {} }
                    ]
                }
            },
            "templateDocs": {
                "prefab": { "camera": { "version": 3, "entities": [
                    { "id": 1, "name": "Camera", "parent": null,
                      "transform": {"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                      "components": {} }
                ]}}
            },
            "templatesByKind": { "prefab": [ { "id": "camera", "label": "camera" } ] },
            "windows": [
                { "id": "sceneView", "title": "Scene View", "menuPath": "Window/Scene View", "shortcut": null }
            ]
        })
    }

    #[test]
    fn the_demo_fixture_carries_no_schema_snapshot() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../public/demo.json");
        let text = std::fs::read_to_string(&path).expect("editor fixture");
        let value: Value = serde_json::from_str(&text).expect("editor fixture is JSON");
        assert!(
            value.get("schema").is_none(),
            "{} must not freeze the component schema; the host installs the \
             prism-editor-api projection",
            path.display()
        );
    }

    #[test]
    fn loads_fixture_documents_templates_and_panels() {
        let workspace = fixture_workspace();
        assert_eq!(workspace.docs.len(), 1);
        assert_eq!(workspace.docs[0].title(), "a.scene.json");
        assert_eq!(workspace.docs[0].doc.entities.len(), 2);
        assert_eq!(workspace.templates.len(), 1);
        assert_eq!(workspace.presets.len(), 2);
        assert_eq!(workspace.assets.len(), 1);
        assert!(workspace.panel("sceneView").is_some());
        assert_eq!(workspace.panel_title("sceneView"), "Scene View");
    }

    #[test]
    fn add_entity_uses_preset_and_parent() {
        let mut workspace = fixture_workspace();
        workspace.select(0, false);
        workspace.add_entity("camera");
        let doc = &workspace.document().doc;
        assert_eq!(doc.entities.len(), 3);
        let added = doc.entities.last().unwrap();
        assert_eq!(added.display_name(), "Camera");
        assert_eq!(added.parent, Some(1), "attached under selected camera");
        assert!(added.has_component("prism_engine::scene::Camera"));
        assert!(workspace.can_undo());
    }

    #[test]
    fn add_entity_names_stay_unique_among_siblings() {
        let mut workspace = fixture_workspace();
        workspace.clear_selection();
        workspace.add_entity("");
        workspace.add_entity("");
        let doc = &workspace.document().doc;
        let names: Vec<String> = doc
            .entities
            .iter()
            .map(|entity| entity.display_name())
            .collect();
        assert!(names.contains(&"GameObject".to_string()));
        assert!(names.contains(&"GameObject (1)".to_string()));
    }

    #[test]
    fn multi_field_edit_writes_all_selected_in_one_step() {
        let mut workspace = fixture_workspace();

        workspace.select(0, false);
        workspace.add_component("prism_engine::scene::Camera");
        workspace.select(1, false);
        workspace.add_component("prism_engine::scene::Camera");

        let ids: Vec<u32> = workspace
            .document()
            .doc
            .entities
            .iter()
            .filter_map(|entity| entity.id)
            .collect();
        let applied = workspace.set_field_multi(
            &ids,
            "prism_engine::scene::Camera",
            "fov_y_degrees",
            json!(55.0),
        );
        assert_eq!(applied, 2);
        for entity in &workspace.document().doc.entities {
            assert_eq!(
                entity.components["prism_engine::scene::Camera"]["fov_y_degrees"],
                json!(55.0)
            );
        }

        workspace.undo();
        for entity in &workspace.document().doc.entities {
            assert_eq!(
                entity.components["prism_engine::scene::Camera"]["fov_y_degrees"],
                json!(60.0)
            );
        }

        let common = workspace.common_components(&[0, 1]);
        assert!(common
            .iter()
            .any(|component| component.name == "prism_engine::scene::Camera"));
        workspace.remove_component_multi(&[0, 1], "prism_engine::scene::Camera");
        for entity in &workspace.document().doc.entities {
            assert!(!entity.has_component("prism_engine::scene::Camera") || entity.id != Some(1));
        }
    }

    #[test]
    fn rotate_and_scale_gizmo_operations_are_undoable() {
        let mut workspace = fixture_workspace();
        let id = workspace.document().doc.entities[0].id.unwrap();
        workspace.rotate_entity(id, 1, std::f32::consts::FRAC_PI_2);
        let rotation = workspace.document().doc.entities[0].transform.rotation;

        assert!((rotation[3] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4);
        assert!(workspace.can_undo());
        workspace.undo();
        assert_eq!(
            workspace.document().doc.entities[0].transform.rotation,
            [0.0, 0.0, 0.0, 1.0]
        );

        workspace.set_scale_axis(id, 0, 2.5);
        assert_eq!(workspace.document().doc.entities[0].transform.scale[0], 2.5);
        workspace.set_scale_axis(id, 0, -5.0);
        assert_eq!(
            workspace.document().doc.entities[0].transform.scale[0],
            0.01,
            "negative clamped to min"
        );

        let history = workspace.document().undo.history().len();
        workspace.rotate_entity(id, 0, 0.0);
        assert_eq!(workspace.document().undo.history().len(), history);
    }

    #[test]
    fn local_gizmo_rotation_uses_entity_axis() {
        use crate::viewport_math::{basis_from_quat, quat_axis_angle, world_rotation};

        let mut workspace = Workspace::sample();
        let index = workspace
            .document()
            .doc
            .entities
            .iter()
            .position(|entity| entity.display_name() == "Cube")
            .expect("Cube in sample");
        let id = workspace.document().doc.entities[index].id.unwrap();
        let base_rotation = quat_axis_angle(1, std::f32::consts::FRAC_PI_2);

        workspace.document_mut().doc.entities[index]
            .transform
            .rotation = base_rotation;
        workspace.gizmo_space = GizmoSpace::World;
        workspace.rotate_entity(id, 0, std::f32::consts::FRAC_PI_2);
        let world_x = basis_from_quat(world_rotation(&workspace.document().doc, index))[0];
        assert!(
            (world_x[1] - 1.0).abs() < 1e-3,
            "world axis after X rotation X should point +Y, actual {world_x:?}"
        );

        workspace.document_mut().doc.entities[index]
            .transform
            .rotation = base_rotation;
        workspace.gizmo_space = GizmoSpace::Local;
        workspace.rotate_entity(id, 0, std::f32::consts::FRAC_PI_2);
        let local_x = basis_from_quat(world_rotation(&workspace.document().doc, index))[0];
        assert!(
            (local_x[2] + 1.0).abs() < 1e-3,
            "local axis after X rotation X should point −Z, actual {local_x:?}"
        );
    }

    #[test]
    fn gizmo_basis_follows_parent_rotation_and_space() {
        let mut workspace = Workspace::sample();
        let cube = workspace
            .document()
            .doc
            .entities
            .iter()
            .position(|entity| entity.display_name() == "Cube")
            .expect("Cube in sample");
        let detail = workspace
            .document()
            .doc
            .entities
            .iter()
            .position(|entity| entity.display_name() == "Detail")
            .expect("Detail in sample");
        workspace.document_mut().doc.entities[cube]
            .transform
            .rotation = crate::viewport_math::quat_axis_angle(1, std::f32::consts::FRAC_PI_2);
        workspace.select(detail, false);

        assert_eq!(
            workspace.gizmo_basis(),
            crate::viewport_math::IDENTITY_BASIS,
            "default is world space"
        );
        workspace.toggle_gizmo_space();
        let basis = workspace.gizmo_basis();
        assert!(
            (basis[0][2] + 1.0).abs() < 1e-3,
            "parent rotated Y 90°, child local X points −Z, actual {:?}",
            basis[0]
        );
    }

    #[test]
    fn gizmo_scale_axis_distinguishes_world_and_local_space() {
        let mut workspace = Workspace::sample();
        let index = index_named(&workspace, "Cube");
        let id = workspace.document().doc.entities[index].id.unwrap();
        workspace.document_mut().doc.entities[index]
            .transform
            .rotation = crate::viewport_math::quat_axis_angle(1, std::f32::consts::FRAC_PI_2);

        workspace.gizmo_space = GizmoSpace::Local;
        assert_eq!(workspace.gizmo_scale_axis(id, 0), 0);

        workspace.gizmo_space = GizmoSpace::World;
        assert_eq!(workspace.gizmo_scale_axis(id, 0), 2);
    }

    #[test]
    fn translate_entity_world_converts_into_parent_space() {
        let mut workspace = Workspace::sample();
        let cube = workspace
            .document()
            .doc
            .entities
            .iter()
            .position(|entity| entity.display_name() == "Cube")
            .expect("Cube in sample");
        let detail = workspace
            .document()
            .doc
            .entities
            .iter()
            .position(|entity| entity.display_name() == "Detail")
            .expect("Detail in sample");
        workspace.document_mut().doc.entities[cube]
            .transform
            .rotation = crate::viewport_math::quat_axis_angle(1, std::f32::consts::FRAC_PI_2);
        let id = workspace.document().doc.entities[detail].id.unwrap();
        let delta = [0.25, 0.5, -0.75];

        workspace.translate_entity_world(id, delta);

        let parent_rotation = crate::viewport_math::world_rotation(&workspace.document().doc, cube);
        let position = workspace.document().doc.entities[detail].transform.position;
        let world_offset = crate::viewport_math::quat_rotate(parent_rotation, position);
        for axis in 0..3 {
            assert!(
                (world_offset[axis] - delta[axis]).abs() < 1e-3,
                "world displacement should equal {delta:?}, actual {world_offset:?}"
            );
        }
        assert!(workspace.can_undo());
    }

    #[test]
    fn translate_entity_world_inverts_parent_scale_and_rotation() {
        let mut workspace = Workspace::sample();
        let parent = index_named(&workspace, "Cube");
        let child = index_named(&workspace, "Detail");
        workspace.document_mut().doc.entities[parent]
            .transform
            .scale = [2.0, 3.0, 4.0];
        workspace.document_mut().doc.entities[parent]
            .transform
            .rotation = crate::viewport_math::quat_axis_angle(1, std::f32::consts::FRAC_PI_2);
        let child_id = workspace.document().doc.entities[child].id.unwrap();
        let before = crate::viewport_math::world_position(&workspace.document().doc, child);
        let delta = [0.8, -1.2, 2.0];

        workspace.translate_entity_world(child_id, delta);

        let after = crate::viewport_math::world_position(&workspace.document().doc, child);
        for axis in 0..3 {
            assert!((after[axis] - before[axis] - delta[axis]).abs() < 1e-3);
        }
    }

    #[test]
    fn multi_select_translate_skips_descendants_of_selected_parents() {
        let mut workspace = Workspace::sample();
        let parent = index_named(&workspace, "Cube");
        let child = index_named(&workspace, "Detail");
        let parent_id = workspace.document().doc.entities[parent].id.unwrap();
        let child_id = workspace.document().doc.entities[child].id.unwrap();
        workspace.document_mut().doc.entities[child]
            .transform
            .position = [0.0, 0.0, 0.0];
        let child_before = world_position(&workspace.document().doc, child);

        workspace.translate_entities_world(&[parent_id, child_id], [1.0, 0.0, 0.0]);

        let child_after = world_position(&workspace.document().doc, child);
        let moved = child_after[0] - child_before[0];
        assert!(
            (moved - 1.0).abs() < 1e-4,
            "child should only follow parent moving 1 unit, actual {moved}"
        );
        assert_eq!(
            workspace.document().doc.entities[child].transform.position,
            [0.0, 0.0, 0.0],
            "child local position should not be changed separately"
        );
    }

    #[test]
    fn multi_select_scale_skips_descendants_of_selected_parents() {
        let mut workspace = Workspace::sample();
        let parent = index_named(&workspace, "Cube");
        let child = index_named(&workspace, "Detail");
        let parent_id = workspace.document().doc.entities[parent].id.unwrap();
        let child_id = workspace.document().doc.entities[child].id.unwrap();
        let parent_start = workspace.document().doc.entities[parent].transform.scale[0];
        let child_start = workspace.document().doc.entities[child].transform.scale[0];

        workspace.scale_entities_axis(
            &[(parent_id, parent_start), (child_id, child_start)],
            0,
            2.0,
        );

        let parent_after = workspace.document().doc.entities[parent].transform.scale[0];
        let child_after = workspace.document().doc.entities[child].transform.scale[0];
        assert!((parent_after - parent_start * 2.0).abs() < 1e-4);
        assert!(
            (child_after - child_start).abs() < 1e-4,
            "child own scale should not be multiplied again, actual {child_after}, start {child_start}"
        );
    }

    #[test]
    fn world_rotation_is_converted_through_parent_rotation() {
        let mut workspace = Workspace::sample();
        let parent = index_named(&workspace, "Cube");
        let child = index_named(&workspace, "Detail");
        workspace.document_mut().doc.entities[parent]
            .transform
            .rotation = crate::viewport_math::quat_axis_angle(1, std::f32::consts::FRAC_PI_2);
        let child_id = workspace.document().doc.entities[child].id.unwrap();
        let before = crate::viewport_math::world_rotation(&workspace.document().doc, child);
        let delta = crate::viewport_math::quat_axis_angle(0, std::f32::consts::FRAC_PI_2);

        workspace.rotate_entity(child_id, 0, std::f32::consts::FRAC_PI_2);

        let after = crate::viewport_math::world_rotation(&workspace.document().doc, child);
        let expected = crate::viewport_math::quat_mul(delta, before);
        let expected_x = crate::viewport_math::quat_rotate(expected, [1.0, 0.0, 0.0]);
        let actual_x = crate::viewport_math::quat_rotate(after, [1.0, 0.0, 0.0]);
        for axis in 0..3 {
            assert!((actual_x[axis] - expected_x[axis]).abs() < 1e-3);
        }
    }

    #[test]
    fn gizmo_center_switches_between_the_group_and_the_active_pivot() {
        let mut workspace = Workspace::sample();
        let cube = index_named(&workspace, "Cube");
        let light = index_named(&workspace, "Directional Light");
        workspace.selection = vec![cube, light];
        workspace.selection_active = Some(light);

        let center = workspace.selection_center_world().unwrap();
        let pivot = world_position(&workspace.document().doc, light);
        assert_eq!(workspace.gizmo_center_world(), Some(center));

        workspace.pivot_mode = PivotMode::Pivot;
        assert_eq!(workspace.gizmo_center_world(), Some(pivot));

        workspace.selection_active = None;
        assert_eq!(workspace.gizmo_center_world(), Some(pivot));
        workspace.selection.clear();
        assert_eq!(workspace.gizmo_center_world(), None);
    }

    #[test]
    fn pivot_mode_rotates_in_place_while_center_mode_orbits() {
        let mut workspace = Workspace::sample();
        let cube = index_named(&workspace, "Cube");
        let light = index_named(&workspace, "Directional Light");
        let cube_id = workspace.document().doc.entities[cube].id.unwrap();
        let light_id = workspace.document().doc.entities[light].id.unwrap();
        let ids = [cube_id, light_id];
        let before = [
            world_position(&workspace.document().doc, cube),
            world_position(&workspace.document().doc, light),
        ];

        workspace.pivot_mode = PivotMode::Pivot;
        workspace.rotate_entities(&ids, 1, std::f32::consts::FRAC_PI_2);
        for (index, expected) in [(cube, before[0]), (light, before[1])] {
            let actual = world_position(&workspace.document().doc, index);
            for axis in 0..3 {
                assert!(
                    (actual[axis] - expected[axis]).abs() < 1e-4,
                    "Pivot mode should not move entity: {actual:?} != {expected:?}"
                );
            }
        }
        assert_ne!(
            workspace.document().doc.entities[cube].transform.rotation,
            [0.0, 0.0, 0.0, 1.0]
        );
    }

    #[test]
    fn center_mode_scale_moves_objects_away_from_the_centre() {
        let mut workspace = Workspace::sample();
        let cube = index_named(&workspace, "Cube");
        let light = index_named(&workspace, "Directional Light");
        let cube_id = workspace.document().doc.entities[cube].id.unwrap();
        let light_id = workspace.document().doc.entities[light].id.unwrap();
        workspace.set_transform_vector(cube_id, "position", [-1.0, 0.0, 0.0]);
        workspace.set_transform_vector(light_id, "position", [1.0, 0.0, 0.0]);
        let targets = [(cube_id, 0usize, 1.0f32), (light_id, 0, 1.0)];

        workspace.scale_entities_gizmo(&targets, 0, 2.0);
        assert!((world_position(&workspace.document().doc, cube)[0] + 2.0).abs() < 1e-4);
        assert!((world_position(&workspace.document().doc, light)[0] - 2.0).abs() < 1e-4);
        assert!((workspace.document().doc.entities[cube].transform.scale[0] - 2.0).abs() < 1e-4);
        assert_eq!(
            workspace.document().undo.history().len(),
            3,
            "two set + one group scale"
        );

        workspace.undo();
        workspace.pivot_mode = PivotMode::Pivot;
        workspace.scale_entities_gizmo(&targets, 0, 2.0);
        assert!((world_position(&workspace.document().doc, cube)[0] + 1.0).abs() < 1e-4);
        assert!((world_position(&workspace.document().doc, light)[0] - 1.0).abs() < 1e-4);
        assert!((workspace.document().doc.entities[light].transform.scale[0] - 2.0).abs() < 1e-4);
    }

    #[test]
    fn batch_world_rotation_moves_entities_around_selection_center() {
        let mut workspace = Workspace::sample();
        let cube = index_named(&workspace, "Cube");
        let light = index_named(&workspace, "Directional Light");
        let cube_id = workspace.document().doc.entities[cube].id.unwrap();
        let light_id = workspace.document().doc.entities[light].id.unwrap();
        let cube_before = crate::viewport_math::world_position(&workspace.document().doc, cube);
        let light_before = crate::viewport_math::world_position(&workspace.document().doc, light);
        let center = [
            (cube_before[0] + light_before[0]) * 0.5,
            (cube_before[1] + light_before[1]) * 0.5,
            (cube_before[2] + light_before[2]) * 0.5,
        ];

        workspace.rotate_entities(&[cube_id, light_id], 1, std::f32::consts::FRAC_PI_2);

        for (index, before) in [(cube, cube_before), (light, light_before)] {
            let offset = [
                before[0] - center[0],
                before[1] - center[1],
                before[2] - center[2],
            ];
            let rotated = crate::viewport_math::quat_rotate(
                crate::viewport_math::quat_axis_angle(1, std::f32::consts::FRAC_PI_2),
                offset,
            );
            let expected = [
                center[0] + rotated[0],
                center[1] + rotated[1],
                center[2] + rotated[2],
            ];
            let actual = crate::viewport_math::world_position(&workspace.document().doc, index);
            for axis in 0..3 {
                assert!((actual[axis] - expected[axis]).abs() < 1e-3);
            }
        }
        assert_eq!(workspace.document().undo.history().len(), 1);
    }

    #[test]
    fn batch_translate_moves_all_entities_in_one_undo_step() {
        let mut workspace = Workspace::sample();
        let cube = index_named(&workspace, "Cube");
        let light = index_named(&workspace, "Directional Light");
        let cube_id = workspace.document().doc.entities[cube].id.unwrap();
        let light_id = workspace.document().doc.entities[light].id.unwrap();
        let cube_before = workspace.document().doc.entities[cube].transform.position;
        let light_before = workspace.document().doc.entities[light].transform.position;

        workspace.translate_entities_world(&[cube_id, light_id], [1.0, 0.0, 0.0]);
        workspace.translate_entities_world(&[cube_id, light_id], [1.0, 0.0, 0.0]);

        assert!(
            (workspace.document().doc.entities[cube].transform.position[0] - cube_before[0] - 2.0)
                .abs()
                < 1e-4
        );
        assert!(
            (workspace.document().doc.entities[light].transform.position[0]
                - light_before[0]
                - 2.0)
                .abs()
                < 1e-4
        );
        assert_eq!(workspace.document().undo.history().len(), 1);

        workspace.undo();
        assert_eq!(
            workspace.document().doc.entities[cube].transform.position,
            cube_before
        );
        assert_eq!(
            workspace.document().doc.entities[light].transform.position,
            light_before
        );
        assert!(!workspace.can_undo());
    }

    #[test]
    fn batch_transform_axis_sets_all_and_one_undo_step() {
        let mut workspace = Workspace::sample();
        let cube = index_named(&workspace, "Cube");
        let light = index_named(&workspace, "Directional Light");
        let cube_id = workspace.document().doc.entities[cube].id.unwrap();
        let light_id = workspace.document().doc.entities[light].id.unwrap();
        let cube_before = workspace.document().doc.entities[cube].transform.position[1];
        let light_before = workspace.document().doc.entities[light].transform.position[1];

        workspace.set_transform_axis_many(&[cube_id, light_id], "position", 1, 2.5);
        workspace.set_transform_axis_many(&[cube_id, light_id], "position", 1, 3.0);

        assert_eq!(
            workspace.document().doc.entities[cube].transform.position[1],
            3.0
        );
        assert_eq!(
            workspace.document().doc.entities[light].transform.position[1],
            3.0
        );
        assert_eq!(workspace.document().undo.history().len(), 1);

        workspace.undo();
        assert_eq!(
            workspace.document().doc.entities[cube].transform.position[1],
            cube_before
        );
        assert_eq!(
            workspace.document().doc.entities[light].transform.position[1],
            light_before
        );
    }

    #[test]
    fn batch_rotate_turns_every_selection_in_one_undo_step() {
        let mut workspace = Workspace::sample();
        let cube = index_named(&workspace, "Cube");
        let light = index_named(&workspace, "Directional Light");
        let cube_id = workspace.document().doc.entities[cube].id.unwrap();
        let light_id = workspace.document().doc.entities[light].id.unwrap();
        let cube_before = workspace.document().doc.entities[cube].transform.rotation;
        let light_before = workspace.document().doc.entities[light].transform.rotation;

        workspace.rotate_entities(&[cube_id, light_id], 1, 0.5);
        workspace.rotate_entities(&[cube_id, light_id], 1, 0.5);

        let cube_after = workspace.document().doc.entities[cube].transform.rotation;
        let light_after = workspace.document().doc.entities[light].transform.rotation;
        assert_ne!(cube_after, cube_before);
        assert_ne!(light_after, light_before);
        assert_eq!(workspace.document().undo.history().len(), 1);

        workspace.undo();
        let cube_restored = workspace.document().doc.entities[cube].transform.rotation;
        let light_restored = workspace.document().doc.entities[light].transform.rotation;
        for (restored, before) in [(cube_restored, cube_before), (light_restored, light_before)] {
            for (a, b) in restored.iter().zip(before) {
                assert!((a - b).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn history_entries_keep_undone_steps_and_jump_both_ways() {
        let mut workspace = Workspace::sample();
        let initial = workspace.document().doc.entities.len();
        workspace.add_entity("camera");
        workspace.add_entity("camera");
        workspace.add_entity("camera");
        assert_eq!(workspace.history_cursor(), 3);
        let entries = workspace.history_entries();
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().all(|(_, undone)| !undone));

        assert_eq!(workspace.jump_history(1), 2);
        assert_eq!(workspace.history_cursor(), 1);
        assert_eq!(workspace.document().doc.entities.len(), initial + 1);
        let entries = workspace.history_entries();
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries.iter().filter(|(_, undone)| *undone).count(),
            2,
            "undone steps must not disappear from history"
        );

        assert_eq!(workspace.jump_history(3), 2);
        assert_eq!(workspace.history_cursor(), 3);
        assert_eq!(workspace.document().doc.entities.len(), initial + 3);
        assert!(workspace
            .history_entries()
            .iter()
            .all(|(_, undone)| !undone));

        assert_eq!(workspace.jump_history(0), 3);
        assert_eq!(workspace.history_cursor(), 0);
        assert_eq!(workspace.document().doc.entities.len(), initial);
        assert_eq!(workspace.jump_history(0), 0, "no-op jump in place");
    }

    #[test]
    fn undo_history_can_jump_back_and_clear() {
        let mut workspace = Workspace::sample();
        let initial = workspace.document().doc.entities.len();
        workspace.add_entity("camera");
        workspace.add_entity("camera");
        workspace.add_entity("camera");
        assert_eq!(workspace.undo_history().len(), 3);
        assert_eq!(
            workspace.undo_history().first().map(String::as_str),
            Some("create entity")
        );

        assert_eq!(workspace.undo_to(1), 2, "undo back before step 2");
        assert_eq!(workspace.undo_history().len(), 1);
        assert_eq!(workspace.document().doc.entities.len(), initial + 1);

        assert_eq!(workspace.undo_to(0), 1);
        assert!(workspace.undo_history().is_empty());
        assert!(!workspace.can_undo());
        assert_eq!(workspace.undo_to(3), 0, "no-op when no history");

        workspace.add_entity("camera");
        assert!(workspace.can_undo());
        workspace.clear_history();
        assert!(!workspace.can_undo());
        assert!(!workspace.can_redo());
        assert!(workspace.undo_history().is_empty());
    }

    #[test]
    fn batch_scale_uses_each_start_value_and_one_undo_step() {
        let mut workspace = Workspace::sample();
        let cube = index_named(&workspace, "Cube");
        let light = index_named(&workspace, "Directional Light");
        let cube_id = workspace.document().doc.entities[cube].id.unwrap();
        let light_id = workspace.document().doc.entities[light].id.unwrap();
        let cube_before = workspace.document().doc.entities[cube].transform.scale;
        let light_before = workspace.document().doc.entities[light].transform.scale;

        let targets = [(cube_id, cube_before[0]), (light_id, light_before[0])];
        workspace.scale_entities_axis(&targets, 0, 2.0);
        workspace.scale_entities_axis(&targets, 0, 3.0);

        assert!(
            (workspace.document().doc.entities[cube].transform.scale[0] - cube_before[0] * 3.0)
                .abs()
                < 1e-4
        );
        assert!(
            (workspace.document().doc.entities[light].transform.scale[0] - light_before[0] * 3.0)
                .abs()
                < 1e-4
        );
        assert_eq!(workspace.document().undo.history().len(), 1);

        workspace.undo();
        assert_eq!(
            workspace.document().doc.entities[cube].transform.scale,
            cube_before
        );
        assert_eq!(
            workspace.document().doc.entities[light].transform.scale,
            light_before
        );
    }

    #[test]
    fn preview_rename_creates_one_undo_step_on_finish() {
        let mut workspace = fixture_workspace();
        let base = workspace.document().doc.clone();
        workspace.preview_rename(0, "C");
        workspace.preview_rename(0, "Ca");
        workspace.preview_rename(0, "Camera2");
        assert_eq!(
            workspace.document().doc.entities[0].display_name(),
            "Camera2"
        );
        assert!(!workspace.can_undo(), "preview stage produces no undo step");

        workspace.finish_rename(base, false);
        assert_eq!(workspace.document().undo.history().len(), 1);
        workspace.undo();
        assert_eq!(
            workspace.document().doc.entities[0].display_name(),
            "Camera"
        );
    }

    #[test]
    fn finish_rename_pushes_exactly_one_undo_step() {
        let mut workspace = fixture_workspace();
        let base = workspace.document().doc.clone();
        let was_dirty = workspace.document().dirty;
        assert!(!was_dirty);

        workspace.preview_rename(0, "F");
        workspace.preview_rename(0, "Floor");
        assert!(
            workspace.document().is_dirty(),
            "dirty mark during preview stage"
        );
        assert!(!workspace.can_undo(), "preview stage produces no undo step");

        workspace.finish_rename(base, was_dirty);
        assert_eq!(workspace.document().undo.history().len(), 1);
        assert!(workspace.document().is_dirty());
        workspace.undo();
        assert_eq!(
            workspace.document().doc.entities[0].display_name(),
            "Camera"
        );
    }

    #[test]
    fn finishing_an_unchanged_rename_leaves_the_document_clean() {
        let mut workspace = fixture_workspace();
        let base = workspace.document().doc.clone();

        workspace.preview_rename(0, "Floor");
        workspace.preview_rename(0, "Camera");
        workspace.finish_rename(base, false);

        assert!(!workspace.document().is_dirty());
        assert!(!workspace.can_undo());
    }

    #[test]
    fn cancel_rename_restores_the_previous_name_and_dirty_flag() {
        let mut workspace = fixture_workspace();
        let base = workspace.document().doc.clone();

        workspace.preview_rename(0, "Floor");
        assert!(workspace.document().is_dirty());
        workspace.cancel_rename(base, false);

        assert_eq!(
            workspace.document().doc.entities[0].display_name(),
            "Camera"
        );
        assert!(!workspace.document().is_dirty());
        assert!(!workspace.can_undo());
    }

    #[test]
    fn duplicate_selection_copies_subtree() {
        let mut workspace = fixture_workspace();

        workspace.select(0, false);
        workspace.duplicate_selection();
        assert_eq!(workspace.document().doc.entities.len(), 4);
        let root = workspace.selection[0];
        assert_eq!(
            workspace.document().doc.entities[root].display_name(),
            "Camera (1)"
        );
        let root_id = workspace.document().doc.entities[root].id.unwrap();
        assert!(workspace
            .document()
            .doc
            .entities
            .iter()
            .any(|entity| entity.parent == Some(root_id) && entity.display_name() == "Cube"));
        workspace.undo();
        assert_eq!(workspace.document().doc.entities.len(), 2);
    }

    #[test]
    fn selection_range_and_toggle_follow_anchor() {
        let mut workspace = fixture_workspace();
        workspace.select_range(1, 0);
        assert_eq!(
            workspace.selection,
            vec![0, 1],
            "reverse interval also normalized"
        );
        workspace.select(0, true);
        assert_eq!(workspace.selection, vec![1], "existing item toggled off");
        workspace.select(0, true);
        assert_eq!(workspace.selection, vec![0, 1]);
    }

    #[test]
    fn visible_range_selection_skips_rows_hidden_by_filters() {
        let mut workspace = Workspace::sample();

        let ordered = [0usize, 2, 3];
        workspace.select_visible_range(&ordered, 0, 3);
        assert_eq!(workspace.selection, vec![0, 2, 3]);
        assert_eq!(workspace.selection_active, Some(3));

        workspace.select_visible_range(&ordered, 3, 0);
        assert_eq!(
            workspace.selection,
            vec![0, 2, 3],
            "reverse selection also normalized"
        );

        workspace.select_visible_range(&ordered, 1, 2);
        assert_eq!(workspace.selection, vec![2]);
        assert_eq!(workspace.selection_active, Some(2));
    }

    #[test]
    fn select_many_overwrites_or_extends() {
        let mut workspace = fixture_workspace();
        workspace.select_many(&[1, 0], false);
        assert_eq!(
            workspace.selection,
            vec![0, 1],
            "sorted and covers old selection"
        );
        workspace.select_many(&[1, 2], true);
        assert_eq!(
            workspace.selection,
            vec![0, 1, 2],
            "additive selection deduplicated"
        );
        workspace.select_many(&[], false);
        assert!(workspace.selection.is_empty());
    }

    #[test]
    fn the_last_clicked_entity_stays_active_in_a_multi_selection() {
        let mut workspace = fixture_workspace();
        workspace.select_with(0, SelectMode::Replace);
        workspace.select_with(2, SelectMode::Add);
        workspace.select_with(1, SelectMode::Add);
        assert_eq!(
            workspace.selection,
            vec![0, 1, 2],
            "set still in document order"
        );
        assert_eq!(
            workspace.primary_selection(),
            Some(1),
            "active object is last clicked entity"
        );

        workspace.select_with(1, SelectMode::Remove);
        assert_eq!(
            workspace.primary_selection(),
            Some(2),
            "fall back to selection end after removing active object"
        );
    }

    #[test]
    fn right_clicking_a_selected_entity_makes_it_active_without_changing_the_set() {
        let mut workspace = fixture_workspace();
        workspace.select_many(&[0, 1, 2], false);
        assert_eq!(workspace.primary_selection(), Some(2));
        assert!(!workspace.select_for_context(0));
        assert_eq!(workspace.selection, vec![0, 1, 2]);
        assert_eq!(workspace.primary_selection(), Some(0));
    }

    #[test]
    fn shift_adds_and_ctrl_removes_in_the_viewport() {
        let mut workspace = fixture_workspace();
        workspace.select_with(0, SelectMode::Replace);
        workspace.select_with(1, SelectMode::Add);
        assert_eq!(
            workspace.selection,
            vec![0, 1],
            "Shift additive keeps original selection"
        );
        workspace.select_with(1, SelectMode::Add);
        assert_eq!(
            workspace.selection,
            vec![0, 1],
            "repeat additive does not switch off existing item"
        );
        workspace.select_with(0, SelectMode::Remove);
        assert_eq!(
            workspace.selection,
            vec![1],
            "Ctrl only removes, does not affect other items"
        );
        workspace.select_with(0, SelectMode::Remove);
        assert_eq!(
            workspace.selection,
            vec![1],
            "removing unselected item is no-op"
        );
        workspace.select_with(2, SelectMode::Replace);
        assert_eq!(workspace.selection, vec![2], "plain click replaces");
    }

    #[test]
    fn right_clicking_an_unselected_entity_selects_only_it() {
        let mut workspace = fixture_workspace();
        workspace.select_with(0, SelectMode::Replace);
        assert!(
            workspace.select_for_context(1),
            "not selected -> switch selection"
        );
        assert_eq!(workspace.selection, vec![1]);
        assert_eq!(workspace.primary_selection(), Some(1));
    }

    #[test]
    fn right_clicking_a_selected_entity_keeps_the_whole_selection() {
        let mut workspace = fixture_workspace();
        workspace.select_many(&[0, 1, 2], false);
        assert!(
            !workspace.select_for_context(1),
            "already selected -> do not change selection"
        );
        assert_eq!(workspace.selection, vec![0, 1, 2]);
    }

    #[test]
    fn right_clicking_a_stale_index_leaves_the_selection_alone() {
        let mut workspace = fixture_workspace();
        workspace.select_with(0, SelectMode::Replace);
        assert!(!workspace.select_for_context(999));
        assert_eq!(workspace.selection, vec![0]);
    }

    #[test]
    fn clicking_empty_space_only_clears_on_replace() {
        let mut workspace = fixture_workspace();
        workspace.select_many(&[0, 1], false);
        workspace.clear_selection_in(SelectMode::Add);
        assert_eq!(
            workspace.selection,
            vec![0, 1],
            "Shift+click blank keeps selection"
        );
        workspace.clear_selection_in(SelectMode::Remove);
        assert_eq!(
            workspace.selection,
            vec![0, 1],
            "Ctrl+click blank keeps selection"
        );
        workspace.clear_selection_in(SelectMode::Replace);
        assert!(workspace.selection.is_empty(), "plain click blank clears");
    }

    #[test]
    fn marquee_modes_add_remove_and_replace() {
        let mut workspace = fixture_workspace();
        workspace.select_many_with(&[0], SelectMode::Replace);
        workspace.select_many_with(&[1, 2], SelectMode::Add);
        assert_eq!(
            workspace.selection,
            vec![0, 1, 2],
            "Shift box-select additive"
        );
        workspace.select_many_with(&[0, 2], SelectMode::Remove);
        assert_eq!(
            workspace.selection,
            vec![1],
            "Ctrl box-select only subtracts hit items"
        );
        workspace.select_many_with(&[2], SelectMode::Replace);
        assert_eq!(workspace.selection, vec![2], "plain box-select overrides");
        workspace.select_many_with(&[], SelectMode::Add);
        assert_eq!(
            workspace.selection,
            vec![2],
            "empty box + Shift does not clear"
        );
        workspace.select_many_with(&[], SelectMode::Replace);
        assert!(
            workspace.selection.is_empty(),
            "empty box + plain click clears"
        );
    }

    #[test]
    fn select_mode_follows_the_modifiers() {
        assert_eq!(
            SelectMode::from_modifiers(false, false),
            SelectMode::Replace
        );
        assert_eq!(SelectMode::from_modifiers(true, false), SelectMode::Add);
        assert_eq!(SelectMode::from_modifiers(false, true), SelectMode::Remove);
        assert_eq!(SelectMode::from_modifiers(true, true), SelectMode::Remove);
    }

    #[test]
    fn selection_duplicate_delete_roundtrip() {
        let mut workspace = fixture_workspace();
        workspace.select(1, false);
        workspace.duplicate_selection();
        assert_eq!(workspace.document().doc.entities.len(), 3);
        assert_eq!(workspace.selection, vec![2]);

        workspace.delete_selection();
        assert_eq!(workspace.document().doc.entities.len(), 2);
        assert!(workspace.selection.is_empty());
    }

    #[test]
    fn field_edit_goes_through_undo() {
        let mut workspace = fixture_workspace();
        workspace.set_field(
            1,
            "prism_engine::scene::Camera",
            "fov_y_degrees",
            json!(75.0),
        );
        assert_eq!(
            workspace.document().doc.entities[0].components["prism_engine::scene::Camera"]
                ["fov_y_degrees"],
            json!(75.0)
        );
        assert!(workspace.document().is_dirty());
        workspace.undo();
        assert_eq!(
            workspace.document().doc.entities[0].components["prism_engine::scene::Camera"]
                ["fov_y_degrees"],
            json!(60.0)
        );
        assert!(
            !workspace.document().is_dirty(),
            "document should be clean after undo back to save point"
        );
    }

    #[test]
    fn saving_moves_the_dirty_baseline() {
        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.set_field(
            1,
            "prism_engine::scene::Camera",
            "fov_y_degrees",
            json!(75.0),
        );
        workspace.mark_saved();
        assert!(!workspace.document().is_dirty());

        workspace.undo();
        assert!(
            workspace.document().is_dirty(),
            "undo after save still counts as unsaved"
        );
        workspace.redo();
        assert!(
            !workspace.document().is_dirty(),
            "redo back to save point should be clean"
        );
    }

    #[test]
    fn add_and_remove_component() {
        let mut workspace = fixture_workspace();
        workspace.select(0, false);

        let available: Vec<&str> = workspace
            .component_types()
            .iter()
            .map(|component| component.name.as_str())
            .collect();
        assert!(!available.contains(&"prism_engine::scene::Camera"));

        workspace.select(1, false);
        assert!(workspace
            .component_types()
            .iter()
            .any(|component| component.name == "prism_engine::scene::Camera"));
        workspace.add_component("prism_engine::scene::Camera");
        assert!(workspace.document().doc.entities[1].has_component("prism_engine::scene::Camera"));
        workspace.remove_component("prism_engine::scene::Camera");
        assert!(!workspace.document().doc.entities[1].has_component("prism_engine::scene::Camera"));
    }

    #[test]
    fn add_component_multi_writes_every_entity_in_one_step() {
        let mut workspace = fixture_workspace();
        let path = "prism_engine::scene::Camera";

        assert!(workspace.document().doc.entities[0].has_component(path));
        let undo_before = workspace.undo_history().len();

        let added = workspace.add_component_multi(&[0, 1], path);

        assert_eq!(added, 1, "entities already having component skipped");
        assert!(workspace.document().doc.entities[1].has_component(path));
        assert_eq!(
            workspace.undo_history().len(),
            undo_before + 1,
            "batch add produces only one undo step"
        );
        workspace.undo();
        assert!(
            !workspace.document().doc.entities[1].has_component(path),
            "one undo rolls back whole batch"
        );
    }

    #[test]
    fn asset_details_reports_references_from_open_documents() {
        let mut workspace = fixture_workspace();
        let path = "assets/scenes/a.scene.json";
        workspace.document_mut().doc.entities[0].components.insert(
            "prism_engine::scene::MeshRenderer".to_string(),
            serde_json::json!({ "mesh_path": "../../models/cube.mesh" }),
        );

        let details = workspace.asset_details(path).expect("asset in index");
        assert_eq!(details.asset.path, path);
        assert_eq!(details.asset.kind, crate::assets::AssetKind::Scene);
        assert_eq!(details.asset.size_bytes, 10);
        assert!(!details.guid.is_empty());

        assert!(details.references.iter().all(|r| !r.document.is_empty()));
    }

    #[test]
    fn asset_details_for_an_unknown_path_is_none() {
        let workspace = fixture_workspace();
        assert!(workspace
            .asset_details("assets/does-not-exist.png")
            .is_none());
    }

    #[test]
    fn undoing_a_delete_restores_the_selection() {
        let mut workspace = fixture_workspace();
        let target = 1;
        let id = workspace.document().doc.entities[target]
            .id
            .expect("fixture has id");
        workspace.selection = vec![target];

        workspace.delete_selection();
        assert!(
            workspace.document().doc.index_of_id(id).is_none(),
            "entity gone after deletion"
        );
        assert!(workspace.selection.is_empty());

        workspace.undo();
        let restored = workspace
            .document()
            .doc
            .index_of_id(id)
            .expect("entity back after undo");
        assert_eq!(workspace.selection, vec![restored]);
        assert_eq!(workspace.selection_ids(), vec![id]);
    }

    #[test]
    fn redoing_a_delete_leaves_an_empty_selection() {
        let mut workspace = fixture_workspace();
        workspace.selection = vec![1];
        workspace.delete_selection();
        workspace.undo();
        assert_eq!(workspace.selection.len(), 1);

        workspace.redo();
        assert!(workspace.selection.is_empty());
        let len = workspace.document().doc.entities.len();
        assert!(workspace.selection.iter().all(|index| *index < len));
    }

    #[test]
    fn undoing_a_duplicate_returns_to_the_original_selection() {
        let mut workspace = fixture_workspace();
        let original = 1;
        let id = workspace.document().doc.entities[original]
            .id
            .expect("fixture has id");
        workspace.selection = vec![original];

        workspace.duplicate_selection();
        assert!(workspace.selection.len() >= 1);
        assert!(
            workspace.selection.iter().any(|index| *index != original),
            "new entity selected after copy"
        );

        workspace.undo();
        assert_eq!(workspace.selection_ids(), vec![id]);
        assert_eq!(workspace.selection, vec![original]);
    }

    #[test]
    fn restoring_a_selection_drops_entities_that_are_gone() {
        let mut workspace = fixture_workspace();
        let kept = workspace.document().doc.entities[0].id.unwrap();
        let selected = workspace.select_ids(&[kept, 9999]);
        assert_eq!(selected, 1);
        assert_eq!(workspace.selection, vec![0]);

        assert_eq!(workspace.select_ids(&[9999]), 0);
        assert!(workspace.selection.is_empty());
    }

    #[test]
    fn undoing_a_field_edit_keeps_the_selection() {
        let mut workspace = fixture_workspace();
        workspace.selection = vec![0];
        workspace.set_field(
            0,
            "prism_engine::scene::Camera",
            "fov_y_degrees",
            json!(75.0),
        );
        assert_eq!(workspace.selection, vec![0]);

        workspace.undo();
        assert_eq!(workspace.selection, vec![0]);
    }

    fn with_extra_components(workspace: &mut Workspace) {
        for name in ["zz_extra", "mm_extra"] {
            workspace.document_mut().doc.entities[0]
                .components
                .insert(name.to_string(), serde_json::json!({ "value": 0 }));
        }
    }

    #[test]
    fn components_can_be_reordered_and_the_move_is_one_undo_step() {
        let mut workspace = fixture_workspace();
        with_extra_components(&mut workspace);
        let entity = 0;
        let before = workspace.component_order(entity);
        let camera = "prism_engine::scene::Camera";
        let position = before
            .iter()
            .position(|name| name == camera)
            .expect("fixture 0th entity carries Camera");
        assert!(
            position + 1 < before.len(),
            "need components below to talk about move-up"
        );

        let undo_before = workspace.undo_history().len();
        assert!(workspace.move_component_at(entity, camera, 1));
        let mut expected = before.clone();
        expected.swap(position, position + 1);
        assert_eq!(workspace.component_order(entity), expected);
        assert_eq!(
            workspace.undo_history().len(),
            undo_before + 1,
            "move component produces only one undo step"
        );

        workspace.undo();
        assert_eq!(
            workspace.component_order(entity),
            before,
            "undo restores original order"
        );
    }

    #[test]
    fn moving_a_component_past_the_edges_is_a_no_op() {
        let mut workspace = fixture_workspace();
        with_extra_components(&mut workspace);
        let entity = 0;
        let order = workspace.component_order(entity);
        let first = order.first().cloned().expect("at least one component");
        let last = order.last().cloned().expect("at least one component");
        let undo_before = workspace.undo_history().len();

        assert!(!workspace.move_component_at(entity, &first, -1));
        assert!(!workspace.move_component_at(entity, &last, 1));
        assert!(!workspace.move_component_at(entity, "not-a-component", 1));
        assert!(!workspace.move_component_at(entity, "Transform", 1));

        assert_eq!(workspace.component_order(entity), order);
        assert_eq!(
            workspace.undo_history().len(),
            undo_before,
            "no move should not produce undo step"
        );
    }

    #[test]
    fn the_inspector_order_survives_a_save_and_reload() {
        let mut workspace = fixture_workspace();
        with_extra_components(&mut workspace);
        let entity = 0;
        let order = workspace.component_order(entity);
        let last = order.last().cloned().expect("at least one component");
        assert!(workspace.move_component_at(entity, &last, -1));

        let saved = serde_json::to_string(&workspace.document().doc).unwrap();
        let reloaded: crate::doc::SceneDoc = serde_json::from_str(&saved).unwrap();
        let reloaded_order: Vec<String> = reloaded.entities[entity].components.order();
        assert_eq!(reloaded_order, workspace.component_order(entity));
    }

    #[test]
    fn add_component_multi_ignores_duplicates_and_repeats() {
        let mut workspace = fixture_workspace();
        let path = "prism_engine::scene::Camera";
        let added = workspace.add_component_multi(&[1, 1, 1], path);
        assert_eq!(added, 1, "duplicate index counted once");

        let undo_before = workspace.undo_history().len();
        assert_eq!(workspace.add_component_multi(&[1], path), 0);
        assert_eq!(
            workspace.undo_history().len(),
            undo_before,
            "no undo step when all already have component"
        );
        assert_eq!(workspace.add_component_multi(&[], path), 0);
        assert_eq!(
            workspace.add_component_multi(&[999], path),
            0,
            "out-of-bounds index skipped"
        );
    }

    #[test]
    fn the_add_component_menu_lists_what_one_entity_is_missing() {
        let workspace = fixture_workspace();
        let path = "prism_engine::scene::Camera";
        let names = |workspace: &Workspace, indices: &[usize]| -> Vec<String> {
            workspace
                .component_types_for(indices)
                .into_iter()
                .map(|component| component.name.clone())
                .collect()
        };
        assert!(!names(&workspace, &[0]).contains(&path.to_string()));
        assert!(names(&workspace, &[1]).contains(&path.to_string()));
        assert!(
            names(&workspace, &[0, 1]).contains(&path.to_string()),
            "still listed when at least one selected item lacks component",
        );
        assert!(names(&workspace, &[]).is_empty());
    }

    #[test]
    fn add_component_does_not_reset_an_attached_component() {
        let mut workspace = fixture_workspace();
        workspace.select(0, false);
        let path = "prism_engine::scene::Camera";
        workspace.set_field(1, path, "fov_y_degrees", json!(30.0));
        let undo_before = workspace.undo_history().len();

        workspace.add_component(path);

        assert_eq!(
            workspace.document().doc.entities[0].components[path]["fov_y_degrees"],
            json!(30.0)
        );
        assert_eq!(workspace.undo_history().len(), undo_before);
    }

    #[test]
    fn maximize_toggles_without_touching_the_dock_tree() {
        let mut workspace = fixture_workspace();
        let before = workspace.dock.clone();

        workspace.toggle_maximized("inspector");
        assert_eq!(workspace.maximized_tab.as_deref(), Some("inspector"));
        assert_eq!(
            workspace.dock, before,
            "maximize must not rewrite the layout"
        );

        workspace.toggle_maximized("log");
        assert_eq!(workspace.maximized_tab.as_deref(), Some("log"));

        workspace.toggle_maximized("log");
        assert_eq!(workspace.maximized_tab, None);

        workspace.toggle_maximized("inspector");
        workspace.restore_maximized();
        assert_eq!(workspace.maximized_tab, None);
        workspace.toggle_maximized("inspector");
        workspace.set_layout_preset("2by3");
        assert_eq!(workspace.maximized_tab, None);
    }

    #[test]
    fn close_panel_drops_the_tab_and_clears_maximize() {
        let mut workspace = fixture_workspace();
        workspace.show_panel("problems");
        assert!(workspace.docked_tabs().iter().any(|tab| tab == "problems"));

        workspace.toggle_maximized("problems");
        workspace.close_panel("problems");

        assert!(!workspace.docked_tabs().iter().any(|tab| tab == "problems"));
        assert_eq!(workspace.maximized_tab, None);
        assert!(
            workspace.leaf_of("inspector").is_some(),
            "dock must survive"
        );

        workspace.close_other_panels("inspector");
        assert_eq!(workspace.docked_tabs(), vec!["inspector".to_string()]);
        workspace.close_panel("inspector");
        assert_eq!(
            workspace.docked_tabs(),
            vec!["inspector".to_string()],
            "closing the only remaining panel must be a no-op"
        );
    }

    #[test]
    fn close_other_panels_keeps_only_the_chosen_tab() {
        let mut workspace = fixture_workspace();
        workspace.float_panel("log", 10.0, 10.0);
        workspace.toggle_maximized("inspector");

        workspace.close_other_panels("inspector");

        assert_eq!(workspace.docked_tabs(), vec!["inspector".to_string()]);
        assert!(workspace.floating.is_empty());

        assert_eq!(workspace.maximized_tab.as_deref(), Some("inspector"));
    }

    #[test]
    fn add_tab_to_leaf_merges_instead_of_splitting() {
        let mut workspace = fixture_workspace();
        let leaf = workspace.leaf_of("inspector").unwrap();
        workspace.add_tab_to_leaf(&leaf, "shortcuts");

        let target = dock::find_leaf(&workspace.dock, "shortcuts").unwrap();
        assert_eq!(target.id(), leaf);
        assert_eq!(
            workspace
                .docked_tabs()
                .iter()
                .filter(|tab| *tab == "shortcuts")
                .count(),
            1
        );
    }

    #[test]
    fn panel_toggle_and_dock_operations_keep_single_instance() {
        let mut workspace = fixture_workspace();
        workspace.toggle_panel("log");
        assert_eq!(
            workspace
                .dock_tabs()
                .iter()
                .filter(|tab| *tab == "log")
                .count(),
            1
        );
        workspace.toggle_panel("log");
        assert_eq!(
            workspace
                .dock_tabs()
                .iter()
                .filter(|tab| *tab == "log")
                .count(),
            1
        );

        workspace.float_panel("log", 100.0, 120.0);
        assert_eq!(workspace.floating.len(), 1);
        assert!(!workspace.docked_tabs().iter().any(|tab| *tab == "log"));
        let inspector_leaf = workspace.leaf_of("inspector").unwrap();
        workspace.dock_tab("log", &inspector_leaf, DockZone::Center);
        assert!(workspace.docked_tabs().iter().any(|tab| *tab == "log"));
        assert!(workspace.floating.is_empty());
    }

    #[test]
    fn layout_presets_switch_and_clear_floating() {
        let mut workspace = fixture_workspace();
        workspace.float_panel("log", 10.0, 10.0);
        workspace.set_layout_preset("2by3");
        assert!(workspace.floating.is_empty());
        assert!(workspace.dock_tabs().iter().any(|tab| tab == "gameView"));
        assert_eq!(workspace.layout_preset, "2by3");
    }

    #[test]
    fn grid_toggle_flips_flag() {
        let mut workspace = Workspace::sample();
        assert!(workspace.show_grid);
        workspace.toggle_grid();
        assert!(!workspace.show_grid);
        workspace.toggle_grid();
        assert!(workspace.show_grid);
    }

    #[test]
    fn skybox_toggle_flips_flag_and_defaults_on() {
        let mut workspace = Workspace::sample();
        assert!(workspace.show_skybox, "Scene view draws skybox by default");
        workspace.toggle_skybox();
        assert!(!workspace.show_skybox);
        workspace.toggle_skybox();
        assert!(workspace.show_skybox);
    }

    #[test]
    fn play_state_machine() {
        let mut workspace = fixture_workspace();
        assert!(!workspace.playing);
        workspace.play();
        assert!(workspace.playing);
        workspace.tick(0.5);
        assert!((workspace.play_time_s - 0.5).abs() < 1e-6);
        workspace.pause();
        workspace.tick(0.5);
        assert!((workspace.play_time_s - 0.5).abs() < 1e-6);
        workspace.step();
        assert!(workspace.play_time_s > 0.5);
        workspace.stop();
        assert!(!workspace.playing);
        assert_eq!(workspace.play_time_s, 0.0);
    }

    #[test]
    fn stopping_play_discards_the_edits_made_while_playing() {
        let mut workspace = fixture_workspace();
        let index = 1;
        workspace.selection = vec![index];
        let id = workspace.document().doc.entities[index]
            .id
            .expect("fixture entities all have stable ids");
        let before = workspace.document().doc.entities[index].transform;
        let entities_before = workspace.document().doc.entities.len();
        let selection_before = workspace.selection.clone();

        workspace.play();
        assert!(workspace.has_play_snapshot());

        workspace.set_transform_axis(id, "position", 0, 99.0);
        workspace.selection = vec![index];
        workspace.delete_selection();
        assert!(workspace.document().doc.entities.len() < entities_before);
        assert!(workspace.document().is_dirty());

        workspace.stop();
        assert!(!workspace.playing);
        assert!(!workspace.has_play_snapshot());
        assert_eq!(workspace.document().doc.entities.len(), entities_before);
        let restored = &workspace.document().doc.entities[index];
        assert_eq!(
            restored.transform, before,
            "Transform should return to pre-Play value"
        );
        assert!(
            !workspace.document().is_dirty(),
            "runtime editing must not dirty document"
        );
        assert_eq!(workspace.selection, selection_before);
        assert!(
            !workspace.can_undo(),
            "undo stack must not mix runtime steps"
        );
    }

    #[test]
    fn stopping_play_only_restores_the_document_that_was_playing() {
        let mut workspace = Workspace::sample();
        let mut other = SceneDoc::default();
        other.add_named_child(None, "Other");
        workspace.docs.push(Document::new(
            DocKind::Scene,
            Some("other.scene.json".into()),
            other.clone(),
        ));
        let playing_index = workspace.active_index();

        workspace.play();
        workspace.set_active(1);
        workspace.docs[1]
            .doc
            .add_named_child(None, "Added while other doc is active");

        workspace.stop();
        assert_eq!(
            workspace.active_index(),
            playing_index,
            "Stop should return to played document"
        );
        assert_eq!(
            workspace.docs[1].doc.entities.len(),
            other.entities.len() + 1
        );
    }

    struct TempProject {
        root: std::path::PathBuf,
    }

    impl TempProject {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "prism-editor-core-ws-{}-{}",
                std::process::id(),
                tag
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("assets/scenes")).unwrap();
            std::fs::create_dir_all(root.join("assets/prefabs")).unwrap();
            Self { root }
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn load_project_accepts_transforms_with_missing_components() {
        let project = TempProject::new("partial-transform");
        std::fs::write(
            project.root.join("assets/scenes/main.scene.json"),
            r#"{"version":3,"entities":[{"id":1,"name":"A","transform":{"position":[1.0,2.0,3.0]}}]}"#,
        )
        .unwrap();

        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.load_project(project.root.clone()).unwrap();

        assert!(
            workspace.project_load_failures().is_empty(),
            "{:?}",
            workspace.project_load_failures()
        );
        let document = workspace
            .docs
            .iter()
            .find(|document| document.path.as_deref() == Some("assets/scenes/main.scene.json"))
            .expect("scene document");
        assert_eq!(
            document.doc.entities[0].transform.rotation,
            [0.0, 0.0, 0.0, 1.0]
        );
        assert_eq!(document.doc.entities[0].transform.scale, [1.0; 3]);
    }

    #[test]
    fn load_project_reports_documents_that_fail_to_parse() {
        let project = TempProject::new("broken-scene");
        std::fs::write(
            project.root.join("assets/scenes/broken.scene.json"),
            r#"{"version":3,"entities":42}"#,
        )
        .unwrap();

        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.load_project(project.root.clone()).unwrap();

        assert_eq!(
            workspace.project_load_failures(),
            ["assets/scenes/broken.scene.json"]
        );
    }

    /// A project whose documents all fail to parse leaves an empty scene, never
    /// the sample fixture: with a project bound the Game view must not show the
    /// editor's demo scene (#1020).
    #[test]
    fn a_project_where_every_document_fails_leaves_an_empty_scene() {
        let project = TempProject::new("all-broken");
        std::fs::write(
            project.root.join("assets/scenes/broken.scene.json"),
            r#"{"version":3,"entities":42}"#,
        )
        .unwrap();

        let mut workspace = Workspace::load_fixture(&fixture());
        workspace.load_project(project.root.clone()).unwrap();

        assert_eq!(
            workspace.project_load_failures(),
            ["assets/scenes/broken.scene.json"]
        );
        assert_eq!(workspace.docs.len(), 1);
        let document = workspace.document();
        assert_eq!(document.path, None, "the empty project scene has no file");
        assert!(
            document.doc.entities.is_empty(),
            "the sample fixture must not stand in for project content"
        );
        assert!(
            workspace
                .log
                .visible_lines()
                .iter()
                .any(|line| line.level == LogLevel::Error
                    && line.text.contains("all failed to load")),
            "loading failures have to show up as an error"
        );
    }

    #[test]
    fn asset_edit_undo_and_redo_write_the_previous_json_back_to_disk() {
        let project = TempProject::new("asset-undo");
        std::fs::create_dir_all(project.root.join("assets/hero.skinning")).unwrap();
        std::fs::write(
            project.root.join("assets/hero.skinning/notifies.json"),
            r#"{"clips": {}}"#,
        )
        .unwrap();
        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();

        let path = "assets/hero.skinning/notifies.json";
        let before = json!({ "clips": {} });
        let after = json!({ "clips": { "walk": [{ "time": 0.2, "name": "Step.L" }] } });
        workspace.write_asset_json(path, &after).unwrap();
        workspace.record_asset_edit(path, &before, &after);
        assert!(workspace.can_undo_asset_edit());
        assert!(!workspace.can_redo_asset_edit());

        assert_eq!(
            workspace.undo_asset_edit().unwrap(),
            Some(vec![path.to_string()])
        );
        let on_disk: Value =
            serde_json::from_str(&std::fs::read_to_string(project.root.join(path)).unwrap())
                .unwrap();
        assert_eq!(on_disk, before);
        assert!(workspace.can_redo_asset_edit());

        assert_eq!(
            workspace.redo_asset_edit().unwrap(),
            Some(vec![path.to_string()])
        );
        let on_disk: Value =
            serde_json::from_str(&std::fs::read_to_string(project.root.join(path)).unwrap())
                .unwrap();
        assert_eq!(on_disk, after);
    }

    #[test]
    fn renaming_a_graph_parameter_rewrites_blend_spaces_and_machine_conditions() {
        let project = TempProject::new("rename-param");
        std::fs::create_dir_all(project.root.join("assets/anim")).unwrap();
        std::fs::write(
            project.root.join("assets/scenes/main.scene.json"),
            r#"{"version":3,"entities":[]}"#,
        )
        .unwrap();
        std::fs::write(
            project.root.join("assets/anim/hero.animgraph.json"),
            r#"{
                "parameters": [{ "name": "speed", "type": "float", "default": 0.0 }],
                "nodes": [
                    { "name": "locomotion", "type": "blend_space_1d",
                      "clips": ["anim/a.ranim", "anim/b.ranim"], "positions": [0.0, 1.0],
                      "parameter": "speed" },
                    { "name": "sm", "type": "state_machine", "state_machine": "anim/hero.rasm" },
                    { "name": "out", "type": "output", "input": "locomotion" }
                ]
            }"#,
        )
        .unwrap();
        std::fs::write(
            project.root.join("assets/anim/hero.statemachine.json"),
            r#"{
                "states": [
                    { "name": "Idle", "clip": "anim/idle.ranim" },
                    { "name": "Run", "clip": "anim/run.ranim" }
                ],
                "transitions": [{
                    "from": "Idle", "to": "Run",
                    "conditions": [{ "type": "float_greater", "name": "speed", "threshold": 1.0 }]
                }]
            }"#,
        )
        .unwrap();

        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();
        let updated = workspace
            .rename_anim_graph_parameter("assets/anim/hero.animgraph.json", "speed", "velocity")
            .expect("renames across files");
        assert_eq!(
            updated,
            vec!["assets/anim/hero.statemachine.json".to_string()]
        );

        let graph: Value = serde_json::from_str(
            &std::fs::read_to_string(project.root.join("assets/anim/hero.animgraph.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(graph["parameters"][0]["name"], "velocity");
        assert_eq!(graph["nodes"][0]["parameter"], "velocity");
        let machine: Value = serde_json::from_str(
            &std::fs::read_to_string(project.root.join("assets/anim/hero.statemachine.json"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            machine["transitions"][0]["conditions"][0]["name"],
            "velocity"
        );

        assert_eq!(
            workspace
                .rename_anim_graph_parameter("assets/anim/hero.animgraph.json", "velocity", "speed")
                .unwrap(),
            vec!["assets/anim/hero.statemachine.json".to_string()],
            "renaming back to original also takes same path"
        );
        let undone = workspace.undo_asset_edit().unwrap().expect("one step");
        assert_eq!(
            undone,
            vec![
                "assets/anim/hero.statemachine.json".to_string(),
                "assets/anim/hero.animgraph.json".to_string(),
            ]
        );
        let graph: Value = serde_json::from_str(
            &std::fs::read_to_string(project.root.join("assets/anim/hero.animgraph.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(graph["parameters"][0]["name"], "velocity");
        let machine: Value = serde_json::from_str(
            &std::fs::read_to_string(project.root.join("assets/anim/hero.statemachine.json"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            machine["transitions"][0]["conditions"][0]["name"],
            "velocity"
        );
    }

    #[test]
    fn renaming_a_state_machine_state_rewrites_animator_references() {
        let project = TempProject::new("rename-state-refs");
        std::fs::create_dir_all(project.root.join("assets/anim")).unwrap();
        std::fs::write(
            project.root.join("assets/anim/hero.statemachine.json"),
            r#"{
                "states": [
                    { "name": "Idle", "clip": "anim/idle.ranim" },
                    { "name": "Run", "clip": "anim/run.ranim" }
                ],
                "transitions": [{ "from": "Idle", "to": "Run", "duration": 0.2 }]
            }"#,
        )
        .unwrap();
        std::fs::write(
            project.root.join("assets/scenes/main.scene.json"),
            r#"{
                "version": 3,
                "entities": [
                    { "id": 1, "name": "Hero", "components": {
                        "prism_engine::scene::Animator": {
                            "state_machine_path": "anim/hero.rasm", "state": "Idle"
                        } } },
                    { "id": 2, "name": "Other", "components": {
                        "prism_engine::scene::Animator": {
                            "state_machine_path": "anim/other.rasm", "state": "Idle"
                        } } }
                ]
            }"#,
        )
        .unwrap();
        std::fs::write(
            project.root.join("assets/prefabs/hero.prefab.json"),
            r#"{
                "version": 3,
                "entities": [
                    { "id": 1, "name": "HeroPrefab", "components": {
                        "prism_engine::scene::Animator": {
                            "state_machine_path": "anim/hero.rasm", "state": "Idle"
                        } } }
                ]
            }"#,
        )
        .unwrap();

        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();
        let prefab = workspace
            .docs
            .iter()
            .position(|document| {
                document.path.as_deref() == Some("assets/prefabs/hero.prefab.json")
            })
            .expect("prefab is open");
        workspace.close_document(prefab);

        let updated = workspace
            .rename_state_machine_state("assets/anim/hero.statemachine.json", 0, "Stand")
            .expect("renames state and references");
        assert_eq!(
            updated,
            vec![
                "assets/prefabs/hero.prefab.json".to_string(),
                "assets/scenes/main.scene.json".to_string(),
            ]
        );

        let scene = workspace
            .docs
            .iter()
            .find(|document| document.path.as_deref() == Some("assets/scenes/main.scene.json"))
            .expect("scene is open");
        assert!(scene.is_dirty(), "opened scene should be dirty");
        let hero = scene
            .doc
            .entities
            .iter()
            .find(|entity| entity.name.as_deref() == Some("Hero"))
            .expect("Hero entity");
        assert_eq!(
            hero.components["prism_engine::scene::Animator"]["state"],
            "Stand"
        );
        let other = scene
            .doc
            .entities
            .iter()
            .find(|entity| entity.name.as_deref() == Some("Other"))
            .expect("Other entity");
        assert_eq!(
            other.components["prism_engine::scene::Animator"]["state"], "Idle",
            "references to other state machines unaffected"
        );

        let prefab: Value = serde_json::from_str(
            &std::fs::read_to_string(project.root.join("assets/prefabs/hero.prefab.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            prefab["entities"][0]["components"]["prism_engine::scene::Animator"]["state"], "Stand",
            "unopened prefab written directly to disk"
        );

        let machine: Value = serde_json::from_str(
            &std::fs::read_to_string(project.root.join("assets/anim/hero.statemachine.json"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(machine["states"][0]["name"], "Stand");
        assert_eq!(machine["transitions"][0]["from"], "Stand");

        let undone = workspace.undo_asset_edit().unwrap().expect("one step");
        assert_eq!(
            undone,
            vec!["assets/anim/hero.statemachine.json".to_string()]
        );
        let machine: Value = serde_json::from_str(
            &std::fs::read_to_string(project.root.join("assets/anim/hero.statemachine.json"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(machine["states"][0]["name"], "Idle");
        assert_eq!(machine["transitions"][0]["from"], "Idle");
    }

    #[test]
    fn renaming_a_state_machine_state_rejects_bad_input_without_touching_documents() {
        let project = TempProject::new("rename-state-bad");
        std::fs::create_dir_all(project.root.join("assets/anim")).unwrap();
        std::fs::write(
            project.root.join("assets/anim/hero.statemachine.json"),
            r#"{
                "states": [
                    { "name": "Idle", "clip": "anim/idle.ranim" },
                    { "name": "Run", "clip": "anim/run.ranim" }
                ]
            }"#,
        )
        .unwrap();
        std::fs::write(
            project.root.join("assets/scenes/main.scene.json"),
            r#"{
                "version": 3,
                "entities": [{ "id": 1, "name": "Hero", "components": {
                    "prism_engine::scene::Animator": {
                        "state_machine_path": "anim/hero.rasm", "state": "Idle"
                    } } }]
            }"#,
        )
        .unwrap();
        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();

        assert!(workspace
            .rename_state_machine_state("assets/anim/hero.statemachine.json", 9, "Stand")
            .is_err());
        assert!(
            workspace
                .rename_state_machine_state("assets/anim/hero.statemachine.json", 0, "Run")
                .is_err(),
            "duplicate name"
        );
        assert!(
            workspace
                .rename_state_machine_state("assets/anim/hero.statemachine.json", 0, "  ")
                .is_err(),
            "empty name"
        );

        let scene = workspace
            .docs
            .iter()
            .find(|document| document.path.as_deref() == Some("assets/scenes/main.scene.json"))
            .expect("scene is open");
        assert!(!scene.is_dirty());
        assert_eq!(
            scene.doc.entities[0].components["prism_engine::scene::Animator"]["state"],
            "Idle"
        );
        let machine: Value = serde_json::from_str(
            &std::fs::read_to_string(project.root.join("assets/anim/hero.statemachine.json"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(machine["states"][0]["name"], "Idle");
    }

    #[test]
    fn saving_after_an_external_edit_is_refused_until_forced() {
        let project = TempProject::new("save-conflict");
        std::fs::write(
            project.root.join("assets/scenes/main.scene.json"),
            r#"{"version":3,"entities":[]}"#,
        )
        .unwrap();
        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();
        let index = workspace
            .docs
            .iter()
            .position(|doc| doc.path.as_deref() == Some("assets/scenes/main.scene.json"))
            .unwrap();

        workspace.docs[index].dirty = true;
        workspace.save_document(index).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(
            project.root.join("assets/scenes/main.scene.json"),
            r#"{"version":3,"entities":[{"id":1,"name":"Theirs","parent":null,"transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},"components":{}}]}"#,
        )
        .unwrap();

        assert!(workspace.check_external_changes() >= 1);
        assert!(workspace.save_conflict(index).is_some());
        assert!(workspace.save_document(index).is_err());

        workspace.docs[index].dirty = true;
        workspace.save_document_forced(index).unwrap();
        let text =
            std::fs::read_to_string(project.root.join("assets/scenes/main.scene.json")).unwrap();
        assert!(!text.contains("Theirs"), "{text}");
    }

    #[test]
    fn reloading_a_conflicted_document_takes_the_disk_version() {
        let project = TempProject::new("save-reload");
        std::fs::write(
            project.root.join("assets/scenes/main.scene.json"),
            r#"{"version":3,"entities":[]}"#,
        )
        .unwrap();
        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();
        let index = workspace
            .docs
            .iter()
            .position(|doc| doc.path.as_deref() == Some("assets/scenes/main.scene.json"))
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(
            project.root.join("assets/scenes/main.scene.json"),
            r#"{"version":3,"entities":[{"id":1,"name":"Theirs","parent":null,"transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},"components":{}}]}"#,
        )
        .unwrap();
        assert!(workspace.check_external_changes() >= 1);

        workspace.reload_document_from_disk(index).unwrap();
        let names: Vec<String> = workspace.docs[index]
            .doc
            .entities
            .iter()
            .map(|entity| entity.name.clone().unwrap_or_default())
            .collect();
        assert_eq!(names, vec!["Theirs".to_string()]);
        assert!(!workspace.docs[index].is_dirty());
        assert!(!workspace.docs[index].externally_modified);
        assert!(workspace.save_conflict(index).is_none());
    }

    #[test]
    fn a_missing_file_counts_as_an_external_change() {
        let project = TempProject::new("save-missing");
        std::fs::write(
            project.root.join("assets/scenes/main.scene.json"),
            r#"{"version":3,"entities":[]}"#,
        )
        .unwrap();
        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();
        let index = workspace
            .docs
            .iter()
            .position(|doc| doc.path.as_deref() == Some("assets/scenes/main.scene.json"))
            .unwrap();

        assert!(workspace.save_conflict(index).is_none());
        std::fs::remove_file(project.root.join("assets/scenes/main.scene.json")).unwrap();

        assert!(workspace.save_conflict(index).is_some());
    }

    #[test]
    fn project_scan_assets_populates_tree_and_picker() {
        let project = TempProject::new("scan-assets");
        std::fs::create_dir_all(project.root.join("assets/meshes")).unwrap();
        std::fs::write(project.root.join("assets/meshes/cube.fbx"), b"fbx").unwrap();
        std::fs::write(
            project.root.join("assets/scenes/a.scene.json"),
            r#"{"version":3,"entities":[]}"#,
        )
        .unwrap();
        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();
        assert!(workspace
            .assets
            .iter()
            .any(|asset| asset.path == "assets/meshes/cube.fbx" && asset.kind == AssetKind::Model));
        assert!(workspace
            .assets
            .iter()
            .any(|asset| asset.path == "assets/scenes/a.scene.json"
                && asset.kind == AssetKind::Scene));
        let meshes = workspace.asset_picker_candidates("mesh_path");
        assert_eq!(meshes.len(), 1);
        assert_eq!(meshes[0].path, "assets/meshes/cube.fbx");
        let expanded = workspace.expanded_assets.clone();
        assert!(workspace
            .tree
            .visible_rows(&expanded)
            .iter()
            .any(|(node, _)| node.name == "meshes"));
    }

    #[test]
    fn material_json_saves_and_reloads_through_the_workspace() {
        let project = TempProject::new("material-json");
        std::fs::create_dir_all(project.root.join("assets/materials")).unwrap();
        std::fs::write(
            project.root.join("assets/materials/wood.mat.json"),
            r#"{"type":"material","metallic":0.0,"base_color":[1,1,1,1]}"#,
        )
        .unwrap();
        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();

        let mut value = workspace
            .read_asset_json("assets/materials/wood.mat.json")
            .unwrap();
        value["metallic"] = serde_json::json!(0.75);
        workspace
            .write_asset_json("assets/materials/wood.mat.json", &value)
            .unwrap();

        let reread = workspace
            .read_asset_json("assets/materials/wood.mat.json")
            .unwrap();
        assert_eq!(reread["metallic"], serde_json::json!(0.75));

        assert!(workspace
            .assets
            .iter()
            .any(|asset| asset.path == "assets/materials/wood.mat.json"
                && asset.kind == crate::assets::AssetKind::Material));
    }

    #[test]
    fn asset_folders_copy_move_and_details() {
        let project = TempProject::new("asset-ops");
        std::fs::create_dir_all(project.root.join("assets/meshes")).unwrap();
        std::fs::write(project.root.join("assets/meshes/cube.fbx"), b"fbx").unwrap();
        std::fs::write(
            project.root.join("assets/scenes/a.scene.json"),
            r#"{"version":3,"entities":[{"id":1,"name":"Mesh","components":{"prism_engine::scene::MeshRenderer":{"mesh_path":"assets/meshes/cube.fbx"}}}]}"#,
        )
        .unwrap();
        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();

        workspace.create_asset_folder("assets/meshes/new").unwrap();
        assert!(project.root.join("assets/meshes/new").is_dir());
        assert!(workspace.tree.find("assets/meshes/new").is_some());

        let copy = workspace.duplicate_asset("assets/meshes/cube.fbx").unwrap();
        assert_eq!(copy, "assets/meshes/cube-2.fbx");
        assert!(project.root.join(&copy).is_file());

        let moved = workspace
            .move_asset_to_folder(&copy, Some("assets/meshes/new"))
            .unwrap();
        assert_eq!(moved, "assets/meshes/new/cube-2.fbx");
        assert!(project.root.join(&moved).is_file());
        assert!(workspace.assets.iter().any(|asset| asset.path == moved));

        let details = workspace.asset_details("assets/meshes/cube.fbx").unwrap();
        assert_eq!(details.guid.len(), 32);
        assert_eq!(details.references.len(), 1);
        assert_eq!(details.references[0].entity_name, "Mesh");
        assert!(details
            .references
            .iter()
            .any(|reference| reference.field.contains("mesh_path")));

        assert!(workspace
            .move_asset_to_folder("assets/scenes/a.scene.json", Some("assets"))
            .is_err());
    }

    #[test]
    fn moving_an_asset_rewrites_the_references_of_open_documents() {
        let project = TempProject::new("asset-move-references");
        std::fs::create_dir_all(project.root.join("assets/meshes")).unwrap();
        std::fs::write(project.root.join("assets/meshes/cube.fbx"), b"fbx").unwrap();
        std::fs::write(
            project.root.join("assets/scenes/a.scene.json"),
            r#"{"version":3,"entities":[{"id":1,"name":"Mesh","components":{"prism_engine::scene::MeshRenderer":{"mesh_path":"assets/meshes/cube.fbx"}}}]}"#,
        )
        .unwrap();
        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();
        workspace.create_asset_folder("assets/moved").unwrap();

        let moved = workspace
            .move_asset_to_folder("assets/meshes/cube.fbx", Some("assets/moved"))
            .unwrap();
        assert_eq!(moved, "assets/moved/cube.fbx");
        assert!(project.root.join(&moved).is_file());
        assert!(!project.root.join("assets/meshes/cube.fbx").exists());

        let document = workspace.document();
        let value = document
            .doc
            .entities
            .iter()
            .find(|entity| entity.name.as_deref() == Some("Mesh"))
            .and_then(|entity| entity.component("prism_engine::scene::MeshRenderer"))
            .and_then(|component| component.get("mesh_path"))
            .and_then(|value| value.as_str());
        assert_eq!(value, Some("assets/moved/cube.fbx"));
        assert!(document.is_dirty(), "document should be dirty if rewritten");
    }

    #[test]
    fn renaming_a_folder_rewrites_references_to_its_contents() {
        let project = TempProject::new("folder-rename");
        std::fs::create_dir_all(project.root.join("assets/tex")).unwrap();
        std::fs::write(project.root.join("assets/tex/wood.png"), b"png").unwrap();
        std::fs::write(
            project.root.join("assets/scenes/a.scene.json"),
            r#"{"version":3,"entities":[{"id":1,"name":"Floor","components":{"prism_engine::scene::SpriteRenderer":{"sprite_path":"assets/tex/wood.png"}}}]}"#,
        )
        .unwrap();
        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();

        let (references, affected) = workspace
            .rename_folder("assets/tex", "assets/wood")
            .unwrap();

        assert_eq!(references, 1);
        assert_eq!(affected.len(), 1);
        assert!(project.root.join("assets/wood/wood.png").is_file());
        assert!(!project.root.join("assets/tex").exists());
        let value = workspace
            .document()
            .doc
            .entities
            .iter()
            .find(|entity| entity.name.as_deref() == Some("Floor"))
            .and_then(|entity| entity.component("prism_engine::scene::SpriteRenderer"))
            .and_then(|component| component.get("sprite_path"))
            .and_then(|value| value.as_str());
        assert_eq!(value, Some("assets/wood/wood.png"));
        assert!(workspace.document().is_dirty());
        assert!(workspace
            .assets
            .iter()
            .any(|a| a.path == "assets/wood/wood.png"));
    }

    #[test]
    fn renaming_a_folder_rejects_its_own_subtree() {
        let project = TempProject::new("folder-rename-subtree");
        std::fs::create_dir_all(project.root.join("assets/tex/inner")).unwrap();
        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();

        assert!(workspace
            .rename_folder("assets/tex", "assets/tex/inner")
            .is_err());
        assert!(project.root.join("assets/tex/inner").is_dir());
    }

    #[test]
    fn deleting_a_folder_requires_it_to_be_empty() {
        let project = TempProject::new("folder-delete");
        std::fs::create_dir_all(project.root.join("assets/empty")).unwrap();
        std::fs::create_dir_all(project.root.join("assets/full")).unwrap();
        std::fs::write(project.root.join("assets/full/keep.png"), b"png").unwrap();
        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();

        let error = workspace.delete_folder("assets/full").unwrap_err();
        assert!(error.to_string().contains("non-empty"), "{error}");

        workspace.delete_folder("assets/empty").unwrap();
        assert!(!project.root.join("assets/empty").exists());
        assert!(workspace.tree.find("assets/empty").is_none());

        assert!(workspace.delete_folder("assets").is_err());
    }

    #[test]
    fn snap_ground_and_align_operations() {
        let mut workspace = Workspace::sample();

        let cube = 1;
        workspace.selection = vec![cube];
        workspace.document_mut().doc.entities[cube]
            .transform
            .position = [0.0, 3.0, 0.0];
        workspace.document_mut().doc.entities[cube].transform.scale = [2.0, 2.0, 2.0];
        assert_eq!(workspace.snap_selection_to_ground(), 1);
        let position = workspace.document().doc.entities[cube].transform.position;
        assert!(
            (position[1] - 1.0).abs() < 1e-3,
            "half-height 1 -> y=1, actual {position:?}"
        );
        assert!(workspace.can_undo());

        workspace.camera.target = [9.0, 9.0, 9.0];
        assert!(workspace.align_selection_to_view());
        let position = workspace.document().doc.entities[cube].transform.position;
        assert!(
            (position[0] - 9.0).abs() < 1e-3,
            "should translate to orbit center: {position:?}"
        );

        assert!(workspace.align_view_to_selection());
        assert!((workspace.camera.target[0] - 9.0).abs() < 1e-3);
        assert!((workspace.camera.target[1] - 9.0).abs() < 1e-3);

        workspace.look_along_axis(0, 1.0);
        assert!((workspace.camera.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-3);
    }

    #[test]
    fn project_scene_manifest_is_loaded_and_saved() {
        let project = TempProject::new("manifest-load");
        std::fs::write(
            project.root.join("assets/scenes.json"),
            r#"{"scenes": [
                {"name": "intro", "path": "scenes/intro.rscn", "enabled": true},
                {"name": "level", "path": "scenes/level.rscn", "enabled": false}
            ]}"#,
        )
        .unwrap();
        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();
        assert_eq!(workspace.scene_manifest.scenes.len(), 2);
        assert_eq!(workspace.scene_manifest.active().unwrap().name, "intro");

        workspace.set_scene_enabled(0, false);
        let reloaded = SceneManifest::read(&project.root).unwrap();
        assert!(!reloaded.scenes[0].enabled);
        assert!(
            reloaded.active().is_none(),
            "level was not enabled originally"
        );

        workspace.set_scene_enabled(1, true);
        let reloaded = SceneManifest::read(&project.root).unwrap();
        assert_eq!(reloaded.active().unwrap().name, "level");
    }

    #[test]
    fn manifest_add_move_remove_and_available_documents() {
        let project = TempProject::new("manifest-edit");
        let scene = r#"{"version":3,"entities":[]}"#;
        std::fs::write(project.root.join("assets/scenes/a.scene.json"), scene).unwrap();
        std::fs::write(project.root.join("assets/scenes/b.scene.json"), scene).unwrap();
        std::fs::write(
            project.root.join("assets/scenes.json"),
            r#"{"scenes": [{"name": "a", "path": "scenes/a.rscn"}]}"#,
        )
        .unwrap();
        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();

        assert_eq!(
            workspace.available_scene_documents(),
            vec!["scenes/b.scene.json".to_string()]
        );
        let index = workspace
            .add_scene_document("assets/scenes/b.scene.json")
            .expect("b should join");
        assert_eq!(index, 1);
        assert!(workspace.available_scene_documents().is_empty());
        assert!(workspace
            .add_scene_document("scenes/b.scene.json")
            .is_none());

        assert_eq!(workspace.move_scene_entry(1, -1), Some(0));
        assert_eq!(
            SceneManifest::read(&project.root).unwrap().scenes[0].name,
            "b"
        );
        assert!(workspace.remove_scene_entry(0));
        assert_eq!(SceneManifest::read(&project.root).unwrap().scenes.len(), 1);
        assert!(!workspace.remove_scene_entry(9));
    }

    #[test]
    fn delete_document_cleans_file_manifest_and_open_tab() {
        let project = TempProject::new("doc-delete");
        let scene = r#"{"version":3,"entities":[]}"#;
        std::fs::write(project.root.join("assets/scenes/a.scene.json"), scene).unwrap();
        std::fs::write(project.root.join("assets/scenes/b.scene.json"), scene).unwrap();
        std::fs::write(
            project.root.join("assets/scenes.json"),
            r#"{"scenes": [
                {"name": "a", "path": "scenes/a.rscn"},
                {"name": "b", "path": "scenes/b.rscn"}
            ]}"#,
        )
        .unwrap();
        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();
        assert_eq!(workspace.docs.len(), 2);
        assert_eq!(
            workspace.document().path.as_deref(),
            Some("assets/scenes/a.scene.json")
        );

        workspace
            .delete_document("assets/scenes/a.scene.json")
            .unwrap();
        assert!(!project.root.join("assets/scenes/a.scene.json").exists());
        assert_eq!(workspace.docs.len(), 1);
        assert_eq!(
            workspace.document().path.as_deref(),
            Some("assets/scenes/b.scene.json")
        );
        assert_eq!(workspace.project_files.len(), 1);
        let manifest = SceneManifest::read(&project.root).unwrap();
        assert_eq!(manifest.scenes.len(), 1);
        assert_eq!(manifest.scenes[0].name, "b");

        assert!(workspace
            .delete_document("assets/scenes/a.scene.json")
            .is_err());
        assert!(workspace.delete_document("assets/settings.ini").is_err());
    }

    #[test]
    fn save_document_can_target_an_inactive_tab() {
        let project = TempProject::new("save-inactive");
        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();
        workspace.new_document(DocKind::Scene, None);
        let index = workspace.docs.len() - 1;
        workspace.set_active(0);
        workspace.docs[index].dirty = true;

        let path = workspace.save_document(index).unwrap();
        assert_eq!(workspace.active, 0);
        assert_eq!(workspace.docs[index].path.as_deref(), Some(path.as_str()));
        assert!(!workspace.docs[index].is_dirty());
        assert!(project.root.join(&path).exists());
    }

    #[test]
    fn save_as_completes_the_suffix_and_rejects_traversal() {
        let project = TempProject::new("saveas");
        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();

        let saved = workspace.save_active_as("level2").unwrap();
        assert_eq!(saved, "assets/scenes/level2.scene.json");
        assert!(project.root.join(&saved).exists());
        assert!(workspace
            .project_files
            .iter()
            .any(|file| file.path == saved));

        let saved = workspace
            .save_active_as("assets/scenes/nested/level3.json")
            .unwrap();
        assert_eq!(saved, "assets/scenes/nested/level3.scene.json");
        assert!(project.root.join(&saved).exists());

        assert!(workspace.save_active_as("../evil").is_err());
        assert!(workspace.save_active_as("assets/../evil").is_err());
        assert!(workspace.save_active_as("/tmp/evil").is_err());
        assert!(!project.root.join("assets/evil.scene.json").exists());
        assert!(workspace.create_asset_folder("assets/../evil").is_err());
    }

    #[test]
    fn closing_documents_keeps_active_index_valid() {
        let mut workspace = fixture_workspace();
        workspace.new_document(DocKind::Scene, None);
        workspace.new_document(DocKind::Scene, None);
        assert!(workspace.docs.len() >= 3);
        workspace.set_active(2);

        workspace.close_document(0);
        assert_eq!(workspace.active, 1);
        workspace.close_document(1);
        assert!(workspace.active < workspace.docs.len());
        assert!(!workspace.docs.is_empty());
    }

    #[test]
    fn reloading_a_shrunken_document_clamps_selection() {
        let project = TempProject::new("reload-selection-clamp");
        let path = "assets/scenes/a.scene.json";
        let five = r#"{
            "version": 3,
            "entities": [
                {"id":1,"name":"A","parent":null,"transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},"components":{}},
                {"id":2,"name":"B","parent":null,"transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},"components":{}},
                {"id":3,"name":"C","parent":null,"transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},"components":{}},
                {"id":4,"name":"D","parent":null,"transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},"components":{}},
                {"id":5,"name":"E","parent":null,"transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},"components":{}}
            ]
        }"#;
        std::fs::write(project.root.join(path), five).unwrap();

        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();
        assert_eq!(workspace.document().doc.entities.len(), 5);
        workspace.selection = (0..5).collect();

        std::fs::write(
            project.root.join(path),
            r#"{
                "version": 3,
                "entities": [
                    {"id":1,"name":"A","parent":null,"transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},"components":{}},
                    {"id":2,"name":"B","parent":null,"transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},"components":{}}
                ]
            }"#,
        )
        .unwrap();

        workspace.reload_active().unwrap();
        assert_eq!(workspace.document().doc.entities.len(), 2);
        assert_eq!(workspace.selection, vec![0, 1]);
    }

    #[test]
    fn saving_a_scene_omits_default_and_read_only_component_fields() {
        let project = TempProject::new("prune-defaults");
        std::fs::write(
            project.root.join("assets/scenes/a.scene.json"),
            r#"{"version":3,"entities":[
                {"id":1,"name":"Camera","parent":null,
                 "transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{"prism_engine::scene::Camera":{"fov_y_degrees":60.0}}}
            ]}"#,
        )
        .unwrap();

        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();
        workspace.save_document(0).unwrap();

        let saved =
            std::fs::read_to_string(project.root.join("assets/scenes/a.scene.json")).unwrap();
        assert!(
            !saved.contains("fov_y_degrees"),
            "default value should not be written to authoring document: {saved}"
        );

        workspace.reload_active().unwrap();
        assert_eq!(
            workspace.document().doc.entities[0].components["prism_engine::scene::Camera"]
                ["fov_y_degrees"],
            json!(60.0),
            "default value must be filled back on read"
        );
    }

    #[test]
    fn project_roundtrip_save_and_reload() {
        let project = TempProject::new("roundtrip");
        let disk_doc = r#"{
            "version": 3,
            "entities": [
                {"id": 1, "name": "Disk Cube", "parent": null,
                 "transform": {"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components": {"prism_engine::scene::Camera": {}}}
            ]
        }"#;
        std::fs::write(project.root.join("assets/scenes/disk.scene.json"), disk_doc).unwrap();

        let mut workspace = fixture_workspace();
        let opened = workspace.load_project(project.root.clone()).unwrap();
        assert_eq!(opened, 1);
        assert_eq!(workspace.project_files.len(), 1);
        assert_eq!(workspace.document().title(), "disk.scene.json");

        assert_eq!(
            workspace.document().doc.entities[0].components["prism_engine::scene::Camera"]
                ["fov_y_degrees"],
            json!(60.0)
        );

        workspace.set_field(
            1,
            "prism_engine::scene::Camera",
            "fov_y_degrees",
            json!(42.0),
        );
        assert!(workspace.document().is_dirty());
        let path = workspace.save_active().unwrap();
        assert_eq!(path, "assets/scenes/disk.scene.json");
        assert!(!workspace.document().is_dirty());

        workspace.set_field(
            1,
            "prism_engine::scene::Camera",
            "fov_y_degrees",
            json!(90.0),
        );
        workspace.reload_active().unwrap();
        assert_eq!(
            workspace.document().doc.entities[0].components["prism_engine::scene::Camera"]
                ["fov_y_degrees"],
            json!(42.0)
        );

        let saved = workspace
            .save_active_as("assets/scenes/copy.scene.json")
            .unwrap();
        assert_eq!(saved, "assets/scenes/copy.scene.json");
        assert!(project.root.join(&saved).exists());
        assert!(workspace
            .project_files
            .iter()
            .any(|file| file.path == saved));
        assert!(workspace.save_active_as("   ").is_err());
        assert!(workspace.save_active_as("outside.scene.json").is_ok());
        assert!(project
            .root
            .join("assets/scenes/outside.scene.json")
            .exists());
    }

    #[test]
    fn instantiate_prefab_asset_from_disk_and_template() {
        let project = TempProject::new("instantiate");
        std::fs::write(
            project.root.join("assets/prefabs/box.prefab.json"),
            r#"{"version":3,"entities":[
                {"id":1,"name":"Box","parent":null,
                 "transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{}},
                {"id":2,"name":"Child","parent":1,
                 "transform":{"position":[0,1,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{}}
            ]}"#,
        )
        .unwrap();
        std::fs::write(
            project.root.join("assets/scenes/main.scene.json"),
            r#"{"version":3,"entities":[
                {"id":1,"name":"Root","parent":null,
                 "transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{}}
            ]}"#,
        )
        .unwrap();

        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();
        workspace.select(0, false);

        let inserted = workspace
            .instantiate_prefab_asset("assets/prefabs/box.prefab.json", Some(0))
            .unwrap();
        assert_eq!(inserted, 2);
        let doc = &workspace.document().doc;
        assert_eq!(doc.entities.len(), 3);
        let instance = &doc.entities[1];
        assert_eq!(instance.parent, Some(1), "attached under selected entity");
        assert_eq!(
            instance.prefab_link(),
            Some("assets/prefabs/box.prefab.json")
        );
        assert_eq!(workspace.selection, vec![1]);
        assert!(workspace.can_undo());

        let mut fixture_workspace = fixture_workspace();
        let count = fixture_workspace
            .instantiate_prefab_asset("assets/prefabs/camera.prefab.json", None)
            .unwrap();
        assert_eq!(count, 1);
        assert!(fixture_workspace
            .document()
            .doc
            .entities
            .iter()
            .any(|entity| entity.name.as_deref() == Some("Camera (1)")));

        assert!(workspace
            .instantiate_prefab_asset("assets/prefabs/nope.prefab.json", None)
            .is_err());
    }

    #[test]
    fn save_selection_as_prefab_writes_subtree_and_dedupes_names() {
        let project = TempProject::new("prefab-save");
        std::fs::write(
            project.root.join("assets/scenes/main.scene.json"),
            r#"{"version":3,"entities":[
                {"id":1,"name":"Root","parent":null,
                 "transform":{"position":[1,2,3],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{}},
                {"id":2,"name":"Child","parent":1,
                 "transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{}}
            ]}"#,
        )
        .unwrap();
        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();
        workspace.select(0, false);

        let path = workspace.save_selection_as_prefab("My Prefab").unwrap();
        assert_eq!(path, "assets/prefabs/My Prefab.prefab.json");
        let written = project::read_document(&project.root, &path).unwrap();
        assert_eq!(written.entities.len(), 2, "includes subtree");
        assert!(written.entities[0].parent.is_none(), "root has no parent");

        let second = workspace.save_selection_as_prefab("My Prefab").unwrap();
        assert_eq!(second, "assets/prefabs/My Prefab-2.prefab.json");
        assert!(workspace
            .project_files
            .iter()
            .any(|file| file.path == second));

        assert!(
            workspace.save_selection_as_prefab("   ").is_ok(),
            "empty name falls back to Untitled"
        );
        assert!(project
            .root
            .join("assets/prefabs/Untitled.prefab.json")
            .exists());
    }

    #[test]
    fn delete_asset_only_removes_regular_assets() {
        let project = TempProject::new("delete-asset");
        std::fs::create_dir_all(project.root.join("assets/textures")).unwrap();
        std::fs::write(project.root.join("assets/textures/a.png"), b"png").unwrap();
        std::fs::write(
            project.root.join("assets/scenes/main.scene.json"),
            r#"{"version":3,"entities":[]}"#,
        )
        .unwrap();

        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();

        assert!(workspace.delete_asset("outside.txt").is_err());
        assert!(workspace
            .delete_asset("assets/textures/missing.png")
            .is_err());
        assert!(workspace
            .delete_asset("assets/scenes/main.scene.json")
            .is_err());
        workspace.delete_asset("assets/textures/a.png").unwrap();
        assert!(!project.root.join("assets/textures/a.png").exists());
    }

    #[test]
    fn rename_asset_moves_file_and_rewrites_references() {
        let project = TempProject::new("rename");
        std::fs::create_dir_all(project.root.join("assets/textures")).unwrap();
        std::fs::write(project.root.join("assets/textures/a.png"), b"png").unwrap();
        std::fs::write(
            project.root.join("assets/scenes/main.scene.json"),
            r#"{"version":3,"entities":[
                {"id":1,"name":"Sprite","parent":null,
                 "transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{"prism_engine::scene::SpriteRenderer":{"sprite_path":"assets/textures/a.png"}}}
            ]}"#,
        )
        .unwrap();

        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();
        assert!(
            workspace
                .document()
                .doc
                .field_references("assets/textures/a.png")
                .len()
                == 1
        );

        let (references, affected) = workspace
            .rename_asset("assets/textures/a.png", "assets/textures/b.png")
            .unwrap();
        assert_eq!(references, 1);
        assert_eq!(affected.len(), 1);
        assert!(!project.root.join("assets/textures/a.png").exists());
        assert!(project.root.join("assets/textures/b.png").exists());
        assert_eq!(
            workspace
                .document()
                .doc
                .field_references("assets/textures/b.png")
                .len(),
            1
        );
        assert!(
            workspace.document().is_dirty(),
            "should be marked unsaved after reference rewrite"
        );

        assert!(workspace
            .rename_asset(
                "assets/scenes/main.scene.json",
                "assets/scenes/other.scene.json"
            )
            .is_err());

        assert!(workspace
            .rename_asset("assets/textures/b.png", "outside.png")
            .is_err());
    }

    /// #466: a GUID-backed reference is not rewritten and still resolves.
    #[test]
    fn rename_asset_with_guid_leaves_document_untouched() {
        let guid = "11111111111111111111111111111111";
        let project = TempProject::new("rename-guid");
        std::fs::create_dir_all(project.root.join("assets/textures")).unwrap();
        std::fs::write(project.root.join("assets/textures/a.png"), b"png").unwrap();
        std::fs::write(
            project.root.join("assets/textures/a.png.rmeta"),
            format!(r#"{{"guid":"{guid}","importer":"TextureImporter"}}"#),
        )
        .unwrap();
        let scene = r#"{"version":3,"entities":[
            {"id":1,"name":"Sprite","parent":null,
             "transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
             "components":{"prism_engine::scene::SpriteRenderer":{
                "sprite_path":"assets/textures/a.png","sprite_guid":"11111111111111111111111111111111"}}}
        ]}"#;
        let scene_path = project.root.join("assets/scenes/main.scene.json");
        std::fs::write(&scene_path, scene).unwrap();

        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();
        let assets_before = workspace.assets.clone();
        let (references, affected) = workspace
            .rename_asset("assets/textures/a.png", "assets/textures/b.png")
            .unwrap();
        println!("DEBUG assets_before={assets_before:?}");
        println!("DEBUG assets_after={:?}", workspace.assets);
        println!(
            "DEBUG components={:?}",
            workspace.document().doc.entities[0].components
        );
        assert_eq!(references, 0);
        assert!(affected.is_empty());
        assert!(
            !workspace.document().is_dirty(),
            "document should not be rewritten"
        );
        assert_eq!(std::fs::read_to_string(&scene_path).unwrap(), scene);
        assert!(project.root.join("assets/textures/b.png.rmeta").is_file());
        assert!(!project.root.join("assets/textures/a.png.rmeta").exists());
        assert!(workspace.broken_references().is_empty());

        let details = workspace.asset_details("assets/textures/b.png").unwrap();
        assert!(
            details
                .references
                .iter()
                .any(|reference| reference.field.ends_with("sprite_path")),
            "asset details must find the stale-path GUID reference"
        );
    }

    /// Broken-reference detection must not flag a live GUID with a stale path.
    #[test]
    fn missing_references_ignores_guid_backed_stale_paths() {
        let guid = "22222222222222222222222222222222";
        let mut workspace = Workspace::sample();
        workspace.assets = vec![AssetInfo {
            path: "assets/models/new.gltf".to_string(),
            size_bytes: 1,
            kind: AssetKind::Model,
            importer: "GltfImporter".to_string(),
            guid: Some(guid.to_string()),
        }];
        workspace.document_mut().doc.entities[0].components.insert(
            "prism_engine::scene::MeshRenderer".to_string(),
            json!({
                "mesh_path": "assets/models/old.gltf",
                "mesh_guid": guid
            }),
        );

        assert!(workspace.broken_references().is_empty());
        assert!(workspace.missing_references(0).is_empty());

        workspace.document_mut().doc.entities[0].components.insert(
            "prism_engine::scene::MeshRenderer".to_string(),
            json!({ "mesh_path": "assets/models/gone.gltf" }),
        );
        assert_eq!(workspace.broken_references().len(), 1);
    }

    #[test]
    fn missing_references_only_flags_broken_asset_fields() {
        let mut workspace = Workspace::sample();

        workspace.assets = vec![
            AssetInfo {
                path: "assets/meshes/ok.rmes".to_string(),
                size_bytes: 1,
                kind: AssetKind::Model,
                importer: String::new(),
                guid: None,
            },
            AssetInfo {
                path: "assets/textures/wood.png".to_string(),
                size_bytes: 1,
                kind: AssetKind::Texture,
                importer: String::new(),
                guid: None,
            },
        ];
        workspace.document_mut().doc.entities[0].components.insert(
            "prism_engine::scene::MeshRenderer".to_string(),
            json!({
                "mesh_path": "assets/meshes/ok.rmes",
                "materials": ["assets/textures/wood.png", "assets/textures/gone.png"],
            }),
        );

        workspace.document_mut().doc.entities[0].name = Some("assets/nope.png".to_string());

        let missing = workspace.missing_references(0);
        let values: Vec<&str> = missing.iter().map(|(_, value)| value.as_str()).collect();
        assert_eq!(values, vec!["assets/textures/gone.png"]);
        assert!(
            missing[0].0.contains("materials"),
            "field path should identify which cell: {:?}",
            missing[0]
        );
        assert!(workspace.missing_references(1).is_empty());
    }

    #[test]
    fn renaming_an_asset_updates_scenes_that_are_not_open() {
        let project = TempProject::new("rename-closed");
        std::fs::create_dir_all(project.root.join("assets/textures")).unwrap();
        std::fs::write(project.root.join("assets/textures/wood.png"), b"png").unwrap();
        let scene = |texture: &str| {
            format!(
                r#"{{"version":3,"entities":[{{"id":1,"name":"Sprite","parent":null,"transform":{{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]}},"components":{{"prism_engine::scene::SpriteRenderer":{{"sprite_path":"{texture}"}}}}}}]}}"#
            )
        };
        for name in ["a", "b"] {
            std::fs::write(
                project
                    .root
                    .join(format!("assets/scenes/{name}.scene.json")),
                scene("assets/textures/wood.png"),
            )
            .unwrap();
        }

        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();

        let index = workspace
            .docs
            .iter()
            .position(|doc| doc.path.as_deref() == Some("assets/scenes/b.scene.json"))
            .expect("b should be opened by load_project");
        workspace.close_document(index);
        assert!(!workspace
            .docs
            .iter()
            .any(|doc| doc.path.as_deref() == Some("assets/scenes/b.scene.json")));

        let (references, affected) = workspace
            .rename_asset("assets/textures/wood.png", "assets/textures/oak.png")
            .unwrap();

        assert_eq!(references, 2);
        assert_eq!(affected.len(), 2);

        let on_disk =
            std::fs::read_to_string(project.root.join("assets/scenes/b.scene.json")).unwrap();
        assert!(on_disk.contains("assets/textures/oak.png"), "{on_disk}");
        assert!(!on_disk.contains("wood.png"), "{on_disk}");

        let open = workspace
            .docs
            .iter()
            .find(|doc| doc.path.as_deref() == Some("assets/scenes/a.scene.json"))
            .expect("a still open");
        assert_eq!(
            open.doc.field_references("assets/textures/oak.png").len(),
            1
        );
        assert!(
            open.is_dirty(),
            "dirty mark after in-memory reference edits"
        );
    }

    fn prefab_project(name: &str) -> (TempProject, Workspace) {
        let project = TempProject::new(name);
        std::fs::create_dir_all(project.root.join("assets/prefabs")).unwrap();
        std::fs::write(
            project.root.join("assets/prefabs/box.prefab.json"),
            r#"{"version":3,"entities":[
                {"id":1,"name":"Box","parent":null,
                 "transform":{"position":[5,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{"prism_engine::scene::Camera":{"fov_y_degrees":33.0}}},
                {"id":2,"name":"Child","parent":1,
                 "transform":{"position":[0,1,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{}}
            ]}"#,
        )
        .unwrap();
        std::fs::write(
            project.root.join("assets/scenes/main.scene.json"),
            r#"{"version":3,"entities":[]}"#,
        )
        .unwrap();
        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();
        workspace
            .instantiate_prefab_asset("assets/prefabs/box.prefab.json", None)
            .unwrap();
        (project, workspace)
    }

    #[test]
    fn prefab_overrides_report_only_changed_fields() {
        let (_project, mut workspace) = prefab_project("overrides");
        let root = workspace.selection[0];
        assert!(
            workspace.prefab_overrides(root).unwrap().is_empty(),
            "no override right after instantiation"
        );

        workspace.document_mut().doc.entities[root]
            .transform
            .position = [1.0, 2.0, 3.0];
        workspace.document_mut().doc.entities[root].components["prism_engine::scene::Camera"]
            ["fov_y_degrees"] = serde_json::json!(60.0);

        let list = workspace.prefab_overrides(root).unwrap();
        let fields: Vec<String> = list
            .iter()
            .map(|entry| format!("{}.{}", entry.component, entry.field))
            .collect();
        assert_eq!(
            fields,
            [
                "transform.position".to_string(),
                "prism_engine::scene::Camera.fov_y_degrees".to_string()
            ]
        );
        assert_eq!(list[0].instance_value, serde_json::json!([1.0, 2.0, 3.0]));
        assert_eq!(list[0].prefab_value, serde_json::json!([5.0, 0.0, 0.0]));
    }

    #[test]
    fn prefab_overrides_survive_a_sibling_inserted_into_the_instance() {
        let (_project, mut workspace) = prefab_project("overrides-insert");
        let root = workspace.selection[0];

        let child = workspace.document().doc.children(root)[0];
        workspace
            .document_mut()
            .doc
            .add_named_child(Some(child), "Extra");
        workspace.document_mut().doc.entities[child]
            .transform
            .position = [0.0, 9.0, 0.0];

        let list = workspace.prefab_overrides(root).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].field, "position");
        assert_eq!(list[0].instance_value, serde_json::json!([0.0, 9.0, 0.0]));
    }

    #[test]
    fn reverting_a_prefab_override_restores_the_prefab_value() {
        let (_project, mut workspace) = prefab_project("overrides-revert");
        let root = workspace.selection[0];
        workspace.document_mut().doc.entities[root]
            .transform
            .position = [1.0, 2.0, 3.0];
        let entry = workspace.prefab_overrides(root).unwrap().remove(0);

        assert!(workspace.revert_prefab_override(&entry));
        assert_eq!(
            workspace.document().doc.entities[root].transform.position,
            [5.0, 0.0, 0.0]
        );
        assert!(workspace.prefab_overrides(root).unwrap().is_empty());
        workspace.undo();
        assert_eq!(
            workspace.undo_history().first().map(|entry| entry.as_str()),
            Some("instantiate prefab"),
            "rollback is a separate undo step (one undo returns to before instantiation)"
        );
        assert_eq!(
            workspace.document().doc.entities[root].transform.position,
            [1.0, 2.0, 3.0]
        );
    }

    #[test]
    fn applying_a_prefab_override_writes_the_prefab_file() {
        let (project, mut workspace) = prefab_project("overrides-apply");
        let root = workspace.selection[0];
        workspace.document_mut().doc.entities[root]
            .transform
            .position = [1.0, 2.0, 3.0];
        let entry = workspace.prefab_overrides(root).unwrap().remove(0);

        workspace.apply_prefab_override(root, &entry).unwrap();
        let prefab =
            crate::project::read_document(&project.root, "assets/prefabs/box.prefab.json").unwrap();
        assert_eq!(prefab.entities[0].transform.position, [1.0, 2.0, 3.0]);
        assert!(
            workspace.prefab_overrides(root).unwrap().is_empty(),
            "instance and source consistent after write-back"
        );
    }

    #[test]
    fn unpack_prefab_drops_every_link_in_the_subtree_in_one_step() {
        let (_project, mut workspace) = prefab_project("unpack");
        let root = workspace.selection[0];
        let before = workspace.document().doc.entities.len();
        assert_eq!(workspace.unpack_prefab(root), 1);
        assert!(workspace.document().doc.entities[root]
            .prefab_link()
            .is_none());
        assert_eq!(workspace.document().doc.entities.len(), before);
        workspace.undo();
        assert_eq!(
            workspace.document().doc.entities[root].prefab_link(),
            Some("assets/prefabs/box.prefab.json")
        );
    }

    fn breadcrumb_project(tag: &str) -> (TempProject, Workspace) {
        let project = TempProject::new(tag);
        std::fs::write(
            project.root.join("assets/prefabs/crate.prefab.json"),
            r#"{"version":3,"entities":[
                {"id":0,"name":"Crate","parent":null,
                 "transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{}}
            ]}"#,
        )
        .unwrap();
        std::fs::write(
            project.root.join("assets/scenes/level.scene.json"),
            r#"{"version":3,"entities":[
                {"id":7,"name":"Crate Instance","parent":null,
                 "transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{"prism_engine::scene::PrefabInstance":{"prefab":"assets/prefabs/crate.prefab.json"}}}
            ]}"#,
        )
        .unwrap();
        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();
        (project, workspace)
    }

    fn breadcrumb_scene_index(workspace: &Workspace) -> usize {
        workspace
            .docs
            .iter()
            .position(|doc| doc.path.as_deref() == Some("assets/scenes/level.scene.json"))
            .expect("level scene")
    }

    #[test]
    fn opening_a_prefab_from_an_instance_remembers_the_origin() {
        let (_project, mut workspace) = breadcrumb_project("prefab-origin");
        let scene = breadcrumb_scene_index(&workspace);
        workspace.set_active(scene);

        workspace.open_prefab_from_instance(scene, 7).unwrap();
        assert_eq!(
            workspace.document().path.as_deref(),
            Some("assets/prefabs/crate.prefab.json")
        );
        assert_eq!(
            workspace.document().prefab_origin,
            Some(PrefabOrigin {
                scene_index: scene,
                entity_id: 7,
                scene_title: "level.scene.json".to_string(),
            })
        );

        workspace.set_active(scene);
        assert!(workspace.document().prefab_origin.is_none());
        assert!(workspace.open_prefab_from_instance(scene, 999).is_err());
    }

    #[test]
    fn returning_from_a_clean_prefab_reinstantiates_and_selects_the_instance() {
        let (_project, mut workspace) = breadcrumb_project("prefab-return-clean");
        let scene = breadcrumb_scene_index(&workspace);
        workspace.set_active(scene);
        workspace.open_prefab_from_instance(scene, 7).unwrap();

        workspace.document_mut().doc.entities[0].transform.position = [5.0, 1.0, 0.0];
        workspace.save_active().unwrap();
        let prefab = workspace.active_index();
        assert!(workspace.return_from_prefab(prefab));

        assert_eq!(
            workspace.document().path.as_deref(),
            Some("assets/scenes/level.scene.json")
        );
        let selected = workspace
            .primary_selection()
            .expect("original instance selected after return");
        let entity = &workspace.document().doc.entities[selected];
        assert_eq!(
            entity.prefab_link(),
            Some("assets/prefabs/crate.prefab.json")
        );
        assert_eq!(
            entity.transform.position,
            [5.0, 1.0, 0.0],
            "clean prefab return should sync content back to instance"
        );

        assert_ne!(entity.id, Some(7));
    }

    #[test]
    fn returning_from_a_dirty_prefab_keeps_the_instance_untouched() {
        let (_project, mut workspace) = breadcrumb_project("prefab-return-dirty");
        let scene = breadcrumb_scene_index(&workspace);
        workspace.set_active(scene);
        workspace.open_prefab_from_instance(scene, 7).unwrap();

        workspace.document_mut().doc.entities[0].transform.position = [9.0, 0.0, 0.0];
        workspace.document_mut().dirty = true;
        let prefab = workspace.active_index();
        assert!(workspace.return_from_prefab(prefab));

        assert_eq!(
            workspace.document().path.as_deref(),
            Some("assets/scenes/level.scene.json")
        );
        let selected = workspace
            .primary_selection()
            .expect("still selects instance");
        let entity = &workspace.document().doc.entities[selected];
        assert_eq!(
            entity.id,
            Some(7),
            "dirty prefab not re-instantiated, entity id unchanged"
        );
        assert_ne!(entity.transform.position, [9.0, 0.0, 0.0]);
    }

    #[test]
    fn returning_without_an_origin_is_a_noop() {
        let (_project, mut workspace) = breadcrumb_project("prefab-return-none");
        let scene = breadcrumb_scene_index(&workspace);
        workspace.set_active(scene);
        assert!(!workspace.return_from_prefab(scene));
        assert_eq!(workspace.active_index(), scene);
    }

    #[test]
    fn revert_prefab_instance_from_disk_keeps_name_and_parent() {
        let project = TempProject::new("revert");
        std::fs::write(
            project.root.join("assets/prefabs/box.prefab.json"),
            r#"{"version":3,"entities":[
                {"id":1,"name":"Box","parent":null,
                 "transform":{"position":[5,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{"prism_engine::scene::Camera":{"fov_y_degrees":33.0}}},
                {"id":2,"name":"Child","parent":1,
                 "transform":{"position":[0,1,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{}}
            ]}"#,
        )
        .unwrap();
        std::fs::write(
            project.root.join("assets/scenes/main.scene.json"),
            r#"{"version":3,"entities":[
                {"id":1,"name":"Parent","parent":null,
                 "transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{}},
                {"id":2,"name":"My Box","parent":1,
                 "transform":{"position":[1,2,3],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{"prism_engine::scene::PrefabInstance":{"prefab":"assets/prefabs/box.prefab.json"}}}
            ]}"#,
        )
        .unwrap();

        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();
        assert_eq!(workspace.document().doc.entities.len(), 2);

        workspace.revert_prefab_instance(2).unwrap();
        let doc = &workspace.document().doc;
        assert_eq!(doc.entities.len(), 3, "instance root + prefab children");
        let root = doc
            .entities
            .iter()
            .find(|entity| entity.name.as_deref() == Some("My Box"))
            .expect("instance name kept");
        assert_eq!(root.parent, Some(1), "parent kept");
        assert_eq!(
            root.transform.position,
            [5.0, 0.0, 0.0],
            "use prefab current content"
        );
        assert_eq!(
            root.prefab_link(),
            Some("assets/prefabs/box.prefab.json"),
            "link kept"
        );
        assert!(doc
            .entities
            .iter()
            .any(|entity| entity.display_name() == "Child"));

        workspace.undo();
        assert_eq!(workspace.document().doc.entities.len(), 2);
        assert_eq!(
            workspace.document().doc.entities[1].transform.position,
            [1.0, 2.0, 3.0]
        );
    }

    #[test]
    fn apply_prefab_instance_writes_subtree_and_strips_root_link() {
        let project = TempProject::new("apply-prefab");
        std::fs::write(
            project.root.join("assets/prefabs/box.prefab.json"),
            r#"{"version":3,"entities":[
                {"id":1,"name":"Box","parent":null,
                 "transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{}},
                {"id":2,"name":"Child","parent":1,
                 "transform":{"position":[0,1,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{}}
            ]}"#,
        )
        .unwrap();
        std::fs::write(
            project.root.join("assets/scenes/main.scene.json"),
            r#"{"version":3,"entities":[
                {"id":1,"name":"Root","parent":null,
                 "transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{}},
                {"id":2,"name":"My Box","parent":1,
                 "transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
                 "components":{"prism_engine::scene::PrefabInstance":{"prefab":"assets/prefabs/box.prefab.json"}}}
            ]}"#,
        )
        .unwrap();

        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();
        workspace.revert_prefab_instance(2).unwrap();
        let root_id = workspace
            .document()
            .doc
            .entities
            .iter()
            .find(|entity| entity.display_name() == "My Box")
            .and_then(|entity| entity.id)
            .expect("instance root");
        {
            let doc = &mut workspace.document_mut().doc;
            let child = doc
                .entities
                .iter_mut()
                .find(|entity| entity.display_name() == "Child")
                .expect("instance child");
            child.transform.position = [7.0, 0.0, 0.0];
            child.name = Some("Renamed Child".to_string());
        }

        let path = workspace.apply_prefab_instance(root_id).unwrap();
        assert_eq!(path, "assets/prefabs/box.prefab.json");
        let prefab = project::read_document(&project.root, &path).unwrap();
        assert_eq!(prefab.entities.len(), 2);
        assert!(
            prefab
                .entities
                .iter()
                .all(|entity| entity.prefab_link().is_none()),
            "prefab root should not carry instance link"
        );
        let child = prefab
            .entities
            .iter()
            .find(|entity| entity.display_name() == "Renamed Child")
            .expect("modification written back to prefab");
        assert_eq!(child.transform.position, [7.0, 0.0, 0.0]);
        assert!(
            workspace.apply_prefab_instance(1).is_err(),
            "non-instance should error"
        );
    }

    #[test]
    fn prefab_falls_back_to_template_in_fixture_mode() {
        let mut workspace = fixture_workspace();

        workspace.document_mut().doc.entities[1].components.insert(
            PREFAB_LINK_COMPONENT.to_string(),
            serde_json::json!({"prefab": "assets/prefabs/camera.prefab.json"}),
        );
        let doc = workspace.prefab_document_for(2).unwrap();
        assert_eq!(doc.entities.len(), 1);
        assert_eq!(doc.entities[0].display_name(), "Camera");

        workspace.revert_prefab_instance(2).unwrap();
        let instance = workspace
            .document()
            .doc
            .entities
            .iter()
            .find(|entity| entity.name.as_deref() == Some("Cube"))
            .expect("instance root");
        assert_eq!(
            instance.prefab_link(),
            Some("assets/prefabs/camera.prefab.json")
        );

        assert!(workspace.revert_prefab_instance(1).is_err());
    }

    #[test]
    fn saving_without_project_fails_cleanly() {
        let mut workspace = fixture_workspace();
        assert!(!workspace.has_project());
        assert!(workspace.save_active().is_err());
        assert!(workspace.reload_active().is_err());
    }

    #[test]
    fn hydration_fills_missing_component_fields() {
        let mut fixture = fixture();

        fixture["docs"]["assets/scenes/a.scene.json"]["entities"][0]["components"]
            ["prism_engine::scene::Camera"] = json!({});
        let mut workspace = Workspace::load_fixture(&fixture);
        workspace.set_schema(fixture_schema(&fixture));
        assert_eq!(
            workspace.document().doc.entities[0].components["prism_engine::scene::Camera"]
                ["fov_y_degrees"],
            json!(60.0)
        );
        workspace.set_field(
            1,
            "prism_engine::scene::Camera",
            "fov_y_degrees",
            json!(80.0),
        );
        assert_eq!(
            workspace.document().doc.entities[0].components["prism_engine::scene::Camera"]
                ["fov_y_degrees"],
            json!(80.0)
        );
    }

    #[test]
    fn unlink_prefab_removes_link_only_once() {
        let mut workspace = fixture_workspace();
        workspace.document_mut().doc.entities[1].components.insert(
            PREFAB_LINK_COMPONENT.to_string(),
            serde_json::json!({ "prefab": "assets/prefabs/camera.prefab.json" }),
        );
        assert!(workspace.unlink_prefab(1));
        assert!(!workspace.document().doc.entities[1].has_component(PREFAB_LINK_COMPONENT));
        assert!(!workspace.unlink_prefab(1), "returns false when no link");
    }

    #[test]
    fn asset_path_fields_are_detected_and_filtered() {
        use crate::assets::{is_asset_path_field, AssetKind};
        assert!(is_asset_path_field(
            "prism_engine::scene::MeshRenderer",
            "mesh_path"
        ));
        assert!(is_asset_path_field(
            "prism_engine::scene::Skybox",
            "hdr_path"
        ));
        assert!(is_asset_path_field(
            "prism_engine::scene::EnvironmentLighting",
            "env_map"
        ));
        assert!(is_asset_path_field(
            "prism_engine::scene::PrefabInstance",
            "prefab"
        ));
        assert!(!is_asset_path_field("prism_engine::scene::Camera", "name"));
        assert!(!is_asset_path_field(
            "prism_engine::scene::Camera",
            "fov_y_degrees"
        ));

        let mut workspace = Workspace::sample();
        use crate::assets::AssetInfo;
        workspace.assets = vec![
            AssetInfo {
                path: "assets/meshes/cube.rmesh".into(),
                size_bytes: 1,
                kind: AssetKind::Model,
                importer: "ModelImporter".into(),
                guid: None,
            },
            AssetInfo {
                path: "assets/textures/grid.png".into(),
                size_bytes: 1,
                kind: AssetKind::Texture,
                importer: "TextureImporter".into(),
                guid: None,
            },
        ];
        let meshes = workspace.asset_picker_candidates("mesh_path");
        assert_eq!(meshes.len(), 1);
        assert_eq!(meshes[0].path, "assets/meshes/cube.rmesh");
        let textures = workspace.asset_picker_candidates("sprite_path");
        assert_eq!(textures.len(), 1);
        assert_eq!(textures[0].kind, AssetKind::Texture);
        let all = workspace.asset_picker_candidates("some_unknown_field");
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn theme_and_locale_switch() {
        let mut workspace = fixture_workspace();
        assert_eq!(workspace.t("common.save"), "Save");
        workspace.set_locale(Locale::EnUs);
        assert_eq!(workspace.t("common.save"), "Save");
        workspace.set_theme_preset("paper");
        assert_eq!(workspace.color(ThemeToken::Bg).to_hex(), "#f2f4f7");
    }

    #[test]
    fn framing_the_selected_entity_centres_the_camera_on_it() {
        let mut workspace = Workspace::sample();

        workspace.select(1, false);
        workspace.camera.target = [99.0, 99.0, 99.0];

        workspace.frame_selection();

        assert!((workspace.camera.target[0]).abs() < 1e-4);
        assert!((workspace.camera.target[1] - 0.5).abs() < 1e-4);
        assert!((workspace.camera.target[2]).abs() < 1e-4);
        assert!(workspace.camera.distance > 0.0);
    }

    #[test]
    fn frame_selection_uses_the_injected_object_bounds() {
        let mut workspace = Workspace::load_fixture(&scene_fixture(json!([
            { "id": 1, "name": "Boat", "parent": null,
              "transform": {"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
              "components": {} }
        ])));
        workspace.select(0, false);
        workspace.frame_selection();
        let unit_distance = workspace.camera.distance;

        workspace.set_object_bounds(vec![Some(PickBounds {
            min: [-32.0, -8.0, -0.05],
            max: [32.0, 8.0, 0.05],
        })]);
        workspace.frame_selection();
        let sprite_distance = workspace.camera.distance;

        assert!(
            sprite_distance > unit_distance * 10.0,
            "64-unit-wide sprite must frame much farther than unit cube ({unit_distance} -> {sprite_distance})"
        );
    }

    #[test]
    fn frame_all_moves_camera_target() {
        let mut workspace = fixture_workspace();
        workspace.frame_all();
        assert!(workspace.camera.distance > 0.5);
        workspace.select(0, false);
        workspace.frame_selection();
        assert_eq!(workspace.camera.target[1], 1.0);
    }

    fn scene_fixture(entities: serde_json::Value) -> Value {
        let mut value = fixture();
        value["docs"]["assets/scenes/a.scene.json"]["entities"] = entities;
        value
    }

    #[test]
    fn framing_a_child_uses_its_world_position() {
        let mut workspace = Workspace::load_fixture(&scene_fixture(json!([
            { "id": 1, "name": "Root", "parent": null,
              "transform": {"position":[100,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
              "components": {} },
            { "id": 2, "name": "Child", "parent": 1,
              "transform": {"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
              "components": {} }
        ])));
        workspace.select(1, false);
        workspace.frame_selection();
        assert!(
            (workspace.camera.target[0] - 100.0).abs() < 1e-4,
            "child should frame at world x=100, actual {:?}",
            workspace.camera.target
        );
    }

    #[test]
    fn framing_a_large_object_pulls_the_camera_back() {
        let mut workspace = Workspace::load_fixture(&scene_fixture(json!([
            { "id": 1, "name": "Wall", "parent": null,
              "transform": {"position":[0,0,0],"rotation":[0,0,0,1],"scale":[100,1,1]},
              "components": {} }
        ])));
        workspace.select(0, false);
        workspace.frame_selection();
        assert!(
            workspace.camera.distance > 100.0,
            "large object must frame back to contain it, actual {}",
            workspace.camera.distance
        );
    }

    #[test]
    fn framing_a_tiny_object_keeps_a_workable_distance() {
        let mut workspace = Workspace::load_fixture(&scene_fixture(json!([
            { "id": 1, "name": "Dot", "parent": null,
              "transform": {"position":[0,0,0],"rotation":[0,0,0,1],"scale":[0.01,0.01,0.01]},
              "components": {} }
        ])));
        workspace.select(0, false);
        workspace.frame_selection();
        assert!(
            (0.4..2.0).contains(&workspace.camera.distance),
            "small object should not be too far, actual {}",
            workspace.camera.distance
        );
    }

    #[test]
    fn frame_all_uses_world_bounds_not_local_positions() {
        let mut workspace = Workspace::load_fixture(&scene_fixture(json!([
            { "id": 1, "name": "Root", "parent": null,
              "transform": {"position":[100,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
              "components": {} },
            { "id": 2, "name": "Child", "parent": 1,
              "transform": {"position":[50,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]},
              "components": {} }
        ])));
        workspace.frame_all();
        assert!(
            (workspace.camera.target[0] - 125.0).abs() < 1e-3,
            "world bounds center should be 125, actual {:?}",
            workspace.camera.target
        );
    }

    #[test]
    fn framing_an_empty_selection_does_not_move_the_camera() {
        let mut workspace = fixture_workspace();
        let before = workspace.camera.target;
        workspace.clear_selection();
        workspace.frame_selection();
        assert_eq!(workspace.camera.target, before);
        assert!(workspace.selection_world_bounds().is_none());
    }

    #[test]
    fn new_document_from_template() {
        let mut workspace = fixture_workspace();
        workspace.new_document(DocKind::Prefab, Some("camera"));
        assert_eq!(workspace.docs.len(), 2);
        assert_eq!(workspace.document().kind, DocKind::Prefab);
        assert!(workspace
            .document()
            .path
            .as_deref()
            .unwrap()
            .ends_with("camera.prefab.json"));
    }

    #[test]
    fn two_documents_from_the_same_template_never_share_a_path() {
        let project = TempProject::new("template-paths");
        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();
        workspace.new_document(DocKind::Prefab, Some("camera"));
        workspace.new_document(DocKind::Prefab, Some("camera"));

        let first = workspace.docs[workspace.docs.len() - 2]
            .path
            .clone()
            .unwrap();
        let second = workspace.docs[workspace.docs.len() - 1]
            .path
            .clone()
            .unwrap();
        assert_eq!(first, "assets/prefabs/camera.prefab.json");
        assert_eq!(second, "assets/prefabs/camera-2.prefab.json");
    }

    #[test]
    fn a_template_document_never_lands_on_an_existing_file() {
        let project = TempProject::new("template-existing");
        std::fs::write(
            project.root.join("assets/prefabs/camera.prefab.json"),
            r#"{"version":3,"entities":[]}"#,
        )
        .unwrap();
        let mut workspace = fixture_workspace();
        workspace.load_project(project.root.clone()).unwrap();
        workspace.new_document(DocKind::Prefab, Some("camera"));

        assert_eq!(
            workspace.document().path.as_deref(),
            Some("assets/prefabs/camera-2.prefab.json")
        );

        let saved = std::fs::read_to_string(project.root.join("assets/prefabs/camera.prefab.json"))
            .unwrap();
        assert!(saved.contains("\"entities\""));
    }

    #[test]
    fn commands_respect_enabled_state() {
        let mut workspace = fixture_workspace();
        assert!(!workspace.is_enabled(Command::Undo));
        workspace.run(Command::SelectAll);
        assert_eq!(workspace.selection.len(), 2);
        workspace.run(Command::DuplicateEntity);
        assert!(workspace.can_undo());
        assert!(workspace.is_enabled(Command::Undo));
        workspace.run(Command::Play);
        assert!(workspace.is_enabled(Command::Stop));
        assert!(!workspace.is_enabled(Command::Play));
    }

    #[test]
    fn undo_snapshot_roundtrip_for_reparent() {
        let mut workspace = fixture_workspace();
        workspace.reparent(1, None, Some(1));
        assert!(workspace.document().doc.entities[1].parent.is_none());
        workspace.undo();
        assert_eq!(workspace.document().doc.entities[1].parent, Some(1));
    }

    fn index_named(workspace: &Workspace, name: &str) -> usize {
        workspace
            .document()
            .doc
            .entities
            .iter()
            .position(|entity| entity.display_name() == name)
            .unwrap_or_else(|| panic!("missing entity {name}"))
    }

    #[test]
    fn selection_roots_drops_nested_entities() {
        let mut workspace = Workspace::sample();
        let cube = index_named(&workspace, "Cube");
        let detail = index_named(&workspace, "Detail");
        workspace.select(cube, false);
        workspace.select(detail, true);
        assert_eq!(workspace.selection_roots(), vec![cube]);
    }

    #[test]
    fn subtree_expand_toggles_all_descendant_ids() {
        let mut workspace = Workspace::sample();
        let cube = index_named(&workspace, "Cube");
        let detail = index_named(&workspace, "Detail");
        let light = index_named(&workspace, "Directional Light");
        let cube_id = workspace.document().doc.entities[cube].id.unwrap();
        let detail_id = workspace.document().doc.entities[detail].id.unwrap();
        let light_id = workspace.document().doc.entities[light].id.unwrap();

        workspace.set_subtree_expanded(cube, true);
        assert!(workspace.expanded.contains(&cube_id));
        assert!(workspace.expanded.contains(&detail_id));
        assert!(!workspace.expanded.contains(&light_id));

        workspace.set_subtree_expanded(cube, false);
        assert!(!workspace.expanded.contains(&cube_id));
        assert!(!workspace.expanded.contains(&detail_id));
    }

    #[test]
    fn reparent_selection_moves_all_roots_in_one_step() {
        let mut workspace = Workspace::sample();
        let cube = index_named(&workspace, "Cube");
        let light = index_named(&workspace, "Directional Light");
        let camera = index_named(&workspace, "Main Camera");
        let camera_id = workspace.document().doc.entities[camera].id;
        let cube_parent = workspace.document().doc.entities[cube].parent;
        let light_parent = workspace.document().doc.entities[light].parent;

        workspace.select(cube, false);
        workspace.select(light, true);
        assert_eq!(workspace.reparent_selection(Some(camera)), 2);
        assert_eq!(workspace.document().doc.entities[cube].parent, camera_id);
        assert_eq!(workspace.document().doc.entities[light].parent, camera_id);
        assert_eq!(workspace.document().undo.history().len(), 1);

        workspace.undo();
        assert_eq!(workspace.document().doc.entities[cube].parent, cube_parent);
        assert_eq!(
            workspace.document().doc.entities[light].parent,
            light_parent
        );
    }

    #[test]
    fn reparent_selection_rejects_dropping_into_own_descendant() {
        let mut workspace = Workspace::sample();
        let cube = index_named(&workspace, "Cube");
        let detail = index_named(&workspace, "Detail");
        workspace.select(cube, false);
        workspace.select(detail, true);
        assert_eq!(workspace.reparent_selection(Some(detail)), 0);
        assert!(!workspace.can_undo());
    }

    #[test]
    fn dragging_a_group_below_a_row_keeps_the_group_order() {
        let mut workspace = Workspace::sample();

        workspace.add_entity("empty");
        let anchor = workspace.document().doc.entities.len() - 1;
        let cube = index_named(&workspace, "Cube");
        let light = index_named(&workspace, "Directional Light");
        workspace.select(cube, false);
        workspace.select(light, true);

        let names = |workspace: &Workspace| -> Vec<String> {
            workspace
                .document()
                .doc
                .entities
                .iter()
                .map(|entity| entity.display_name())
                .collect()
        };
        let anchor_parent = workspace.document().doc.entities[anchor].parent;
        let before = names(&workspace);
        let steps = workspace.document().undo.history().len();

        assert_eq!(workspace.reparent_selection_at(anchor, false), 2);

        let moved_cube = index_named(&workspace, "Cube");
        let moved_light = index_named(&workspace, "Directional Light");
        assert_eq!(
            workspace.document().doc.entities[moved_cube].parent,
            anchor_parent
        );
        assert_eq!(
            workspace.document().doc.entities[moved_light].parent,
            anchor_parent
        );

        let moved_anchor = index_named(&workspace, "GameObject");
        assert!(
            moved_cube > moved_anchor && moved_light > moved_anchor,
            "cube={moved_cube} light={moved_light} anchor={moved_anchor} order={:?}",
            names(&workspace)
        );
        assert!(moved_cube < moved_light, "group order must be preserved");

        let detail = index_named(&workspace, "Detail");
        let cube_id = workspace.document().doc.entities[moved_cube].id;
        assert_eq!(workspace.document().doc.entities[detail].parent, cube_id);
        assert_eq!(
            workspace.document().undo.history().len(),
            steps + 1,
            "one drag = one undo step"
        );

        workspace.undo();
        assert_eq!(names(&workspace), before, "undo restores order and parent");
    }

    #[test]
    fn dragging_a_group_above_a_row_inserts_before_it() {
        let mut workspace = Workspace::sample();
        let cube = index_named(&workspace, "Cube");
        let light = index_named(&workspace, "Directional Light");
        let camera = index_named(&workspace, "Main Camera");
        workspace.select(cube, false);
        workspace.select(light, true);

        assert_eq!(workspace.reparent_selection_at(camera, true), 2);
        let moved_cube = index_named(&workspace, "Cube");
        let moved_light = index_named(&workspace, "Directional Light");
        let moved_camera = index_named(&workspace, "Main Camera");
        assert!(moved_cube < moved_camera && moved_light < moved_camera);
        assert!(moved_cube < moved_light);
    }

    #[test]
    fn a_group_cannot_be_dropped_between_its_own_children() {
        let mut workspace = Workspace::sample();
        let cube = index_named(&workspace, "Cube");
        let detail = index_named(&workspace, "Detail");
        workspace.select(cube, false);
        assert_eq!(workspace.reparent_selection_at(detail, true), 0);
        assert!(!workspace.can_undo());
    }

    #[test]
    fn copy_paste_keeps_subtree_and_attaches_parent() {
        let mut workspace = Workspace::sample();
        let cube = index_named(&workspace, "Cube");
        let light = index_named(&workspace, "Directional Light");
        workspace.select(cube, false);
        assert_eq!(workspace.copy_selection(), 2, "Cube + Detail");

        let before = workspace.document().doc.entities.len();
        let pasted = workspace.paste_entities(None);
        assert_eq!(pasted.len(), 1);
        assert_eq!(workspace.document().doc.entities.len(), before + 2);
        assert_eq!(workspace.selection, pasted);
        let root = &workspace.document().doc.entities[pasted[0]];
        assert_eq!(root.display_name(), "Cube (1)");
        assert!(root.parent.is_none());

        let pasted = workspace.paste_entities(Some(light));
        let light_id = workspace.document().doc.entities[light].id;
        assert_eq!(
            workspace.document().doc.entities[pasted[0]].parent,
            light_id
        );
    }

    #[test]
    fn cut_removes_source_then_paste_restores() {
        let mut workspace = Workspace::sample();
        let before = workspace.document().doc.entities.len();
        let detail = index_named(&workspace, "Detail");
        workspace.select(detail, false);
        assert_eq!(workspace.cut_selection(), 1);
        assert!(workspace
            .document()
            .doc
            .entities
            .iter()
            .all(|entity| entity.display_name() != "Detail"));
        assert!(workspace.entity_clipboard.as_ref().unwrap().cut);

        let pasted = workspace.paste_entities(None);
        assert_eq!(
            workspace.document().doc.entities[pasted[0]].display_name(),
            "Detail"
        );
        workspace.undo();
        workspace.undo();
        assert_eq!(workspace.document().doc.entities.len(), before);
        assert_eq!(
            workspace.document().doc.entities[detail].display_name(),
            "Detail"
        );
    }

    #[test]
    fn copy_preserves_prefab_instance_link() {
        let mut workspace = Workspace::sample();
        let cube = index_named(&workspace, "Cube");
        workspace.document_mut().doc.entities[cube]
            .components
            .insert(
                PREFAB_LINK_COMPONENT.to_string(),
                serde_json::json!({ "prefab": "assets/prefabs/cube.prefab.json" }),
            );
        workspace.select(cube, false);
        workspace.copy_selection();
        let pasted = workspace.paste_entities(None);
        assert_eq!(
            workspace.document().doc.entities[pasted[0]].prefab_link(),
            Some("assets/prefabs/cube.prefab.json")
        );
    }

    #[test]
    fn duplicate_multiple_roots_copies_each_subtree() {
        let mut workspace = Workspace::sample();
        let cube = index_named(&workspace, "Cube");
        let light = index_named(&workspace, "Directional Light");
        workspace.select(cube, false);
        workspace.select(light, true);
        let before = workspace.document().doc.entities.len();
        workspace.run(Command::DuplicateEntity);
        assert_eq!(workspace.document().doc.entities.len(), before + 3);
        assert_eq!(workspace.selection.len(), 2);
    }

    #[test]
    fn entity_clipboard_commands_enable_state() {
        let mut workspace = Workspace::sample();
        assert!(!workspace.is_enabled(Command::PasteEntity));
        workspace.select(0, false);
        assert!(workspace.is_enabled(Command::CopyEntity));
        workspace.run(Command::CopyEntity);
        assert!(workspace.is_enabled(Command::PasteEntity));
        workspace.run(Command::CutEntity);
        assert!(workspace.entity_clipboard.as_ref().unwrap().cut);
    }

    #[test]
    fn snap_settings_round_to_the_configured_steps() {
        let snap = SnapSettings::default();
        assert!(
            !snap.enabled,
            "snap default off, enabled via toolbar or Ctrl"
        );
        assert_eq!(snap.snapped_translate(0.31), 0.25);
        assert_eq!(snap.snapped_translate(0.38), 0.5);
        assert_eq!(snap.snapped_translate(-0.38), -0.5);
        let thirty = 30.0f32.to_radians();
        assert!((snap.snapped_radians(thirty) - thirty).abs() < 1e-6);
        assert!((snap.snapped_radians(thirty + 5.0f32.to_radians()) - thirty).abs() < 1e-6);
        assert_eq!(snap.snapped_scale(1.4), 1.0);
        assert_eq!(snap.snapped_scale(1.6), 2.0);
    }

    #[test]
    fn snap_settings_never_round_to_zero_scale() {
        let snap = SnapSettings {
            scale: 0.0,
            ..SnapSettings::default()
        };
        assert_eq!(snap.snapped_scale(0.3), 0.3, "step 0 returns as-is");
        let coarse = SnapSettings {
            scale: 10.0,
            ..SnapSettings::default()
        };
        assert_eq!(
            coarse.snapped_scale(0.2),
            0.01,
            "snapped to 0 clamped to 0.01"
        );
    }

    #[test]
    fn scrubbing_a_whole_vector_is_one_undo_step() {
        let mut workspace = Workspace::sample();
        let entity = workspace.document().doc.id_of_index(1).unwrap();
        workspace.set_transform_vector(entity, "position", [1.0, 2.0, 3.0]);
        let transform = workspace.document().doc.entities[1].transform;
        assert_eq!(transform.position, [1.0, 2.0, 3.0]);
        workspace.undo();
        assert_ne!(
            workspace.document().doc.entities[1].transform.position,
            [1.0, 2.0, 3.0],
            "one undo returns to before drag"
        );
        assert!(
            !workspace.can_undo(),
            "whole-row drag leaves only one undo step, actual {:?}",
            workspace.undo_history()
        );
    }

    #[test]
    fn set_transform_rotation_normalizes_and_undoes_in_one_step() {
        let mut workspace = Workspace::sample();
        let entity = workspace.document().doc.id_of_index(0).unwrap();
        let before = workspace.document().doc.entities[0].transform.rotation;
        workspace.set_transform_rotation(entity, [0.0, 0.0, 3.0, 3.0]);
        let after = workspace.document().doc.entities[0].transform.rotation;
        let length = (after.iter().map(|v| v * v).sum::<f32>()).sqrt();
        assert!(
            (length - 1.0).abs() < 1e-5,
            "normalized before write: {after:?}"
        );
        workspace.undo();
        assert_eq!(
            workspace.document().doc.entities[0].transform.rotation,
            before
        );
    }

    #[test]
    fn set_transform_rotation_ignores_degenerate_input() {
        let mut workspace = Workspace::sample();
        let entity = workspace.document().doc.id_of_index(0).unwrap();
        let before = workspace.document().doc.entities[0].transform.rotation;
        workspace.set_transform_rotation(entity, [0.0; 4]);
        assert_eq!(
            workspace.document().doc.entities[0].transform.rotation,
            before
        );
        assert!(
            !workspace.can_undo(),
            "illegal input should not produce undo step"
        );
    }

    #[test]
    fn set_transform_rotation_many_writes_every_selection() {
        let mut workspace = Workspace::sample();
        let ids: Vec<u32> = (0..2)
            .map(|index| workspace.document().doc.id_of_index(index).unwrap())
            .collect();
        let quat = crate::viewport_math::euler_degrees_to_quat([0.0, 90.0, 0.0]);
        workspace.set_transform_rotation_many(&ids, quat);
        for index in 0..2 {
            let rotation = workspace.document().doc.entities[index].transform.rotation;
            for axis in 0..4 {
                assert!((rotation[axis] - quat[axis]).abs() < 1e-5, "{rotation:?}");
            }
        }
        workspace.undo();
        let restored = workspace.document().doc.entities[0].transform.rotation;
        assert!(
            (restored[3] - 1.0).abs() < 1e-5,
            "one undo returns to state where neither entity changed"
        );
    }

    #[test]
    fn recovery_snapshot_round_trips_only_dirty_documents() {
        let project = TempProject::new("recovery-roundtrip");
        let mut workspace = Workspace::sample();
        workspace.project_root = Some(project.root.clone());
        workspace.document_mut().path = Some("assets/scenes/recovery.scene.json".to_string());
        workspace
            .document_mut()
            .doc
            .add_named_child(None, "recovered");
        workspace.document_mut().dirty = true;
        let expected = workspace.document().doc.clone();

        workspace.docs.push(Document::new(
            DocKind::Scene,
            Some("assets/scenes/clean.scene.json".to_string()),
            SceneDoc::default(),
        ));

        assert_eq!(workspace.write_recovery().unwrap(), 1);

        let mut fresh = Workspace::sample();
        fresh.project_root = Some(project.root.clone());
        assert_eq!(fresh.recovery_pending().unwrap(), 1);
        assert_eq!(fresh.recover_unsaved().unwrap(), 1);
        assert_eq!(fresh.recovery_pending().unwrap(), 0);
        let recovered = fresh
            .docs
            .iter()
            .find(|document| document.path.as_deref() == Some("assets/scenes/recovery.scene.json"))
            .expect("recovered document must appear in workspace");
        assert!(recovered.is_dirty(), "recovered document must stay dirty");
        assert_eq!(recovered.doc, expected);
    }

    #[test]
    fn recovery_replaces_an_already_open_document_without_a_second_tab() {
        let project = TempProject::new("recovery-replace");
        let mut workspace = Workspace::sample();
        workspace.project_root = Some(project.root.clone());
        workspace.document_mut().path = Some("assets/scenes/open.scene.json".to_string());
        workspace
            .document_mut()
            .doc
            .add_named_child(None, "from-recovery");
        workspace.document_mut().dirty = true;
        workspace.write_recovery().unwrap();

        let mut fresh = Workspace::sample();
        fresh.project_root = Some(project.root.clone());
        fresh.document_mut().path = Some("assets/scenes/open.scene.json".to_string());
        let tabs = fresh.docs.len();

        assert_eq!(fresh.recover_unsaved().unwrap(), 1);
        assert_eq!(
            fresh.docs.len(),
            tabs,
            "same-path document replaced in place, must not open another tab"
        );
        assert!(fresh.document().is_dirty());
        assert!(fresh
            .document()
            .doc
            .entities
            .iter()
            .any(|entity| entity.name.as_deref() == Some("from-recovery")));
    }

    #[test]
    fn discarding_recovery_removes_the_snapshot() {
        let project = TempProject::new("recovery-discard");
        let mut workspace = Workspace::sample();
        workspace.project_root = Some(project.root.clone());
        workspace.document_mut().dirty = true;
        workspace.write_recovery().unwrap();
        assert_eq!(workspace.recovery_pending().unwrap(), 1);

        workspace.discard_recovery().unwrap();
        assert_eq!(workspace.recovery_pending().unwrap(), 0);
        assert!(!project
            .root
            .join(crate::recovery::RECOVERY_DIR)
            .join(crate::recovery::RECOVERY_FILE)
            .exists());
    }

    #[test]
    fn world_delta_and_position_preserve_nested_rotated_negative_scale() {
        let mut workspace = Workspace::sample();
        let grandparent = workspace
            .document_mut()
            .doc
            .add_named_child(None, "Grandparent");
        workspace.document_mut().doc.entities[grandparent].transform = TransformJson {
            position: [5.0, 1.0, -2.0],
            rotation: crate::viewport_math::quat_axis_angle(1, std::f32::consts::FRAC_PI_2),
            scale: [-2.0, 1.5, 3.0],
        };
        let parent = workspace
            .document_mut()
            .doc
            .add_named_child(Some(grandparent), "Parent");
        workspace.document_mut().doc.entities[parent].transform = TransformJson {
            position: [1.0, 2.0, -1.0],
            rotation: crate::viewport_math::quat_axis_angle(2, std::f32::consts::FRAC_PI_4),
            scale: [2.0, -1.0, 0.5],
        };
        let child = workspace
            .document_mut()
            .doc
            .add_named_child(Some(parent), "Child");
        workspace.document_mut().doc.entities[child]
            .transform
            .position = [0.5, -0.5, 1.0];
        let child_id = workspace.document().doc.entities[child].id.unwrap();

        let before = world_position(&workspace.document().doc, child);
        let delta = [1.25, -0.75, 2.5];
        workspace.translate_entity_world(child_id, delta);
        let after = world_position(&workspace.document().doc, child);
        for axis in 0..3 {
            assert!(
                (after[axis] - before[axis] - delta[axis]).abs() < 1e-3,
                "axis {axis}: before={before:?}, after={after:?}"
            );
        }
    }
}

#[cfg(test)]
mod visibility_tests {
    use super::*;

    #[test]
    fn hiding_a_parent_hides_its_subtree_without_touching_the_document() {
        let mut workspace = Workspace::sample();

        workspace.set_entity_hidden(1, true);

        assert!(workspace.is_entity_hidden(1));
        assert!(workspace.is_entity_hidden(3));
        assert!(!workspace.is_entity_hidden(0));
        assert!(
            !workspace.document().is_dirty(),
            "visibility is not authoring data"
        );

        workspace.set_entity_hidden(3, false);
        assert!(workspace.is_entity_hidden(1));
        assert!(
            !workspace.is_entity_hidden(3),
            "child can be shown separately"
        );
    }

    #[test]
    fn isolate_keeps_the_selection_and_its_subtree() {
        let mut workspace = Workspace::sample();
        workspace.select(1, false);
        workspace.set_isolate(true);

        assert!(!workspace.is_entity_hidden(1));
        assert!(
            !workspace.is_entity_hidden(3),
            "subtree kept together when parent selected"
        );
        assert!(workspace.is_entity_hidden(0));
        assert!(workspace.is_entity_hidden(2));

        workspace.show_all_entities();
        assert!(!workspace.isolate);
        assert!(workspace.hidden_entity_ids().is_empty());
    }

    #[test]
    fn isolate_requires_a_selection_and_resets_when_switching_documents() {
        let mut workspace = Workspace::sample();
        workspace.set_isolate(true);
        assert!(
            !workspace.isolate,
            "empty selection must not isolate whole scene to black"
        );

        workspace.select(1, false);
        workspace.set_isolate(true);
        assert!(workspace.isolate);
        workspace.docs.push(Document::new(
            DocKind::Scene,
            Some("assets/scenes/other.scene.json".to_string()),
            SceneDoc::default(),
        ));
        workspace.set_active(1);
        assert!(!workspace.isolate, "switching document auto-exits Isolate");
    }
}
