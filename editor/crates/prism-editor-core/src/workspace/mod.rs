//! Editor workspace state: open documents, selection, dock layout, theme, log, and Play state.

mod asset_ops;
mod camera;
mod hierarchy_ops;
mod inspector_ops;
mod prefab_ops;
mod selection;
#[cfg(test)]
mod tests;
mod transform_ops;

pub use camera::{OrbitCamera, SnapSettings};
use selection::selection_ids_in;

use crate::anim_graph_view::AnimGraphLayout;
use crate::assets::{AssetDetails, AssetInfo, AssetKind, AssetQuery, AssetReference, AssetTree};
use crate::doc::{EntityJson, SceneDoc, TransformJson, PREFAB_LINK_COMPONENT};
use crate::dock::{
    self, collect_tabs, find_leaf, find_leaf_by_id, insert_panel, leaf, remove_panel, DockEdge,
    DockNode, DockZone, FloatPanel, LAYOUT_PRESETS,
};
use crate::i18n::{I18n, Locale};
use crate::keys::Command;
use crate::logbuf::{LogBuffer, LogLevel, LogSource};
use crate::manifest::SceneManifest;
use crate::project::{self, ProjectFile};
use crate::schema::{component_value_with_overrides, EntityPreset, Schema, TypeSchema};
use crate::theme::{ThemeColors, ThemeState, ThemeToken};
use crate::viewport_math::{
    authored_local_bounds, basis_from_quat, parent_world_rotation, quat_axis_angle, quat_conjugate,
    quat_mul, quat_normalize, quat_rotate, world_bounds_with, world_matrix_invertible,
    world_position, world_rotation, world_scale, GizmoSpace, PickBounds,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DOCUMENT_UID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DocKind {
    Scene,
    Prefab,
}

impl DocKind {
    pub fn folder(self) -> &'static str {
        match self {
            DocKind::Scene => "assets/scenes",
            DocKind::Prefab => "assets/prefabs",
        }
    }

    pub fn suffix(self) -> &'static str {
        match self {
            DocKind::Scene => ".scene.json",
            DocKind::Prefab => ".prefab.json",
        }
    }

    pub fn i18n_key(self) -> &'static str {
        match self {
            DocKind::Scene => "common.scene",
            DocKind::Prefab => "common.prefab",
        }
    }

    pub fn parse(text: &str) -> DocKind {
        if text == "prefab" {
            DocKind::Prefab
        } else {
            DocKind::Scene
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefabOrigin {
    pub scene_index: usize,

    pub entity_id: u32,

    pub scene_title: String,
}

#[derive(Debug, Clone)]
pub struct Document {
    /// Process-unique document identity to detect replacement across tabs or reloads.
    pub uid: u64,
    pub kind: DocKind,
    pub path: Option<String>,
    pub doc: SceneDoc,
    pub undo: crate::undo::UndoStack,
    /// Monotonic counter bumped on structural, component, or document-level edits.
    pub revision: u64,
    /// Monotonic counter bumped on transform-only edits so ECS can sync in place.
    pub transform_revision: u64,
    /// Unsaved edits outside undo history (batch rewrites, recovered docs, etc.).
    pub dirty: bool,
    pub saved_stamp: Option<project::FileStamp>,
    pub externally_modified: bool,
    pub prefab_origin: Option<PrefabOrigin>,
    pub hidden_entities: HashSet<u32>,
}

impl Document {
    pub fn new(kind: DocKind, path: Option<String>, doc: SceneDoc) -> Self {
        Self {
            uid: NEXT_DOCUMENT_UID.fetch_add(1, Ordering::Relaxed),
            kind,
            path,
            doc,
            undo: crate::undo::UndoStack::new(),
            revision: 0,
            transform_revision: 0,
            dirty: false,
            saved_stamp: None,
            externally_modified: false,
            prefab_origin: None,
            hidden_entities: HashSet::new(),
        }
    }

    /// Increment structural revision after non-transform document changes.
    pub fn mark_dirty(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// Increment transform revision for 0ms in-place ECS synchronization.
    pub fn mark_transform_dirty(&mut self) {
        self.transform_revision = self.transform_revision.wrapping_add(1);
    }

    /// Apply a single entity transform step and mark transform state dirty.
    pub fn apply_transform_step(
        &mut self,
        entity_id: u32,
        before: TransformJson,
        after: TransformJson,
        now_ms: u64,
    ) {
        self.undo
            .push_transform(&mut self.doc, entity_id, before, after, now_ms);
        self.mark_transform_dirty();
    }

    /// Apply one or more entity transform changes as one undo step and mark transform dirty.
    pub fn apply_transform_batch(
        &mut self,
        changes: Vec<(u32, TransformJson, TransformJson)>,
        key: String,
        now_ms: u64,
    ) {
        if changes.len() == 1 {
            let (entity_id, before, after) = changes[0];
            self.undo
                .push_transform(&mut self.doc, entity_id, before, after, now_ms);
        } else if !changes.is_empty() {
            self.undo
                .push_transform_batch(&mut self.doc, changes, key, now_ms);
        }
        self.mark_transform_dirty();
    }

    pub fn note_disk_state(&mut self, root: Option<&std::path::Path>) {
        let (Some(root), Some(path)) = (root, self.path.as_deref()) else {
            self.saved_stamp = None;
            self.externally_modified = false;
            return;
        };
        self.saved_stamp = project::file_content_stamp(root, path);
        self.externally_modified = false;
    }

    pub fn title(&self) -> String {
        match &self.path {
            Some(path) => path.rsplit('/').next().unwrap_or(path).to_string(),
            None => match self.kind {
                DocKind::Scene => "Untitled.scene.json".to_string(),
                DocKind::Prefab => "Untitled.prefab.json".to_string(),
            },
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty || self.undo.is_dirty()
    }

    pub fn mark_saved(&mut self) {
        self.dirty = false;
        self.undo.mark_saved();
    }

    pub fn title_with_dirty(&self) -> String {
        if self.is_dirty() {
            format!("{} *", self.title())
        } else {
            self.title()
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PanelInfo {
    pub id: String,
    pub title: String,
    pub menu_path: String,
    pub shortcut: Option<String>,

    pub utility: bool,
}

impl PanelInfo {
    pub fn new(id: &str, title: &str) -> Self {
        Self {
            id: id.to_string(),
            title: title.to_string(),
            menu_path: String::new(),
            shortcut: None,
            utility: dock::UTILITY_TABS.contains(&id),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TemplateDoc {
    pub id: String,
    pub label: String,
    pub kind: DocKind,
    pub doc: SceneDoc,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComponentClipboard {
    pub component: String,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntityClipboard {
    pub trees: Vec<SceneDoc>,

    pub cut: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PivotMode {
    #[default]
    Center,

    Pivot,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SelectMode {
    #[default]
    Replace,

    Add,

    Remove,
}

impl SelectMode {
    pub fn from_modifiers(shift: bool, command: bool) -> Self {
        if command {
            Self::Remove
        } else if shift {
            Self::Add
        } else {
            Self::Replace
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentPreset {
    pub component: String,
    pub name: String,
    pub value: Value,
}

#[derive(Debug, Clone)]
pub struct Workspace {
    pub docs: Vec<Document>,
    pub(super) active: usize,

    pub(super) object_bounds: Vec<Option<PickBounds>>,
    pub schema: Schema,
    pub presets: Vec<EntityPreset>,

    pub component_presets: Vec<ComponentPreset>,
    pub templates: Vec<TemplateDoc>,
    pub assets: Vec<AssetInfo>,
    pub tree: AssetTree,
    pub asset_query: AssetQuery,
    pub expanded_assets: HashSet<String>,
    pub panels: Vec<PanelInfo>,
    pub dock: DockNode,
    pub floating: Vec<FloatPanel>,

    pub maximized_tab: Option<String>,

    pub maximize_on_play: bool,

    pub(super) play_maximized_before: Option<Option<String>>,
    pub selection: Vec<usize>,

    pub selection_active: Option<usize>,
    pub expanded: HashSet<u32>,
    pub clipboard: Option<ComponentClipboard>,

    pub entity_clipboard: Option<EntityClipboard>,
    pub theme: ThemeState,
    pub i18n: I18n,
    pub log: LogBuffer,
    pub camera: OrbitCamera,

    pub gizmo_space: GizmoSpace,

    pub snap: SnapSettings,

    pub pivot_mode: PivotMode,

    pub show_grid: bool,

    pub show_selection_outline: bool,

    pub show_skeleton: bool,

    pub isolate: bool,

    pub show_camera_icons: bool,

    pub show_light_icons: bool,
    /// Show collider wireframe gizmos even when unselected.
    pub show_collider_gizmos: bool,
    /// Show audio source distance gizmos in Scene View.
    pub show_audio_gizmos: bool,
    /// Baked PRT probe volume Scene View visualization settings.
    pub prt_gizmos: crate::gizmo::PrtGizmoSettings,

    pub show_skybox: bool,
    pub playing: bool,
    pub paused: bool,
    pub play_time_s: f32,
    pub status: String,

    pub pending_save_conflict: Option<usize>,
    pub layout_preset: String,

    pub project_root: Option<PathBuf>,

    pub project_files: Vec<ProjectFile>,

    pub scene_manifest: SceneManifest,

    pub asset_edits: crate::asset_edit::AssetEditHistory,

    pub(super) play_snapshot: Option<PlaySnapshot>,
    /// Documents that failed to parse during the last `load_project` (project-relative paths).
    /// The UI uses this to say "N scene files failed" instead of silently
    /// showing the fixture documents (#1020).
    pub(super) project_load_failures: Vec<String>,
}

impl Default for Workspace {
    fn default() -> Self {
        Self::sample()
    }
}

#[derive(Debug, Clone)]
pub(super) struct PlaySnapshot {
    doc: usize,
    scene: SceneDoc,
    undo: crate::undo::UndoStack,
    dirty: bool,
    selection: Vec<usize>,
    selection_active: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PrefabOverride {
    pub entity: usize,
    pub entity_name: String,

    pub component: String,
    pub field: String,
    pub instance_value: Value,
    pub prefab_value: Value,
}

impl Workspace {
    /// Project-relative paths of documents that failed to parse in [`Self::load_project`].
    pub fn project_load_failures(&self) -> &[String] {
        &self.project_load_failures
    }

    pub fn sample() -> Self {
        let mut doc = SceneDoc::default();
        let camera = doc.add_named_child(None, "Main Camera");
        doc.entities[camera].transform = TransformJson {
            position: [0.0, 1.0, -10.0],
            ..Default::default()
        };
        let cube = doc.add_named_child(None, "Cube");
        doc.entities[cube].transform.position = [0.0, 0.5, 0.0];
        let light = doc.add_named_child(None, "Directional Light");
        doc.entities[light].transform.rotation = [0.3, 0.2, 0.1, 0.9];
        let _ = doc.add_named_child(Some(cube), "Detail");

        let mut workspace = Self {
            docs: vec![Document::new(DocKind::Scene, None, doc)],
            active: 0,
            object_bounds: Vec::new(),
            schema: Schema::default(),
            presets: default_presets(),
            component_presets: Vec::new(),
            templates: default_templates(),
            assets: Vec::new(),
            tree: AssetTree::default(),
            asset_query: AssetQuery::default(),
            expanded_assets: HashSet::new(),
            panels: default_panels(),
            dock: dock::default_dock(),
            floating: Vec::new(),
            maximized_tab: None,
            maximize_on_play: false,
            play_maximized_before: None,
            selection: Vec::new(),
            selection_active: None,
            expanded: HashSet::new(),
            clipboard: None,
            entity_clipboard: None,
            theme: ThemeState::default(),
            i18n: I18n::default(),
            log: LogBuffer::default(),
            camera: OrbitCamera::default(),
            gizmo_space: GizmoSpace::World,
            snap: SnapSettings::default(),
            pivot_mode: PivotMode::Center,
            show_grid: true,
            show_selection_outline: true,
            show_skeleton: true,
            isolate: false,
            show_camera_icons: true,
            show_light_icons: true,
            show_collider_gizmos: false,
            show_audio_gizmos: true,
            prt_gizmos: crate::gizmo::PrtGizmoSettings::default(),
            show_skybox: true,
            playing: false,
            paused: false,
            play_time_s: 0.0,
            status: String::new(),
            pending_save_conflict: None,
            layout_preset: "default".to_string(),
            project_root: None,
            project_files: Vec::new(),
            scene_manifest: SceneManifest::default(),
            asset_edits: crate::asset_edit::AssetEditHistory::new(),
            play_snapshot: None,
            project_load_failures: Vec::new(),
        };
        workspace.log(
            LogSource::Editor,
            LogLevel::Info,
            "Prisma egui editor started",
        );
        workspace
    }
}

pub(super) fn default_presets() -> Vec<EntityPreset> {
    serde_json::from_value(json!([
        {
            "id": "cube",
            "label": "Cube",
            "name": "Cube",
            "components": ["prism_engine::scene::MeshRenderer", "prism_engine::scene::Active"],
            "overrides": { "prism_engine::scene::MeshRenderer": { "mesh_path": "__builtin/box" } },
            "position": [0.0, 0.5, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0]
        },
        {
            "id": "sphere",
            "label": "Sphere",
            "name": "Sphere",
            "components": ["prism_engine::scene::MeshRenderer", "prism_engine::scene::Active"],
            "overrides": { "prism_engine::scene::MeshRenderer": { "mesh_path": "__builtin/sphere" } },
            "position": [0.0, 0.5, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0]
        },
        {
            "id": "cylinder",
            "label": "Cylinder",
            "name": "Cylinder",
            "components": ["prism_engine::scene::MeshRenderer", "prism_engine::scene::Active"],
            "overrides": { "prism_engine::scene::MeshRenderer": { "mesh_path": "__builtin/cylinder" } },
            "position": [0.0, 0.5, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0]
        },
        {
            "id": "capsule",
            "label": "Capsule",
            "name": "Capsule",
            "components": ["prism_engine::scene::MeshRenderer", "prism_engine::scene::Active"],
            "overrides": { "prism_engine::scene::MeshRenderer": { "mesh_path": "__builtin/capsule" } },
            "position": [0.0, 1.0, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0]
        },
        {
            "id": "quad",
            "label": "Quad",
            "name": "Quad",
            "components": ["prism_engine::scene::MeshRenderer", "prism_engine::scene::Active"],
            "overrides": { "prism_engine::scene::MeshRenderer": { "mesh_path": "__builtin/quad" } },
            "position": [0.0, 0.0, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0]
        },
        {
            "id": "plane",
            "label": "Plane",
            "name": "Plane",
            "components": ["prism_engine::scene::MeshRenderer", "prism_engine::scene::Active"],
            "overrides": { "prism_engine::scene::MeshRenderer": { "mesh_path": "__builtin/quad" } },
            "position": [0.0, 0.0, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0]
        },
        {
            "id": "empty",
            "label": "Empty",
            "name": "GameObject",
            "components": [],
            "overrides": {},
            "position": [0.0, 0.0, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0]
        },
        {
            "id": "camera",
            "label": "Camera",
            "name": "Camera",
            "components": ["prism_engine::scene::Camera", "prism_engine::scene::FlyCameraController"],
            "overrides": {},
            "position": [0.0, 0.0, 10.0],
            "rotation": [0.0, 0.0, 0.0, 1.0]
        },
        {
            "id": "sun",
            "label": "Directional Light",
            "name": "Sun",
            "components": ["prism_engine::scene::Light"],
            "overrides": { "prism_engine::scene::Light": { "light_type": "Directional", "intensity": 100000.0, "shadows": "Soft" } },
            "position": [0.0, 6.0, 0.0],
            "rotation": [-0.4082483, -0.28867513, 0.0, 0.8660254]
        },
        {
            "id": "point_light",
            "label": "Point Light",
            "name": "PointLight",
            "components": ["prism_engine::scene::Light"],
            "overrides": {},
            "position": [3.0, 3.0, 3.0],
            "rotation": [0.0, 0.0, 0.0, 1.0]
        },
        {
            "id": "spot_light",
            "label": "Spot Light",
            "name": "SpotLight",
            "components": ["prism_engine::scene::Light"],
            "overrides": { "prism_engine::scene::Light": { "light_type": "Spot", "intensity": 200.0, "range": 20.0, "inner_spot_angle": 30.0, "spot_angle": 45.0, "shadows": "Soft" } },
            "position": [0.0, 6.0, 0.0],
            "rotation": [0.70710677, 0.0, 0.0, 0.70710677]
        },
        {
            "id": "mesh",
            "label": "Mesh",
            "name": "Mesh",
            "components": ["prism_engine::scene::MeshRenderer", "prism_engine::scene::Active"],
            "overrides": {},
            "position": [0.0, 0.0, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0]
        },
        {
            "id": "skybox",
            "label": "Skybox",
            "name": "Skybox",
            "components": ["prism_engine::scene::Skybox"],
            "overrides": {},
            "position": [0.0, 0.0, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0]
        },
        {
            "id": "ui",
            "label": "UI Element",
            "name": "ui_node",
            "components": ["prism_engine::ui::Node", "prism_engine::ui::Style"],
            "overrides": {},
            "position": [0.0, 0.0, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0]
        }
    ]))
    .unwrap_or_default()
}

pub(super) fn default_templates() -> Vec<TemplateDoc> {
    let parse_scene = |val: Value| -> SceneDoc {
        let mut doc: SceneDoc = serde_json::from_value(val).unwrap_or_default();
        doc.normalize();
        doc
    };
    vec![
        TemplateDoc {
            id: "camera".to_string(),
            label: "Camera".to_string(),
            kind: DocKind::Scene,
            doc: parse_scene(json!({
                "version": 3,
                "entities": [{
                    "id": 0,
                    "name": "Camera",
                    "parent": null,
                    "transform": { "position": [0.0, 0.0, 10.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0] },
                    "components": { "prism_engine::scene::Camera": { "enabled": true, "fov_y_degrees": 60.0, "near": 0.1, "far": 1000.0, "clear_flags": "Skybox" } }
                }]
            })),
        },
        TemplateDoc {
            id: "lit".to_string(),
            label: "Camera + Lighting".to_string(),
            kind: DocKind::Scene,
            doc: parse_scene(json!({
                "version": 3,
                "entities": [
                    {
                        "id": 0,
                        "name": "Camera",
                        "parent": null,
                        "transform": { "position": [0.0, 2.5, 18.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0] },
                        "components": { "prism_engine::scene::Camera": { "enabled": true, "fov_y_degrees": 60.0, "near": 0.1, "far": 1000.0, "clear_flags": "Skybox" } }
                    },
                    {
                        "id": 1,
                        "name": "Sun",
                        "parent": null,
                        "transform": { "position": [0.0, 6.0, 0.0], "rotation": [-0.4082483, -0.28867513, 0.0, 0.8660254], "scale": [1.0, 1.0, 1.0] },
                        "components": { "prism_engine::scene::Light": { "light_type": "Directional", "intensity": 100000.0, "shadows": "Soft" } }
                    },
                    {
                        "id": 2,
                        "name": "PointLight",
                        "parent": null,
                        "transform": { "position": [3.0, 3.0, 3.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0] },
                        "components": { "prism_engine::scene::Light": { "intensity": 100.0, "range": 12.0 } }
                    }
                ]
            })),
        },
        TemplateDoc {
            id: "ui".to_string(),
            label: "UI Overlay".to_string(),
            kind: DocKind::Scene,
            doc: parse_scene(json!({
                "version": 3,
                "entities": [
                    {
                        "id": 0,
                        "name": "Canvas",
                        "parent": null,
                        "transform": { "position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0] },
                        "components": { "prism_engine::ui::Node": {}, "prism_engine::ui::Style": {} }
                    },
                    {
                        "id": 1,
                        "name": "Title",
                        "parent": 0,
                        "transform": { "position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0] },
                        "components": { "prism_engine::ui::Node": {}, "prism_engine::ui::Style": { "width": 900.0, "height": 108.0 }, "prism_engine::ui::Text": { "content": "Prisma", "font_size": 84.0, "alignment": "Center" } }
                    }
                ]
            })),
        },
        TemplateDoc {
            id: "empty".to_string(),
            label: "Empty".to_string(),
            kind: DocKind::Scene,
            doc: parse_scene(json!({
                "version": 3,
                "entities": [{
                    "id": 0,
                    "name": "Entity",
                    "parent": null,
                    "transform": { "position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0] },
                    "components": {}
                }]
            })),
        },
    ]
}

pub(super) fn hydrate_doc(schema: &Schema, doc: &mut SceneDoc) {
    for entity in &mut doc.entities {
        for (component_name, value) in entity.components.iter_mut() {
            let Some(component) = schema.component(component_name) else {
                continue;
            };
            let defaults = component.default_value();
            merge_missing(value, &defaults);
        }
    }
}

pub(super) fn merge_missing(target: &mut Value, defaults: &Value) {
    if let (Value::Object(target_map), Value::Object(default_map)) = (target, defaults) {
        for (key, value) in default_map {
            match target_map.get_mut(key) {
                Some(existing) => merge_missing(existing, value),
                None => {
                    target_map.insert(key.clone(), value.clone());
                }
            }
        }
    }
}

pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

pub(super) fn default_panels() -> Vec<PanelInfo> {
    let mut panels: Vec<PanelInfo> = [
        ("sceneView", "场景视图"),
        ("gameView", "游戏视图"),
        ("docs", "文档"),
        ("hierarchy", "层级"),
        ("assets", "资产"),
        ("inspector", "检查器"),
        ("log", "日志"),
        ("renderGraph", "渲染图"),
        ("projectSettings", "项目设置"),
        ("preferences", "首选项"),
        ("theme", "界面主题"),
        ("build", "构建项目"),
        ("tools", "离线工具"),
        ("history", "撤销历史"),
        ("shortcuts", "键盘快捷键"),
        ("about", "关于 Prisma Editor"),
    ]
    .iter()
    .map(|(id, title)| PanelInfo::new(id, title))
    .collect();
    panels.sort_by(|a, b| a.id.cmp(&b.id));
    panels
}

pub fn layout_preset_ids() -> Vec<(&'static str, &'static str)> {
    LAYOUT_PRESETS
        .iter()
        .map(|preset| (preset.id, preset.label))
        .collect()
}

impl Workspace {
    /// Restore the workspace from the editor fixture (`editor/public/demo.json`).
    /// The fixture only carries documents, templates and presets. The component
    /// schema is not part of it: the host installs the live projection of the
    /// `prism-scene` registry (`Workspace::schema`), so a new component never
    /// needs a fixture refresh to appear in the editor.
    pub fn load_fixture(value: &Value) -> Self {
        let mut workspace = Self::sample();

        if let Some(presets) = value.get("presets") {
            workspace.presets = EntityPreset::from_list(presets);
        }
        if let Some(assets) = value.get("assetInfos") {
            workspace.assets = crate::assets::parse_assets(assets);
        }
        workspace.tree = AssetTree::build(&workspace.assets);

        // Template documents (prefabs + scenes). Replace defaults if fixture supplies any.
        let mut loaded_templates = Vec::new();
        if let Some(template_docs) = value.get("templateDocs").and_then(|v| v.as_object()) {
            if let Some(labels) = value.get("templatesByKind").and_then(|v| v.as_object()) {
                for (kind_name, entries) in labels {
                    let kind = DocKind::parse(kind_name);
                    let Some(items) = entries.as_array() else {
                        continue;
                    };
                    for item in items {
                        let Some(id) = item.get("id").and_then(|v| v.as_str()) else {
                            continue;
                        };
                        let label = item
                            .get("label")
                            .and_then(|v| v.as_str())
                            .unwrap_or(id)
                            .to_string();
                        if let Some(doc_value) =
                            template_docs.get(kind_name).and_then(|docs| docs.get(id))
                        {
                            if let Ok(mut doc) =
                                serde_json::from_value::<SceneDoc>(doc_value.clone())
                            {
                                doc.normalize();
                                loaded_templates.push(TemplateDoc {
                                    id: id.to_string(),
                                    label,
                                    kind,
                                    doc,
                                });
                            }
                        }
                    }
                }
            }
        }
        if !loaded_templates.is_empty() {
            workspace.templates = loaded_templates;
        }

        let mut opened: Vec<Document> = Vec::new();
        if let Some(docs) = value.get("docs").and_then(|v| v.as_object()) {
            for (path, doc_value) in docs {
                let Ok(mut doc) = serde_json::from_value::<SceneDoc>(doc_value.clone()) else {
                    continue;
                };
                doc.normalize();
                let kind = if path.ends_with(".prefab.json") {
                    DocKind::Prefab
                } else {
                    DocKind::Scene
                };
                opened.push(Document::new(kind, Some(path.clone()), doc));
            }
        }
        if !opened.is_empty() {
            opened.sort_by(|a, b| {
                a.path
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.path.as_deref().unwrap_or(""))
            });
            workspace.docs = opened;
        }

        if let Some(windows) = value.get("windows").and_then(|v| v.as_array()) {
            let mut panels = Vec::new();
            for window in windows {
                let Some(id) = window.get("id").and_then(|v| v.as_str()) else {
                    continue;
                };
                panels.push(PanelInfo {
                    id: id.to_string(),
                    title: window
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or(id)
                        .to_string(),
                    menu_path: window
                        .get("menuPath")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    shortcut: window
                        .get("shortcut")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    utility: dock::UTILITY_TABS.contains(&id),
                });
            }
            if !panels.is_empty() {
                workspace.panels = panels;
            }
        }

        workspace.active = 0;
        workspace.log(
            LogSource::Editor,
            LogLevel::Info,
            format!(
                "已载入 fixture：{} 个文档 / {} 个资产",
                workspace.docs.len(),
                workspace.assets.len()
            ),
        );
        workspace
    }

    /// Install the component schema supplied by the host.
    /// Documents may already be open when the schema arrives, so installing it
    /// hydrates them: every field missing from the author JSON is filled from
    /// the type's `defaultJson`.
    pub fn set_schema(&mut self, schema: Schema) {
        self.schema = schema;
        self.hydrate_documents();
    }

    pub fn hydrate_documents(&mut self) {
        let schema = self.schema.clone();
        for document in &mut self.docs {
            hydrate_doc(&schema, &mut document.doc);
        }
        for template in &mut self.templates {
            hydrate_doc(&schema, &mut template.doc);
        }
    }

    pub fn document(&self) -> &Document {
        &self.docs[self.active.min(self.docs.len() - 1)]
    }

    pub fn document_mut(&mut self) -> &mut Document {
        let index = self.active.min(self.docs.len() - 1);
        &mut self.docs[index]
    }

    pub fn active_index(&self) -> usize {
        self.active.min(self.docs.len().saturating_sub(1))
    }

    pub fn set_active(&mut self, index: usize) {
        if index < self.docs.len() {
            self.active = index;
            self.clear_selection();
            self.expanded.clear();
            self.isolate = false;
        }
    }

    pub fn open_path(&mut self, path: &str) {
        match self
            .docs
            .iter()
            .position(|doc| doc.path.as_deref() == Some(path))
        {
            Some(index) => self.set_active(index),
            None => {
                let template = self
                    .templates
                    .iter()
                    .find(|template| template.id == path)
                    .cloned();
                if let Some(template) = template {
                    let mut template = template;
                    hydrate_doc(&self.schema, &mut template.doc);
                    let document = Document::new(
                        template.kind,
                        Some(format!(
                            "{}/{}{}",
                            template.kind.folder(),
                            template.id,
                            template.kind.suffix()
                        )),
                        template.doc,
                    );
                    self.docs.push(document);
                    self.active = self.docs.len() - 1;
                    self.clear_selection();
                    self.log(
                        LogSource::Editor,
                        LogLevel::Info,
                        format!("已打开模板 {path}"),
                    );
                }
            }
        }
    }

    pub fn new_document(&mut self, kind: DocKind, template_id: Option<&str>) {
        let template = template_id
            .and_then(|id| self.templates.iter().find(|template| template.id == id))
            .filter(|template| template.kind == kind)
            .cloned();
        let mut doc = template
            .as_ref()
            .map(|template| template.doc.clone())
            .unwrap_or_default();
        hydrate_doc(&self.schema, &mut doc);

        let path = template.as_ref().map(|template| {
            self.unique_document_path(kind, &template.id)
                .unwrap_or_else(|| format!("{}/{}{}", kind.folder(), template.id, kind.suffix()))
        });
        self.docs.push(Document::new(kind, path, doc));
        self.active = self.docs.len() - 1;
        self.clear_selection();
        self.expanded.clear();
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!("新建{}", kind.folder()),
        );
    }

    pub(super) fn unique_document_path(&self, kind: DocKind, title: &str) -> Option<String> {
        let root = self.project_root.as_ref()?;
        let taken: Vec<String> = self
            .docs
            .iter()
            .filter_map(|doc| doc.path.clone())
            .collect();
        Some(project::unique_save_path(root, kind, title, &taken))
    }

    pub fn close_document(&mut self, index: usize) {
        if self.docs.len() <= 1 || index >= self.docs.len() {
            return;
        }
        self.docs.remove(index);
        if index < self.active {
            self.active -= 1;
        }
        if self.active >= self.docs.len() {
            self.active = self.docs.len() - 1;
        }
        self.clear_selection();
    }

    pub fn mark_saved(&mut self) {
        let title = {
            let document = self.document_mut();
            document.mark_saved();
            document.title()
        };
        self.log(LogSource::Editor, LogLevel::Info, format!("已保存 {title}"));
    }

    pub fn edit<T>(
        &mut self,
        label: impl Into<String>,
        mutate: impl FnOnce(&mut Document) -> T,
    ) -> T {
        let before = self.document().doc.clone();
        let selection_before = self.selection_ids();
        let label = label.into();
        let result = {
            let document = self.document_mut();
            mutate(document)
        };
        let after = self.document().doc.clone();
        if before != after {
            let selection_after = selection_ids_in(&after, &self.selection);
            let document = self.document_mut();
            document.undo.push_snapshot_with_selection(
                label,
                before,
                Some(crate::undo::SelectionSnapshot {
                    before: selection_before,
                    after: selection_after,
                }),
                &after,
            );
            document.mark_dirty();
        }
        result
    }

    pub fn undo(&mut self) {
        let outcome = {
            let document = self.document_mut();
            let outcome = document.undo.undo(&mut document.doc);
            if let Some(ref outcome) = outcome {
                if outcome.transform_only {
                    document.mark_transform_dirty();
                } else {
                    document.mark_dirty();
                }
            }
            outcome
        };
        let Some(outcome) = outcome else {
            return;
        };

        self.restore_selection(outcome.selection);
    }

    pub fn redo(&mut self) {
        let outcome = {
            let document = self.document_mut();
            let outcome = document.undo.redo(&mut document.doc);
            if let Some(ref outcome) = outcome {
                if outcome.transform_only {
                    document.mark_transform_dirty();
                } else {
                    document.mark_dirty();
                }
            }
            outcome
        };
        let Some(outcome) = outcome else {
            return;
        };

        self.restore_selection(outcome.selection);
    }

    pub(super) fn restore_selection(&mut self, ids: Option<Vec<u32>>) {
        match ids {
            Some(ids) => {
                self.select_ids(&ids);
            }
            None => self.clamp_selection(),
        }
    }

    pub fn can_undo(&self) -> bool {
        self.document().undo.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.document().undo.can_redo()
    }

    pub fn begin_interaction(&mut self) {
        self.document_mut().undo.begin_interaction();
    }

    pub fn end_interaction(&mut self) {
        self.document_mut().undo.commit();
    }

    pub fn undo_history(&self) -> Vec<String> {
        self.document()
            .undo
            .history()
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    pub fn undo_to(&mut self, position: usize) -> usize {
        let mut undone = 0;
        for _ in 0..=position {
            if !self.can_undo() {
                break;
            }
            self.undo();
            undone += 1;
        }
        if undone > 0 {
            self.log(
                LogSource::Editor,
                LogLevel::Info,
                format!("撤销 {undone} 步"),
            );
        }
        undone
    }

    pub fn clear_history(&mut self) {
        self.document_mut().undo.clear();
        self.log(LogSource::Editor, LogLevel::Info, "已清空撤销历史");
    }

    pub fn history_entries(&self) -> Vec<(String, bool)> {
        self.document().undo.history_entries()
    }

    pub fn history_cursor(&self) -> usize {
        self.document().undo.history_cursor()
    }

    pub fn jump_history(&mut self, target: usize) -> usize {
        let cursor = self.history_cursor();
        let mut moved = 0;
        if target < cursor {
            for _ in target..cursor {
                if !self.can_undo() {
                    break;
                }
                self.undo();
                moved += 1;
            }
        } else {
            for _ in cursor..target {
                if !self.can_redo() {
                    break;
                }
                self.redo();
                moved += 1;
            }
        }
        if moved > 0 {
            self.log(
                LogSource::Editor,
                LogLevel::Info,
                format!("history jump: {} step(s)", moved),
            );
        }
        moved
    }

    pub fn panel(&self, id: &str) -> Option<&PanelInfo> {
        self.panels.iter().find(|panel| panel.id == id)
    }

    pub fn panel_title(&self, id: &str) -> String {
        match id {
            "sceneView" => self.i18n.t1("menu.sceneView"),
            "gameView" => self.i18n.t1("menu.gameView"),
            "docs" => self.i18n.t1("menu.docs"),
            "hierarchy" => self.i18n.t1("menu.hierarchy"),
            "assets" => self.i18n.t1("menu.assets"),
            "inspector" => self.i18n.t1("menu.inspector"),
            "log" => self.i18n.t1("menu.log"),
            "renderGraph" => self.i18n.t1("menu.renderGraph"),
            "projectSettings" => self.i18n.t1("menu.projectSettings"),
            "preferences" => self.i18n.t1("menu.preferences"),
            "theme" => self.i18n.t1("menu.theme"),
            "build" => self.i18n.t1("menu.buildPanel"),
            "tools" => self.i18n.t1("menu.tools"),
            "history" => self.i18n.t1("menu.history"),
            "shortcuts" => self.i18n.t1("menu.shortcuts"),
            "about" => self.i18n.t1("menu.about"),
            "problems" => self.i18n.t1("menu.problems"),
            "prefabOverrides" => self.i18n.t1("menu.prefabOverrides"),
            "material" => self.i18n.t1("menu.material"),
            "animGraph" => self.i18n.t1("menu.animationGraph"),
            "gaspDebug" => self.i18n.t1("menu.gaspDebug"),
            other => self
                .panel(other)
                .map(|panel| panel.title.clone())
                .unwrap_or_else(|| other.to_string()),
        }
    }

    pub fn known_panels(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.panels.iter().map(|panel| panel.id.clone()).collect();
        for id in dock::UTILITY_TABS {
            if !ids.iter().any(|existing| existing == id) {
                ids.push(id.to_string());
            }
        }
        ids
    }

    pub fn show_panel(&mut self, id: &str) {
        self.dock = dock::show_panel(&self.dock, id);
    }

    pub fn toggle_panel(&mut self, id: &str) {
        if let Some(leaf) = find_leaf(&self.dock, id) {
            let leaf_id = leaf.id().to_string();
            self.dock = dock::activate_tab(&self.dock, &leaf_id, id);
            return;
        }
        self.dock = dock::show_panel(&self.dock, id);
    }

    pub fn activate_panel(&mut self, leaf_id: &str, tab: &str) {
        self.dock = dock::activate_tab(&self.dock, leaf_id, tab);
    }

    pub fn toggle_maximized(&mut self, tab: &str) {
        self.maximized_tab = if self.maximized_tab.as_deref() == Some(tab) {
            None
        } else {
            Some(tab.to_string())
        };
    }

    pub fn restore_maximized(&mut self) {
        self.maximized_tab = None;
    }

    pub fn close_panel(&mut self, tab: &str) {
        if dock::collect_tabs(&self.dock).len() <= 1 {
            return;
        }
        if let Some(next) = remove_panel(&self.dock, tab) {
            self.dock = next;
        }
        self.floating.retain(|panel| panel.tab != tab);
        if self.maximized_tab.as_deref() == Some(tab) {
            self.maximized_tab = None;
        }
    }

    pub fn close_other_panels(&mut self, tab: &str) {
        if dock::find_leaf(&self.dock, tab).is_none() {
            return;
        }
        self.dock = dock::leaf([tab]);
        self.floating.clear();
        if self.maximized_tab.as_deref() != Some(tab) {
            self.maximized_tab = None;
        }
    }

    pub fn add_tab_to_leaf(&mut self, leaf_id: &str, tab: &str) {
        self.dock = insert_panel(&self.dock, leaf_id, tab, DockZone::Center, 0.5, None);
        self.floating.retain(|panel| panel.tab != tab);
    }

    pub fn dock_tab(&mut self, tab: &str, target_leaf: &str, zone: DockZone) {
        self.dock = insert_panel(&self.dock, target_leaf, tab, zone, 0.5, None);
        self.floating.retain(|panel| panel.tab != tab);
    }

    pub fn dock_tab_at_edge(&mut self, tab: &str, edge: DockEdge) {
        self.dock = dock::dock_panel_at_edge(Some(&self.dock), tab, edge, 0.25);
        self.floating.retain(|panel| panel.tab != tab);
    }

    pub fn float_panel(&mut self, tab: &str, x: f32, y: f32) {
        self.dock = remove_panel(&self.dock, tab).unwrap_or_else(|| leaf([tab]));
        let z = self.floating.iter().map(|panel| panel.z).max().unwrap_or(0) + 1;
        let panel = dock::clamp_float_panel(
            FloatPanel {
                tab: tab.to_string(),
                x,
                y,
                width: 340.0,
                height: 380.0,
                z,
            },
            1600.0,
            900.0,
        );
        self.floating.push(panel);
    }

    pub fn close_floating(&mut self, tab: &str) {
        self.floating.retain(|panel| panel.tab != tab);
    }

    pub fn raise_floating(&mut self, tab: &str) {
        let z = self.floating.iter().map(|panel| panel.z).max().unwrap_or(0) + 1;
        if let Some(panel) = self.floating.iter_mut().find(|panel| panel.tab == tab) {
            panel.z = z;
        }
    }

    pub fn set_layout_preset(&mut self, id: &str) {
        if let Some(node) = dock::layout_preset(id) {
            self.dock = node;
            self.floating.clear();
            self.maximized_tab = None;
            self.layout_preset = id.to_string();
        }
    }

    pub fn layout_json(&self) -> Value {
        dock::DockLayout {
            version: dock::DockLayout::VERSION,
            dock: self.dock.clone(),
            floating: self.floating.clone(),
        }
        .to_json()
    }

    pub fn colors(&self) -> ThemeColors {
        self.theme.colors()
    }

    pub fn color(&self, token: ThemeToken) -> crate::theme::Rgba {
        self.colors().color(token)
    }

    pub fn set_theme_preset(&mut self, id: &str) {
        self.theme.set_preset(id);
    }

    pub fn set_locale(&mut self, locale: Locale) {
        self.i18n.set_locale(locale);
    }

    pub fn t(&self, key: &str) -> String {
        self.i18n.t1(key)
    }

    /// Interpolates `{param}` placeholders, e.g. `tf("assets.deleteItems", &[("count", "3")])`.
    pub fn tf(&self, key: &str, params: &[(&str, &str)]) -> String {
        self.i18n.t(key, params)
    }

    pub fn log(&mut self, source: LogSource, level: LogLevel, text: impl Into<String>) {
        self.log.push(source, level, text);
    }

    pub fn play(&mut self) {
        if self.playing {
            return;
        }

        self.play_snapshot = Some(self.take_play_snapshot());

        if self.maximize_on_play {
            self.play_maximized_before = Some(self.maximized_tab.clone());
            self.maximized_tab = Some("gameView".to_string());
        }
        self.playing = true;
        self.paused = false;
        self.play_time_s = 0.0;
        self.log(LogSource::Editor, LogLevel::Info, "进入 Play 模式");
    }

    pub fn stop(&mut self) {
        let restored = self.play_snapshot.take().map(|snapshot| {
            self.restore_play_snapshot(snapshot);
        });
        if let Some(previous) = self.play_maximized_before.take() {
            self.maximized_tab = previous;
        }
        self.playing = false;
        self.paused = false;
        self.play_time_s = 0.0;
        if restored.is_some() {
            self.log(
                LogSource::Editor,
                LogLevel::Info,
                "停止 Play：已丢弃播放期间的编辑，恢复进入前的文档",
            );
        } else {
            self.log(LogSource::Editor, LogLevel::Info, "停止 Play");
        }
    }

    pub fn has_play_snapshot(&self) -> bool {
        self.play_snapshot.is_some()
    }

    pub(super) fn take_play_snapshot(&self) -> PlaySnapshot {
        let document = &self.docs[self.active];
        PlaySnapshot {
            doc: self.active,
            scene: document.doc.clone(),
            undo: document.undo.clone(),
            dirty: document.dirty,
            selection: self.selection.clone(),
            selection_active: self.selection_active,
        }
    }

    pub(super) fn restore_play_snapshot(&mut self, snapshot: PlaySnapshot) {
        let Some(document) = self.docs.get_mut(snapshot.doc) else {
            return;
        };
        document.doc = snapshot.scene;
        document.undo = snapshot.undo;
        document.dirty = snapshot.dirty;
        document.mark_dirty();
        self.active = snapshot.doc;
        let entity_count = document.doc.entities.len();
        let mut selection = snapshot.selection;
        selection.retain(|index| *index < entity_count);
        self.selection = selection;
        self.selection_active = snapshot
            .selection_active
            .filter(|index| self.selection.contains(index))
            .or_else(|| self.selection.last().copied());
    }

    pub fn pause(&mut self) {
        if self.playing {
            self.paused = true;
        }
    }

    pub fn resume(&mut self) {
        self.paused = false;
    }

    pub fn step(&mut self) {
        if self.playing {
            self.paused = true;
            self.play_time_s += 1.0 / 60.0;
        }
    }

    pub fn tick(&mut self, dt: f32) {
        if self.playing && !self.paused {
            self.play_time_s += dt;
        }
    }

    pub fn play_state_label(&self) -> &'static str {
        if !self.playing {
            "编辑"
        } else if self.paused {
            "暂停"
        } else {
            "运行"
        }
    }

    pub fn run(&mut self, command: Command) {
        match command {
            Command::NewScene => self.new_document(DocKind::Scene, None),
            Command::NewPrefab => self.new_document(DocKind::Prefab, None),
            Command::OpenDocument => self.toggle_panel("docs"),
            Command::Save => match self.save_active() {
                Ok(path) => self.status = format!("已保存 {path}"),
                Err(error) => self.log(
                    LogSource::Editor,
                    LogLevel::Error,
                    format!("保存失败：{error}"),
                ),
            },
            Command::ReloadDocument => {
                if let Err(error) = self.reload_active() {
                    self.log(
                        LogSource::Editor,
                        LogLevel::Error,
                        format!("重新读取失败：{error}"),
                    );
                }
            }
            Command::RenameDocument | Command::DeleteDocument => {
                self.log(
                    LogSource::Editor,
                    LogLevel::Warn,
                    "重命名 / 删除请在资产面板或文件系统里做（编辑器暂不提供破坏性操作）",
                );
            }
            Command::Undo => self.undo(),
            Command::Redo => self.redo(),
            Command::DuplicateEntity => self.duplicate_selection(),
            Command::DeleteSelection => self.delete_selection(),
            Command::SelectAll => {
                self.selection = (0..self.document().doc.entities.len()).collect();
                self.selection_active = self.selection.last().copied();
            }
            Command::PasteComponent => self.paste_component(),
            Command::ClearComponentClipboard => {
                self.clipboard = None;
                self.status = "已清空组件剪贴板".to_string();
            }
            Command::ResetLayout => self.set_layout_preset("default"),
            Command::CopyEntity => {
                self.copy_selection();
            }
            Command::CutEntity => {
                self.cut_selection();
            }
            Command::PasteEntity => {
                self.paste_entities(None);
            }
            Command::BuildProject => {
                self.toggle_panel("build");
            }
            Command::CookScenes => {
                self.toggle_panel("build");
            }
            Command::Play => self.play(),
            Command::Pause => {
                if self.paused {
                    self.resume();
                } else {
                    self.pause();
                }
            }
            Command::Stop => self.stop(),
            Command::Step => self.step(),
            Command::FrameAll => self.frame_all(),
            Command::FrameSelection => self.frame_selection(),
            Command::ResetSceneCamera => self.camera = OrbitCamera::default(),
            other => {
                if let Some(panel) = other.panel_id() {
                    self.toggle_panel(panel);
                }
            }
        }
    }

    pub fn is_enabled(&self, command: Command) -> bool {
        match command {
            Command::Undo => self.can_undo(),
            Command::Redo => self.can_redo(),
            Command::DuplicateEntity
            | Command::DeleteSelection
            | Command::CopyEntity
            | Command::CutEntity
            | Command::FrameSelection => !self.selection.is_empty(),
            Command::PasteEntity => self.entity_clipboard.is_some(),
            Command::PasteComponent => self.clipboard.is_some() && !self.selection.is_empty(),
            Command::ClearComponentClipboard => self.clipboard.is_some(),
            Command::Pause | Command::Stop | Command::Step => self.playing,
            Command::Play => !self.playing,
            Command::BuildProject | Command::CookScenes | Command::OfflineTools => true,
            _ => true,
        }
    }

    pub fn docked_tabs(&self) -> Vec<String> {
        collect_tabs(&self.dock)
    }

    pub fn dock_tabs(&self) -> Vec<String> {
        let mut tabs = collect_tabs(&self.dock);
        tabs.extend(self.floating.iter().map(|panel| panel.tab.clone()));
        tabs
    }

    pub fn leaf_of(&self, tab: &str) -> Option<String> {
        find_leaf(&self.dock, tab).map(|leaf| leaf.id().to_string())
    }

    pub fn leaf_by_id(&self, id: &str) -> Option<&DockNode> {
        find_leaf_by_id(&self.dock, id)
    }
}
