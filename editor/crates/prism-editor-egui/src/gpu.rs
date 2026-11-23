//! Vulkan device shared with egui-ash.
//!
//! egui-ash requires host to create instance / device and return [`AshRenderState`];
//! This gives us engine-style model: **editor and engine share same `VkDevice` /
//! queue**, so Scene/Game viewport only needs to register engine offscreen image `VkImageView` as
//! egui texture (`ImageRegistry::register_user_texture`), no GPU<->CPU copy.
//!
//! Device created by [`VulkanContext::from_host`], so features `prism-render` needs
//! (bindless / timeline semaphore / synchronization2 / device address) consistent with window mode.

use std::sync::{Arc, Mutex};

use crate::egui_host::{raw_window_handle, AshRenderState, CreationContext};
use anyhow::Context as _;
use ash::vk;
use gpu_allocator::vulkan::{Allocator, AllocatorCreateDesc};
use prism_render::context::{
    create_host_instance, pick_host_physical_device, pick_present_queue_family,
};
use prism_render::VulkanContext;
use raw_window_handle::{HasDisplayHandle as _, HasWindowHandle as _};

/// Vulkan handles needed by egui-ash + engine-shared context.
pub struct SharedVulkan {
    pub entry: ash::Entry,
    pub instance: ash::Instance,
    pub physical_device: vk::PhysicalDevice,
    pub queue_family: u32,
    pub context: Arc<VulkanContext>,
    pub allocator: Arc<Mutex<Allocator>>,
    /// egui-ash per-frame command buffer allocated from here.
    pub command_pool: vk::CommandPool,
    pub surface_loader: ash::khr::surface::Instance,
    pub swapchain_loader: ash::khr::swapchain::Device,
    /// Only for naming/destroy timing; instance lifecycle decided by process (not context).
    #[allow(dead_code)]
    debug_messenger: Option<vk::DebugUtilsMessengerEXT>,
}

impl SharedVulkan {
    /// Create instance / physical device / device per egui-ash requirement, wrap into
    /// `prism-render` context.
    pub fn create(cc: &CreationContext<'_>) -> anyhow::Result<Self> {
        let entry = unsafe { ash::Entry::load() }.context("failed to load Vulkan loader")?;

        let instance_extensions: Vec<&str> = cc
            .required_instance_extensions
            .iter()
            .filter_map(|name| name.to_str().ok())
            .collect();
        let (instance, debug_messenger) = create_host_instance(&entry, &instance_extensions)?;

        let surface_loader = ash::khr::surface::Instance::new(&entry, &instance);

        let window = cc.main_window;
        let window_handle = window
            .window_handle()
            .context("main window has no raw window handle")?;
        let display_handle = window
            .display_handle()
            .context("main window has no raw display handle")?;
        let surface = unsafe {
            ash_window::create_surface(
                &entry,
                &instance,
                display_handle.as_raw(),
                window_handle.as_raw(),
                None,
            )
        }
        .context("create temporary Vulkan surface")?;

        let physical_device = match pick_host_physical_device(&instance, &surface_loader, surface) {
            Ok(device) => device,
            Err(error) => {
                unsafe { surface_loader.destroy_surface(surface, None) };
                return Err(error);
            }
        };
        let queue_family =
            pick_present_queue_family(&instance, physical_device, &surface_loader, surface)
                .context("no queue family supports both graphics and presentation")?;

        let device_extensions: Vec<&str> = cc
            .required_device_extensions
            .iter()
            .filter_map(|name| name.to_str().ok())
            .collect();

        let context = Arc::new(VulkanContext::from_host(
            entry.clone(),
            instance.clone(),
            physical_device,
            queue_family,
            &device_extensions,
        )?);

        unsafe { surface_loader.destroy_surface(surface, None) };

        let allocator = Allocator::new(&AllocatorCreateDesc {
            instance: instance.clone(),
            device: context.device.clone(),
            physical_device,
            debug_settings: Default::default(),
            buffer_device_address: context.rt_caps.buffer_device_address,
            allocation_sizes: Default::default(),
        })
        .context("create gpu-allocator")?;

        let command_pool = unsafe {
            context.device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
                    .queue_family_index(queue_family),
                None,
            )
        }
        .context("create egui command pool")?;

        let swapchain_loader = ash::khr::swapchain::Device::new(&instance, &context.device);

        Ok(Self {
            entry,
            instance,
            physical_device,
            queue_family,
            context,
            allocator: Arc::new(Mutex::new(allocator)),
            command_pool,
            surface_loader,
            swapchain_loader,
            debug_messenger,
        })
    }

    /// Render state handed to egui-ash. Handles are `Copy` Vulkan objects, egui-ash
    /// only uses, does not destroy.
    pub fn render_state(&self) -> AshRenderState<Arc<Mutex<Allocator>>> {
        AshRenderState {
            entry: self.entry.clone(),
            instance: self.instance.clone(),
            physical_device: self.physical_device,
            device: self.context.device.clone(),
            surface_loader: self.surface_loader.clone(),
            swapchain_loader: self.swapchain_loader.clone(),
            queue: self.context.graphics_queue,
            queue_family_index: self.queue_family,
            command_pool: self.command_pool,
            allocator: self.allocator.clone(),
        }
    }

    /// Destroy debug messenger (device / instance itself left to process exit recycle: egui-ash's
    /// integration holds their copies, destroying first causes dangling handles).
    ///
    /// Current editor exits directly at process exit, so no call site; kept for future
    /// "close project but keep editor running" path.
    #[allow(dead_code)]
    pub fn shutdown(&mut self) {
        let Some(messenger) = self.debug_messenger.take() else {
            return;
        };
        let ext = ash::ext::debug_utils::Instance::new(&self.entry, &self.instance);
        unsafe { ext.destroy_debug_utils_messenger(messenger, None) };
    }
}
