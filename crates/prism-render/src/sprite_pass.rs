//! 2D sprite pass.
//!
//! [`SpritePass`] draws ECS-collected sprite instances ([`SpriteDrawItem`]) as
//! **instanced quads** into ForwardPass HDR color attachment — like
//! [`crate::skybox_pass::SkyboxPass`], it does not own its own render pass /
//! framebuffer, just records commands in ForwardPass already-open pass.
//!
//! Design points (see `projects/demos/fishing/DESIGN.md`):
//!
//! - **One draw call**: all sprites share one instance buffer, texture index into bindless table;
//! - **Disable depth test / write**: 2D front-back order guaranteed by CPU sort
//!   (`sorting_layer` → `order_in_layer` → `z`), not depending on depth buffer;
//! - **Nearest sampling**: pixel art in `sprites.slang` fixed uses nearest sampler from bindless table,
//!   frame rect pre-clamped on CPU to avoid sprite texture bleeding;
//! - Output to linear HDR target, `PostPass` with `tonemap_mode = 2` (passthrough)
//!   preserves original palette.
//!
//! Descriptor layout: binds ForwardPass shared bindless table, but in **set 0**
//! (layout object independent of set index; scene pipeline binds it in set 1).

use std::mem::size_of;

use anyhow::{Context as _, Result};
use ash::vk;

use crate::buffer::{create_buffer, BufferUsage, MemoryProperties};
use crate::context::VulkanContext;
use crate::pipeline::{blend_factor, blend_operation, GraphicsPipeline, PipelineDesc};
use crate::render_graph::{SpriteDrawItem, SpriteLight, SPRITE_LIGHT_MAX};
use crate::shader;
use crate::shader_bindings::sprites as bindings;
use prism_gfx::{BlendMode, BlendOp};

const _: () = assert!(size_of::<bindings::SpritePush>() as u32 == bindings::PUSH_CONSTANT_SIZE);

const SPRITE_VERT_SPV: &[u8] = prism_render_shaders::SPRITES_VERT_SPV;
const SPRITE_FRAG_SPV: &[u8] = prism_render_shaders::SPRITES_FRAG_SPV;

/// Initial instance capacity (grows on demand).
const INITIAL_CAPACITY: u32 = 256;

/// Single instance vertex data — must match `sprites.slang` `SpriteVertexIn`.
///
/// Four corner points are **world-space** coords (order `BL, BR, TR, TL`): sprite model matrix expanded on
/// CPU side, polyline triangles given directly as vertices (#604). Triangle = quad split into last two corners.
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct SpriteInstance {
    /// Quad four corners (world space).
    corners: [[f32; 3]; 4],
    /// UV rect `(u0, v0, u1, v1)`.
    uv_rect: [f32; 4],
    /// Linear RGBA multiply color.
    color: [f32; 4],
    /// Bindless texture index.
    texture: u32,
    _pad: [u32; 3],
    /// World-space clip rect `[min_x, min_y, max_x, max_y]` (#582).
    /// `max < min` = no clip.
    clip: [f32; 4],
    /// Bit flags: bit 0 = receives 2D lighting (#611).
    flags: u32,
}

impl SpriteInstance {
    fn binding_description() -> vk::VertexInputBindingDescription {
        vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(size_of::<Self>() as u32)
            .input_rate(vk::VertexInputRate::INSTANCE)
    }

    fn attribute_descriptions() -> [vk::VertexInputAttributeDescription; 9] {
        let vec4 = |location: u32, offset: u32| {
            vk::VertexInputAttributeDescription::default()
                .location(location)
                .binding(0)
                .format(vk::Format::R32G32B32A32_SFLOAT)
                .offset(offset)
        };
        let vec3 = |location: u32, offset: u32| {
            vk::VertexInputAttributeDescription::default()
                .location(location)
                .binding(0)
                .format(vk::Format::R32G32B32_SFLOAT)
                .offset(offset)
        };
        [
            vec3(0, 0),
            vec3(1, 12),
            vec3(2, 24),
            vec3(3, 36),
            vec4(4, 48),
            vec4(5, 64),
            vk::VertexInputAttributeDescription::default()
                .binding(0)
                .location(6)
                .format(vk::Format::R32_UINT)
                .offset(80),
            vec4(7, 96),
            vk::VertexInputAttributeDescription::default()
                .binding(0)
                .location(8)
                .format(vk::Format::R32_UINT)
                .offset(112),
        ]
    }
}

/// Expand four world-space corners from model matrix (fully consistent with old vertex shader
/// `m0*x + m1*y + m3`, order `BL, BR, TR, TL`).
fn corners_from_model(model: &[[f32; 4]; 4]) -> [[f32; 3]; 4] {
    const UNIT: [[f32; 2]; 4] = [[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]];
    let (m0, m1, m3) = (model[0], model[1], model[3]);
    let mut out = [[0.0f32; 3]; 4];
    for (corner, xy) in out.iter_mut().zip(UNIT) {
        for (axis, value) in corner.iter_mut().enumerate() {
            *value = m0[axis] * xy[0] + m1[axis] * xy[1] + m3[axis];
        }
    }
    out
}

/// `(src, dst, op)` uniquely identifies a blend pipeline.
type BlendKey = (BlendMode, BlendMode, BlendOp);

/// 2D sprite instanced draw pass.
pub struct SpritePass {
    /// Pipeline cached by blend state (#610): alpha / Additive / Multiply share instance buffer.
    pipelines: std::collections::HashMap<BlendKey, GraphicsPipeline>,
    /// Render pass used at pipeline compile (needs recompile when ForwardPass rebuilds).
    built_for_render_pass: Option<vk::RenderPass>,
    instance_buffer: vk::Buffer,
    instance_memory: vk::DeviceMemory,
    instance_capacity: u32,
    device: Option<ash::Device>,

    /// set 1 binding 0: `StructuredBuffer<SpriteLight>`.
    light_layout: vk::DescriptorSetLayout,
    light_pool: vk::DescriptorPool,
    light_set: vk::DescriptorSet,
    light_buffer: vk::Buffer,
    light_memory: vk::DeviceMemory,
    /// Instance buffers still referenced by in-flight work (see `crate::retire`).
    retire: crate::retire::RetireQueue,
    /// Frame slot being recorded.
    frame_index: u32,
    /// Reusable CPU staging vector for instance packing.
    scratch_instances: Vec<SpriteInstance>,
}

impl SpritePass {
    /// Creates an empty pass; the instance buffer is allocated lazily on the
    /// first [`SpritePass::draw`] (ForwardPass is constructed without a
    /// `VulkanContext`, so it cannot allocate GPU memory here).
    pub fn new() -> Self {
        Self {
            pipelines: std::collections::HashMap::new(),
            built_for_render_pass: None,
            instance_buffer: vk::Buffer::null(),
            instance_memory: vk::DeviceMemory::null(),
            instance_capacity: 0,
            device: None,
            light_layout: vk::DescriptorSetLayout::null(),
            light_pool: vk::DescriptorPool::null(),
            light_set: vk::DescriptorSet::null(),
            light_buffer: vk::Buffer::null(),
            light_memory: vk::DeviceMemory::null(),
            retire: crate::retire::RetireQueue::new(),
            frame_index: 0,
            scratch_instances: Vec::new(),
        }
    }
}

impl Default for SpritePass {
    fn default() -> Self {
        Self::new()
    }
}

impl SpritePass {
    /// Draw `items` inside already-open render pass (in given order, first drawn covered first).
    ///
    /// `bindless_set` / `bindless_layout` provided by ForwardPass; `view_proj`
    /// is this frame world→clip matrix (for 2D the orthographic camera matrix).
    #[expect(
        clippy::too_many_arguments,
        reason = "draw takes the per-frame matrices and descriptor sets the caller owns"
    )]
    pub fn draw(
        &mut self,
        device: &ash::Device,
        context: &VulkanContext,
        cmd: vk::CommandBuffer,
        render_pass: vk::RenderPass,
        bindless_set: vk::DescriptorSet,
        bindless_layout: vk::DescriptorSetLayout,
        view_proj: &[[f32; 4]; 4],
        items: &[SpriteDrawItem],
        lights: &[SpriteLight],
        frame_index: u32,
    ) -> Result<()> {
        self.draw_inner(
            device,
            context,
            cmd,
            render_pass,
            bindless_set,
            bindless_layout,
            view_proj,
            items,
            lights,
            frame_index,
            false,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "draw_inner takes the per-frame matrices and descriptor sets the caller owns"
    )]
    pub(crate) fn draw_inner(
        &mut self,
        device: &ash::Device,
        context: &VulkanContext,
        cmd: vk::CommandBuffer,
        render_pass: vk::RenderPass,
        bindless_set: vk::DescriptorSet,
        bindless_layout: vk::DescriptorSetLayout,
        view_proj: &[[f32; 4]; 4],
        items: &[SpriteDrawItem],
        lights: &[SpriteLight],
        frame_index: u32,
        single_color_target: bool,
    ) -> Result<()> {
        self.retire.collect(device, frame_index);
        self.frame_index = frame_index;
        if items.is_empty() || bindless_set == vk::DescriptorSet::null() {
            return Ok(());
        }
        self.ensure_lights(device, context)?;
        self.upload_lights(device, lights)?;
        if self.built_for_render_pass != Some(render_pass) {
            self.pipelines.clear();
            self.built_for_render_pass = Some(render_pass);
        }
        self.grow(device, context, items.len() as u32)?;
        self.upload(context, items)?;

        let mut start = 0usize;
        while start < items.len() {
            let key = (
                items[start].src_blend,
                items[start].dst_blend,
                items[start].blend_op,
            );
            let mut end = start + 1;
            while end < items.len()
                && (
                    items[end].src_blend,
                    items[end].dst_blend,
                    items[end].blend_op,
                ) == key
            {
                end += 1;
            }
            self.ensure_pipeline(
                device,
                render_pass,
                bindless_layout,
                key,
                single_color_target,
            )?;
            let pipeline = self.pipelines.get(&key).expect("pipeline built above");
            unsafe {
                device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline.pipeline);
                device.cmd_bind_descriptor_sets(
                    cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    pipeline.layout,
                    0,
                    std::slice::from_ref(&bindless_set),
                    &[],
                );
                device.cmd_bind_descriptor_sets(
                    cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    pipeline.layout,
                    1,
                    std::slice::from_ref(&self.light_set),
                    &[],
                );
                let push = bindings::SpritePush {
                    view_proj: *view_proj,
                    light_params: [lights.len().min(SPRITE_LIGHT_MAX) as f32, 0.0, 0.0, 0.0],
                };
                device.cmd_push_constants(
                    cmd,
                    pipeline.layout,
                    vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                    0,
                    bytemuck::bytes_of(&push),
                );
                device.cmd_bind_vertex_buffers(cmd, 0, &[self.instance_buffer], &[0]);
                device.cmd_draw(cmd, 6, (end - start) as u32, 0, start as u32);
            }
            start = end;
        }

        Ok(())
    }

    fn ensure_pipeline(
        &mut self,
        device: &ash::Device,
        render_pass: vk::RenderPass,
        bindless_layout: vk::DescriptorSetLayout,
        blend: BlendKey,
        single_color_target: bool,
    ) -> Result<()> {
        if self.pipelines.contains_key(&blend) {
            return Ok(());
        }
        let bindless_set_layout = bindless_layout;
        self.device = Some(device.clone());

        let vert_module =
            shader::load_shader_module(device, SPRITE_VERT_SPV).context("SpritePass: vert")?;
        let frag_module =
            shader::load_shader_module(device, SPRITE_FRAG_SPV).context("SpritePass: frag")?;
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

        let binding_descs = [SpriteInstance::binding_description()];
        let attr_descs = SpriteInstance::attribute_descriptions();

        let push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(bindings::PUSH_CONSTANT_SIZE)];

        let (src_blend, dst_blend, blend_op) = blend;
        let alpha_blend = src_blend == BlendMode::SrcAlpha
            && dst_blend == BlendMode::OneMinusSrcAlpha
            && blend_op == BlendOp::Add;
        let blend_attachments = [
            vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(vk::ColorComponentFlags::RGBA)
                .blend_enable(true)
                .src_color_blend_factor(blend_factor(src_blend))
                .dst_color_blend_factor(blend_factor(dst_blend))
                .color_blend_op(blend_operation(blend_op))
                .src_alpha_blend_factor(if alpha_blend {
                    vk::BlendFactor::ONE
                } else {
                    vk::BlendFactor::ZERO
                })
                .dst_alpha_blend_factor(if alpha_blend {
                    vk::BlendFactor::ZERO
                } else {
                    vk::BlendFactor::ONE
                })
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

        let active_attachments = if single_color_target {
            &blend_attachments[..1]
        } else {
            &blend_attachments[..]
        };
        let pipeline = GraphicsPipeline::new(&PipelineDesc {
            device,
            shader_stages: &shader_stages,
            vertex_binding_desc: &binding_descs,
            vertex_attr_descs: &attr_descs,
            descriptor_set_layouts: &[bindless_set_layout, self.light_layout],
            push_constant_ranges: &push,
            render_pass,
            subpass: 0,
            cull_mode: Some(vk::CullModeFlags::NONE),
            depth_bias_enable: None,
            depth_bias_constant_factor: None,
            depth_bias_slope_factor: None,
            depth_write_enable: Some(false),
            depth_test_enable: Some(false),
            color_attachment_count: None,
            color_blend_attachments: Some(active_attachments),
            front_face: None,
            polygon_mode: None,
        })
        .context("SpritePass: create pipeline")?;

        unsafe {
            device.destroy_shader_module(vert_module, None);
            device.destroy_shader_module(frag_module, None);
        }

        self.pipelines.insert(blend, pipeline);
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
            capacity as vk::DeviceSize * size_of::<SpriteInstance>() as vk::DeviceSize,
            BufferUsage::VERTEX_BUFFER,
            MemoryProperties::HOST_VISIBLE | MemoryProperties::HOST_COHERENT,
        )
        .context("SpritePass: grow instance buffer")?;
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

    fn upload(&mut self, context: &VulkanContext, items: &[SpriteDrawItem]) -> Result<()> {
        let bytes = items.len() * size_of::<SpriteInstance>();
        self.scratch_instances.clear();
        self.scratch_instances.extend(items.iter().map(|item| {
            SpriteInstance {
                corners: item
                    .corners
                    .unwrap_or_else(|| corners_from_model(&item.model)),
                uv_rect: item.uv_rect,
                color: item.color,
                texture: item.texture,
                clip: item.clip,
                flags: u32::from(item.receive_lights),
                _pad: [0; 3],
            }
        }));
        unsafe {
            let ptr = context
                .device
                .map_memory(
                    self.instance_memory,
                    0,
                    bytes as vk::DeviceSize,
                    vk::MemoryMapFlags::empty(),
                )
                .context("SpritePass: map instance memory")?;
            std::ptr::copy_nonoverlapping(
                bytemuck::cast_slice(&self.scratch_instances).as_ptr(),
                ptr as *mut u8,
                bytes,
            );
            context.device.unmap_memory(self.instance_memory);
        }
        Ok(())
    }

    /// Release GPU resources before swapchain rebuild / destroy.
    pub fn destroy(&mut self, device: &ash::Device) {
        self.pipelines.clear();
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
            if self.light_buffer != vk::Buffer::null() {
                device.destroy_buffer(self.light_buffer, None);
                self.light_buffer = vk::Buffer::null();
            }
            if self.light_memory != vk::DeviceMemory::null() {
                device.free_memory(self.light_memory, None);
                self.light_memory = vk::DeviceMemory::null();
            }
            if self.light_pool != vk::DescriptorPool::null() {
                device.destroy_descriptor_pool(self.light_pool, None);
                self.light_pool = vk::DescriptorPool::null();
            }
            if self.light_layout != vk::DescriptorSetLayout::null() {
                device.destroy_descriptor_set_layout(self.light_layout, None);
                self.light_layout = vk::DescriptorSetLayout::null();
            }
            self.light_set = vk::DescriptorSet::null();
        }
        self.instance_capacity = 0;
        self.device = None;
    }

    /// Lazily create 2D lighting resources (set 1 = `StructuredBuffer<SpriteLight>`, #611).
    ///
    /// `SpritePass::new()` has no `VulkanContext`, so first `draw` creates;
    /// capacity fixed at [`SPRITE_LIGHT_MAX`] (16 × 48 = 768 bytes), no growth needed.
    fn ensure_lights(&mut self, device: &ash::Device, context: &VulkanContext) -> Result<()> {
        if self.light_set != vk::DescriptorSet::null() {
            return Ok(());
        }
        self.device = Some(device.clone());

        let binding = vk::DescriptorSetLayoutBinding::default()
            .binding(bindings::SPRITE_LIGHTS_BINDING)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT);
        let layout_info =
            vk::DescriptorSetLayoutCreateInfo::default().bindings(std::slice::from_ref(&binding));
        let layout = unsafe { device.create_descriptor_set_layout(&layout_info, None) }
            .context("SpritePass: light descriptor set layout")?;

        let pool_size = vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1);
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .pool_sizes(std::slice::from_ref(&pool_size))
            .max_sets(1);
        let pool = unsafe { device.create_descriptor_pool(&pool_info, None) }
            .context("SpritePass: light descriptor pool")?;

        let alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool)
            .set_layouts(std::slice::from_ref(&layout));
        let sets = unsafe { device.allocate_descriptor_sets(&alloc_info) }
            .context("SpritePass: allocate light descriptor set")?;
        let set = sets[0];

        let (buffer, memory) = create_buffer(
            context,
            (SPRITE_LIGHT_MAX * size_of::<SpriteLight>()) as vk::DeviceSize,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .context("SpritePass: light storage buffer")?;

        let buffer_info = vk::DescriptorBufferInfo::default()
            .buffer(buffer)
            .offset(0)
            .range(vk::WHOLE_SIZE);
        let write = vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(bindings::SPRITE_LIGHTS_BINDING)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&buffer_info));
        unsafe { device.update_descriptor_sets(std::slice::from_ref(&write), &[]) };

        self.light_layout = layout;
        self.light_pool = pool;
        self.light_set = set;
        self.light_buffer = buffer;
        self.light_memory = memory;
        Ok(())
    }

    /// Write this frame lights into host-visible SSBO (max [`SPRITE_LIGHT_MAX`]).
    fn upload_lights(&self, device: &ash::Device, lights: &[SpriteLight]) -> Result<()> {
        let count = lights.len().min(SPRITE_LIGHT_MAX);
        if count == 0 || self.light_memory == vk::DeviceMemory::null() {
            return Ok(());
        }
        unsafe {
            let ptr = device
                .map_memory(
                    self.light_memory,
                    0,
                    vk::WHOLE_SIZE,
                    vk::MemoryMapFlags::empty(),
                )
                .context("SpritePass: map light buffer")?;
            std::ptr::copy_nonoverlapping(lights.as_ptr(), ptr as *mut SpriteLight, count);
            device.unmap_memory(self.light_memory);
        }
        Ok(())
    }
}

impl Drop for SpritePass {
    fn drop(&mut self) {
        if let Some(device) = self.device.take() {
            self.pipelines.clear();
            unsafe {
                if self.instance_buffer != vk::Buffer::null() {
                    device.destroy_buffer(self.instance_buffer, None);
                }
                if self.instance_memory != vk::DeviceMemory::null() {
                    device.free_memory(self.instance_memory, None);
                }
                if self.light_buffer != vk::Buffer::null() {
                    device.destroy_buffer(self.light_buffer, None);
                }
                if self.light_memory != vk::DeviceMemory::null() {
                    device.free_memory(self.light_memory, None);
                }
                if self.light_pool != vk::DescriptorPool::null() {
                    device.destroy_descriptor_pool(self.light_pool, None);
                }
                if self.light_layout != vk::DescriptorSetLayout::null() {
                    device.destroy_descriptor_set_layout(self.light_layout, None);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{corners_from_model, SpriteInstance};

    /// #604: corners expanded by model matrix, must match old vertex shader bit-by-bit
    /// (`m0*x + m1*y + m3`, order BL, BR, TR, TL).
    #[test]
    fn model_corners_match_the_old_vertex_shader() {
        let model = [
            [4.0, 0.0, 0.0, 0.0],
            [0.0, 2.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [10.0, 20.0, 0.0, 1.0],
        ];
        let corners = corners_from_model(&model);
        assert_eq!(corners[0], [8.0, 19.0, 0.0]);
        assert_eq!(corners[1], [12.0, 19.0, 0.0]);
        assert_eq!(corners[2], [12.0, 21.0, 0.0]);
        assert_eq!(corners[3], [8.0, 21.0, 0.0]);
    }

    /// Vertex layout is manual contract between Rust and `sprites.slang`, change one must change other.
    #[test]
    fn the_instance_layout_matches_the_shader_locations() {
        assert_eq!(std::mem::size_of::<SpriteInstance>(), 116);
        let attrs = SpriteInstance::attribute_descriptions();
        let locations: Vec<u32> = attrs.iter().map(|attr| attr.location).collect();
        assert_eq!(locations, vec![0, 1, 2, 3, 4, 5, 6, 7, 8]);
        let offsets: Vec<u32> = attrs.iter().map(|attr| attr.offset).collect();
        assert_eq!(offsets, vec![0, 12, 24, 36, 48, 64, 80, 96, 112]);
    }
}
