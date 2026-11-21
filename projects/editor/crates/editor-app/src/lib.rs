//! Prisma self-hosted editor ("editor as a game") entry & composable workspace.

#![deny(warnings)]

pub mod bridge;
pub mod inspector;

use std::collections::{BTreeMap, HashSet};

use glam::Vec2;
use prism_app::Subsystem;
use prism_editor_api::schema::{build_schema, Schema};
use prism_ui::prelude::*;
use serde_json::{json, Value};

use crate::inspector::{build_inspector_ui, field_node_key, FieldEdit, UndoStack};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoMode {
    Translate,
    Rotate,
    Scale,
}

/// Minimal in-memory scene entity record for the self-hosted editor shell.
#[derive(Clone, Debug)]
pub struct EditorEntity {
    pub id: u32,
    pub name: String,
    pub parent: Option<u32>,
    pub components: BTreeMap<String, Value>,
}

/// Retained editor state hosted as a resource inside `prism-app` / `prism-engine`.
pub struct EditorUiState {
    pub tree: UiTree,
    pub draw_list: UiDrawList,
    pub schema: Schema,
    pub undo: UndoStack,
    pub entities: Vec<EditorEntity>,
    pub expanded: HashSet<u32>,
    pub selected_entity: u32,
    pub search_query: String,
    pub gizmo_mode: GizmoMode,
    pub playing: bool,
    pub elapsed_time: f64,
    pub assets: Vec<String>,
}

impl Default for EditorUiState {
    fn default() -> Self {
        let schema = {
            let mut span = prism_engine::util::timing::span("editor::build_schema");
            let schema = build_schema();
            span.detail(format!("types={}", schema.types.len()));
            schema
        };
        let mut expanded = HashSet::new();
        expanded.insert(1);

        let mut cam_comps = BTreeMap::new();
        cam_comps.insert(
            "prism_engine::scene::LocalTransform".to_string(),
            json!({
                "position": [0.0, 2.0, 5.0],
                "rotation": [0.0, 0.0, 0.0, 1.0],
                "scale": [1.0, 1.0, 1.0],
            }),
        );
        if let Some(cam_schema) = schema.type_by_name("prism_engine::scene::Camera") {
            if let Some(def) = &cam_schema.default_json {
                cam_comps.insert("prism_engine::scene::Camera".to_string(), def.clone());
            }
        }

        let mut light_comps = BTreeMap::new();
        light_comps.insert(
            "prism_engine::scene::LocalTransform".to_string(),
            json!({
                "position": [3.0, 6.0, 2.0],
                "rotation": [0.0, 0.0, 0.0, 1.0],
                "scale": [1.0, 1.0, 1.0],
            }),
        );

        Self {
            tree: UiTree::new(),
            draw_list: UiDrawList::default(),
            schema,
            undo: UndoStack::default(),
            entities: vec![
                EditorEntity {
                    id: 1,
                    name: "SceneRoot".to_string(),
                    parent: None,
                    components: BTreeMap::new(),
                },
                EditorEntity {
                    id: 2,
                    name: "MainCamera".to_string(),
                    parent: Some(1),
                    components: cam_comps,
                },
                EditorEntity {
                    id: 3,
                    name: "SunLight".to_string(),
                    parent: Some(1),
                    components: light_comps,
                },
            ],
            expanded,
            selected_entity: 2,
            search_query: String::new(),
            gizmo_mode: GizmoMode::Translate,
            playing: false,
            elapsed_time: 0.0,
            assets: vec![
                "scenes/main.scene.json".to_string(),
                "materials/pbr_metal.mat.json".to_string(),
                "meshes/hero.gltf".to_string(),
                "shaders/forward.slang".to_string(),
            ],
        }
    }
}

impl EditorUiState {
    fn visible_hierarchy_rows(&self) -> Vec<(u32, u16, bool, bool, String)> {
        let query = self.search_query.to_lowercase();
        let mut out = Vec::new();
        self.collect_hierarchy_rows(None, 0, &query, &mut out);
        out
    }

    fn collect_hierarchy_rows(
        &self,
        parent: Option<u32>,
        depth: u16,
        query: &str,
        out: &mut Vec<(u32, u16, bool, bool, String)>,
    ) {
        for ent in &self.entities {
            if ent.parent != parent {
                continue;
            }
            let has_children = self.entities.iter().any(|c| c.parent == Some(ent.id));
            let expanded = self.expanded.contains(&ent.id);
            if query.is_empty() || ent.name.to_lowercase().contains(query) {
                out.push((ent.id, depth, has_children, expanded, ent.name.clone()));
            }
            if expanded || !query.is_empty() {
                self.collect_hierarchy_rows(Some(ent.id), depth + 1, query, out);
            }
        }
    }

    /// Build one frame of the self-hosted editor UI and process interaction actions.
    pub fn build_and_step(
        &mut self,
        viewport_size: Vec2,
        input: &UiInput,
        dt: f32,
    ) -> Vec<UiAction> {
        self.tree.begin_frame();

        let playing = self.playing;
        let gizmo = self.gizmo_mode;
        let can_undo = self.undo.can_undo();
        let can_redo = self.undo.can_redo();
        let query = self.search_query.clone();
        let hierarchy = self.visible_hierarchy_rows();
        let tree_meta: Vec<(u32, bool, bool)> = hierarchy
            .iter()
            .map(|(id, _, has_c, exp, _)| (*id, *has_c, *exp))
            .collect();
        let tree_depths: Vec<u16> = hierarchy.iter().map(|(_, d, _, _, _)| *d).collect();
        let tree_names: Vec<(u32, String)> = hierarchy
            .iter()
            .map(|(id, _, _, _, name)| (*id, name.clone()))
            .collect();
        let assets = self.assets.clone();
        let selected = self.selected_entity;
        let inspector_panel = self
            .entities
            .iter()
            .find(|e| e.id == selected)
            .map(|e| build_inspector_ui(&self.schema, e.id, &e.components))
            .unwrap_or_else(|| muted_label("No entity selected"));

        {
            let mut ctx = UiContext::new(&mut self.tree);
            ctx.mount(
                col()
                    .size(Length::Px(viewport_size.x), Length::Px(viewport_size.y))
                    .bg(UiPalette::SURFACE_0)
                    .child(
                        // Top toolbar (composed from uniform button / toggle_button atoms)
                        row()
                            .height(Length::Px(36.0))
                            .padding(Edges::symmetric(10.0, 4.0))
                            .gap(6.0)
                            .align(Align::Center)
                            .bg(UiPalette::SURFACE_1)
                            .child(label("Prisma Editor"))
                            .child(divider(true))
                            .child(toggle_button(
                                "gizmo:translate",
                                "W",
                                gizmo == GizmoMode::Translate,
                            ))
                            .child(toggle_button(
                                "gizmo:rotate",
                                "E",
                                gizmo == GizmoMode::Rotate,
                            ))
                            .child(toggle_button("gizmo:scale", "R", gizmo == GizmoMode::Scale))
                            .child(spacer())
                            .child(toggle_button(
                                "toolbar:play",
                                if playing { "Stop" } else { "Play" },
                                playing,
                            ))
                            .child(button(
                                "toolbar:undo",
                                if can_undo { "Undo" } else { "Undo (0)" },
                            ))
                            .child(button(
                                "toolbar:redo",
                                if can_redo { "Redo" } else { "Redo (0)" },
                            )),
                    )
                    .child(divider(false))
                    .child(
                        // Middle workspace: Hierarchy | Viewport | Inspector
                        row()
                            .grow(1.0)
                            .child(
                                col()
                                    .width(Length::Px(250.0))
                                    .pad(8.0)
                                    .gap(6.0)
                                    .bg(UiPalette::SURFACE_1)
                                    .child(muted_label("HIERARCHY"))
                                    .child(text_field("hierarchy:search", query, "Filter..."))
                                    .child(tree_view(
                                        "hierarchy:tree",
                                        tree_meta,
                                        tree_depths,
                                        24.0,
                                        (viewport_size.y - 220.0).max(120.0),
                                        0.0,
                                        move |idx| {
                                            let (id, ref name) = tree_names[idx];
                                            toggle_button(
                                                ("entity", id),
                                                name.clone(),
                                                id == selected,
                                            )
                                        },
                                    )),
                            )
                            .child(divider(true))
                            .child(
                                col().grow(1.0).child(viewport(UiTextureId(0))).child(
                                    // Bottom Asset Browser (reuses virtual_list)
                                    col()
                                        .height(Length::Px(140.0))
                                        .pad(8.0)
                                        .gap(4.0)
                                        .bg(UiPalette::SURFACE_1)
                                        .child(muted_label("ASSETS"))
                                        .child(virtual_list(
                                            "assets:list",
                                            assets.len(),
                                            22.0,
                                            100.0,
                                            0.0,
                                            move |idx| {
                                                button(("asset", idx as u32), assets[idx].clone())
                                            },
                                        )),
                                ),
                            )
                            .child(divider(true))
                            .child(
                                col()
                                    .width(Length::Px(320.0))
                                    .pad(10.0)
                                    .gap(8.0)
                                    .bg(UiPalette::SURFACE_1)
                                    .child(muted_label(format!("INSPECTOR (#{selected})")))
                                    .child(inspector_panel),
                            ),
                    ),
            );
        }

        {
            let _layout = prism_engine::util::timing::span("editor::ui_layout");
            self.tree.compute_layout(viewport_size);
        }
        let actions = {
            let _input = prism_engine::util::timing::span("editor::ui_input");
            self.tree.process_input(input)
        };
        self.apply_actions(&actions);
        self.tree.step_animations(dt);
        {
            let mut draw = prism_engine::util::timing::span("editor::ui_draw_list");
            self.tree.build_draw_list(&mut self.draw_list);
            draw.detail(format!("commands={}", self.draw_list.commands.len()));
        }
        actions
    }

    fn apply_actions(&mut self, actions: &[UiAction]) {
        for action in actions {
            match action {
                UiAction::Toggled { key, value } => {
                    if *key == "toolbar:play".into_node_key() {
                        self.playing = *value;
                    } else if *key == "gizmo:translate".into_node_key() {
                        self.gizmo_mode = GizmoMode::Translate;
                    } else if *key == "gizmo:rotate".into_node_key() {
                        self.gizmo_mode = GizmoMode::Rotate;
                    } else if *key == "gizmo:scale".into_node_key() {
                        self.gizmo_mode = GizmoMode::Scale;
                    } else {
                        for ent in &self.entities {
                            if *key == ("tree_exp", ent.id).into_node_key() {
                                if *value {
                                    self.expanded.insert(ent.id);
                                } else {
                                    self.expanded.remove(&ent.id);
                                }
                            } else if *key == ("entity", ent.id).into_node_key() {
                                self.selected_entity = ent.id;
                            }
                        }
                    }
                }
                UiAction::Clicked(key) => {
                    if *key == "toolbar:undo".into_node_key() {
                        if let Some(edit) = self.undo.pop_undo() {
                            self.apply_field_value(
                                edit.entity_id,
                                &edit.component,
                                &edit.field,
                                edit.axis,
                                edit.old_value,
                            );
                        }
                    } else if *key == "toolbar:redo".into_node_key() {
                        if let Some(edit) = self.undo.pop_redo() {
                            self.apply_field_value(
                                edit.entity_id,
                                &edit.component,
                                &edit.field,
                                edit.axis,
                                edit.new_value,
                            );
                        }
                    }
                }
                UiAction::TextChanged { key, text } => {
                    if *key == "hierarchy:search".into_node_key() {
                        self.search_query = text.clone();
                    }
                }
                UiAction::ValueChanged { key, value } => {
                    self.update_matching_numeric_field(*key, *value, None);
                }
                UiAction::ValueCommitted { key, old, new } => {
                    self.update_matching_numeric_field(*key, *new, Some(*old));
                }
                _ => {}
            }
        }
    }

    fn update_matching_numeric_field(
        &mut self,
        key: NodeKey,
        new_val: f64,
        commit_old: Option<f64>,
    ) {
        let selected = self.selected_entity;
        let Some(ent) = self.entities.iter().find(|e| e.id == selected) else {
            return;
        };
        let mut matched: Option<(String, String, Option<usize>)> = None;
        for comp_name in ent.components.keys() {
            let Some(ts) = self.schema.type_by_name(comp_name) else {
                continue;
            };
            for field in &ts.fields {
                if key == field_node_key(selected, comp_name, &field.name, None) {
                    matched = Some((comp_name.clone(), field.name.clone(), None));
                    break;
                }
                for axis in 0..3 {
                    if key == field_node_key(selected, comp_name, &field.name, Some(axis)) {
                        matched = Some((comp_name.clone(), field.name.clone(), Some(axis)));
                        break;
                    }
                }
            }
        }
        if let Some((comp, field, axis)) = matched {
            self.apply_field_value(selected, &comp, &field, axis, json!(new_val));
            if let Some(old_val) = commit_old {
                self.undo.push(FieldEdit {
                    entity_id: selected,
                    component: comp,
                    field,
                    axis,
                    old_value: json!(old_val),
                    new_value: json!(new_val),
                });
            }
        }
    }

    fn apply_field_value(
        &mut self,
        entity_id: u32,
        component: &str,
        field: &str,
        axis: Option<usize>,
        val: Value,
    ) {
        let Some(ent) = self.entities.iter_mut().find(|e| e.id == entity_id) else {
            return;
        };
        let Some(comp_obj) = ent
            .components
            .get_mut(component)
            .and_then(|v| v.as_object_mut())
        else {
            return;
        };
        if let Some(ax) = axis {
            if let Some(arr) = comp_obj.get_mut(field).and_then(|v| v.as_array_mut()) {
                if let Some(slot) = arr.get_mut(ax) {
                    *slot = val;
                }
            }
        } else {
            comp_obj.insert(field.to_string(), val);
        }
    }
}

fn editor_system(world: &mut prism_engine::ecs::World, dt: f32) {
    let _span = prism_engine::util::timing::span("editor::tick");
    let mut state = world.remove_resource::<EditorUiState>().unwrap_or_default();
    state.elapsed_time += dt as f64;
    let screen = bridge::screen_size(world);
    let input = bridge::read_engine_input(world, state.elapsed_time);
    state.build_and_step(screen, &input, dt);
    bridge::submit_draw_list(world, &state.draw_list);
    world.insert_resource(state);
}

/// Build the editor application.
///
/// Uses [`prism_app::App::hosted`] rather than the player entry point
/// [`prism_app::app`]: the editor owns its own documents and loads them through
/// [`prism_app::App::load_scene_json_file`], so it must not attempt a startup
/// scene load. A player entry point would try to resolve `assets/scenes.json`,
/// fail (an editor checkout has no scene manifest), and paint the
/// "no scene" placeholder over the whole window.
pub fn build_app() -> prism_app::App {
    let _span = prism_engine::util::timing::span("editor::build_app");
    let mut app = prism_app::App::hosted(prism_app::load_config())
        .with_pointer_lock_policy(prism_app::PointerLockPolicy::Manual)
        .with_subsystem(Subsystem::Render);
    {
        let _state = prism_engine::util::timing::span("editor::state_default");
        app.engine_mut()
            .world_mut()
            .insert_resource(EditorUiState::default());
    }
    app.add_system("editor::tick", editor_system);
    app
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_inspector_edit_and_undo_redo_roundtrip() {
        let mut editor = EditorUiState::default();
        editor.build_and_step(Vec2::new(1280.0, 720.0), &UiInput::default(), 0.016);

        // Locate Transform.position[0] on entity 2 (MainCamera)
        let key = field_node_key(
            2,
            "prism_engine::scene::LocalTransform",
            "position",
            Some(0),
        );
        let rect = editor
            .tree
            .node(editor.tree.node_by_key(key).expect("field node exists"))
            .unwrap()
            .computed_rect;
        let center = Vec2::new(rect.x + rect.w * 0.5, rect.y + rect.h * 0.5);

        editor.build_and_step(
            Vec2::new(1280.0, 720.0),
            &UiInput {
                pointer_pos: center,
                primary_down: true,
                primary_just_pressed: true,
                ..Default::default()
            },
            0.016,
        );
        editor.build_and_step(
            Vec2::new(1280.0, 720.0),
            &UiInput {
                pointer_pos: center + Vec2::new(40.0, 0.0),
                primary_down: true,
                ..Default::default()
            },
            0.016,
        );
        editor.build_and_step(
            Vec2::new(1280.0, 720.0),
            &UiInput {
                pointer_pos: center + Vec2::new(40.0, 0.0),
                primary_just_released: true,
                ..Default::default()
            },
            0.016,
        );

        let edited_x = editor.entities[1].components["prism_engine::scene::LocalTransform"]
            ["position"][0]
            .as_f64()
            .unwrap();
        assert!((edited_x - 1.0).abs() < 1e-4);
        assert!(editor.undo.can_undo());

        // Click Undo button
        let undo_rect = editor
            .tree
            .node(
                editor
                    .tree
                    .node_by_key("toolbar:undo".into_node_key())
                    .unwrap(),
            )
            .unwrap()
            .computed_rect;
        let u_center = Vec2::new(
            undo_rect.x + undo_rect.w * 0.5,
            undo_rect.y + undo_rect.h * 0.5,
        );
        editor.build_and_step(
            Vec2::new(1280.0, 720.0),
            &UiInput {
                pointer_pos: u_center,
                primary_down: true,
                primary_just_pressed: true,
                ..Default::default()
            },
            0.016,
        );
        editor.build_and_step(
            Vec2::new(1280.0, 720.0),
            &UiInput {
                pointer_pos: u_center,
                primary_just_released: true,
                ..Default::default()
            },
            0.016,
        );

        let undone_x = editor.entities[1].components["prism_engine::scene::LocalTransform"]
            ["position"][0]
            .as_f64()
            .unwrap();
        assert!((undone_x - 0.0).abs() < 1e-4);
    }

    #[test]
    fn engine_draw_list_conversion_preserves_viewport_and_text() {
        let mut editor = EditorUiState::default();
        editor.build_and_step(Vec2::new(1280.0, 720.0), &UiInput::default(), 0.016);
        let engine_list = bridge::convert_to_engine_draw_list(&editor.draw_list);
        assert!(engine_list.quads().any(|q| q.texture == Some(0)));
        assert!(engine_list.texts().any(|t| t.content == "Prisma Editor"));
    }

    #[test]
    fn submitting_the_editor_draw_list_does_not_erase_engine_ui() {
        use prism_engine::ui::{UiDrawList as EngineUiDrawList, UiQuad};

        let mut world = prism_engine::ecs::World::new();
        // Stand-in for what `ui::render` produced this frame from ECS UI entities.
        let mut engine_list = EngineUiDrawList::default();
        engine_list.push_quad(UiQuad {
            rect: [0.0, 0.0, 10.0, 10.0],
            color: [1.0, 0.0, 0.0, 1.0],
            ..Default::default()
        });
        world.insert_resource(engine_list);

        let mut editor = EditorUiState::default();
        editor.build_and_step(Vec2::new(1280.0, 720.0), &UiInput::default(), 0.016);
        bridge::submit_draw_list(&mut world, &editor.draw_list);

        let list = world
            .get_resource::<EngineUiDrawList>()
            .expect("draw list resource exists");
        assert!(
            list.quads().any(|q| q.color == [1.0, 0.0, 0.0, 1.0]),
            "editor submission must append to the engine's draw list, not replace it"
        );
        assert!(
            list.len() > 1,
            "editor draw commands should be present alongside the engine's"
        );
    }

    /// The editor's pointer source is the engine-published `UiInputState`, not
    /// `InputManager`: the latter is not a `World` resource (it lives on `App`),
    /// so sampling it every frame yielded `default()` — cursor at the origin, no
    /// buttons — and no click could ever reach the UI tree.
    #[test]
    fn editor_pointer_input_comes_from_the_engine_ui_state() {
        let mut world = prism_engine::ecs::World::new();
        let empty = bridge::read_engine_input(&world, 0.0);
        assert_eq!(empty.pointer_pos, Vec2::ZERO);
        assert!(!empty.primary_down);

        world.insert_resource(prism_engine::ui::UiInputState {
            cursor_pos: [321.0, 123.0],
            left_clicked: true,
            left_held: true,
            scroll_delta: [0.0, 2.0],
            shift_held: true,
            pressed_keys: vec![prism_engine::input::KeyCode::Escape],
            ..Default::default()
        });
        let input = bridge::read_engine_input(&world, 0.5);
        assert_eq!(input.pointer_pos, Vec2::new(321.0, 123.0));
        assert!(input.primary_down);
        assert!(input.primary_just_pressed);
        assert!(!input.primary_just_released);
        assert_eq!(input.scroll_delta, Vec2::new(0.0, 2.0));
        assert!(input.shift_held);
        assert!(!input.ctrl_held);
        assert!(input.escape_pressed);
        assert_eq!(input.time_seconds, 0.5);
    }

    /// End to end: a press and a release at the Play button's rectangle, both
    /// coming through the engine's `UiInputState`, must reach editor state.
    #[test]
    fn engine_pointer_state_reaches_the_ui_tree() {
        let mut editor = EditorUiState::default();
        editor.build_and_step(Vec2::new(1280.0, 720.0), &UiInput::default(), 0.016);
        let rect = editor
            .tree
            .node(
                editor
                    .tree
                    .node_by_key("toolbar:play".into_node_key())
                    .expect("play button exists"),
            )
            .unwrap()
            .computed_rect;
        let center = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];

        let mut world = prism_engine::ecs::World::new();
        world.insert_resource(prism_engine::ui::UiInputState {
            cursor_pos: center,
            left_clicked: true,
            left_held: true,
            ..Default::default()
        });
        let press = bridge::read_engine_input(&world, 0.0);
        editor.build_and_step(Vec2::new(1280.0, 720.0), &press, 0.016);

        world.insert_resource(prism_engine::ui::UiInputState {
            cursor_pos: center,
            left_released: true,
            ..Default::default()
        });
        let release = bridge::read_engine_input(&world, 0.1);
        let actions = editor.build_and_step(Vec2::new(1280.0, 720.0), &release, 0.016);

        assert!(
            actions
                .iter()
                .any(|a| matches!(a, UiAction::Toggled { .. } | UiAction::Clicked(_))),
            "clicking the Play button must emit an action, got {actions:?}"
        );
        assert!(editor.playing, "the Play toggle must reach editor state");
    }
}
