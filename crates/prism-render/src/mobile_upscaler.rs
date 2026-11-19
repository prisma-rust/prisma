//! Open-source mobile temporal upscalers: Qualcomm SGSR 2 and Arm ASR.
//!
//! Sits between scene rendering (`VoxelDdaPass` / `ForwardPass`) and `PostPass`
//! when `RenderSettings::mobile_upscaler` is `Sgsr2` or `ArmAsr`.
//! Consumes low-resolution HDR color, depth, and motion vectors at
//! `ctx.render_extent` and outputs full-resolution HDR color at `ctx.extent`
//! published as [`UPSCALED_COLOR_H`].

use std::ffi::CStr;

use anyhow::{Context as _, Result};
use ash::vk;

use crate::buffer::create_buffer;
use crate::compute::ComputePipeline;
use crate::context::VulkanContext;
use crate::render_graph::{
    GraphResources, MobileUpscaler, PassInfo, PassKind, RenderContext, RenderGraphBuilder,
    RenderMode, RenderPassNode, RenderSettings, ResourceType, ResourceUsage, FORWARD_COLOR_H,
    FORWARD_DEPTH_H, FORWARD_VELOCITY_H, UPSCALED_COLOR_H, VOXEL_COLOR_H, VOXEL_DEPTH_H,
};
use crate::render_pass::find_memory_type;
use crate::shader::load_shader_module;

const SGSR2_SPV: &[u8] = prism_render_shaders::SGSR2_UPSCALE_COMP_SPV;
const ARM_ASR_SPV: &[u8] = prism_render_shaders::ARM_ASR_UPSCALE_COMP_SPV;
const ENTRY_MAIN: &CStr = c"main";

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct UpscalerParamsUbo {
    inv_view_proj: [[f32; 4]; 4],
    prev_view_proj: [[f32; 4]; 4],
    input_size: [f32; 2],
    output_size: [f32; 2],
    inv_input_size: [f32; 2],
    inv_output_size: [f32; 2],
    jitter_uv: [f32; 2],
    sharpness: f32,
    reset: u32,
}

/// Compute pass implementing both SGSR 2 and Arm ASR temporal upscaling.
pub struct MobileUpscalerPass {
    output_extent: vk::Extent2D,
    images: [vk::Image; 2],
    memories: [vk::DeviceMemory; 2],
    views: [vk::ImageView; 2],
    history_initialized: [bool; 2],
    history_valid: [bool; 2],
    prev_view_proj: Option<[[f32; 4]; 4]>,
    prev_mode: MobileUpscaler,
    linear_sampler: vk::Sampler,
    nearest_sampler: vk::Sampler,
    ds_layout: vk::DescriptorSetLayout,
    ds_pool: vk::DescriptorPool,
    sets: [vk::DescriptorSet; 2],
    ubos: [vk::Buffer; 2],
    ubo_memories: [vk::DeviceMemory; 2],
    sgsr2_pipeline: ComputePipeline,
    arm_asr_pipeline: ComputePipeline,
    device: ash::Device,
}

impl MobileUpscalerPass {
    pub fn new(context: &VulkanContext, output_extent: vk::Extent2D) -> Result<Self> {
        let device = &context.device;
        let output_extent = vk::Extent2D {
            width: output_extent.width.max(1),
            height: output_extent.height.max(1),
        };

        let mut images = [vk::Image::null(); 2];
        let mut memories = [vk::DeviceMemory::null(); 2];
        let mut views = [vk::ImageView::null(); 2];
        for i in 0..2 {
            let (img, mem, view) = create_upscaler_image(context, output_extent)?;
            images[i] = img;
            memories[i] = mem;
            views[i] = view;
        }

        let linear_sampler = create_sampler(device, vk::Filter::LINEAR)?;
        let nearest_sampler = create_sampler(device, vk::Filter::NEAREST)?;

        let bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(2)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(3)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(4)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(5)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
        ];
        let ds_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )
        }
        .context("MobileUpscalerPass: create descriptor set layout")?;

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                descriptor_count: 8,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_IMAGE,
                descriptor_count: 2,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UNIFORM_BUFFER,
                descriptor_count: 2,
            },
        ];
        let ds_pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(2)
                    .pool_sizes(&pool_sizes),
                None,
            )
        }
        .context("MobileUpscalerPass: create descriptor pool")?;

        let layouts = [ds_layout, ds_layout];
        let allocated = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(ds_pool)
                    .set_layouts(&layouts),
            )
        }
        .context("MobileUpscalerPass: allocate descriptor sets")?;
        let sets = [allocated[0], allocated[1]];

        let mut ubos = [vk::Buffer::null(); 2];
        let mut ubo_memories = [vk::DeviceMemory::null(); 2];
        for i in 0..2 {
            let (buf, mem) = create_buffer(
                context,
                std::mem::size_of::<UpscalerParamsUbo>() as vk::DeviceSize,
                vk::BufferUsageFlags::UNIFORM_BUFFER,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            )
            .context("MobileUpscalerPass: create UBO")?;
            ubos[i] = buf;
            ubo_memories[i] = mem;
        }

        let sgsr2_mod = load_shader_module(device, SGSR2_SPV)
            .context("MobileUpscalerPass: load sgsr2_upscale.comp.spv")?;
        let sgsr2_pipeline = ComputePipeline::new(device, sgsr2_mod, ENTRY_MAIN, &[ds_layout], &[]);
        unsafe { device.destroy_shader_module(sgsr2_mod, None) };
        let sgsr2_pipeline = sgsr2_pipeline?;

        let arm_asr_mod = load_shader_module(device, ARM_ASR_SPV)
            .context("MobileUpscalerPass: load arm_asr_upscale.comp.spv")?;
        let arm_asr_pipeline =
            ComputePipeline::new(device, arm_asr_mod, ENTRY_MAIN, &[ds_layout], &[]);
        unsafe { device.destroy_shader_module(arm_asr_mod, None) };
        let arm_asr_pipeline = arm_asr_pipeline?;

        Ok(Self {
            output_extent,
            images,
            memories,
            views,
            history_initialized: [false; 2],
            history_valid: [false; 2],
            prev_view_proj: None,
            prev_mode: MobileUpscaler::Off,
            linear_sampler,
            nearest_sampler,
            ds_layout,
            ds_pool,
            sets,
            ubos,
            ubo_memories,
            sgsr2_pipeline,
            arm_asr_pipeline,
            device: device.clone(),
        })
    }

    pub fn recreate_target(
        &mut self,
        context: &VulkanContext,
        output_extent: vk::Extent2D,
    ) -> Result<()> {
        let output_extent = vk::Extent2D {
            width: output_extent.width.max(1),
            height: output_extent.height.max(1),
        };
        if self.output_extent == output_extent {
            return Ok(());
        }
        unsafe {
            let _ = self.device.device_wait_idle();
        }
        for i in 0..2 {
            unsafe {
                if self.views[i] != vk::ImageView::null() {
                    self.device.destroy_image_view(self.views[i], None);
                }
                if self.images[i] != vk::Image::null() {
                    self.device.destroy_image(self.images[i], None);
                }
                if self.memories[i] != vk::DeviceMemory::null() {
                    self.device.free_memory(self.memories[i], None);
                }
            }
            let (img, mem, view) = create_upscaler_image(context, output_extent)?;
            self.images[i] = img;
            self.memories[i] = mem;
            self.views[i] = view;
        }
        self.output_extent = output_extent;
        self.history_initialized = [false; 2];
        self.history_valid = [false; 2];
        self.prev_view_proj = None;
        Ok(())
    }

    fn reset_history(&mut self) {
        self.history_valid = [false; 2];
        self.prev_view_proj = None;
    }
}

impl Drop for MobileUpscalerPass {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            for i in 0..2 {
                if self.views[i] != vk::ImageView::null() {
                    self.device.destroy_image_view(self.views[i], None);
                }
                if self.images[i] != vk::Image::null() {
                    self.device.destroy_image(self.images[i], None);
                }
                if self.memories[i] != vk::DeviceMemory::null() {
                    self.device.free_memory(self.memories[i], None);
                }
                if self.ubos[i] != vk::Buffer::null() {
                    self.device.destroy_buffer(self.ubos[i], None);
                }
                if self.ubo_memories[i] != vk::DeviceMemory::null() {
                    self.device.free_memory(self.ubo_memories[i], None);
                }
            }
            self.device.destroy_descriptor_pool(self.ds_pool, None);
            self.device
                .destroy_descriptor_set_layout(self.ds_layout, None);
            self.device.destroy_sampler(self.linear_sampler, None);
            self.device.destroy_sampler(self.nearest_sampler, None);
        }
    }
}

impl RenderPassNode for MobileUpscalerPass {
    fn name(&self) -> &str {
        "MobileUpscalerPass"
    }

    fn graph_info(&self) -> PassInfo {
        PassInfo {
            index: usize::MAX,
            name: "MobileUpscalerPass".into(),
            kind: PassKind::MobileUpscaler,
            inputs: vec![FORWARD_COLOR_H, FORWARD_DEPTH_H, FORWARD_VELOCITY_H],
            outputs: vec![UPSCALED_COLOR_H],
        }
    }

    fn setup(&mut self, graph: &mut RenderGraphBuilder, _settings: &RenderSettings) {
        graph.create_resource_at(
            UPSCALED_COLOR_H,
            ResourceType::ColorAttachment {
                format: vk::Format::R16G16B16A16_SFLOAT,
                extent: self.output_extent,
                sample_count: vk::SampleCountFlags::TYPE_1,
            },
        );
        for handle in [
            FORWARD_COLOR_H,
            FORWARD_VELOCITY_H,
            VOXEL_COLOR_H,
            VOXEL_DEPTH_H,
        ] {
            graph.read_usage(ResourceUsage {
                handle,
                access: vk::AccessFlags::SHADER_READ,
                stage: vk::PipelineStageFlags::COMPUTE_SHADER,
                layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            });
        }
        graph.read_usage(ResourceUsage {
            handle: FORWARD_DEPTH_H,
            access: vk::AccessFlags::SHADER_READ,
            stage: vk::PipelineStageFlags::COMPUTE_SHADER,
            layout: vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL,
        });
        graph.write_usage(ResourceUsage {
            handle: UPSCALED_COLOR_H,
            access: vk::AccessFlags::SHADER_WRITE,
            stage: vk::PipelineStageFlags::COMPUTE_SHADER,
            layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        });
    }

    fn execute(&mut self, ctx: &RenderContext, resources: &mut GraphResources) -> Result<()> {
        let mode = ctx.settings.mobile_upscaler;
        if !mode.is_temporal_compute()
            || ctx.settings.anf_sr
            || ctx.frame.render_mode == RenderMode::PathTrace
            || ctx.frame.debug_rt != 0
        {
            self.reset_history();
            self.prev_mode = MobileUpscaler::Off;
            return Ok(());
        }

        if self.prev_mode != mode {
            self.reset_history();
            self.prev_mode = mode;
        }

        self.recreate_target(ctx.context, ctx.extent)?;

        let has_voxel = resources.published_view(VOXEL_COLOR_H).is_some()
            && resources.published_view(VOXEL_DEPTH_H).is_some();
        let (color_handle, depth_handle) = if has_voxel {
            (VOXEL_COLOR_H, VOXEL_DEPTH_H)
        } else {
            (FORWARD_COLOR_H, FORWARD_DEPTH_H)
        };
        let Some(color_view) = resources.published_view(color_handle) else {
            return Ok(());
        };
        let Some(depth_view) = resources.published_view(depth_handle) else {
            return Ok(());
        };
        let Some(velocity_view) = resources.published_view(FORWARD_VELOCITY_H) else {
            return Ok(());
        };

        let frame = (ctx.frame_index as usize) % 2;
        let prev = (frame + 1) % 2;

        // Recover unjittered camera view-projection for accurate reprojection.
        let mut view_proj = ctx.frame.view_proj;
        view_proj[3][0] -= ctx.frame.view_proj_jitter[0];
        view_proj[3][1] -= ctx.frame.view_proj_jitter[1];
        let inv_view_proj = crate::math::mat_inverse(&view_proj);
        let prev_view_proj = self.prev_view_proj.unwrap_or(view_proj);
        let reset = if self.history_valid[prev] && !ctx.frame.freeze_temporal {
            0
        } else {
            1
        };

        let in_w = ctx.render_extent.width.max(1) as f32;
        let in_h = ctx.render_extent.height.max(1) as f32;
        let out_w = self.output_extent.width.max(1) as f32;
        let out_h = self.output_extent.height.max(1) as f32;

        let params = UpscalerParamsUbo {
            inv_view_proj,
            prev_view_proj,
            input_size: [in_w, in_h],
            output_size: [out_w, out_h],
            inv_input_size: [1.0 / in_w, 1.0 / in_h],
            inv_output_size: [1.0 / out_w, 1.0 / out_h],
            jitter_uv: [
                ctx.frame.view_proj_jitter[0] * 0.5,
                ctx.frame.view_proj_jitter[1] * 0.5,
            ],
            sharpness: ctx.settings.upscaler_sharpness.clamp(0.0, 1.0),
            reset,
        };

        unsafe {
            let ptr = self
                .device
                .map_memory(
                    self.ubo_memories[frame],
                    0,
                    std::mem::size_of::<UpscalerParamsUbo>() as vk::DeviceSize,
                    vk::MemoryMapFlags::empty(),
                )
                .context("MobileUpscalerPass: map UBO")?;
            std::ptr::copy_nonoverlapping(
                &params as *const UpscalerParamsUbo as *const u8,
                ptr as *mut u8,
                std::mem::size_of::<UpscalerParamsUbo>(),
            );
            self.device.unmap_memory(self.ubo_memories[frame]);
        }

        let depth_layout = if has_voxel {
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
        } else {
            vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL
        };

        let color_info = [vk::DescriptorImageInfo::default()
            .sampler(self.linear_sampler)
            .image_view(color_view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let depth_info = [vk::DescriptorImageInfo::default()
            .sampler(self.nearest_sampler)
            .image_view(depth_view)
            .image_layout(depth_layout)];
        let velocity_info = [vk::DescriptorImageInfo::default()
            .sampler(self.nearest_sampler)
            .image_view(velocity_view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let history_info = [vk::DescriptorImageInfo::default()
            .sampler(self.linear_sampler)
            .image_view(self.views[prev])
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let output_info = [vk::DescriptorImageInfo::default()
            .image_view(self.views[frame])
            .image_layout(vk::ImageLayout::GENERAL)];
        let ubo_info = [vk::DescriptorBufferInfo::default()
            .buffer(self.ubos[frame])
            .offset(0)
            .range(std::mem::size_of::<UpscalerParamsUbo>() as vk::DeviceSize)];

        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(self.sets[frame])
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&color_info),
            vk::WriteDescriptorSet::default()
                .dst_set(self.sets[frame])
                .dst_binding(1)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&depth_info),
            vk::WriteDescriptorSet::default()
                .dst_set(self.sets[frame])
                .dst_binding(2)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&velocity_info),
            vk::WriteDescriptorSet::default()
                .dst_set(self.sets[frame])
                .dst_binding(3)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(&history_info),
            vk::WriteDescriptorSet::default()
                .dst_set(self.sets[frame])
                .dst_binding(4)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .image_info(&output_info),
            vk::WriteDescriptorSet::default()
                .dst_set(self.sets[frame])
                .dst_binding(5)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .buffer_info(&ubo_info),
        ];
        unsafe {
            self.device.update_descriptor_sets(&writes, &[]);
        }

        // Ensure prev history image is in SHADER_READ_ONLY_OPTIMAL if never initialized,
        // and transition current output image to GENERAL for compute write.
        let mut pre_barriers = Vec::with_capacity(2);
        if !self.history_initialized[prev] {
            pre_barriers.push(image_barrier(
                self.images[prev],
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::AccessFlags::empty(),
                vk::AccessFlags::SHADER_READ,
            ));
            self.history_initialized[prev] = true;
        }
        let curr_old_layout = if self.history_initialized[frame] {
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
        } else {
            vk::ImageLayout::UNDEFINED
        };
        pre_barriers.push(image_barrier(
            self.images[frame],
            curr_old_layout,
            vk::ImageLayout::GENERAL,
            vk::AccessFlags::empty(),
            vk::AccessFlags::SHADER_WRITE,
        ));

        let pipeline = match mode {
            MobileUpscaler::Sgsr2 => &self.sgsr2_pipeline,
            MobileUpscaler::ArmAsr => &self.arm_asr_pipeline,
            _ => unreachable!(),
        };

        unsafe {
            ctx.device.cmd_pipeline_barrier(
                ctx.cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &pre_barriers,
            );
            ctx.device.cmd_bind_pipeline(
                ctx.cmd,
                vk::PipelineBindPoint::COMPUTE,
                pipeline.pipeline,
            );
            ctx.device.cmd_bind_descriptor_sets(
                ctx.cmd,
                vk::PipelineBindPoint::COMPUTE,
                pipeline.layout,
                0,
                &[self.sets[frame]],
                &[],
            );
            ctx.device.cmd_dispatch(
                ctx.cmd,
                self.output_extent.width.div_ceil(8),
                self.output_extent.height.div_ceil(8),
                1,
            );

            let post_barrier = [image_barrier(
                self.images[frame],
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::AccessFlags::SHADER_WRITE,
                vk::AccessFlags::SHADER_READ,
            )];
            ctx.device.cmd_pipeline_barrier(
                ctx.cmd,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::FRAGMENT_SHADER | vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &post_barrier,
            );
        }

        self.history_initialized[frame] = true;
        self.history_valid[frame] = true;
        self.prev_view_proj = Some(view_proj);

        resources.set_image_view(UPSCALED_COLOR_H, self.views[frame]);
        resources.set_image(UPSCALED_COLOR_H, self.images[frame]);

        Ok(())
    }
}

fn image_barrier(
    image: vk::Image,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
    src_access: vk::AccessFlags,
    dst_access: vk::AccessFlags,
) -> vk::ImageMemoryBarrier<'static> {
    vk::ImageMemoryBarrier::default()
        .old_layout(old_layout)
        .new_layout(new_layout)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .src_access_mask(src_access)
        .dst_access_mask(dst_access)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        })
}

fn create_sampler(device: &ash::Device, filter: vk::Filter) -> Result<vk::Sampler> {
    unsafe {
        device.create_sampler(
            &vk::SamplerCreateInfo::default()
                .mag_filter(filter)
                .min_filter(filter)
                .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .min_lod(0.0)
                .max_lod(vk::LOD_CLAMP_NONE),
            None,
        )
    }
    .context("MobileUpscalerPass: create sampler")
}

fn create_upscaler_image(
    context: &VulkanContext,
    extent: vk::Extent2D,
) -> Result<(vk::Image, vk::DeviceMemory, vk::ImageView)> {
    let device = &context.device;
    let format = vk::Format::R16G16B16A16_SFLOAT;
    let image = unsafe {
        device.create_image(
            &vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(format)
                .extent(vk::Extent3D {
                    width: extent.width,
                    height: extent.height,
                    depth: 1,
                })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(
                    vk::ImageUsageFlags::SAMPLED
                        | vk::ImageUsageFlags::STORAGE
                        | vk::ImageUsageFlags::TRANSFER_DST,
                )
                .sharing_mode(vk::SharingMode::EXCLUSIVE)
                .initial_layout(vk::ImageLayout::UNDEFINED),
            None,
        )
    }
    .context("MobileUpscalerPass: create image")?;

    let req = unsafe { device.get_image_memory_requirements(image) };
    let mem_props = unsafe {
        context
            .instance
            .get_physical_device_memory_properties(context.physical_device)
    };
    let mem_type = find_memory_type(
        &mem_props,
        req.memory_type_bits,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
    )
    .context("MobileUpscalerPass: no suitable device memory type")?;
    let memory = unsafe {
        device.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(req.size)
                .memory_type_index(mem_type),
            None,
        )
    }
    .context("MobileUpscalerPass: allocate image memory")?;
    unsafe {
        device
            .bind_image_memory(image, memory, 0)
            .context("MobileUpscalerPass: bind image memory")?;
    }

    let view = unsafe {
        device.create_image_view(
            &vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format)
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
    .context("MobileUpscalerPass: create image view")?;

    Ok((image, memory, view))
}
