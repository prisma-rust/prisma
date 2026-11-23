//! Minimal, self-contained Ash + `gpu-allocator` Vulkan renderer for `egui`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use ash::vk;
use gpu_allocator::vulkan::{
    Allocation, AllocationCreateDesc, AllocationScheme, Allocator, AllocatorCreateDesc,
};
use gpu_allocator::MemoryLocation;

const VERT_SPV: &[u8] = include_bytes!("../../shaders/egui.vert.spv");
const FRAG_SPV: &[u8] = include_bytes!("../../shaders/egui.frag.spv");
const MAX_FRAMES_IN_FLIGHT: usize = 2;
const MAX_DESCRIPTOR_SETS: u32 = 1024;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PushConstants {
    screen_size: [f32; 2],
}

#[derive(Clone, Copy)]
struct UserTextureEntry {
    descriptor_set: vk::DescriptorSet,
}

#[derive(Default)]
struct ImageRegistryInner {
    next_id: u64,
    textures: HashMap<u64, UserTextureEntry>,
    pending_register: Vec<(u64, vk::ImageView, vk::Sampler)>,
    pending_unregister: Vec<vk::DescriptorSet>,
}

/// Thread-safe registry mapping external Vulkan `(ImageView, Sampler)` pairs
/// to `egui::TextureId::User(id)` for zero-copy viewport sampling.
#[derive(Clone, Default)]
pub struct ImageRegistry {
    inner: Arc<Mutex<ImageRegistryInner>>,
}

impl ImageRegistry {
    pub fn register_user_texture(
        &self,
        view: vk::ImageView,
        sampler: vk::Sampler,
    ) -> egui::TextureId {
        let mut inner = self.inner.lock().expect("image registry poisoned");
        let id = inner.next_id;
        inner.next_id += 1;
        inner.pending_register.push((id, view, sampler));
        egui::TextureId::User(id)
    }

    pub fn unregister_user_texture(&self, texture_id: egui::TextureId) {
        let egui::TextureId::User(id) = texture_id else {
            return;
        };
        let mut inner = self.inner.lock().expect("image registry poisoned");
        inner.pending_register.retain(|(pid, _, _)| *pid != id);
        if let Some(entry) = inner.textures.remove(&id) {
            inner.pending_unregister.push(entry.descriptor_set);
        }
    }
}

struct GpuBuffer {
    buffer: vk::Buffer,
    allocation: Option<Allocation>,
    size: u64,
}

impl GpuBuffer {
    fn new(
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        size: u64,
        usage: vk::BufferUsageFlags,
        location: MemoryLocation,
        name: &str,
    ) -> anyhow::Result<Self> {
        let size = size.max(256);
        let info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = unsafe { device.create_buffer(&info, None) }.context("create egui buffer")?;
        let reqs = unsafe { device.get_buffer_memory_requirements(buffer) };
        let allocation = allocator
            .lock()
            .expect("allocator poisoned")
            .allocate(&AllocationCreateDesc {
                name,
                requirements: reqs,
                location,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .context("allocate egui buffer")?;
        unsafe { device.bind_buffer_memory(buffer, allocation.memory(), allocation.offset()) }
            .context("bind egui buffer memory")?;
        Ok(Self {
            buffer,
            allocation: Some(allocation),
            size,
        })
    }

    fn write_bytes(&mut self, data: &[u8]) -> anyhow::Result<()> {
        let alloc = self
            .allocation
            .as_mut()
            .context("buffer allocation missing")?;
        let slice = alloc
            .mapped_slice_mut()
            .context("buffer is not host-mapped")?;
        slice[..data.len()].copy_from_slice(data);
        Ok(())
    }

    fn destroy(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        if let Some(alloc) = self.allocation.take() {
            let _ = allocator.lock().expect("allocator poisoned").free(alloc);
        }
        if self.buffer != vk::Buffer::null() {
            unsafe { device.destroy_buffer(self.buffer, None) };
            self.buffer = vk::Buffer::null();
        }
    }
}

struct ManagedTexture {
    image: vk::Image,
    allocation: Option<Allocation>,
    view: vk::ImageView,
    sampler: vk::Sampler,
    descriptor_set: vk::DescriptorSet,
}

impl ManagedTexture {
    fn destroy(
        mut self,
        device: &ash::Device,
        allocator: &Arc<Mutex<Allocator>>,
        pool: vk::DescriptorPool,
    ) {
        unsafe {
            let _ = device.free_descriptor_sets(pool, &[self.descriptor_set]);
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
        }
        if let Some(alloc) = self.allocation.take() {
            let _ = allocator.lock().expect("allocator poisoned").free(alloc);
        }
    }
}

struct FrameSync {
    image_available: vk::Semaphore,
    in_flight: vk::Fence,
    cmd: vk::CommandBuffer,
    vertex_buf: Option<GpuBuffer>,
    index_buf: Option<GpuBuffer>,
    retired_textures: Vec<ManagedTexture>,
    retired_buffers: Vec<GpuBuffer>,
    retired_descriptor_sets: Vec<vk::DescriptorSet>,
}

struct SwapchainState {
    swapchain: vk::SwapchainKHR,
    views: Vec<vk::ImageView>,
    framebuffers: Vec<vk::Framebuffer>,
    /// Per-swapchain-image render_finished semaphores to avoid
    /// VUID-vkQueueSubmit-pSignalSemaphores-00067.
    render_finished: Vec<vk::Semaphore>,
    format: vk::SurfaceFormatKHR,
    extent: vk::Extent2D,
}

pub(super) struct EguiRenderer {
    device: ash::Device,
    physical_device: vk::PhysicalDevice,
    surface_loader: ash::khr::surface::Instance,
    swapchain_loader: ash::khr::swapchain::Device,
    surface: vk::SurfaceKHR,
    queue: vk::Queue,
    allocator: Arc<Mutex<Allocator>>,
    descriptor_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    pipeline_layout: vk::PipelineLayout,
    render_pass: vk::RenderPass,
    pipeline: vk::Pipeline,
    swapchain: SwapchainState,
    frames: Vec<FrameSync>,
    frame_index: usize,
    managed_textures: HashMap<egui::TextureId, ManagedTexture>,
    registry: ImageRegistry,
    clear_color: [f32; 4],
}

impl EguiRenderer {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        entry: &ash::Entry,
        instance: &ash::Instance,
        physical_device: vk::PhysicalDevice,
        device: ash::Device,
        surface_loader: ash::khr::surface::Instance,
        swapchain_loader: ash::khr::swapchain::Device,
        surface: vk::SurfaceKHR,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        allocator: Arc<Mutex<Allocator>>,
        registry: ImageRegistry,
        width: u32,
        height: u32,
        clear_color: [f32; 4],
    ) -> anyhow::Result<Self> {
        let _ = (
            entry,
            instance,
            AllocatorCreateDesc {
                instance: instance.clone(),
                device: device.clone(),
                physical_device,
                debug_settings: Default::default(),
                buffer_device_address: false,
                allocation_sizes: Default::default(),
            },
        );

        let binding = vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT);
        let descriptor_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default()
                    .bindings(std::slice::from_ref(&binding)),
                None,
            )
        }
        .context("create egui descriptor set layout")?;

        let pool_size = vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(MAX_DESCRIPTOR_SETS);
        let descriptor_pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .flags(vk::DescriptorPoolCreateFlags::FREE_DESCRIPTOR_SET)
                    .max_sets(MAX_DESCRIPTOR_SETS)
                    .pool_sizes(std::slice::from_ref(&pool_size)),
                None,
            )
        }
        .context("create egui descriptor pool")?;

        let push_range = vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(std::mem::size_of::<PushConstants>() as u32);
        let pipeline_layout = unsafe {
            device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(std::slice::from_ref(&descriptor_layout))
                    .push_constant_ranges(std::slice::from_ref(&push_range)),
                None,
            )
        }
        .context("create egui pipeline layout")?;

        let surface_format = choose_surface_format(&surface_loader, physical_device, surface)?;
        let render_pass = create_render_pass(&device, surface_format.format)?;
        let pipeline = create_pipeline(&device, pipeline_layout, render_pass)?;

        let swapchain = create_swapchain(
            &device,
            physical_device,
            &surface_loader,
            &swapchain_loader,
            surface,
            render_pass,
            surface_format,
            width,
            height,
            vk::SwapchainKHR::null(),
        )?;

        let cmd_buffers = unsafe {
            device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(MAX_FRAMES_IN_FLIGHT as u32),
            )
        }
        .context("allocate egui command buffers")?;

        let mut frames = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        for cmd in cmd_buffers {
            let image_available =
                unsafe { device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None) }
                    .context("create image_available semaphore")?;
            let in_flight = unsafe {
                device.create_fence(
                    &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                    None,
                )
            }
            .context("create in_flight fence")?;
            frames.push(FrameSync {
                image_available,
                in_flight,
                cmd,
                vertex_buf: None,
                index_buf: None,
                retired_textures: Vec::new(),
                retired_buffers: Vec::new(),
                retired_descriptor_sets: Vec::new(),
            });
        }

        Ok(Self {
            device,
            physical_device,
            surface_loader,
            swapchain_loader,
            surface,
            queue,
            allocator,
            descriptor_layout,
            descriptor_pool,
            pipeline_layout,
            render_pass,
            pipeline,
            swapchain,
            frames,
            frame_index: 0,
            managed_textures: HashMap::new(),
            registry,
            clear_color,
        })
    }

    pub(super) fn resize(&mut self, width: u32, height: u32) -> anyhow::Result<()> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        unsafe { self.device.device_wait_idle() }.context("wait idle before resize")?;
        let old_swapchain = self.swapchain.swapchain;
        let format = self.swapchain.format;
        let new_swapchain = create_swapchain(
            &self.device,
            self.physical_device,
            &self.surface_loader,
            &self.swapchain_loader,
            self.surface,
            self.render_pass,
            format,
            width,
            height,
            old_swapchain,
        )?;
        let old = std::mem::replace(&mut self.swapchain, new_swapchain);
        destroy_swapchain_resources(&self.device, &self.swapchain_loader, old);
        Ok(())
    }

    pub(super) fn paint(
        &mut self,
        egui_ctx: &egui::Context,
        textures_delta: egui::TexturesDelta,
        clipped_primitives: Vec<egui::ClippedPrimitive>,
        pixels_per_point: f32,
        width: u32,
        height: u32,
    ) -> anyhow::Result<()> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        let slot = self.frame_index % MAX_FRAMES_IN_FLIGHT;
        let fence = self.frames[slot].in_flight;
        unsafe { self.device.wait_for_fences(&[fence], true, u64::MAX) }
            .context("wait for egui frame fence")?;

        for tex in self.frames[slot].retired_textures.drain(..) {
            tex.destroy(&self.device, &self.allocator, self.descriptor_pool);
        }
        for mut buf in self.frames[slot].retired_buffers.drain(..) {
            buf.destroy(&self.device, &self.allocator);
        }
        let retired_sets: Vec<vk::DescriptorSet> = self.frames[slot]
            .retired_descriptor_sets
            .drain(..)
            .collect();
        if !retired_sets.is_empty() {
            unsafe {
                let _ = self
                    .device
                    .free_descriptor_sets(self.descriptor_pool, &retired_sets);
            }
        }

        let image_available = self.frames[slot].image_available;
        let acquire = unsafe {
            self.swapchain_loader.acquire_next_image(
                self.swapchain.swapchain,
                u64::MAX,
                image_available,
                vk::Fence::null(),
            )
        };
        let image_index = match acquire {
            Ok((idx, _suboptimal)) => idx,
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.resize(width, height)?;
                return Ok(());
            }
            Err(err) => return Err(err).context("acquire next swapchain image"),
        };

        unsafe { self.device.reset_fences(&[fence]) }.context("reset egui fence")?;

        self.sync_user_textures(slot)?;

        let cmd = self.frames[slot].cmd;
        unsafe {
            self.device
                .reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())
        }
        .context("reset egui command buffer")?;
        unsafe {
            self.device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
        }
        .context("begin egui command buffer")?;

        self.apply_texture_deltas(slot, cmd, &textures_delta)?;

        let mut vertices: Vec<egui::epaint::Vertex> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        for clipped in &clipped_primitives {
            if let egui::epaint::Primitive::Mesh(mesh) = &clipped.primitive {
                vertices.extend_from_slice(&mesh.vertices);
                indices.extend_from_slice(&mesh.indices);
            }
        }

        if !vertices.is_empty() && !indices.is_empty() {
            let v_bytes = bytemuck::cast_slice(&vertices);
            let i_bytes = bytemuck::cast_slice(&indices);
            ensure_host_buffer(
                &self.device,
                &self.allocator,
                &mut self.frames[slot].vertex_buf,
                v_bytes.len() as u64,
                vk::BufferUsageFlags::VERTEX_BUFFER,
                "egui_vb",
            )?;
            ensure_host_buffer(
                &self.device,
                &self.allocator,
                &mut self.frames[slot].index_buf,
                i_bytes.len() as u64,
                vk::BufferUsageFlags::INDEX_BUFFER,
                "egui_ib",
            )?;
            self.frames[slot]
                .vertex_buf
                .as_mut()
                .unwrap()
                .write_bytes(v_bytes)?;
            self.frames[slot]
                .index_buf
                .as_mut()
                .unwrap()
                .write_bytes(i_bytes)?;
        }

        let extent = self.swapchain.extent;
        let clear_value = vk::ClearValue {
            color: vk::ClearColorValue {
                float32: self.clear_color,
            },
        };
        let rp_begin = vk::RenderPassBeginInfo::default()
            .render_pass(self.render_pass)
            .framebuffer(self.swapchain.framebuffers[image_index as usize])
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent,
            })
            .clear_values(std::slice::from_ref(&clear_value));

        unsafe {
            self.device
                .cmd_begin_render_pass(cmd, &rp_begin, vk::SubpassContents::INLINE);
            self.device
                .cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);

            let viewport = vk::Viewport {
                x: 0.0,
                y: 0.0,
                width: extent.width as f32,
                height: extent.height as f32,
                min_depth: 0.0,
                max_depth: 1.0,
            };
            self.device
                .cmd_set_viewport(cmd, 0, std::slice::from_ref(&viewport));

            let pc = PushConstants {
                screen_size: [
                    extent.width as f32 / pixels_per_point,
                    extent.height as f32 / pixels_per_point,
                ],
            };
            self.device.cmd_push_constants(
                cmd,
                self.pipeline_layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                bytemuck::bytes_of(&pc),
            );

            if let (Some(vb), Some(ib)) = (
                self.frames[slot].vertex_buf.as_ref(),
                self.frames[slot].index_buf.as_ref(),
            ) {
                self.device
                    .cmd_bind_vertex_buffers(cmd, 0, &[vb.buffer], &[0]);
                self.device
                    .cmd_bind_index_buffer(cmd, ib.buffer, 0, vk::IndexType::UINT32);

                let mut v_offset: i32 = 0;
                let mut i_offset: u32 = 0;
                for clipped in &clipped_primitives {
                    let egui::epaint::Primitive::Mesh(mesh) = &clipped.primitive else {
                        continue;
                    };
                    if let Some(ds) = self.descriptor_for_texture(mesh.texture_id) {
                        let min_x = (clipped.clip_rect.min.x * pixels_per_point)
                            .round()
                            .clamp(0.0, extent.width as f32)
                            as i32;
                        let min_y = (clipped.clip_rect.min.y * pixels_per_point)
                            .round()
                            .clamp(0.0, extent.height as f32)
                            as i32;
                        let max_x = (clipped.clip_rect.max.x * pixels_per_point)
                            .round()
                            .clamp(0.0, extent.width as f32)
                            as i32;
                        let max_y = (clipped.clip_rect.max.y * pixels_per_point)
                            .round()
                            .clamp(0.0, extent.height as f32)
                            as i32;
                        if max_x > min_x && max_y > min_y {
                            let scissor = vk::Rect2D {
                                offset: vk::Offset2D { x: min_x, y: min_y },
                                extent: vk::Extent2D {
                                    width: (max_x - min_x) as u32,
                                    height: (max_y - min_y) as u32,
                                },
                            };
                            self.device
                                .cmd_set_scissor(cmd, 0, std::slice::from_ref(&scissor));
                            self.device.cmd_bind_descriptor_sets(
                                cmd,
                                vk::PipelineBindPoint::GRAPHICS,
                                self.pipeline_layout,
                                0,
                                &[ds],
                                &[],
                            );
                            self.device.cmd_draw_indexed(
                                cmd,
                                mesh.indices.len() as u32,
                                1,
                                i_offset,
                                v_offset,
                                0,
                            );
                        }
                    }
                    v_offset += mesh.vertices.len() as i32;
                    i_offset += mesh.indices.len() as u32;
                }
            }

            self.device.cmd_end_render_pass(cmd);
            self.device.end_command_buffer(cmd)?;
        }

        let render_finished = self.swapchain.render_finished[image_index as usize];
        let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let submit_info = vk::SubmitInfo::default()
            .wait_semaphores(std::slice::from_ref(&image_available))
            .wait_dst_stage_mask(&wait_stages)
            .command_buffers(std::slice::from_ref(&cmd))
            .signal_semaphores(std::slice::from_ref(&render_finished));
        unsafe { self.device.queue_submit(self.queue, &[submit_info], fence) }
            .context("submit egui command buffer")?;

        let present_info = vk::PresentInfoKHR::default()
            .wait_semaphores(std::slice::from_ref(&render_finished))
            .swapchains(std::slice::from_ref(&self.swapchain.swapchain))
            .image_indices(std::slice::from_ref(&image_index));
        let present = unsafe {
            self.swapchain_loader
                .queue_present(self.queue, &present_info)
        };
        match present {
            Ok(false) => {}
            Ok(true) | Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.resize(width, height)?;
            }
            Err(err) => return Err(err).context("present egui swapchain image"),
        }

        for id in &textures_delta.free {
            if let Some(tex) = self.managed_textures.remove(id) {
                self.frames[slot].retired_textures.push(tex);
            }
        }
        let _ = egui_ctx;
        self.frame_index = self.frame_index.wrapping_add(1);
        Ok(())
    }

    fn sync_user_textures(&mut self, slot: usize) -> anyhow::Result<()> {
        let mut inner = self.registry.inner.lock().expect("image registry poisoned");
        let unregister: Vec<vk::DescriptorSet> = inner.pending_unregister.drain(..).collect();
        if !unregister.is_empty() {
            self.frames[slot].retired_descriptor_sets.extend(unregister);
        }
        let register: Vec<(u64, vk::ImageView, vk::Sampler)> =
            inner.pending_register.drain(..).collect();
        drop(inner);

        if register.is_empty() {
            return Ok(());
        }
        let layouts = vec![self.descriptor_layout; register.len()];
        let sets = unsafe {
            self.device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(self.descriptor_pool)
                    .set_layouts(&layouts),
            )
        }
        .context("allocate user texture descriptor sets")?;

        let mut inner = self.registry.inner.lock().expect("image registry poisoned");
        for ((id, view, sampler), ds) in register.into_iter().zip(sets) {
            let img_info = vk::DescriptorImageInfo::default()
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .image_view(view)
                .sampler(sampler);
            let write = vk::WriteDescriptorSet::default()
                .dst_set(ds)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(std::slice::from_ref(&img_info));
            unsafe {
                self.device
                    .update_descriptor_sets(std::slice::from_ref(&write), &[])
            };
            inner
                .textures
                .insert(id, UserTextureEntry { descriptor_set: ds });
        }
        Ok(())
    }

    fn descriptor_for_texture(&self, id: egui::TextureId) -> Option<vk::DescriptorSet> {
        match id {
            egui::TextureId::Managed(_) => self.managed_textures.get(&id).map(|t| t.descriptor_set),
            egui::TextureId::User(uid) => self
                .registry
                .inner
                .lock()
                .expect("image registry poisoned")
                .textures
                .get(&uid)
                .map(|e| e.descriptor_set),
        }
    }

    fn apply_texture_deltas(
        &mut self,
        slot: usize,
        cmd: vk::CommandBuffer,
        delta: &egui::TexturesDelta,
    ) -> anyhow::Result<()> {
        for (id, image_delta) in &delta.set {
            let (width, height) = (
                image_delta.image.width() as u32,
                image_delta.image.height() as u32,
            );
            let rgba: Vec<u8> = match &image_delta.image {
                egui::ImageData::Color(c) => unpremultiply_srgba_pixels(&c.pixels),
            };

            let mut staging = GpuBuffer::new(
                &self.device,
                &self.allocator,
                rgba.len() as u64,
                vk::BufferUsageFlags::TRANSFER_SRC,
                MemoryLocation::CpuToGpu,
                "egui_tex_staging",
            )?;
            staging.write_bytes(&rgba)?;

            let (offset_x, offset_y, recreate) = match image_delta.pos {
                Some([x, y]) if self.managed_textures.contains_key(id) => {
                    (x as i32, y as i32, false)
                }
                _ => (0, 0, true),
            };

            if recreate {
                if let Some(old) = self.managed_textures.remove(id) {
                    self.frames[slot].retired_textures.push(old);
                }
                let tex = self.create_managed_texture(width, height, image_delta.options)?;
                self.managed_textures.insert(*id, tex);
            }

            let tex = self.managed_textures.get(id).expect("texture just created");
            let old_layout = if recreate {
                vk::ImageLayout::UNDEFINED
            } else {
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
            };

            transition_image(
                &self.device,
                cmd,
                tex.image,
                old_layout,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            );

            let region = vk::BufferImageCopy::default()
                .buffer_offset(0)
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .image_offset(vk::Offset3D {
                    x: offset_x,
                    y: offset_y,
                    z: 0,
                })
                .image_extent(vk::Extent3D {
                    width,
                    height,
                    depth: 1,
                });
            unsafe {
                self.device.cmd_copy_buffer_to_image(
                    cmd,
                    staging.buffer,
                    tex.image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    std::slice::from_ref(&region),
                );
            }
            transition_image(
                &self.device,
                cmd,
                tex.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            );
            self.frames[slot].retired_buffers.push(staging);
        }
        Ok(())
    }

    fn create_managed_texture(
        &self,
        width: u32,
        height: u32,
        options: egui::TextureOptions,
    ) -> anyhow::Result<ManagedTexture> {
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::R8G8B8A8_SRGB)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image =
            unsafe { self.device.create_image(&image_info, None) }.context("create egui image")?;
        let reqs = unsafe { self.device.get_image_memory_requirements(image) };
        let allocation = self
            .allocator
            .lock()
            .expect("allocator poisoned")
            .allocate(&AllocationCreateDesc {
                name: "egui_texture",
                requirements: reqs,
                location: MemoryLocation::GpuOnly,
                linear: false,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .context("allocate egui texture")?;
        unsafe {
            self.device
                .bind_image_memory(image, allocation.memory(), allocation.offset())
        }
        .context("bind egui texture memory")?;

        let view = unsafe {
            self.device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(vk::Format::R8G8B8A8_SRGB)
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    }),
                None,
            )
        }
        .context("create egui texture view")?;

        let filter = |f: egui::TextureFilter| match f {
            egui::TextureFilter::Nearest => vk::Filter::NEAREST,
            egui::TextureFilter::Linear => vk::Filter::LINEAR,
        };
        let address = match options.wrap_mode {
            egui::TextureWrapMode::ClampToEdge => vk::SamplerAddressMode::CLAMP_TO_EDGE,
            egui::TextureWrapMode::Repeat => vk::SamplerAddressMode::REPEAT,
            egui::TextureWrapMode::MirroredRepeat => vk::SamplerAddressMode::MIRRORED_REPEAT,
        };
        let sampler = unsafe {
            self.device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(filter(options.magnification))
                    .min_filter(filter(options.minification))
                    .address_mode_u(address)
                    .address_mode_v(address)
                    .address_mode_w(address),
                None,
            )
        }
        .context("create egui sampler")?;

        let descriptor_set = unsafe {
            self.device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(self.descriptor_pool)
                    .set_layouts(std::slice::from_ref(&self.descriptor_layout)),
            )
        }
        .context("allocate egui texture descriptor set")?[0];

        let img_info = vk::DescriptorImageInfo::default()
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image_view(view)
            .sampler(sampler);
        let write = vk::WriteDescriptorSet::default()
            .dst_set(descriptor_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(std::slice::from_ref(&img_info));
        unsafe {
            self.device
                .update_descriptor_sets(std::slice::from_ref(&write), &[])
        };

        Ok(ManagedTexture {
            image,
            allocation: Some(allocation),
            view,
            sampler,
            descriptor_set,
        })
    }

    pub(super) fn destroy(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
        }
        for (_, tex) in self.managed_textures.drain() {
            tex.destroy(&self.device, &self.allocator, self.descriptor_pool);
        }
        for frame in &mut self.frames {
            for tex in frame.retired_textures.drain(..) {
                tex.destroy(&self.device, &self.allocator, self.descriptor_pool);
            }
            for mut buf in frame.retired_buffers.drain(..) {
                buf.destroy(&self.device, &self.allocator);
            }
            let retired_sets: Vec<vk::DescriptorSet> =
                frame.retired_descriptor_sets.drain(..).collect();
            if !retired_sets.is_empty() {
                unsafe {
                    let _ = self
                        .device
                        .free_descriptor_sets(self.descriptor_pool, &retired_sets);
                }
            }
            if let Some(mut vb) = frame.vertex_buf.take() {
                vb.destroy(&self.device, &self.allocator);
            }
            if let Some(mut ib) = frame.index_buf.take() {
                ib.destroy(&self.device, &self.allocator);
            }
            unsafe {
                self.device.destroy_semaphore(frame.image_available, None);
                self.device.destroy_fence(frame.in_flight, None);
            }
        }
        let sw = std::mem::replace(
            &mut self.swapchain,
            SwapchainState {
                swapchain: vk::SwapchainKHR::null(),
                views: Vec::new(),
                framebuffers: Vec::new(),
                render_finished: Vec::new(),
                format: vk::SurfaceFormatKHR::default(),
                extent: vk::Extent2D::default(),
            },
        );
        destroy_swapchain_resources(&self.device, &self.swapchain_loader, sw);
        unsafe {
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_render_pass(self.render_pass, None);
            self.device
                .destroy_pipeline_layout(self.pipeline_layout, None);
            self.device
                .destroy_descriptor_pool(self.descriptor_pool, None);
            self.device
                .destroy_descriptor_set_layout(self.descriptor_layout, None);
            self.surface_loader.destroy_surface(self.surface, None);
        }
    }
}

fn ensure_host_buffer(
    device: &ash::Device,
    allocator: &Arc<Mutex<Allocator>>,
    slot: &mut Option<GpuBuffer>,
    needed: u64,
    usage: vk::BufferUsageFlags,
    name: &str,
) -> anyhow::Result<()> {
    let fits = slot.as_ref().is_some_and(|b| b.size >= needed);
    if !fits {
        if let Some(mut old) = slot.take() {
            old.destroy(device, allocator);
        }
        *slot = Some(GpuBuffer::new(
            device,
            allocator,
            needed.next_power_of_two(),
            usage,
            MemoryLocation::CpuToGpu,
            name,
        )?);
    }
    Ok(())
}

fn transition_image(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    image: vk::Image,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
) {
    let (src_access, src_stage) = match old_layout {
        vk::ImageLayout::TRANSFER_DST_OPTIMAL => (
            vk::AccessFlags::TRANSFER_WRITE,
            vk::PipelineStageFlags::TRANSFER,
        ),
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL => (
            vk::AccessFlags::SHADER_READ,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
        ),
        _ => (
            vk::AccessFlags::empty(),
            vk::PipelineStageFlags::TOP_OF_PIPE,
        ),
    };
    let (dst_access, dst_stage) = match new_layout {
        vk::ImageLayout::TRANSFER_DST_OPTIMAL => (
            vk::AccessFlags::TRANSFER_WRITE,
            vk::PipelineStageFlags::TRANSFER,
        ),
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL => (
            vk::AccessFlags::SHADER_READ,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
        ),
        _ => (
            vk::AccessFlags::empty(),
            vk::PipelineStageFlags::BOTTOM_OF_PIPE,
        ),
    };
    let barrier = vk::ImageMemoryBarrier::default()
        .old_layout(old_layout)
        .new_layout(new_layout)
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
        .src_access_mask(src_access)
        .dst_access_mask(dst_access);
    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            src_stage,
            dst_stage,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            std::slice::from_ref(&barrier),
        );
    }
}

fn choose_surface_format(
    surface_loader: &ash::khr::surface::Instance,
    physical_device: vk::PhysicalDevice,
    surface: vk::SurfaceKHR,
) -> anyhow::Result<vk::SurfaceFormatKHR> {
    let formats =
        unsafe { surface_loader.get_physical_device_surface_formats(physical_device, surface) }
            .context("query surface formats")?;
    Ok(formats
        .iter()
        .copied()
        .find(|f| {
            matches!(
                f.format,
                vk::Format::B8G8R8A8_SRGB | vk::Format::R8G8B8A8_SRGB
            ) && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
        })
        .unwrap_or(formats[0]))
}

fn create_render_pass(device: &ash::Device, format: vk::Format) -> anyhow::Result<vk::RenderPass> {
    let attachment = vk::AttachmentDescription::default()
        .format(format)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);
    let color_ref = vk::AttachmentReference::default()
        .attachment(0)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
    let subpass = vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(std::slice::from_ref(&color_ref));
    let dep = vk::SubpassDependency::default()
        .src_subpass(vk::SUBPASS_EXTERNAL)
        .dst_subpass(0)
        .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
        .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
        .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE);
    unsafe {
        device.create_render_pass(
            &vk::RenderPassCreateInfo::default()
                .attachments(std::slice::from_ref(&attachment))
                .subpasses(std::slice::from_ref(&subpass))
                .dependencies(std::slice::from_ref(&dep)),
            None,
        )
    }
    .context("create egui render pass")
}

fn create_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    render_pass: vk::RenderPass,
) -> anyhow::Result<vk::Pipeline> {
    let vert_words = bytes_to_words(VERT_SPV);
    let frag_words = bytes_to_words(FRAG_SPV);
    let vert_mod = unsafe {
        device.create_shader_module(
            &vk::ShaderModuleCreateInfo::default().code(&vert_words),
            None,
        )
    }
    .context("create egui vert shader")?;
    let frag_mod = unsafe {
        device.create_shader_module(
            &vk::ShaderModuleCreateInfo::default().code(&frag_words),
            None,
        )
    }
    .context("create egui frag shader")?;

    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vert_mod)
            .name(c"main"),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(frag_mod)
            .name(c"main"),
    ];
    let binding = vk::VertexInputBindingDescription::default()
        .binding(0)
        .stride(std::mem::size_of::<egui::epaint::Vertex>() as u32)
        .input_rate(vk::VertexInputRate::VERTEX);
    let attrs = [
        vk::VertexInputAttributeDescription::default()
            .location(0)
            .binding(0)
            .format(vk::Format::R32G32_SFLOAT)
            .offset(0),
        vk::VertexInputAttributeDescription::default()
            .location(1)
            .binding(0)
            .format(vk::Format::R32G32_SFLOAT)
            .offset(8),
        vk::VertexInputAttributeDescription::default()
            .location(2)
            .binding(0)
            .format(vk::Format::R8G8B8A8_UNORM)
            .offset(16),
    ];
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
        .vertex_binding_descriptions(std::slice::from_ref(&binding))
        .vertex_attribute_descriptions(&attrs);
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let raster = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::CLOCKWISE)
        .line_width(1.0);
    let multisample = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let blend_att = vk::PipelineColorBlendAttachmentState::default()
        .blend_enable(true)
        .src_color_blend_factor(vk::BlendFactor::ONE)
        .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
        .color_blend_op(vk::BlendOp::ADD)
        .src_alpha_blend_factor(vk::BlendFactor::ONE)
        .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
        .alpha_blend_op(vk::BlendOp::ADD)
        .color_write_mask(vk::ColorComponentFlags::RGBA);
    let color_blend = vk::PipelineColorBlendStateCreateInfo::default()
        .attachments(std::slice::from_ref(&blend_att));
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

    let info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&raster)
        .multisample_state(&multisample)
        .color_blend_state(&color_blend)
        .dynamic_state(&dynamic)
        .layout(layout)
        .render_pass(render_pass)
        .subpass(0);

    let result =
        unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None) };
    unsafe {
        device.destroy_shader_module(vert_mod, None);
        device.destroy_shader_module(frag_mod, None);
    }
    let pipelines = result.map_err(|(_, e)| e).context("create egui pipeline")?;
    Ok(pipelines[0])
}

#[allow(clippy::too_many_arguments)]
fn create_swapchain(
    device: &ash::Device,
    physical_device: vk::PhysicalDevice,
    surface_loader: &ash::khr::surface::Instance,
    swapchain_loader: &ash::khr::swapchain::Device,
    surface: vk::SurfaceKHR,
    render_pass: vk::RenderPass,
    format: vk::SurfaceFormatKHR,
    width: u32,
    height: u32,
    old_swapchain: vk::SwapchainKHR,
) -> anyhow::Result<SwapchainState> {
    let caps = unsafe {
        surface_loader.get_physical_device_surface_capabilities(physical_device, surface)
    }
    .context("query surface capabilities")?;
    let extent = if caps.current_extent.width != u32::MAX {
        caps.current_extent
    } else {
        vk::Extent2D {
            width: width.clamp(caps.min_image_extent.width, caps.max_image_extent.width),
            height: height.clamp(caps.min_image_extent.height, caps.max_image_extent.height),
        }
    };
    let mut image_count = caps.min_image_count + 1;
    if caps.max_image_count > 0 {
        image_count = image_count.min(caps.max_image_count);
    }
    let present_modes = unsafe {
        surface_loader.get_physical_device_surface_present_modes(physical_device, surface)
    }
    .unwrap_or_default();
    let present_mode = if present_modes.contains(&vk::PresentModeKHR::MAILBOX) {
        vk::PresentModeKHR::MAILBOX
    } else {
        vk::PresentModeKHR::FIFO
    };

    let swapchain = unsafe {
        swapchain_loader.create_swapchain(
            &vk::SwapchainCreateInfoKHR::default()
                .surface(surface)
                .min_image_count(image_count)
                .image_format(format.format)
                .image_color_space(format.color_space)
                .image_extent(extent)
                .image_array_layers(1)
                .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
                .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
                .pre_transform(caps.current_transform)
                .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
                .present_mode(present_mode)
                .clipped(true)
                .old_swapchain(old_swapchain),
            None,
        )
    }
    .context("create egui swapchain")?;

    let images = unsafe { swapchain_loader.get_swapchain_images(swapchain) }
        .context("get egui swapchain images")?;
    let mut views = Vec::with_capacity(images.len());
    let mut framebuffers = Vec::with_capacity(images.len());
    let mut render_finished = Vec::with_capacity(images.len());
    for &img in &images {
        let view = unsafe {
            device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(img)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(format.format)
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    }),
                None,
            )
        }
        .context("create egui swapchain image view")?;
        let fb = unsafe {
            device.create_framebuffer(
                &vk::FramebufferCreateInfo::default()
                    .render_pass(render_pass)
                    .attachments(std::slice::from_ref(&view))
                    .width(extent.width)
                    .height(extent.height)
                    .layers(1),
                None,
            )
        }
        .context("create egui framebuffer")?;
        let sem = unsafe { device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None) }
            .context("create per-image render_finished semaphore")?;
        views.push(view);
        framebuffers.push(fb);
        render_finished.push(sem);
    }

    Ok(SwapchainState {
        swapchain,
        views,
        framebuffers,
        render_finished,
        format,
        extent,
    })
}

fn destroy_swapchain_resources(
    device: &ash::Device,
    swapchain_loader: &ash::khr::swapchain::Device,
    sw: SwapchainState,
) {
    unsafe {
        for sem in sw.render_finished {
            device.destroy_semaphore(sem, None);
        }
        for fb in sw.framebuffers {
            device.destroy_framebuffer(fb, None);
        }
        for view in sw.views {
            device.destroy_image_view(view, None);
        }
        if sw.swapchain != vk::SwapchainKHR::null() {
            swapchain_loader.destroy_swapchain(sw.swapchain, None);
        }
    }
}

fn bytes_to_words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

pub(crate) fn unpremultiply_srgba_pixels(pixels: &[egui::Color32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len() * 4);
    for p in pixels {
        let [r, g, b, a] = p.to_array();
        if a == 0 {
            out.extend_from_slice(&[0, 0, 0, 0]);
        } else if a == 255 {
            out.extend_from_slice(&[r, g, b, 255]);
        } else {
            let alpha = a as u32;
            let half = alpha / 2;
            out.push(((r as u32 * 255 + half) / alpha).min(255) as u8);
            out.push(((g as u32 * 255 + half) / alpha).min(255) as u8);
            out.push(((b as u32 * 255 + half) / alpha).min(255) as u8);
            out.push(a);
        }
    }
    out
}

#[cfg(test)]
mod srgb_unpremultiply_tests {
    use super::unpremultiply_srgba_pixels;
    use egui::Color32;

    #[test]
    fn unpremultiply_recovers_straight_srgb_for_alpha_textures() {
        let pixels = [
            Color32::from_rgba_premultiplied(0, 0, 0, 0),
            Color32::from_rgba_premultiplied(128, 128, 128, 128),
            Color32::from_rgba_premultiplied(128, 64, 0, 128),
            Color32::from_rgba_premultiplied(200, 100, 50, 255),
        ];
        let out = unpremultiply_srgba_pixels(&pixels);
        assert_eq!(&out[0..4], &[0, 0, 0, 0]);
        assert_eq!(&out[4..8], &[255, 255, 255, 128]);
        assert_eq!(&out[8..12], &[255, 128, 0, 128]);
        assert_eq!(&out[12..16], &[200, 100, 50, 255]);
    }
}
