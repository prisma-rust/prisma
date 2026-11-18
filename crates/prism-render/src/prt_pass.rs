//! PRT64 + 64-tree runtime GI compute pass (`PrtPass`).
//!
//! Executes Pass 1 (`clusterEvalMain`), Pass 2 (`propagateMain`), and Pass 3
//! (`resolveVolumeMain`) of the PRT64 pipeline (`docs/prt-gi-64tree-design.md` §4-5),
//! producing a double-buffered `64^3 * 9` RGBA16F `gi_volume` sampled by
//! `ForwardPass` on the next frame (1-frame latency, matching `GtaoPass` / `RestirGiPass`).

use std::ffi::CStr;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use ash::vk;

use crate::buffer::{create_buffer, find_memory_type};
use crate::compute::ComputePipeline;
use crate::context::VulkanContext;
use crate::prt_runtime::{eval_sh64, PrtStreamingManager};
use crate::prt_volume::{f32_to_f16, PrtVolume, SH_COEFF_COUNT};
use crate::render_graph::{
    GraphResources, PassInfo, PassKind, RasterGiMode, RenderContext, RenderGraphBuilder,
    RenderMode, RenderPassNode, RenderSettings, ResourceType, PRT_GI_VOLUME_H,
};
use crate::shader::load_shader_module;
use crate::shader_bindings::prt as bindings;
use crate::shader_bindings::scene_frag::ProbeVolumeInfo;

const CLUSTER_EVAL_SPV: &[u8] = include_bytes!("../../../assets/shaders/prt_cluster_eval.comp.spv");
const PROPAGATE_SPV: &[u8] = include_bytes!("../../../assets/shaders/prt_propagate.comp.spv");
const RESOLVE_VOLUME_SPV: &[u8] =
    include_bytes!("../../../assets/shaders/prt_resolve_volume.comp.spv");

const ENTRY_CLUSTER_EVAL: &CStr = c"clusterEvalMain";
const ENTRY_PROPAGATE: &CStr = c"propagateMain";
const ENTRY_RESOLVE_VOLUME: &CStr = c"resolveVolumeMain";

/// Spatial resolution of the resolved order-2 SH GI volume (64^3).
pub const PRT_VOLUME_DIM: u32 = 64;
/// Number of order-2 SH layers stored along Z (`64 * 9 = 576`).
const PRT_VOLUME_DEPTH: u32 = PRT_VOLUME_DIM * 9;
/// Maximum dynamic lights per cluster before explicit overflow abort.
pub const MAX_DYN_LIGHTS: usize = 64;
/// Default streaming memory budget for PRT64 clusters (4 MB).
const DEFAULT_STREAMING_BUDGET_BYTES: usize = 4 * 1024 * 1024;

struct BufferAlloc {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
}

impl BufferAlloc {
    fn new_host_visible(
        context: &VulkanContext,
        size: vk::DeviceSize,
        usage: vk::BufferUsageFlags,
    ) -> Result<Self> {
        let alloc_size = size.max(16);
        let (buffer, memory) = create_buffer(
            context,
            alloc_size,
            usage,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )?;
        Ok(Self { buffer, memory })
    }

    fn write_bytes(&self, device: &ash::Device, data: &[u8]) -> Result<()> {
        self.write_bytes_at(device, 0, data)
    }

    fn write_bytes_at(
        &self,
        device: &ash::Device,
        offset: vk::DeviceSize,
        data: &[u8],
    ) -> Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        unsafe {
            let ptr = device.map_memory(
                self.memory,
                offset,
                data.len() as vk::DeviceSize,
                vk::MemoryMapFlags::empty(),
            )?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len());
            device.unmap_memory(self.memory);
        }
        Ok(())
    }

    fn read_u32(&self, device: &ash::Device) -> Result<u32> {
        let mut val = 0u32;
        unsafe {
            let ptr = device.map_memory(self.memory, 0, 4, vk::MemoryMapFlags::empty())?;
            std::ptr::copy_nonoverlapping(ptr as *const u8, &mut val as *mut u32 as *mut u8, 4);
            device.unmap_memory(self.memory);
        }
        Ok(val)
    }

    fn destroy(&self, device: &ash::Device) {
        unsafe {
            device.destroy_buffer(self.buffer, None);
            device.free_memory(self.memory, None);
        }
    }
}

struct GiVolumeTarget {
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
    info_buf: BufferAlloc,
    forward_set5: vk::DescriptorSet,
    initialized: bool,
}

impl GiVolumeTarget {
    fn new(
        context: &VulkanContext,
        forward_pool: vk::DescriptorPool,
        forward_set5_layout: vk::DescriptorSetLayout,
    ) -> Result<Self> {
        let device = &context.device;
        let image = unsafe {
            device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_3D)
                    .format(vk::Format::R16G16B16A16_SFLOAT)
                    .extent(vk::Extent3D {
                        width: PRT_VOLUME_DIM,
                        height: PRT_VOLUME_DIM,
                        depth: PRT_VOLUME_DEPTH,
                    })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::SAMPLED)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE)
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )
        }
        .context("PrtPass: create 3D gi_volume image")?;

        let reqs = unsafe { device.get_image_memory_requirements(image) };
        let mem_type = find_memory_type(
            context,
            reqs.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .context("PrtPass: find memory type for gi_volume")?;
        let memory = unsafe {
            device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(reqs.size)
                    .memory_type_index(mem_type),
                None,
            )
        }
        .context("PrtPass: allocate gi_volume memory")?;
        unsafe { device.bind_image_memory(image, memory, 0) }
            .context("PrtPass: bind gi_volume memory")?;

        let view = unsafe {
            device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_3D)
                    .format(vk::Format::R16G16B16A16_SFLOAT)
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
        .context("PrtPass: create gi_volume view")?;

        let info_buf = BufferAlloc::new_host_visible(
            context,
            std::mem::size_of::<ProbeVolumeInfo>() as vk::DeviceSize,
            vk::BufferUsageFlags::UNIFORM_BUFFER,
        )?;

        let forward_set5 = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(forward_pool)
                    .set_layouts(std::slice::from_ref(&forward_set5_layout)),
            )
        }
        .context("PrtPass: allocate forward set5 descriptor")?[0];

        let img_info = vk::DescriptorImageInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::GENERAL);
        let buf_info = vk::DescriptorBufferInfo::default()
            .buffer(info_buf.buffer)
            .offset(0)
            .range(std::mem::size_of::<ProbeVolumeInfo>() as vk::DeviceSize);
        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(forward_set5)
                .dst_binding(crate::shader_bindings::scene_frag::PROBE_VOLUME_BINDING)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(std::slice::from_ref(&img_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(forward_set5)
                .dst_binding(crate::shader_bindings::scene_frag::PROBE_INFO_BINDING)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .buffer_info(std::slice::from_ref(&buf_info)),
        ];
        unsafe { device.update_descriptor_sets(&writes, &[]) };

        Ok(Self {
            image,
            memory,
            view,
            info_buf,
            forward_set5,
            initialized: false,
        })
    }

    fn destroy(&self, device: &ash::Device) {
        self.info_buf.destroy(device);
        unsafe {
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
            device.free_memory(self.memory, None);
        }
    }
}

struct GpuState {
    context: Arc<VulkanContext>,
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    descriptor_sets: [vk::DescriptorSet; 2],
    cluster_eval_pipe: ComputePipeline,
    propagate_pipe: ComputePipeline,
    resolve_pipe: ComputePipeline,
    forward_set5_layout: vk::DescriptorSetLayout,
    forward_pool: vk::DescriptorPool,
    targets: [GiVolumeTarget; 2],
    // SceneScope-like PRT volume buffers (rebuilt on volume change).
    nodes_buf: Option<BufferAlloc>,
    clusters_buf: Option<BufferAlloc>,
    valid_buf: Option<BufferAlloc>,
    leaf_bounds_buf: Option<BufferAlloc>,
    cluster_radiance_buf: Option<BufferAlloc>,
    dyn_accum_buf: Option<BufferAlloc>,
    overflow_flag_buf: BufferAlloc,
    dyn_lights_buf: BufferAlloc,
    sky_sh64_buf: BufferAlloc,
    frame_ubo_buf: BufferAlloc,
    leaf_count: u32,
    node_count: u32,
    leaf_grid: u32,
    world_origin: [f32; 3],
    world_extent: f32,
}

impl GpuState {
    fn new(context: Arc<VulkanContext>) -> Result<Self> {
        let device = &context.device;
        let bindings_layout = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::PRT_NODES_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::PRT_CLUSTERS_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::PRT_CLUSTER_VALID_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::PRT_LEAF_BOUNDS_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::CLUSTER_RADIANCE_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::DYN_ACCUM_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::OVERFLOW_FLAG_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::DYN_LIGHTS_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::PRT_FRAME_BINDING)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::GI_VOLUME_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::SKY_SH64_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
        ];

        let set_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings_layout),
                None,
            )
        }
        .context("PrtPass: create compute descriptor set layout")?;

        let pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(2)
                    .pool_sizes(&[
                        vk::DescriptorPoolSize {
                            ty: vk::DescriptorType::STORAGE_BUFFER,
                            descriptor_count: 18,
                        },
                        vk::DescriptorPoolSize {
                            ty: vk::DescriptorType::UNIFORM_BUFFER,
                            descriptor_count: 2,
                        },
                        vk::DescriptorPoolSize {
                            ty: vk::DescriptorType::STORAGE_IMAGE,
                            descriptor_count: 2,
                        },
                    ]),
                None,
            )
        }
        .context("PrtPass: create compute descriptor pool")?;

        let layouts = [set_layout, set_layout];
        let allocated = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(pool)
                    .set_layouts(&layouts),
            )
        }
        .context("PrtPass: allocate compute descriptor sets")?;
        let descriptor_sets = [allocated[0], allocated[1]];

        let m_eval = load_shader_module(device, CLUSTER_EVAL_SPV)?;
        let m_prop = load_shader_module(device, PROPAGATE_SPV)?;
        let m_res = load_shader_module(device, RESOLVE_VOLUME_SPV)?;

        let cluster_eval_pipe =
            ComputePipeline::new(device, m_eval, ENTRY_CLUSTER_EVAL, &[set_layout], &[]);
        let propagate_pipe =
            ComputePipeline::new(device, m_prop, ENTRY_PROPAGATE, &[set_layout], &[]);
        let resolve_pipe =
            ComputePipeline::new(device, m_res, ENTRY_RESOLVE_VOLUME, &[set_layout], &[]);

        unsafe {
            device.destroy_shader_module(m_eval, None);
            device.destroy_shader_module(m_prop, None);
            device.destroy_shader_module(m_res, None);
        }

        let cluster_eval_pipe = cluster_eval_pipe?;
        let propagate_pipe = propagate_pipe?;
        let resolve_pipe = resolve_pipe?;

        // ForwardPass-compatible set 5 layout (SAMPLED_IMAGE + UNIFORM_BUFFER).
        let fwd_bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(crate::shader_bindings::scene_frag::PROBE_VOLUME_BINDING)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(crate::shader_bindings::scene_frag::PROBE_INFO_BINDING)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        let forward_set5_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&fwd_bindings),
                None,
            )
        }
        .context("PrtPass: create forward set5 layout")?;

        let forward_pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(2)
                    .pool_sizes(&[
                        vk::DescriptorPoolSize {
                            ty: vk::DescriptorType::SAMPLED_IMAGE,
                            descriptor_count: 2,
                        },
                        vk::DescriptorPoolSize {
                            ty: vk::DescriptorType::UNIFORM_BUFFER,
                            descriptor_count: 2,
                        },
                    ]),
                None,
            )
        }
        .context("PrtPass: create forward set5 pool")?;

        let t0 = GiVolumeTarget::new(&context, forward_pool, forward_set5_layout)?;
        let t1 = GiVolumeTarget::new(&context, forward_pool, forward_set5_layout)?;

        let overflow_flag_buf =
            BufferAlloc::new_host_visible(&context, 4, vk::BufferUsageFlags::STORAGE_BUFFER)?;
        let dyn_lights_buf = BufferAlloc::new_host_visible(
            &context,
            (MAX_DYN_LIGHTS * std::mem::size_of::<bindings::PrtDynamicLightGpu>())
                as vk::DeviceSize,
            vk::BufferUsageFlags::STORAGE_BUFFER,
        )?;
        let sky_sh64_buf = BufferAlloc::new_host_visible(
            &context,
            (SH_COEFF_COUNT * std::mem::size_of::<[f32; 4]>()) as vk::DeviceSize,
            vk::BufferUsageFlags::STORAGE_BUFFER,
        )?;
        let frame_ubo_buf = BufferAlloc::new_host_visible(
            &context,
            std::mem::size_of::<bindings::PrtFrameUbo>() as vk::DeviceSize,
            vk::BufferUsageFlags::UNIFORM_BUFFER,
        )?;

        Ok(Self {
            context,
            set_layout,
            pool,
            descriptor_sets,
            cluster_eval_pipe,
            propagate_pipe,
            resolve_pipe,
            forward_set5_layout,
            forward_pool,
            targets: [t0, t1],
            nodes_buf: None,
            clusters_buf: None,
            valid_buf: None,
            leaf_bounds_buf: None,
            cluster_radiance_buf: None,
            dyn_accum_buf: None,
            overflow_flag_buf,
            dyn_lights_buf,
            sky_sh64_buf,
            frame_ubo_buf,
            leaf_count: 0,
            node_count: 0,
            leaf_grid: 1,
            world_origin: [0.0; 3],
            world_extent: 0.0,
        })
    }

    fn upload_volume(&mut self, volume: &PrtVolume) -> Result<()> {
        let device = &self.context.device;
        unsafe {
            let _ = device.device_wait_idle();
        }

        for b in [
            self.nodes_buf.take(),
            self.clusters_buf.take(),
            self.valid_buf.take(),
            self.leaf_bounds_buf.take(),
            self.cluster_radiance_buf.take(),
            self.dyn_accum_buf.take(),
        ]
        .into_iter()
        .flatten()
        {
            b.destroy(device);
        }

        let gpu_nodes: Vec<bindings::PrtNodeGpu> = volume
            .nodes
            .iter()
            .map(|n| bindings::PrtNodeGpu {
                first_child: n.first_child,
                leaf_index: n.leaf_index,
                child_mask: [n.child_mask as u32, (n.child_mask >> 32) as u32],
            })
            .collect();

        // Pack 64 coeffs * 3 f16 = 192 u16 -> 96 u32 words per cluster.
        let mut cluster_words = Vec::with_capacity(volume.clusters.len() * 96);
        for cluster in &volume.clusters {
            let mut halves = [0u16; SH_COEFF_COUNT * 3];
            for (c, rgb) in cluster.iter().enumerate() {
                halves[c * 3] = f32_to_f16(rgb[0]);
                halves[c * 3 + 1] = f32_to_f16(rgb[1]);
                halves[c * 3 + 2] = f32_to_f16(rgb[2]);
            }
            for pair in halves.as_chunks::<2>().0 {
                cluster_words.push((pair[0] as u32) | ((pair[1] as u32) << 16));
            }
        }

        let valid_words: Vec<u32> = match &volume.cluster_valid {
            Some(v) => v.iter().map(|&b| u32::from(b)).collect(),
            None => vec![1u32; volume.clusters.len()],
        };

        let leaf_bounds = compute_leaf_bounds(volume);

        let leaf_count = volume.clusters.len();
        let nodes_buf = BufferAlloc::new_host_visible(
            &self.context,
            std::mem::size_of_val(gpu_nodes.as_slice()) as vk::DeviceSize,
            vk::BufferUsageFlags::STORAGE_BUFFER,
        )?;
        nodes_buf.write_bytes(device, bytemuck::cast_slice(&gpu_nodes))?;

        let clusters_buf = BufferAlloc::new_host_visible(
            &self.context,
            std::mem::size_of_val(cluster_words.as_slice()) as vk::DeviceSize,
            vk::BufferUsageFlags::STORAGE_BUFFER,
        )?;
        clusters_buf.write_bytes(device, bytemuck::cast_slice(&cluster_words))?;

        let valid_buf = BufferAlloc::new_host_visible(
            &self.context,
            std::mem::size_of_val(valid_words.as_slice()) as vk::DeviceSize,
            vk::BufferUsageFlags::STORAGE_BUFFER,
        )?;
        valid_buf.write_bytes(device, bytemuck::cast_slice(&valid_words))?;

        let leaf_bounds_buf = BufferAlloc::new_host_visible(
            &self.context,
            std::mem::size_of_val(leaf_bounds.as_slice()) as vk::DeviceSize,
            vk::BufferUsageFlags::STORAGE_BUFFER,
        )?;
        leaf_bounds_buf.write_bytes(device, bytemuck::cast_slice(&leaf_bounds))?;

        let cluster_radiance_buf = BufferAlloc::new_host_visible(
            &self.context,
            (leaf_count.max(1) * 8) as vk::DeviceSize,
            vk::BufferUsageFlags::STORAGE_BUFFER,
        )?;
        let dyn_accum_buf = BufferAlloc::new_host_visible(
            &self.context,
            (leaf_count.max(1) * 12) as vk::DeviceSize,
            vk::BufferUsageFlags::STORAGE_BUFFER,
        )?;

        self.leaf_count = leaf_count as u32;
        self.node_count = volume.nodes.len() as u32;
        self.leaf_grid = volume.leaf_grid;
        self.world_origin = volume.world_origin;
        self.world_extent = volume.world_extent;

        self.nodes_buf = Some(nodes_buf);
        self.clusters_buf = Some(clusters_buf);
        self.valid_buf = Some(valid_buf);
        self.leaf_bounds_buf = Some(leaf_bounds_buf);
        self.cluster_radiance_buf = Some(cluster_radiance_buf);
        self.dyn_accum_buf = Some(dyn_accum_buf);

        // Update ProbeVolumeInfo UBOs for both ping-pong targets.
        let spacing = volume.world_extent / (PRT_VOLUME_DIM.saturating_sub(1).max(1) as f32);
        let probe_info = ProbeVolumeInfo {
            origin: [
                volume.world_origin[0],
                volume.world_origin[1],
                volume.world_origin[2],
                0.0,
            ],
            spacing: [spacing, spacing, spacing, 0.0],
            dims: [PRT_VOLUME_DIM, PRT_VOLUME_DIM, PRT_VOLUME_DIM, 0],
        };
        for t in &self.targets {
            t.info_buf
                .write_bytes(device, bytemuck::bytes_of(&probe_info))?;
        }

        // Bind descriptors for both compute sets (targeting targets[0] and targets[1]).
        for (idx, &ds) in self.descriptor_sets.iter().enumerate() {
            let n_info = vk::DescriptorBufferInfo::default()
                .buffer(self.nodes_buf.as_ref().unwrap().buffer)
                .range(vk::WHOLE_SIZE);
            let c_info = vk::DescriptorBufferInfo::default()
                .buffer(self.clusters_buf.as_ref().unwrap().buffer)
                .range(vk::WHOLE_SIZE);
            let v_info = vk::DescriptorBufferInfo::default()
                .buffer(self.valid_buf.as_ref().unwrap().buffer)
                .range(vk::WHOLE_SIZE);
            let lb_info = vk::DescriptorBufferInfo::default()
                .buffer(self.leaf_bounds_buf.as_ref().unwrap().buffer)
                .range(vk::WHOLE_SIZE);
            let cr_info = vk::DescriptorBufferInfo::default()
                .buffer(self.cluster_radiance_buf.as_ref().unwrap().buffer)
                .range(vk::WHOLE_SIZE);
            let da_info = vk::DescriptorBufferInfo::default()
                .buffer(self.dyn_accum_buf.as_ref().unwrap().buffer)
                .range(vk::WHOLE_SIZE);
            let ov_info = vk::DescriptorBufferInfo::default()
                .buffer(self.overflow_flag_buf.buffer)
                .range(vk::WHOLE_SIZE);
            let dl_info = vk::DescriptorBufferInfo::default()
                .buffer(self.dyn_lights_buf.buffer)
                .range(vk::WHOLE_SIZE);
            let ubo_info = vk::DescriptorBufferInfo::default()
                .buffer(self.frame_ubo_buf.buffer)
                .range(vk::WHOLE_SIZE);
            let img_info = vk::DescriptorImageInfo::default()
                .image_view(self.targets[idx].view)
                .image_layout(vk::ImageLayout::GENERAL);
            let sky_info = vk::DescriptorBufferInfo::default()
                .buffer(self.sky_sh64_buf.buffer)
                .range(vk::WHOLE_SIZE);

            let writes = [
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::PRT_NODES_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&n_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::PRT_CLUSTERS_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&c_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::PRT_CLUSTER_VALID_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&v_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::PRT_LEAF_BOUNDS_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&lb_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::CLUSTER_RADIANCE_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&cr_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::DYN_ACCUM_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&da_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::OVERFLOW_FLAG_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&ov_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::DYN_LIGHTS_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&dl_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::PRT_FRAME_BINDING)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .buffer_info(std::slice::from_ref(&ubo_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::GI_VOLUME_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                    .image_info(std::slice::from_ref(&img_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(ds)
                    .dst_binding(bindings::SKY_SH64_BINDING)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&sky_info)),
            ];
            unsafe { device.update_descriptor_sets(&writes, &[]) };
        }

        Ok(())
    }

    fn destroy(&self) {
        let device = &self.context.device;
        unsafe {
            let _ = device.device_wait_idle();
        }
        for b in [
            &self.nodes_buf,
            &self.clusters_buf,
            &self.valid_buf,
            &self.leaf_bounds_buf,
            &self.cluster_radiance_buf,
            &self.dyn_accum_buf,
        ]
        .into_iter()
        .flatten()
        {
            b.destroy(device);
        }
        self.overflow_flag_buf.destroy(device);
        self.dyn_lights_buf.destroy(device);
        self.sky_sh64_buf.destroy(device);
        self.frame_ubo_buf.destroy(device);
        for t in &self.targets {
            t.destroy(device);
        }
        unsafe {
            device.destroy_descriptor_pool(self.forward_pool, None);
            device.destroy_descriptor_set_layout(self.forward_set5_layout, None);
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
        }
    }
}

fn compute_leaf_bounds(volume: &PrtVolume) -> Vec<[f32; 4]> {
    let mut bounds = vec![
        [
            volume.world_origin[0] + volume.world_extent * 0.5,
            volume.world_origin[1] + volume.world_extent * 0.5,
            volume.world_origin[2] + volume.world_extent * 0.5,
            volume.world_extent * 0.5,
        ];
        volume.clusters.len().max(1)
    ];
    if volume.nodes.is_empty() || volume.clusters.is_empty() {
        return bounds;
    }

    fn walk(
        volume: &PrtVolume,
        node_idx: usize,
        origin: [f32; 3],
        extent: f32,
        out: &mut [[f32; 4]],
    ) {
        let Some(node) = volume.nodes.get(node_idx) else {
            return;
        };
        if node.is_leaf() {
            if let Some(slot) = out.get_mut(node.leaf_index as usize) {
                let half = extent * 0.5;
                *slot = [origin[0] + half, origin[1] + half, origin[2] + half, half];
            }
            return;
        }
        let child_extent = extent * 0.25;
        let mut child_cursor = node.first_child as usize;
        for slot in 0..64u32 {
            if !node.has_child(slot) {
                continue;
            }
            let cx = (slot & 3) as f32;
            let cy = ((slot >> 2) & 3) as f32;
            let cz = ((slot >> 4) & 3) as f32;
            walk(
                volume,
                child_cursor,
                [
                    origin[0] + cx * child_extent,
                    origin[1] + cy * child_extent,
                    origin[2] + cz * child_extent,
                ],
                child_extent,
                out,
            );
            child_cursor += 1;
        }
    }

    walk(
        volume,
        0,
        volume.world_origin,
        volume.world_extent,
        &mut bounds,
    );
    bounds
}

/// Render Graph pass for PRT64 + 64-tree GI (`GiMode::Prt`).
pub struct PrtPass {
    context: Arc<VulkanContext>,
    gpu: Option<GpuState>,
    pending_volume: Option<PrtVolume>,
    streaming: Option<PrtStreamingManager>,
    cpu_clusters: Vec<[u32; 96]>,
    cpu_valid: Vec<u32>,
    has_written_once: bool,
}

impl PrtPass {
    pub fn new(context: Arc<VulkanContext>) -> Self {
        Self {
            context,
            gpu: None,
            pending_volume: None,
            streaming: None,
            cpu_clusters: Vec::new(),
            cpu_valid: Vec::new(),
            has_written_once: false,
        }
    }

    /// Stage a decoded `PrtVolume` for GPU upload on the next frame.
    pub fn load_volume(&mut self, volume: PrtVolume) {
        let leaf_count = volume.clusters.len();
        self.cpu_clusters = volume
            .clusters
            .iter()
            .map(|cluster| {
                let mut halves = [0u16; SH_COEFF_COUNT * 3];
                for (c, rgb) in cluster.iter().enumerate() {
                    halves[c * 3] = f32_to_f16(rgb[0]);
                    halves[c * 3 + 1] = f32_to_f16(rgb[1]);
                    halves[c * 3 + 2] = f32_to_f16(rgb[2]);
                }
                let mut words = [0u32; 96];
                for (dst, pair) in words.iter_mut().zip(halves.as_chunks::<2>().0) {
                    *dst = (pair[0] as u32) | ((pair[1] as u32) << 16);
                }
                words
            })
            .collect();
        self.cpu_valid = match &volume.cluster_valid {
            Some(v) => v.iter().map(|&b| u32::from(b)).collect(),
            None => vec![1u32; leaf_count],
        };
        self.streaming = Some(PrtStreamingManager::new(
            leaf_count,
            DEFAULT_STREAMING_BUDGET_BYTES,
            leaf_count.clamp(1, 256),
        ));
        if let Some(mgr) = &mut self.streaming {
            for i in 0..leaf_count {
                mgr.request_leaf(i);
            }
        }
        self.pending_volume = Some(volume);
        self.has_written_once = false;
    }

    /// Previous-frame `set 5` descriptor set (`gi_volume` + `ProbeVolumeInfo`)
    /// for `ForwardPass` consumption (1-frame latency).
    pub fn gi_volume_descriptor_set(&self, frame_index: u32) -> Option<vk::DescriptorSet> {
        if !self.has_written_once {
            return None;
        }
        let gpu = self.gpu.as_ref()?;
        if gpu.leaf_count == 0 {
            return None;
        }
        let prev = ((frame_index + 1) % 2) as usize;
        gpu.targets[prev]
            .initialized
            .then_some(gpu.targets[prev].forward_set5)
    }

    pub fn streaming_stats(&self) -> Option<crate::prt_runtime::PrtStreamingStats> {
        self.streaming.as_ref().map(|s| s.stats())
    }
}

impl Drop for PrtPass {
    fn drop(&mut self) {
        if let Some(gpu) = self.gpu.take() {
            gpu.destroy();
        }
    }
}

impl RenderPassNode for PrtPass {
    fn name(&self) -> &str {
        "PrtPass"
    }

    fn graph_info(&self) -> PassInfo {
        PassInfo {
            index: usize::MAX,
            name: self.name().to_string(),
            kind: PassKind::Gtao,
            inputs: Vec::new(),
            outputs: vec![PRT_GI_VOLUME_H],
        }
    }

    fn setup(&mut self, graph: &mut RenderGraphBuilder, _settings: &RenderSettings) {
        graph.create_resource_at(
            PRT_GI_VOLUME_H,
            ResourceType::StorageImage {
                format: vk::Format::R16G16B16A16_SFLOAT,
                extent: vk::Extent3D {
                    width: PRT_VOLUME_DIM,
                    height: PRT_VOLUME_DIM,
                    depth: PRT_VOLUME_DEPTH,
                },
            },
        );
    }

    fn execute(&mut self, ctx: &RenderContext, _resources: &mut GraphResources) -> Result<()> {
        if ctx.frame.render_mode != RenderMode::Raster
            || ctx.settings.raster_gi != RasterGiMode::Prt
        {
            return Ok(());
        }

        if self.gpu.is_none() {
            self.gpu = Some(GpuState::new(self.context.clone())?);
        }
        let gpu = self.gpu.as_mut().unwrap();

        if let Some(vol) = self.pending_volume.take() {
            gpu.upload_volume(&vol)?;
        }

        if gpu.leaf_count == 0 {
            return Ok(());
        }

        // Stream newly resident / evicted cluster slices within the frame budget
        // (zero mid-frame vkFreeMemory; pre-allocated cluster buffer).
        if let Some(mgr) = &mut self.streaming {
            let (_stats, uploaded, evicted) = mgr.step_frame_deltas();
            if let (Some(cbuf), Some(vbuf)) = (&gpu.clusters_buf, &gpu.valid_buf) {
                for leaf_idx in evicted {
                    let zero = 0u32;
                    vbuf.write_bytes_at(
                        ctx.device,
                        (leaf_idx * 4) as vk::DeviceSize,
                        bytemuck::bytes_of(&zero),
                    )?;
                }
                for leaf_idx in uploaded {
                    if let Some(words) = self.cpu_clusters.get(leaf_idx) {
                        cbuf.write_bytes_at(
                            ctx.device,
                            (leaf_idx * 384) as vk::DeviceSize,
                            bytemuck::cast_slice(words),
                        )?;
                    }
                    let valid = self.cpu_valid.get(leaf_idx).copied().unwrap_or(1);
                    vbuf.write_bytes_at(
                        ctx.device,
                        (leaf_idx * 4) as vk::DeviceSize,
                        bytemuck::bytes_of(&valid),
                    )?;
                }
            }
        }

        // Check overflow flag from previous frame (explicit error on >64 lights).
        if self.has_written_once && gpu.overflow_flag_buf.read_u32(ctx.device)? != 0 {
            log::error!(
                "PrtPass: dynamic light count exceeded {} per cluster; dynamic propagation aborted",
                MAX_DYN_LIGHTS
            );
        }

        // Update sky/sun SH64 buffer from the directional light + ambient sky.
        let sun_dir = ctx.frame.light_dir;
        let sun_col = ctx.frame.light_color;
        let sun_sh = eval_sh64([sun_dir[0], sun_dir[1], sun_dir[2]]);
        let mut sky_sh64 = [[0.0f32; 4]; SH_COEFF_COUNT];
        for i in 0..SH_COEFF_COUNT {
            let w = sun_sh[i];
            sky_sh64[i] = [sun_col[0] * w, sun_col[1] * w, sun_col[2] * w, 0.0];
        }
        // Baseline sky fill from ambient term in light_color.w.
        let amb = sun_col[3].max(0.05);
        sky_sh64[0][0] += amb * 3.544908;
        sky_sh64[0][1] += amb * 3.544908;
        sky_sh64[0][2] += amb * 3.544908;
        gpu.sky_sh64_buf
            .write_bytes(ctx.device, bytemuck::cast_slice(&sky_sh64))?;

        // Update dynamic lights buffer (point/spot lights).
        let mut dyn_lights = Vec::with_capacity(ctx.frame.lights.len().min(MAX_DYN_LIGHTS));
        for l in ctx.frame.lights.iter().take(MAX_DYN_LIGHTS) {
            let intensity = l.color[3];
            dyn_lights.push(bindings::PrtDynamicLightGpu {
                position_radius: [
                    l.position[0],
                    l.position[1],
                    l.position[2],
                    l.position[3].max(0.1),
                ],
                color_intensity: [
                    l.color[0] * intensity,
                    l.color[1] * intensity,
                    l.color[2] * intensity,
                    0.0,
                ],
            });
        }
        if !dyn_lights.is_empty() {
            gpu.dyn_lights_buf
                .write_bytes(ctx.device, bytemuck::cast_slice(&dyn_lights))?;
        }

        let spacing = gpu.world_extent / (PRT_VOLUME_DIM.saturating_sub(1).max(1) as f32);
        let ubo = bindings::PrtFrameUbo {
            world_origin_ext: [
                gpu.world_origin[0],
                gpu.world_origin[1],
                gpu.world_origin[2],
                gpu.world_extent,
            ],
            counts: [
                gpu.leaf_count,
                gpu.node_count,
                gpu.leaf_grid,
                ctx.frame.lights.len() as u32,
            ],
            grid_origin: [
                gpu.world_origin[0],
                gpu.world_origin[1],
                gpu.world_origin[2],
                0.0,
            ],
            grid_spacing: [spacing, spacing, spacing, 0.0],
            grid_dims: [PRT_VOLUME_DIM, PRT_VOLUME_DIM, PRT_VOLUME_DIM, 0],
        };
        gpu.frame_ubo_buf
            .write_bytes(ctx.device, bytemuck::bytes_of(&ubo))?;

        let cur = (ctx.frame_index % 2) as usize;
        let ds = gpu.descriptor_sets[cur];
        let target = &mut gpu.targets[cur];

        // Transition gi_volume target to GENERAL on first use.
        if !target.initialized {
            let barrier = vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::SHADER_WRITE)
                .image(target.image)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                });
            unsafe {
                ctx.device.cmd_pipeline_barrier(
                    ctx.cmd,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[barrier],
                );
            }
            target.initialized = true;
        }

        let mem_barrier = vk::MemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::SHADER_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE);

        unsafe {
            // Pass 1: ClusterEval
            ctx.device.cmd_bind_pipeline(
                ctx.cmd,
                vk::PipelineBindPoint::COMPUTE,
                gpu.cluster_eval_pipe.pipeline,
            );
            ctx.device.cmd_bind_descriptor_sets(
                ctx.cmd,
                vk::PipelineBindPoint::COMPUTE,
                gpu.cluster_eval_pipe.layout,
                0,
                &[ds],
                &[],
            );
            ctx.device.cmd_dispatch(ctx.cmd, gpu.leaf_count, 1, 1);

            ctx.device.cmd_pipeline_barrier(
                ctx.cmd,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[mem_barrier],
                &[],
                &[],
            );

            // Pass 2: Propagate dynamic lights (Q8.12 InterlockedAdd)
            ctx.device.cmd_bind_pipeline(
                ctx.cmd,
                vk::PipelineBindPoint::COMPUTE,
                gpu.propagate_pipe.pipeline,
            );
            ctx.device.cmd_dispatch(ctx.cmd, gpu.leaf_count, 1, 1);

            ctx.device.cmd_pipeline_barrier(
                ctx.cmd,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[mem_barrier],
                &[],
                &[],
            );

            // Pass 3: ResolveVolume into 64^3 order-2 SH texture
            ctx.device.cmd_bind_pipeline(
                ctx.cmd,
                vk::PipelineBindPoint::COMPUTE,
                gpu.resolve_pipe.pipeline,
            );
            let wg = PRT_VOLUME_DIM / 4;
            ctx.device.cmd_dispatch(ctx.cmd, wg, wg, wg);

            // Make gi_volume writes visible to next frame's ForwardPass fragment reads.
            let frag_barrier = vk::MemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ);
            ctx.device.cmd_pipeline_barrier(
                ctx.cmd,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[frag_barrier],
                &[],
                &[],
            );
        }

        self.has_written_once = true;
        Ok(())
    }
}
