//! 3D particle billboard pass.
//!
//! [`ParticlePass`] draws CPU-simulated particles ([`ParticleDrawItem`]) as
//! **instanced quads** into ForwardPass HDR color attachment. Difference from
//! [`crate::sprite_pass::SpritePass`] only three points:
//!
//! - **Keep depth test** (write disabled): particles in world space, foreground geometry must occlude them;
//! - **Camera-facing expansion**: vertex stage uses `GraphFrame::inv_view_rot` right / up
//!   columns to expand quad, CPU only submits center + size;
//! - **No texture sampling**: alpha from quad UV radial falloff (soft round point), so
//!   particles need no asset, nor bindless table.
//!
//! Sorting (far to near) done by engine before submit, `ForwardPass` just draws in list order;
//! blend is straight alpha (`SRC_ALPHA` / `ONE_MINUS_SRC_ALPHA`).

use std::mem::size_of;

use anyhow::{Context as _, Result};
use ash::vk;

use crate::buffer::{create_buffer, BufferUsage, MemoryProperties};
use crate::context::VulkanContext;
use crate::pipeline::{GraphicsPipeline, PipelineDesc};
use crate::render_graph::ParticleDrawItem;
use crate::shader;
use crate::shader_bindings::particles as bindings;

const _: () = assert!(size_of::<bindings::ParticlePush>() as u32 == bindings::PUSH_CONSTANT_SIZE);

const PARTICLE_VERT_SPV: &[u8] = prism_render_shaders::PARTICLES_VERT_SPV;
const PARTICLE_FRAG_SPV: &[u8] = prism_render_shaders::PARTICLES_FRAG_SPV;

/// Initial instance capacity (grows on demand).
const INITIAL_CAPACITY: u32 = 256;

/// Single instance vertex data — must match `particles.slang` `ParticleVertexIn`.
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct ParticleInstance {
    /// xyz = world-space center, w = radius in world units.
    center_size: [f32; 4],
    /// Linear RGBA (already interpolated by particle age).
    color: [f32; 4],
}

impl ParticleInstance {
    fn binding_description() -> vk::VertexInputBindingDescription {
        vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(size_of::<Self>() as u32)
            .input_rate(vk::VertexInputRate::INSTANCE)
    }

    fn attribute_descriptions() -> [vk::VertexInputAttributeDescription; 2] {
        let vec4 = |location: u32, offset: u32| {
            vk::VertexInputAttributeDescription::default()
                .location(location)
                .binding(0)
                .format(vk::Format::R32G32B32A32_SFLOAT)
                .offset(offset)
        };
        [vec4(0, 0), vec4(1, 16)]
    }
}

/// 3D particle instanced draw pass.
pub struct ParticlePass {
    pipeline: Option<GraphicsPipeline>,
    /// Render pass used at pipeline compile (needs recompile when ForwardPass rebuilds).
    built_for_render_pass: Option<vk::RenderPass>,
    instance_buffer: vk::Buffer,
    instance_memory: vk::DeviceMemory,
    instance_capacity: u32,
    device: Option<ash::Device>,
    /// Instance buffers still referenced by in-flight work (see `crate::retire`).
    retire: crate::retire::RetireQueue,
    /// Frame slot being recorded.
    frame_index: u32,
}

impl ParticlePass {
    /// Create empty pass; instance buffer lazily allocated at first [`ParticlePass::draw`]
    /// (no `VulkanContext` at ForwardPass construction).
    pub fn new() -> Self {
        Self {
            pipeline: None,
            built_for_render_pass: None,
            instance_buffer: vk::Buffer::null(),
            instance_memory: vk::DeviceMemory::null(),
            instance_capacity: 0,
            device: None,
            retire: crate::retire::RetireQueue::new(),
            frame_index: 0,
        }
    }

    /// Draw `items` inside already-open render pass (in given order, first drawn gets covered).
    ///
    /// `inv_view_rot` is "transpose of view rotation" (`GraphFrame::inv_view_rot`),
    /// its column 0 / 1 are camera world-space right / up, used to expand quad into
    /// billboard.
    #[expect(
        clippy::too_many_arguments,
        reason = "arity mirrors the float-parameter packing that the callers already flatten"
    )]
    pub fn draw(
        &mut self,
        device: &ash::Device,
        context: &VulkanContext,
        cmd: vk::CommandBuffer,
        render_pass: vk::RenderPass,
        view_proj: &[[f32; 4]; 4],
        inv_view_rot: &[[f32; 4]; 4],
        items: &[ParticleDrawItem],
        frame_index: u32,
    ) -> Result<()> {
        self.retire.collect(device, frame_index);
        self.frame_index = frame_index;
        if items.is_empty() {
            return Ok(());
        }
        self.ensure_pipeline(device, render_pass)?;
        self.grow(device, context, items.len() as u32)?;
        self.upload(context, items)?;

        let pipeline = self.pipeline.as_ref().expect("pipeline built above");

        let camera_right = [
            inv_view_rot[0][0],
            inv_view_rot[0][1],
            inv_view_rot[0][2],
            0.0,
        ];
        let camera_up = [
            inv_view_rot[1][0],
            inv_view_rot[1][1],
            inv_view_rot[1][2],
            0.0,
        ];
        let push = bindings::ParticlePush {
            view_proj: *view_proj,
            cam_right: camera_right,
            cam_up: camera_up,
        };

        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline.pipeline);
            device.cmd_push_constants(
                cmd,
                pipeline.layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                bytemuck::bytes_of(&push),
            );
            device.cmd_bind_vertex_buffers(cmd, 0, &[self.instance_buffer], &[0]);
            device.cmd_draw(cmd, 6, items.len() as u32, 0, 0);
        }

        Ok(())
    }

    fn ensure_pipeline(&mut self, device: &ash::Device, render_pass: vk::RenderPass) -> Result<()> {
        if self.pipeline.is_some() && self.built_for_render_pass == Some(render_pass) {
            return Ok(());
        }
        self.device = Some(device.clone());
        self.pipeline = None;

        let vert_module =
            shader::load_shader_module(device, PARTICLE_VERT_SPV).context("ParticlePass: vert")?;
        let frag_module =
            shader::load_shader_module(device, PARTICLE_FRAG_SPV).context("ParticlePass: frag")?;
        let vert_entry = std::ffi::CString::new(bindings::ENTRY_VERTEX_MAIN).unwrap();
        let frag_entry = std::ffi::CString::new(bindings::ENTRY_FRAGMENT_MAIN).unwrap();
        let vert_stage = shader::shader_stage(
            vk::ShaderStageFlags::VERTEX,
            vert_module,
            vert_entry.as_c_str(),
        );
        let frag_stage = shader::shader_stage(
            vk::ShaderStageFlags::FRAGMENT,
            frag_module,
            frag_entry.as_c_str(),
        );
        let shader_stages = [vert_stage, frag_stage];

        let binding_descs = [ParticleInstance::binding_description()];
        let attr_descs = ParticleInstance::attribute_descriptions();

        let push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(bindings::PUSH_CONSTANT_SIZE)];

        let blend_attachments = [
            vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(vk::ColorComponentFlags::RGBA)
                .blend_enable(true)
                .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
                .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(vk::BlendFactor::ONE)
                .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
                .alpha_blend_op(vk::BlendOp::ADD),
            vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(vk::ColorComponentFlags::empty())
                .blend_enable(false),
            vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(vk::ColorComponentFlags::empty())
                .blend_enable(false),
            vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(vk::ColorComponentFlags::empty())
                .blend_enable(false),
        ];

        let pipeline = GraphicsPipeline::new(&PipelineDesc {
            device,
            shader_stages: &shader_stages,
            vertex_binding_desc: &binding_descs,
            vertex_attr_descs: &attr_descs,
            descriptor_set_layouts: &[],
            push_constant_ranges: &push,
            render_pass,
            subpass: 0,
            cull_mode: Some(vk::CullModeFlags::NONE),
            depth_bias_enable: None,
            depth_bias_constant_factor: None,
            depth_bias_slope_factor: None,
            depth_write_enable: Some(false),
            depth_test_enable: Some(true),
            color_attachment_count: None,
            color_blend_attachments: Some(&blend_attachments),
            front_face: None,
            polygon_mode: None,
        })
        .context("ParticlePass: create pipeline")?;

        unsafe {
            device.destroy_shader_module(vert_module, None);
            device.destroy_shader_module(frag_module, None);
        }

        self.pipeline = Some(pipeline);
        self.built_for_render_pass = Some(render_pass);
        Ok(())
    }

    fn grow(&mut self, _device: &ash::Device, context: &VulkanContext, needed: u32) -> Result<()> {
        if needed <= self.instance_capacity {
            return Ok(());
        }
        let capacity = needed.next_power_of_two().max(INITIAL_CAPACITY);
        let (buffer, memory) = create_buffer(
            context,
            capacity as vk::DeviceSize * size_of::<ParticleInstance>() as vk::DeviceSize,
            BufferUsage::VERTEX_BUFFER,
            MemoryProperties::HOST_VISIBLE | MemoryProperties::HOST_COHERENT,
        )
        .context("ParticlePass: grow instance buffer")?;
        if self.instance_buffer != vk::Buffer::null()
            || self.instance_memory != vk::DeviceMemory::null()
        {
            self.retire.retire(
                self.frame_index,
                crate::retire::RetiredResource::Buffer {
                    buffer: self.instance_buffer,
                    memory: self.instance_memory,
                },
            );
        }
        self.instance_buffer = buffer;
        self.instance_memory = memory;
        self.instance_capacity = capacity;
        Ok(())
    }

    fn upload(&self, context: &VulkanContext, items: &[ParticleDrawItem]) -> Result<()> {
        let bytes = items.len() * size_of::<ParticleInstance>();
        let instances: Vec<ParticleInstance> = items
            .iter()
            .map(|item| ParticleInstance {
                center_size: [item.center[0], item.center[1], item.center[2], item.size],
                color: item.color,
            })
            .collect();
        unsafe {
            let ptr = context
                .device
                .map_memory(
                    self.instance_memory,
                    0,
                    bytes as vk::DeviceSize,
                    vk::MemoryMapFlags::empty(),
                )
                .context("ParticlePass: map instance memory")?;
            std::ptr::copy_nonoverlapping(
                bytemuck::cast_slice(&instances).as_ptr(),
                ptr as *mut u8,
                bytes,
            );
            context.device.unmap_memory(self.instance_memory);
        }
        Ok(())
    }

    /// Release GPU resources before swapchain rebuild / destroy.
    pub fn destroy(&mut self, device: &ash::Device) {
        self.pipeline = None;
        self.built_for_render_pass = None;
        self.retire.destroy_all(device);
        unsafe {
            if self.instance_buffer != vk::Buffer::null() {
                device.destroy_buffer(self.instance_buffer, None);
                self.instance_buffer = vk::Buffer::null();
            }
            if self.instance_memory != vk::DeviceMemory::null() {
                device.free_memory(self.instance_memory, None);
                self.instance_memory = vk::DeviceMemory::null();
            }
        }
        self.instance_capacity = 0;
        self.device = None;
    }
}

impl Default for ParticlePass {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ParticlePass {
    fn drop(&mut self) {
        if let Some(device) = self.device.take() {
            self.pipeline = None;
            unsafe {
                if self.instance_buffer != vk::Buffer::null() {
                    device.destroy_buffer(self.instance_buffer, None);
                }
                if self.instance_memory != vk::DeviceMemory::null() {
                    device.free_memory(self.instance_memory, None);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Push constant block in shader is `viewProj`(64) + `camRight`(16) +
    /// `camUp`(16); Rust side must not differ by a single byte, otherwise camera basis wrong.
    #[test]
    fn push_constant_layout_matches_the_shader() {
        assert_eq!(size_of::<bindings::ParticlePush>(), 96);
        assert_eq!(bindings::PUSH_CONSTANT_SIZE, 96);
        let push = bindings::ParticlePush::default();
        assert_eq!(
            (&push.view_proj as *const _ as usize) - (&push as *const _ as usize),
            0
        );
        assert_eq!(
            (&push.cam_right as *const _ as usize) - (&push as *const _ as usize),
            64
        );
        assert_eq!(
            (&push.cam_up as *const _ as usize) - (&push as *const _ as usize),
            80
        );
    }

    /// Single instance record = center(12) + size(4) + color(16).
    #[test]
    fn the_instance_record_is_thirty_two_bytes() {
        assert_eq!(size_of::<ParticleInstance>(), 32);
    }
}
