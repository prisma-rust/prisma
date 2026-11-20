//! Compute RenderGraph pass driving GPU Virtual Geometry Cluster-DAG culling,
//! 32-bit atomic software rasterization, analytic VisBuffer resolve, and HZB builds.

use std::ffi::CStr;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use ash::vk;

use crate::buffer::{create_buffer, find_memory_type};
use crate::compute::ComputePipeline;
use crate::context::VulkanContext;
use crate::render_graph::{
    GraphResources, PassInfo, PassKind, RenderContext, RenderGraphBuilder, RenderMode,
    RenderPassNode, RenderSettings, ResourceUsage, FORWARD_COLOR_H, FORWARD_DEPTH_H,
    FORWARD_NORMAL_H, FORWARD_VELOCITY_H,
};
use crate::shader::load_shader_module;
use crate::shader_bindings::virtual_geo::{
    self as bindings, VgClusterGpu, VgDispatchedClusterGpu, VgFrameUbo, VgGroupGpu, VgPushConstants,
};
use crate::virtual_geo::{VgMeshDesc, VirtualGeoCutConfig, VirtualGeoPagePool};

const VG_CULL_SPV: &[u8] = include_bytes!("../../../assets/shaders/vg_cull.comp.spv");
const VG_SW_RASTER_SPV: &[u8] = include_bytes!("../../../assets/shaders/vg_sw_raster.comp.spv");
const VG_RESOLVE_SPV: &[u8] = include_bytes!("../../../assets/shaders/vg_resolve.comp.spv");
const VG_HZB_BUILD_SPV: &[u8] = include_bytes!("../../../assets/shaders/vg_hzb_build.comp.spv");

const ENTRY_VG_CULL: &CStr = c"vgCullMain";
const ENTRY_VG_SW_RASTER: &CStr = c"vgSwRasterMain";
const ENTRY_VG_RESOLVE: &CStr = c"vgResolveMain";
const ENTRY_VG_HZB_BUILD: &CStr = c"vgHzbBuildMain";

const MAX_DISPATCH_QUEUE_ITEMS: usize = 65_536;
const MAX_FAULT_RECORDS: usize = 4_096;

/// Packed CPU->GPU SSBO payloads extracted from a runtime [`VgMeshDesc`].
#[derive(Clone, Debug, Default)]
pub struct PackedVgMeshBuffers {
    pub groups: Vec<VgGroupGpu>,
    pub clusters: Vec<VgClusterGpu>,
}

/// Converts a [`VgMeshDesc`] and current page pool residency into GPU `VgGroupGpu` and `VgClusterGpu` records.
pub fn pack_vg_mesh_gpu_buffers(
    mesh: &VgMeshDesc,
    pool: &VirtualGeoPagePool,
    cfg: &VirtualGeoCutConfig,
) -> PackedVgMeshBuffers {
    let mut groups = Vec::with_capacity(mesh.groups.len());
    for g in &mesh.groups {
        let resident = u32::from(pool.is_resident(crate::virtual_geo::PageKey {
            asset_id: mesh.asset_id,
            page_index: g.page_index,
        }));
        let packed_topo =
            u32::from(g.lod_level) | ((g.primitive_kind as u32) << 8) | (resident << 16);
        groups.push(VgGroupGpu {
            boundingSphere: g.bounding_sphere,
            errorAndRange: [g.parent_error, g.self_error, cfg.geomorph_window_ratio, 0.0],
            topology: [
                g.group_id,
                packed_topo,
                g.first_cluster,
                g.cluster_count.into(),
            ],
        });
    }

    let mut clusters = Vec::with_capacity(mesh.clusters.len());
    let mut vertex_word_cursor = 0u32;
    let mut tri_word_cursor = 0u32;
    for c in &mesh.clusters {
        let prim_packed = u32::from(c.material_index) | ((c.primitive_kind as u32) << 16);
        clusters.push(VgClusterGpu {
            aabbMin: [c.aabb_min[0], c.aabb_min[1], c.aabb_min[2], c.parent_error],
            aabbMax: [c.aabb_max[0], c.aabb_max[1], c.aabb_max[2], c.self_error],
            normalCone: c.normal_cone,
            meta: [
                c.cluster_id,
                c.group_id,
                vertex_word_cursor,
                tri_word_cursor,
            ],
            countsAndMat: [
                u32::from(c.vertex_count),
                u32::from(c.triangle_count),
                prim_packed,
                c.geomorph_scale.to_bits(),
            ],
            splatCenterOpacity: [0.0, 0.0, 0.0, 1.0],
            splatCovDiag: [1.0, 1.0, 1.0, 1.0],
            splatCovOff: [0.0, 0.0, 0.0, 1.0],
        });
        vertex_word_cursor = vertex_word_cursor.saturating_add(u32::from(c.vertex_count) * 5);
        tri_word_cursor = tri_word_cursor.saturating_add(u32::from(c.triangle_count));
    }

    PackedVgMeshBuffers { groups, clusters }
}

/// Builds a [`VgFrameUbo`] from virtual geometry cut settings and frame lighting metadata.
pub fn build_vg_frame_ubo(
    cfg: &VirtualGeoCutConfig,
    prev_view_proj: [[f32; 4]; 4],
    sun_dir: [f32; 4],
    sun_color: [f32; 4],
    group_count: u32,
    cluster_count: u32,
    has_prev_hzb: bool,
) -> VgFrameUbo {
    let flags = u32::from(cfg.hzb_reverse_z_projection) | (u32::from(has_prev_hzb) << 1);
    VgFrameUbo {
        viewProj: cfg.view_proj,
        invViewProj: crate::math::mat_inverse(&cfg.view_proj),
        prevViewProj: prev_view_proj,
        cameraPosNear: [
            cfg.camera_pos[0],
            cfg.camera_pos[1],
            cfg.camera_pos[2],
            cfg.near_plane,
        ],
        screenParams: [
            cfg.output_extent[0].max(1) as f32,
            cfg.output_extent[1].max(1) as f32,
            cfg.cot_half_fovy,
            cfg.effective_error_threshold_px(),
        ],
        sunDirIntensity: sun_dir,
        sunColorAmbient: sun_color,
        counts: [group_count, cluster_count, 1, flags],
    }
}

/// Per-frame GPU upload payload consumed by [`VirtualGeoPass`].
#[derive(Clone, Debug)]
pub struct VgSceneUpload {
    pub signature: u64,
    pub groups: Vec<VgGroupGpu>,
    pub clusters: Vec<VgClusterGpu>,
    pub packed_vertices: Vec<u32>,
    pub packed_indices: Vec<u32>,
    pub instance_models: Vec<[[f32; 4]; 4]>,
    pub cut_config: VirtualGeoCutConfig,
}

struct HostBuffer {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    capacity_bytes: vk::DeviceSize,
}

impl HostBuffer {
    fn new(
        context: &VulkanContext,
        size: vk::DeviceSize,
        usage: vk::BufferUsageFlags,
    ) -> Result<Self> {
        let cap = size.max(64);
        let (buffer, memory) = create_buffer(
            context,
            cap,
            usage,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .context("VirtualGeoPass: create buffer")?;
        Ok(Self {
            buffer,
            memory,
            capacity_bytes: cap,
        })
    }

    fn write_bytes(&self, device: &ash::Device, data: &[u8]) -> Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        unsafe {
            let ptr = device.map_memory(
                self.memory,
                0,
                data.len() as vk::DeviceSize,
                vk::MemoryMapFlags::empty(),
            )?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len());
            device.unmap_memory(self.memory);
        }
        Ok(())
    }

    fn ensure_and_write(
        &mut self,
        context: &VulkanContext,
        usage: vk::BufferUsageFlags,
        bytes: &[u8],
    ) -> Result<()> {
        let needed = (bytes.len() as vk::DeviceSize).max(64);
        if self.capacity_bytes < needed {
            self.destroy(&context.device);
            let grown = (needed * 3 / 2).max(needed);
            *self = Self::new(context, grown, usage)?;
        }
        self.write_bytes(&context.device, bytes)
    }

    fn ensure_capacity(
        &mut self,
        context: &VulkanContext,
        usage: vk::BufferUsageFlags,
        needed_bytes: vk::DeviceSize,
    ) -> Result<()> {
        let needed = needed_bytes.max(64);
        if self.capacity_bytes < needed {
            self.destroy(&context.device);
            let grown = (needed * 3 / 2).max(needed);
            *self = Self::new(context, grown, usage)?;
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
        self.capacity_bytes = 0;
    }
}

struct ImageResource {
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
}

impl ImageResource {
    fn create(
        context: &VulkanContext,
        width: u32,
        height: u32,
        format: vk::Format,
    ) -> Result<Self> {
        let device = &context.device;
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width: width.max(1),
                height: height.max(1),
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image =
            unsafe { device.create_image(&info, None) }.context("VirtualGeoPass: create image")?;
        let reqs = unsafe { device.get_image_memory_requirements(image) };
        let mem_type = find_memory_type(
            context,
            reqs.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .context("VirtualGeoPass: find image memory type")?;
        let memory = unsafe {
            device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(reqs.size)
                    .memory_type_index(mem_type),
                None,
            )
        }
        .context("VirtualGeoPass: allocate image memory")?;
        unsafe { device.bind_image_memory(image, memory, 0) }
            .context("VirtualGeoPass: bind image memory")?;

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
        .context("VirtualGeoPass: create image view")?;

        Ok(Self {
            image,
            memory,
            view,
        })
    }

    fn destroy(&mut self, device: &ash::Device) {
        unsafe {
            if self.view != vk::ImageView::null() {
                device.destroy_image_view(self.view, None);
                self.view = vk::ImageView::null();
            }
            if self.image != vk::Image::null() {
                device.destroy_image(self.image, None);
                self.image = vk::Image::null();
            }
            if self.memory != vk::DeviceMemory::null() {
                device.free_memory(self.memory, None);
                self.memory = vk::DeviceMemory::null();
            }
        }
    }
}

struct TargetImages {
    extent: vk::Extent2D,
    color: ImageResource,
    depth: ImageResource,
    normal_roughness: ImageResource,
    velocity: ImageResource,
    hzb_prev: ImageResource,
    hzb_curr: ImageResource,
    initialized: bool,
}

impl TargetImages {
    fn create(context: &VulkanContext, extent: vk::Extent2D) -> Result<Self> {
        let w = extent.width.max(1);
        let h = extent.height.max(1);
        let hw = (w / 2).max(1);
        let hh = (h / 2).max(1);
        Ok(Self {
            extent: vk::Extent2D {
                width: w,
                height: h,
            },
            color: ImageResource::create(context, w, h, vk::Format::R16G16B16A16_SFLOAT)?,
            depth: ImageResource::create(context, w, h, vk::Format::R32_SFLOAT)?,
            normal_roughness: ImageResource::create(
                context,
                w,
                h,
                vk::Format::R16G16B16A16_SFLOAT,
            )?,
            velocity: ImageResource::create(context, w, h, vk::Format::R16G16_SFLOAT)?,
            hzb_prev: ImageResource::create(context, hw, hh, vk::Format::R32_SFLOAT)?,
            hzb_curr: ImageResource::create(context, hw, hh, vk::Format::R32_SFLOAT)?,
            initialized: false,
        })
    }

    fn destroy(&mut self, device: &ash::Device) {
        self.color.destroy(device);
        self.depth.destroy(device);
        self.normal_roughness.destroy(device);
        self.velocity.destroy(device);
        self.hzb_prev.destroy(device);
        self.hzb_curr.destroy(device);
    }
}

fn image_barrier(
    image: vk::Image,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
    src_access: vk::AccessFlags,
    dst_access: vk::AccessFlags,
) -> vk::ImageMemoryBarrier<'static> {
    vk::ImageMemoryBarrier::default()
        .image(image)
        .old_layout(old_layout)
        .new_layout(new_layout)
        .src_access_mask(src_access)
        .dst_access_mask(dst_access)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        })
}

struct FrameSlot {
    ds_cull: vk::DescriptorSet,
    ds_hzb: vk::DescriptorSet,
    ubo_buf: HostBuffer,
    groups_buf: HostBuffer,
    clusters_buf: HostBuffer,
    vertices_buf: HostBuffer,
    indices_buf: HostBuffer,
    instances_buf: HostBuffer,
    hw_queue_buf: HostBuffer,
    sw_queue_buf: HostBuffer,
    splat_queue_buf: HostBuffer,
    counters_buf: HostBuffer,
    fault_buf: HostBuffer,
    vis_atomic_buf: HostBuffer,
    uploaded_signature: Option<u64>,
    target: Option<TargetImages>,
}

impl FrameSlot {
    fn new(
        context: &VulkanContext,
        ds_cull: vk::DescriptorSet,
        ds_hzb: vk::DescriptorSet,
    ) -> Result<Self> {
        let storage_rw = vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST;
        let storage = vk::BufferUsageFlags::STORAGE_BUFFER;
        Ok(Self {
            ds_cull,
            ds_hzb,
            ubo_buf: HostBuffer::new(
                context,
                std::mem::size_of::<VgFrameUbo>() as vk::DeviceSize,
                vk::BufferUsageFlags::UNIFORM_BUFFER,
            )?,
            groups_buf: HostBuffer::new(context, 256, storage)?,
            clusters_buf: HostBuffer::new(context, 256, storage)?,
            vertices_buf: HostBuffer::new(context, 256, storage)?,
            indices_buf: HostBuffer::new(context, 256, storage)?,
            instances_buf: HostBuffer::new(context, 64, storage)?,
            hw_queue_buf: HostBuffer::new(
                context,
                (std::mem::size_of::<VgDispatchedClusterGpu>() * MAX_DISPATCH_QUEUE_ITEMS.min(1024))
                    as vk::DeviceSize,
                storage,
            )?,
            sw_queue_buf: HostBuffer::new(
                context,
                (std::mem::size_of::<VgDispatchedClusterGpu>() * MAX_DISPATCH_QUEUE_ITEMS.min(1024))
                    as vk::DeviceSize,
                storage,
            )?,
            splat_queue_buf: HostBuffer::new(
                context,
                (std::mem::size_of::<VgDispatchedClusterGpu>() * MAX_DISPATCH_QUEUE_ITEMS.min(1024))
                    as vk::DeviceSize,
                storage,
            )?,
            counters_buf: HostBuffer::new(context, 64, storage_rw)?,
            fault_buf: HostBuffer::new(
                context,
                (std::mem::size_of::<[u32; 2]>() * MAX_FAULT_RECORDS) as vk::DeviceSize,
                storage,
            )?,
            vis_atomic_buf: HostBuffer::new(context, 4096, storage_rw)?,
            uploaded_signature: None,
            target: None,
        })
    }

    fn upload_geometry(&mut self, context: &VulkanContext, upload: &VgSceneUpload) -> Result<()> {
        if self.uploaded_signature == Some(upload.signature) {
            return Ok(());
        }
        let storage = vk::BufferUsageFlags::STORAGE_BUFFER;

        let groups_fallback = [VgGroupGpu::default()];
        let groups = if upload.groups.is_empty() {
            groups_fallback.as_slice()
        } else {
            upload.groups.as_slice()
        };
        let clusters_fallback = [VgClusterGpu::default()];
        let clusters = if upload.clusters.is_empty() {
            clusters_fallback.as_slice()
        } else {
            upload.clusters.as_slice()
        };
        let verts_fallback = [0u32; 5];
        let verts = if upload.packed_vertices.is_empty() {
            verts_fallback.as_slice()
        } else {
            upload.packed_vertices.as_slice()
        };
        let tris_fallback = [0u32; 1];
        let tris = if upload.packed_indices.is_empty() {
            tris_fallback.as_slice()
        } else {
            upload.packed_indices.as_slice()
        };
        let models_fallback = [[
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ]];
        let models = if upload.instance_models.is_empty() {
            models_fallback.as_slice()
        } else {
            upload.instance_models.as_slice()
        };

        self.groups_buf
            .ensure_and_write(context, storage, bytemuck::cast_slice(groups))?;
        self.clusters_buf
            .ensure_and_write(context, storage, bytemuck::cast_slice(clusters))?;
        self.vertices_buf
            .ensure_and_write(context, storage, bytemuck::cast_slice(verts))?;
        self.indices_buf
            .ensure_and_write(context, storage, bytemuck::cast_slice(tris))?;
        self.instances_buf
            .ensure_and_write(context, storage, bytemuck::cast_slice(models))?;

        let queue_bytes = (upload.clusters.len().clamp(1, MAX_DISPATCH_QUEUE_ITEMS)
            * std::mem::size_of::<VgDispatchedClusterGpu>())
            as vk::DeviceSize;
        self.hw_queue_buf
            .ensure_capacity(context, storage, queue_bytes)?;
        self.sw_queue_buf
            .ensure_capacity(context, storage, queue_bytes)?;
        self.splat_queue_buf
            .ensure_capacity(context, storage, queue_bytes)?;

        self.uploaded_signature = Some(upload.signature);
        Ok(())
    }

    fn write_descriptor_sets(&self, device: &ash::Device) {
        let target = self.target.as_ref().expect("target images initialized");
        let hzb_read_view = target.hzb_prev.view;
        let resolve_depth_view = target.depth.view;
        let ubo_info = vk::DescriptorBufferInfo::default()
            .buffer(self.ubo_buf.buffer)
            .offset(0)
            .range(std::mem::size_of::<VgFrameUbo>() as vk::DeviceSize);
        let groups_info = vk::DescriptorBufferInfo::default()
            .buffer(self.groups_buf.buffer)
            .offset(0)
            .range(vk::WHOLE_SIZE);
        let clusters_info = vk::DescriptorBufferInfo::default()
            .buffer(self.clusters_buf.buffer)
            .offset(0)
            .range(vk::WHOLE_SIZE);
        let verts_info = vk::DescriptorBufferInfo::default()
            .buffer(self.vertices_buf.buffer)
            .offset(0)
            .range(vk::WHOLE_SIZE);
        let idx_info = vk::DescriptorBufferInfo::default()
            .buffer(self.indices_buf.buffer)
            .offset(0)
            .range(vk::WHOLE_SIZE);
        let inst_info = vk::DescriptorBufferInfo::default()
            .buffer(self.instances_buf.buffer)
            .offset(0)
            .range(vk::WHOLE_SIZE);
        let hw_q_info = vk::DescriptorBufferInfo::default()
            .buffer(self.hw_queue_buf.buffer)
            .offset(0)
            .range(vk::WHOLE_SIZE);
        let sw_q_info = vk::DescriptorBufferInfo::default()
            .buffer(self.sw_queue_buf.buffer)
            .offset(0)
            .range(vk::WHOLE_SIZE);
        let splat_q_info = vk::DescriptorBufferInfo::default()
            .buffer(self.splat_queue_buf.buffer)
            .offset(0)
            .range(vk::WHOLE_SIZE);
        let cnt_info = vk::DescriptorBufferInfo::default()
            .buffer(self.counters_buf.buffer)
            .offset(0)
            .range(vk::WHOLE_SIZE);
        let fault_info = vk::DescriptorBufferInfo::default()
            .buffer(self.fault_buf.buffer)
            .offset(0)
            .range(vk::WHOLE_SIZE);
        let vis_info = vk::DescriptorBufferInfo::default()
            .buffer(self.vis_atomic_buf.buffer)
            .offset(0)
            .range(vk::WHOLE_SIZE);

        let hzb_in_img = vk::DescriptorImageInfo::default()
            .image_view(hzb_read_view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        let hzb_depth_src = vk::DescriptorImageInfo::default()
            .image_view(resolve_depth_view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        let hzb_out_img = vk::DescriptorImageInfo::default()
            .image_view(target.hzb_curr.view)
            .image_layout(vk::ImageLayout::GENERAL);
        let col_out_img = vk::DescriptorImageInfo::default()
            .image_view(target.color.view)
            .image_layout(vk::ImageLayout::GENERAL);
        let dep_out_img = vk::DescriptorImageInfo::default()
            .image_view(target.depth.view)
            .image_layout(vk::ImageLayout::GENERAL);
        let norm_out_img = vk::DescriptorImageInfo::default()
            .image_view(target.normal_roughness.view)
            .image_layout(vk::ImageLayout::GENERAL);
        let vel_out_img = vk::DescriptorImageInfo::default()
            .image_view(target.velocity.view)
            .image_layout(vk::ImageLayout::GENERAL);

        for (ds, hzb_src) in [(self.ds_cull, &hzb_in_img), (self.ds_hzb, &hzb_depth_src)] {
            let writes = [
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::VG_FRAME_BINDING)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .buffer_info(std::slice::from_ref(&ubo_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::VG_GROUPS_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&groups_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::VG_CLUSTERS_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&clusters_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::VG_PACKED_VERTICES_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&verts_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::VG_PACKED_INDICES_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&idx_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::VG_INSTANCE_MODELS_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&inst_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::VG_HW_QUEUE_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&hw_q_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::VG_SW_QUEUE_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&sw_q_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::VG_SPLAT_QUEUE_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&splat_q_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::VG_QUEUE_COUNTERS_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&cnt_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::VG_FAULT_BUFFER_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&fault_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::VG_VIS_ATOMIC_BUFFER_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&vis_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::HZB_INPUT_TEX_BINDING)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(std::slice::from_ref(hzb_src)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::HZB_OUTPUT_TEX_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                    .image_info(std::slice::from_ref(&hzb_out_img)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::OUT_COLOR_TEX_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                    .image_info(std::slice::from_ref(&col_out_img)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::OUT_DEPTH_TEX_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                    .image_info(std::slice::from_ref(&dep_out_img)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::OUT_NORMAL_ROUGHNESS_TEX_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                    .image_info(std::slice::from_ref(&norm_out_img)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::OUT_VELOCITY_TEX_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                    .image_info(std::slice::from_ref(&vel_out_img)),
            ];
            unsafe {
                device.update_descriptor_sets(&writes, &[]);
            }
        }
    }

    fn destroy(&mut self, device: &ash::Device) {
        self.ubo_buf.destroy(device);
        self.groups_buf.destroy(device);
        self.clusters_buf.destroy(device);
        self.vertices_buf.destroy(device);
        self.indices_buf.destroy(device);
        self.instances_buf.destroy(device);
        self.hw_queue_buf.destroy(device);
        self.sw_queue_buf.destroy(device);
        self.splat_queue_buf.destroy(device);
        self.counters_buf.destroy(device);
        self.fault_buf.destroy(device);
        self.vis_atomic_buf.destroy(device);
        if let Some(mut t) = self.target.take() {
            t.destroy(device);
        }
    }
}

/// Virtual geometry GPU compute render pass executing Cluster-DAG culling, atomic
/// software micro-triangle rasterization, analytic VisBuffer resolve, and HZB updates.
pub struct VirtualGeoPass {
    context: Arc<VulkanContext>,
    descriptor_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    pipe_cull: ComputePipeline,
    pipe_sw_raster: ComputePipeline,
    pipe_resolve: ComputePipeline,
    pipe_hzb_build: ComputePipeline,
    slots: [FrameSlot; 2],
    upload: Option<VgSceneUpload>,
    prev_view_proj: Option<[[f32; 4]; 4]>,
}

impl VirtualGeoPass {
    /// Creates all four Virtual Geometry compute pipelines and double-buffered GPU resources.
    pub fn new(context: Arc<VulkanContext>) -> Result<Self> {
        let device = &context.device;
        let storage_bindings = [
            bindings::VG_GROUPS_BINDING,
            bindings::VG_CLUSTERS_BINDING,
            bindings::VG_PACKED_VERTICES_BINDING,
            bindings::VG_PACKED_INDICES_BINDING,
            bindings::VG_INSTANCE_MODELS_BINDING,
            bindings::VG_HW_QUEUE_BINDING,
            bindings::VG_SW_QUEUE_BINDING,
            bindings::VG_SPLAT_QUEUE_BINDING,
            bindings::VG_QUEUE_COUNTERS_BINDING,
            bindings::VG_FAULT_BUFFER_BINDING,
            bindings::VG_VIS_ATOMIC_BUFFER_BINDING,
        ];
        let mut layout_bindings = Vec::with_capacity(18);
        layout_bindings.push(
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::VG_FRAME_BINDING)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
        );
        for b in storage_bindings {
            layout_bindings.push(
                vk::DescriptorSetLayoutBinding::default()
                    .binding(b)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::COMPUTE),
            );
        }
        layout_bindings.push(
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::HZB_INPUT_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
        );
        for b in [
            bindings::HZB_OUTPUT_TEX_BINDING,
            bindings::OUT_COLOR_TEX_BINDING,
            bindings::OUT_DEPTH_TEX_BINDING,
            bindings::OUT_NORMAL_ROUGHNESS_TEX_BINDING,
            bindings::OUT_VELOCITY_TEX_BINDING,
        ] {
            layout_bindings.push(
                vk::DescriptorSetLayoutBinding::default()
                    .binding(b)
                    .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::COMPUTE),
            );
        }

        let descriptor_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&layout_bindings),
                None,
            )
        }
        .context("VirtualGeoPass: create descriptor layout")?;

        const TOTAL_SETS: u32 = 4;
        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UNIFORM_BUFFER,
                descriptor_count: TOTAL_SETS,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_BUFFER,
                descriptor_count: TOTAL_SETS * 11,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLED_IMAGE,
                descriptor_count: TOTAL_SETS,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_IMAGE,
                descriptor_count: TOTAL_SETS * 5,
            },
        ];
        let descriptor_pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(TOTAL_SETS)
                    .pool_sizes(&pool_sizes),
                None,
            )
        }
        .context("VirtualGeoPass: create descriptor pool")?;

        let set_layouts = [descriptor_layout; TOTAL_SETS as usize];
        let sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(descriptor_pool)
                    .set_layouts(&set_layouts),
            )
        }
        .context("VirtualGeoPass: allocate descriptor sets")?;

        let push_range = [vk::PushConstantRange {
            stage_flags: vk::ShaderStageFlags::COMPUTE,
            offset: 0,
            size: bindings::PUSH_CONSTANT_SIZE,
        }];

        let build_pipe = |spv: &[u8], entry: &CStr| -> Result<ComputePipeline> {
            let module = load_shader_module(device, spv)?;
            let pipe = ComputePipeline::new(
                device,
                module,
                entry,
                std::slice::from_ref(&descriptor_layout),
                &push_range,
            );
            unsafe { device.destroy_shader_module(module, None) };
            pipe
        };

        let pipe_cull = build_pipe(VG_CULL_SPV, ENTRY_VG_CULL)?;
        let pipe_sw_raster = build_pipe(VG_SW_RASTER_SPV, ENTRY_VG_SW_RASTER)?;
        let pipe_resolve = build_pipe(VG_RESOLVE_SPV, ENTRY_VG_RESOLVE)?;
        let pipe_hzb_build = build_pipe(VG_HZB_BUILD_SPV, ENTRY_VG_HZB_BUILD)?;

        let slot0 = FrameSlot::new(&context, sets[0], sets[1])?;
        let slot1 = FrameSlot::new(&context, sets[2], sets[3])?;

        Ok(Self {
            context,
            descriptor_layout,
            descriptor_pool,
            pipe_cull,
            pipe_sw_raster,
            pipe_resolve,
            pipe_hzb_build,
            slots: [slot0, slot1],
            upload: None,
            prev_view_proj: None,
        })
    }

    /// Supplies the latest Virtual Geometry scene upload for upcoming frames.
    pub fn set_scene_upload(&mut self, upload: Option<VgSceneUpload>) {
        self.upload = upload;
    }
}

impl Drop for VirtualGeoPass {
    fn drop(&mut self) {
        let device = &self.context.device;
        unsafe {
            let _ = device.device_wait_idle();
        }
        for slot in &mut self.slots {
            slot.destroy(device);
        }
        unsafe {
            if self.descriptor_pool != vk::DescriptorPool::null() {
                device.destroy_descriptor_pool(self.descriptor_pool, None);
            }
            if self.descriptor_layout != vk::DescriptorSetLayout::null() {
                device.destroy_descriptor_set_layout(self.descriptor_layout, None);
            }
        }
    }
}

impl RenderPassNode for VirtualGeoPass {
    fn name(&self) -> &str {
        "virtual_geo"
    }

    fn setup(&mut self, graph: &mut RenderGraphBuilder, _settings: &RenderSettings) {
        graph.read_usage(ResourceUsage {
            handle: FORWARD_DEPTH_H,
            access: vk::AccessFlags::SHADER_READ,
            stage: vk::PipelineStageFlags::COMPUTE_SHADER,
            layout: vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL,
        });
        for out_h in [FORWARD_COLOR_H, FORWARD_NORMAL_H, FORWARD_VELOCITY_H] {
            graph.write_usage(ResourceUsage {
                handle: out_h,
                access: vk::AccessFlags::SHADER_WRITE,
                stage: vk::PipelineStageFlags::COMPUTE_SHADER,
                layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            });
        }
    }

    fn execute(&mut self, ctx: &RenderContext, resources: &mut GraphResources) -> Result<()> {
        if !ctx.settings.virtual_geometry_enabled
            || ctx.frame.render_mode != RenderMode::Raster
            || !ctx.frame.has_camera
        {
            return Ok(());
        }
        let Some(upload) = self
            .upload
            .as_ref()
            .filter(|u| !u.clusters.is_empty())
            .cloned()
        else {
            return Ok(());
        };

        let extent = vk::Extent2D {
            width: ctx.render_extent.width.max(1),
            height: ctx.render_extent.height.max(1),
        };
        let slot_idx = (ctx.frame_index as usize) % 2;
        let slot = &mut self.slots[slot_idx];

        let recreate = slot.target.as_ref().is_none_or(|t| t.extent != extent);
        if recreate {
            if let Some(mut old) = slot.target.take() {
                old.destroy(&self.context.device);
            }
            slot.target = Some(TargetImages::create(&self.context, extent)?);
        }

        let vis_bytes = (u64::from(extent.width) * u64::from(extent.height) * 4) as vk::DeviceSize;
        slot.vis_atomic_buf.ensure_capacity(
            &self.context,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            vis_bytes,
        )?;
        slot.upload_geometry(&self.context, &upload)?;

        let has_prev_hzb = slot.target.as_ref().is_some_and(|t| t.initialized);
        let prev_vp = self.prev_view_proj.unwrap_or(upload.cut_config.view_proj);
        let ubo = build_vg_frame_ubo(
            &upload.cut_config,
            prev_vp,
            ctx.frame.light_dir,
            ctx.frame.light_color,
            upload.groups.len() as u32,
            upload.clusters.len() as u32,
            has_prev_hzb,
        );
        slot.ubo_buf
            .write_bytes(ctx.device, bytemuck::bytes_of(&ubo))?;

        slot.write_descriptor_sets(ctx.device);
        let target = slot.target.as_mut().unwrap();

        let ds_cull = slot.ds_cull;
        let ds_hzb = slot.ds_hzb;

        unsafe {
            ctx.device
                .cmd_fill_buffer(ctx.cmd, slot.counters_buf.buffer, 0, 16, 0);
            ctx.device
                .cmd_fill_buffer(ctx.cmd, slot.vis_atomic_buf.buffer, 0, vis_bytes, 0);

            let mem_clear = vk::MemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE);
            let mut img_barriers = vec![
                image_barrier(
                    target.color.image,
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::GENERAL,
                    vk::AccessFlags::empty(),
                    vk::AccessFlags::SHADER_WRITE,
                ),
                image_barrier(
                    target.depth.image,
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::GENERAL,
                    vk::AccessFlags::empty(),
                    vk::AccessFlags::SHADER_WRITE,
                ),
                image_barrier(
                    target.normal_roughness.image,
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::GENERAL,
                    vk::AccessFlags::empty(),
                    vk::AccessFlags::SHADER_WRITE,
                ),
                image_barrier(
                    target.velocity.image,
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::GENERAL,
                    vk::AccessFlags::empty(),
                    vk::AccessFlags::SHADER_WRITE,
                ),
                image_barrier(
                    target.hzb_curr.image,
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::GENERAL,
                    vk::AccessFlags::empty(),
                    vk::AccessFlags::SHADER_WRITE,
                ),
            ];
            if !target.initialized {
                img_barriers.push(image_barrier(
                    target.hzb_prev.image,
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    vk::AccessFlags::empty(),
                    vk::AccessFlags::SHADER_READ,
                ));
            }
            ctx.device.cmd_pipeline_barrier(
                ctx.cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[mem_clear],
                &[],
                &img_barriers,
            );

            let pc = VgPushConstants {
                passMode: 0,
                width: extent.width,
                height: extent.height,
                mipLevel: 0,
            };

            ctx.device.cmd_bind_pipeline(
                ctx.cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipe_cull.pipeline,
            );
            ctx.device.cmd_bind_descriptor_sets(
                ctx.cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipe_cull.layout,
                0,
                &[ds_cull],
                &[],
            );
            ctx.device.cmd_push_constants(
                ctx.cmd,
                self.pipe_cull.layout,
                vk::ShaderStageFlags::COMPUTE,
                0,
                bytemuck::bytes_of(&pc),
            );
            let cull_groups = (upload.clusters.len() as u32).max(1).div_ceil(64);
            ctx.device.cmd_dispatch(ctx.cmd, cull_groups, 1, 1);

            let compute_sync = vk::MemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE);
            ctx.device.cmd_pipeline_barrier(
                ctx.cmd,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[compute_sync],
                &[],
                &[],
            );

            ctx.device.cmd_bind_pipeline(
                ctx.cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipe_sw_raster.pipeline,
            );
            ctx.device.cmd_bind_descriptor_sets(
                ctx.cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipe_sw_raster.layout,
                0,
                &[ds_cull],
                &[],
            );
            ctx.device.cmd_push_constants(
                ctx.cmd,
                self.pipe_sw_raster.layout,
                vk::ShaderStageFlags::COMPUTE,
                0,
                bytemuck::bytes_of(&pc),
            );
            let sw_groups = (upload.clusters.len().clamp(1, 512) as u32) * 2;
            ctx.device.cmd_dispatch(ctx.cmd, sw_groups, 1, 1);

            ctx.device.cmd_pipeline_barrier(
                ctx.cmd,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[compute_sync],
                &[],
                &[],
            );

            ctx.device.cmd_bind_pipeline(
                ctx.cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipe_resolve.pipeline,
            );
            ctx.device.cmd_bind_descriptor_sets(
                ctx.cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipe_resolve.layout,
                0,
                &[ds_cull],
                &[],
            );
            ctx.device.cmd_push_constants(
                ctx.cmd,
                self.pipe_resolve.layout,
                vk::ShaderStageFlags::COMPUTE,
                0,
                bytemuck::bytes_of(&pc),
            );
            let gx = extent.width.div_ceil(8);
            let gy = extent.height.div_ceil(8);
            ctx.device.cmd_dispatch(ctx.cmd, gx, gy, 1);

            let depth_for_hzb = image_barrier(
                target.depth.image,
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::AccessFlags::SHADER_WRITE,
                vk::AccessFlags::SHADER_READ,
            );
            ctx.device.cmd_pipeline_barrier(
                ctx.cmd,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[depth_for_hzb],
            );

            let hzb_pc = VgPushConstants {
                passMode: 1,
                width: (extent.width / 2).max(1),
                height: (extent.height / 2).max(1),
                mipLevel: 0,
            };
            ctx.device.cmd_bind_pipeline(
                ctx.cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipe_hzb_build.pipeline,
            );
            ctx.device.cmd_bind_descriptor_sets(
                ctx.cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipe_hzb_build.layout,
                0,
                &[ds_hzb],
                &[],
            );
            ctx.device.cmd_push_constants(
                ctx.cmd,
                self.pipe_hzb_build.layout,
                vk::ShaderStageFlags::COMPUTE,
                0,
                bytemuck::bytes_of(&hzb_pc),
            );
            ctx.device.cmd_dispatch(
                ctx.cmd,
                hzb_pc.width.div_ceil(8),
                hzb_pc.height.div_ceil(8),
                1,
            );

            let end_barriers = [
                image_barrier(
                    target.color.image,
                    vk::ImageLayout::GENERAL,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    vk::AccessFlags::SHADER_WRITE,
                    vk::AccessFlags::SHADER_READ,
                ),
                image_barrier(
                    target.normal_roughness.image,
                    vk::ImageLayout::GENERAL,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    vk::AccessFlags::SHADER_WRITE,
                    vk::AccessFlags::SHADER_READ,
                ),
                image_barrier(
                    target.velocity.image,
                    vk::ImageLayout::GENERAL,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    vk::AccessFlags::SHADER_WRITE,
                    vk::AccessFlags::SHADER_READ,
                ),
                image_barrier(
                    target.hzb_curr.image,
                    vk::ImageLayout::GENERAL,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    vk::AccessFlags::SHADER_WRITE,
                    vk::AccessFlags::SHADER_READ,
                ),
            ];
            ctx.device.cmd_pipeline_barrier(
                ctx.cmd,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::FRAGMENT_SHADER | vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &end_barriers,
            );
        }

        std::mem::swap(&mut target.hzb_curr, &mut target.hzb_prev);
        target.initialized = true;
        self.prev_view_proj = Some(upload.cut_config.view_proj);

        resources.set_image_view(FORWARD_COLOR_H, target.color.view);
        resources.set_image(FORWARD_COLOR_H, target.color.image);
        resources.set_image_view(FORWARD_DEPTH_H, target.depth.view);
        resources.set_image(FORWARD_DEPTH_H, target.depth.image);
        resources.set_image_view(FORWARD_NORMAL_H, target.normal_roughness.view);
        resources.set_image(FORWARD_NORMAL_H, target.normal_roughness.image);
        resources.set_image_view(FORWARD_VELOCITY_H, target.velocity.view);
        resources.set_image(FORWARD_VELOCITY_H, target.velocity.image);
        Ok(())
    }

    fn graph_info(&self) -> PassInfo {
        PassInfo {
            index: 0,
            name: self.name().to_string(),
            kind: PassKind::Forward,
            inputs: vec![FORWARD_DEPTH_H],
            outputs: vec![
                FORWARD_COLOR_H,
                FORWARD_DEPTH_H,
                FORWARD_NORMAL_H,
                FORWARD_VELOCITY_H,
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::VirtualGeometryTier;
    use crate::render_graph::{DlssMode, MobileUpscaler};
    use crate::virtual_geo::{VgClusterDesc, VgClusterGroupDesc, VgPageDesc, VgPrimitiveKind};

    fn spirv_magic(bytes: &[u8]) -> Option<u32> {
        let arr: [u8; 4] = bytes.get(0..4)?.try_into().ok()?;
        Some(u32::from_le_bytes(arr))
    }

    #[test]
    fn embedded_virtual_geo_spirv_and_reflected_bindings_are_valid() {
        for spv in [
            VG_CULL_SPV,
            VG_SW_RASTER_SPV,
            VG_RESOLVE_SPV,
            VG_HZB_BUILD_SPV,
        ] {
            assert!(spv.len() >= 20 && spv.len() % 4 == 0);
            assert_eq!(spirv_magic(spv), Some(0x0723_0203));
        }
        assert_eq!(bindings::ENTRY_VG_CULL_MAIN, "vgCullMain");
        assert_eq!(bindings::ENTRY_VG_SW_RASTER_MAIN, "vgSwRasterMain");
        assert_eq!(bindings::ENTRY_VG_RESOLVE_MAIN, "vgResolveMain");
        assert_eq!(bindings::ENTRY_VG_HZB_BUILD_MAIN, "vgHzbBuildMain");
        assert_eq!(bindings::PUSH_CONSTANT_SIZE, 16);
    }

    #[test]
    fn pack_vg_mesh_gpu_buffers_and_frame_ubo_preserve_residency_and_offsets() {
        let mesh = VgMeshDesc {
            asset_id: 99,
            root_group_index: 0,
            max_lod_level: 1,
            bounds_min: [-1.0; 3],
            bounds_max: [1.0; 3],
            pages: vec![VgPageDesc {
                page_index: 0,
                byte_offset: 64,
                byte_size: 256,
                first_cluster: 0,
                cluster_count: 2,
                lod_level: 1,
                is_resident_root: true,
            }],
            groups: vec![VgClusterGroupDesc {
                group_id: 0,
                lod_level: 1,
                primitive_kind: VgPrimitiveKind::SggxGaussianSplats,
                page_index: 0,
                child_group_first: 0,
                child_group_count: 0,
                first_cluster: 0,
                cluster_count: 2,
                parent_error: 100.0,
                self_error: 0.2,
                bounding_sphere: [0.0, 0.0, -4.0, 1.5],
            }],
            clusters: vec![
                VgClusterDesc {
                    cluster_id: 0,
                    group_id: 0,
                    lod_level: 1,
                    primitive_kind: VgPrimitiveKind::Triangles,
                    material_index: 3,
                    aabb_min: [-1.0; 3],
                    aabb_max: [1.0; 3],
                    normal_cone: [0.0, 0.0, 1.0, 0.25],
                    parent_error: 100.0,
                    self_error: 0.2,
                    geomorph_scale: 0.1,
                    vertex_count: 10,
                    triangle_count: 8,
                },
                VgClusterDesc {
                    cluster_id: 1,
                    group_id: 0,
                    lod_level: 1,
                    primitive_kind: VgPrimitiveKind::SggxGaussianSplats,
                    material_index: 5,
                    aabb_min: [-0.5; 3],
                    aabb_max: [0.5; 3],
                    normal_cone: [0.0, 1.0, 0.0, 0.9],
                    parent_error: 100.0,
                    self_error: 0.2,
                    geomorph_scale: 0.0,
                    vertex_count: 6,
                    triangle_count: 4,
                },
            ],
        };

        let mut pool = VirtualGeoPagePool::for_tier(VirtualGeometryTier::FlagshipMeshSw);
        pool.pin_root_page(crate::virtual_geo::PageKey {
            asset_id: 99,
            page_index: 0,
        });
        let cfg = VirtualGeoCutConfig {
            view_proj: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, -1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, -1.0],
                [0.0, 0.0, 0.1, 0.0],
            ],
            camera_pos: [0.0, 0.0, 0.0],
            cot_half_fovy: 1.73,
            near_plane: 0.1,
            output_extent: [1920, 1080],
            internal_extent: [1280, 720],
            dlss_mode: DlssMode::Quality,
            mobile_upscaler: MobileUpscaler::Off,
            anf_active: false,
            base_error_threshold_px: 1.0,
            geomorph_window_ratio: 0.35,
            frame_index: 1,
            hzb_reverse_z_projection: true,
        };

        let packed = pack_vg_mesh_gpu_buffers(&mesh, &pool, &cfg);
        assert_eq!(packed.groups.len(), 1);
        assert_eq!((packed.groups[0].topology[1] >> 16) & 1, 1);
        assert_eq!(packed.clusters.len(), 2);
        assert_eq!(packed.clusters[0].meta[2], 0);
        assert_eq!(packed.clusters[1].meta[2], 50);
        assert_eq!(packed.clusters[1].meta[3], 8);
        assert_eq!(packed.clusters[1].countsAndMat[2] & 0xFFFF, 5);
        assert_eq!((packed.clusters[1].countsAndMat[2] >> 16) & 0xFF, 2);

        let ubo = build_vg_frame_ubo(
            &cfg,
            cfg.view_proj,
            [0.0, 1.0, 0.0, 2.0],
            [1.0, 1.0, 1.0, 0.3],
            packed.groups.len() as u32,
            packed.clusters.len() as u32,
            true,
        );
        assert_eq!(ubo.counts, [1, 2, 1, 3]);
    }
}
