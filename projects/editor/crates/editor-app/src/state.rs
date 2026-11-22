//! Pure domain state slices for the editor, decoupled from UI widgets and engine internals.

use std::collections::{BTreeMap, HashSet};

use prism_editor_api::schema::{build_schema, Schema};
use serde_json::{json, Value};

/// Active 3D viewport manipulation gizmo mode.
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

/// Document-level state that represents authored scene data and selection.
pub struct EditorDocument {
    pub schema: Schema,
    pub entities: Vec<EditorEntity>,
    pub expanded: HashSet<u32>,
    pub selected_entity: u32,
    pub assets: Vec<String>,
    pub dirty: bool,
}

impl Default for EditorDocument {
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
            schema,
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
            assets: vec![
                "scenes/main.scene.json".to_string(),
                "materials/pbr_metal.mat.json".to_string(),
                "meshes/hero.gltf".to_string(),
                "shaders/forward.slang".to_string(),
            ],
            dirty: false,
        }
    }
}

impl EditorDocument {
    /// Return the currently selected entity, if it still exists.
    pub fn selected_entity_ref(&self) -> Option<&EditorEntity> {
        self.entities.iter().find(|e| e.id == self.selected_entity)
    }

    /// Compute the next unused entity id.
    pub fn next_entity_id(&self) -> u32 {
        self.entities.iter().map(|e| e.id).max().unwrap_or(0) + 1
    }

    /// Collect visible hierarchy rows `(id, depth, has_children, expanded, name)` filtered by `query`.
    pub fn visible_hierarchy_rows(&self, query: &str) -> Vec<(u32, u16, bool, bool, String)> {
        let query_lower = query.to_lowercase();
        let mut out = Vec::new();
        self.collect_hierarchy_rows(None, 0, &query_lower, &mut out);
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

    /// Write a component field (or vector axis) on an entity.
    pub fn apply_field_value(
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
        self.dirty = true;
    }
}

/// Ephemeral editor session state (not persisted as part of the scene document).
pub struct EditorSession {
    pub search_query: String,
    pub gizmo_mode: GizmoMode,
    pub playing: bool,
    pub elapsed_time: f64,
    pub show_grid: bool,
    pub show_gizmos: bool,
}

impl Default for EditorSession {
    fn default() -> Self {
        Self {
            search_query: String::new(),
            gizmo_mode: GizmoMode::Translate,
            playing: false,
            elapsed_time: 0.0,
            show_grid: false,
            show_gizmos: true,
        }
    }
}
