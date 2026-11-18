//! ECS UI overlay and external host overlay management for [`super::GraphRenderer`].

use std::collections::BTreeMap;

use anyhow::Context as _;
use ash::vk;

#[cfg(feature = "ui")]
use crate::context::VulkanContext;
use crate::external_overlay::{OverlayMessage, OverlayRecordCtx, SwapchainOverlay};

/// Owns the lazily created ECS UI overlays (swapchain + offscreen), registered
/// project fonts, and the optional external host overlay (e.g. editor UI).
#[derive(Default)]
pub(super) struct UiOverlayHost {
    external_overlay: Option<Box<dyn SwapchainOverlay>>,
    #[cfg(feature = "ui")]
    ui_overlay: Option<crate::ui_overlay::UiOverlay>,
    #[cfg(feature = "ui")]
    ui_overlay_offscreen: Option<crate::ui_overlay::UiOverlay>,
    ui_fonts: BTreeMap<String, Vec<u8>>,
}

#[cfg(feature = "ui")]
fn apply_ui_fonts(fonts: &BTreeMap<String, Vec<u8>>, overlay: &mut crate::ui_overlay::UiOverlay) {
    for (name, bytes) in fonts {
        overlay.register_font(name, bytes.clone());
    }
}

impl UiOverlayHost {
    pub(super) fn set_external_overlay(&mut self, overlay: Box<dyn SwapchainOverlay>) {
        self.external_overlay = Some(overlay);
    }

    pub(super) fn apply_overlay_message(&mut self, msg: OverlayMessage) {
        if let Some(overlay) = self.external_overlay.as_mut() {
            msg(overlay.as_mut());
        }
    }

    pub(super) fn register_ui_font(&mut self, name: String, bytes: Vec<u8>) {
        self.ui_fonts.insert(name.clone(), bytes.clone());
        #[cfg(feature = "ui")]
        {
            if let Some(gpu) = self.ui_overlay.as_mut() {
                gpu.register_font(&name, bytes.clone());
            }
            if let Some(gpu) = self.ui_overlay_offscreen.as_mut() {
                gpu.register_font(&name, bytes);
            }
        }
        #[cfg(not(feature = "ui"))]
        let _ = (name, bytes);
    }

    /// Release per-image-view framebuffers before swapchain recreation.
    pub(super) fn on_swapchain_pre_recreate(&mut self) {
        #[cfg(feature = "ui")]
        if let Some(gpu) = self.ui_overlay.as_mut() {
            gpu.on_swapchain_views_changed();
        }
    }

    /// Notify external overlay of new swapchain image views.
    pub(super) fn on_swapchain_post_recreate(
        &mut self,
        views: &[vk::ImageView],
        extent: vk::Extent2D,
    ) {
        if let Some(overlay) = self.external_overlay.as_mut() {
            overlay.on_swapchain_views_changed(views, extent);
        }
    }

    /// Release per-image-view framebuffers before offscreen target recreation.
    pub(super) fn on_offscreen_pre_recreate(&mut self) {
        #[cfg(feature = "ui")]
        {
            if let Some(gpu) = self.ui_overlay.as_mut() {
                gpu.on_swapchain_views_changed();
            }
            if let Some(gpu) = self.ui_overlay_offscreen.as_mut() {
                gpu.on_swapchain_views_changed();
            }
        }
    }

    #[cfg(feature = "ui")]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn record_ecs_ui_swapchain(
        &mut self,
        context: &VulkanContext,
        command_pool: vk::CommandPool,
        cmd: vk::CommandBuffer,
        color_format: vk::Format,
        image: vk::Image,
        view: vk::ImageView,
        extent: vk::Extent2D,
        dest: vk::Rect2D,
        ui_overlay: &crate::ui_overlay::UiOverlayInput,
        bindless_layout: vk::DescriptorSetLayout,
        bindless_set: vk::DescriptorSet,
        frame_index: u32,
    ) -> anyhow::Result<()> {
        if self.ui_overlay.is_none() {
            let mut gpu = crate::ui_overlay::UiOverlay::with_color_format(
                context,
                command_pool,
                color_format,
                bindless_layout,
            )
            .context("create ui overlay")?;
            apply_ui_fonts(&self.ui_fonts, &mut gpu);
            self.ui_overlay = Some(gpu);
        }
        if let Some(gpu) = self.ui_overlay.as_mut() {
            gpu.set_color_format(color_format)
                .context("ui overlay color format")?;
            // PostPass leaves the swapchain image in PRESENT; turn it
            // back into a color attachment for the UI load pass.
            let barrier = vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .src_access_mask(vk::AccessFlags::NONE)
                .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE);
            unsafe {
                context.device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    std::slice::from_ref(&barrier),
                );
            }
            gpu.record(
                context,
                cmd,
                extent,
                dest,
                view,
                ui_overlay,
                bindless_set,
                frame_index,
            )
            .context("ui overlay record")?;
        }
        Ok(())
    }

    #[cfg(feature = "ui")]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn record_ecs_ui_offscreen(
        &mut self,
        context: &VulkanContext,
        command_pool: vk::CommandPool,
        cmd: vk::CommandBuffer,
        color_format: vk::Format,
        image: vk::Image,
        view: vk::ImageView,
        target_extent: vk::Extent2D,
        dest: vk::Rect2D,
        ui_overlay: &crate::ui_overlay::UiOverlayInput,
        bindless_layout: vk::DescriptorSetLayout,
        bindless_set: vk::DescriptorSet,
        frame_index: u32,
    ) -> anyhow::Result<()> {
        if self.ui_overlay_offscreen.is_none() {
            let mut gpu = crate::ui_overlay::UiOverlay::with_color_format_and_final_layout(
                context,
                command_pool,
                color_format,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                bindless_layout,
            )
            .context("create offscreen ui overlay")?;
            apply_ui_fonts(&self.ui_fonts, &mut gpu);
            self.ui_overlay_offscreen = Some(gpu);
        }
        if let Some(gpu) = self.ui_overlay_offscreen.as_mut() {
            gpu.set_color_format(color_format)
                .context("offscreen ui overlay color format")?;
            // The graph already leaves the target in COLOR_ATTACHMENT_OPTIMAL;
            // this barrier only makes its color writes visible to the UI pass.
            let barrier = vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                .dst_access_mask(
                    vk::AccessFlags::COLOR_ATTACHMENT_READ
                        | vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                );
            unsafe {
                context.device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                    vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    std::slice::from_ref(&barrier),
                );
            }
            gpu.record(
                context,
                cmd,
                target_extent,
                dest,
                view,
                ui_overlay,
                bindless_set,
                frame_index,
            )
            .context("offscreen ui overlay record")?;
        }
        Ok(())
    }

    pub(super) fn has_external_overlay_content(&self) -> bool {
        self.external_overlay
            .as_ref()
            .is_some_and(|o| o.has_content())
    }

    pub(super) fn record_external_overlay(
        &mut self,
        oc: &OverlayRecordCtx<'_>,
    ) -> anyhow::Result<()> {
        if let Some(overlay) = self.external_overlay.as_mut() {
            overlay.record(oc).context("external overlay record")?;
        }
        Ok(())
    }

    pub(super) fn destroy(&mut self) {
        if let Some(overlay) = self.external_overlay.as_mut() {
            overlay.destroy();
        }
    }
}
