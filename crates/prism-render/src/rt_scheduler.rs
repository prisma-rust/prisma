//! Unified CRT scheduler (Custom Render Texture management system).
//!
//! Engine low-level each frame traverses all registered [`RenderTexture`], decides by config
//! ([`RtUpdateMode`] + period + bound shader) whether to execute one full-screen blit:
//! - `OnLoad`: render once on first scheduling
//! - `Realtime`: auto-render every frame (or at period interval)
//! - `OnDemand`: render only after `request_update()` flag
//!
//! Attached early in render graph as single [`RenderPassNode`] (before Scene), zero coupling with scene.
//! After render publishes each RT image/view/bindless slot to graph,
//! downstream passes (like `RtPreviewPass`) sample directly.

use anyhow::Context as _;
use ash::vk;
use std::collections::HashMap;
use std::ffi::CString;

use crate::bindless::{BindlessTextureTable, TextureHandle};
use crate::context::VulkanContext;
use crate::pipeline::{GraphicsPipeline, PipelineDesc};
use crate::render_graph::{
    GraphResources, PassInfo, PassKind, RenderContext, RenderGraphBuilder, RenderPassNode,
    RenderSettings, ResourceHandle, ResourceUsage,
};
use crate::render_texture::{RenderTexture, RtShader};
use crate::shader::{load_shader_module, shader_stage};
use crate::shader_bindings::rt_render::{
    RtRenderPush, ENTRY_FRAGMENT_MAIN as RT_FRAG, ENTRY_VERTEX_MAIN as RT_VERT,
};

const RT_RENDER_VERT_SPV: &[u8] = prism_render_shaders::RT_RENDER_VERT_SPV;
const RT_RENDER_FRAG_SPV: &[u8] = prism_render_shaders::RT_RENDER_FRAG_SPV;

/// Scheduler output handle base: i-th registered RT publishes to `RT_OUTPUT_H + i`.
/// Fixed base (same convention as SCENE_*_H) so downstream passes need not know registration order.
pub const RT_OUTPUT_H: ResourceHandle = ResourceHandle(1004);

/// Unified CRT scheduler: holds all registered RenderTextures, renders each frame by config.
pub struct RenderTextureScheduler {
    /// Cloned device (destroyed on Drop).
    device: Option<ash::Device>,
    /// Registered RT list (handle = RT_OUTPUT_H + index).
    rts: Vec<RenderTexture>,
    /// Generic offscreen render pass (one, shared by all RTs).
    render_pass: Option<vk::RenderPass>,
    /// Per-RT framebuffer (same index as `rts`; rebuilt on RT resize).
    framebuffers: Vec<Option<vk::Framebuffer>>,
    /// RT view corresponding to each framebuffer creation (for resize comparison).
    fb_views: Vec<Option<vk::ImageView>>,
    /// One pipeline per shader (cached, lazily created).
    pipelines: HashMap<RtShader, GraphicsPipeline>,
    /// Current scheduling size (from GraphRenderer, for RT resize).
    extent: vk::Extent2D,
}

impl RenderTextureScheduler {
    pub fn new(context: &VulkanContext) -> Self {
        Self {
            device: Some(context.device.clone()),
            rts: Vec::new(),
            render_pass: None,
            framebuffers: Vec::new(),
            fb_views: Vec::new(),
            pipelines: HashMap::new(),
            extent: vk::Extent2D {
                width: 0,
                height: 0,
            },
        }
    }

    /// Register an RT. Returns its output handle (`RT_OUTPUT_H + index`), downstream passes
    /// use it to sample / read. RT extent fixed at creation.
    pub fn add(&mut self, rt: RenderTexture) -> ResourceHandle {
        self.rts.push(rt);
        let handle = ResourceHandle(RT_OUTPUT_H.0 + self.rts.len() as u32 - 1);
        log::trace!("RenderTextureScheduler: registered rt -> {handle:?}");
        handle
    }

    pub fn len(&self) -> usize {
        self.rts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rts.is_empty()
    }

    /// Access RT by output handle.
    pub fn rt(&self, handle: ResourceHandle) -> Option<&RenderTexture> {
        let idx = handle.0.checked_sub(RT_OUTPUT_H.0)? as usize;
        self.rts.get(idx)
    }

    /// Mutably access RT by output handle (config update: mode / period / shader / request_update).
    pub fn rt_mut(&mut self, handle: ResourceHandle) -> Option<&mut RenderTexture> {
        let idx = handle.0.checked_sub(RT_OUTPUT_H.0)? as usize;
        self.rts.get_mut(idx)
    }

    /// Bindless handles of all RTs (for debug / external use).
    pub fn handles(&self) -> impl Iterator<Item = (ResourceHandle, TextureHandle)> + '_ {
        self.rts.iter().enumerate().map(|(i, rt)| {
            (
                ResourceHandle(RT_OUTPUT_H.0 + i as u32),
                rt.texture_handle(),
            )
        })
    }

    /// Uniformly resize all RTs (called on swapchain recreate).
    /// Bindless handles stay unchanged; framebuffer rebuilt by `drop_stale_framebuffers`
    /// on next frame execute detecting view change.
    pub fn resize_all(
        &mut self,
        context: &VulkanContext,
        bindless: &mut BindlessTextureTable,
        extent: vk::Extent2D,
    ) -> anyhow::Result<()> {
        self.extent = extent;
        for rt in self.rts.iter_mut() {
            rt.resize(context, bindless, extent)?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Internal: GPU infrastructure (lazily created)
    // ------------------------------------------------------------------

    /// Generic offscreen render pass: shared by all RTs. final_layout =
    /// SHADER_READ_ONLY_OPTIMAL for downstream sampling.
    fn ensure_render_pass(&mut self, device: &ash::Device) -> anyhow::Result<()> {
        if self.render_pass.is_none() {
            let rt = self
                .rts
                .first()
                .context("RenderTextureScheduler: no RT registered")?;
            let color_attachment = vk::AttachmentDescription::default()
                .format(rt.format())
                .samples(vk::SampleCountFlags::TYPE_1)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .initial_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                .final_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
            let color_ref = vk::AttachmentReference::default()
                .attachment(0)
                .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
            let subpass = vk::SubpassDescription::default()
                .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
                .color_attachments(std::slice::from_ref(&color_ref));
            let create_info = vk::RenderPassCreateInfo::default()
                .attachments(std::slice::from_ref(&color_attachment))
                .subpasses(std::slice::from_ref(&subpass));
            let rp = unsafe { device.create_render_pass(&create_info, None) }
                .context("RenderTextureScheduler: create render pass")?;
            self.render_pass = Some(rp);
        }
        Ok(())
    }

    /// Lazily create pipeline per shader (cached; this pass has no descriptor set).
    fn ensure_pipeline(
        &mut self,
        device: &ash::Device,
        shader: RtShader,
    ) -> anyhow::Result<vk::Pipeline> {
        if let Some(p) = self.pipelines.get(&shader) {
            return Ok(p.pipeline);
        }
        let rp = self.render_pass.context("render pass missing")?;
        let (vert_spv, frag_spv, vert_entry, frag_entry) = match shader {
            RtShader::BitmapPattern => (
                RT_RENDER_VERT_SPV,
                RT_RENDER_FRAG_SPV,
                CString::new(RT_VERT).unwrap(),
                CString::new(RT_FRAG).unwrap(),
            ),
        };
        let vert_module = load_shader_module(device, vert_spv)
            .context("RenderTextureScheduler: load vert module")?;
        let frag_module = load_shader_module(device, frag_spv)
            .context("RenderTextureScheduler: load frag module")?;
        let stages = [
            shader_stage(vk::ShaderStageFlags::VERTEX, vert_module, &vert_entry),
            shader_stage(vk::ShaderStageFlags::FRAGMENT, frag_module, &frag_entry),
        ];

        // RtRenderPush = { uint pattern } (4 bytes, no std140 tail padding).
        let push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(std::mem::size_of::<RtRenderPush>() as u32)];

        let pipeline = GraphicsPipeline::new(&PipelineDesc {
            device,
            shader_stages: &stages,
            vertex_binding_desc: &[],
            vertex_attr_descs: &[],
            descriptor_set_layouts: &[],
            push_constant_ranges: &push,
            render_pass: rp,
            subpass: 0,
            cull_mode: None,
            depth_bias_enable: None,
            depth_bias_constant_factor: None,
            depth_bias_slope_factor: None,
            depth_write_enable: None,
            depth_test_enable: None,
            color_attachment_count: Some(1),
            color_blend_attachments: None,
            front_face: None,
            polygon_mode: None,
        })
        .context("RenderTextureScheduler: create pipeline")?;

        unsafe {
            device.destroy_shader_module(vert_module, None);
            device.destroy_shader_module(frag_module, None);
        }
        Ok(self.pipelines.entry(shader).or_insert(pipeline).pipeline)
    }

    /// Ensure RT framebuffer exists (on RT resize caller first drops old one,
    /// here only responsible for creating missing ones).
    fn ensure_framebuffer(
        &mut self,
        device: &ash::Device,
        index: usize,
    ) -> anyhow::Result<vk::Framebuffer> {
        let rt = &self.rts[index];
        let rp = self.render_pass.context("render pass missing")?;
        let fb = match self.framebuffers.get(index).copied().flatten() {
            Some(fb) => fb,
            None => {
                let attachments = [rt.view()];
                let fb = unsafe {
                    device.create_framebuffer(
                        &vk::FramebufferCreateInfo::default()
                            .render_pass(rp)
                            .attachments(&attachments)
                            .width(rt.extent().width)
                            .height(rt.extent().height)
                            .layers(1),
                        None,
                    )
                }
                .context("RenderTextureScheduler: create framebuffer")?;
                if self.framebuffers.len() <= index {
                    self.framebuffers.resize(index + 1, None);
                    self.fb_views.resize(index + 1, None);
                }
                self.framebuffers[index] = Some(fb);
                self.fb_views[index] = Some(rt.view());
                fb
            }
        };
        Ok(fb)
    }

    /// Drop framebuffers referencing already-invalidated RT views (after RT resize), rebuilt next frame.
    fn drop_stale_framebuffers(&mut self, device: &ash::Device) {
        for (i, rt) in self.rts.iter().enumerate() {
            let fb_view = self.fb_views.get(i).copied().flatten();
            if fb_view.is_some() && fb_view != Some(rt.view()) {
                if let Some(fb) = self.framebuffers[i].take() {
                    unsafe { device.destroy_framebuffer(fb, None) };
                    self.fb_views[i] = None;
                }
            }
        }
    }
}

impl RenderPassNode for RenderTextureScheduler {
    fn name(&self) -> &str {
        "RenderTextures"
    }

    fn setup(&mut self, graph: &mut RenderGraphBuilder, _settings: &RenderSettings) {
        // Each RT declares a write edge: after render layout = SHADER_READ_ONLY_OPTIMAL
        // (render pass final_layout), for downstream read-edge barrier derivation. RTs are external
        // persistent resources, graph does not allocate, no create_resource_at.
        for i in 0..self.rts.len() {
            graph.write_usage(ResourceUsage {
                handle: ResourceHandle(RT_OUTPUT_H.0 + i as u32),
                access: vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                stage: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            });
        }
    }

    fn execute(
        &mut self,
        ctx: &RenderContext,
        resources: &mut GraphResources,
    ) -> anyhow::Result<()> {
        if self.rts.is_empty() {
            return Ok(());
        }
        let device = ctx.device;
        let cmd = ctx.cmd;
        self.ensure_render_pass(device)?;
        self.drop_stale_framebuffers(device);
        let rp = self.render_pass.context("render pass missing")?;
        let mut rendered = vec![false; self.rts.len()];

        // First phase: snapshot each RT render decision (shader + whether need render).
        // Next step extends execution phase needing both `&mut self` (pipeline / framebuffer creation) and
        // `&mut self.rts[i]` (pattern generation), snapshot first avoids borrow conflict.
        let plan: Vec<(Option<RtShader>, bool)> = self
            .rts
            .iter()
            .map(|rt| (rt.active_shader(), rt.needs_render()))
            .collect();

        // Second phase: execute blits by snapshot.
        for (i, &(shader, needs)) in plan.iter().enumerate() {
            if let Some(shader) = shader {
                if needs {
                    let pipeline = self.ensure_pipeline(device, shader)?;
                    let fb = self.ensure_framebuffer(device, i)?;
                    let pattern = self.rts[i].next_pattern() as u32;
                    let (image, extent) = {
                        let rt = &self.rts[i];
                        (rt.image(), rt.extent())
                    };

                    // Any old layout → COLOR_ATTACHMENT_OPTIMAL. Full-screen repaint,
                    // no content preservation needed (discard-style update, same as Realtime default).
                    let barrier = vk::ImageMemoryBarrier2::default()
                        .old_layout(vk::ImageLayout::UNDEFINED)
                        .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                        .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                        .dst_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
                        .src_access_mask(vk::AccessFlags2::MEMORY_READ)
                        .dst_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
                        .image(image)
                        .subresource_range(vk::ImageSubresourceRange {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            base_mip_level: 0,
                            level_count: 1,
                            base_array_layer: 0,
                            layer_count: 1,
                        });
                    let barriers = [barrier];
                    let dep = vk::DependencyInfo::default().image_memory_barriers(&barriers);
                    // `vkCmdPipelineBarrier2` (1.3 core) is not exported on a 1.2
                    // Instance — use the KHR wrapper like `buffer.rs` does.
                    let sync2 = crate::vulkan_compat::LegacySync2 {
                        device: &ctx.context.device,
                    };
                    unsafe { sync2.cmd_pipeline_barrier2(cmd, &dep) };

                    let clear = vk::ClearValue {
                        color: vk::ClearColorValue {
                            float32: [0.0, 0.0, 0.0, 1.0],
                        },
                    };
                    let begin = vk::RenderPassBeginInfo::default()
                        .render_pass(rp)
                        .framebuffer(fb)
                        .render_area(vk::Rect2D {
                            offset: vk::Offset2D { x: 0, y: 0 },
                            extent,
                        })
                        .clear_values(std::slice::from_ref(&clear));
                    unsafe {
                        device.cmd_begin_render_pass(cmd, &begin, vk::SubpassContents::INLINE);
                    }
                    unsafe {
                        device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
                    }
                    let push = RtRenderPush { pattern };
                    unsafe {
                        device.cmd_push_constants(
                            cmd,
                            self.pipelines[&shader].layout,
                            vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                            0,
                            bytemuck::bytes_of(&push),
                        );
                    }
                    unsafe { device.cmd_draw(cmd, 3, 1, 0, 0) };
                    unsafe { device.cmd_end_render_pass(cmd) };
                    rendered[i] = true;
                }
            }

            // Publish RT (image + view + bindless slot) for downstream sampling.
            let rt = &self.rts[i];
            resources.set_image(ResourceHandle(RT_OUTPUT_H.0 + i as u32), rt.image());
            resources.set_image_view(ResourceHandle(RT_OUTPUT_H.0 + i as u32), rt.view());
            resources.set_param(
                ResourceHandle(RT_OUTPUT_H.0 + i as u32),
                rt.texture_handle().0,
            );
        }

        // End-of-frame state advance: initialize render frame flag / clear pending, all RTs tick unconditionally
        // (counter must advance every frame, otherwise Realtime+period decision freezes).
        for (i, rt) in self.rts.iter_mut().enumerate() {
            if rendered[i] {
                rt.mark_rendered();
            }
            rt.tick();
        }
        Ok(())
    }

    fn graph_info(&self) -> PassInfo {
        PassInfo {
            index: usize::MAX,
            name: self.name().to_string(),
            kind: PassKind::Rt,
            inputs: Vec::new(),
            outputs: (0..self.rts.len())
                .map(|i| ResourceHandle(RT_OUTPUT_H.0 + i as u32))
                .collect(),
        }
    }
}

impl Drop for RenderTextureScheduler {
    fn drop(&mut self) {
        if let Some(device) = self.device.take() {
            for fb in self.framebuffers.drain(..).flatten() {
                unsafe { device.destroy_framebuffer(fb, None) };
            }
            if let Some(rp) = self.render_pass.take() {
                unsafe { device.destroy_render_pass(rp, None) };
            }
            // Pipelines released by GraphicsPipeline RAII; rts each Drop.
        }
    }
}
