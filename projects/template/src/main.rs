//! Prisma user game project entry point.
//!
//! Engine initialization, event loop, rendering entirely handled by [`prism_app::app`]; this project
//! integrates via pure ECS:
//! - `register_scene`: register startup scene ECS entities and resources;
//! - `add_scene_system`: register scene ECS systems;
//! - `run()`: start event loop, engine automatically drives scene per `PRISMA_LAUNCH_CONFIG`.
//!
//! Desktop side spawned by launcher/ as `template` binary (hub mode); settable
//! `PRISMA_LAUNCH_CONFIG` env overrides launch config (see `LaunchConfig`).
//! Android entry in `lib.rs` (cdylib `libtemplate_core.so`).

#![deny(warnings)]

fn main() {
    // Build app (complete engine init), register intro scene then launch.
    template_core::build_app().run();
}
