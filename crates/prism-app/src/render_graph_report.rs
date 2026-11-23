//! Old path of render graph report.
//!
//! DTO and writer sunk into [`prism_render::render_graph_report`] (pure data,
//! editor also uses same one); here only re-export, keep `prism_app::render_graph_report`
//! existing references available.

#[expect(
    unused_imports,
    reason = "imports are kept for the feature-gated report body"
)]
pub use prism_render::render_graph_report::{
    RenderGraphReport, ReportHandle, ReportPass, ReportResource, ReportSettings, ReportWriter,
    ENV_KEY, REFRESH_INTERVAL_FRAMES,
};
