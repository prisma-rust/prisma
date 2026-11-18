//! RenderRuntime — long-lived GPU session owner.
//!
//! Owns the Vulkan context, command pool, primary command buffers, frame
//! descriptor infrastructure, and per-frame UBOs. Survives swapchain
//! recreation and must outlive all render passes.

use std::sync::Arc;

use anyhow::Context as _;
use ash::vk;

use crate::context::VulkanContext;
use crate::descriptor::{DescriptorLayout, DescriptorPool, FrameUBO};

/// Long-lived GPU session (device, command pool, descriptors, UBOs).
///
/// Survives swapchain recreation and is the minimum set of fields that must
/// outlive all render passes.
pub struct RenderRuntime {
    pub command_pool: vk::CommandPool,
    pub command_buffers: Vec<vk::CommandBuffer>,
    #[allow(dead_code)]
    pub descriptor_layout: DescriptorLayout,
    #[allow(dead_code)]
    pub descriptor_pool: DescriptorPool,
    pub frame_ubos: Vec<FrameUBO>,
    /// **Must be the last field** — Rust drops struct fields in declaration
    /// order, so `context` (which owns the `ash::Device`) is destroyed *last*,
    /// after all child Vulkan objects (`descriptor_layout`, `descriptor_pool`,
    /// `frame_ubos`) have been cleaned up. Without this ordering the device is
    /// freed first and subsequent drops use a dangling handle → access violation.
    pub context: Arc<VulkanContext>,
}

impl RenderRuntime {
    /// Build the runtime from a pre-created Vulkan context.
    ///
    /// `cmd_buffer_count` determines how many command buffers to allocate
    /// (one per swapchain image).
    pub(crate) fn new(context: Arc<VulkanContext>, cmd_buffer_count: u32) -> anyhow::Result<Self> {
        let descriptor_layout =
            DescriptorLayout::new(&context.device).context("create descriptor layout")?;
        let frame_count = 2u32;
        let descriptor_pool =
            DescriptorPool::new(&context.device, frame_count).context("create descriptor pool")?;
        let descriptor_sets = descriptor_pool
            .allocate_sets(&context.device, &descriptor_layout, frame_count)
            .context("allocate descriptor sets")?;

        let frame_ubos = descriptor_sets
            .into_iter()
            .map(|set| FrameUBO::new(&context, set))
            .collect::<anyhow::Result<Vec<_>>>()
            .context("create frame UBOs")?;

        let pool_info = vk::CommandPoolCreateInfo::default()
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
            .queue_family_index(context.graphics_queue_family);
        let command_pool = unsafe { context.device.create_command_pool(&pool_info, None) }
            .context("create command pool")?;

        let alloc_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(cmd_buffer_count);
        let command_buffers = unsafe { context.device.allocate_command_buffers(&alloc_info) }
            .context("allocate command buffers")?;

        Ok(Self {
            command_pool,
            command_buffers,
            descriptor_layout,
            descriptor_pool,
            frame_ubos,
            context, // last — dropped last via declaration order
        })
    }
}
