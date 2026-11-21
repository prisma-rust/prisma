//! Render-plugin model — reserved, not wired up.
//!
//! DESIGN §8.4 sketches a model where each plugin registers its pass nodes into
//! the RenderGraph in `setup` and is driven through update / prepare / render.
//! Only the trait and the registry exist: nothing constructs `PluginRegistry`,
//! nothing calls `setup_all`, and no type implements `RenderPlugin`.
//!
//! The graph today is assembled directly in `GraphRenderer::new`
//! (`ForwardPass` → `ShadowMapPass` → `GtaoPass` → `PostPass` in a fixed order).
//! Do not read this module as evidence that passes are pluggable; it is a
//! placeholder kept for the extension route described in DESIGN §8.4.

use crate::render_graph::{RenderGraphBuilder, RenderSettings};

pub trait RenderPlugin: Send {
    fn name(&self) -> &'static str;
    fn setup(&mut self, _graph: &mut RenderGraphBuilder, _settings: &RenderSettings) {}
    fn update(&mut self) {}
    fn prepare(&mut self) {}
    fn render(&mut self) {}
    fn shutdown(&mut self) {}
}

/// 空白 Plugin 注册表（薄封装）
pub struct PluginRegistry {
    plugins: Vec<Box<dyn RenderPlugin>>,
}
impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }
    pub fn register<P: RenderPlugin + 'static>(&mut self, p: P) {
        self.plugins.push(Box::new(p));
    }
    pub fn setup_all(&mut self, g: &mut RenderGraphBuilder, s: &RenderSettings) {
        for p in &mut self.plugins {
            p.setup(g, s);
        }
    }
}
impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}
