//! NVIDIA Streamline DLSS-SR / DLSS-RR / DLFG render-graph pass.
//!
//! Sits after scene rendering (`ForwardPass` / `VoxelDdaPass` / `PathTracePass`)
//! and before `PostPass`. Tags frame color, depth, motion vectors, and optional
//! normal/roughness/albedo guides via `slSetTagForFrame`, evaluates DLSS-SR or
//! DLSS-RR into a full-resolution HDR target, and publishes [`DLSS_COLOR_H`].

use std::ffi::c_void;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use ash::vk;

use crate::context::VulkanContext;
use crate::render_graph::{
    DlssMode as GraphDlssMode, DlssPreUpscaleMode, GraphResources, PassInfo, PassKind,
    RenderContext, RenderGraphBuilder, RenderMode, RenderPassNode, RenderSettings, ResourceHandle,
    ResourceType, ResourceUsage, DLSS_COLOR_H, FORWARD_ALBEDO_H, FORWARD_COLOR_H, FORWARD_DEPTH_H,
    FORWARD_NORMAL_H, FORWARD_VELOCITY_H, PT_COLOR_H, PT_DEPTH_H, PT_NORMAL_ROUGHNESS_H,
    PT_VELOCITY_H, VOXEL_COLOR_H, VOXEL_DEPTH_H,
};
use crate::render_pass::find_memory_type;
use crate::streamline::{
    BaseStructure, BufferType, DlssMode, DlssOptions, DlssdNormalRoughnessMode, DlssdOptions,
    DlssgMode, SlExtent, SlResource, SlResourceTag, SlResult, StreamlineRuntime,
    BUFFER_TYPE_ALBEDO, BUFFER_TYPE_DEPTH, BUFFER_TYPE_MOTION_VECTORS,
    BUFFER_TYPE_NORMAL_ROUGHNESS, BUFFER_TYPE_SCALING_INPUT_COLOR,
    BUFFER_TYPE_SCALING_OUTPUT_COLOR, RESOURCE_GUID, RESOURCE_TAG_GUID,
};

struct TaggedImage {
    buffer_type: BufferType,
    image: vk::Image,
    view: vk::ImageView,
    layout: vk::ImageLayout,
    format: vk::Format,
    extent: vk::Extent2D,
    usage: vk::ImageUsageFlags,
}

struct ImageTriple {
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
}

struct PreUpscaleTargets {
    extent: vk::Extent2D,
    depth_format: vk::Format,
    color: [ImageTriple; 2],
    velocity: [ImageTriple; 2],
    depth: [ImageTriple; 2],
}

impl PreUpscaleTargets {
    fn new(
        context: &VulkanContext,
        extent: vk::Extent2D,
        depth_format: vk::Format,
    ) -> Result<Self> {
        let color_usage = vk::ImageUsageFlags::TRANSFER_DST
            | vk::ImageUsageFlags::SAMPLED
            | vk::ImageUsageFlags::STORAGE;
        let depth_aspect = if depth_format == vk::Format::D32_SFLOAT {
            vk::ImageAspectFlags::DEPTH
        } else {
            vk::ImageAspectFlags::COLOR
        };
        let depth_usage = vk::ImageUsageFlags::TRANSFER_DST
            | vk::ImageUsageFlags::SAMPLED
            | if depth_format == vk::Format::D32_SFLOAT {
                vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
            } else {
                vk::ImageUsageFlags::STORAGE
            };
        let color = [
            create_2d_image(
                context,
                extent,
                vk::Format::R16G16B16A16_SFLOAT,
                color_usage,
                vk::ImageAspectFlags::COLOR,
            )?,
            create_2d_image(
                context,
                extent,
                vk::Format::R16G16B16A16_SFLOAT,
                color_usage,
                vk::ImageAspectFlags::COLOR,
            )?,
        ];
        let velocity = [
            create_2d_image(
                context,
                extent,
                vk::Format::R16G16_SFLOAT,
                color_usage,
                vk::ImageAspectFlags::COLOR,
            )?,
            create_2d_image(
                context,
                extent,
                vk::Format::R16G16_SFLOAT,
                color_usage,
                vk::ImageAspectFlags::COLOR,
            )?,
        ];
        let depth = [
            create_2d_image(context, extent, depth_format, depth_usage, depth_aspect)?,
            create_2d_image(context, extent, depth_format, depth_usage, depth_aspect)?,
        ];
        Ok(Self {
            extent,
            depth_format,
            color,
            velocity,
            depth,
        })
    }

    fn destroy(&mut self, device: &ash::Device) {
        unsafe {
            for t in self
                .color
                .iter_mut()
                .chain(self.velocity.iter_mut())
                .chain(self.depth.iter_mut())
            {
                if t.view != vk::ImageView::null() {
                    device.destroy_image_view(t.view, None);
                    t.view = vk::ImageView::null();
                }
                if t.image != vk::Image::null() {
                    device.destroy_image(t.image, None);
                    t.image = vk::Image::null();
                }
                if t.memory != vk::DeviceMemory::null() {
                    device.free_memory(t.memory, None);
                    t.memory = vk::DeviceMemory::null();
                }
            }
        }
    }
}

/// Render-graph pass that tags Streamline inputs and evaluates DLSS-SR / DLSS-RR.
pub struct DlssPass {
    output_extent: vk::Extent2D,
    images: [vk::Image; 2],
    memories: [vk::DeviceMemory; 2],
    views: [vk::ImageView; 2],
    initialized: [bool; 2],
    pre_targets: Option<PreUpscaleTargets>,
    device: ash::Device,
    streamline: Option<Arc<StreamlineRuntime>>,
    logged_eval_error: bool,
}

impl DlssPass {
    pub fn new(
        context: &VulkanContext,
        output_extent: vk::Extent2D,
        streamline: Option<Arc<StreamlineRuntime>>,
    ) -> Result<Self> {
        let output_extent = vk::Extent2D {
            width: output_extent.width.max(1),
            height: output_extent.height.max(1),
        };
        let mut images = [vk::Image::null(); 2];
        let mut memories = [vk::DeviceMemory::null(); 2];
        let mut views = [vk::ImageView::null(); 2];
        for i in 0..2 {
            let (img, mem, view) = create_dlss_image(context, output_extent)?;
            images[i] = img;
            memories[i] = mem;
            views[i] = view;
        }
        Ok(Self {
            output_extent,
            images,
            memories,
            views,
            initialized: [false; 2],
            pre_targets: None,
            device: context.device.clone(),
            streamline,
            logged_eval_error: false,
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
        if output_extent == self.output_extent {
            return Ok(());
        }
        self.destroy_images();
        for i in 0..2 {
            let (img, mem, view) = create_dlss_image(context, output_extent)?;
            self.images[i] = img;
            self.memories[i] = mem;
            self.views[i] = view;
            self.initialized[i] = false;
        }
        self.output_extent = output_extent;
        Ok(())
    }

    /// Release Streamline runtime and owned images before `VkDevice` destruction.
    pub fn release_streamline(&mut self) {
        self.streamline = None;
        self.destroy_images();
    }

    fn destroy_images(&mut self) {
        if let Some(mut pre) = self.pre_targets.take() {
            pre.destroy(&self.device);
        }
        unsafe {
            for i in 0..2 {
                if self.views[i] != vk::ImageView::null() {
                    self.device.destroy_image_view(self.views[i], None);
                    self.views[i] = vk::ImageView::null();
                }
                if self.images[i] != vk::Image::null() {
                    self.device.destroy_image(self.images[i], None);
                    self.images[i] = vk::Image::null();
                }
                if self.memories[i] != vk::DeviceMemory::null() {
                    self.device.free_memory(self.memories[i], None);
                    self.memories[i] = vk::DeviceMemory::null();
                }
            }
        }
    }
}

impl Drop for DlssPass {
    fn drop(&mut self) {
        self.destroy_images();
    }
}

impl RenderPassNode for DlssPass {
    fn name(&self) -> &'static str {
        "DlssPass"
    }

    fn graph_info(&self) -> PassInfo {
        PassInfo {
            index: usize::MAX,
            name: "DlssPass".into(),
            kind: PassKind::Dlss,
            inputs: vec![FORWARD_COLOR_H, FORWARD_DEPTH_H, FORWARD_VELOCITY_H],
            outputs: vec![DLSS_COLOR_H],
        }
    }

    fn setup(&mut self, graph: &mut RenderGraphBuilder, _settings: &RenderSettings) {
        graph.create_resource_at(
            DLSS_COLOR_H,
            ResourceType::ColorAttachment {
                format: vk::Format::R16G16B16A16_SFLOAT,
                extent: self.output_extent,
                sample_count: vk::SampleCountFlags::TYPE_1,
            },
        );
        for handle in [
            FORWARD_COLOR_H,
            FORWARD_VELOCITY_H,
            FORWARD_NORMAL_H,
            FORWARD_ALBEDO_H,
            VOXEL_COLOR_H,
            VOXEL_DEPTH_H,
            PT_COLOR_H,
            PT_DEPTH_H,
            PT_VELOCITY_H,
            PT_NORMAL_ROUGHNESS_H,
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
            handle: DLSS_COLOR_H,
            access: vk::AccessFlags::SHADER_WRITE,
            stage: vk::PipelineStageFlags::COMPUTE_SHADER,
            layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        });
    }

    fn execute(&mut self, ctx: &RenderContext, resources: &mut GraphResources) -> Result<()> {
        let Some(sl) = self.streamline.clone() else {
            return Ok(());
        };
        if ctx.frame.debug_rt != 0 {
            return Ok(());
        }

        let sl_dlss_mode = map_dlss_mode(ctx.settings.dlss_mode);
        let dlfg_active = ctx.settings.dlfg_mode != DlssgMode::Off && sl.caps().dlss_fg_supported;
        if sl_dlss_mode == DlssMode::Off && !dlfg_active {
            return Ok(());
        }

        self.recreate_target(ctx.context, ctx.extent)?;

        let is_pt = ctx.frame.render_mode == RenderMode::PathTrace;
        let has_voxel = !is_pt
            && resources.published_view(VOXEL_COLOR_H).is_some()
            && resources.published_view(VOXEL_DEPTH_H).is_some();

        let (color_h, depth_h, velocity_h, depth_layout, depth_format) = if is_pt {
            (
                PT_COLOR_H,
                PT_DEPTH_H,
                PT_VELOCITY_H,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::Format::R32_SFLOAT,
            )
        } else if has_voxel {
            (
                VOXEL_COLOR_H,
                VOXEL_DEPTH_H,
                FORWARD_VELOCITY_H,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::Format::R32_SFLOAT,
            )
        } else {
            (
                FORWARD_COLOR_H,
                FORWARD_DEPTH_H,
                FORWARD_VELOCITY_H,
                vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL,
                vk::Format::D32_SFLOAT,
            )
        };

        let Some((color_img, color_view)) = published_pair(resources, color_h) else {
            return Ok(());
        };
        let Some((depth_img, depth_view)) = published_pair(resources, depth_h) else {
            return Ok(());
        };
        let Some((vel_img, vel_view)) = published_pair(resources, velocity_h) else {
            return Ok(());
        };

        let frame = (ctx.frame_index as usize) & 1;

        let (
            input_color_img,
            input_color_view,
            input_depth_img,
            input_depth_view,
            input_vel_img,
            input_vel_view,
            input_extent,
        ) = if sl_dlss_mode != DlssMode::Off
            && ctx.settings.dlss_pre_upscale != DlssPreUpscaleMode::Off
        {
            let min_extent = dlss_min_extent(&sl, sl_dlss_mode, self.output_extent);
            if ctx.render_extent.width < min_extent.width
                || ctx.render_extent.height < min_extent.height
            {
                let pre_extent = vk::Extent2D {
                    width: ctx.render_extent.width.max(min_extent.width),
                    height: ctx.render_extent.height.max(min_extent.height),
                };
                let recreate_pre = self
                    .pre_targets
                    .as_ref()
                    .is_none_or(|p| p.extent != pre_extent || p.depth_format != depth_format);
                if recreate_pre {
                    if let Some(mut old) = self.pre_targets.take() {
                        old.destroy(&self.device);
                    }
                    self.pre_targets = Some(PreUpscaleTargets::new(
                        ctx.context,
                        pre_extent,
                        depth_format,
                    )?);
                }
                let pre = self.pre_targets.as_ref().unwrap();
                let pre_c = &pre.color[frame];
                let pre_d = &pre.depth[frame];
                let pre_v = &pre.velocity[frame];
                let depth_aspect = if depth_format == vk::Format::D32_SFLOAT {
                    vk::ImageAspectFlags::DEPTH
                } else {
                    vk::ImageAspectFlags::COLOR
                };
                let color_filter = match ctx.settings.dlss_pre_upscale {
                    DlssPreUpscaleMode::Nearest => vk::Filter::NEAREST,
                    DlssPreUpscaleMode::Linear | DlssPreUpscaleMode::Off => vk::Filter::LINEAR,
                };

                let to_transfer = [
                    image_barrier_aspect(
                        color_img,
                        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        vk::AccessFlags::SHADER_READ,
                        vk::AccessFlags::TRANSFER_READ,
                        vk::ImageAspectFlags::COLOR,
                    ),
                    image_barrier_aspect(
                        vel_img,
                        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        vk::AccessFlags::SHADER_READ,
                        vk::AccessFlags::TRANSFER_READ,
                        vk::ImageAspectFlags::COLOR,
                    ),
                    image_barrier_aspect(
                        depth_img,
                        depth_layout,
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        vk::AccessFlags::SHADER_READ,
                        vk::AccessFlags::TRANSFER_READ,
                        depth_aspect,
                    ),
                    image_barrier_aspect(
                        pre_c.image,
                        vk::ImageLayout::UNDEFINED,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        vk::AccessFlags::empty(),
                        vk::AccessFlags::TRANSFER_WRITE,
                        vk::ImageAspectFlags::COLOR,
                    ),
                    image_barrier_aspect(
                        pre_v.image,
                        vk::ImageLayout::UNDEFINED,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        vk::AccessFlags::empty(),
                        vk::AccessFlags::TRANSFER_WRITE,
                        vk::ImageAspectFlags::COLOR,
                    ),
                    image_barrier_aspect(
                        pre_d.image,
                        vk::ImageLayout::UNDEFINED,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        vk::AccessFlags::empty(),
                        vk::AccessFlags::TRANSFER_WRITE,
                        depth_aspect,
                    ),
                ];
                unsafe {
                    ctx.device.cmd_pipeline_barrier(
                        ctx.cmd,
                        vk::PipelineStageFlags::ALL_COMMANDS,
                        vk::PipelineStageFlags::TRANSFER,
                        vk::DependencyFlags::empty(),
                        &[],
                        &[],
                        &to_transfer,
                    );
                    blit_2d(
                        ctx.device,
                        ctx.cmd,
                        color_img,
                        pre_c.image,
                        ctx.render_extent,
                        pre_extent,
                        vk::ImageAspectFlags::COLOR,
                        color_filter,
                    );
                    blit_2d(
                        ctx.device,
                        ctx.cmd,
                        vel_img,
                        pre_v.image,
                        ctx.render_extent,
                        pre_extent,
                        vk::ImageAspectFlags::COLOR,
                        vk::Filter::NEAREST,
                    );
                    blit_2d(
                        ctx.device,
                        ctx.cmd,
                        depth_img,
                        pre_d.image,
                        ctx.render_extent,
                        pre_extent,
                        depth_aspect,
                        vk::Filter::NEAREST,
                    );
                    let to_shader = [
                        image_barrier_aspect(
                            color_img,
                            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                            vk::AccessFlags::TRANSFER_READ,
                            vk::AccessFlags::SHADER_READ,
                            vk::ImageAspectFlags::COLOR,
                        ),
                        image_barrier_aspect(
                            vel_img,
                            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                            vk::AccessFlags::TRANSFER_READ,
                            vk::AccessFlags::SHADER_READ,
                            vk::ImageAspectFlags::COLOR,
                        ),
                        image_barrier_aspect(
                            depth_img,
                            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                            depth_layout,
                            vk::AccessFlags::TRANSFER_READ,
                            vk::AccessFlags::SHADER_READ,
                            depth_aspect,
                        ),
                        image_barrier_aspect(
                            pre_c.image,
                            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                            vk::AccessFlags::TRANSFER_WRITE,
                            vk::AccessFlags::SHADER_READ,
                            vk::ImageAspectFlags::COLOR,
                        ),
                        image_barrier_aspect(
                            pre_v.image,
                            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                            vk::AccessFlags::TRANSFER_WRITE,
                            vk::AccessFlags::SHADER_READ,
                            vk::ImageAspectFlags::COLOR,
                        ),
                        image_barrier_aspect(
                            pre_d.image,
                            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                            depth_layout,
                            vk::AccessFlags::TRANSFER_WRITE,
                            vk::AccessFlags::SHADER_READ,
                            depth_aspect,
                        ),
                    ];
                    ctx.device.cmd_pipeline_barrier(
                        ctx.cmd,
                        vk::PipelineStageFlags::TRANSFER,
                        vk::PipelineStageFlags::COMPUTE_SHADER,
                        vk::DependencyFlags::empty(),
                        &[],
                        &[],
                        &to_shader,
                    );
                }
                (
                    pre_c.image,
                    pre_c.view,
                    pre_d.image,
                    pre_d.view,
                    pre_v.image,
                    pre_v.view,
                    pre_extent,
                )
            } else {
                (
                    color_img,
                    color_view,
                    depth_img,
                    depth_view,
                    vel_img,
                    vel_view,
                    ctx.render_extent,
                )
            }
        } else {
            (
                color_img,
                color_view,
                depth_img,
                depth_view,
                vel_img,
                vel_view,
                ctx.render_extent,
            )
        };
        if sl_dlss_mode != DlssMode::Off {
            let old_layout = if self.initialized[frame] {
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
            } else {
                vk::ImageLayout::UNDEFINED
            };
            let pre_barrier = [image_barrier(
                self.images[frame],
                old_layout,
                vk::ImageLayout::GENERAL,
                vk::AccessFlags::empty(),
                vk::AccessFlags::SHADER_WRITE,
            )];
            unsafe {
                ctx.device.cmd_pipeline_barrier(
                    ctx.cmd,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &pre_barrier,
                );
            }
        }

        let mut tagged = Vec::with_capacity(6);
        tagged.push(TaggedImage {
            buffer_type: BUFFER_TYPE_DEPTH,
            image: input_depth_img,
            view: input_depth_view,
            layout: depth_layout,
            format: depth_format,
            extent: input_extent,
            usage: if depth_layout == vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL {
                vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::SAMPLED
            } else {
                vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::SAMPLED
            },
        });
        tagged.push(TaggedImage {
            buffer_type: BUFFER_TYPE_MOTION_VECTORS,
            image: input_vel_img,
            view: input_vel_view,
            layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            format: vk::Format::R16G16_SFLOAT,
            extent: input_extent,
            usage: vk::ImageUsageFlags::COLOR_ATTACHMENT
                | vk::ImageUsageFlags::STORAGE
                | vk::ImageUsageFlags::SAMPLED,
        });

        let normal_roughness_h = if is_pt {
            PT_NORMAL_ROUGHNESS_H
        } else {
            FORWARD_NORMAL_H
        };
        let has_normal_roughness = if let Some((nr_img, nr_view)) =
            published_pair(resources, normal_roughness_h)
        {
            if ctx.settings.dlss_rr_enabled && sl.caps().dlss_rr_supported {
                tagged.push(TaggedImage {
                    buffer_type: BUFFER_TYPE_NORMAL_ROUGHNESS,
                    image: nr_img,
                    view: nr_view,
                    layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    format: vk::Format::R16G16B16A16_SFLOAT,
                    extent: ctx.render_extent,
                    usage: vk::ImageUsageFlags::COLOR_ATTACHMENT
                        | vk::ImageUsageFlags::STORAGE
                        | vk::ImageUsageFlags::SAMPLED,
                });
                if let Some((alb_img, alb_view)) = published_pair(resources, FORWARD_ALBEDO_H) {
                    tagged.push(TaggedImage {
                        buffer_type: BUFFER_TYPE_ALBEDO,
                        image: alb_img,
                        view: alb_view,
                        layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        format: vk::Format::R8G8B8A8_UNORM,
                        extent: ctx.render_extent,
                        usage: vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED,
                    });
                }
                true
            } else {
                false
            }
        } else {
            false
        };

        if sl_dlss_mode != DlssMode::Off {
            tagged.push(TaggedImage {
                buffer_type: BUFFER_TYPE_SCALING_INPUT_COLOR,
                image: input_color_img,
                view: input_color_view,
                layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                format: vk::Format::R16G16B16A16_SFLOAT,
                extent: input_extent,
                usage: vk::ImageUsageFlags::COLOR_ATTACHMENT
                    | vk::ImageUsageFlags::STORAGE
                    | vk::ImageUsageFlags::SAMPLED,
            });
            tagged.push(TaggedImage {
                buffer_type: BUFFER_TYPE_SCALING_OUTPUT_COLOR,
                image: self.images[frame],
                view: self.views[frame],
                layout: vk::ImageLayout::GENERAL,
                format: vk::Format::R16G16B16A16_SFLOAT,
                extent: self.output_extent,
                usage: vk::ImageUsageFlags::STORAGE
                    | vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::TRANSFER_DST,
            });
        }

        let mut sl_resources: Vec<SlResource> = tagged
            .iter()
            .map(|t| SlResource {
                base: BaseStructure::new(RESOURCE_GUID, 1),
                res_type: 0,
                native: vk::Handle::as_raw(t.image) as *mut c_void,
                memory: std::ptr::null_mut(),
                view: vk::Handle::as_raw(t.view) as *mut c_void,
                state: t.layout.as_raw() as u32,
                width: t.extent.width,
                height: t.extent.height,
                native_format: t.format.as_raw() as u32,
                mip_levels: 1,
                array_layers: 1,
                gpu_virtual_address: 0,
                flags: 0,
                usage: t.usage.as_raw(),
                internal_flags: 0,
                reserved: 0,
            })
            .collect();

        let tags: Vec<SlResourceTag> = tagged
            .iter()
            .zip(sl_resources.iter_mut())
            .map(|(t, res)| SlResourceTag {
                base: BaseStructure::new(RESOURCE_TAG_GUID, 1),
                resource: &raw mut *res,
                buffer_type: t.buffer_type,
                lifecycle: 2,
                extent: SlExtent {
                    top: 0,
                    left: 0,
                    width: t.extent.width,
                    height: t.extent.height,
                },
            })
            .collect();

        let tag_res = sl.tag_resource(ctx.frame_index, &tags, ctx.cmd);
        if tag_res != SlResult::Ok && !self.logged_eval_error {
            log::warn!("Streamline slSetTagForFrame returned {tag_res:?}");
            self.logged_eval_error = true;
        }

        if sl_dlss_mode != DlssMode::Off {
            let eval_res = if ctx.settings.dlss_rr_enabled
                && sl.caps().dlss_rr_supported
                && has_normal_roughness
            {
                let world_to_camera = ctx.frame.view;
                let camera_to_world = glam::Mat4::from_cols_array_2d(&world_to_camera)
                    .inverse()
                    .to_cols_array_2d();
                let rr_opts = DlssdOptions {
                    mode: sl_dlss_mode,
                    output_width: self.output_extent.width,
                    output_height: self.output_extent.height,
                    normal_roughness_mode: DlssdNormalRoughnessMode::Packed,
                    world_to_camera_view: world_to_camera,
                    camera_view_to_world: camera_to_world,
                    ..Default::default()
                };
                sl.evaluate_dlss_rr(ctx.cmd, ctx.frame_index, &rr_opts)
            } else if sl.caps().dlss_sr_supported {
                let sr_opts = DlssOptions {
                    mode: sl_dlss_mode,
                    output_width: self.output_extent.width,
                    output_height: self.output_extent.height,
                    ..Default::default()
                };
                sl.evaluate_dlss_sr(ctx.cmd, ctx.frame_index, &sr_opts)
            } else {
                SlResult::ErrorFeatureNotSupported
            };

            let post_barrier = [image_barrier(
                self.images[frame],
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::AccessFlags::SHADER_WRITE,
                vk::AccessFlags::SHADER_READ,
            )];
            unsafe {
                ctx.device.cmd_pipeline_barrier(
                    ctx.cmd,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::PipelineStageFlags::FRAGMENT_SHADER
                        | vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &post_barrier,
                );
            }
            self.initialized[frame] = true;

            if eval_res == SlResult::Ok {
                resources.set_image_view(DLSS_COLOR_H, self.views[frame]);
                resources.set_image(DLSS_COLOR_H, self.images[frame]);
            } else if !self.logged_eval_error {
                log::warn!("Streamline DLSS evaluation returned {eval_res:?}");
                self.logged_eval_error = true;
            }
        }

        Ok(())
    }
}

fn map_dlss_mode(mode: GraphDlssMode) -> DlssMode {
    match mode {
        GraphDlssMode::Off => DlssMode::Off,
        GraphDlssMode::UltraPerformance => DlssMode::UltraPerformance,
        GraphDlssMode::Performance => DlssMode::MaxPerformance,
        GraphDlssMode::Balanced => DlssMode::Balanced,
        GraphDlssMode::Quality => DlssMode::MaxQuality,
        GraphDlssMode::Dlaa => DlssMode::Dlaa,
    }
}

fn published_pair(
    resources: &GraphResources,
    handle: ResourceHandle,
) -> Option<(vk::Image, vk::ImageView)> {
    let img = resources.published_image(handle)?;
    let view = resources.published_view(handle)?;
    Some((img, view))
}

fn dlss_min_extent(
    sl: &StreamlineRuntime,
    mode: DlssMode,
    output_extent: vk::Extent2D,
) -> vk::Extent2D {
    let opts = DlssOptions {
        mode,
        output_width: output_extent.width,
        output_height: output_extent.height,
        ..Default::default()
    };
    if let Some(opt) = sl.dlss_optimal_settings(&opts) {
        if opt.render_width_min > 0 && opt.render_height_min > 0 {
            return vk::Extent2D {
                width: opt.render_width_min,
                height: opt.render_height_min,
            };
        }
    }
    match mode {
        DlssMode::UltraPerformance => vk::Extent2D {
            width: output_extent.width.div_ceil(3).max(1),
            height: output_extent.height.div_ceil(3).max(1),
        },
        DlssMode::Dlaa => output_extent,
        _ => vk::Extent2D {
            width: output_extent.width.div_ceil(2).max(1),
            height: output_extent.height.div_ceil(2).max(1),
        },
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "arity mirrors the float-parameter packing that the callers already flatten"
)]
unsafe fn blit_2d(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    src_image: vk::Image,
    dst_image: vk::Image,
    src_extent: vk::Extent2D,
    dst_extent: vk::Extent2D,
    aspect_mask: vk::ImageAspectFlags,
    filter: vk::Filter,
) {
    let region = vk::ImageBlit::default()
        .src_subresource(vk::ImageSubresourceLayers {
            aspect_mask,
            mip_level: 0,
            base_array_layer: 0,
            layer_count: 1,
        })
        .src_offsets([
            vk::Offset3D { x: 0, y: 0, z: 0 },
            vk::Offset3D {
                x: src_extent.width as i32,
                y: src_extent.height as i32,
                z: 1,
            },
        ])
        .dst_subresource(vk::ImageSubresourceLayers {
            aspect_mask,
            mip_level: 0,
            base_array_layer: 0,
            layer_count: 1,
        })
        .dst_offsets([
            vk::Offset3D { x: 0, y: 0, z: 0 },
            vk::Offset3D {
                x: dst_extent.width as i32,
                y: dst_extent.height as i32,
                z: 1,
            },
        ]);
    device.cmd_blit_image(
        cmd,
        src_image,
        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
        dst_image,
        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        &[region],
        filter,
    );
}

fn image_barrier_aspect(
    image: vk::Image,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
    src_access: vk::AccessFlags,
    dst_access: vk::AccessFlags,
    aspect_mask: vk::ImageAspectFlags,
) -> vk::ImageMemoryBarrier<'static> {
    vk::ImageMemoryBarrier::default()
        .old_layout(old_layout)
        .new_layout(new_layout)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        })
        .src_access_mask(src_access)
        .dst_access_mask(dst_access)
}

fn image_barrier(
    image: vk::Image,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
    src_access: vk::AccessFlags,
    dst_access: vk::AccessFlags,
) -> vk::ImageMemoryBarrier<'static> {
    image_barrier_aspect(
        image,
        old_layout,
        new_layout,
        src_access,
        dst_access,
        vk::ImageAspectFlags::COLOR,
    )
}

fn create_dlss_image(
    context: &VulkanContext,
    extent: vk::Extent2D,
) -> Result<(vk::Image, vk::DeviceMemory, vk::ImageView)> {
    let t = create_2d_image(
        context,
        extent,
        vk::Format::R16G16B16A16_SFLOAT,
        vk::ImageUsageFlags::SAMPLED
            | vk::ImageUsageFlags::STORAGE
            | vk::ImageUsageFlags::TRANSFER_DST,
        vk::ImageAspectFlags::COLOR,
    )?;
    Ok((t.image, t.memory, t.view))
}

fn create_2d_image(
    context: &VulkanContext,
    extent: vk::Extent2D,
    format: vk::Format,
    usage: vk::ImageUsageFlags,
    aspect_mask: vk::ImageAspectFlags,
) -> Result<ImageTriple> {
    let device = &context.device;
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
                .usage(usage)
                .sharing_mode(vk::SharingMode::EXCLUSIVE)
                .initial_layout(vk::ImageLayout::UNDEFINED),
            None,
        )
    }
    .context("DlssPass: create image")?;

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
    .context("DlssPass: find DEVICE_LOCAL memory")?;

    let memory = unsafe {
        device.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(req.size)
                .memory_type_index(mem_type),
            None,
        )
    }
    .context("DlssPass: allocate image memory")?;

    unsafe {
        device
            .bind_image_memory(image, memory, 0)
            .context("DlssPass: bind image memory")?;
    }

    let view = unsafe {
        device.create_image_view(
            &vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                }),
            None,
        )
    }
    .context("DlssPass: create image view")?;

    Ok(ImageTriple {
        image,
        memory,
        view,
    })
}
