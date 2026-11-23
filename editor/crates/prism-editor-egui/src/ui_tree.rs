//! UI node tree for agent (like browser MCP DOM / accessibility snapshot).
//!
//! egui has no DOM, nor stable externally clickable control handles, so here does not guess coordinates: node tree
//! directly comes from **data model** (`Workspace` / `UiState`) + dock panel rectangles recorded each frame.
//! Each node has stable `id`, `ui.click` / `ui.set` go by id through exactly same code path as UI
//! (`Command` / `Workspace::select` / `Workspace::set_field`), no synthetic mouse events injected.
//!
//! Node id convention:
//!
//! | prefix | meaning | clickable | settable |
//! |---|---|---|---|
//! | `command:<Name>` | `Command` enum member (`Command::ALL`) | ✅ | — |
//! | `panel:<tab>` | panel in dock (`sceneView` / `hierarchy` / …) | ✅ | — |
//! | `doc:<index>` | opened document | ✅ | — |
//! | `entity:<id>` | hierarchy tree entity | ✅ | — |
//! | `asset:<path>` | asset panel entry (select only) | ✅ | — |
//! | `open:<path>` | open / activate document for that asset (same as double-click) | ✅ | — |
//! | `field:<entity>:<component>:<path>` | inspector field | — | ✅ |
//! | `resolution:<WxH>` / `resolution:fit:<WxH>` / `resolution:match` | Game view preview resolution / aspect ratio (#463/#585) | ✅ | — |
//! | `dialog:<kind>` | modal dialog (`delete-entities` / `close` / …) | — | — |
//! | `dialog:<kind>:<button>` | modal dialog button (`delete` / `cancel` / `save` / …) | ✅ | — |

use std::collections::BTreeMap;

use prism_editor_core::{Command, Workspace};
use serde_json::{json, Value};

/// Runtime state needed for snapshot (all `EditorApp` fields, passed separately for testing).
pub struct SnapshotInput<'a> {
    pub workspace: &'a Workspace,
    pub playing: bool,
    pub paused: bool,
    pub status: &'a str,
    pub frames: u64,
    pub fps: f32,
    pub gizmo_tool: Option<&'a str>,
    pub gizmo_space: &'a str,
    /// `panel_body` records tab -> screen rectangle each frame.
    pub rects: &'a BTreeMap<String, egui::Rect>,
    /// leaf id -> currently active tab.
    pub leaves: &'a [(String, String, Vec<String>)],
    /// Log lines currently visible in Console (already user-filtered), at most first 50 lines.
    pub logs: &'a [String],
    /// Objects in Scene / Game viewport (world -> viewport pixel projection done).
    pub viewports: &'a [ViewObjects],
    /// Currently open modal dialog (#618).
    pub dialogs: &'a [Dialog],
}

/// One object in viewport: world coordinates + projected viewport pixel coordinates.
///
/// Node id is `view:<tab>:entity:<id>` (see [`view_object_node`]),
/// `ui.click` accepts it, click lands on projected point --- same picking path as user clicking same pixel in viewport.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewObject {
    /// Node id (`view:<tab>:entity:<scene id>` or `view:<tab>:ecs:<index>`).
    ///
    /// Entities spawned by project code have no stable id from `.scene.json`, fall back to ECS index,
    /// Two prefixes will not collide.
    pub node: String,
    /// Stable id from scene document; `None` = entity spawned at runtime.
    pub entity: Option<u32>,
    pub name: String,
    pub world: [f32; 3],
    /// Projected viewport local pixels (top-left origin); `None` = behind camera.
    pub screen: Option<[f32; 2]>,
    /// View-space depth (larger = further); `None` together with `screen`.
    pub depth: Option<f32>,
    /// Whether participates in rendering (Scene: `Active`; Game: `ActiveInHierarchy`).
    pub visible: bool,
}

/// Object list of one viewport.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewObjects {
    /// Panel tab name: `sceneView` / `gameView`.
    pub tab: &'static str,
    pub objects: Vec<ViewObject>,
}

/// One modal dialog visible to control channel (#618).
#[derive(Debug, Clone, PartialEq)]
pub struct Dialog {
    /// Stable id, e.g. `delete-entities` / `close` / `save-conflict`.
    pub id: String,
    pub title: String,
    pub message: String,
    /// Button: `(action, label)`; node id is `dialog:<id>:<action>`.
    pub buttons: Vec<(String, String)>,
    /// Current value for input-box dialogs (`None` for other dialogs).
    pub value: Option<String>,
}

impl Dialog {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            message: message.into(),
            buttons: Vec::new(),
            value: None,
        }
    }

    pub fn button(mut self, action: &str, label: &str) -> Self {
        self.buttons.push((action.to_string(), label.to_string()));
        self
    }

    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn node(&self) -> String {
        format!("dialog:{}", self.id)
    }

    pub fn button_node(&self, action: &str) -> String {
        format!("dialog:{}:{action}", self.id)
    }
}

/// `dialog:<id>:<action>` -> `(id, action)`; `dialog:<id>` returns `None`.
pub fn parse_dialog_node(node: &str) -> Option<(&str, Option<&str>)> {
    let rest = node.strip_prefix("dialog:")?;
    match rest.split_once(':') {
        Some((id, action)) if !id.is_empty() && !action.is_empty() => Some((id, Some(action))),
        Some(_) => None,
        None if !rest.is_empty() => Some((rest, None)),
        None => None,
    }
}

fn dialogs_json(dialogs: &[Dialog]) -> Value {
    Value::Array(
        dialogs
            .iter()
            .map(|dialog| {
                let buttons: Vec<Value> = dialog
                    .buttons
                    .iter()
                    .map(|(action, label)| {
                        json!({
                            "id": dialog.button_node(action),
                            "action": action,
                            "label": label,
                        })
                    })
                    .collect();
                let mut node = json!({
                    "id": dialog.node(),
                    "role": "dialog",
                    "title": dialog.title,
                    "message": dialog.message,
                    "buttons": buttons,
                });
                if let Some(value) = &dialog.value {
                    node["value"] = json!(value);
                }
                node
            })
            .collect(),
    )
}

/// Node id of viewport object (scene entity).
pub fn view_object_node(tab: &str, entity: u32) -> String {
    format!("view:{tab}:entity:{entity}")
}

/// Node id of viewport object (runtime-spawned entity, by ECS index).
pub fn view_object_node_ecs(tab: &str, index: u32) -> String {
    format!("view:{tab}:ecs:{index}")
}

/// `view:<tab>:entity:<id>` / `view:<tab>:ecs:<index>` -> `tab`.
///
/// Only accepts these two kinds, avoid treating arbitrary `view:*` nodes as objects.
pub fn parse_view_object_node(node: &str) -> Option<&str> {
    let rest = node.strip_prefix("view:")?;
    let mut parts = rest.split(':');
    let tab = parts.next()?;
    let kind = parts.next()?;
    let key = parts.next()?;
    if parts.next().is_some()
        || tab.is_empty()
        || (kind != "entity" && kind != "ecs")
        || key.parse::<u32>().is_err()
    {
        return None;
    }
    Some(tab)
}

/// Convert one viewport object into snapshot `objects` array.
fn view_objects_json(_tab: &str, objects: &[ViewObject]) -> Value {
    Value::Array(
        objects
            .iter()
            .map(|object| {
                let mut node = json!({
                    "id": object.node,
                    "role": "viewObject",
                    "entity": object.entity,
                    "name": object.name,
                    "world": object.world,
                    "visible": object.visible,
                    "on_screen": object.screen.is_some(),
                });
                if let Some(screen) = object.screen {
                    node["screen"] = json!(screen);
                }
                if let Some(depth) = object.depth {
                    node["depth"] = json!(depth);
                }
                node
            })
            .collect(),
    )
}

/// Node tree of whole editor UI.
pub fn snapshot(input: &SnapshotInput<'_>) -> Value {
    let workspace = input.workspace;
    let mut root = json!({
        "role": "editor",
        "status": input.status,
        "frames": input.frames,
        "fps": input.fps,
        "playing": input.playing,
        "paused": input.paused,
        "gizmo": {
            "tool": input.gizmo_tool,
            "space": input.gizmo_space,
        },
    });

    root["dialogs"] = dialogs_json(input.dialogs);
    root["pending_dialog"] = input
        .dialogs
        .first()
        .map(|dialog| json!(dialog.node()))
        .unwrap_or(Value::Null);

    let mut dock = Vec::new();
    for (leaf, active, tabs) in input.leaves {
        for tab in tabs {
            let mut node = json!({
                "id": format!("panel:{tab}"),
                "role": "panel",
                "label": panel_title(tab),
                "leaf": leaf,
                "tabs": tabs,
                "active": tab == active,
            });
            if let Some(rect) = input.rects.get(tab) {
                node["bounds"] = json!([rect.min.x, rect.min.y, rect.width(), rect.height()]);
            }
            if tab == active {
                node["children"] = panel_children(input, tab);
            }
            if let Some(objects) = input.viewports.iter().find(|view| view.tab == tab.as_str()) {
                node["objects"] = view_objects_json(tab, &objects.objects);
            }
            dock.push(node);
        }
    }
    root["dock"] = Value::Array(dock);

    root["documents"] = Value::Array(
        workspace
            .docs
            .iter()
            .enumerate()
            .map(|(index, doc)| {
                json!({
                    "id": format!("doc:{index}"),
                    "role": "document",
                    "label": doc
                        .path
                        .as_deref()
                        .map(file_name)
                        .unwrap_or_else(|| format!("untitled:{index}")),
                    "path": doc.path,
                    "dirty": doc.is_dirty(),
                    "active": index == workspace.active_index(),
                })
            })
            .collect(),
    );

    root["assets"] = asset_tree(input);
    root["commands"] = Value::Array(
        Command::ALL
            .iter()
            .map(|command| json!(command_name(*command)))
            .collect(),
    );
    root
}

/// Panel content nodes (only active tab expanded).
fn panel_children(input: &SnapshotInput<'_>, tab: &str) -> Value {
    let workspace = input.workspace;
    match tab {
        "hierarchy" => Value::Array(hierarchy_nodes(workspace)),
        "inspector" => Value::Array(inspector_components(workspace)),
        "docs" => Value::Array(
            workspace
                .docs
                .iter()
                .enumerate()
                .map(|(index, doc)| {
                    json!({
                        "id": format!("doc:{index}"),
                        "role": "document",
                        "label": doc.path.as_deref().map(file_name).unwrap_or_default(),
                        "dirty": doc.is_dirty(),
                        "active": index == workspace.active_index(),
                    })
                })
                .collect(),
        ),
        "log" => Value::Array(
            input
                .logs
                .iter()
                .rev()
                .take(50)
                .map(|line| json!({ "role": "log", "message": line }))
                .collect(),
        ),
        _ => Value::Array(Vec::new()),
    }
}

/// Hierarchy tree (in document order, with `depth`).
fn hierarchy_nodes(workspace: &Workspace) -> Vec<Value> {
    let doc = &workspace.document().doc;
    let mut out = Vec::new();
    for (index, entity) in doc.entities.iter().enumerate() {
        let id = entity.id.unwrap_or(index as u32);
        out.push(json!({
            "id": format!("entity:{id}"),
            "role": "entity",
            "label": entity.display_name(),
            "parent": entity.parent,
            "selected": workspace.selection.contains(&index),
            "components": entity.components.keys().cloned().collect::<Vec<_>>(),
        }));
    }
    out
}

/// Component fields of selected entity (paths settable via `ui.set`).
fn inspector_components(workspace: &Workspace) -> Vec<Value> {
    let doc = &workspace.document().doc;
    let mut out = Vec::new();
    for index in workspace.selection.iter().take(4) {
        let Some(entity) = doc.entities.get(*index) else {
            continue;
        };
        let entity_id = entity.id.unwrap_or(*index as u32);
        let mut components = Vec::new();
        components.push(component_node(
            entity_id,
            "Transform",
            &serde_json::to_value(&entity.transform).unwrap_or(Value::Null),
        ));
        for (name, value) in entity.components.iter() {
            components.push(component_node(entity_id, name, value));
        }
        out.push(json!({
            "id": format!("entity:{entity_id}"),
            "role": "entity",
            "label": entity.display_name(),
            "entity": entity_id,
            "children": components,
        }));
    }
    out
}

fn component_node(entity_id: u32, name: &str, value: &Value) -> Value {
    let mut fields = Vec::new();
    flatten(value, "", &mut |path, value| {
        fields.push(json!({
            "id": format!("field:{entity_id}:{name}:{path}"),
            "role": "field",
            "path": path,
            "value": value,
        }));
    });
    json!({
        "id": format!("component:{entity_id}:{name}"),
        "role": "component",
        "label": name,
        "children": fields,
    })
}

/// Flatten nested object into field paths.
///
/// Object uses dot notation (`position.x`), array **simultaneously** gives whole value and index
/// (`position` is `[x,y,z]`, also `position[0]` …), so agent can both replace whole group
/// or change single component (`prism_editor_core::schema::parse_path` recognizes `a.b[2]`).
fn flatten(value: &Value, prefix: &str, push: &mut impl FnMut(String, Value)) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten(child, &path, push);
            }
        }
        Value::Array(items) => {
            push(prefix.to_string(), value.clone());
            for (index, child) in items.iter().enumerate() {
                if child.is_array() || child.is_object() {
                    flatten(child, &format!("{prefix}[{index}]"), push);
                } else {
                    push(format!("{prefix}[{index}]"), child.clone());
                }
            }
        }
        other => push(prefix.to_string(), other.clone()),
    }
}

fn asset_tree(input: &SnapshotInput<'_>) -> Value {
    let workspace = input.workspace;
    let assets = workspace
        .filtered_assets()
        .into_iter()
        .map(|asset| {
            json!({
                "id": format!("asset:{}", asset.path),
                "role": "asset",
                "label": file_name(&asset.path),
                "path": asset.path,
                "kind": format!("{:?}", asset.kind).to_ascii_lowercase(),
            })
        })
        .collect::<Vec<_>>();
    json!({ "role": "panel", "children": assets })
}

fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

/// `panel:<tab>` -> UI title.
pub fn panel_title(tab: &str) -> &'static str {
    match tab {
        "sceneView" => "Scene",
        "gameView" => "Game",
        "hierarchy" => "Hierarchy",
        "inspector" => "Inspector",
        "assets" => "Assets",
        "docs" => "Documents",
        "log" => "Console",
        "history" => "History",
        "renderGraph" => "Render Graph",
        "build" => "Build Settings",
        "problems" => "Problems",
        "prefabOverrides" => "Prefab Overrides",
        "material" => "Material",
        "animGraph" => "Animation Graph",
        "tools" => "Offline Tools",
        "settings" => "Project Settings",
        "preferences" => "Preferences",
        "theme" => "Theme",
        _ => "Panel",
    }
}

/// Stable name of `Command` (consistent with `Debug`, agent clicks by this name).
pub fn command_name(command: Command) -> String {
    format!("{command:?}")
}

/// Name -> `Command` (`command:<Name>` of `ui.click`).
pub fn command_by_name(name: &str) -> Option<Command> {
    Command::ALL
        .iter()
        .copied()
        .find(|command| command_name(*command) == name)
}

/// Error hint exposed to `ui.set`: lists writable field paths in component.
pub fn flatten_paths(value: &Value, prefix: &str, push: &mut impl FnMut(&str, &Value)) {
    flatten(value, prefix, &mut |path, value| push(&path, &value));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_produces_dotted_paths() {
        let value = json!({ "position": { "x": 1.0, "y": 2 }, "tags": [1, 2] });
        let mut paths = Vec::new();
        flatten(&value, "", &mut |path, value| paths.push((path, value)));
        let names: Vec<&str> = paths.iter().map(|(path, _)| path.as_str()).collect();
        assert!(names.contains(&"position.x"));
        assert!(names.contains(&"position.y"));
        assert!(names.contains(&"tags"));
        assert!(names.contains(&"tags[0]"));
    }

    #[test]
    fn command_names_round_trip() {
        for command in Command::ALL {
            assert_eq!(command_by_name(&command_name(*command)), Some(*command));
        }
        assert_eq!(command_by_name("Nope"), None);
    }

    #[test]
    fn panel_titles_are_stable() {
        assert_eq!(panel_title("sceneView"), "Scene");
        assert_eq!(panel_title("hierarchy"), "Hierarchy");
    }

    #[test]
    fn snapshot_includes_gizmo_tool_and_space() {
        let workspace = Workspace::sample();
        let rects = BTreeMap::new();
        let leaves = Vec::new();
        let logs = Vec::new();
        let input = SnapshotInput {
            workspace: &workspace,
            playing: false,
            paused: false,
            status: "ready",
            frames: 3,
            fps: 60.0,
            gizmo_tool: Some("Move"),
            gizmo_space: "World",
            rects: &rects,
            leaves: &leaves,
            logs: &logs,
            viewports: &[],
            dialogs: &[],
        };
        let value = snapshot(&input);
        assert_eq!(value["gizmo"]["tool"], "Move");
        assert_eq!(value["gizmo"]["space"], "World");
        assert_eq!(value["fps"], 60.0);
    }

    #[test]
    fn view_object_nodes_round_trip() {
        assert_eq!(view_object_node("sceneView", 2), "view:sceneView:entity:2");
        assert_eq!(view_object_node_ecs("gameView", 9), "view:gameView:ecs:9");
        assert_eq!(
            parse_view_object_node("view:gameView:entity:17"),
            Some("gameView")
        );
        assert_eq!(
            parse_view_object_node("view:gameView:ecs:17"),
            Some("gameView")
        );
        assert_eq!(parse_view_object_node("panel:sceneView"), None);
        assert_eq!(parse_view_object_node("view:sceneView:entity:x"), None);
        assert_eq!(parse_view_object_node("view:sceneView"), None);
        assert_eq!(parse_view_object_node("view::entity:1"), None);
        assert_eq!(parse_view_object_node("view:sceneView:created:1"), None);
    }

    #[test]
    fn snapshot_exposes_modal_dialogs_and_buttons() {
        let workspace = Workspace::sample();
        let rects = BTreeMap::new();
        let leaves = Vec::new();
        let logs = Vec::new();
        let dialogs = vec![Dialog::new(
            "delete-entities",
            "Delete",
            "Delete 5 selected entities and their children?",
        )
        .button("delete", "Delete")
        .button("cancel", "Cancel")];
        let input = SnapshotInput {
            workspace: &workspace,
            playing: false,
            paused: false,
            status: "ready",
            frames: 1,
            fps: 60.0,
            gizmo_tool: None,
            gizmo_space: "World",
            rects: &rects,
            leaves: &leaves,
            logs: &logs,
            viewports: &[],
            dialogs: &dialogs,
        };
        let value = snapshot(&input);
        assert_eq!(value["pending_dialog"], "dialog:delete-entities");
        assert_eq!(value["dialogs"][0]["role"], "dialog");
        assert_eq!(
            value["dialogs"][0]["buttons"][0]["id"],
            "dialog:delete-entities:delete"
        );
        assert_eq!(
            parse_dialog_node("dialog:delete-entities:delete"),
            Some(("delete-entities", Some("delete")))
        );
        assert_eq!(
            parse_dialog_node("dialog:delete-entities"),
            Some(("delete-entities", None))
        );
        assert_eq!(parse_dialog_node("delete-entities:delete"), None);
    }

    #[test]
    fn snapshot_lists_viewport_objects() {
        let workspace = Workspace::sample();
        let mut rects = BTreeMap::new();
        rects.insert(
            "sceneView".to_string(),
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0)),
        );
        let leaves = vec![(
            "leaf-1".to_string(),
            "sceneView".to_string(),
            vec!["sceneView".to_string()],
        )];
        let logs = Vec::new();
        let viewports = vec![ViewObjects {
            tab: "sceneView",
            objects: vec![ViewObject {
                node: view_object_node("sceneView", 2),
                entity: Some(2),
                name: "boat".into(),
                world: [13.0, 80.0, 0.0],
                screen: Some([400.0, 300.0]),
                depth: Some(5.0),
                visible: true,
            }],
        }];
        let input = SnapshotInput {
            workspace: &workspace,
            playing: false,
            paused: false,
            status: "ready",
            frames: 1,
            fps: 60.0,
            gizmo_tool: None,
            gizmo_space: "World",
            rects: &rects,
            leaves: &leaves,
            logs: &logs,
            viewports: &viewports,
            dialogs: &[],
        };
        let value = snapshot(&input);
        let node = value["dock"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == "panel:sceneView")
            .expect("scene view node");
        let objects = node["objects"].as_array().expect("objects array");
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0]["id"], "view:sceneView:entity:2");
        assert_eq!(objects[0]["name"], "boat");
        assert_eq!(objects[0]["screen"], serde_json::json!([400.0, 300.0]));
        assert_eq!(objects[0]["on_screen"], true);
    }
}
