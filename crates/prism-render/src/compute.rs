//! Compute pipeline creation.
//!
//! Minimal wrapper around `vk::Pipeline` + `vk::PipelineLayout`, used for compute shaders.
//! By global illumination baker (raycast query probe volume bake) and not-yet-
//! DDGI real-time update channel use.

use anyhow::Context as _;
use ash::vk;

/// An already compiled compute pipeline and its layout
pub struct ComputePipeline {
    pub pipeline: vk::Pipeline,
    pub layout: vk::PipelineLayout,
    device: ash::Device,
}

impl ComputePipeline {
    /// Create a compute pipeline from a SPIR-V shader module.
    ///
    /// * `entry_point` — shader entry name (e.g. `"bakeMain"`).
    /// * `set_layouts` — descriptor set layouts the shader expects.
    /// * `push_ranges` — optional push constant ranges.
    pub fn new(
        device: &ash::Device,
        shader_module: vk::ShaderModule,
        entry_point: &std::ffi::CStr,
        set_layouts: &[vk::DescriptorSetLayout],
        push_ranges: &[vk::PushConstantRange],
    ) -> anyhow::Result<Self> {
        let layout = unsafe {
            device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(set_layouts)
                    .push_constant_ranges(push_ranges),
                None,
            )
        }
        .context("ComputePipeline: create pipeline layout")?;

        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(shader_module)
            .name(entry_point);

        let pipeline = unsafe {
            device.create_compute_pipelines(
                vk::PipelineCache::null(),
                &[vk::ComputePipelineCreateInfo::default()
                    .stage(stage)
                    .layout(layout)],
                None,
            )
        }
        .map_err(|(_, e)| anyhow::anyhow!("ComputePipeline: create pipeline: {e:?}"))?[0];

        Ok(Self {
            pipeline,
            layout,
            device: device.clone(),
        })
    }
}

impl Drop for ComputePipeline {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_pipeline_layout(self.layout, None);
        }
    }
}
