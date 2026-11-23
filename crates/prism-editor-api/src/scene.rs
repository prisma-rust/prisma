//! Authoring document (`.scene.json` / `.prefab.json`) location, enumeration, read/write and validation.
//!
//! Scene and prefab **share the same document shape** ([`prism_scene::SceneJson`]: `{version,
//! entities}`), so here uses [`DocKind`] to distinguish directory and suffix, remaining logic (normalized reading,
//! structure validation, save/rename/delete) completely shared. Before write-back validated with same schema,
//! avoid producing documents cooker cannot consume.

use std::path::{Path, PathBuf};

use prism_asset::GuidIndex;
use prism_scene::references::{sync_document, SyncMode};
use prism_scene::SceneJson;
use serde::Serialize;

/// Document kind. Authoring JSON shape same, only directory and filename suffix differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocKind {
    /// Scene: `assets/scenes/*.scene.json`
    Scene,
    /// Prefab: `assets/prefabs/*.prefab.json`
    Prefab,
}

impl DocKind {
    /// Subdirectory relative to project root.
    pub fn subdir(self) -> &'static str {
        match self {
            Self::Scene => "assets/scenes",
            Self::Prefab => "assets/prefabs",
        }
    }

    /// File name suffix (without leading dot).
    pub fn ext(self) -> &'static str {
        match self {
            Self::Scene => "scene.json",
            Self::Prefab => "prefab.json",
        }
    }

    /// Chinese label (for error messages).
    pub fn label(self) -> &'static str {
        match self {
            Self::Scene => "Scene",
            Self::Prefab => "Prefab",
        }
    }

    /// Frontend passes string → kind.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "scene" => Ok(Self::Scene),
            "prefab" => Ok(Self::Prefab),
            other => Err(format!("Unknown document type '{other}'")),
        }
    }

    /// Infer kind from path (used when rename only needs path).
    pub fn of_path(path: &Path) -> Self {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.ends_with(&format!(".{}", Self::Prefab.ext())) {
            Self::Prefab
        } else {
            Self::Scene
        }
    }
}

/// Scene subdirectory under `assets/` (relative to project root).
pub const SCENES_SUBDIR: &str = "assets/scenes";

/// Scene file extension.
pub const SCENE_EXT: &str = "scene.json";

/// A document file openable in editor.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SceneEntry {
    /// Logical name (suffix stripped, e.g. `intro`).
    pub name: String,
    /// File name (with suffix, e.g. `intro.scene.json`).
    pub file_name: String,
    /// Absolute path (passed back to read/write as-is by frontend).
    pub path: String,
}

/// Directory where a kind of document is located.
pub fn docs_dir(project_root: &Path, kind: DocKind) -> PathBuf {
    project_root.join(kind.subdir())
}

/// `assets/scenes` directory.
pub fn scenes_dir(project_root: &Path) -> PathBuf {
    docs_dir(project_root, DocKind::Scene)
}

/// Find project root containing `assets/scenes` by walking up from current working directory.
///
/// Editor / MCP CWD may not be at repo root, so walk up level by level;
/// Return error if not found, frontend can prompt user to specify manually.
#[expect(
    clippy::disallowed_methods,
    reason = "editor/MCP project root discovery walks up from cwd"
)]
pub fn find_project_root() -> Result<PathBuf, String> {
    let mut dir =
        std::env::current_dir().map_err(|e| format!("Cannot get current directory: {e}"))?;
    for _ in 0..12 {
        if scenes_dir(&dir).is_dir() {
            return Ok(dir);
        }
        if !dir.pop() {
            break;
        }
    }
    Err("Cannot find project root containing assets/scenes".into())
}

/// Enumerate a kind of document, sorted by logical name. Empty when directory does not exist (prefab directory may not exist yet).
///
/// Prefabs may live anywhere under `assets/` (the hierarchy drag-and-drop
/// target is an arbitrary asset folder), so prefab discovery is recursive;
/// files under `assets/prefabs` keep their plain names, while files elsewhere
/// use their `assets/`-relative stem (e.g. `props/chair`).
pub fn list_docs(project_root: &Path, kind: DocKind) -> Result<Vec<SceneEntry>, String> {
    if kind == DocKind::Prefab {
        let assets = project_root.join("assets");
        if !assets.is_dir() {
            return Ok(Vec::new());
        }
        let mut docs = Vec::new();
        collect_docs(&assets, &assets, kind, &mut docs)?;
        docs.sort_by(|a, b| a.name.cmp(&b.name));
        return Ok(docs);
    }

    let dir = docs_dir(project_root, kind);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let read =
        std::fs::read_dir(&dir).map_err(|e| format!("Cannot read {}: {e}", dir.display()))?;

    let mut docs = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(name) = file_name.strip_suffix(&format!(".{}", kind.ext())) else {
            continue;
        };
        docs.push(SceneEntry {
            name: name.to_string(),
            file_name: file_name.to_string(),
            path: path.to_string_lossy().into_owned(),
        });
    }
    docs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(docs)
}

fn collect_docs(
    assets_root: &Path,
    dir: &Path,
    kind: DocKind,
    docs: &mut Vec<SceneEntry>,
) -> Result<(), String> {
    let read = std::fs::read_dir(dir).map_err(|e| format!("Cannot read {}: {e}", dir.display()))?;
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if matches!(name, "cooked" | "heightmaps" | "target") || name.starts_with('.') {
                continue;
            }
            collect_docs(assets_root, &path, kind, docs)?;
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(stem) = file_name.strip_suffix(&format!(".{}", kind.ext())) else {
            continue;
        };
        let rel = path
            .strip_prefix(assets_root)
            .ok()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|| stem.to_string());
        let name = rel
            .trim_end_matches(&format!(".{}", kind.ext()))
            .trim_start_matches("prefabs/")
            .to_string();
        docs.push(SceneEntry {
            name,
            file_name: file_name.to_string(),
            path: path.to_string_lossy().into_owned(),
        });
    }
    Ok(())
}

/// Enumerate all `.scene.json`.
pub fn list_scenes(project_root: &Path) -> Result<Vec<SceneEntry>, String> {
    list_docs(project_root, DocKind::Scene)
}

/// Read authoring document and **normalize** to complete JSON.
///
/// Round-trip once via [`prism_scene::SceneJson`] so missing optional fields (`transform` /
/// `components` etc) fall to serde defaults, shape frontend gets always complete.
pub fn read_doc(path: &Path) -> Result<serde_json::Value, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let scene = SceneJson::parse(&text)
        .map_err(|e| format!("{} JSON parse failed: {e}", path.display()))?;
    serde_json::to_value(&scene).map_err(|e| e.to_string())
}

/// [`read_doc`] scene alias.
pub fn read_scene_doc(path: &Path) -> Result<serde_json::Value, String> {
    read_doc(path)
}

/// Find the project root containing `path` by walking up to the directory
/// that has an `assets/` child.
pub fn project_root_for_doc(path: &Path) -> Option<PathBuf> {
    let mut dir = if path.is_dir() { path } else { path.parent()? };
    loop {
        if dir.join("assets").is_dir() {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

fn doc_relative_to_assets(assets_root: &Path, path: &Path) -> String {
    path.strip_prefix(assets_root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| {
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string()
        })
}

fn sync_doc_with_project(
    project_root: &Path,
    path: &Path,
    doc: &serde_json::Value,
    mode: SyncMode,
) -> serde_json::Value {
    let assets_root = project_root.join("assets");
    let index = GuidIndex::scan(&assets_root);
    let doc_rel = doc_relative_to_assets(&assets_root, path);
    let mut doc = doc.clone();
    sync_document(
        &mut doc,
        &doc_rel,
        mode,
        |guid| index.path_for_guid(guid).map(str::to_string),
        |asset| index.guid_for_path(asset).map(str::to_string),
    );
    doc
}

/// Read an authoring document and resolve its GUID references (GUID wins,
/// path fallback). Old documents without GUIDs are unchanged.
pub fn read_doc_in_project(project_root: &Path, path: &Path) -> Result<serde_json::Value, String> {
    let doc = read_doc(path)?;
    Ok(sync_doc_with_project(
        project_root,
        path,
        &doc,
        SyncMode::Load,
    ))
}

/// Resolve GUIDs and fill/refresh GUID fields before writing.
pub fn sync_doc_for_save(
    project_root: &Path,
    path: &Path,
    doc: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let normalized = prism_scene::normalize_scene_doc(doc)?;
    Ok(sync_doc_with_project(
        project_root,
        path,
        &normalized,
        SyncMode::Save,
    ))
}

/// Write an authoring document with both paths and GUIDs.
pub fn write_doc_in_project(
    project_root: &Path,
    path: &Path,
    doc: &serde_json::Value,
) -> Result<(), String> {
    let synced = sync_doc_for_save(project_root, path, doc)?;
    write_doc(path, &synced)
}

/// Validate authoring document: shape + structural invariants (parent index out of bounds / self-parent / empty document).
pub fn validate_doc(doc: &serde_json::Value) -> Result<(), String> {
    let scene = SceneJson::from_value(doc.clone())
        .map_err(|e| format!("Not a valid authoring document structure: {e}"))?;
    scene.validate()
}

/// [`validate_doc`] scene alias.
pub fn validate_scene_doc(doc: &serde_json::Value) -> Result<(), String> {
    validate_doc(doc)
}

/// Validate and write back authoring document (2-space indent + trailing newline).
///
/// Written to disk is **normalized** document: fill entity `id`, upgrade to
/// [`prism_scene::SCENE_DOC_VERSION`], strip derived components, not raw input.
pub fn write_doc(path: &Path, doc: &serde_json::Value) -> Result<(), String> {
    let doc = &prism_scene::normalize_scene_doc(doc)?;
    validate_doc(doc)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Cannot create {}: {e}", parent.display()))?;
    }
    let mut text = serde_json::to_string_pretty(doc).map_err(|e| e.to_string())?;
    text.push('\n');
    std::fs::write(path, text).map_err(|e| format!("Write {} failed: {e}", path.display()))
}

/// [`write_doc`] scene alias.
pub fn write_scene_doc(path: &Path, doc: &serde_json::Value) -> Result<(), String> {
    write_doc(path, doc)
}

/// Validate logical name and construct full path under a kind of document directory.
pub fn doc_path(root: &Path, kind: DocKind, name: &str) -> Result<PathBuf, String> {
    doc_file_in(&docs_dir(root, kind), kind, name)
}

fn doc_file_in(dir: &Path, kind: DocKind, name: &str) -> Result<PathBuf, String> {
    let trimmed = name.trim();
    let label = kind.label();
    if trimmed.is_empty() {
        return Err(format!("{label} name cannot be empty"));
    }
    const ILLEGAL: [char; 9] = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];
    if trimmed.contains("..") || trimmed.contains(ILLEGAL) {
        return Err(format!("Illegal {label} name '{trimmed}'"));
    }
    let ext = kind.ext();
    let file = if trimmed.ends_with(&format!(".{ext}")) {
        trimmed.to_string()
    } else if let Some(stem) = trimmed.strip_suffix(".json") {
        format!("{stem}.{ext}")
    } else {
        format!("{trimmed}.{ext}")
    };
    Ok(dir.join(file))
}

/// [`doc_path`] scene alias.
pub fn scene_path(root: &Path, name: &str) -> Result<PathBuf, String> {
    doc_path(root, DocKind::Scene, name)
}

/// Write a document into an explicit directory (used by prefab drag-drop).
pub fn write_doc_in_dir(
    dir: &Path,
    kind: DocKind,
    name: &str,
    doc: &serde_json::Value,
) -> Result<PathBuf, String> {
    let path = doc_file_in(dir, kind, name)?;
    if path.exists() {
        return Err(format!(
            "Same-named {} already exists: {}",
            kind.label(),
            path.display()
        ));
    }
    write_doc(&path, doc)?;
    Ok(path)
}

/// Save as with new name (do not overwrite existing same-named file), return new path.
pub fn write_doc_as(
    root: &Path,
    kind: DocKind,
    name: &str,
    doc: &serde_json::Value,
) -> Result<PathBuf, String> {
    write_doc_in_dir(&docs_dir(root, kind), kind, name, doc)
}

/// [`write_doc_as`] scene alias.
pub fn write_scene_as(root: &Path, name: &str, doc: &serde_json::Value) -> Result<PathBuf, String> {
    write_doc_as(root, DocKind::Scene, name, doc)
}

/// Rename document (within same directory), return new path.
pub fn rename_doc(path: &Path, kind: DocKind, new_name: &str) -> Result<PathBuf, String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("{} path has no parent directory", kind.label()))?;
    let target = doc_file_in(dir, kind, new_name)?;
    if target == path {
        return Ok(target);
    }
    if target.exists() {
        return Err(format!(
            "Same-named {} already exists: {}",
            kind.label(),
            target.display()
        ));
    }
    std::fs::rename(path, &target).map_err(|e| format!("Rename failed: {e}"))?;
    Ok(target)
}

/// [`rename_doc`] scene alias (kind inferred from path suffix).
pub fn rename_scene(path: &Path, new_name: &str) -> Result<PathBuf, String> {
    rename_doc(path, DocKind::of_path(path), new_name)
}

/// Delete document file.
pub fn delete_doc(path: &Path) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|e| format!("Delete {} failed: {e}", path.display()))
}

/// [`delete_doc`] scene alias.
pub fn delete_scene(path: &Path) -> Result<(), String> {
    delete_doc(path)
}

/// A built-in scene template.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SceneTemplate {
    pub id: String,
    pub label: String,
}

/// All built-in templates (order is UI display order).
pub fn templates() -> Vec<SceneTemplate> {
    [
        ("camera", "Camera"),
        ("lit", "Camera + Lighting"),
        ("ui", "UI Overlay"),
        ("empty", "Empty Entity"),
    ]
    .into_iter()
    .map(|(id, label)| SceneTemplate {
        id: id.to_string(),
        label: label.to_string(),
    })
    .collect()
}

/// Initial document for new scene (`camera` template: camera entity at `[0, 0, 10]`).
pub fn empty_scene_doc() -> serde_json::Value {
    template_scene_doc("camera")
}

/// Generate scene document by template id; unknown id falls back to `camera`.
///
/// Component defaults from `prism-scene` registry (`default_json`), template only covers difference fields,
/// avoid drifting from component definitions.
pub fn template_scene_doc(id: &str) -> serde_json::Value {
    use serde_json::json;

    let camera = || component_default("prism_engine::scene::Camera");

    let entities = match id {
        "empty" => vec![entity("Entity", None, [0.0, 0.0, 0.0], json!({}))],
        "lit" => vec![
            entity(
                "Camera",
                None,
                [0.0, 2.5, 18.0],
                json!({
                    "prism_engine::scene::Camera": camera(),
                }),
            ),
            entity(
                "Sun",
                None,
                [0.0, 6.0, 0.0],
                json!({
                    "prism_engine::scene::DirectionalLight":
                        component_default("prism_engine::scene::DirectionalLight"),
                }),
            ),
            entity(
                "PointLight",
                None,
                [3.0, 3.0, 3.0],
                json!({
                    "prism_engine::scene::PointLight":
                        component_default("prism_engine::scene::PointLight"),
                }),
            ),
        ],
        "ui" => vec![
            entity(
                "overlay",
                None,
                [0.0, 0.0, 0.0],
                json!({
                    "prism_engine::ui::Node": component_default("prism_engine::ui::Node"),
                    "prism_engine::ui::Style": merge(
                        component_default("prism_engine::ui::Style"),
                        json!({ "background": [0.0, 0.0, 0.0, 1.0] }),
                    ),
                }),
            ),
            entity(
                "title",
                Some(0),
                [0.0, 0.0, 0.0],
                json!({
                    "prism_engine::ui::Node": component_default("prism_engine::ui::Node"),
                    "prism_engine::ui::Style": merge(
                        component_default("prism_engine::ui::Style"),
                        json!({
                            "width": 900.0,
                            "height": 108.0,
                            "anchors": { "min_x": 0.5, "min_y": 0.5, "max_x": 0.5, "max_y": 0.5 },
                            "pivot": { "x": 0.5, "y": 0.5 },
                        }),
                    ),
                    "prism_engine::ui::Text": merge(
                        component_default("prism_engine::ui::Text"),
                        json!({
                            "content": "Prisma",
                            "font_size": 84.0,
                            "color": [0.91, 0.925, 0.957, 1.0],
                            "alignment": "Center",
                        }),
                    ),
                }),
            ),
        ],
        _ => vec![entity(
            "Camera",
            None,
            [0.0, 0.0, 10.0],
            json!({
                "prism_engine::scene::Camera": camera(),
            }),
        )],
    };

    normalize(json!({ "version": prism_scene::SCENE_DOC_VERSION, "entities": entities }))
}

/// Fill entity `id` for template document (authoring documents must have id from v3).
fn normalize(doc: serde_json::Value) -> serde_json::Value {
    prism_scene::normalize_scene_doc(&doc).unwrap_or(doc)
}

fn component_default(name: &str) -> serde_json::Value {
    crate::build_schema()
        .type_by_name(name)
        .and_then(|ty| ty.default_json.clone())
        .unwrap_or_else(|| serde_json::json!({}))
}

fn merge(base: serde_json::Value, patch: serde_json::Value) -> serde_json::Value {
    match (base, patch) {
        (serde_json::Value::Object(mut base), serde_json::Value::Object(patch)) => {
            for (key, value) in patch {
                base.insert(key, value);
            }
            serde_json::Value::Object(base)
        }
        (_, patch) => patch,
    }
}

fn transform(position: [f32; 3]) -> serde_json::Value {
    serde_json::json!({
        "position": position,
        "rotation": [0.0, 0.0, 0.0, 1.0],
        "scale": [1.0, 1.0, 1.0],
    })
}

fn entity(
    name: &str,
    parent: Option<u32>,
    position: [f32; 3],
    components: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "parent": parent,
        "transform": transform(position),
        "components": components,
    })
}

/// Available templates for a kind of document.
pub fn templates_for(kind: DocKind) -> Vec<SceneTemplate> {
    match kind {
        DocKind::Scene => templates(),
        DocKind::Prefab => crate::presets::entity_presets()
            .into_iter()
            .map(|preset| SceneTemplate {
                id: preset.id,
                label: preset.label,
            })
            .collect(),
    }
}

/// Generate initial document by template.
pub fn template_doc(kind: DocKind, template: &str) -> serde_json::Value {
    match kind {
        DocKind::Scene => template_scene_doc(template),
        DocKind::Prefab => prefab_template_doc(template),
    }
}

/// Single-entity prefab document (component defaults from registry, consistent with "quick add entity").
pub fn prefab_template_doc(preset_id: &str) -> serde_json::Value {
    use serde_json::json;

    let presets = crate::presets::entity_presets();
    let preset = presets
        .iter()
        .find(|preset| preset.id == preset_id)
        .or_else(|| presets.iter().find(|preset| preset.id == "empty"));

    let Some(preset) = preset else {
        return json!({ "version": prism_scene::SCENE_DOC_VERSION, "entities": [] });
    };

    let mut components = serde_json::Map::new();
    for name in &preset.components {
        components.insert(name.clone(), component_default(name));
    }

    normalize(json!({
        "version": prism_scene::SCENE_DOC_VERSION,
        "entities": [
            entity(&preset.name, None, preset.position, serde_json::Value::Object(components)),
        ],
    }))
}

/// Prefab instantiation result.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InstantiateResult {
    /// Scene document after insertion.
    pub doc: serde_json::Value,
    /// Index of newly inserted entity in scene (by prefab order).
    pub inserted: Vec<usize>,
}

fn entities_of(doc: &serde_json::Value) -> Result<&Vec<serde_json::Value>, String> {
    doc.get("entities")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "Document missing entities array".to_string())
}

fn id_of(entity: &serde_json::Value) -> u32 {
    entity
        .get("id")
        .and_then(|value| value.as_u64())
        .unwrap_or(0) as u32
}

/// `parent` stores parent entity **id** (from v3), not array index.
fn parent_of(entity: &serde_json::Value) -> Option<u32> {
    entity
        .get("parent")
        .and_then(|value| value.as_u64())
        .map(|parent| parent as u32)
}

fn index_of_id(entities: &[serde_json::Value], id: u32) -> Option<usize> {
    entities.iter().position(|entity| id_of(entity) == id)
}

/// Unified entry for edit operations: first migrate to current schema (fill id / upgrade version / strip derived components).
fn normalized(doc: &serde_json::Value) -> Result<serde_json::Value, String> {
    prism_scene::normalize_scene_doc(doc)
}

fn next_entity_id(entities: &[serde_json::Value]) -> u32 {
    entities
        .iter()
        .map(id_of)
        .max()
        .map_or(0, |max| max.saturating_add(1))
}

/// Rebuild document with new entity array (keep `version`).
fn doc_with_entities(
    doc: &serde_json::Value,
    entities: Vec<serde_json::Value>,
) -> serde_json::Value {
    let version = doc
        .get("version")
        .cloned()
        .unwrap_or(serde_json::json!(prism_scene::SCENE_DOC_VERSION));
    serde_json::json!({ "version": version, "entities": entities })
}

/// Whether `node` is in `ancestor`'s subtree (walk up via parent id).
fn is_descendant(entities: &[serde_json::Value], node: usize, ancestor: usize) -> bool {
    let ancestor_id = id_of(&entities[ancestor]);
    let mut current = parent_of(&entities[node]);
    let mut guard = 0;
    while let Some(parent_id) = current {
        if parent_id == ancestor_id {
            return true;
        }
        guard += 1;
        if guard > entities.len() {
            return false;
        }
        current = index_of_id(entities, parent_id).and_then(|index| parent_of(&entities[index]));
    }
    false
}

/// Hierarchy operation result: new document + new index of entity in array.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MoveResult {
    pub doc: serde_json::Value,
    pub index: usize,
}

/// Change parent (does not move array position). `parent = None` means become root entity.
///
/// Reject self-parent and "attach to own descendant" (would create cycle).
pub fn reparent_doc(
    doc: &serde_json::Value,
    child: usize,
    parent: Option<usize>,
) -> Result<serde_json::Value, String> {
    let result = move_entity(doc, child, child, parent)?;
    Ok(result.doc)
}

/// Move `child` to array `insert_at` position (old index space, `len` = end),
/// at the same time set its parent to `parent` (old index space).
///
/// Other entities' parent indices remapped with array reshuffle, so return value is whole new document.
pub fn move_entity(
    doc: &serde_json::Value,
    child: usize,
    insert_at: usize,
    parent: Option<usize>,
) -> Result<MoveResult, String> {
    let doc = &normalized(doc)?;
    validate_doc(doc)?;
    let entities = entities_of(doc)?;
    let count = entities.len();

    if child >= count {
        return Err(format!("Entity index {child} out of bounds"));
    }
    if insert_at > count {
        return Err(format!("Insert position {insert_at} out of bounds"));
    }
    if let Some(parent) = parent {
        if parent >= count {
            return Err(format!("Parent entity index {parent} out of bounds"));
        }
        if parent == child {
            return Err("Cannot parent self".into());
        }
        if is_descendant(entities, parent, child) {
            return Err("Cannot attach to own descendant (would cycle)".into());
        }
    }

    let mut order: Vec<usize> = (0..count).collect();
    order.remove(child);
    let target = if insert_at > child {
        insert_at - 1
    } else {
        insert_at
    };
    order.insert(target.min(order.len()), child);

    let new_parent_id = parent.map(|index| id_of(&entities[index]));
    let new_index = target.min(order.len() - 1);

    let mut out = Vec::with_capacity(count);
    for old_index in &order {
        let mut entity = entities[*old_index].clone();
        if *old_index == child {
            entity["parent"] = match new_parent_id {
                Some(parent) => serde_json::json!(parent),
                None => serde_json::json!(null),
            };
        }
        out.push(entity);
    }

    Ok(MoveResult {
        doc: doc_with_entities(doc, out),
        index: new_index,
    })
}

/// Batch delete entities, and make deleted parents roots, remap remaining parent indices.
///
/// At least one entity must remain (consistent with validation).
pub fn delete_entities(
    doc: &serde_json::Value,
    indices: &[usize],
) -> Result<serde_json::Value, String> {
    let doc = &normalized(doc)?;
    validate_doc(doc)?;
    let entities = entities_of(doc)?;
    let count = entities.len();

    let mut removed = vec![false; count];
    for index in indices {
        if *index >= count {
            return Err(format!("Entity index {index} out of bounds"));
        }
        removed[*index] = true;
    }
    let remaining = removed.iter().filter(|flag| !**flag).count();
    if remaining == 0 {
        return Err("Must retain at least one entity".into());
    }

    let removed_ids: std::collections::HashSet<u32> = indices
        .iter()
        .map(|index| id_of(&entities[*index]))
        .collect();

    let mut out = Vec::with_capacity(remaining);
    for (old_index, entity) in entities.iter().enumerate() {
        if removed[old_index] {
            continue;
        }
        let mut entity = entity.clone();
        let new_parent = parent_of(&entity).filter(|parent| !removed_ids.contains(parent));
        entity["parent"] = match new_parent {
            Some(parent) => serde_json::json!(parent),
            None => serde_json::json!(null),
        };
        out.push(entity);
    }

    Ok(doc_with_entities(doc, out))
}

/// Component name where prefab instance link is stored.
pub const PREFAB_LINK_COMPONENT: &str = "prism_engine::scene::PrefabInstance";

/// Subtree index (DFS, parent before child, `root` at first).
/// New position of original array index after deleting entities in `removed`.
///
/// Only count deleted indices before it; caller must ensure `index` itself is not in `removed`.
fn index_after_removal(index: usize, removed: &[usize]) -> usize {
    index - removed.iter().filter(|removed| **removed < index).count()
}

fn subtree_indices(entities: &[serde_json::Value], root: usize) -> Vec<usize> {
    let mut order: Vec<usize> = Vec::new();
    let mut stack = vec![root];
    while let Some(index) = stack.pop() {
        if order.contains(&index) {
            continue;
        }
        order.push(index);
        let node_id = id_of(&entities[index]);
        for child in (0..entities.len()).rev() {
            if parent_of(&entities[child]) == Some(node_id) && !order.contains(&child) {
                stack.push(child);
            }
        }
    }
    order
}

/// Extract "root entity + all descendants" from scene as a prefab document.
///
/// Extracted root entity parent set to `null` (decide where to attach at instantiation time), internal parent-child indices remapped to
/// prefab's own index space; [`PREFAB_LINK_COMPONENT`] on root will be stripped
/// (prefab should not record "self is instance of which prefab").
pub fn extract_subtree(
    scene: &serde_json::Value,
    root: usize,
) -> Result<serde_json::Value, String> {
    let scene = &normalized(scene)?;
    validate_doc(scene)?;
    let entities = entities_of(scene)?;
    if root >= entities.len() {
        return Err(format!("Entity index {root} out of bounds"));
    }

    let order = subtree_indices(entities, root);
    let mapping: std::collections::HashMap<u32, u32> = order
        .iter()
        .enumerate()
        .map(|(new_id, old_index)| (id_of(&entities[*old_index]), new_id as u32))
        .collect();

    let mut extracted = Vec::with_capacity(order.len());
    for old_index in &order {
        let mut entity = entities[*old_index].clone();
        let parent = parent_of(&entity);
        let new_parent = parent.and_then(|parent| mapping.get(&parent).copied());
        entity["id"] = serde_json::json!(mapping[&id_of(&entity)]);
        entity["parent"] = match new_parent {
            Some(parent) => serde_json::json!(parent),
            None => serde_json::json!(null),
        };
        extracted.push(entity);
    }

    if let Some(first) = extracted.first_mut() {
        if let Some(components) = first
            .get_mut("components")
            .and_then(|value| value.as_object_mut())
        {
            components.remove(PREFAB_LINK_COMPONENT);
        }
    }

    Ok(serde_json::json!({ "version": prism_scene::SCENE_DOC_VERSION, "entities": extracted }))
}

/// Prefab instance link (for editor "reverse link").
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PrefabLink {
    /// Index of instance root entity in scene.
    pub index: usize,
    /// Prefab path (relative to project root).
    pub prefab: String,
}

/// Instantiate prefab into scene (append to end), return new document and insert index.
///
/// Prefab internal parent indices globally shifted to new position; prefab root entity attached to `parent`
/// (`None` = as scene root). Name deduplicated with existing scene entities (append `_2`, `_3`…).
/// When `link` non-empty, mark each prefab root with [`PrefabLink`] record.
pub fn instantiate_prefab(
    scene: &serde_json::Value,
    prefab: &serde_json::Value,
    parent: Option<u32>,
    link: Option<&str>,
) -> Result<InstantiateResult, String> {
    let scene = &normalized(scene)?;
    let prefab = &normalized(prefab)?;
    validate_doc(scene)?;
    validate_doc(prefab)?;

    let scene_entities = entities_of(scene)?.clone();
    let prefab_entities = entities_of(prefab)?.clone();
    if let Some(parent) = parent {
        if parent as usize >= scene_entities.len() {
            return Err(format!("Parent entity index {parent} out of bounds"));
        }
    }

    let version = scene
        .get("version")
        .cloned()
        .unwrap_or(serde_json::json!(prism_scene::SCENE_DOC_VERSION));
    let id_base = next_entity_id(&scene_entities);
    let id_map: std::collections::HashMap<u32, u32> = prefab_entities
        .iter()
        .enumerate()
        .map(|(offset, entity)| (id_of(entity), id_base + offset as u32))
        .collect();
    let root_parent_id = parent.map(|index| id_of(&scene_entities[index as usize]));

    let mut names: Vec<String> = scene_entities
        .iter()
        .filter_map(|entity| entity.get("name").and_then(|value| value.as_str()))
        .map(|name| name.to_string())
        .collect();

    let mut out = scene_entities;
    let mut inserted = Vec::with_capacity(prefab_entities.len());

    for entity in &prefab_entities {
        let mut entity = entity.clone();

        let base_name = entity
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("entity")
            .to_string();
        let mut name = base_name.clone();
        let mut suffix = 2;
        while names.contains(&name) {
            name = format!("{base_name}_{suffix}");
            suffix += 1;
        }
        names.push(name.clone());
        entity["name"] = serde_json::json!(name);

        let prefab_parent = parent_of(&entity);
        let new_parent = match prefab_parent {
            Some(prefab_parent) => id_map.get(&prefab_parent).copied(),
            None => root_parent_id,
        };
        entity["id"] = serde_json::json!(id_map[&id_of(&entity)]);
        entity["parent"] = match new_parent {
            Some(parent) => serde_json::json!(parent),
            None => serde_json::json!(null),
        };

        if prefab_parent.is_none() {
            if let Some(path) = link {
                if let Some(components) = entity
                    .get_mut("components")
                    .and_then(|value| value.as_object_mut())
                {
                    components.insert(
                        PREFAB_LINK_COMPONENT.to_string(),
                        serde_json::json!({ "prefab": path }),
                    );
                }
            }
        }

        inserted.push(out.len());
        out.push(entity);
    }

    Ok(InstantiateResult {
        doc: serde_json::json!({ "version": version, "entities": out }),
        inserted,
    })
}

/// List all prefab instance roots in document (reverse links).
pub fn prefab_instances(doc: &serde_json::Value) -> Vec<PrefabLink> {
    let doc = &normalized(doc).unwrap_or_else(|_| doc.clone());
    let Ok(entities) = entities_of(doc) else {
        return Vec::new();
    };
    entities
        .iter()
        .enumerate()
        .filter_map(|(index, entity)| {
            let prefab = entity
                .get("components")?
                .get(PREFAB_LINK_COMPONENT)?
                .get("prefab")?
                .as_str()?;
            Some(PrefabLink {
                index,
                prefab: prefab.to_string(),
            })
        })
        .collect()
}

/// Restore a instance root **back to** prefab's current content.
///
/// Delete entity's entire subtree, then re-instantiate `prefab` under original parent, keeping original name.
/// Rebuilt subtree **inserted back to original array position**: array order is hierarchy order, appending to end makes instance
/// jump to last sibling in hierarchy tree, and invalidate its index (and undo/selection depending on index).
pub fn revert_prefab_instance(
    scene: &serde_json::Value,
    root: usize,
    prefab: &serde_json::Value,
    link: Option<&str>,
) -> Result<MoveResult, String> {
    let scene = &normalized(scene)?;
    validate_doc(scene)?;
    let entities = entities_of(scene)?;
    if root >= entities.len() {
        return Err(format!("Entity index {root} out of bounds"));
    }

    let keep_name = entities[root]
        .get("name")
        .and_then(|value| value.as_str())
        .map(|name| name.to_string());
    let subtree = subtree_indices(entities, root);
    let parent = parent_of(&entities[root])
        .map(|id| {
            let index = index_of_id(entities, id)
                .ok_or_else(|| format!("Parent entity id {id} does not exist"))?;
            if subtree.contains(&index) {
                return Err(format!(
                    "Parent entity id {id} is in pending-deleted subtree (hierarchy cycle)"
                ));
            }
            Ok(index_after_removal(index, &subtree) as u32)
        })
        .transpose()?;
    let insert_at = index_after_removal(root, &subtree);

    let trimmed = delete_entities(scene, &subtree)?;
    let result = instantiate_prefab(&trimmed, prefab, parent, link)?;
    let first_new = *result
        .inserted
        .first()
        .ok_or_else(|| "Prefab has no entities, cannot restore".to_string())?;

    let mut out = entities_of(&result.doc)?.clone();
    let mut rebuilt = out.split_off(first_new);
    if let Some(name) = keep_name {
        if let Some(entity) = rebuilt.first_mut() {
            entity["name"] = serde_json::json!(name);
        }
    }
    out.splice(insert_at..insert_at, rebuilt);

    Ok(MoveResult {
        doc: doc_with_entities(&result.doc, out),
        index: insert_at,
    })
}

/// Break instance link (only remove record, do not touch entity content).
pub fn unlink_prefab_instance(
    scene: &serde_json::Value,
    root: usize,
) -> Result<serde_json::Value, String> {
    let scene = &normalized(scene)?;
    validate_doc(scene)?;
    let mut entities = entities_of(scene)?.clone();
    if root >= entities.len() {
        return Err(format!("Entity index {root} out of bounds"));
    }
    if let Some(components) = entities[root]
        .get_mut("components")
        .and_then(|value| value.as_object_mut())
    {
        components.remove(PREFAB_LINK_COMPONENT);
    }
    Ok(doc_with_entities(scene, entities))
}

/// Restore all instances referencing `prefab_path` in document to `prefab`'s content.
///
/// Process instance roots from large to small: deletion only affects later indices, so earlier roots remain valid.
pub fn sync_prefab_instances(
    scene: &serde_json::Value,
    prefab_path: &str,
    prefab: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let mut roots: Vec<usize> = prefab_instances(scene)
        .into_iter()
        .filter(|link| link.prefab == prefab_path)
        .map(|link| link.index)
        .collect();
    roots.sort_unstable_by(|a, b| b.cmp(a));

    let mut doc = scene.clone();
    for root in roots {
        doc = revert_prefab_instance(&doc, root, prefab, Some(prefab_path))?.doc;
    }
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_dir(tag: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "prism_editor_api_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(scenes_dir(&dir)).unwrap();
        dir
    }

    #[test]
    fn empty_scene_validates() {
        let doc = empty_scene_doc();
        validate_scene_doc(&doc).unwrap();
        assert_eq!(doc["entities"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn validate_rejects_empty_and_out_of_range_parents() {
        assert!(validate_scene_doc(
            &json!({ "version": prism_scene::SCENE_DOC_VERSION, "entities": [] })
        )
        .is_err());

        let bad = json!({
            "version": prism_scene::SCENE_DOC_VERSION,
            "entities": [
                { "name": "a", "parent": 5, "transform": {}, "components": {} }
            ]
        });
        assert!(validate_scene_doc(&bad).is_err());

        let self_parent = json!({
            "version": prism_scene::SCENE_DOC_VERSION,
            "entities": [
                { "name": "a", "parent": 0, "transform": {}, "components": {} }
            ]
        });
        assert!(validate_scene_doc(&self_parent).is_err());
    }

    #[test]
    fn list_read_write_roundtrip() {
        let root = temp_dir("roundtrip");
        let path = scenes_dir(&root).join("demo.scene.json");

        write_scene_doc(&path, &empty_scene_doc()).unwrap();

        let scenes = list_scenes(&root).unwrap();
        assert_eq!(scenes.len(), 1);
        assert_eq!(scenes[0].name, "demo");
        assert_eq!(scenes[0].file_name, "demo.scene.json");
        assert_eq!(Path::new(&scenes[0].path), path.as_path());

        let doc = read_scene_doc(&path).unwrap();
        validate_scene_doc(&doc).unwrap();

        let bad = json!({ "version": prism_scene::SCENE_DOC_VERSION, "entities": [] });
        assert!(write_scene_doc(&path, &bad).is_err());
        assert_eq!(read_scene_doc(&path).unwrap(), doc);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn all_templates_validate_and_round_trip() {
        let ids: Vec<String> = templates().into_iter().map(|t| t.id).collect();
        assert!(ids.contains(&"camera".to_string()));
        assert!(ids.contains(&"lit".to_string()));
        assert!(ids.contains(&"ui".to_string()));

        for id in ids {
            let doc = template_scene_doc(&id);
            validate_scene_doc(&doc).unwrap_or_else(|e| panic!("template '{id}' invalid: {e}"));

            for entity in doc["entities"].as_array().unwrap() {
                assert!(entity["transform"].is_object());
                assert!(entity["components"].is_object());
            }

            let normalized: serde_json::Value =
                serde_json::to_value(serde_json::from_value::<SceneJson>(doc.clone()).unwrap())
                    .unwrap();
            assert_eq!(normalized, doc, "template '{id}' is not normalized");
        }

        assert_eq!(template_scene_doc("nope"), template_scene_doc("camera"));
    }

    #[test]
    fn scene_names_are_validated() {
        let root = temp_dir("names");
        assert!(scene_path(&root, "My Scene").is_ok());
        assert!(scene_path(&root, "level1")
            .unwrap()
            .ends_with("level1.scene.json"));
        assert!(scene_path(&root, "level2.scene.json")
            .unwrap()
            .ends_with("level2.scene.json"));
        assert!(scene_path(&root, "").is_err());
        assert!(scene_path(&root, "../escape").is_err());
        assert!(scene_path(&root, "a/b").is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn prefab_templates_validate() {
        for template in templates_for(DocKind::Prefab) {
            let doc = template_doc(DocKind::Prefab, &template.id);
            validate_doc(&doc).unwrap_or_else(|e| panic!("prefab template '{}': {e}", template.id));
            assert_eq!(doc["entities"].as_array().unwrap().len(), 1);
        }
        assert!(validate_doc(&template_doc(DocKind::Prefab, "nope")).is_ok());
        assert!(docs_dir(Path::new("/x"), DocKind::Prefab).ends_with("assets/prefabs"));
    }

    #[test]
    fn extract_subtree_keeps_children_and_rebases_parents() {
        let scene = serde_json::json!({
            "version": prism_scene::SCENE_DOC_VERSION,
            "entities": [
                { "name": "a", "parent": null, "transform": {}, "components": {} },
                { "name": "b", "parent": 0, "transform": {}, "components": {} },
                { "name": "root", "parent": 0, "transform": {}, "components": {} },
                { "name": "child", "parent": 2, "transform": {}, "components": {} },
                { "name": "grandchild", "parent": 3, "transform": {}, "components": {} },
                { "name": "other", "parent": null, "transform": {}, "components": {} }
            ]
        });

        let prefab = extract_subtree(&scene, 2).unwrap();
        validate_doc(&prefab).unwrap();
        let entities = prefab["entities"].as_array().unwrap();
        assert_eq!(entities.len(), 3);
        assert_eq!(entities[0]["name"], "root");
        assert_eq!(entities[0]["parent"], serde_json::Value::Null);
        assert_eq!(entities[1]["name"], "child");
        assert_eq!(entities[1]["parent"], 0);
        assert_eq!(entities[2]["name"], "grandchild");
        assert_eq!(entities[2]["parent"], 1);

        assert!(extract_subtree(&scene, 99).is_err());
    }

    #[test]
    fn instantiate_prefab_rebases_and_dedupes_names() {
        let scene = serde_json::json!({
            "version": prism_scene::SCENE_DOC_VERSION,
            "entities": [
                { "name": "host", "parent": null, "transform": {}, "components": {} },
                { "name": "Camera", "parent": null, "transform": {}, "components": {} }
            ]
        });
        let prefab = serde_json::json!({
            "version": prism_scene::SCENE_DOC_VERSION,
            "entities": [
                { "name": "Camera", "parent": null, "transform": {}, "components": {} },
                { "name": "Lens", "parent": 0, "transform": {}, "components": {} }
            ]
        });

        let result = instantiate_prefab(&scene, &prefab, Some(0), None).unwrap();
        validate_doc(&result.doc).unwrap();
        assert_eq!(result.inserted, vec![2, 3]);

        let entities = result.doc["entities"].as_array().unwrap();
        assert_eq!(entities.len(), 4);
        assert_eq!(entities[2]["name"], "Camera_2");
        assert_eq!(entities[2]["parent"], 0);
        assert_eq!(entities[3]["parent"], 2);

        assert!(instantiate_prefab(&scene, &prefab, Some(9), None).is_err());
    }

    fn demo_scene_with_host() -> serde_json::Value {
        serde_json::json!({
            "version": prism_scene::SCENE_DOC_VERSION,
            "entities": [
                { "name": "host", "parent": null, "transform": {}, "components": {} }
            ]
        })
    }

    fn demo_prefab() -> serde_json::Value {
        serde_json::json!({
            "version": prism_scene::SCENE_DOC_VERSION,
            "entities": [
                { "name": "Door", "parent": null, "transform": {}, "components": {
                    "prism_engine::scene::Active": true } },
                { "name": "Handle", "parent": 0, "transform": {}, "components": {} }
            ]
        })
    }

    #[test]
    fn instantiate_records_and_extract_strips_the_link() {
        let scene = demo_scene_with_host();
        let prefab = demo_prefab();
        let path = "assets/prefabs/door.prefab.json";

        let result = instantiate_prefab(&scene, &prefab, Some(0), Some(path)).unwrap();
        let links = prefab_instances(&result.doc);
        assert_eq!(
            links,
            vec![PrefabLink {
                index: 1,
                prefab: path.to_string()
            }]
        );

        let extracted = extract_subtree(&result.doc, 1).unwrap();
        assert!(extracted["entities"][0]["components"]
            .get(PREFAB_LINK_COMPONENT)
            .is_none());

        let unlinked = unlink_prefab_instance(&result.doc, 1).unwrap();
        assert!(prefab_instances(&unlinked).is_empty());
    }

    #[test]
    fn revert_restores_prefab_content_and_keeps_name_and_parent() {
        let scene = demo_scene_with_host();
        let prefab = demo_prefab();
        let path = "assets/prefabs/door.prefab.json";

        let instantiated = instantiate_prefab(&scene, &prefab, Some(0), Some(path)).unwrap();
        let mut broken = instantiated.doc.clone();
        broken["entities"][1]["name"] = serde_json::json!("Broken");
        broken["entities"][1]["components"] = serde_json::json!({});

        let reverted = revert_prefab_instance(&broken, 1, &prefab, Some(path)).unwrap();
        validate_doc(&reverted.doc).unwrap();

        let entities = reverted.doc["entities"].as_array().unwrap();
        assert_eq!(entities.len(), 3);
        assert_eq!(entities[reverted.index]["name"], "Broken");
        assert_eq!(entities[reverted.index]["parent"], 0);
        assert_eq!(
            entities[reverted.index]["components"]["prism_engine::scene::Active"],
            serde_json::json!(true)
        );
        assert_eq!(prefab_instances(&reverted.doc).len(), 1);
    }

    #[test]
    fn revert_keeps_the_instance_at_its_original_position() {
        let path = "assets/prefabs/door.prefab.json";
        let mut door = serde_json::json!({
            "name": "Door",
            "parent": 0,
            "transform": {},
            "components": {}
        });
        door["components"][PREFAB_LINK_COMPONENT] = serde_json::json!({ "prefab": path });
        let scene = serde_json::json!({
            "version": prism_scene::SCENE_DOC_VERSION,
            "entities": [
                { "name": "host", "parent": null, "transform": {}, "components": {} },
                { "name": "filler", "parent": null, "transform": {}, "components": {} },
                door,
                { "name": "Handle", "parent": 2, "transform": {}, "components": {} },
                { "name": "tail", "parent": null, "transform": {}, "components": {} }
            ]
        });

        let reverted = revert_prefab_instance(&scene, 2, &demo_prefab(), Some(path)).unwrap();
        validate_doc(&reverted.doc).unwrap();

        assert_eq!(reverted.index, 2);
        let names: Vec<&str> = reverted.doc["entities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entity| entity["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["host", "filler", "Door", "Handle", "tail"]);
        assert_eq!(reverted.doc["entities"][2]["parent"], 0);
        assert_eq!(
            prefab_instances(&reverted.doc),
            vec![PrefabLink {
                index: 2,
                prefab: path.to_string()
            }]
        );
    }

    #[test]
    fn revert_maps_the_parent_id_to_its_current_index() {
        let path = "assets/prefabs/door.prefab.json";
        let mut door = serde_json::json!({
            "id": 30,
            "name": "Door",
            "parent": 10,
            "transform": {},
            "components": {}
        });
        door["components"][PREFAB_LINK_COMPONENT] = serde_json::json!({ "prefab": path });
        let scene = serde_json::json!({
            "version": prism_scene::SCENE_DOC_VERSION,
            "entities": [
                { "id": 10, "name": "host", "parent": null, "transform": {}, "components": {} },
                door,
                { "id": 40, "name": "Handle", "parent": 30, "transform": {}, "components": {} }
            ]
        });

        let reverted = revert_prefab_instance(&scene, 1, &demo_prefab(), Some(path)).unwrap();
        validate_doc(&reverted.doc).unwrap();

        let entities = reverted.doc["entities"].as_array().unwrap();
        assert_eq!(reverted.index, 1);
        assert_eq!(entities.len(), 3);
        assert_eq!(entities[1]["parent"], 10);
        let ids: Vec<u64> = entities
            .iter()
            .map(|entity| entity["id"].as_u64().unwrap())
            .collect();
        assert_eq!(ids, vec![10, 11, 12]);
    }

    #[test]
    fn revert_resolves_the_parent_index_after_deleting_the_subtree() {
        let path = "assets/prefabs/door.prefab.json";
        let mut door = serde_json::json!({
            "id": 1,
            "name": "Door",
            "parent": 0,
            "transform": {},
            "components": {}
        });
        door["components"][PREFAB_LINK_COMPONENT] = serde_json::json!({ "prefab": path });
        let scene = serde_json::json!({
            "version": prism_scene::SCENE_DOC_VERSION,
            "entities": [
                { "id": 9, "name": "grandchild", "parent": 1, "transform": {}, "components": {} },
                { "id": 0, "name": "host", "parent": null, "transform": {}, "components": {} },
                door,
                { "id": 7, "name": "tail", "parent": null, "transform": {}, "components": {} }
            ]
        });

        let reverted = revert_prefab_instance(&scene, 2, &demo_prefab(), Some(path)).unwrap();
        validate_doc(&reverted.doc).unwrap();

        let entities = reverted.doc["entities"].as_array().unwrap();
        let names: Vec<&str> = entities
            .iter()
            .map(|entity| entity["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["host", "Door", "Handle", "tail"]);
        assert_eq!(entities[reverted.index]["parent"], 0);
        assert_eq!(reverted.index, 1);
        assert_eq!(prefab_instances(&reverted.doc).len(), 1);
    }

    #[test]
    fn revert_rejects_a_parent_that_is_inside_the_subtree() {
        let path = "assets/prefabs/door.prefab.json";
        let mut door = serde_json::json!({
            "id": 1,
            "name": "Door",
            "parent": 9,
            "transform": {},
            "components": {}
        });
        door["components"][PREFAB_LINK_COMPONENT] = serde_json::json!({ "prefab": path });
        let scene = serde_json::json!({
            "version": prism_scene::SCENE_DOC_VERSION,
            "entities": [
                { "id": 9, "name": "grandchild", "parent": 1, "transform": {}, "components": {} },
                { "id": 0, "name": "host", "parent": null, "transform": {}, "components": {} },
                door
            ]
        });

        let error = revert_prefab_instance(&scene, 2, &demo_prefab(), Some(path)).unwrap_err();
        assert!(
            error.contains("in pending-deleted subtree"),
            "Cycle: cannot change parent id to non-existent trimmed index: {error}"
        );
    }

    #[test]
    fn sync_keeps_every_instance_in_its_own_slot() {
        let path = "assets/prefabs/door.prefab.json";
        let prefab = demo_prefab();

        let once = instantiate_prefab(&demo_scene_with_host(), &prefab, None, Some(path)).unwrap();
        let twice = instantiate_prefab(&once.doc, &prefab, None, Some(path)).unwrap();

        let mut broken = twice.doc.clone();
        broken["entities"].as_array_mut().unwrap().insert(
            3,
            serde_json::json!({
                "id": 99, "name": "mid", "parent": null, "transform": {}, "components": {}
            }),
        );
        for entity in broken["entities"].as_array_mut().unwrap().iter_mut() {
            if let Some(components) = entity["components"].as_object_mut() {
                components.remove("prism_engine::scene::Active");
            }
        }

        let synced = sync_prefab_instances(&broken, path, &prefab).unwrap();
        validate_doc(&synced).unwrap();

        let names: Vec<&str> = synced["entities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entity| entity["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec!["host", "Door", "Handle", "mid", "Door_2", "Handle_2"]
        );
        assert_eq!(
            prefab_instances(&synced)
                .into_iter()
                .map(|link| link.index)
                .collect::<Vec<_>>(),
            vec![1, 4]
        );
    }

    #[test]
    fn sync_updates_every_instance_of_the_same_prefab() {
        let scene = demo_scene_with_host();
        let prefab = demo_prefab();
        let path = "assets/prefabs/door.prefab.json";

        let once = instantiate_prefab(&scene, &prefab, None, Some(path)).unwrap();
        let twice = instantiate_prefab(&once.doc, &prefab, None, Some(path)).unwrap();
        assert_eq!(prefab_instances(&twice.doc).len(), 2);

        let mut broken = twice.doc.clone();
        for entity in broken["entities"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .skip(1)
        {
            if let Some(components) = entity["components"].as_object_mut() {
                components.remove("prism_engine::scene::Active");
            }
        }
        assert_eq!(prefab_instances(&broken).len(), 2);

        let synced = sync_prefab_instances(&broken, path, &prefab).unwrap();
        validate_doc(&synced).unwrap();
        let entities = synced["entities"].as_array().unwrap();
        assert_eq!(entities.len(), 5);
        for entity in entities.iter().skip(1) {
            if entity["name"].as_str().unwrap_or("").starts_with("Door") {
                assert!(
                    entity["components"]
                        .get("prism_engine::scene::Active")
                        .is_some(),
                    "instance content was not restored: {entity}"
                );
            }
        }
        assert_eq!(prefab_instances(&synced).len(), 2);
    }

    /// a(0) → b(1) → c(2), plus root d(3).
    fn hierarchy_doc() -> serde_json::Value {
        serde_json::json!({
            "version": prism_scene::SCENE_DOC_VERSION,
            "entities": [
                { "name": "a", "parent": null, "transform": {}, "components": {} },
                { "name": "b", "parent": 0, "transform": {}, "components": {} },
                { "name": "c", "parent": 1, "transform": {}, "components": {} },
                { "name": "d", "parent": null, "transform": {}, "components": {} }
            ]
        })
    }

    #[test]
    fn reparent_moves_a_subtree_and_rejects_cycles() {
        let doc = hierarchy_doc();

        let out = reparent_doc(&doc, 2, Some(3)).unwrap();
        let entities = out["entities"].as_array().unwrap();
        assert_eq!(entities[2]["name"], "c");
        assert_eq!(entities[2]["parent"], 3);

        let out = reparent_doc(&doc, 1, None).unwrap();
        assert_eq!(out["entities"][1]["parent"], serde_json::Value::Null);
        assert_eq!(out["entities"][2]["parent"], 1);

        assert!(reparent_doc(&doc, 1, Some(1)).is_err());
        assert!(reparent_doc(&doc, 0, Some(2)).is_err());
        assert!(reparent_doc(&doc, 9, Some(0)).is_err());
    }

    #[test]
    fn move_entity_reorders_without_rewriting_parents() {
        let doc = hierarchy_doc();
        validate_doc(&doc).unwrap();

        let result = move_entity(&doc, 0, 4, None).unwrap();
        validate_doc(&result.doc).unwrap();
        assert_eq!(result.index, 3);

        let entities = result.doc["entities"].as_array().unwrap();
        assert_eq!(entities[0]["name"], "b");
        assert_eq!(entities[0]["parent"], 0);
        assert_eq!(entities[1]["name"], "c");
        assert_eq!(entities[1]["parent"], 1);
        assert_eq!(entities[3]["name"], "a");
        assert_eq!(entities[3]["parent"], serde_json::Value::Null);

        let result = move_entity(&doc, 2, 1, Some(3)).unwrap();
        validate_doc(&result.doc).unwrap();
        let entities = result.doc["entities"].as_array().unwrap();
        assert_eq!(entities[1]["name"], "c");
        assert_eq!(result.index, 1);
        assert_eq!(entities[1]["parent"], 3);

        assert!(move_entity(&doc, 0, 0, Some(2)).is_err());
    }

    #[test]
    fn delete_entities_roots_orphans_without_rewriting_ids() {
        let doc = hierarchy_doc();

        let out = delete_entities(&doc, &[1]).unwrap();
        validate_doc(&out).unwrap();
        let entities = out["entities"].as_array().unwrap();
        assert_eq!(entities.len(), 3);
        assert_eq!(entities[1]["name"], "c");
        assert_eq!(entities[1]["parent"], serde_json::Value::Null);

        let out = delete_entities(&doc, &[0, 3]).unwrap();
        validate_doc(&out).unwrap();
        let entities = out["entities"].as_array().unwrap();
        assert_eq!(entities.len(), 2);
        assert_eq!(entities[0]["name"], "b");
        assert_eq!(entities[0]["parent"], serde_json::Value::Null);
        assert_eq!(entities[1]["parent"], 1);

        assert!(delete_entities(&doc, &[0, 1, 2, 3]).is_err());
        assert!(delete_entities(&doc, &[9]).is_err());
    }

    #[test]
    fn write_as_rename_and_delete_roundtrip() {
        let root = temp_dir("ops");

        let path = write_scene_as(&root, "alpha", &empty_scene_doc()).unwrap();
        assert!(path.exists());
        assert!(write_scene_as(&root, "alpha", &empty_scene_doc()).is_err());

        let renamed = rename_scene(&path, "beta").unwrap();
        assert!(!path.exists());
        assert!(renamed.ends_with("beta.scene.json"));

        let scenes = list_scenes(&root).unwrap();
        assert_eq!(scenes.len(), 1);
        assert_eq!(scenes[0].name, "beta");

        delete_scene(&renamed).unwrap();
        assert!(list_scenes(&root).unwrap().is_empty());

        std::fs::remove_dir_all(&root).ok();
    }

    /// #466: a scene opened through the project API resolves GUIDs to paths.
    #[test]
    fn project_read_prefers_guid_and_save_writes_both() {
        let root = temp_dir("scene_guid");
        let models = root.join("assets/models");
        std::fs::create_dir_all(&models).unwrap();
        std::fs::write(models.join("box.gltf"), b"mesh").unwrap();
        let meta = prism_asset::AssetMeta::ensure(&root, "assets/models/box.gltf", "GltfImporter")
            .unwrap();

        let path = scene_path(&root, "arena").unwrap();
        let doc = json!({
            "version": prism_scene::SCENE_DOC_VERSION,
            "entities": [{
                "id": 0,
                "name": "Box",
                "parent": null,
                "transform": {},
                "components": {
                    "prism_engine::scene::MeshRenderer": {
                        "mesh_path": "../models/renamed.gltf",
                        "mesh_guid": meta.guid
                    }
                }
            }]
        });
        write_doc(&path, &doc).unwrap();

        let opened = read_doc_in_project(&root, &path).unwrap();
        assert_eq!(
            opened["entities"][0]["components"]["prism_engine::scene::MeshRenderer"]["mesh_path"],
            "../models/box.gltf"
        );

        let saved = sync_doc_for_save(&root, &path, &opened).unwrap();
        write_doc(&path, &saved).unwrap();
        let raw = read_doc(&path).unwrap();
        let renderer = &raw["entities"][0]["components"]["prism_engine::scene::MeshRenderer"];
        assert_eq!(renderer["mesh_path"], "../models/box.gltf");
        assert_eq!(renderer["mesh_guid"], meta.guid);

        let path_only = json!({
            "version": prism_scene::SCENE_DOC_VERSION,
            "entities": [{
                "id": 0,
                "name": "Box",
                "parent": null,
                "transform": {},
                "components": {
                    "prism_engine::scene::MeshRenderer": {
                        "mesh_path": "../models/box.gltf"
                    }
                }
            }]
        });
        write_doc(&path, &path_only).unwrap();
        let saved = sync_doc_for_save(&root, &path, &path_only).unwrap();
        write_doc(&path, &saved).unwrap();
        let raw = read_doc(&path).unwrap();
        assert_eq!(
            raw["entities"][0]["components"]["prism_engine::scene::MeshRenderer"]["mesh_guid"],
            meta.guid
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// A document without GUIDs and without sidecars must be byte-for-byte
    /// unchanged by a load/save round trip.
    #[test]
    fn legacy_scene_without_sidecars_is_unchanged() {
        let root = temp_dir("scene_legacy");
        let path = scene_path(&root, "legacy").unwrap();
        let doc = json!({
            "version": prism_scene::SCENE_DOC_VERSION,
            "entities": [{
                "id": 0,
                "name": "Box",
                "parent": null,
                "transform": {},
                "components": {
                    "prism_engine::scene::MeshRenderer": {
                        "mesh_path": "../models/missing.gltf"
                    }
                }
            }]
        });
        write_doc(&path, &doc).unwrap();

        let opened = read_doc_in_project(&root, &path).unwrap();
        let saved = sync_doc_for_save(&root, &path, &opened).unwrap();
        let renderer = &saved["entities"][0]["components"]["prism_engine::scene::MeshRenderer"];
        assert_eq!(renderer["mesh_path"], "../models/missing.gltf");
        assert!(renderer.get("mesh_guid").is_none());

        std::fs::remove_dir_all(&root).ok();
    }
}
