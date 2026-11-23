//! **Reflection-based field value modification**.
//!
//! Editor does not write values directly into `.scene.json`; instead it hands here "component name + original JSON + field path +
//! modified JSON": first uses serde to restore modified JSON into valid component (so enum strings,
//! single-field newtypes and other author shapes parse correctly), then writes that field **reflected value** into original object.
//!
//! Field-level writes thus go through `Reflect::set_field`:
//! - `#[reflect(read_only)]` fields are rejected (e.g. `Camera.aspect`);
//! - value must be that field declared reflection type (`from_object` already validates);
//! - finally uses registry `object_to_json` to restore object into author JSON.
//!
//! Field path syntax (consistent with frontend `FieldEditor` path):
//! `fov_y_degrees`, `anchors.min_x`, `background[0]`, `nested.list[2].x`.

use crate::builtin_registry;
use prism_reflect::{Object, Value};

/// One segment in a field path.
#[derive(Debug, Clone, PartialEq)]
enum Segment {
    Key(String),
    Index(usize),
}

fn parse_path(path: &str) -> Result<Vec<Segment>, String> {
    let mut segments = Vec::new();
    let mut chars = path.chars().peekable();
    let mut current = String::new();

    while let Some(c) = chars.next() {
        match c {
            '.' => {
                if !current.is_empty() {
                    segments.push(Segment::Key(std::mem::take(&mut current)));
                }
            }
            '[' => {
                if !current.is_empty() {
                    segments.push(Segment::Key(std::mem::take(&mut current)));
                }
                let mut digits = String::new();
                for d in chars.by_ref() {
                    if d == ']' {
                        break;
                    }
                    digits.push(d);
                }
                let index = digits
                    .parse::<usize>()
                    .map_err(|_| format!("Invalid array index '{digits}'"))?;
                segments.push(Segment::Index(index));
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        segments.push(Segment::Key(current));
    }
    if segments.is_empty() {
        return Err("Field path is empty".into());
    }
    if !matches!(segments[0], Segment::Key(_)) {
        return Err("Field path must start with a field name".into());
    }
    Ok(segments)
}

/// Get **clone** of value described by `segments` (for "modified" object).
fn read_at(value: &Value, segments: &[Segment]) -> Result<Value, String> {
    let mut current = value.clone();
    for segment in segments {
        current = match (segment, current) {
            (Segment::Key(key), Value::Object(object)) => object
                .get(key)
                .cloned()
                .ok_or_else(|| format!("Field '{key}' does not exist"))?,
            (Segment::Index(index), Value::List(items)) => items
                .get(*index)
                .cloned()
                .ok_or_else(|| format!("Index [{index}] out of bounds"))?,
            (Segment::Key(key), _) => return Err(format!("'{key}' is not an object field")),
            (Segment::Index(index), _) => return Err(format!("[{index}] is not an array element")),
        };
    }
    Ok(current)
}

/// Write `new_value` into `target` at `segments` position (in place).
fn write_at(target: &mut Value, segments: &[Segment], new_value: Value) -> Result<(), String> {
    match segments {
        [] => {
            *target = new_value;
            Ok(())
        }
        [Segment::Key(key), rest @ ..] => {
            let Value::Object(object) = target else {
                return Err(format!("'{key}' container is not an object"));
            };
            let slot = object
                .fields
                .get_mut(key)
                .ok_or_else(|| format!("Field '{key}' does not exist"))?;
            write_at(slot, rest, new_value)
        }
        [Segment::Index(index), rest @ ..] => {
            let Value::List(items) = target else {
                return Err("Array index container is not a list".into());
            };
            let slot = items
                .get_mut(*index)
                .ok_or_else(|| format!("Index [{index}] out of bounds"))?;
            write_at(slot, rest, new_value)
        }
    }
}

/// Write one field on component JSON with reflection semantics, return new author JSON.
///
/// `before` is current value of component, `after` is frontend modified whole value (both same shape).
/// Empty `path` means whole replace (still goes through one `from_object` validation and normalization).
pub fn set_component_field(
    component: &str,
    before: &serde_json::Value,
    path: &str,
    after: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let registry = builtin_registry();

    let before_object = registry
        .object_from_json(component, before)
        .ok_or_else(|| format!("'{component}' is not a registered reflectable component"))?;
    let after_object = registry.object_from_json(component, after).ok_or_else(|| {
        format!("'{component}' new value cannot be restored (invalid type or shape)")
    })?;

    let segments = if path.trim().is_empty() {
        Vec::new()
    } else {
        parse_path(path)?
    };

    if segments.is_empty() {
        return registry
            .object_to_json(component, &after_object)
            .ok_or_else(|| format!("'{component}' missing object_to_json converter"));
    }

    let new_value = read_at(&Value::Object(after_object), &segments)?;

    let Segment::Key(field) = &segments[0] else {
        unreachable!("parse_path guarantees starts with Key");
    };
    let mut root = before_object
        .get(field)
        .cloned()
        .ok_or_else(|| format!("Field '{field}' does not exist"))?;
    write_at(&mut root, &segments[1..], new_value)?;

    let mut patched = before_object;
    registry
        .set_object_field(&mut patched, field, root)
        .map_err(|e| format!("Field '{field}' write rejected: {e}"))?;

    registry
        .object_to_json(component, &patched)
        .ok_or_else(|| format!("'{component}' missing object_to_json converter"))
}

/// Normalize component JSON (`from_object` validation + `object_to_json` restore).
pub fn normalize_component(
    component: &str,
    value: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let registry = builtin_registry();
    let object: Object = registry
        .object_from_json(component, value)
        .ok_or_else(|| format!("'{component}' value cannot be restored"))?;
    registry
        .object_to_json(component, &object)
        .ok_or_else(|| format!("'{component}' missing object_to_json converter"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CAMERA: &str = "prism_engine::scene::Camera";
    const STYLE: &str = "prism_engine::ui::Style";
    const TEXT: &str = "prism_engine::ui::Text";
    const NAME: &str = "prism_engine::scene::Name";

    fn camera_default() -> serde_json::Value {
        crate::build_schema()
            .type_by_name(CAMERA)
            .unwrap()
            .default_json
            .clone()
            .unwrap()
    }

    fn component_default(name: &str) -> serde_json::Value {
        crate::build_schema()
            .type_by_name(name)
            .unwrap()
            .default_json
            .clone()
            .unwrap()
    }

    #[test]
    fn writes_plain_and_nested_fields() {
        let before = camera_default();
        let mut after = before.clone();
        after["fov_y_degrees"] = json!(72.0);

        let updated = set_component_field(CAMERA, &before, "fov_y_degrees", &after).unwrap();
        assert_eq!(updated["fov_y_degrees"], json!(72.0));

        let before = component_default(STYLE);
        let mut after = before.clone();
        after["anchors"]["min_x"] = json!(0.25);
        let updated = set_component_field(STYLE, &before, "anchors.min_x", &after).unwrap();
        assert_eq!(updated["anchors"]["min_x"], json!(0.25));
    }

    #[test]
    fn writes_array_elements() {
        let before = component_default(TEXT);
        let mut after = before.clone();
        after["color"][1] = json!(0.5);
        let updated = set_component_field(TEXT, &before, "color[1]", &after).unwrap();
        assert_eq!(updated["color"][1], json!(0.5));
    }

    #[test]
    fn rejects_read_only_fields() {
        let before = camera_default();
        let mut after = before.clone();
        after["aspect"] = json!(2.0);

        let err = set_component_field(CAMERA, &before, "aspect", &after).unwrap_err();
        assert!(err.contains("aspect"), "unexpected error: {err}");
    }

    #[test]
    fn rejects_wrong_types() {
        let before = camera_default();
        let mut after = before.clone();
        after["fov_y_degrees"] = json!("not a number");

        assert!(set_component_field(CAMERA, &before, "fov_y_degrees", &after).is_err());
    }

    #[test]
    fn handles_serde_shaped_enums_and_newtypes() {
        let before = component_default(TEXT);
        let mut after = before.clone();
        after["alignment"] = json!("Left");
        let updated = set_component_field(TEXT, &before, "alignment", &after).unwrap();
        assert_eq!(updated["alignment"], json!("Left"));

        let before = json!("old");
        let after = json!("new");
        let updated = set_component_field(NAME, &before, "", &after).unwrap();
        assert_eq!(updated, json!("new"));
    }

    #[test]
    fn empty_path_replaces_whole_component() {
        let before = camera_default();
        let mut after = before.clone();
        after["near"] = json!(0.5);
        let updated = set_component_field(CAMERA, &before, "", &after).unwrap();
        assert_eq!(updated["near"], json!(0.5));
        assert_eq!(updated["far"], before["far"]);
    }

    #[test]
    fn unknown_component_is_rejected() {
        assert!(set_component_field("nope::Nope", &json!({}), "x", &json!({})).is_err());
    }

    #[test]
    fn field_writes_reuse_one_registry_across_threads() {
        assert!(std::ptr::eq(builtin_registry(), builtin_registry()));

        let before = camera_default();
        let mut after = before.clone();
        after["fov_y_degrees"] = json!(60.0);

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let before = before.clone();
                let after = after.clone();
                std::thread::spawn(move || {
                    for _ in 0..25 {
                        let updated =
                            set_component_field(CAMERA, &before, "fov_y_degrees", &after).unwrap();
                        assert_eq!(updated["fov_y_degrees"], json!(60.0));
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("Write thread should not panic");
        }
    }
}
