//! Backdrop filter pass for the UI overlay.
//!
//! When a UI surface carries a material that samples the backdrop, the
//! composited scene is copied into an offscreen image and blurred with a
//! separable Gaussian (horizontal, then vertical). The UI overlay samples the
//! result, which is what turns an ordinary quad into frosted glass.
//!
//! Two targets with fixed roles: target 0 receives the scene copy and the
//! vertical result, target 1 receives the horizontal result. Because the
//! targets are rewritten every frame, their layouts are declared explicitly
//! instead of tracked — there is no cross-frame state to drift out of sync.

use anyhow::Context as _;
use ash::vk;

use crate::buffer::{find_memory_type, MemoryProperties};
use crate::context::VulkanContext;
use crate::shader_bindings::ui_backdrop as bindings;

const _: () =
    assert!(std::mem::size_of::<bindings::UiBackdropPush>() as u32 == bindings::PUSH_CONSTANT_SIZE);

/// Samples per side for a given sigma.
///
/// Three sigma carries essentially the whole Gaussian, and the shader caps the
/// count, so both ends of this range are deliberate.
fn taps_for_sigma(sigma: f32) -> u32 {
    (sigma * 3.0).ceil().clamp(1.0, 12.0) as u32
}

/// One offscreen colour image plus its view.
struct Target {
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
    framebuffer: vk::Framebuffer,
    extent: vk::Extent2D,
    /// Layout the image is actually in.
    ///
    /// A freshly created image is `UNDEFINED`, and every pass starts by
    /// overwriting its target, so this cannot be inferred per frame position.
    layout: vk::ImageLayout,
}

/// Separable Gaussian blur over the composited scene.
pub struct UiBackdropPass {
    render_pass: vk::RenderPass,
    /// `[0]` = scene copy and vertical result, `[1]` = horizontal result.
    targets: [Target; 2],
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    /// Horizontal pass reads target 0, vertical reads target 1.
    sets: [vk::DescriptorSet; 2],
    sampler: vk::Sampler,
    pipeline: Option<vk::Pipeline>,
    pipeline_layout: Option<vk::PipelineLayout>,
    format: vk::Format,
    device: ash::Device,
}

impl UiBackdropPass {
    /// Create the pass at `extent`; `format` must match the swapchain so the
    /// copy between them is a plain texel-for-texel blit.
    pub fn new(
        context: &VulkanContext,
        extent: vk::Extent2D,
        format: vk::Format,
    ) -> anyhow::Result<Self> {
        let device = context.device.clone();
        let render_pass = Self::create_render_pass(&device, format)?;
        let targets = [
            Self::create_target(context, extent, format, render_pass)?,
            Self::create_target(context, extent, format, render_pass)?,
        ];

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
            descriptor_count: 4,
        }];
        let pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(2)
                    .pool_sizes(&pool_sizes),
                None,
            )
        }
        .context("UiBackdropPass: descriptor pool")?;
        let layouts = [set_layout, set_layout];
        let sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(pool)
                    .set_layouts(&layouts),
            )
        }
        .context("UiBackdropPass: allocate descriptor sets")?;

        let mut pass = Self {
            render_pass,
            targets,
            set_layout,
            pool,
            sets: [sets[0], sets[1]],
            sampler,
            pipeline: None,
            pipeline_layout: None,
            format,
            device,
        };
        pass.write_descriptors();
        Ok(pass)
    }

    /// The blurred image the UI overlay samples.
    ///
    /// Fixed role: the vertical pass always writes target 1, so the descriptor
    /// that points at it never has to change after creation.
    pub fn blurred_view(&self) -> vk::ImageView {
        self.targets[1].view
    }

    /// Recreate the targets after a swapchain resize or format change.
    pub fn resize(
        &mut self,
        context: &VulkanContext,
        extent: vk::Extent2D,
        format: vk::Format,
    ) -> anyhow::Result<()> {
        if self.targets[0].extent == extent && self.format == format {
            return Ok(());
        }
        if self.format != format {
            // The copy requires identical texel sizes, so the render pass is
            // rebuilt against the new attachment format.
            let new_pass = Self::create_render_pass(&self.device, format)?;
            unsafe {
                self.device
                    .destroy_render_pass(std::mem::replace(&mut self.render_pass, new_pass), None);
            }
            self.format = format;
        }
        let new_targets = [
            Self::create_target(context, extent, format, self.render_pass)?,
            Self::create_target(context, extent, format, self.render_pass)?,
        ];
        let old = std::mem::replace(&mut self.targets, new_targets);
        for target in old {
            unsafe {
                self.device.destroy_framebuffer(target.framebuffer, None);
                self.device.destroy_image_view(target.view, None);
                self.device.destroy_image(target.image, None);
                self.device.free_memory(target.memory, None);
            }
        }
        self.write_descriptors();
        Ok(())
    }

    /// Blur `scene_image` into the target [`Self::blurred_view`] names.
    ///
    /// `scene_image` must be in `TRANSFER_SRC_OPTIMAL` on entry and is left
    /// there. The result is in `SHADER_READ_ONLY_OPTIMAL`.
    pub fn execute(
        &mut self,
        context: &VulkanContext,
        cmd: vk::CommandBuffer,
        scene_image: vk::Image,
        sigma: f32,
    ) -> anyhow::Result<()> {
        let extent = self.targets[0].extent;
        let texel = [
            1.0 / extent.width.max(1) as f32,
            1.0 / extent.height.max(1) as f32,
        ];
        let taps = taps_for_sigma(sigma);

        self.ensure_pipeline(context)?;
        self.copy_scene(context, cmd, scene_image)?;
        // Horizontal: target 0 (scene copy) -> target 1.
        Self::transition(
            context,
            cmd,
            &mut self.targets[1],
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        );
        self.draw(
            context,
            cmd,
            0,
            self.targets[1].framebuffer,
            bindings::UiBackdropPush {
                texel_step: [texel[0], 0.0],
                sigma,
                taps,
            },
            extent,
        )?;

        // Vertical: target 1 -> target 0, then the result is copied into target
        // 1, which is the fixed role the UI descriptor points at.
        Self::transition(
            context,
            cmd,
            &mut self.targets[1],
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );
        Self::transition(
            context,
            cmd,
            &mut self.targets[0],
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        );
        self.draw(
            context,
            cmd,
            1,
            self.targets[0].framebuffer,
            bindings::UiBackdropPush {
                texel_step: [0.0, texel[1]],
                sigma,
                taps,
            },
            extent,
        )?;

        // The vertical pass wrote target 0, but `blurred_view` names target 1.
        // Copy the finished blur across so the roles stay fixed.
        self.copy_result(context, cmd)
    }

    /// Copy the finished blur from target 0 into target 1.
    ///
    /// Fixed roles are what let the UI descriptor be written once per frame
    /// instead of being rewritten while a command buffer still references it.
    fn copy_result(
        &mut self,
        context: &VulkanContext,
        cmd: vk::CommandBuffer,
    ) -> anyhow::Result<()> {
        let extent = self.targets[0].extent;
        let device = &context.device;
        let regions = [vk::ImageCopy::default()
            .src_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
            .dst_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
            .extent(vk::Extent3D {
                width: extent.width,
                height: extent.height,
                depth: 1,
            })];

        Self::transition(
            context,
            cmd,
            &mut self.targets[0],
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
        );
        Self::transition(
            context,
            cmd,
            &mut self.targets[1],
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        );
        unsafe {
            device.cmd_copy_image(
                cmd,
                self.targets[0].image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                self.targets[1].image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &regions,
            );
        }
        // Target 1 is what the UI samples.
        Self::transition(
            context,
            cmd,
            &mut self.targets[1],
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );
        Ok(())
    }

    fn draw(
        &self,
        context: &VulkanContext,
        cmd: vk::CommandBuffer,
        set_index: usize,
        framebuffer: vk::Framebuffer,
        push: bindings::UiBackdropPush,
        extent: vk::Extent2D,
    ) -> anyhow::Result<()> {
        let pipeline = self.pipeline.context("UiBackdropPass: pipeline missing")?;
        let layout = self
            .pipeline_layout
            .context("UiBackdropPass: pipeline layout missing")?;
        let device = &context.device;
        let begin = vk::RenderPassBeginInfo::default()
            .render_pass(self.render_pass)
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
            // The pipeline declares viewport and scissor as dynamic state, so
            // they must be set before the draw covers the target.
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
            // Fullscreen triangle from SV_VertexID: no vertex buffer.
            device.cmd_draw(cmd, 3, 1, 0, 0);
            device.cmd_end_render_pass(cmd);
        }
        Ok(())
    }

    /// Copy the composited scene into target 0 and expose it to the fragment
    /// shader.
    fn copy_scene(
        &mut self,
        context: &VulkanContext,
        cmd: vk::CommandBuffer,
        scene_image: vk::Image,
    ) -> anyhow::Result<()> {
        let extent = self.targets[0].extent;
        let device = &context.device;
        let regions = [vk::ImageCopy::default()
            .src_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
            .dst_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
            .extent(vk::Extent3D {
                width: extent.width,
                height: extent.height,
                depth: 1,
            })];

        let barriers = [vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(scene_image)
            .subresource_range(color_range())
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ)];
        Self::transition(
            context,
            cmd,
            &mut self.targets[0],
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        );
        // Target 1 still holds last frame's result, which the UI sampled last
        // frame; make that read visible before this frame overwrites it.
        Self::transition(
            context,
            cmd,
            &mut self.targets[1],
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );
        unsafe {
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &barriers,
            );
            device.cmd_blit_image(
                cmd,
                scene_image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                self.targets[0].image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[vk::ImageBlit {
                    src_subresource: regions[0].src_subresource,
                    src_offsets: [
                        vk::Offset3D { x: 0, y: 0, z: 0 },
                        vk::Offset3D {
                            x: extent.width as i32,
                            y: extent.height as i32,
                            z: 1,
                        },
                    ],
                    dst_subresource: regions[0].dst_subresource,
                    dst_offsets: [
                        vk::Offset3D { x: 0, y: 0, z: 0 },
                        vk::Offset3D {
                            x: extent.width as i32,
                            y: extent.height as i32,
                            z: 1,
                        },
                    ],
                }],
                vk::Filter::NEAREST,
            );
        }
        // The horizontal pass samples this copy, so it becomes a texture.
        Self::transition(
            context,
            cmd,
            &mut self.targets[0],
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );
        Ok(())
    }

    fn write_descriptors(&mut self) {
        // Horizontal samples the scene copy in target 0; vertical samples the
        // horizontal result in target 1.
        let sources = [self.targets[0].view, self.targets[1].view];
        for (set_index, source) in sources.into_iter().enumerate() {
            let image_info = vk::DescriptorImageInfo::default()
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .image_view(source)
                .sampler(self.sampler);
            let write = vk::WriteDescriptorSet::default()
                .dst_set(self.sets[set_index])
                .dst_binding(bindings::SRC_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(std::slice::from_ref(&image_info));
            unsafe {
                self.device.update_descriptor_sets(&[write], &[]);
            }
        }
    }

    fn ensure_pipeline(&mut self, context: &VulkanContext) -> anyhow::Result<()> {
        if self.pipeline.is_some() {
            return Ok(());
        }
        let device = &self.device;
        let vert =
            crate::shader::load_shader_module(device, prism_render_shaders::UI_BACKDROP_VERT_SPV)
                .context("UiBackdropPass: load vertex shader")?;
        let frag =
            crate::shader::load_shader_module(device, prism_render_shaders::UI_BACKDROP_FRAG_SPV)
                .context("UiBackdropPass: load fragment shader")?;
        let vert_entry = std::ffi::CString::new(bindings::ENTRY_VERTEX_MAIN).unwrap();
        let frag_entry = std::ffi::CString::new(bindings::ENTRY_FRAGMENT_MAIN).unwrap();
        let stages = [
            crate::shader::shader_stage(vk::ShaderStageFlags::VERTEX, vert, vert_entry.as_c_str()),
            crate::shader::shader_stage(
                vk::ShaderStageFlags::FRAGMENT,
                frag,
                frag_entry.as_c_str(),
            ),
        ];

        let push_ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(bindings::PUSH_CONSTANT_SIZE)];
        let layout = unsafe {
            device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(std::slice::from_ref(&self.set_layout))
                    .push_constant_ranges(&push_ranges),
                None,
            )
        }
        .context("UiBackdropPass: pipeline layout")?;

        let empty_bindings: [vk::VertexInputBindingDescription; 0] = [];
        let empty_attrs: [vk::VertexInputAttributeDescription; 0] = [];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&empty_bindings)
            .vertex_attribute_descriptions(&empty_attrs);
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic_state =
            vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let blend = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(false);
        let color_blend = vk::PipelineColorBlendStateCreateInfo::default()
            .attachments(std::slice::from_ref(&blend));
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(false)
            .depth_write_enable(false);

        let info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterizer)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth_stencil)
            .color_blend_state(&color_blend)
            .dynamic_state(&dynamic_state)
            .layout(layout)
            .render_pass(self.render_pass)
            .subpass(0);
        let pipeline =
            unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None) }
                .map_err(|(_, err)| anyhow::anyhow!("UiBackdropPass: create pipeline: {err:?}"))?
                [0];
        unsafe {
            device.destroy_shader_module(vert, None);
            device.destroy_shader_module(frag, None);
        }
        self.pipeline = Some(pipeline);
        self.pipeline_layout = Some(layout);
        let _ = context;
        Ok(())
    }

    /// Move `target` to `new_layout`, updating its recorded layout.
    ///
    /// Each direction names the stages that actually performed the old write
    /// and will perform the new one: an access mask outside its stage mask is
    /// a validation error, so the pairs are enumerated rather than guessed.
    fn transition(
        context: &VulkanContext,
        cmd: vk::CommandBuffer,
        target: &mut Target,
        new_layout: vk::ImageLayout,
    ) {
        let old_layout = std::mem::replace(&mut target.layout, new_layout);
        use vk::{AccessFlags as A, ImageLayout as L, PipelineStageFlags as S};
        let (src_stage, src, dst_stage, dst) = match (old_layout, new_layout) {
            // Attachment write -> texture read.
            (L::COLOR_ATTACHMENT_OPTIMAL, L::SHADER_READ_ONLY_OPTIMAL) => (
                S::COLOR_ATTACHMENT_OUTPUT,
                A::COLOR_ATTACHMENT_WRITE,
                S::FRAGMENT_SHADER,
                A::SHADER_READ,
            ),
            // Copy destination -> texture read (the horizontal pass's input).
            (L::TRANSFER_DST_OPTIMAL, L::SHADER_READ_ONLY_OPTIMAL) => (
                S::TRANSFER,
                A::TRANSFER_WRITE,
                S::FRAGMENT_SHADER,
                A::SHADER_READ,
            ),
            // Copy destination -> attachment write, with no shader read between.
            (L::TRANSFER_DST_OPTIMAL, L::COLOR_ATTACHMENT_OPTIMAL) => (
                S::TRANSFER,
                A::TRANSFER_WRITE,
                S::COLOR_ATTACHMENT_OUTPUT,
                A::COLOR_ATTACHMENT_WRITE,
            ),
            // Texture read -> attachment write (recycle a sampled target).
            (L::SHADER_READ_ONLY_OPTIMAL, L::COLOR_ATTACHMENT_OPTIMAL) => (
                S::FRAGMENT_SHADER,
                A::SHADER_READ,
                S::COLOR_ATTACHMENT_OUTPUT,
                A::COLOR_ATTACHMENT_WRITE,
            ),
            // Attachment write -> copy source.
            (L::COLOR_ATTACHMENT_OPTIMAL, L::TRANSFER_SRC_OPTIMAL) => (
                S::COLOR_ATTACHMENT_OUTPUT,
                A::COLOR_ATTACHMENT_WRITE,
                S::TRANSFER,
                A::TRANSFER_READ,
            ),
            // Fresh allocation: contents are discarded, so no source scope.
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
            .image(target.image)
            .subresource_range(color_range())
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
    ) -> anyhow::Result<vk::RenderPass> {
        let attachment = vk::AttachmentDescription::default()
            .format(format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::DONT_CARE)
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
            .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE);
        unsafe {
            device.create_render_pass(
                &vk::RenderPassCreateInfo::default()
                    .attachments(std::slice::from_ref(&attachment))
                    .subpasses(std::slice::from_ref(&subpass))
                    .dependencies(std::slice::from_ref(&dependency)),
                None,
            )
        }
        .context("UiBackdropPass: create render pass")
    }

    fn create_target(
        context: &VulkanContext,
        extent: vk::Extent2D,
        format: vk::Format,
        render_pass: vk::RenderPass,
    ) -> anyhow::Result<Target> {
        let extent = vk::Extent2D {
            width: extent.width.max(1),
            height: extent.height.max(1),
        };
        let image = unsafe {
            context.device.create_image(
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
                        vk::ImageUsageFlags::COLOR_ATTACHMENT
                            | vk::ImageUsageFlags::SAMPLED
                            | vk::ImageUsageFlags::TRANSFER_DST
                            | vk::ImageUsageFlags::TRANSFER_SRC,
                    ),
                None,
            )
        }
        .context("UiBackdropPass: create image")?;

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
        .context("UiBackdropPass: allocate image memory")?;
        unsafe { context.device.bind_image_memory(image, memory, 0) }
            .context("UiBackdropPass: bind image memory")?;

        let view = unsafe {
            context.device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(format)
                    .subresource_range(color_range()),
                None,
            )
        }
        .context("UiBackdropPass: create image view")?;

        let framebuffer = unsafe {
            context.device.create_framebuffer(
                &vk::FramebufferCreateInfo::default()
                    .render_pass(render_pass)
                    .attachments(std::slice::from_ref(&view))
                    .width(extent.width)
                    .height(extent.height)
                    .layers(1),
                None,
            )
        }
        .context("UiBackdropPass: create framebuffer")?;

        Ok(Target {
            image,
            memory,
            view,
            framebuffer,
            extent,
            layout: vk::ImageLayout::UNDEFINED,
        })
    }
}

impl Drop for UiBackdropPass {
    fn drop(&mut self) {
        unsafe {
            if let Some(pipeline) = self.pipeline.take() {
                self.device.destroy_pipeline(pipeline, None);
            }
            if let Some(layout) = self.pipeline_layout.take() {
                self.device.destroy_pipeline_layout(layout, None);
            }
            self.device.destroy_sampler(self.sampler, None);
            self.device.destroy_descriptor_pool(self.pool, None);
            self.device
                .destroy_descriptor_set_layout(self.set_layout, None);
            for target in &self.targets {
                self.device.destroy_framebuffer(target.framebuffer, None);
                self.device.destroy_image_view(target.view, None);
                self.device.destroy_image(target.image, None);
                self.device.free_memory(target.memory, None);
            }
            self.device.destroy_render_pass(self.render_pass, None);
        }
    }
}

fn color_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
    }
}
