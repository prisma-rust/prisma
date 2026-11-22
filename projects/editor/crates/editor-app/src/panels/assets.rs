//! Project asset browser panel.

use prism_ui::prelude::*;

use crate::panels::{DockZone, EditorPanel, PanelViewCtx};
use crate::widgets::panel_shell;

pub struct AssetsPanel;

impl EditorPanel for AssetsPanel {
    fn id(&self) -> &'static str {
        "assets"
    }

    fn title_key(&self) -> &'static str {
        "window.assets"
    }

    fn default_dock(&self) -> DockZone {
        DockZone::CenterBottom
    }

    fn view(&self, ctx: &PanelViewCtx, ui: &mut UiContext) {
        let asset_list = ctx.doc.assets.clone();
        ui.mount(panel_shell("ASSETS", 8.0, 4.0).child(virtual_list(
            "assets:list",
            asset_list.len(),
            22.0,
            100.0,
            0.0,
            move |idx| button(("asset", idx as u32), asset_list[idx].clone()),
        )));
    }
}
