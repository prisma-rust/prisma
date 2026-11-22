//! Modular, plugin-driven editor panel system.
//!
//! Every panel implements [`EditorPanel`], receives a read-only [`PanelViewCtx`]
//! during [`EditorPanel::view`], and emits [`EditorCommand`]s from
//! [`EditorPanel::handle_action`]. Panels never mutate state directly and never
//! touch engine internals (`World`, Vulkan, winit).

pub mod assets;
pub mod hierarchy;
pub mod inspector_panel;
pub mod viewport;

#[cfg(test)]
mod _template;

use glam::Vec2;
use prism_ui::prelude::*;

use crate::commands::EditorCommand;
use crate::i18n::EditorI18n;
use crate::state::{EditorDocument, EditorSession};

/// Build a panel-scoped [`NodeKey`] so different panels never collide on widget keys.
#[macro_export]
macro_rules! panel_key {
    ($panel_id:expr, $local:expr) => {
        ($panel_id, $local).into_node_key()
    };
}

/// Default dock slot assigned when building the initial workspace layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockZone {
    Left,
    CenterTop,
    CenterBottom,
    Right,
}

/// Strictly read-only context passed to [`EditorPanel::view`] and [`EditorPanel::handle_action`].
pub struct PanelViewCtx<'a> {
    pub doc: &'a EditorDocument,
    pub session: &'a EditorSession,
    pub theme: &'a Theme,
    pub i18n: &'a EditorI18n,
    pub icons: &'a IconSet,
    pub viewport_size: Vec2,
}

/// Contract implemented by every dockable editor panel.
pub trait EditorPanel: Send + Sync {
    /// Stable window identifier (e.g. `"hierarchy"`, `"inspector"`).
    fn id(&self) -> &'static str;

    /// Localization key used for the panel tab title.
    fn title_key(&self) -> &'static str;

    /// Default dock zone for this panel when resetting or initializing layout.
    fn default_dock(&self) -> DockZone {
        DockZone::Right
    }

    /// Whether the user can close this panel tab.
    fn closable(&self) -> bool {
        true
    }

    /// Build the panel's UI tree from read-only editor state.
    fn view(&self, ctx: &PanelViewCtx, ui: &mut UiContext);

    /// Translate a UI interaction into zero or more [`EditorCommand`]s.
    fn handle_action(
        &mut self,
        _action: &UiAction,
        _ctx: &PanelViewCtx,
        _out: &mut Vec<EditorCommand>,
    ) {
    }
}

/// Plugin contract for grouping and registering editor panels.
pub trait EditorPlugin {
    /// Unique identifier for this plugin.
    fn id(&self) -> &'static str;
    /// Register panels (and future extensions) into `registry`.
    fn register(&self, registry: &mut PanelRegistry);
}

/// Built-in plugin providing the core Hierarchy, Scene, Assets, and Inspector panels.
pub struct CoreEditorPlugin;

impl EditorPlugin for CoreEditorPlugin {
    fn id(&self) -> &'static str {
        "core"
    }

    fn register(&self, registry: &mut PanelRegistry) {
        registry.register(Box::new(hierarchy::HierarchyPanel));
        registry.register(Box::new(viewport::ViewportPanel));
        registry.register(Box::new(assets::AssetsPanel));
        registry.register(Box::new(inspector_panel::InspectorPanel));
    }
}

/// Central registry of all dockable editor panels.
pub struct PanelRegistry {
    panels: Vec<Box<dyn EditorPanel>>,
}

impl Default for PanelRegistry {
    fn default() -> Self {
        let mut reg = Self::empty();
        reg.install_plugin(&CoreEditorPlugin);
        reg
    }
}

impl PanelRegistry {
    /// Create an empty registry with no panels installed.
    pub fn empty() -> Self {
        Self { panels: Vec::new() }
    }

    /// Install all panels contributed by `plugin`.
    pub fn install_plugin(&mut self, plugin: &dyn EditorPlugin) {
        let _id = plugin.id();
        plugin.register(self);
    }

    /// Register a panel dynamically; ignores duplicate IDs to keep window keys unique.
    pub fn register(&mut self, panel: Box<dyn EditorPanel>) {
        if self.panels.iter().any(|p| p.id() == panel.id()) {
            log::warn!("editor: ignoring duplicate panel id '{}'", panel.id());
            return;
        }
        self.panels.push(panel);
    }

    /// Iterate over all registered panels in registration order.
    pub fn iter(&self) -> impl Iterator<Item = &dyn EditorPanel> {
        self.panels.iter().map(|p| p.as_ref())
    }

    /// Register every panel's window metadata into `layout`.
    pub fn register_windows(&self, layout: &mut WindowLayout, i18n: &EditorI18n) {
        for panel in &self.panels {
            let title = i18n.t(panel.title_key());
            layout.register_window(
                EditorWindowMeta::new(panel.id(), title).with_closable(panel.closable()),
            );
        }
    }

    /// Build the default Unity-inspired docking tree dynamically from registered panels' [`DockZone`]s.
    pub fn build_default_layout(
        &self,
        i18n: &EditorI18n,
        about_close_key: NodeKey,
    ) -> WindowLayout {
        let mut layout = WindowLayout::new();
        self.register_windows(&mut layout, i18n);
        layout.register_window(
            EditorWindowMeta::new("about", i18n.t("window.about"))
                .with_min_size(Vec2::new(360.0, 240.0))
                .with_closable(true)
                .with_close_key(about_close_key),
        );

        let mut left = Vec::new();
        let mut center_top = Vec::new();
        let mut center_bottom = Vec::new();
        let mut right = Vec::new();
        for panel in &self.panels {
            let win_id = WindowId::from(panel.id());
            match panel.default_dock() {
                DockZone::Left => left.push(win_id),
                DockZone::CenterTop => center_top.push(win_id),
                DockZone::CenterBottom => center_bottom.push(win_id),
                DockZone::Right => right.push(win_id),
            }
        }

        let left_area = layout.alloc_area_id();
        let center_top_area = layout.alloc_area_id();
        let center_bottom_area = layout.alloc_area_id();
        let right_area = layout.alloc_area_id();
        let center_split = layout.alloc_split_id();
        let right_split = layout.alloc_split_id();
        let root_split = layout.alloc_split_id();

        layout.main_root = DockNode::Split(SplitView {
            id: root_split,
            axis: SplitAxis::Horizontal,
            ratio: 0.20,
            first: Box::new(DockNode::Area(DockArea::new(left_area, left))),
            second: Box::new(DockNode::Split(SplitView {
                id: right_split,
                axis: SplitAxis::Horizontal,
                ratio: 0.68,
                first: Box::new(DockNode::Split(SplitView {
                    id: center_split,
                    axis: SplitAxis::Vertical,
                    ratio: 0.76,
                    first: Box::new(DockNode::Area(DockArea::new(center_top_area, center_top))),
                    second: Box::new(DockNode::Area(DockArea::new(
                        center_bottom_area,
                        center_bottom,
                    ))),
                })),
                second: Box::new(DockNode::Area(DockArea::new(right_area, right))),
            })),
        });
        layout
    }

    /// Refresh all registered panel titles after a locale change.
    pub fn sync_window_titles(&self, layout: &mut WindowLayout, i18n: &EditorI18n) {
        for panel in &self.panels {
            if let Some(meta) = layout.windows.get_mut(&WindowId::from(panel.id())) {
                meta.title = i18n.t(panel.title_key()).to_string();
            }
        }
    }

    /// Mount the panel matching `window_id` into `ui`, returning `true` if found.
    pub fn mount_panel(&self, window_id: &str, ctx: &PanelViewCtx, ui: &mut UiContext) -> bool {
        let Some(panel) = self.panels.iter().find(|p| p.id() == window_id) else {
            return false;
        };
        panel.view(ctx, ui);
        true
    }

    /// Let every panel translate `action` into [`EditorCommand`]s.
    pub fn handle_action(
        &mut self,
        action: &UiAction,
        ctx: &PanelViewCtx,
        out: &mut Vec<EditorCommand>,
    ) {
        for panel in &mut self.panels {
            panel.handle_action(action, ctx, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct CustomPluginPanel;
    impl EditorPanel for CustomPluginPanel {
        fn id(&self) -> &'static str {
            "profiler"
        }
        fn title_key(&self) -> &'static str {
            "Profiler"
        }
        fn default_dock(&self) -> DockZone {
            DockZone::CenterBottom
        }
        fn view(&self, _ctx: &PanelViewCtx, ui: &mut UiContext) {
            ui.mount(label("Profiler"));
        }
    }

    struct CustomPlugin;
    impl EditorPlugin for CustomPlugin {
        fn id(&self) -> &'static str {
            "custom_profiler"
        }
        fn register(&self, registry: &mut PanelRegistry) {
            registry.register(Box::new(CustomPluginPanel));
        }
    }

    #[test]
    fn custom_plugin_registers_panel_into_layout_and_iteration() {
        let mut reg = PanelRegistry::default();
        reg.install_plugin(&CustomPlugin);
        assert!(reg.iter().any(|p| p.id() == "profiler"));

        let layout =
            reg.build_default_layout(&EditorI18n::default(), "about:close".into_node_key());
        assert!(layout.is_window_open(&WindowId::from("profiler")));
    }
}
