//! Render state enum — authoritative definition already extracted into [`prism_gfx`].
//!
//! These types are scene schema (`prism-scene`) and Vulkan pipeline state
//! (`prism-render`) input, so placed in standalone crate, shared by both crates.
//! This retains re-export, `prism_scene::render_enums::*` and
//! `prism_scene::CullMode` etc. historical paths continue available.

pub use prism_gfx::*;
