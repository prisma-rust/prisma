//! Prisma editor (egui + ash Vulkan).
//!
//! Shares same data layer (`prism-editor-core`) with Web editor (Tauri + Vue),
//! Shares same layout / document / undo model with Web editor. Key difference in viewport:
//! Scene / Game view **zero-copy** --- engine renders to offscreen `VkImage`, egui samples directly via
//! the same `VkDevice`, no GPU->CPU readback.
//!
//! Run: `cargo run -p prism-editor-egui` (needs Vulkan device + compiled `.spv`).

mod app;
mod build;
mod control;
mod diagnostics;
mod egui_host;
mod engine_host;
mod gasp_debug;
mod gpu;
mod logging;
mod loose_assets;
mod palette;
mod panels;
mod project_module;
mod screenshot;
mod selection_history;
mod skeleton_overlay;
#[cfg(test)]
mod test_harness;
mod theme;
mod ui_gizmo;
mod ui_tree;
mod viewport;
mod widgets;

use crate::egui_host::{AppCreator, AshRenderState, CreationContext, RunOption};
use gpu_allocator::vulkan::Allocator;
use std::sync::{Arc, Mutex};

struct EditorCreator;

impl AppCreator<Arc<Mutex<Allocator>>> for EditorCreator {
    type App = app::EditorApp;

    fn create(&self, cc: CreationContext) -> (Self::App, AshRenderState<Arc<Mutex<Allocator>>>) {
        let app = match app::EditorApp::new(&cc) {
            Ok(app) => app,
            Err(error) => {
                panic!("failed to create Prisma editor: {error:#}");
            }
        };
        let render_state = app.render_state();
        (app, render_state)
    }
}

fn main() -> std::process::ExitCode {
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!("prism-editor-egui {}", env!("CARGO_PKG_VERSION"));
        return std::process::ExitCode::SUCCESS;
    }

    logging::install("warn");

    if std::env::args().any(|arg| arg == "--no-control") {
        control::set_cli_disabled(true);
    }

    let options = RunOption {
        clear_color: [0.055, 0.06, 0.07, 1.0],
        viewport_builder: Some(
            egui::ViewportBuilder::default()
                .with_title("Prisma Editor")
                .with_inner_size(egui::vec2(1440.0, 900.0))
                .with_min_inner_size(egui::vec2(900.0, 600.0)),
        ),
        ..Default::default()
    };

    crate::egui_host::run("prism-editor-egui", EditorCreator, options)
}
