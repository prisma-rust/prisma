//! Scene hierarchy tree panel with search filtering and entity selection.

use prism_ui::prelude::*;

use crate::commands::EditorCommand;
use crate::panels::{DockZone, EditorPanel, PanelViewCtx};
use crate::widgets::{entity_row, panel_shell, search_bar};

pub struct HierarchyPanel;

impl EditorPanel for HierarchyPanel {
    fn id(&self) -> &'static str {
        "hierarchy"
    }

    fn title_key(&self) -> &'static str {
        "window.hierarchy"
    }

    fn default_dock(&self) -> DockZone {
        DockZone::Left
    }

    fn view(&self, ctx: &PanelViewCtx, ui: &mut UiContext) {
        let query = ctx.session.search_query.clone();
        let selected = ctx.doc.selected_entity;
        let hierarchy = ctx.doc.visible_hierarchy_rows(&query);
        let tree_meta: Vec<(u32, bool, bool)> = hierarchy
            .iter()
            .map(|(id, _, has_c, exp, _)| (*id, *has_c, *exp))
            .collect();
        let tree_depths: Vec<u16> = hierarchy.iter().map(|(_, d, _, _, _)| *d).collect();
        let tree_names: Vec<(u32, String)> = hierarchy
            .iter()
            .map(|(id, _, _, _, name)| (*id, name.clone()))
            .collect();

        ui.mount(
            panel_shell("HIERARCHY", 8.0, 6.0)
                .child(search_bar("hierarchy:search", query, "Filter..."))
                .child(tree_view(
                    "hierarchy:tree",
                    tree_meta,
                    tree_depths,
                    24.0,
                    (ctx.viewport_size.y - 220.0).max(120.0),
                    0.0,
                    move |idx| {
                        let (id, ref name) = tree_names[idx];
                        entity_row(id, name.clone(), id == selected)
                    },
                )),
        );
    }

    fn handle_action(
        &mut self,
        action: &UiAction,
        ctx: &PanelViewCtx,
        out: &mut Vec<EditorCommand>,
    ) {
        match action {
            UiAction::TextChanged { key, text } if *key == "hierarchy:search".into_node_key() => {
                out.push(EditorCommand::SetSearchQuery(text.clone()));
            }
            UiAction::Toggled { key, value } => {
                for ent in &ctx.doc.entities {
                    if *key == ("tree_exp", ent.id).into_node_key() {
                        out.push(EditorCommand::SetEntityExpanded {
                            id: ent.id,
                            expanded: *value,
                        });
                    } else if *key == ("entity", ent.id).into_node_key() {
                        out.push(EditorCommand::SelectEntity(ent.id));
                    }
                }
            }
            _ => {}
        }
    }
}
