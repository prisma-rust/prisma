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
    DlssMode as GraphDlssMode, GraphResources, PassInfo, PassKind, RenderContext,
    RenderGraphBuilder, RenderMode, RenderPassNode, RenderSettings, ResourceHandle, ResourceType,
    ResourceUsage, DLSS_COLOR_H, FORWARD_ALBEDO_H, FORWARD_COLOR_H, FORWARD_DEPTH_H,
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

/// Render-graph pass that tags Streamline inputs and evaluates DLSS-SR / DLSS-RR.
pub struct DlssPass {
    output_extent: vk::Extent2D,
    images: [vk::Image; 2],
    memories: [vk::DeviceMemory; 2],
    views: [vk::ImageView; 2],
    initialized: [bool; 2],
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

    fn destroy_images(&mut self) {
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
            image: depth_img,
            view: depth_view,
            layout: depth_layout,
            format: depth_format,
            extent: ctx.render_extent,
            usage: if depth_layout == vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL {
                vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::SAMPLED
            } else {
                vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::SAMPLED
            },
        });
        tagged.push(TaggedImage {
            buffer_type: BUFFER_TYPE_MOTION_VECTORS,
            image: vel_img,
            view: vel_view,
            layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            format: vk::Format::R16G16_SFLOAT,
            extent: ctx.render_extent,
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
                image: color_img,
                view: color_view,
                layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                format: vk::Format::R16G16B16A16_SFLOAT,
                extent: ctx.render_extent,
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
                lifecycle: 2, // ValidUntilEvaluate
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

fn image_barrier(
    image: vk::Image,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
    src_access: vk::AccessFlags,
    dst_access: vk::AccessFlags,
) -> vk::ImageMemoryBarrier<'static> {
    vk::ImageMemoryBarrier::default()
        .old_layout(old_layout)
        .new_layout(new_layout)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        })
        .src_access_mask(src_access)
        .dst_access_mask(dst_access)
}

fn create_dlss_image(
    context: &VulkanContext,
    extent: vk::Extent2D,
) -> Result<(vk::Image, vk::DeviceMemory, vk::ImageView)> {
    let device = &context.device;
    let format = vk::Format::R16G16B16A16_SFLOAT;
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
                .usage(
                    vk::ImageUsageFlags::SAMPLED
                        | vk::ImageUsageFlags::STORAGE
                        | vk::ImageUsageFlags::TRANSFER_DST,
                )
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
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                }),
            None,
        )
    }
    .context("DlssPass: create image view")?;

    Ok((image, memory, view))
}
