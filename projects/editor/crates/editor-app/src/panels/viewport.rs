//! 3D/2D scene viewport panel.

use prism_ui::prelude::*;

use crate::panels::{DockZone, EditorPanel, PanelViewCtx};

pub struct ViewportPanel;

impl EditorPanel for ViewportPanel {
    fn id(&self) -> &'static str {
        "scene"
    }

    fn title_key(&self) -> &'static str {
        "window.scene"
    }

    fn default_dock(&self) -> DockZone {
        DockZone::CenterTop
    }

    fn closable(&self) -> bool {
        false
    }

    fn view(&self, _ctx: &PanelViewCtx, ui: &mut UiContext) {
        ui.mount(viewport(UiTextureId(0)));
    }
}
