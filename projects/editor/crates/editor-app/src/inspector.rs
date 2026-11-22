//! Schema-driven Inspector & field edit payload built on `prism-editor-api` + `prism-ui`.

use std::collections::BTreeMap;

use prism_editor_api::property_drawer::PropertyAttribute;
use prism_editor_api::schema::{FieldSchema, Schema};
use prism_ui::prelude::*;
use serde_json::Value;

/// Single reversible field edit command.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldEdit {
    pub entity_id: u32,
    pub component: String,
    pub field: String,
    pub axis: Option<usize>,
    pub old_value: Value,
    pub new_value: Value,
}

/// Build a stable `NodeKey` for an inspector field (also usable by `prism-editor-mcp`).
pub fn field_node_key(
    entity_id: u32,
    component: &str,
    field: &str,
    axis: Option<usize>,
) -> NodeKey {
    let tag = match axis {
        Some(a) => format!("field:{entity_id}:{component}:{field}[{a}]"),
        None => format!("field:{entity_id}:{component}:{field}"),
    };
    (&tag).into_node_key()
}

use crate::widgets::collapsible_section;

/// Render all reflected components of an entity from `prism_editor_api::schema::Schema`.
pub fn build_inspector_ui(
    schema: &Schema,
    entity_id: u32,
    components: &BTreeMap<String, Value>,
) -> ElementBox {
    let mut root = col().gap(10.0);
    for (comp_name, comp_json) in components {
        let Some(type_schema) = schema.type_by_name(comp_name) else {
            continue;
        };
        let short_name = comp_name.rsplit("::").next().unwrap_or(comp_name);
        let mut children = Vec::with_capacity(type_schema.fields.len());
        for field in &type_schema.fields {
            let val = comp_json.get(&field.name).cloned().unwrap_or(Value::Null);
            children.push(render_field_row(entity_id, comp_name, field, &val));
        }
        let sec_key = format!("section:{entity_id}:{comp_name}");
        root = root.child(collapsible_section(&sec_key, short_name, true, children));
    }
    root
}

/// Single uniform dispatch table mapping `FieldSchema` to composed `prism-ui` widgets.
fn render_field_row(
    entity_id: u32,
    component: &str,
    field: &FieldSchema,
    value: &Value,
) -> ElementBox {
    let (mut min, mut max) = (f64::NEG_INFINITY, f64::INFINITY);
    if let Some(drawer) = &field.drawer {
        for attr in &drawer.attributes {
            match attr {
                PropertyAttribute::Range { min: lo, max: hi } => {
                    min = *lo as f64;
                    max = *hi as f64;
                }
                PropertyAttribute::Min { min: lo } => min = *lo as f64,
                PropertyAttribute::Max { max: hi } => max = *hi as f64,
                _ => {}
            }
        }
    }

    let editor: ElementBox = if let Some(variants) = &field.variants {
        let active = value
            .as_str()
            .unwrap_or_else(|| variants.first().map(String::as_str).unwrap_or("None"));
        button(
            field_node_key(entity_id, component, &field.name, None),
            active,
        )
    } else {
        match field.kind {
            "bool" => {
                let b = value.as_bool().unwrap_or(false);
                toggle_button(
                    field_node_key(entity_id, component, &field.name, None),
                    if b { "On" } else { "Off" },
                    b,
                )
            }
            "f32" | "f64" => {
                let n = value.as_f64().unwrap_or(0.0);
                numeric_field(
                    field_node_key(entity_id, component, &field.name, None),
                    n,
                    min,
                    max,
                    0.05,
                )
            }
            "i32" | "u32" | "i64" | "u64" => {
                let n = value.as_f64().unwrap_or(0.0);
                numeric_field(
                    field_node_key(entity_id, component, &field.name, None),
                    n,
                    min,
                    max,
                    1.0,
                )
            }
            "string" => {
                let s = value.as_str().unwrap_or("");
                text_field(
                    field_node_key(entity_id, component, &field.name, None),
                    s,
                    "...",
                )
            }
            "struct" if field.struct_type.as_deref() == Some("glam::Vec3") => {
                let arr = extract_vec3(value);
                let mut r = row().gap(4.0).align(Align::Center);
                let axes = [
                    ("X", Color::rgb(0.85, 0.30, 0.30), arr[0]),
                    ("Y", Color::rgb(0.32, 0.78, 0.36), arr[1]),
                    ("Z", Color::rgb(0.30, 0.56, 0.92), arr[2]),
                ];
                for (idx, (axis, color, v)) in axes.into_iter().enumerate() {
                    r = r
                        .child(
                            label(axis)
                                .padding(Edges::symmetric(4.0, 2.0))
                                .bg(color)
                                .radius(3.0),
                        )
                        .child(numeric_field(
                            field_node_key(entity_id, component, &field.name, Some(idx)),
                            v as f64,
                            f64::NEG_INFINITY,
                            f64::INFINITY,
                            0.05,
                        ));
                }
                r
            }
            _ => muted_label(format!("<{}>", field.kind)),
        }
    };

    row()
        .gap(8.0)
        .align(Align::Center)
        .justify(Justify::SpaceBetween)
        .child(muted_label(field.name.clone()).width(Length::Px(92.0)))
        .child(editor)
}

fn extract_vec3(value: &Value) -> [f32; 3] {
    if let Some(arr) = value.as_array() {
        return [
            arr.first().and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
            arr.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
            arr.get(2).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
        ];
    }
    if let Some(obj) = value.as_object() {
        return [
            obj.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
            obj.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
            obj.get("z").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
        ];
    }
    [0.0; 3]
}
