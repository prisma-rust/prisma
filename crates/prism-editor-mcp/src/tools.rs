//! Document and reflection-field tools: all work happens in
//! `prism-editor-api`; this module only parses arguments.
//!
//! Design constraint: **every write goes through reflection**
//! (`set_component_field`), so invalid types, read-only fields and unknown
//! enum variants are rejected and `.scene.json` cannot be corrupted.

use std::path::{Path, PathBuf};

use prism_editor_api::asset_inspect::{inspect_asset, validate_asset};
use prism_editor_api::reflect_edit::set_component_field;
use prism_editor_api::scene::{self, DocKind};
use prism_editor_api::schema::{build_schema, Schema};
use serde_json::{json, Map, Value};

use crate::ToolDef;

pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "documents_list",
            description: "List scene and prefab documents in the project (assets/scenes/*.scene.json, assets/prefabs/*.prefab.json).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "enum": ["scene", "prefab", "all"], "default": "all" }
                }
            }),
        },
        ToolDef {
            name: "document_read",
            description: "Read the full JSON of a document (migrated to current format, entities carry stable ids).",
            input_schema: json!({
                "type": "object",
                "properties": { "path": { "type": "string", "description": "Path relative to project root" } },
                "required": ["path"]
            }),
        },
        ToolDef {
            name: "document_write",
            description: "Validate and write back entire document. For field-level edits prefer field_set (it does reflection validation).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "doc": { "type": "object", "description": "{ version, entities }" }
                },
                "required": ["path", "doc"]
            }),
        },
        ToolDef {
            name: "schema_list",
            description: "List all registered components and nested structs, with each field's kind / enum variants / read-only flag.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "component": { "type": "string", "description": "Fill to look at only this component" }
                }
            }),
        },
        ToolDef {
            name: "entity_get",
            description: "Get an entity by id or name (with transform and all components).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "entity": { "type": "string", "description": "Entity id (number) or name" }
                },
                "required": ["path", "entity"]
            }),
        },
        ToolDef {
            name: "field_set",
            description: "Write a component field via reflection. field supports a.b and list[0].x; value given in author JSON shape (enum as string).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "entity": { "type": "string" },
                    "component": { "type": "string", "description": "e.g. prism_engine::scene::SpriteRenderer" },
                    "field": { "type": "string", "description": "Field path, e.g. color or points[0]" },
                    "value": { "description": "New value" }
                },
                "required": ["path", "entity", "component", "field", "value"]
            }),
        },
        ToolDef {
            name: "component_add",
            description: "Add a component to entity (using schema default value).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "entity": { "type": "string" },
                    "component": { "type": "string" }
                },
                "required": ["path", "entity", "component"]
            }),
        },
        ToolDef {
            name: "asset_inspect",
            description: "Inspect a cooked asset (RSKL/RSKN/RANI/RMES/RTEX/RMAT): counts, summary and validation issues. Path relative to project root, e.g. assets/cooked/hero.rskin.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path relative to project root" }
                },
                "required": ["path"]
            }),
        },
        ToolDef {
            name: "asset_validate",
            description: "Run validation only on cooked assets (weight sums, joint out-of-range, index/submesh ranges, keyframe time and order): ok=true means no issues.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "skeleton": { "type": "string", "description": "Optional: skeleton file for .ranim, used to check track indices" }
                },
                "required": ["path"]
            }),
        },
        ToolDef {
            name: "component_remove",
            description: "Remove a component from entity.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "entity": { "type": "string" },
                    "component": { "type": "string" }
                },
                "required": ["path", "entity", "component"]
            }),
        },
    ]
}

/// Dispatches a tool call; an `Err` becomes MCP's `isError: true`.
pub fn call(root: &Path, name: &str, args: &Value) -> Result<String, String> {
    match name {
        "documents_list" => documents_list(root, args),
        "document_read" => document_read(root, args),
        "document_write" => document_write(root, args),
        "schema_list" => schema_list(args),
        "entity_get" => entity_get(root, args),
        "field_set" => field_set(root, args),
        "component_add" => component_add(root, args),
        "asset_inspect" => asset_inspect(root, args),
        "asset_validate" => asset_validate(root, args),
        "component_remove" => component_remove(root, args),
        other => Err(format!("Unknown tool '{other}'")),
    }
}

fn asset_inspect(root: &Path, args: &Value) -> Result<String, String> {
    let relative = str_arg(args, "path")?;
    let inspection = inspect_asset(root, &relative)?;
    serde_json::to_string_pretty(&inspection).map_err(|error| error.to_string())
}

fn asset_validate(root: &Path, args: &Value) -> Result<String, String> {
    let relative = str_arg(args, "path")?;
    let skeleton = args.get("skeleton").and_then(Value::as_str);
    let problems = validate_asset(root, &relative, skeleton)?;
    serde_json::to_string_pretty(&json!({
        "path": relative.replace('\\', "/"),
        "ok": problems.is_empty(),
        "problems": problems,
    }))
    .map_err(|error| error.to_string())
}

fn str_arg(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("Missing string parameter '{key}'"))
}

fn doc_path(root: &Path, raw: &str) -> PathBuf {
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn documents_list(root: &Path, args: &Value) -> Result<String, String> {
    let kind = args.get("kind").and_then(Value::as_str).unwrap_or("all");
    let kinds: &[DocKind] = match kind {
        "scene" => &[DocKind::Scene],
        "prefab" => &[DocKind::Prefab],
        _ => &[DocKind::Scene, DocKind::Prefab],
    };
    let mut out = Vec::new();
    for kind in kinds {
        let entries = scene::list_docs(root, *kind)?;
        for entry in entries {
            // Always expose project-relative paths so agents do not have to
            // deal with an absolute prefix when building `path`.
            let root_prefix = root.to_string_lossy();
            let relative = entry
                .path
                .strip_prefix(root_prefix.as_ref())
                .unwrap_or(&entry.path)
                .trim_start_matches(['\\', '/'])
                .replace('\\', "/");
            out.push(json!({
                "name": entry.name,
                "path": relative,
                "kind": format!("{:?}", kind).to_lowercase(),
            }));
        }
    }
    serde_json::to_string_pretty(&out).map_err(|err| err.to_string())
}

fn document_read(root: &Path, args: &Value) -> Result<String, String> {
    let path = doc_path(root, &str_arg(args, "path")?);
    let doc = scene::read_doc_in_project(root, &path)?;
    serde_json::to_string_pretty(&doc).map_err(|err| err.to_string())
}

fn document_write(root: &Path, args: &Value) -> Result<String, String> {
    let path = doc_path(root, &str_arg(args, "path")?);
    let doc = args
        .get("doc")
        .ok_or_else(|| "Missing 'doc' parameter".to_string())?;
    scene::validate_doc(doc)?;
    scene::write_doc_in_project(root, &path, doc)?;
    Ok(format!("Wrote {}", path.display()))
}

fn schema_list(args: &Value) -> Result<String, String> {
    let schema = build_schema();
    let only = args.get("component").and_then(Value::as_str);
    let types: Vec<&_> = schema
        .types
        .iter()
        .filter(|ty| only.is_none_or(|name| ty.name == name))
        .collect();
    if types.is_empty() {
        return Err(match only {
            Some(name) => format!("No type named '{name}'"),
            None => "Schema is empty".to_string(),
        });
    }
    serde_json::to_string_pretty(&types).map_err(|err| err.to_string())
}

/// Locates an entity: a plain number matches `id`, otherwise `name` (first
/// match wins).
fn locate<'a>(doc: &'a mut Value, key: &str) -> Result<&'a mut Map<String, Value>, String> {
    let entities = doc
        .get_mut("entities")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "Document has no entities array".to_string())?;
    let by_id = key.parse::<u64>().ok();
    let found = entities.iter_mut().find(|entity| match by_id {
        Some(id) => entity.get("id").and_then(Value::as_u64) == Some(id),
        None => entity.get("name").and_then(Value::as_str) == Some(key),
    });
    found
        .and_then(Value::as_object_mut)
        .ok_or_else(|| format!("Entity not found '{key}'"))
}

fn entity_get(root: &Path, args: &Value) -> Result<String, String> {
    let path = doc_path(root, &str_arg(args, "path")?);
    let mut doc = scene::read_doc_in_project(root, &path)?;
    let entity = locate(&mut doc, &str_arg(args, "entity")?)?;
    serde_json::to_string_pretty(entity).map_err(|err| err.to_string())
}

fn field_set(root: &Path, args: &Value) -> Result<String, String> {
    let path = doc_path(root, &str_arg(args, "path")?);
    let entity_key = str_arg(args, "entity")?;
    let component = str_arg(args, "component")?;
    let field = str_arg(args, "field")?;
    let value = args
        .get("value")
        .ok_or_else(|| "Missing 'value' parameter".to_string())?
        .clone();

    let mut doc = scene::read_doc_in_project(root, &path)?;
    let entity = locate(&mut doc, &entity_key)?;
    let components = entity
        .get_mut("components")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| format!("Entity '{entity_key}' has no components"))?;
    let before = components
        .get(&component)
        .cloned()
        .ok_or_else(|| format!("Entity '{entity_key}' has no component '{component}'"))?;

    // `set_component_field` reads the new value from the "after" object, so
    // patch the path first; an invalid type is rejected inside its
    // `object_from_json`.
    let mut after = before.clone();
    patch_path(&mut after, &field, value)?;
    let updated = set_component_field(&component, &before, &field, &after)?;
    components.insert(component.clone(), updated.clone());

    scene::validate_doc(&doc)?;
    scene::write_doc_in_project(root, &path, &doc)?;
    serde_json::to_string_pretty(&updated).map_err(|err| err.to_string())
}

fn component_add(root: &Path, args: &Value) -> Result<String, String> {
    let path = doc_path(root, &str_arg(args, "path")?);
    let entity_key = str_arg(args, "entity")?;
    let component = str_arg(args, "component")?;
    let schema: Schema = build_schema();
    let ty = schema
        .type_by_name(&component)
        .ok_or_else(|| format!("'{component}' is not a registered type"))?;
    if !ty.is_component {
        return Err(format!(
            "'{component}' is a nested struct and cannot be attached to entity"
        ));
    }
    let default = ty
        .default_json
        .clone()
        .ok_or_else(|| format!("'{component}' has no default value"))?;

    let mut doc = scene::read_doc_in_project(root, &path)?;
    let entity = locate(&mut doc, &entity_key)?;
    let components = entity
        .get_mut("components")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| format!("Entity '{entity_key}' has no components"))?;
    if components.contains_key(&component) {
        return Err(format!("Entity '{entity_key}' already has '{component}'"));
    }
    components.insert(component.clone(), default.clone());

    scene::validate_doc(&doc)?;
    scene::write_doc_in_project(root, &path, &doc)?;
    serde_json::to_string_pretty(&default).map_err(|err| err.to_string())
}

fn component_remove(root: &Path, args: &Value) -> Result<String, String> {
    let path = doc_path(root, &str_arg(args, "path")?);
    let entity_key = str_arg(args, "entity")?;
    let component = str_arg(args, "component")?;

    let mut doc = scene::read_doc_in_project(root, &path)?;
    let entity = locate(&mut doc, &entity_key)?;
    let components = entity
        .get_mut("components")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| format!("Entity '{entity_key}' has no components"))?;
    if components.remove(&component).is_none() {
        return Err(format!("Entity '{entity_key}' has no '{component}'"));
    }

    scene::validate_doc(&doc)?;
    scene::write_doc_in_project(root, &path, &doc)?;
    Ok(format!("Removed {component}"))
}

/// Writes a value into JSON by path such as `a.b[2].c`; only used to build the
/// "after" value for `set_component_field`.
fn patch_path(target: &mut Value, path: &str, value: Value) -> Result<(), String> {
    let mut cursor = target;
    for segment in parse_path(path)? {
        match segment {
            Segment::Key(key) => {
                let map = cursor
                    .as_object_mut()
                    .ok_or_else(|| format!("'{key}' container is not an object"))?;
                cursor = map
                    .get_mut(&key)
                    .ok_or_else(|| format!("Field '{key}' does not exist"))?;
            }
            Segment::Index(index) => {
                let list = cursor
                    .as_array_mut()
                    .ok_or_else(|| format!("[{index}] container is not a list"))?;
                let len = list.len();
                cursor = list
                    .get_mut(index)
                    .ok_or_else(|| format!("Index [{index}] out of bounds (length {len})"))?;
            }
        }
    }
    *cursor = value;
    Ok(())
}

enum Segment {
    Key(String),
    Index(usize),
}

fn parse_path(path: &str) -> Result<Vec<Segment>, String> {
    let mut segments = Vec::new();
    for part in path.split('.') {
        if part.is_empty() {
            return Err(format!("Field path '{path}' has an empty segment"));
        }
        let (name, indices) = match part.find('[') {
            Some(at) => (&part[..at], &part[at..]),
            None => (part, ""),
        };
        if !name.is_empty() {
            segments.push(Segment::Key(name.to_string()));
        }
        let mut rest = indices;
        while let Some(stripped) = rest.strip_prefix('[') {
            let end = stripped
                .find(']')
                .ok_or_else(|| format!("Field path '{path}' missing ']'"))?;
            let index = stripped[..end]
                .parse::<usize>()
                .map_err(|_| format!("Field path '{path}' index is not a number"))?;
            segments.push(Segment::Index(index));
            rest = &stripped[end + 1..];
        }
    }
    Ok(segments)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_nested_paths() {
        let mut value = json!({ "a": { "b": [1, 2, 3] }, "c": 0 });
        patch_path(&mut value, "a.b[1]", json!(9)).unwrap();
        assert_eq!(value["a"]["b"][1], 9);
        patch_path(&mut value, "c", json!(5)).unwrap();
        assert_eq!(value["c"], 5);
    }

    #[test]
    fn rejects_bad_paths() {
        let mut value = json!({ "a": 1 });
        assert!(patch_path(&mut value, "missing", json!(2)).is_err());
        assert!(patch_path(&mut value, "a[0]", json!(2)).is_err());
        assert!(parse_path("a..b").is_err());
        assert!(parse_path("a[1").is_err());
    }
}
