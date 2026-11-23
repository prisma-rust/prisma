//! RenderTexture consumer example pass: full-screen sample (bindless) → swapchain.
//!
//! Demonstrates engine-style RT consumption: take bindless slot published by scheduler from
//! [`crate::rt_scheduler::RT_OUTPUT_H`] into push constant, shader directly samples from `bindlessSrvs[]`
//! — no different from sampling a normal texture. RTs rendered uniformly by [`crate::rt_scheduler::RenderTextureScheduler`]
//! this pass only reads not writes, zero coupling with scene.

use anyhow::Context as _;
use ash::vk;
use std::ffi::CString;

use crate::context::VulkanContext;
use crate::pipeline::{GraphicsPipeline, PipelineDesc};
use crate::render_graph::{
    GraphResources, PassInfo, PassKind, RenderContext, RenderGraphBuilder, RenderPassNode,
    RenderSettings, ResourceUsage,
};
use crate::rt_scheduler::RT_OUTPUT_H;
use crate::shader::{load_shader_module, shader_stage};
use crate::shader_bindings::rt_preview::{
    RtPreviewPush, ENTRY_FRAGMENT_MAIN as PREVIEW_FRAG, ENTRY_VERTEX_MAIN as PREVIEW_VERT,
};

const RT_PREVIEW_VERT_SPV: &[u8] = prism_render_shaders::RT_PREVIEW_VERT_SPV;
const RT_PREVIEW_FRAG_SPV: &[u8] = prism_render_shaders::RT_PREVIEW_FRAG_SPV;

/// Consumer pass that full-screen samples RenderTexture (bindless) → swapchain.
pub struct RtPreviewPass {
    /// Cloned device (destroyed on Drop).
    device: Option<ash::Device>,
    color_format: vk::Format,
    /// Bindless table layout (set 0 for this pass), injected by GraphRenderer (for pipeline creation).
    bindless_layout: Option<vk::DescriptorSetLayout>,
    /// Bindless table set (bind set 0 at execute).
    bindless_set: Option<vk::DescriptorSet>,
    render_pass: Option<vk::RenderPass>,
    /// Per-swapchain-image framebuffers (reusing post/ui_overlay mode).
    framebuffers: Vec<Option<vk::Framebuffer>>,
    target_views: Vec<vk::ImageView>,
    extent: vk::Extent2D,
    pipeline: Option<GraphicsPipeline>,
}

impl RtPreviewPass {
    pub fn new(context: &VulkanContext, color_format: vk::Format) -> Self {
        Self {
            device: Some(context.device.clone()),
            color_format,
            bindless_layout: None,
            bindless_set: None,
            render_pass: None,
            framebuffers: Vec::new(),
            target_views: Vec::new(),
            extent: vk::Extent2D {
                width: 0,
                height: 0,
            },
            pipeline: None,
        }
    }

    /// Inject bindless table layout (called after GraphRenderer builds graph).
    pub fn set_bindless_layout(&mut self, layout: vk::DescriptorSetLayout) {
        self.bindless_layout = Some(layout);
    }

    /// Inject bindless table set (bind set 0 at execute).
    pub fn set_bindless_set(&mut self, set: vk::DescriptorSet) {
        self.bindless_set = Some(set);
    }

    fn ensure_render_pass(&mut self, device: &ash::Device) -> anyhow::Result<()> {
        if self.render_pass.is_none() {
            let color_attachment = vk::AttachmentDescription::default()
                .format(self.color_format)
                .samples(vk::SampleCountFlags::TYPE_1)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);
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
                .context("RtPreviewPass: create render pass")?;
            self.render_pass = Some(rp);
        }
        Ok(())
    }

    fn ensure_pipeline(&mut self, device: &ash::Device) -> anyhow::Result<()> {
        if self.pipeline.is_none() {
            let layout = self
                .bindless_layout
                .context("RtPreviewPass: set_bindless_layout not called")?;
            let rp = self.render_pass.context("render pass missing")?;

            let vert_module = load_shader_module(device, RT_PREVIEW_VERT_SPV)
                .context("RtPreviewPass: load vert module")?;
            let frag_module = load_shader_module(device, RT_PREVIEW_FRAG_SPV)
                .context("RtPreviewPass: load frag module")?;
            let vert_entry = CString::new(PREVIEW_VERT).unwrap();
            let frag_entry = CString::new(PREVIEW_FRAG).unwrap();
            let stages = [
                shader_stage(vk::ShaderStageFlags::VERTEX, vert_module, &vert_entry),
                shader_stage(vk::ShaderStageFlags::FRAGMENT, frag_module, &frag_entry),
            ];

            let push = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                .offset(0)
                .size(std::mem::size_of::<RtPreviewPush>() as u32)];

            let pipeline = GraphicsPipeline::new(&PipelineDesc {
                device,
                shader_stages: &stages,
                vertex_binding_desc: &[],
                vertex_attr_descs: &[],
                descriptor_set_layouts: &[layout],
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
            .context("RtPreviewPass: create pipeline")?;

            unsafe {
                device.destroy_shader_module(vert_module, None);
                device.destroy_shader_module(frag_module, None);
            }
            self.pipeline = Some(pipeline);
        }
        Ok(())
    }

    /// Ensure per-swapchain framebuffers match current swapchain views + extent.
    fn ensure_framebuffers(
        &mut self,
        device: &ash::Device,
        swapchain_views: &[vk::ImageView],
        extent: vk::Extent2D,
    ) -> anyhow::Result<()> {
        if self.target_views == swapchain_views
            && self.extent == extent
            && !self.framebuffers.is_empty()
        {
            return Ok(());
        }
        for fb in self.framebuffers.drain(..).flatten() {
            unsafe { device.destroy_framebuffer(fb, None) };
        }
        self.framebuffers = Vec::with_capacity(swapchain_views.len());
        let rp = self.render_pass.context("render pass missing")?;
        for &view in swapchain_views {
            let attachments = [view];
            let fb = unsafe {
                device.create_framebuffer(
                    &vk::FramebufferCreateInfo::default()
                        .render_pass(rp)
                        .attachments(&attachments)
                        .width(extent.width)
                        .height(extent.height)
                        .layers(1),
                    None,
                )
            }
            .context("RtPreviewPass: create framebuffer")?;
            self.framebuffers.push(Some(fb));
        }
        self.target_views = swapchain_views.to_vec();
        self.extent = extent;
        Ok(())
    }
}

impl RenderPassNode for RtPreviewPass {
    fn name(&self) -> &str {
        "RtPreview"
    }

    fn setup(&mut self, graph: &mut RenderGraphBuilder, _settings: &RenderSettings) {
        graph.read_usage(ResourceUsage {
            handle: RT_OUTPUT_H,
            access: vk::AccessFlags::SHADER_READ,
            stage: vk::PipelineStageFlags::FRAGMENT_SHADER,
            layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        });
    }

    fn execute(
        &mut self,
        ctx: &RenderContext,
        resources: &mut GraphResources,
    ) -> anyhow::Result<()> {
        let device = ctx.device;
        let cmd = ctx.cmd;
        let extent = ctx.extent;
        let swapchain_views = ctx.frame.swapchain_views;

        self.ensure_render_pass(device)?;
        self.ensure_pipeline(device)?;
        self.ensure_framebuffers(device, swapchain_views, extent)?;

        let fb = self
            .framebuffers
            .get(ctx.image_index as usize)
            .copied()
            .flatten()
            .context("RtPreviewPass: no framebuffer for image index")?;
        let pipeline = self.pipeline.as_ref().context("pipeline missing")?;

        let slot = resources.param(RT_OUTPUT_H).unwrap_or(0);

        let clear = vk::ClearValue {
            color: vk::ClearColorValue {
                float32: [0.0, 0.0, 0.0, 1.0],
            },
        };
        let begin = vk::RenderPassBeginInfo::default()
            .render_pass(self.render_pass.expect("rp"))
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
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline.pipeline)
        };
        let bindless_set = self
            .bindless_set
            .context("RtPreviewPass: set_bindless_set not called")?;
        unsafe {
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline.layout,
                0,
                std::slice::from_ref(&bindless_set),
                &[],
            );
        }
        let push = RtPreviewPush {
            texture_handle: slot,
        };
        unsafe {
            device.cmd_push_constants(
                cmd,
                pipeline.layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                0,
                bytemuck::bytes_of(&push),
            );
        }
        unsafe { device.cmd_draw(cmd, 3, 1, 0, 0) };
        unsafe { device.cmd_end_render_pass(cmd) };

        log::trace!("RtPreview: sampled RT slot {slot} -> swapchain");
        Ok(())
    }

    fn graph_info(&self) -> PassInfo {
        PassInfo {
            index: usize::MAX,
            name: self.name().to_string(),
            kind: PassKind::Post,
            inputs: vec![RT_OUTPUT_H],
            outputs: Vec::new(),
        }
    }
}

impl Drop for RtPreviewPass {
    fn drop(&mut self) {
        if let Some(device) = self.device.take() {
            for fb in self.framebuffers.drain(..).flatten() {
                unsafe { device.destroy_framebuffer(fb, None) };
            }
            if let Some(rp) = self.render_pass.take() {
                unsafe { device.destroy_render_pass(rp, None) };
            }
        }
    }
}
