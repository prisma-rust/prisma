//! Neural Radiance Cache (NRC) GPU compute pass (`nrc_clear`, `nrc_train`, `nrc_infer`).
//!
//! Implements the RTXGI v2.0 online multi-resolution hash-grid + MLP radiance
//! cache in Vulkan compute, with automatic fallback when `VK_KHR_ray_query` or
//! NVIDIA Tensor Core / NRC acceleration is unavailable.

use std::ffi::CStr;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use ash::vk;

use crate::buffer::create_buffer;
use crate::compute::ComputePipeline;
use crate::context::VulkanContext;
use crate::rtxgi_suite::NrcConfig;
use crate::shader::load_shader_module;
use crate::shader_bindings::nrc::{self as bindings, NrcSample, NrcUbo};

pub const NRC_HASH_LEVELS: usize = 4;
pub const NRC_HASH_SIZE: usize = 16_384;
pub const NRC_FEAT_DIM: usize = 2;
pub const NRC_TOTAL_WEIGHTS: usize = 1571;
pub const NRC_MAX_INFER_QUERIES: usize = 65_536;

const ENTRY_CLEAR: &CStr = c"nrcClearMain";
const ENTRY_TRAIN: &CStr = c"nrcTrainMain";
const ENTRY_INFER: &CStr = c"nrcInferMain";

struct NrcBuffer {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
}

impl NrcBuffer {
    fn create(
        context: &VulkanContext,
        size: vk::DeviceSize,
        usage: vk::BufferUsageFlags,
        props: vk::MemoryPropertyFlags,
    ) -> Result<Self> {
        let size = size.max(16);
        let (buffer, memory) =
            create_buffer(context, size, usage, props).context("NrcPass: create buffer")?;
        Ok(Self { buffer, memory })
    }

    fn write(&self, device: &ash::Device, bytes: &[u8]) -> Result<()> {
        unsafe {
            let ptr = device
                .map_memory(
                    self.memory,
                    0,
                    bytes.len() as vk::DeviceSize,
                    vk::MemoryMapFlags::empty(),
                )
                .context("NrcPass: map buffer")?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
            device.unmap_memory(self.memory);
        }
        Ok(())
    }

    fn destroy(&mut self, device: &ash::Device) {
        unsafe {
            if self.buffer != vk::Buffer::null() {
                device.destroy_buffer(self.buffer, None);
                self.buffer = vk::Buffer::null();
            }
            if self.memory != vk::DeviceMemory::null() {
                device.free_memory(self.memory, None);
                self.memory = vk::DeviceMemory::null();
            }
        }
    }
}

/// Manages GPU hash-grid + MLP weights, Adam moments, and compute pipelines for NRC.
pub struct NrcPass {
    context: Arc<VulkanContext>,
    config: NrcConfig,
    set0_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
    pipeline_clear: ComputePipeline,
    pipeline_train: ComputePipeline,
    pipeline_infer: ComputePipeline,
    ubo_buf: NrcBuffer,
    weights_buf: NrcBuffer,
    adam_m_buf: NrcBuffer,
    adam_v_buf: NrcBuffer,
    hash_grid_buf: NrcBuffer,
    train_samples_buf: NrcBuffer,
    infer_queries_buf: NrcBuffer,
    infer_output_buf: NrcBuffer,
    needs_clear: bool,
    step_index: u32,
}

impl NrcPass {
    /// Allocate NRC GPU buffers and compile `nrcClearMain`, `nrcTrainMain`, and `nrcInferMain`.
    pub fn new(context: Arc<VulkanContext>, config: NrcConfig) -> Result<Self> {
        let device = &context.device;
        let storage_bindings = [
            (
                bindings::NRC_UBO_BINDING,
                vk::DescriptorType::UNIFORM_BUFFER,
            ),
            (
                bindings::WEIGHTS_BINDING,
                vk::DescriptorType::STORAGE_BUFFER,
            ),
            (bindings::ADAM_M_BINDING, vk::DescriptorType::STORAGE_BUFFER),
            (bindings::ADAM_V_BINDING, vk::DescriptorType::STORAGE_BUFFER),
            (
                bindings::HASH_GRID_BINDING,
                vk::DescriptorType::STORAGE_BUFFER,
            ),
            (
                bindings::TRAIN_SAMPLES_BINDING,
                vk::DescriptorType::STORAGE_BUFFER,
            ),
            (
                bindings::INFER_QUERIES_BINDING,
                vk::DescriptorType::STORAGE_BUFFER,
            ),
            (
                bindings::INFER_OUTPUT_BINDING,
                vk::DescriptorType::STORAGE_BUFFER,
            ),
        ];
        let layout_bindings: Vec<vk::DescriptorSetLayoutBinding<'_>> = storage_bindings
            .iter()
            .map(|&(binding, ty)| vk::DescriptorSetLayoutBinding {
                binding,
                descriptor_type: ty,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::COMPUTE,
                ..Default::default()
            })
            .collect();

        let set0_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&layout_bindings),
                None,
            )
        }
        .context("NrcPass: create descriptor layout")?;

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UNIFORM_BUFFER,
                descriptor_count: 1,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_BUFFER,
                descriptor_count: 7,
            },
        ];
        let descriptor_pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&pool_sizes),
                None,
            )
        }
        .context("NrcPass: create descriptor pool")?;

        let descriptor_set = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(descriptor_pool)
                    .set_layouts(std::slice::from_ref(&set0_layout)),
            )
        }
        .context("NrcPass: allocate descriptor set")?[0];

        let build_pipe = |spv: &[u8], entry: &CStr| -> Result<ComputePipeline> {
            let module = load_shader_module(device, spv)?;
            let res = ComputePipeline::new(
                device,
                module,
                entry,
                std::slice::from_ref(&set0_layout),
                &[],
            );
            unsafe { device.destroy_shader_module(module, None) };
            res
        };
        let pipeline_clear = build_pipe(prism_render_shaders::NRC_CLEAR_COMP_SPV, ENTRY_CLEAR)?;
        let pipeline_train = build_pipe(prism_render_shaders::NRC_TRAIN_COMP_SPV, ENTRY_TRAIN)?;
        let pipeline_infer = build_pipe(prism_render_shaders::NRC_INFER_COMP_SPV, ENTRY_INFER)?;

        let weight_bytes = (NRC_TOTAL_WEIGHTS * 4) as vk::DeviceSize;
        let hash_bytes = (NRC_HASH_LEVELS * NRC_HASH_SIZE * NRC_FEAT_DIM * 4) as vk::DeviceSize;
        let train_cap = config.training_rays_per_frame.max(64) as vk::DeviceSize;
        let sample_stride = std::mem::size_of::<NrcSample>() as vk::DeviceSize;
        let infer_cap = NRC_MAX_INFER_QUERIES as vk::DeviceSize;

        let ubo_buf = NrcBuffer::create(
            &context,
            std::mem::size_of::<NrcUbo>() as vk::DeviceSize,
            vk::BufferUsageFlags::UNIFORM_BUFFER,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )?;
        let weights_buf = NrcBuffer::create(
            &context,
            weight_bytes,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )?;
        let adam_m_buf = NrcBuffer::create(
            &context,
            weight_bytes,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )?;
        let adam_v_buf = NrcBuffer::create(
            &context,
            weight_bytes,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )?;
        let hash_grid_buf = NrcBuffer::create(
            &context,
            hash_bytes,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )?;
        let train_samples_buf = NrcBuffer::create(
            &context,
            train_cap * sample_stride,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )?;
        let infer_queries_buf = NrcBuffer::create(
            &context,
            infer_cap * sample_stride,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )?;
        let infer_output_buf = NrcBuffer::create(
            &context,
            infer_cap * 16,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )?;

        let pass = Self {
            context,
            config,
            set0_layout,
            descriptor_pool,
            descriptor_set,
            pipeline_clear,
            pipeline_train,
            pipeline_infer,
            ubo_buf,
            weights_buf,
            adam_m_buf,
            adam_v_buf,
            hash_grid_buf,
            train_samples_buf,
            infer_queries_buf,
            infer_output_buf,
            needs_clear: true,
            step_index: 0,
        };
        pass.write_descriptor_set();
        Ok(pass)
    }

    /// Mark NRC weights and hash grid for re-initialization on next dispatch.
    pub fn invalidate(&mut self) {
        self.needs_clear = true;
        self.step_index = 0;
    }

    /// Upload a batch of training samples + inference queries and dispatch NRC compute passes.
    pub fn record_step(
        &mut self,
        cmd: vk::CommandBuffer,
        scene_min: [f32; 3],
        inv_scene_extent: f32,
        train_samples: &[NrcSample],
        infer_queries: &[NrcSample],
    ) -> Result<()> {
        let device = &self.context.device;
        let max_train = self.config.training_rays_per_frame.max(64) as usize;
        let train_count = train_samples.len().min(max_train);
        let infer_count = infer_queries.len().min(NRC_MAX_INFER_QUERIES);

        if train_count > 0 {
            self.train_samples_buf
                .write(device, bytemuck::cast_slice(&train_samples[..train_count]))?;
        }
        if infer_count > 0 {
            self.infer_queries_buf
                .write(device, bytemuck::cast_slice(&infer_queries[..infer_count]))?;
        }

        let ubo = NrcUbo {
            scene_min_scale: [scene_min[0], scene_min[1], scene_min[2], inv_scene_extent],
            train_params: [self.config.learning_rate, 0.9, 0.999, 1e-8],
            counts: [train_count as u32, infer_count as u32, self.step_index, 0],
        };
        self.ubo_buf.write(device, bytemuck::bytes_of(&ubo))?;

        if self.needs_clear {
            self.dispatch_stage(
                cmd,
                &self.pipeline_clear,
                [(NRC_TOTAL_WEIGHTS as u32).div_ceil(64), 1, 1],
            );
            self.compute_barrier(cmd);
            self.needs_clear = false;
        }
        if train_count > 0 {
            self.dispatch_stage(cmd, &self.pipeline_train, [1, 1, 1]);
            self.compute_barrier(cmd);
        }
        if infer_count > 0 {
            self.dispatch_stage(
                cmd,
                &self.pipeline_infer,
                [(infer_count as u32).div_ceil(64), 1, 1],
            );
            self.compute_barrier(cmd);
        }
        self.step_index = self.step_index.wrapping_add(1);
        Ok(())
    }

    fn dispatch_stage(&self, cmd: vk::CommandBuffer, pipeline: &ComputePipeline, groups: [u32; 3]) {
        unsafe {
            self.context.device.cmd_bind_pipeline(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                pipeline.pipeline,
            );
            self.context.device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                pipeline.layout,
                0,
                std::slice::from_ref(&self.descriptor_set),
                &[],
            );
            self.context
                .device
                .cmd_dispatch(cmd, groups[0], groups[1], groups[2]);
        }
    }

    fn compute_barrier(&self, cmd: vk::CommandBuffer) {
        let mem = vk::MemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::SHADER_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE);
        unsafe {
            self.context.device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[mem],
                &[],
                &[],
            );
        }
    }

    fn write_descriptor_set(&self) {
        let infos = [
            vk::DescriptorBufferInfo::default()
                .buffer(self.ubo_buf.buffer)
                .range(vk::WHOLE_SIZE),
            vk::DescriptorBufferInfo::default()
                .buffer(self.weights_buf.buffer)
                .range(vk::WHOLE_SIZE),
            vk::DescriptorBufferInfo::default()
                .buffer(self.adam_m_buf.buffer)
                .range(vk::WHOLE_SIZE),
            vk::DescriptorBufferInfo::default()
                .buffer(self.adam_v_buf.buffer)
                .range(vk::WHOLE_SIZE),
            vk::DescriptorBufferInfo::default()
                .buffer(self.hash_grid_buf.buffer)
                .range(vk::WHOLE_SIZE),
            vk::DescriptorBufferInfo::default()
                .buffer(self.train_samples_buf.buffer)
                .range(vk::WHOLE_SIZE),
            vk::DescriptorBufferInfo::default()
                .buffer(self.infer_queries_buf.buffer)
                .range(vk::WHOLE_SIZE),
            vk::DescriptorBufferInfo::default()
                .buffer(self.infer_output_buf.buffer)
                .range(vk::WHOLE_SIZE),
        ];
        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(bindings::NRC_UBO_BINDING)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .buffer_info(std::slice::from_ref(&infos[0])),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(bindings::WEIGHTS_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(std::slice::from_ref(&infos[1])),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(bindings::ADAM_M_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(std::slice::from_ref(&infos[2])),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(bindings::ADAM_V_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(std::slice::from_ref(&infos[3])),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(bindings::HASH_GRID_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(std::slice::from_ref(&infos[4])),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(bindings::TRAIN_SAMPLES_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(std::slice::from_ref(&infos[5])),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(bindings::INFER_QUERIES_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(std::slice::from_ref(&infos[6])),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_set)
                .dst_binding(bindings::INFER_OUTPUT_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(std::slice::from_ref(&infos[7])),
        ];
        unsafe {
            self.context.device.update_descriptor_sets(&writes, &[]);
        }
    }
}

impl Drop for NrcPass {
    fn drop(&mut self) {
        let device = &self.context.device;
        self.ubo_buf.destroy(device);
        self.weights_buf.destroy(device);
        self.adam_m_buf.destroy(device);
        self.adam_v_buf.destroy(device);
        self.hash_grid_buf.destroy(device);
        self.train_samples_buf.destroy(device);
        self.infer_queries_buf.destroy(device);
        self.infer_output_buf.destroy(device);
        unsafe {
            if self.descriptor_pool != vk::DescriptorPool::null() {
                device.destroy_descriptor_pool(self.descriptor_pool, None);
            }
            if self.set0_layout != vk::DescriptorSetLayout::null() {
                device.destroy_descriptor_set_layout(self.set0_layout, None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nrc_weight_and_sample_layouts_match_shader() {
        assert_eq!(NRC_TOTAL_WEIGHTS, 12 * 32 + 32 + 32 * 32 + 32 + 32 * 3 + 3);
        assert_eq!(std::mem::size_of::<NrcUbo>(), 48);
        assert_eq!(std::mem::size_of::<NrcSample>(), 48);
    }
}
