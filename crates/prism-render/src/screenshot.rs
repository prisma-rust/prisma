//! Frame screenshot: read back last frame swapchain image to CPU, optional scale to target resolution then write PNG.
//!
//! Usage is two-phase, because readback must happen after frame submit:
//!
//! ```ignore
//! renderer.request_screenshot(ScreenshotRequest { path, size: Some([1920, 1080]) });
//! // ... on next frame execute insert copy, present submit ...
//! renderer.finish_screenshot(&ctx)?;
//! ```
//!
//! `execute` inserts one `vkCmdCopyImageToBuffer` before returning swapchain image to present
//! (`PRESENT_SRC → TRANSFER_SRC → PRESENT_SRC`),
//! so screenshot gets the frame **that was on screen**, not a re-render.
//!
//! Scaling uses nearest neighbor: pixel art keeps hard edges after upscale, no interpolation.

use std::path::PathBuf;

use anyhow::{Context, Result};
use ash::vk;

use crate::buffer::find_memory_type;
use crate::context::VulkanContext;

/// One screenshot request.
#[derive(Debug, Clone)]
pub struct ScreenshotRequest {
    /// PNG output path (parent directory must already exist).
    pub path: PathBuf,
    /// Target resolution; `None` means same as swapchain.
    pub size: Option<[u32; 2]>,
    /// Target scale factor (relative swapchain resolution); `size` takes priority.
    pub scale: Option<f32>,
}

/// Write RGBA8 pixels to PNG, optional nearest-neighbor scale to target resolution.
///
/// For host reuse (editor window screenshot, offscreen output): caller only responsible for getting RGBA8.
pub fn save_rgba_png(
    path: &std::path::Path,
    width: u32,
    height: u32,
    pixels: &[u8],
    size: Option<[u32; 2]>,
) -> Result<PathBuf> {
    let expected = (width as usize) * (height as usize) * 4;
    if pixels.len() != expected {
        anyhow::bail!(
            "screenshot: pixel buffer is {} bytes, expected {expected} for {width}x{height}",
            pixels.len()
        );
    }
    let mut image = image::RgbaImage::from_raw(width, height, pixels.to_vec())
        .context("screenshot: build image buffer")?;
    if let Some([target_w, target_h]) = size {
        if target_w != width || target_h != height {
            image = image::imageops::resize(
                &image,
                target_w,
                target_h,
                image::imageops::FilterType::Nearest,
            );
        }
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("screenshot: create {}", parent.display()))?;
        }
    }
    image
        .save(path)
        .with_context(|| format!("screenshot: write {}", path.display()))?;
    log::info!(
        "screenshot: {}x{} -> {}",
        image.width(),
        image.height(),
        path.display()
    );
    Ok(path.to_path_buf())
}

/// Convert BGRA8 in place to RGBA8 (swapchain / Win32 DIB both BGRA order).
pub fn bgra_to_rgba_in_place(pixels: &mut [u8]) {
    for texel in pixels.as_chunks_mut::<4>().0 {
        texel.swap(0, 2);
    }
}

/// One in-progress screenshot: host-visible buffer + request.
pub(crate) struct ScreenshotCapture {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    extent: vk::Extent2D,
    request: ScreenshotRequest,
}

impl ScreenshotCapture {
    /// Allocate readback buffer by swapchain size.
    pub(crate) fn new(
        context: &VulkanContext,
        extent: vk::Extent2D,
        mut request: ScreenshotRequest,
    ) -> Result<Self> {
        if request.size.is_none() {
            if let Some(scale) = request.scale.filter(|s| *s > 0.0) {
                let w = ((extent.width as f32) * scale).round().max(1.0) as u32;
                let h = ((extent.height as f32) * scale).round().max(1.0) as u32;
                request.size = Some([w, h]);
            }
        }
        let size = (extent.width as u64) * (extent.height as u64) * 4;
        let buffer_info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(vk::BufferUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = unsafe { context.device.create_buffer(&buffer_info, None) }
            .context("screenshot: create buffer")?;
        let requirements = unsafe { context.device.get_buffer_memory_requirements(buffer) };
        let memory_type = find_memory_type(
            context,
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .context("screenshot: no host-visible memory type")?;
        let alloc_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type);
        let memory = match unsafe { context.device.allocate_memory(&alloc_info, None) } {
            Ok(memory) => memory,
            Err(err) => {
                unsafe { context.device.destroy_buffer(buffer, None) };
                return Err(err).context("screenshot: allocate memory");
            }
        };
        unsafe { context.device.bind_buffer_memory(buffer, memory, 0) }
            .context("screenshot: bind buffer memory")?;
        Ok(Self {
            buffer,
            memory,
            extent,
            request,
        })
    }

    /// Record in command buffer: image → buffer → resume original layout.
    ///
    /// Supports window mode (`PRESENT_SRC_KHR`) and windowless offscreen mode (`COLOR_ATTACHMENT_OPTIMAL`).
    pub(crate) fn record_copy(
        &self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        image: vk::Image,
        current_layout: vk::ImageLayout,
    ) -> Result<()> {
        let range = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };
        let to_transfer = vk::ImageMemoryBarrier::default()
            .old_layout(current_layout)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(range)
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ);
        unsafe {
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                std::slice::from_ref(&to_transfer),
            );
        }

        let region = vk::BufferImageCopy::default()
            .buffer_offset(0)
            .buffer_row_length(0)
            .buffer_image_height(0)
            .image_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
            .image_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
            .image_extent(vk::Extent3D {
                width: self.extent.width,
                height: self.extent.height,
                depth: 1,
            });
        unsafe {
            device.cmd_copy_image_to_buffer(
                cmd,
                image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                self.buffer,
                std::slice::from_ref(&region),
            );
        }

        let (dst_stage, dst_access) = if current_layout == vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
        {
            (
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            )
        } else {
            (
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::AccessFlags::empty(),
            )
        };

        let restore_layout = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .new_layout(current_layout)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(range)
            .src_access_mask(vk::AccessFlags::TRANSFER_READ)
            .dst_access_mask(dst_access);
        unsafe {
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                dst_stage,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                std::slice::from_ref(&restore_layout),
            );
        }
        Ok(())
    }

    /// After frame submit complete, read back and write PNG, return actual written path.
    pub(crate) fn finish(
        &self,
        context: &VulkanContext,
        swapchain_format: vk::Format,
    ) -> Result<PathBuf> {
        let width = self.extent.width as usize;
        let height = self.extent.height as usize;
        let size = width * height * 4;
        let ptr = unsafe {
            context
                .device
                .map_memory(self.memory, 0, size as u64, vk::MemoryMapFlags::empty())
        }
        .context("screenshot: map readback buffer")?;
        let mut pixels = vec![0u8; size];
        unsafe {
            std::ptr::copy_nonoverlapping(ptr as *const u8, pixels.as_mut_ptr(), size);
            context.device.unmap_memory(self.memory);
        }

        let bgra = matches!(
            swapchain_format,
            vk::Format::B8G8R8A8_SRGB | vk::Format::B8G8R8A8_UNORM
        );
        if bgra {
            for texel in pixels.as_chunks_mut::<4>().0 {
                texel.swap(0, 2);
            }
        }

        let mut image = image::RgbaImage::from_raw(width as u32, height as u32, pixels)
            .context("screenshot: build image buffer")?;
        if let Some([target_w, target_h]) = self.request.size {
            if target_w != width as u32 || target_h != height as u32 {
                image = image::imageops::resize(
                    &image,
                    target_w,
                    target_h,
                    image::imageops::FilterType::Nearest,
                );
            }
        }
        image
            .save(&self.request.path)
            .with_context(|| format!("screenshot: write {}", self.request.path.display()))?;
        log::info!(
            "screenshot: {}x{} -> {}",
            image.width(),
            image.height(),
            self.request.path.display()
        );
        Ok(self.request.path.clone())
    }

    pub(crate) unsafe fn destroy(&mut self, device: &ash::Device) {
        unsafe {
            device.destroy_buffer(self.buffer, None);
            device.free_memory(self.memory, None);
        }
    }
}
