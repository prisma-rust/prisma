//! Prisma engine library: render system, ECS component, resource parse,
//! scene management and editor integration.
//!
//! This is a **pure logic crate** — not depend on any window system.
//! Event loop, window creation and platform-related input dispatch bits in `src/app.rs` (binary crate).

#![deny(warnings)]

pub mod anim_ground;
pub mod asset;
pub mod asset_paths;
pub mod asset_resolver;
#[cfg(feature = "audio")]
pub mod audio;
pub mod config;
pub mod dirty_router;
pub mod ecs;
pub mod engine;
pub mod input;
pub mod input_actions;
pub mod launch_config;
pub mod physics;
pub mod physics2d;
#[cfg(test)]
mod physics2d_tests;
pub mod render_settings;
pub mod render_system;
pub mod scene;
pub mod scene_state;
pub mod sound;
pub mod ui;
pub mod user_data;
pub mod util;

pub use engine::Engine;
pub use input_actions::{key_code_from_vk, EngineAction, KeyBinding, KeyBindings};
pub use render_system::{euler_xyz_deg_to_dir, render_system};
