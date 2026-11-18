//! GPU timing and screenshot capture state for [`super::GraphRenderer`].

use std::path::PathBuf;

use anyhow::Context as _;
use ash::vk;

use crate::context::VulkanContext;
use crate::gpu_timer::{GpuHistory, GpuTimer, GpuTimings};
use crate::screenshot::{ScreenshotCapture, ScreenshotRequest};

/// Per-frame GPU timestamp profiling and one-shot screenshot capture state.
pub(super) struct RenderTelemetry {
    gpu_timer: Option<GpuTimer>,
    gpu_timings: GpuTimings,
    gpu_history: GpuHistory,
    pending_screenshot: Option<ScreenshotRequest>,
    screenshot: Option<ScreenshotCapture>,
}

impl RenderTelemetry {
    /// Initialize GPU timing (if supported by the physical device) and empty screenshot state.
    pub(super) fn new(context: &VulkanContext, frame_count: usize) -> anyhow::Result<Self> {
        let gpu_timer = GpuTimer::new(
            &context.instance,
            &context.device,
            context.physical_device,
            frame_count,
        )?;
        Ok(Self {
            gpu_timer,
            gpu_timings: GpuTimings::default(),
            gpu_history: GpuHistory::default(),
            pending_screenshot: None,
            screenshot: None,
        })
    }

    /// Read back completed timestamps for `frame_index` before opening the command buffer.
    pub(super) fn begin_frame_timings(&mut self, device: &ash::Device, frame_index: u32) {
        let Some(timer) = self.gpu_timer.as_mut() else {
            return;
        };
        timer.begin_frame(device, frame_index);
        self.gpu_timings = timer.timings(frame_index).clone();
        if self.gpu_timings.supported {
            self.gpu_history.push(self.gpu_timings.frame_or_total_ms());
        }
    }

    /// Open the frame GPU timestamp span after `vkBeginCommandBuffer`.
    pub(super) fn begin_frame_span(
        &mut self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        frame_index: u32,
    ) {
        if let Some(timer) = self.gpu_timer.as_mut() {
            timer.begin_frame_span(device, cmd, frame_index);
        }
    }

    /// Close the frame GPU timestamp span before ending the command buffer.
    pub(super) fn end_frame(
        &mut self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        frame_index: u32,
    ) {
        if let Some(timer) = self.gpu_timer.as_mut() {
            timer.end_frame_span(device, cmd, frame_index);
        }
    }

    pub(super) fn gpu_timer_mut(&mut self) -> Option<&mut GpuTimer> {
        self.gpu_timer.as_mut()
    }

    pub(super) fn gpu_timings(&self) -> &GpuTimings {
        &self.gpu_timings
    }

    pub(super) fn gpu_history(&self) -> &GpuHistory {
        &self.gpu_history
    }

    pub(super) fn request_screenshot(
        &mut self,
        path: PathBuf,
        size: Option<[u32; 2]>,
        scale: Option<f32>,
    ) {
        self.pending_screenshot = Some(ScreenshotRequest { path, size, scale });
    }

    /// Record a copy from the active swapchain/offscreen image when a screenshot was requested.
    pub(super) fn record_screenshot_if_requested(
        &mut self,
        context: &VulkanContext,
        cmd: vk::CommandBuffer,
        extent: vk::Extent2D,
        target: Option<(vk::Image, vk::ImageLayout)>,
    ) -> anyhow::Result<()> {
        let Some(request) = self.pending_screenshot.take() else {
            return Ok(());
        };
        let capture = ScreenshotCapture::new(context, extent, request)?;
        let (image, current_layout) = target
            .ok_or_else(|| anyhow::anyhow!("screenshot: no swapchain or offscreen target"))?;
        capture.record_copy(&context.device, cmd, image, current_layout)?;
        self.screenshot = Some(capture);
        Ok(())
    }

    /// Wait for the submitted fence, read back pixels, and write the PNG to disk.
    pub(super) fn finish_screenshot(
        &mut self,
        context: &VulkanContext,
        fence: vk::Fence,
        color_format: vk::Format,
    ) -> anyhow::Result<Option<PathBuf>> {
        let Some(mut capture) = self.screenshot.take() else {
            return Ok(None);
        };
        if fence != vk::Fence::null() {
            unsafe { context.device.wait_for_fences(&[fence], true, u64::MAX) }
                .context("screenshot: wait for fence")?;
        }
        let result = capture.finish(context, color_format);
        unsafe { capture.destroy(&context.device) };
        result.map(Some)
    }

    /// Destroy any pending screenshot capture and the GPU timestamp query pool.
    pub(super) fn destroy(&mut self, device: &ash::Device) {
        if let Some(mut capture) = self.screenshot.take() {
            unsafe { capture.destroy(device) };
        }
        if let Some(timer) = self.gpu_timer.as_mut() {
            timer.destroy(device);
        }
    }
}
