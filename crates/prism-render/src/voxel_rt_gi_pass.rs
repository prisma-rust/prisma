//! Hardware Ray Tracing (`VK_KHR_ray_query`) 1-spp voxel GI render pass.
//!
//! Operates alongside `VoxelRtAccel` and `voxel_rt_gi.comp.spv`, decoupled from the
//! software DDA pass so it can be toggled cleanly based on device `ray_query` support.

use anyhow::Context as _;
use ash::vk;

use crate::context::VulkanContext;
use crate::render_graph::{
    GraphResources, RenderContext, RenderGraphBuilder, RenderPassNode, RenderSettings,
};
use crate::voxel_dda_pass::VoxelChunkUpload;
use crate::voxel_rt_accel::VoxelRtAccel;

const VOXEL_RT_GI_SPV: &[u8] = include_bytes!("../../../assets/shaders/voxel_rt_gi.comp.spv");

pub struct VoxelRtGiPass {
    device: ash::Device,
    accel: VoxelRtAccel,
    descriptor_layout: vk::DescriptorSetLayout,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    descriptor_pool: vk::DescriptorPool,
    #[allow(dead_code)]
    descriptor_set: vk::DescriptorSet,
}

impl VoxelRtGiPass {
    pub fn new(context: &VulkanContext) -> anyhow::Result<Self> {
        let device = context.device.clone();

        let bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(2)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(3)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(4)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(5)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(6)
                .descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(7)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(8)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(9)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
        ];

        let layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        let descriptor_layout = unsafe { device.create_descriptor_set_layout(&layout_info, None) }
            .context("VoxelRtGiPass: create descriptor set layout")?;

        let pipeline_layout_info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(std::slice::from_ref(&descriptor_layout));
        let pipeline_layout = unsafe { device.create_pipeline_layout(&pipeline_layout_info, None) }
            .context("VoxelRtGiPass: create pipeline layout")?;

        let words: Vec<u32> = VOXEL_RT_GI_SPV
            .as_chunks::<4>()
            .0
            .iter()
            .copied()
            .map(u32::from_le_bytes)
            .collect();
        let shader_info = vk::ShaderModuleCreateInfo::default().code(&words);
        let shader_module = unsafe { device.create_shader_module(&shader_info, None) }
            .context("VoxelRtGiPass: create shader module")?;

        let entry = c"voxelRtGiMain";
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(shader_module)
            .name(entry);
        let cp_info = vk::ComputePipelineCreateInfo::default()
            .stage(stage)
            .layout(pipeline_layout);

        let pipeline =
            unsafe { device.create_compute_pipelines(vk::PipelineCache::null(), &[cp_info], None) }
                .map_err(|(_, e)| e)
                .context("VoxelRtGiPass: create compute pipeline")?[0];

        unsafe {
            device.destroy_shader_module(shader_module, None);
        }

        let pool_sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(5),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
                .descriptor_count(1),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(2),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1),
        ];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(1)
            .pool_sizes(&pool_sizes);
        let descriptor_pool = unsafe { device.create_descriptor_pool(&pool_info, None) }
            .context("VoxelRtGiPass: create descriptor pool")?;

        let alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(descriptor_pool)
            .set_layouts(std::slice::from_ref(&descriptor_layout));
        let descriptor_set = unsafe { device.allocate_descriptor_sets(&alloc_info) }
            .context("VoxelRtGiPass: allocate descriptor set")?[0];

        Ok(Self {
            device,
            accel: VoxelRtAccel::new(),
            descriptor_layout,
            pipeline_layout,
            pipeline,
            descriptor_pool,
            descriptor_set,
        })
    }

    pub fn sync_chunks(
        &mut self,
        context: &VulkanContext,
        command_pool: vk::CommandPool,
        chunks: &[VoxelChunkUpload],
    ) -> anyhow::Result<()> {
        self.accel.sync_chunks(context, command_pool, chunks)
    }

    pub fn tlas_handle(&self) -> Option<vk::AccelerationStructureKHR> {
        self.accel.tlas_handle()
    }
}

impl Drop for VoxelRtGiPass {
    fn drop(&mut self) {
        unsafe {
            self.device
                .destroy_descriptor_pool(self.descriptor_pool, None);
            self.device.destroy_pipeline(self.pipeline, None);
            self.device
                .destroy_pipeline_layout(self.pipeline_layout, None);
            self.device
                .destroy_descriptor_set_layout(self.descriptor_layout, None);
        }
    }
}

impl RenderPassNode for VoxelRtGiPass {
    fn name(&self) -> &str {
        "VoxelRtGiPass"
    }

    fn setup(&mut self, _graph: &mut RenderGraphBuilder, _settings: &RenderSettings) {}

    fn execute(
        &mut self,
        _ctx: &RenderContext,
        _resources: &mut GraphResources,
    ) -> anyhow::Result<()> {
        // Ready to bind VoxelDdaPass G-Buffer & TLAS when GraphRenderer schedules RT GI.
        Ok(())
    }
}
