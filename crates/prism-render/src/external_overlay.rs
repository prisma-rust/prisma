//! Neutral "external overlay layer" interface — render thread can draw another layer on top of ECS UI
//! (debug HUD, host UI, etc.).
//!
//! Design: `GraphRenderer` does not know concrete overlay impls, only holds
//! [`SwapchainOverlay`] trait objects; impls lazily create their own
//! GPU resources during record. CPU frame data fed in via
//! `GraphRenderer::apply_overlay_message` with type-erased closures.
//!
//! Layout contract: this layer records after ECS UI (both final layout is
//! PRESENT_SRC_KHR), and `GraphRenderer` is responsible for inserting
//! PRESENT_SRC_KHR → COLOR_ATTACHMENT_OPTIMAL barrier before record.

use ash::vk;
use std::any::Any;
use std::fmt::Debug;

use crate::context::VulkanContext;

/// All render context needed to record one overlay (Vulkan handle + swapchain target).
pub struct OverlayRecordCtx<'a> {
    pub device: &'a ash::Device,
    pub context: &'a VulkanContext,
    pub command_pool: vk::CommandPool,
    pub graphics_queue: vk::Queue,
    pub cmd: vk::CommandBuffer,
    pub swapchain_views: &'a [vk::ImageView],
    pub image_index: u32,
    pub extent: vk::Extent2D,
    /// Swapchain color format (for lazily creating overlay own render pass).
    pub color_format: vk::Format,
}

impl Debug for OverlayRecordCtx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OverlayRecordCtx")
            .field("image_index", &self.image_index)
            .field("extent", &self.extent)
            .finish()
    }
}

/// One type-erased message delivered to overlay layer (like "this is new host UI frame").
///
/// Host builds closure capturing its own CPU frame data;
/// render thread extracts and applies it to overlay reference (inside closure can
/// downcast via [`AsAny::as_any_mut`] to concrete impl).
pub type OverlayMessage = Box<dyn FnOnce(&mut dyn SwapchainOverlay) + Send>;

/// Downcast capability for type-erased overlay references.
///
/// When host delivers overlay message, closure receives `&mut dyn SwapchainOverlay`;
/// via `as_any_mut().downcast_mut::<T>()` can recover concrete impl
/// (like debug HUD overlay impl).
pub trait AsAny: Send {
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Send + 'static> AsAny for T {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// One overlay layer recordable onto swapchain (cross-thread: main thread constructs,
/// render thread records).
pub trait SwapchainOverlay: AsAny {
    /// Whether has content to draw (false → `GraphRenderer` skips this layer,
    /// image stays PRESENT_SRC_KHR and presented directly).
    fn has_content(&self) -> bool;

    /// Record overlay draw commands into `ctx.cmd`.
    ///
    /// GPU resources can be lazily created here (ctx carries VulkanContext/command_pool).
    /// On call image already in COLOR_ATTACHMENT_OPTIMAL, this layer responsible for own
    /// render pass (final should be PRESENT_SRC_KHR).
    fn record(&mut self, ctx: &OverlayRecordCtx<'_>) -> anyhow::Result<()>;

    /// Called after swapchain rebuild: discard framebuffers built on old image views etc.
    fn on_swapchain_views_changed(&mut self, views: &[vk::ImageView], extent: vk::Extent2D);

    /// Release all GPU resources (when render thread exits).
    fn destroy(&mut self);
}
