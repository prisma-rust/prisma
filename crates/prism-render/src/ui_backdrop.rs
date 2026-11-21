//! Backdrop filter pass for the UI overlay.
//!
//! When a UI surface carries a material that samples the backdrop, the
//! composited frame is downsampled into a 4-level Dual-Kawase blur pyramid
//! (`1/2`, `1/4`, `1/8`, `1/16` of the swapchain extent). The UI overlay samples
//! the pyramid at a mip LOD derived from each quad's `backdrop_sigma`, so
//! different glass panels on the same layer can request different blur radii
//! from a single capture.

use anyhow::Context as _;
use ash::vk;

use crate::buffer::{find_memory_type, MemoryProperties};
use crate::context::VulkanContext;
use crate::shader_bindings::ui_backdrop as bindings;

const _: () =
    assert!(std::mem::size_of::<bindings::UiBackdropPush>() as u32 == bindings::PUSH_CONSTANT_SIZE);

/// Number of mip levels in the blur pyramid (`1/2`, `1/4`, `1/8`, `1/16`).
pub const BACKDROP_MIP_LEVELS: usize = 4;

/// Compute the pixel extent of pyramid mip `level` (`0` = half swapchain).
fn mip_extent(base: vk::Extent2D, level: usize) -> vk::Extent2D {
    let shift = (level + 1) as u32;
    vk::Extent2D {
        width: (base.width >> shift).max(1),
        height: (base.height >> shift).max(1),
    }
}

/// A 4-level mipmapped color image plus per-mip views, framebuffers, and layouts.
struct Pyramid {
    image: vk::Image,
    memory: vk::DeviceMemory,
    full_view: vk::ImageView,
    mip_views: [vk::ImageView; BACKDROP_MIP_LEVELS],
    framebuffers: [vk::Framebuffer; BACKDROP_MIP_LEVELS],
    mip_extents: [vk::Extent2D; BACKDROP_MIP_LEVELS],
    mip_layouts: [vk::ImageLayout; BACKDROP_MIP_LEVELS],
    source_extent: vk::Extent2D,
}

/// Dual-Kawase mip-pyramid blur over the composited frame.
pub struct UiBackdropPass {
    downsample_render_pass: vk::RenderPass,
    upsample_render_pass: vk::RenderPass,
    pyramid: Pyramid,
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    /// `sets[i]` binds `pyramid.mip_views[i]` as `srcTex`.
    sets: [vk::DescriptorSet; BACKDROP_MIP_LEVELS],
    sampler: vk::Sampler,
    downsample_pipeline: Option<vk::Pipeline>,
    upsample_pipeline: Option<vk::Pipeline>,
    pipeline_layout: Option<vk::PipelineLayout>,
    format: vk::Format,
    device: ash::Device,
}

impl UiBackdropPass {
    /// Create the blur pyramid sized for `extent` and `format`.
    pub fn new(
        context: &VulkanContext,
        extent: vk::Extent2D,
        format: vk::Format,
    ) -> anyhow::Result<Self> {
        let device = context.device.clone();
        let downsample_render_pass =
            Self::create_render_pass(&device, format, vk::AttachmentLoadOp::DONT_CARE)?;
        let upsample_render_pass =
            Self::create_render_pass(&device, format, vk::AttachmentLoadOp::LOAD)?;
        let pyramid = Self::create_pyramid(context, extent, format, downsample_render_pass)?;

        let sampler = unsafe {
            device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .min_lod(0.0)
                    .max_lod(1.0),
                None,
            )
        }
        .context("UiBackdropPass: create sampler")?;

        let binding = vk::DescriptorSetLayoutBinding::default()
            .binding(bindings::SRC_TEX_BINDING)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT);
        let set_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default()
                    .bindings(std::slice::from_ref(&binding)),
                None,
            )
        }
        .context("UiBackdropPass: descriptor set layout")?;

        let pool_sizes = [vk::DescriptorPoolSize {
            ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
            descriptor_count: BACKDROP_MIP_LEVELS as u32,
        }];
        let pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(BACKDROP_MIP_LEVELS as u32)
                    .pool_sizes(&pool_sizes),
                None,
            )
        }
        .context("UiBackdropPass: descriptor pool")?;
        let layouts = [set_layout; BACKDROP_MIP_LEVELS];
        let allocated = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(pool)
                    .set_layouts(&layouts),
            )
        }
        .context("UiBackdropPass: allocate descriptor sets")?;
        let sets: [vk::DescriptorSet; BACKDROP_MIP_LEVELS] = allocated
            .try_into()
            .map_err(|_| anyhow::anyhow!("UiBackdropPass: descriptor set count mismatch"))?;

        let mut pass = Self {
            downsample_render_pass,
            upsample_render_pass,
            pyramid,
            set_layout,
            pool,
            sets,
            sampler,
            downsample_pipeline: None,
            upsample_pipeline: None,
            pipeline_layout: None,
            format,
            device,
        };
        pass.write_descriptors();
        Ok(pass)
    }

    /// The full-mip-chain view the UI overlay samples with `SampleLevel(uv, lod)`.
    pub fn blurred_view(&self) -> vk::ImageView {
        self.pyramid.full_view
    }

    /// Ensure every mip level is in `SHADER_READ_ONLY_OPTIMAL` before any
    /// earlier UI render pass binds the pyramid descriptor.
    pub fn ensure_readable_layout(&mut self, context: &VulkanContext, cmd: vk::CommandBuffer) {
        for mip in 0..BACKDROP_MIP_LEVELS {
            if self.pyramid.mip_layouts[mip] == vk::ImageLayout::UNDEFINED {
                Self::transition_mip(
                    context,
                    cmd,
                    &mut self.pyramid,
                    mip,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                );
            }
        }
    }

    /// Recreate the pyramid after a swapchain resize or format change.
    pub fn resize(
        &mut self,
        context: &VulkanContext,
        extent: vk::Extent2D,
        format: vk::Format,
    ) -> anyhow::Result<()> {
        if self.pyramid.source_extent == extent && self.format == format {
            return Ok(());
        }
        if self.format != format {
            let new_down =
                Self::create_render_pass(&self.device, format, vk::AttachmentLoadOp::DONT_CARE)?;
            let new_up =
                Self::create_render_pass(&self.device, format, vk::AttachmentLoadOp::LOAD)?;
            unsafe {
                self.device.destroy_render_pass(
                    std::mem::replace(&mut self.downsample_render_pass, new_down),
                    None,
                );
                self.device.destroy_render_pass(
                    std::mem::replace(&mut self.upsample_render_pass, new_up),
                    None,
                );
                if let Some(p) = self.downsample_pipeline.take() {
                    self.device.destroy_pipeline(p, None);
                }
                if let Some(p) = self.upsample_pipeline.take() {
                    self.device.destroy_pipeline(p, None);
                }
                if let Some(l) = self.pipeline_layout.take() {
                    self.device.destroy_pipeline_layout(l, None);
                }
            }
            self.format = format;
        }
        let new_pyramid =
            Self::create_pyramid(context, extent, format, self.downsample_render_pass)?;
        let old = std::mem::replace(&mut self.pyramid, new_pyramid);
        Self::destroy_pyramid(&self.device, &old);
        self.write_descriptors();
        Ok(())
    }

    /// Downsample and blur `scene_image` into the 4-level mip pyramid.
    ///
    /// `scene_image` must be in `TRANSFER_SRC_OPTIMAL` on entry and is left
    /// there. All mip levels of the pyramid end in `SHADER_READ_ONLY_OPTIMAL`.
    pub fn execute(
        &mut self,
        context: &VulkanContext,
        cmd: vk::CommandBuffer,
        scene_image: vk::Image,
        _sigma: f32,
    ) -> anyhow::Result<()> {
        self.ensure_pipelines(context)?;
        self.blit_scene_to_mip0(context, cmd, scene_image);

        // Downsample: mip 0 -> mip 1 -> mip 2 -> mip 3.
        for dst_mip in 1..BACKDROP_MIP_LEVELS {
            let src_mip = dst_mip - 1;
            let src_ext = self.pyramid.mip_extents[src_mip];
            let dst_ext = self.pyramid.mip_extents[dst_mip];
            Self::transition_mip(
                context,
                cmd,
                &mut self.pyramid,
                dst_mip,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            );
            self.draw(
                context,
                cmd,
                self.downsample_pipeline.unwrap(),
                self.downsample_render_pass,
                src_mip,
                self.pyramid.framebuffers[dst_mip],
                bindings::UiBackdropPush {
                    texel_step: [
                        1.0 / src_ext.width.max(1) as f32,
                        1.0 / src_ext.height.max(1) as f32,
                    ],
                    sigma: 0.0,
                    taps: 0,
                },
                dst_ext,
            )?;
            Self::transition_mip(
                context,
                cmd,
                &mut self.pyramid,
                dst_mip,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            );
        }

        // Upsample: mip 3 -> mip 2 -> mip 1 -> mip 0 (50% tent blend over each mip).
        for dst_mip in (0..BACKDROP_MIP_LEVELS - 1).rev() {
            let src_mip = dst_mip + 1;
            let src_ext = self.pyramid.mip_extents[src_mip];
            let dst_ext = self.pyramid.mip_extents[dst_mip];
            Self::transition_mip(
                context,
                cmd,
                &mut self.pyramid,
                dst_mip,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            );
            self.draw(
                context,
                cmd,
                self.upsample_pipeline.unwrap(),
                self.upsample_render_pass,
                src_mip,
                self.pyramid.framebuffers[dst_mip],
                bindings::UiBackdropPush {
                    texel_step: [
                        1.0 / src_ext.width.max(1) as f32,
                        1.0 / src_ext.height.max(1) as f32,
                    ],
                    sigma: 0.0,
                    taps: 1,
                },
                dst_ext,
            )?;
            Self::transition_mip(
                context,
                cmd,
                &mut self.pyramid,
                dst_mip,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            );
        }

        Ok(())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "draw helper binds pipeline, render pass, source set, framebuffer, push constants, and extent"
    )]
    fn draw(
        &self,
        context: &VulkanContext,
        cmd: vk::CommandBuffer,
        pipeline: vk::Pipeline,
        render_pass: vk::RenderPass,
        set_index: usize,
        framebuffer: vk::Framebuffer,
        push: bindings::UiBackdropPush,
        extent: vk::Extent2D,
    ) -> anyhow::Result<()> {
        let layout = self
            .pipeline_layout
            .context("UiBackdropPass: pipeline layout missing")?;
        let device = &context.device;
        let begin = vk::RenderPassBeginInfo::default()
            .render_pass(render_pass)
            .framebuffer(framebuffer)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent,
            });
        unsafe {
            device.cmd_begin_render_pass(cmd, &begin, vk::SubpassContents::INLINE);
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                layout,
                0,
                std::slice::from_ref(&self.sets[set_index]),
                &[],
            );
            device.cmd_push_constants(
                cmd,
                layout,
                vk::ShaderStageFlags::FRAGMENT,
                0,
                bytemuck::bytes_of(&push),
            );
            device.cmd_set_viewport(
                cmd,
                0,
                &[vk::Viewport {
                    x: 0.0,
                    y: 0.0,
                    width: extent.width as f32,
                    height: extent.height as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                }],
            );
            device.cmd_set_scissor(
                cmd,
                0,
                &[vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent,
                }],
            );
            device.cmd_draw(cmd, 3, 1, 0, 0);
            device.cmd_end_render_pass(cmd);
        }
        Ok(())
    }

    fn blit_scene_to_mip0(
        &mut self,
        context: &VulkanContext,
        cmd: vk::CommandBuffer,
        scene_image: vk::Image,
    ) {
        let src_ext = self.pyramid.source_extent;
        let dst_ext = self.pyramid.mip_extents[0];
        let device = &context.device;

        let scene_barrier = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(scene_image)
            .subresource_range(mip_range(0, 1))
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ);

        Self::transition_mip(
            context,
            cmd,
            &mut self.pyramid,
            0,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        );
        unsafe {
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                std::slice::from_ref(&scene_barrier),
            );
            let subresource = vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            };
            device.cmd_blit_image(
                cmd,
                scene_image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                self.pyramid.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[vk::ImageBlit {
                    src_subresource: subresource,
                    src_offsets: [
                        vk::Offset3D { x: 0, y: 0, z: 0 },
                        vk::Offset3D {
                            x: src_ext.width as i32,
                            y: src_ext.height as i32,
                            z: 1,
                        },
                    ],
                    dst_subresource: subresource,
                    dst_offsets: [
                        vk::Offset3D { x: 0, y: 0, z: 0 },
                        vk::Offset3D {
                            x: dst_ext.width as i32,
                            y: dst_ext.height as i32,
                            z: 1,
                        },
                    ],
                }],
                vk::Filter::LINEAR,
            );
        }
        Self::transition_mip(
            context,
            cmd,
            &mut self.pyramid,
            0,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );
    }

    fn write_descriptors(&mut self) {
        for (set_index, view) in self.pyramid.mip_views.iter().enumerate() {
            let image_info = vk::DescriptorImageInfo::default()
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .image_view(*view)
                .sampler(self.sampler);
            let write = vk::WriteDescriptorSet::default()
                .dst_set(self.sets[set_index])
                .dst_binding(bindings::SRC_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(std::slice::from_ref(&image_info));
            unsafe {
                self.device
                    .update_descriptor_sets(std::slice::from_ref(&write), &[]);
            }
        }
    }

    fn ensure_pipelines(&mut self, context: &VulkanContext) -> anyhow::Result<()> {
        if self.downsample_pipeline.is_some() && self.upsample_pipeline.is_some() {
            return Ok(());
        }
        const VERT_SPV: &[u8] = include_bytes!("../../../assets/shaders/ui_backdrop.vert.spv");
        const FRAG_SPV: &[u8] = include_bytes!("../../../assets/shaders/ui_backdrop.frag.spv");
        let vert_module = crate::shader::load_shader_module(&context.device, VERT_SPV)
            .context("UiBackdropPass: load vert")?;
        let frag_module = crate::shader::load_shader_module(&context.device, FRAG_SPV)
            .context("UiBackdropPass: load frag")?;
        let vert_entry = std::ffi::CString::new(bindings::ENTRY_VERTEX_MAIN).unwrap();
        let frag_entry = std::ffi::CString::new(bindings::ENTRY_FRAGMENT_MAIN).unwrap();
        let stages = [
            crate::shader::shader_stage(
                vk::ShaderStageFlags::VERTEX,
                vert_module,
                vert_entry.as_c_str(),
            ),
            crate::shader::shader_stage(
                vk::ShaderStageFlags::FRAGMENT,
                frag_module,
                frag_entry.as_c_str(),
            ),
        ];

        let push_range = vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(bindings::PUSH_CONSTANT_SIZE);
        let pipeline_layout = unsafe {
            context.device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(std::slice::from_ref(&self.set_layout))
                    .push_constant_ranges(std::slice::from_ref(&push_range)),
                None,
            )
        }
        .context("UiBackdropPass: pipeline layout")?;

        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic_state =
            vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::CLOCKWISE)
            .line_width(1.0);
        let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);

        let build_pipeline = |render_pass: vk::RenderPass,
                              blend_attachment: vk::PipelineColorBlendAttachmentState|
         -> anyhow::Result<vk::Pipeline> {
            let color_blend = vk::PipelineColorBlendStateCreateInfo::default()
                .attachments(std::slice::from_ref(&blend_attachment));
            let info = vk::GraphicsPipelineCreateInfo::default()
                .stages(&stages)
                .vertex_input_state(&vertex_input)
                .input_assembly_state(&input_assembly)
                .viewport_state(&viewport_state)
                .rasterization_state(&rasterizer)
                .multisample_state(&multisampling)
                .color_blend_state(&color_blend)
                .dynamic_state(&dynamic_state)
                .layout(pipeline_layout)
                .render_pass(render_pass)
                .subpass(0);
            let pipelines = unsafe {
                context.device.create_graphics_pipelines(
                    vk::PipelineCache::null(),
                    std::slice::from_ref(&info),
                    None,
                )
            }
            .map_err(|(_, err)| err)
            .context("UiBackdropPass: graphics pipeline")?;
            Ok(pipelines[0])
        };

        let down_blend = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(false);
        let up_blend = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
            .alpha_blend_op(vk::BlendOp::ADD);

        let down_pipe = build_pipeline(self.downsample_render_pass, down_blend);
        let up_pipe = down_pipe
            .as_ref()
            .map_or(Err(anyhow::anyhow!("skipped")), |_| {
                build_pipeline(self.upsample_render_pass, up_blend)
            });

        unsafe {
            context.device.destroy_shader_module(vert_module, None);
            context.device.destroy_shader_module(frag_module, None);
        }

        match (down_pipe, up_pipe) {
            (Ok(d), Ok(u)) => {
                self.pipeline_layout = Some(pipeline_layout);
                self.downsample_pipeline = Some(d);
                self.upsample_pipeline = Some(u);
                Ok(())
            }
            (d, u) => {
                unsafe {
                    if let Ok(p) = d {
                        context.device.destroy_pipeline(p, None);
                    }
                    if let Ok(p) = u {
                        context.device.destroy_pipeline(p, None);
                    }
                    context
                        .device
                        .destroy_pipeline_layout(pipeline_layout, None);
                }
                d.and(u).map(|_| ())
            }
        }
    }

    fn transition_mip(
        context: &VulkanContext,
        cmd: vk::CommandBuffer,
        pyramid: &mut Pyramid,
        mip: usize,
        new_layout: vk::ImageLayout,
    ) {
        let old_layout = std::mem::replace(&mut pyramid.mip_layouts[mip], new_layout);
        use vk::{AccessFlags as A, ImageLayout as L, PipelineStageFlags as S};
        let (src_stage, src, dst_stage, dst) = match (old_layout, new_layout) {
            (L::COLOR_ATTACHMENT_OPTIMAL, L::SHADER_READ_ONLY_OPTIMAL) => (
                S::COLOR_ATTACHMENT_OUTPUT,
                A::COLOR_ATTACHMENT_WRITE,
                S::FRAGMENT_SHADER,
                A::SHADER_READ,
            ),
            (L::TRANSFER_DST_OPTIMAL, L::SHADER_READ_ONLY_OPTIMAL) => (
                S::TRANSFER,
                A::TRANSFER_WRITE,
                S::FRAGMENT_SHADER,
                A::SHADER_READ,
            ),
            (L::SHADER_READ_ONLY_OPTIMAL, L::COLOR_ATTACHMENT_OPTIMAL) => (
                S::FRAGMENT_SHADER,
                A::SHADER_READ,
                S::COLOR_ATTACHMENT_OUTPUT,
                A::COLOR_ATTACHMENT_WRITE | A::COLOR_ATTACHMENT_READ,
            ),
            (L::SHADER_READ_ONLY_OPTIMAL, L::TRANSFER_DST_OPTIMAL) => (
                S::FRAGMENT_SHADER,
                A::SHADER_READ,
                S::TRANSFER,
                A::TRANSFER_WRITE,
            ),
            (L::UNDEFINED, _) => (
                S::TOP_OF_PIPE,
                A::empty(),
                match new_layout {
                    L::COLOR_ATTACHMENT_OPTIMAL => S::COLOR_ATTACHMENT_OUTPUT,
                    L::SHADER_READ_ONLY_OPTIMAL => S::FRAGMENT_SHADER,
                    _ => S::TRANSFER,
                },
                match new_layout {
                    L::COLOR_ATTACHMENT_OPTIMAL => A::COLOR_ATTACHMENT_WRITE,
                    L::SHADER_READ_ONLY_OPTIMAL => A::SHADER_READ,
                    _ => A::TRANSFER_WRITE,
                },
            ),
            _ => (
                S::ALL_COMMANDS,
                A::MEMORY_WRITE,
                S::ALL_COMMANDS,
                A::MEMORY_READ | A::MEMORY_WRITE,
            ),
        };
        let barrier = vk::ImageMemoryBarrier::default()
            .old_layout(old_layout)
            .new_layout(new_layout)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(pyramid.image)
            .subresource_range(mip_range(mip as u32, 1))
            .src_access_mask(src)
            .dst_access_mask(dst);
        unsafe {
            context.device.cmd_pipeline_barrier(
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

    fn create_render_pass(
        device: &ash::Device,
        format: vk::Format,
        load_op: vk::AttachmentLoadOp,
    ) -> anyhow::Result<vk::RenderPass> {
        let attachment = vk::AttachmentDescription::default()
            .format(format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(load_op)
            .store_op(vk::AttachmentStoreOp::STORE)
            .initial_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        let color_ref = vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(std::slice::from_ref(&color_ref));
        let dependency = vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::TRANSFER)
            .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE | vk::AccessFlags::COLOR_ATTACHMENT_READ,
            );
        unsafe {
            device.create_render_pass(
                &vk::RenderPassCreateInfo::default()
                    .attachments(std::slice::from_ref(&attachment))
                    .subpasses(std::slice::from_ref(&subpass))
                    .dependencies(std::slice::from_ref(&dependency)),
                None,
            )
        }
        .context("UiBackdropPass: render pass")
    }

    fn create_pyramid(
        context: &VulkanContext,
        source_extent: vk::Extent2D,
        format: vk::Format,
        render_pass: vk::RenderPass,
    ) -> anyhow::Result<Pyramid> {
        let base_extent = mip_extent(source_extent, 0);
        let image = unsafe {
            context.device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(format)
                    .extent(vk::Extent3D {
                        width: base_extent.width,
                        height: base_extent.height,
                        depth: 1,
                    })
                    .mip_levels(BACKDROP_MIP_LEVELS as u32)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(
                        vk::ImageUsageFlags::COLOR_ATTACHMENT
                            | vk::ImageUsageFlags::SAMPLED
                            | vk::ImageUsageFlags::TRANSFER_DST,
                    ),
                None,
            )
        }
        .context("UiBackdropPass: create pyramid image")?;

        let reqs = unsafe { context.device.get_image_memory_requirements(image) };
        let type_index = find_memory_type(
            context,
            reqs.memory_type_bits,
            MemoryProperties::DEVICE_LOCAL,
        )
        .context("UiBackdropPass: no device-local memory")?;
        let memory = unsafe {
            context.device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(reqs.size)
                    .memory_type_index(type_index),
                None,
            )
        }
        .context("UiBackdropPass: allocate pyramid memory")?;
        unsafe { context.device.bind_image_memory(image, memory, 0) }
            .context("UiBackdropPass: bind pyramid memory")?;

        let full_view = unsafe {
            context.device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(format)
                    .subresource_range(mip_range(0, BACKDROP_MIP_LEVELS as u32)),
                None,
            )
        }
        .context("UiBackdropPass: create full pyramid view")?;

        let mut mip_views = [vk::ImageView::null(); BACKDROP_MIP_LEVELS];
        let mut framebuffers = [vk::Framebuffer::null(); BACKDROP_MIP_LEVELS];
        let mut mip_extents = [vk::Extent2D::default(); BACKDROP_MIP_LEVELS];

        for i in 0..BACKDROP_MIP_LEVELS {
            let ext = mip_extent(source_extent, i);
            mip_extents[i] = ext;
            mip_views[i] = unsafe {
                context.device.create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(format)
                        .subresource_range(mip_range(i as u32, 1)),
                    None,
                )
            }
            .context("UiBackdropPass: create mip view")?;
            framebuffers[i] = unsafe {
                context.device.create_framebuffer(
                    &vk::FramebufferCreateInfo::default()
                        .render_pass(render_pass)
                        .attachments(std::slice::from_ref(&mip_views[i]))
                        .width(ext.width)
                        .height(ext.height)
                        .layers(1),
                    None,
                )
            }
            .context("UiBackdropPass: create mip framebuffer")?;
        }

        Ok(Pyramid {
            image,
            memory,
            full_view,
            mip_views,
            framebuffers,
            mip_extents,
            mip_layouts: [vk::ImageLayout::UNDEFINED; BACKDROP_MIP_LEVELS],
            source_extent,
        })
    }

    fn destroy_pyramid(device: &ash::Device, pyramid: &Pyramid) {
        unsafe {
            for fb in pyramid.framebuffers {
                device.destroy_framebuffer(fb, None);
            }
            for view in pyramid.mip_views {
                device.destroy_image_view(view, None);
            }
            device.destroy_image_view(pyramid.full_view, None);
            device.destroy_image(pyramid.image, None);
            device.free_memory(pyramid.memory, None);
        }
    }
}

impl Drop for UiBackdropPass {
    fn drop(&mut self) {
        unsafe {
            if let Some(p) = self.downsample_pipeline.take() {
                self.device.destroy_pipeline(p, None);
            }
            if let Some(p) = self.upsample_pipeline.take() {
                self.device.destroy_pipeline(p, None);
            }
            if let Some(l) = self.pipeline_layout.take() {
                self.device.destroy_pipeline_layout(l, None);
            }
            self.device.destroy_sampler(self.sampler, None);
            self.device.destroy_descriptor_pool(self.pool, None);
            self.device
                .destroy_descriptor_set_layout(self.set_layout, None);
            Self::destroy_pyramid(&self.device, &self.pyramid);
            self.device
                .destroy_render_pass(self.downsample_render_pass, None);
            self.device
                .destroy_render_pass(self.upsample_render_pass, None);
        }
    }
}

fn mip_range(base_mip_level: u32, level_count: u32) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level,
        level_count,
        base_array_layer: 0,
        layer_count: 1,
    }
}
