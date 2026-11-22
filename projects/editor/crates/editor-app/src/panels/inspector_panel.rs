//! Schema-driven entity inspector panel.

use prism_ui::prelude::*;
use serde_json::json;

use crate::commands::{DocCommand, EditorCommand};
use crate::inspector::{build_inspector_ui, field_node_key, FieldEdit};
use crate::panels::{DockZone, EditorPanel, PanelViewCtx};
use crate::widgets::panel_shell;

pub struct InspectorPanel;

impl EditorPanel for InspectorPanel {
    fn id(&self) -> &'static str {
        "inspector"
    }

    fn title_key(&self) -> &'static str {
        "window.inspector"
    }

    fn default_dock(&self) -> DockZone {
        DockZone::Right
    }

    fn view(&self, ctx: &PanelViewCtx, ui: &mut UiContext) {
        let selected = ctx.doc.selected_entity;
        let body = ctx
            .doc
            .selected_entity_ref()
            .map(|e| build_inspector_ui(&ctx.doc.schema, e.id, &e.components))
            .unwrap_or_else(|| muted_label("No entity selected"));

        ui.mount(panel_shell(format!("INSPECTOR (#{selected})"), 10.0, 8.0).child(body));
    }

    fn handle_action(
        &mut self,
        action: &UiAction,
        ctx: &PanelViewCtx,
        out: &mut Vec<EditorCommand>,
    ) {
        match action {
            UiAction::ValueChanged { key, value } => {
                if let Some((comp, field, axis)) = find_matching_field(ctx, *key) {
                    out.push(EditorCommand::PreviewField {
                        entity_id: ctx.doc.selected_entity,
                        component: comp,
                        field,
                        axis,
                        value: json!(*value),
                    });
                }
            }
            UiAction::ValueCommitted { key, old, new } => {
                if let Some((comp, field, axis)) = find_matching_field(ctx, *key) {
                    out.push(EditorCommand::Doc(DocCommand::SetField(FieldEdit {
                        entity_id: ctx.doc.selected_entity,
                        component: comp,
                        field,
                        axis,
                        old_value: json!(*old),
                        new_value: json!(*new),
                    })));
                }
            }
            _ => {}
        }
    }
}

fn find_matching_field(
    ctx: &PanelViewCtx,
    key: NodeKey,
) -> Option<(String, String, Option<usize>)> {
    let selected = ctx.doc.selected_entity;
    let ent = ctx.doc.selected_entity_ref()?;
    for comp_name in ent.components.keys() {
        let Some(ts) = ctx.doc.schema.type_by_name(comp_name) else {
            continue;
        };
        for field in &ts.fields {
            if key == field_node_key(selected, comp_name, &field.name, None) {
                return Some((comp_name.clone(), field.name.clone(), None));
            }
            for axis in 0..3 {
                if key == field_node_key(selected, comp_name, &field.name, Some(axis)) {
                    return Some((comp_name.clone(), field.name.clone(), Some(axis)));
                }
            }
        }
    }
    None
}
