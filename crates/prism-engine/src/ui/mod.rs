//! UI system — declarative ECS-based UI
//!
//! # Architecture
//!
//! ```text
//! UI entity  = Node + Style + (Text | Interaction)
//!
//! `ComputedLayout` inserted on demand by Layout System and rewritten each frame, author does not write it.
//!
//! [Layout System]    read  Style → write  ComputedLayout
//! [Render System]    read  ComputedLayout + Text → generate  UiDrawList
//! ```
//!
//! Each UI element is an ECS Entity, appearance and behavior defined by component composition.
//! Panel base class wraps common UI Entity creation logic.
//!
//! # Integration
//!
//! The layout / input / render systems are part of `Engine::default_schedule()`, so
//! they run on every tick without registration. A host that builds its own schedule
//! adds them the same way any other system is added:
//!
//! ```ignore
//! engine.schedule_mut().add_system("ui::layout", |world, _dt| {
//!     crate::ui::ui_layout_system(world)
//! });
//! ```

pub mod components {
    //! UI schema components now live in `prism-scene`; re-export for path compatibility.
    pub use prism_scene::ui::*;
}
mod input;
mod layout;
mod no_scene;
mod panel_base;
mod particles;
mod render;

pub use components::*;
pub use input::{ui_input_system, UiFocus, UiInputState, UiPressState};
pub use layout::{assign_draw_order, ui_layout_system, ScreenSize, UiScreenMapping};
pub use no_scene::{push_no_scene_screen, push_no_scene_screen_into};
pub use panel_base::PanelBase;
pub use particles::{Particle2D, ParticleSystem2D, SpawnKind};
pub use render::{
    convert_ui_draw_list_to_overlay, convert_ui_draw_list_to_overlay_into, merge_external_quads,
    ui_render_system, ExternalUiQuads, UiDrawItem, UiDrawList, UiQuad, UiTextCmd, EXTERNAL_LAYER,
    EXTERNAL_UI_ORDER,
};

/// One RGBA8 UI image queued for upload into the renderer's bindless table.
#[derive(Clone, Debug)]
pub struct PendingUiImageUpload {
    pub key: String,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Queue of UI RGBA8 textures (e.g. rasterized SVG icons) waiting for GPU upload.
#[derive(Clone, Debug, Default)]
pub struct PendingUiImageUploads {
    pub items: Vec<PendingUiImageUpload>,
}

/// Bindless SRV slots assigned by the renderer for uploaded UI images.
#[derive(Clone, Debug, Default)]
pub struct UploadedUiImages {
    pub slots: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, u32>>>,
}

/// Whether entity and its ancestor chain are all in active state.
///
/// Missing `Active` treated as active — only explicitly disabled entities carry `Active(false)`.
/// Parent disable hides entire child tree, so this walks up along `Parent`.
///
/// Both render (`ui_render_system`) and hit test (`ui_input_system`) must use it,
/// otherwise "hidden" only affects editor panel visibility icons, rendering and clicks unaffected.
pub(crate) fn is_ui_active(world: &prism_ecs::World, entity: prism_ecs::Entity) -> bool {
    let mut current = entity;
    // Max 64 levels up on cyclic dirty data (consistent with depth limit in `ui::input`).
    for _ in 0..64 {
        if matches!(
            world.get::<prism_scene::components::Active>(current),
            Some(prism_scene::components::Active(false))
        ) {
            return false;
        }
        // Scene view speech-level hide (#417) also must block UI draw and hit test; at runtime
        // This component absent, so game process unaffected by editor visibility.
        if world
            .get::<crate::scene::runtime_components::Hidden>(current)
            .is_some_and(|hidden| hidden.0)
        {
            return false;
        }
        match world.get::<prism_scene::components::Parent>(current) {
            Some(parent) => current = parent.0,
            None => return true,
        }
    }
    true
}

/// UI panel interface (to be gradually replaced by ECS component approach).
pub trait Panel {
    /// Update each frame. Return `false` means panel requests disable.
    fn on_update(&mut self, _dt: f32) -> bool {
        true
    }
    /// Handle input events.
    fn on_event(&mut self, _event: &()) {}
    /// Cleanup callback when panel disabled.
    fn on_close(&mut self) {}
}
