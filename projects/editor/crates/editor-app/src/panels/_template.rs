//! Copy-pasteable template for adding a new editor panel.
//!
//! Steps to add a new panel:
//! 1. Copy this file to `src/panels/<your_panel>.rs` and add `pub mod <your_panel>;` in `panels/mod.rs`.
//! 2. Implement [`EditorPanel`] (note `view` only receives `&PanelViewCtx`, never `&mut`).
//! 3. Register `Box::new(<your_panel>::YourPanel)` in `PanelRegistry::default()`.
//! 4. Add a headless unit test at the bottom of your file like the one below.

use glam::Vec2;
use prism_ui::prelude::*;

use crate::commands::{DocCommand, EditorCommand};
use crate::i18n::EditorI18n;
use crate::panel_key;
use crate::panels::{EditorPanel, PanelViewCtx};
use crate::state::{EditorDocument, EditorSession};
use crate::widgets::panel_shell;

pub struct ExamplePanel;

impl EditorPanel for ExamplePanel {
    fn id(&self) -> &'static str {
        "example"
    }

    fn title_key(&self) -> &'static str {
        "window.example"
    }

    fn view(&self, ctx: &PanelViewCtx, ui: &mut UiContext) {
        ui.mount(
            panel_shell("EXAMPLE", 8.0, 6.0)
                .child(label(format!("Entities: {}", ctx.doc.entities.len())))
                .child(button(panel_key!(self.id(), "add_entity"), "Create Entity")),
        );
    }

    fn handle_action(
        &mut self,
        action: &UiAction,
        ctx: &PanelViewCtx,
        out: &mut Vec<EditorCommand>,
    ) {
        if let UiAction::Clicked(key) = action {
            if *key == panel_key!(self.id(), "add_entity") {
                let next_id = ctx.doc.next_entity_id();
                out.push(EditorCommand::Doc(DocCommand::CreateEntity {
                    id: next_id,
                    name: format!("Entity_{next_id}"),
                    parent: Some(1),
                }));
            }
        }
    }
}

#[test]
fn template_panel_emits_create_entity_command_on_click() {
    let doc = EditorDocument::default();
    let session = EditorSession::default();
    let theme = Theme::dark();
    let i18n = EditorI18n::default();
    let icons = IconSet::default();
    let ctx = PanelViewCtx {
        doc: &doc,
        session: &session,
        theme: &theme,
        i18n: &i18n,
        icons: &icons,
        viewport_size: Vec2::new(400.0, 300.0),
    };

    let mut tree = UiTree::new();
    tree.begin_frame();
    {
        let mut ui = UiContext::new(&mut tree);
        ExamplePanel.view(&ctx, &mut ui);
    }
    tree.compute_layout(ctx.viewport_size);

    let mut panel = ExamplePanel;
    let mut out = Vec::new();
    panel.handle_action(
        &UiAction::Clicked(panel_key!(panel.id(), "add_entity")),
        &ctx,
        &mut out,
    );
    assert_eq!(
        out,
        vec![EditorCommand::Doc(DocCommand::CreateEntity {
            id: 4,
            name: "Entity_4".to_string(),
            parent: Some(1),
        })]
    );
}
