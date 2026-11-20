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

const SGSR2_CONVERT_SPV: &[u8] = prism_render_shaders::SGSR2_CONVERT_COMP_SPV;
const SGSR2_ACTIVATE_SPV: &[u8] = prism_render_shaders::SGSR2_ACTIVATE_COMP_SPV;
const SGSR2_SPV: &[u8] = prism_render_shaders::SGSR2_UPSCALE_COMP_SPV;
const ARM_ASR_SPV: &[u8] = prism_render_shaders::ARM_ASR_UPSCALE_COMP_SPV;
const ENTRY_MAIN: &CStr = c"main";

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct UpscalerParamsUbo {
    inv_view_proj: [[f32; 4]; 4],
    prev_view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    input_size: [f32; 2],
    output_size: [f32; 2],
    inv_input_size: [f32; 2],
    inv_output_size: [f32; 2],
    jitter_uv: [f32; 2],
    sharpness: f32,
    reset: u32,
}

/// Compute pass implementing 3-pass SGSR 2 and Arm ASR temporal upscaling.
pub struct MobileUpscalerPass {
    output_extent: vk::Extent2D,
    input_extent: vk::Extent2D,
    images: [vk::Image; 2],
    memories: [vk::DeviceMemory; 2],
    views: [vk::ImageView; 2],
    motion_depth_clip_img: vk::Image,
    motion_depth_clip_mem: vk::DeviceMemory,
    motion_depth_clip_view: vk::ImageView,
    ycocg_color_img: vk::Image,
    ycocg_color_mem: vk::DeviceMemory,
    ycocg_color_view: vk::ImageView,
    motion_depth_alpha_img: vk::Image,
    motion_depth_alpha_mem: vk::DeviceMemory,
    motion_depth_alpha_view: vk::ImageView,
    luma_history_imgs: [vk::Image; 2],
    luma_history_mems: [vk::DeviceMemory; 2],
    luma_history_views: [vk::ImageView; 2],
    luma_history_initialized: [bool; 2],
    sgsr2_intermediates_initialized: bool,
    history_initialized: [bool; 2],
    history_valid: [bool; 2],
    prev_view_proj: Option<[[f32; 4]; 4]>,
    prev_mode: MobileUpscaler,
    linear_sampler: vk::Sampler,
    nearest_sampler: vk::Sampler,
    ds_layout: vk::DescriptorSetLayout,
    convert_ds_layout: vk::DescriptorSetLayout,
    ds_pool: vk::DescriptorPool,
    sets: [vk::DescriptorSet; 2],
    convert_sets: [vk::DescriptorSet; 2],
    activate_sets: [vk::DescriptorSet; 2],
    ubos: [vk::Buffer; 2],
    ubo_memories: [vk::DeviceMemory; 2],
    sgsr2_convert_pipeline: ComputePipeline,
    sgsr2_activate_pipeline: ComputePipeline,
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

        let input_extent = output_extent;
        let (motion_depth_clip_img, motion_depth_clip_mem, motion_depth_clip_view) =
            create_upscaler_image(context, input_extent)?;
        let (ycocg_color_img, ycocg_color_mem, ycocg_color_view) =
            create_upscaler_image(context, input_extent)?;
        let (motion_depth_alpha_img, motion_depth_alpha_mem, motion_depth_alpha_view) =
            create_upscaler_image(context, input_extent)?;
        let mut luma_history_imgs = [vk::Image::null(); 2];
        let mut luma_history_mems = [vk::DeviceMemory::null(); 2];
        let mut luma_history_views = [vk::ImageView::null(); 2];
        for i in 0..2 {
            let (img, mem, view) = create_upscaler_image(context, input_extent)?;
            luma_history_imgs[i] = img;
            luma_history_mems[i] = mem;
            luma_history_views[i] = view;
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

        let convert_bindings = [
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
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
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
        let convert_ds_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&convert_bindings),
                None,
            )
        }
        .context("MobileUpscalerPass: create convert descriptor set layout")?;

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                descriptor_count: 20,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_IMAGE,
                descriptor_count: 10,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UNIFORM_BUFFER,
                descriptor_count: 6,
            },
        ];
        let ds_pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(6)
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

        let convert_layouts = [convert_ds_layout, convert_ds_layout];
        let convert_allocated = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(ds_pool)
                    .set_layouts(&convert_layouts),
            )
        }
        .context("MobileUpscalerPass: allocate convert descriptor sets")?;
        let convert_sets = [convert_allocated[0], convert_allocated[1]];

        let activate_allocated = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(ds_pool)
                    .set_layouts(&convert_layouts),
            )
        }
        .context("MobileUpscalerPass: allocate activate descriptor sets")?;
        let activate_sets = [activate_allocated[0], activate_allocated[1]];

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

        let sgsr2_convert_mod = load_shader_module(device, SGSR2_CONVERT_SPV)
            .context("MobileUpscalerPass: load sgsr2_convert.comp.spv")?;
        let sgsr2_convert_pipeline = ComputePipeline::new(
            device,
            sgsr2_convert_mod,
            ENTRY_MAIN,
            &[convert_ds_layout],
            &[],
        );
        unsafe { device.destroy_shader_module(sgsr2_convert_mod, None) };
        let sgsr2_convert_pipeline = sgsr2_convert_pipeline?;

        let sgsr2_activate_mod = load_shader_module(device, SGSR2_ACTIVATE_SPV)
            .context("MobileUpscalerPass: load sgsr2_activate.comp.spv")?;
        let sgsr2_activate_pipeline = ComputePipeline::new(
            device,
            sgsr2_activate_mod,
            ENTRY_MAIN,
            &[convert_ds_layout],
            &[],
        );
        unsafe { device.destroy_shader_module(sgsr2_activate_mod, None) };
        let sgsr2_activate_pipeline = sgsr2_activate_pipeline?;

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
            input_extent,
            images,
            memories,
            views,
            motion_depth_clip_img,
            motion_depth_clip_mem,
            motion_depth_clip_view,
            ycocg_color_img,
            ycocg_color_mem,
            ycocg_color_view,
            motion_depth_alpha_img,
            motion_depth_alpha_mem,
            motion_depth_alpha_view,
            luma_history_imgs,
            luma_history_mems,
            luma_history_views,
            luma_history_initialized: [false; 2],
            sgsr2_intermediates_initialized: false,
            history_initialized: [false; 2],
            history_valid: [false; 2],
            prev_view_proj: None,
            prev_mode: MobileUpscaler::Off,
            linear_sampler,
            nearest_sampler,
            ds_layout,
            convert_ds_layout,
            ds_pool,
            sets,
            convert_sets,
            activate_sets,
            ubos,
            ubo_memories,
            sgsr2_convert_pipeline,
            sgsr2_activate_pipeline,
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

    fn recreate_input_targets(
        &mut self,
        context: &VulkanContext,
        input_extent: vk::Extent2D,
    ) -> Result<()> {
        let input_extent = vk::Extent2D {
            width: input_extent.width.max(1),
            height: input_extent.height.max(1),
        };
        if self.input_extent == input_extent {
            return Ok(());
        }
        unsafe {
            let _ = self.device.device_wait_idle();
            for (view, img, mem) in [
                (
                    self.motion_depth_clip_view,
                    self.motion_depth_clip_img,
                    self.motion_depth_clip_mem,
                ),
                (
                    self.ycocg_color_view,
                    self.ycocg_color_img,
                    self.ycocg_color_mem,
                ),
                (
                    self.motion_depth_alpha_view,
                    self.motion_depth_alpha_img,
                    self.motion_depth_alpha_mem,
                ),
                (
                    self.luma_history_views[0],
                    self.luma_history_imgs[0],
                    self.luma_history_mems[0],
                ),
                (
                    self.luma_history_views[1],
                    self.luma_history_imgs[1],
                    self.luma_history_mems[1],
                ),
            ] {
                if view != vk::ImageView::null() {
                    self.device.destroy_image_view(view, None);
                }
                if img != vk::Image::null() {
                    self.device.destroy_image(img, None);
                }
                if mem != vk::DeviceMemory::null() {
                    self.device.free_memory(mem, None);
                }
            }
        }
        let (mdc_img, mdc_mem, mdc_view) = create_upscaler_image(context, input_extent)?;
        let (ycc_img, ycc_mem, ycc_view) = create_upscaler_image(context, input_extent)?;
        let (mda_img, mda_mem, mda_view) = create_upscaler_image(context, input_extent)?;
        let (lh0_img, lh0_mem, lh0_view) = create_upscaler_image(context, input_extent)?;
        let (lh1_img, lh1_mem, lh1_view) = create_upscaler_image(context, input_extent)?;
        self.motion_depth_clip_img = mdc_img;
        self.motion_depth_clip_mem = mdc_mem;
        self.motion_depth_clip_view = mdc_view;
        self.ycocg_color_img = ycc_img;
        self.ycocg_color_mem = ycc_mem;
        self.ycocg_color_view = ycc_view;
        self.motion_depth_alpha_img = mda_img;
        self.motion_depth_alpha_mem = mda_mem;
        self.motion_depth_alpha_view = mda_view;
        self.luma_history_imgs = [lh0_img, lh1_img];
        self.luma_history_mems = [lh0_mem, lh1_mem];
        self.luma_history_views = [lh0_view, lh1_view];
        self.luma_history_initialized = [false; 2];
        self.input_extent = input_extent;
        self.sgsr2_intermediates_initialized = false;
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
            for (view, img, mem) in [
                (
                    self.motion_depth_clip_view,
                    self.motion_depth_clip_img,
                    self.motion_depth_clip_mem,
                ),
                (
                    self.ycocg_color_view,
                    self.ycocg_color_img,
                    self.ycocg_color_mem,
                ),
                (
                    self.motion_depth_alpha_view,
                    self.motion_depth_alpha_img,
                    self.motion_depth_alpha_mem,
                ),
                (
                    self.luma_history_views[0],
                    self.luma_history_imgs[0],
                    self.luma_history_mems[0],
                ),
                (
                    self.luma_history_views[1],
                    self.luma_history_imgs[1],
                    self.luma_history_mems[1],
                ),
            ] {
                if view != vk::ImageView::null() {
                    self.device.destroy_image_view(view, None);
                }
                if img != vk::Image::null() {
                    self.device.destroy_image(img, None);
                }
                if mem != vk::DeviceMemory::null() {
                    self.device.free_memory(mem, None);
                }
            }
            self.device.destroy_descriptor_pool(self.ds_pool, None);
            self.device
                .destroy_descriptor_set_layout(self.convert_ds_layout, None);
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
            camera_pos: ctx.frame.camera_pos,
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

        let ubo_info = [vk::DescriptorBufferInfo::default()
            .buffer(self.ubos[frame])
            .offset(0)
            .range(std::mem::size_of::<UpscalerParamsUbo>() as vk::DeviceSize)];

        if mode == MobileUpscaler::Sgsr2 {
            self.recreate_input_targets(ctx.context, ctx.render_extent)?;

            let in_color_info = [vk::DescriptorImageInfo::default()
                .sampler(self.nearest_sampler)
                .image_view(color_view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let in_depth_info = [vk::DescriptorImageInfo::default()
                .sampler(self.nearest_sampler)
                .image_view(depth_view)
                .image_layout(depth_layout)];
            let in_vel_info = [vk::DescriptorImageInfo::default()
                .sampler(self.nearest_sampler)
                .image_view(velocity_view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let mdc_out_info = [vk::DescriptorImageInfo::default()
                .image_view(self.motion_depth_clip_view)
                .image_layout(vk::ImageLayout::GENERAL)];
            let ycc_out_info = [vk::DescriptorImageInfo::default()
                .image_view(self.ycocg_color_view)
                .image_layout(vk::ImageLayout::GENERAL)];

            let convert_writes = [
                vk::WriteDescriptorSet::default()
                    .dst_set(self.convert_sets[frame])
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&in_color_info),
                vk::WriteDescriptorSet::default()
                    .dst_set(self.convert_sets[frame])
                    .dst_binding(1)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&in_depth_info),
                vk::WriteDescriptorSet::default()
                    .dst_set(self.convert_sets[frame])
                    .dst_binding(2)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&in_vel_info),
                vk::WriteDescriptorSet::default()
                    .dst_set(self.convert_sets[frame])
                    .dst_binding(3)
                    .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                    .image_info(&mdc_out_info),
                vk::WriteDescriptorSet::default()
                    .dst_set(self.convert_sets[frame])
                    .dst_binding(4)
                    .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                    .image_info(&ycc_out_info),
                vk::WriteDescriptorSet::default()
                    .dst_set(self.convert_sets[frame])
                    .dst_binding(5)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .buffer_info(&ubo_info),
            ];
            unsafe {
                self.device.update_descriptor_sets(&convert_writes, &[]);
            }

            let old_layout = if self.sgsr2_intermediates_initialized {
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
            } else {
                vk::ImageLayout::UNDEFINED
            };
            let convert_pre = [
                image_barrier(
                    self.motion_depth_clip_img,
                    old_layout,
                    vk::ImageLayout::GENERAL,
                    vk::AccessFlags::empty(),
                    vk::AccessFlags::SHADER_WRITE,
                ),
                image_barrier(
                    self.ycocg_color_img,
                    old_layout,
                    vk::ImageLayout::GENERAL,
                    vk::AccessFlags::empty(),
                    vk::AccessFlags::SHADER_WRITE,
                ),
            ];
            let convert_post = [
                image_barrier(
                    self.motion_depth_clip_img,
                    vk::ImageLayout::GENERAL,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    vk::AccessFlags::SHADER_WRITE,
                    vk::AccessFlags::SHADER_READ,
                ),
                image_barrier(
                    self.ycocg_color_img,
                    vk::ImageLayout::GENERAL,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    vk::AccessFlags::SHADER_WRITE,
                    vk::AccessFlags::SHADER_READ,
                ),
            ];
            unsafe {
                ctx.device.cmd_pipeline_barrier(
                    ctx.cmd,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &convert_pre,
                );
                ctx.device.cmd_bind_pipeline(
                    ctx.cmd,
                    vk::PipelineBindPoint::COMPUTE,
                    self.sgsr2_convert_pipeline.pipeline,
                );
                ctx.device.cmd_bind_descriptor_sets(
                    ctx.cmd,
                    vk::PipelineBindPoint::COMPUTE,
                    self.sgsr2_convert_pipeline.layout,
                    0,
                    &[self.convert_sets[frame]],
                    &[],
                );
                ctx.device.cmd_dispatch(
                    ctx.cmd,
                    self.input_extent.width.div_ceil(8),
                    self.input_extent.height.div_ceil(8),
                    1,
                );
                ctx.device.cmd_pipeline_barrier(
                    ctx.cmd,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &convert_post,
                );
            }
            self.sgsr2_intermediates_initialized = true;

            let use_activate = ctx.settings.sgsr2_three_pass;
            if use_activate {
                let ycc_in_info = [vk::DescriptorImageInfo::default()
                    .sampler(self.nearest_sampler)
                    .image_view(self.ycocg_color_view)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
                let mdc_in_info = [vk::DescriptorImageInfo::default()
                    .sampler(self.nearest_sampler)
                    .image_view(self.motion_depth_clip_view)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
                let prev_lh_info = [vk::DescriptorImageInfo::default()
                    .sampler(self.linear_sampler)
                    .image_view(self.luma_history_views[prev])
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
                let curr_lh_out_info = [vk::DescriptorImageInfo::default()
                    .image_view(self.luma_history_views[frame])
                    .image_layout(vk::ImageLayout::GENERAL)];
                let mda_out_info = [vk::DescriptorImageInfo::default()
                    .image_view(self.motion_depth_alpha_view)
                    .image_layout(vk::ImageLayout::GENERAL)];

                let activate_writes = [
                    vk::WriteDescriptorSet::default()
                        .dst_set(self.activate_sets[frame])
                        .dst_binding(0)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&ycc_in_info),
                    vk::WriteDescriptorSet::default()
                        .dst_set(self.activate_sets[frame])
                        .dst_binding(1)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&mdc_in_info),
                    vk::WriteDescriptorSet::default()
                        .dst_set(self.activate_sets[frame])
                        .dst_binding(2)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&prev_lh_info),
                    vk::WriteDescriptorSet::default()
                        .dst_set(self.activate_sets[frame])
                        .dst_binding(3)
                        .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                        .image_info(&curr_lh_out_info),
                    vk::WriteDescriptorSet::default()
                        .dst_set(self.activate_sets[frame])
                        .dst_binding(4)
                        .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                        .image_info(&mda_out_info),
                    vk::WriteDescriptorSet::default()
                        .dst_set(self.activate_sets[frame])
                        .dst_binding(5)
                        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                        .buffer_info(&ubo_info),
                ];
                unsafe {
                    self.device.update_descriptor_sets(&activate_writes, &[]);
                }

                let mut act_pre = Vec::with_capacity(3);
                if !self.luma_history_initialized[prev] {
                    act_pre.push(image_barrier(
                        self.luma_history_imgs[prev],
                        vk::ImageLayout::UNDEFINED,
                        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        vk::AccessFlags::empty(),
                        vk::AccessFlags::SHADER_READ,
                    ));
                    self.luma_history_initialized[prev] = true;
                }
                let lh_old = if self.luma_history_initialized[frame] {
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
                } else {
                    vk::ImageLayout::UNDEFINED
                };
                act_pre.push(image_barrier(
                    self.luma_history_imgs[frame],
                    lh_old,
                    vk::ImageLayout::GENERAL,
                    vk::AccessFlags::empty(),
                    vk::AccessFlags::SHADER_WRITE,
                ));
                let mda_old =
                    if self.luma_history_initialized[0] || self.luma_history_initialized[1] {
                        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
                    } else {
                        vk::ImageLayout::UNDEFINED
                    };
                act_pre.push(image_barrier(
                    self.motion_depth_alpha_img,
                    mda_old,
                    vk::ImageLayout::GENERAL,
                    vk::AccessFlags::empty(),
                    vk::AccessFlags::SHADER_WRITE,
                ));

                let act_post = [
                    image_barrier(
                        self.luma_history_imgs[frame],
                        vk::ImageLayout::GENERAL,
                        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        vk::AccessFlags::SHADER_WRITE,
                        vk::AccessFlags::SHADER_READ,
                    ),
                    image_barrier(
                        self.motion_depth_alpha_img,
                        vk::ImageLayout::GENERAL,
                        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        vk::AccessFlags::SHADER_WRITE,
                        vk::AccessFlags::SHADER_READ,
                    ),
                ];

                unsafe {
                    ctx.device.cmd_pipeline_barrier(
                        ctx.cmd,
                        vk::PipelineStageFlags::COMPUTE_SHADER,
                        vk::PipelineStageFlags::COMPUTE_SHADER,
                        vk::DependencyFlags::empty(),
                        &[],
                        &[],
                        &act_pre,
                    );
                    ctx.device.cmd_bind_pipeline(
                        ctx.cmd,
                        vk::PipelineBindPoint::COMPUTE,
                        self.sgsr2_activate_pipeline.pipeline,
                    );
                    ctx.device.cmd_bind_descriptor_sets(
                        ctx.cmd,
                        vk::PipelineBindPoint::COMPUTE,
                        self.sgsr2_activate_pipeline.layout,
                        0,
                        &[self.activate_sets[frame]],
                        &[],
                    );
                    ctx.device.cmd_dispatch(
                        ctx.cmd,
                        self.input_extent.width.div_ceil(8),
                        self.input_extent.height.div_ceil(8),
                        1,
                    );
                    ctx.device.cmd_pipeline_barrier(
                        ctx.cmd,
                        vk::PipelineStageFlags::COMPUTE_SHADER,
                        vk::PipelineStageFlags::COMPUTE_SHADER,
                        vk::DependencyFlags::empty(),
                        &[],
                        &[],
                        &act_post,
                    );
                }
                self.luma_history_initialized[frame] = true;
            }
        }

        let (pass2_color_view, pass2_color_sampler, pass2_vel_view) = match mode {
            MobileUpscaler::Sgsr2 => (
                self.ycocg_color_view,
                self.nearest_sampler,
                if ctx.settings.sgsr2_three_pass {
                    self.motion_depth_alpha_view
                } else {
                    self.motion_depth_clip_view
                },
            ),
            MobileUpscaler::ArmAsr => (color_view, self.linear_sampler, velocity_view),
            _ => unreachable!(),
        };

        let color_info = [vk::DescriptorImageInfo::default()
            .sampler(pass2_color_sampler)
            .image_view(pass2_color_view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let depth_info = [vk::DescriptorImageInfo::default()
            .sampler(self.nearest_sampler)
            .image_view(depth_view)
            .image_layout(depth_layout)];
        let velocity_info = [vk::DescriptorImageInfo::default()
            .sampler(self.nearest_sampler)
            .image_view(pass2_vel_view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let history_info = [vk::DescriptorImageInfo::default()
            .sampler(self.linear_sampler)
            .image_view(self.views[prev])
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let output_info = [vk::DescriptorImageInfo::default()
            .image_view(self.views[frame])
            .image_layout(vk::ImageLayout::GENERAL)];

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
