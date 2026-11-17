//! Compute DDA pass rendering sparse three-tier voxel chunks (`L1 -> L2 -> L3`).
//!
//! Reads `FORWARD_COLOR_H` and `FORWARD_DEPTH_H`, raymarches active [`VoxelChunkUpload`]s
//! against scene geometry, shades direct sun + shadows + ambient + transient visual states,
//! draws the optional 3D cursor wireframe, and publishes `VOXEL_COLOR_H` and `VOXEL_DEPTH_H`.
//! When no voxel chunks are active, the pass is a zero-cost no-op that leaves
//! `VOXEL_COLOR_H` unpublished so downstream passes consume `FORWARD_COLOR_H` unchanged.

use std::ffi::CStr;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use ash::vk;

use crate::buffer::{create_buffer, find_memory_type};
use crate::compute::ComputePipeline;
use crate::context::VulkanContext;
use crate::render_graph::{
    GraphResources, PassInfo, PassKind, RenderContext, RenderGraphBuilder, RenderMode,
    RenderPassNode, RenderSettings, ResourceUsage, FORWARD_COLOR_H, FORWARD_DEPTH_H, VOXEL_COLOR_H,
    VOXEL_DEPTH_H,
};
use crate::shader::load_shader_module;
use crate::shader_bindings::voxel_dda::{self as bindings, VoxelChunkGpu, VoxelFrameUbo};

const VOXEL_DDA_SPV: &[u8] = include_bytes!("../../../assets/shaders/voxel_dda.comp.spv");
const ENTRY_VOXEL_DDA: &CStr = c"voxelDdaMain";

/// CPU-extracted chunk buffers ready to upload into GPU SSBOs.
#[derive(Clone, Debug)]
pub struct VoxelChunkUpload {
    pub entity_id: u64,
    pub version: u64,
    pub origin_l1: [i32; 3],
    pub dims_l1: [u32; 3],
    /// `[brick_ref, uniform_rgba, uniform_visual, 0]` per L1 cell.
    pub l1_cells: Vec<[u32; 4]>,
    /// `[occ_lo, occ_hi]` per allocated L2 brick.
    pub brick_occ: Vec<[u32; 2]>,
    /// `[rgba, visual, l3_lo, l3_hi]` (`64` entries per brick).
    pub brick_cells: Vec<[u32; 4]>,
}

/// Optional 3D wireframe box highlighted by the compute pass.
#[derive(Clone, Copy, Debug)]
pub struct VoxelCursorUpload {
    pub active: bool,
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub color: [f32; 4],
}

/// Aggregated voxel scene payload passed from `FramePacket` into `GraphRenderer`.
#[derive(Clone, Debug, Default)]
pub struct VoxelSceneUpload {
    pub signature: u64,
    pub elapsed_seconds: f32,
    pub chunks: Vec<VoxelChunkUpload>,
    pub cursor: Option<VoxelCursorUpload>,
    /// Optional 256x48 SPBR LabPBR atlas (16 tiles x 3 rows of 16x16 RGBA8 packed `u32`).
    pub atlas: Option<Arc<Vec<u32>>>,
}

/// Loads the 16-tile (256x48) SPBR LabPBR atlas from `<assets_root>/textures/blocks/` if present.
///
/// Row 0 (`y = 0..16`): albedo `.png`
/// Row 1 (`y = 16..32`): normal `_n.png`
/// Row 2 (`y = 32..48`): LabPBR specular `_s.png`
pub fn load_spbr_atlas_from_assets(assets_root: &std::path::Path) -> Option<Arc<Vec<u32>>> {
    let blocks_dir = assets_root.join("textures").join("blocks");
    if !blocks_dir.is_dir() {
        return None;
    }
    const TILES: [&str; 16] = [
        "bedrock",
        "dirt",
        "grass_block_top",
        "stone",
        "iron_block",
        "glass_pane_top",
        "oak_planks",
        "water_still",
        "emerald_block",
        "ice",
        "amethyst_block",
        "copper_block",
        "iron_block",
        "redstone_ore",
        "emerald_block",
        "grass_block_side",
    ];
    let mut pixels = vec![0u32; 256 * 48];
    let mut loaded_any = false;

    for (tile_idx, name) in TILES.iter().enumerate() {
        let suffixes = [("", 0usize), ("_n", 16usize), ("_s", 32usize)];
        for (suffix, row_y) in suffixes {
            let path = blocks_dir.join(format!("{name}{suffix}.png"));
            let Ok(img) = image::open(&path) else {
                continue;
            };
            loaded_any = true;
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            for py in 0..16u32 {
                let sy = (py * h) / 16;
                for px in 0..16u32 {
                    let sx = (px * w) / 16;
                    let p = rgba.get_pixel(sx, sy).0;
                    let packed = (p[0] as u32)
                        | ((p[1] as u32) << 8)
                        | ((p[2] as u32) << 16)
                        | ((p[3] as u32) << 24);
                    pixels[(row_y + py as usize) * 256 + tile_idx * 16 + px as usize] = packed;
                }
            }
        }
    }

    if loaded_any {
        Some(Arc::new(pixels))
    } else {
        None
    }
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
        let alloc_size = size.max(64);
        let (buffer, memory) = create_buffer(
            context,
            alloc_size,
            usage,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )?;
        Ok(Self {
            buffer,
            memory,
            capacity_bytes: alloc_size,
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

struct TargetImages {
    extent: vk::Extent2D,
    color_image: vk::Image,
    color_memory: vk::DeviceMemory,
    color_view: vk::ImageView,
    depth_image: vk::Image,
    depth_memory: vk::DeviceMemory,
    depth_view: vk::ImageView,
    initialized: bool,
}

impl TargetImages {
    fn create(context: &VulkanContext, extent: vk::Extent2D) -> Result<Self> {
        let w = extent.width.max(1);
        let h = extent.height.max(1);
        let (color_image, color_memory, color_view) =
            create_storage_image(context, w, h, vk::Format::R16G16B16A16_SFLOAT)?;
        let (depth_image, depth_memory, depth_view) =
            create_storage_image(context, w, h, vk::Format::R32_SFLOAT)?;
        Ok(Self {
            extent: vk::Extent2D {
                width: w,
                height: h,
            },
            color_image,
            color_memory,
            color_view,
            depth_image,
            depth_memory,
            depth_view,
            initialized: false,
        })
    }

    fn destroy(&mut self, device: &ash::Device) {
        unsafe {
            if self.color_view != vk::ImageView::null() {
                device.destroy_image_view(self.color_view, None);
                self.color_view = vk::ImageView::null();
            }
            if self.color_image != vk::Image::null() {
                device.destroy_image(self.color_image, None);
                self.color_image = vk::Image::null();
            }
            if self.color_memory != vk::DeviceMemory::null() {
                device.free_memory(self.color_memory, None);
                self.color_memory = vk::DeviceMemory::null();
            }
            if self.depth_view != vk::ImageView::null() {
                device.destroy_image_view(self.depth_view, None);
                self.depth_view = vk::ImageView::null();
            }
            if self.depth_image != vk::Image::null() {
                device.destroy_image(self.depth_image, None);
                self.depth_image = vk::Image::null();
            }
            if self.depth_memory != vk::DeviceMemory::null() {
                device.free_memory(self.depth_memory, None);
                self.depth_memory = vk::DeviceMemory::null();
            }
        }
    }
}

fn create_storage_image(
    context: &VulkanContext,
    width: u32,
    height: u32,
    format: vk::Format,
) -> Result<(vk::Image, vk::DeviceMemory, vk::ImageView)> {
    let device = &context.device;
    let info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D {
            width,
            height,
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
        unsafe { device.create_image(&info, None) }.context("VoxelDdaPass: create image")?;
    let reqs = unsafe { device.get_image_memory_requirements(image) };
    let mem_type = find_memory_type(
        context,
        reqs.memory_type_bits,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
    )
    .context("VoxelDdaPass: find image memory type")?;
    let alloc = vk::MemoryAllocateInfo::default()
        .allocation_size(reqs.size)
        .memory_type_index(mem_type);
    let memory =
        unsafe { device.allocate_memory(&alloc, None) }.context("VoxelDdaPass: allocate image")?;
    unsafe { device.bind_image_memory(image, memory, 0) }
        .context("VoxelDdaPass: bind image memory")?;

    let view_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(format)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        });
    let view = unsafe { device.create_image_view(&view_info, None) }
        .context("VoxelDdaPass: create image view")?;
    Ok((image, memory, view))
}

struct FrameSlot {
    descriptor_set: vk::DescriptorSet,
    ubo_buf: HostBuffer,
    chunks_buf: HostBuffer,
    l1_buf: HostBuffer,
    occ_buf: HostBuffer,
    cells_buf: HostBuffer,
    atlas_buf: HostBuffer,
    uploaded_signature: Option<u64>,
    atlas_uploaded: bool,
    target: Option<TargetImages>,
}

impl FrameSlot {
    fn new(context: &VulkanContext, descriptor_set: vk::DescriptorSet) -> Result<Self> {
        let ubo_buf = HostBuffer::new(
            context,
            std::mem::size_of::<VoxelFrameUbo>() as vk::DeviceSize,
            vk::BufferUsageFlags::UNIFORM_BUFFER,
        )?;
        let chunks_buf = HostBuffer::new(context, 256, vk::BufferUsageFlags::STORAGE_BUFFER)?;
        let l1_buf = HostBuffer::new(context, 256, vk::BufferUsageFlags::STORAGE_BUFFER)?;
        let occ_buf = HostBuffer::new(context, 256, vk::BufferUsageFlags::STORAGE_BUFFER)?;
        let cells_buf = HostBuffer::new(context, 256, vk::BufferUsageFlags::STORAGE_BUFFER)?;
        let atlas_buf = HostBuffer::new(context, 64, vk::BufferUsageFlags::STORAGE_BUFFER)?;
        Ok(Self {
            descriptor_set,
            ubo_buf,
            chunks_buf,
            l1_buf,
            occ_buf,
            cells_buf,
            atlas_buf,
            uploaded_signature: None,
            atlas_uploaded: false,
            target: None,
        })
    }

    fn ensure_ssbo(buf: &mut HostBuffer, context: &VulkanContext, bytes: &[u8]) -> Result<()> {
        let needed = (bytes.len() as vk::DeviceSize).max(64);
        if buf.capacity_bytes < needed {
            buf.destroy(&context.device);
            let grown = (needed * 3 / 2).max(needed);
            *buf = HostBuffer::new(context, grown, vk::BufferUsageFlags::STORAGE_BUFFER)?;
        }
        buf.write_bytes(&context.device, bytes)
    }

    fn upload_scene(&mut self, context: &VulkanContext, scene: &VoxelSceneUpload) -> Result<()> {
        if self.uploaded_signature == Some(scene.signature) {
            return Ok(());
        }
        let mut gpu_chunks = Vec::with_capacity(scene.chunks.len());
        let mut flat_l1: Vec<[u32; 4]> = Vec::new();
        let mut flat_occ: Vec<[u32; 2]> = Vec::new();
        let mut flat_cells: Vec<[u32; 4]> = Vec::new();

        for chunk in &scene.chunks {
            let l1_offset = flat_l1.len() as u32;
            let brick_offset = flat_occ.len() as u32;
            flat_l1.extend_from_slice(&chunk.l1_cells);
            flat_occ.extend_from_slice(&chunk.brick_occ);
            flat_cells.extend_from_slice(&chunk.brick_cells);
            gpu_chunks.push(VoxelChunkGpu {
                originAndL1Offset: [
                    chunk.origin_l1[0],
                    chunk.origin_l1[1],
                    chunk.origin_l1[2],
                    l1_offset as i32,
                ],
                dimsAndBrickOffset: [
                    chunk.dims_l1[0],
                    chunk.dims_l1[1],
                    chunk.dims_l1[2],
                    brick_offset,
                ],
            });
        }

        if gpu_chunks.is_empty() {
            gpu_chunks.push(VoxelChunkGpu::default());
        }
        if flat_l1.is_empty() {
            flat_l1.push([0; 4]);
        }
        if flat_occ.is_empty() {
            flat_occ.push([0; 2]);
        }
        if flat_cells.is_empty() {
            flat_cells.push([0; 4]);
        }

        Self::ensure_ssbo(
            &mut self.chunks_buf,
            context,
            bytemuck::cast_slice(&gpu_chunks),
        )?;
        Self::ensure_ssbo(&mut self.l1_buf, context, bytemuck::cast_slice(&flat_l1))?;
        Self::ensure_ssbo(&mut self.occ_buf, context, bytemuck::cast_slice(&flat_occ))?;
        Self::ensure_ssbo(
            &mut self.cells_buf,
            context,
            bytemuck::cast_slice(&flat_cells),
        )?;
        if !self.atlas_uploaded {
            if let Some(atlas) = &scene.atlas {
                Self::ensure_ssbo(&mut self.atlas_buf, context, bytemuck::cast_slice(atlas))?;
                self.atlas_uploaded = true;
            }
        }
        self.uploaded_signature = Some(scene.signature);
        Ok(())
    }

    fn destroy(&mut self, device: &ash::Device) {
        self.ubo_buf.destroy(device);
        self.chunks_buf.destroy(device);
        self.l1_buf.destroy(device);
        self.occ_buf.destroy(device);
        self.cells_buf.destroy(device);
        self.atlas_buf.destroy(device);
        if let Some(mut t) = self.target.take() {
            t.destroy(device);
        }
    }
}

/// Three-tier compute DDA render pass node registered in `GraphRenderer`.
pub struct VoxelDdaPass {
    context: Arc<VulkanContext>,
    descriptor_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    pipeline: ComputePipeline,
    slots: [FrameSlot; 2],
    scene: Option<Arc<VoxelSceneUpload>>,
}

impl VoxelDdaPass {
    /// Allocate Vulkan pipeline, descriptor pool/layout, and double-buffered SSBOs.
    pub fn new(context: Arc<VulkanContext>) -> Result<Self> {
        let device = &context.device;
        let layout_bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::UBO_BINDING)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::CHUNKS_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::L1_CELLS_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::BRICK_OCC_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::BRICK_CELLS_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::FORWARD_COLOR_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::FORWARD_DEPTH_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::OUT_COLOR_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::OUT_DEPTH_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::SPBR_ATLAS_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
        ];

        let descriptor_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&layout_bindings),
                None,
            )
        }
        .context("VoxelDdaPass: create descriptor layout")?;

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UNIFORM_BUFFER,
                descriptor_count: 2,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_BUFFER,
                descriptor_count: 10,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLED_IMAGE,
                descriptor_count: 4,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_IMAGE,
                descriptor_count: 4,
            },
        ];
        let descriptor_pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(2)
                    .pool_sizes(&pool_sizes),
                None,
            )
        }
        .context("VoxelDdaPass: create descriptor pool")?;

        let layouts = [descriptor_layout, descriptor_layout];
        let allocated = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(descriptor_pool)
                    .set_layouts(&layouts),
            )
        }
        .context("VoxelDdaPass: allocate descriptor sets")?;

        let shader = load_shader_module(device, VOXEL_DDA_SPV)?;
        let pipeline = ComputePipeline::new(
            device,
            shader,
            ENTRY_VOXEL_DDA,
            std::slice::from_ref(&descriptor_layout),
            &[],
        );
        unsafe { device.destroy_shader_module(shader, None) };
        let pipeline = pipeline?;

        let slot0 = FrameSlot::new(&context, allocated[0])?;
        let slot1 = FrameSlot::new(&context, allocated[1])?;

        Ok(Self {
            context,
            descriptor_layout,
            descriptor_pool,
            pipeline,
            slots: [slot0, slot1],
            scene: None,
        })
    }

    /// Replace the active voxel scene snapshot rendered by subsequent frames.
    pub fn set_scene(&mut self, scene: Option<Arc<VoxelSceneUpload>>) {
        self.scene = scene;
    }

    /// Release GPU target images before swapchain/offscreen resize.
    pub fn drop_targets(&mut self) {
        for slot in &mut self.slots {
            if let Some(mut t) = slot.target.take() {
                t.destroy(&self.context.device);
            }
        }
    }
}

impl Drop for VoxelDdaPass {
    fn drop(&mut self) {
        let device = &self.context.device;
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

impl RenderPassNode for VoxelDdaPass {
    fn name(&self) -> &str {
        "VoxelDdaPass"
    }

    fn setup(&mut self, graph: &mut RenderGraphBuilder, _settings: &RenderSettings) {
        graph.read_usage(ResourceUsage {
            handle: FORWARD_COLOR_H,
            access: vk::AccessFlags::SHADER_READ,
            stage: vk::PipelineStageFlags::COMPUTE_SHADER,
            layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        });
        graph.read_usage(ResourceUsage {
            handle: FORWARD_DEPTH_H,
            access: vk::AccessFlags::SHADER_READ,
            stage: vk::PipelineStageFlags::COMPUTE_SHADER,
            layout: vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL,
        });
        graph.write_usage(ResourceUsage {
            handle: VOXEL_COLOR_H,
            access: vk::AccessFlags::SHADER_WRITE,
            stage: vk::PipelineStageFlags::COMPUTE_SHADER,
            layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        });
        graph.write_usage(ResourceUsage {
            handle: VOXEL_DEPTH_H,
            access: vk::AccessFlags::SHADER_WRITE,
            stage: vk::PipelineStageFlags::COMPUTE_SHADER,
            layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        });
    }

    fn execute(&mut self, ctx: &RenderContext, resources: &mut GraphResources) -> Result<()> {
        let Some(scene) = self
            .scene
            .as_ref()
            .filter(|s| !s.chunks.is_empty())
            .cloned()
        else {
            return Ok(());
        };
        if !ctx.frame.has_camera
            || ctx.frame.render_mode != RenderMode::Raster
            || ctx.frame.debug_rt != 0
        {
            return Ok(());
        }
        let Some(forward_color_view) = resources.published_view(FORWARD_COLOR_H) else {
            return Ok(());
        };
        let Some(forward_depth_view) = resources.published_view(FORWARD_DEPTH_H) else {
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

        slot.upload_scene(&self.context, &scene)?;

        let mut sun_dir = [
            ctx.frame.light_dir[0],
            ctx.frame.light_dir[1],
            ctx.frame.light_dir[2],
        ];
        let sun_len =
            (sun_dir[0] * sun_dir[0] + sun_dir[1] * sun_dir[1] + sun_dir[2] * sun_dir[2]).sqrt();
        if sun_len > 1e-5 {
            sun_dir = [
                sun_dir[0] / sun_len,
                sun_dir[1] / sun_len,
                sun_dir[2] / sun_len,
            ];
        } else {
            sun_dir = [0.408, 0.816, 0.408];
        }
        let sun_rad = if ctx.frame.light_dir[3] > 0.01 {
            ctx.frame.light_dir[3]
        } else {
            1.8
        };
        let sun_col = if ctx.frame.light_color[0]
            + ctx.frame.light_color[1]
            + ctx.frame.light_color[2]
            > 0.01
        {
            ctx.frame.light_color
        } else {
            [1.0, 0.96, 0.90, 0.35]
        };
        let cursor = scene.cursor.unwrap_or(VoxelCursorUpload {
            active: false,
            min: [0.0; 3],
            max: [0.0; 3],
            color: [1.0, 0.9, 0.3, 1.0],
        });

        let inv_vp = crate::math::mat_inverse(&ctx.frame.view_proj);
        let ubo_data = VoxelFrameUbo {
            viewProj: ctx.frame.view_proj,
            invViewProj: inv_vp,
            cameraPos: [
                ctx.frame.camera_pos[0],
                ctx.frame.camera_pos[1],
                ctx.frame.camera_pos[2],
                scene.elapsed_seconds,
            ],
            sunDirRadiance: [sun_dir[0], sun_dir[1], sun_dir[2], sun_rad],
            sunColorAmbient: sun_col,
            counts: [
                scene.chunks.len() as u32,
                extent.width,
                extent.height,
                u32::from(cursor.active),
            ],
            cursorMin: [
                cursor.min[0],
                cursor.min[1],
                cursor.min[2],
                if scene.atlas.is_some() { 16.0 } else { 0.0 },
            ],
            cursorMax: [
                cursor.max[0],
                cursor.max[1],
                cursor.max[2],
                ctx.frame.exposure.max(0.01),
            ],
            cursorColor: cursor.color,
        };
        slot.ubo_buf
            .write_bytes(ctx.device, bytemuck::bytes_of(&ubo_data))?;

        let target = slot.target.as_mut().unwrap();
        let ds = slot.descriptor_set;

        let ubo_info = vk::DescriptorBufferInfo::default()
            .buffer(slot.ubo_buf.buffer)
            .range(std::mem::size_of::<VoxelFrameUbo>() as vk::DeviceSize);
        let chunks_info = vk::DescriptorBufferInfo::default()
            .buffer(slot.chunks_buf.buffer)
            .range(vk::WHOLE_SIZE);
        let l1_info = vk::DescriptorBufferInfo::default()
            .buffer(slot.l1_buf.buffer)
            .range(vk::WHOLE_SIZE);
        let occ_info = vk::DescriptorBufferInfo::default()
            .buffer(slot.occ_buf.buffer)
            .range(vk::WHOLE_SIZE);
        let cells_info = vk::DescriptorBufferInfo::default()
            .buffer(slot.cells_buf.buffer)
            .range(vk::WHOLE_SIZE);
        let atlas_info = vk::DescriptorBufferInfo::default()
            .buffer(slot.atlas_buf.buffer)
            .range(vk::WHOLE_SIZE);
        let fwd_col_info = vk::DescriptorImageInfo::default()
            .image_view(forward_color_view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        let fwd_depth_info = vk::DescriptorImageInfo::default()
            .image_view(forward_depth_view)
            .image_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL);
        let out_col_info = vk::DescriptorImageInfo::default()
            .image_view(target.color_view)
            .image_layout(vk::ImageLayout::GENERAL);
        let out_depth_info = vk::DescriptorImageInfo::default()
            .image_view(target.depth_view)
            .image_layout(vk::ImageLayout::GENERAL);

        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(ds)
                .dst_binding(bindings::UBO_BINDING)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .buffer_info(std::slice::from_ref(&ubo_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(ds)
                .dst_binding(bindings::CHUNKS_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(std::slice::from_ref(&chunks_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(ds)
                .dst_binding(bindings::L1_CELLS_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(std::slice::from_ref(&l1_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(ds)
                .dst_binding(bindings::BRICK_OCC_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(std::slice::from_ref(&occ_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(ds)
                .dst_binding(bindings::BRICK_CELLS_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(std::slice::from_ref(&cells_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(ds)
                .dst_binding(bindings::FORWARD_COLOR_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(std::slice::from_ref(&fwd_col_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(ds)
                .dst_binding(bindings::FORWARD_DEPTH_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(std::slice::from_ref(&fwd_depth_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(ds)
                .dst_binding(bindings::OUT_COLOR_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .image_info(std::slice::from_ref(&out_col_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(ds)
                .dst_binding(bindings::OUT_DEPTH_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .image_info(std::slice::from_ref(&out_depth_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(ds)
                .dst_binding(bindings::SPBR_ATLAS_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(std::slice::from_ref(&atlas_info)),
        ];
        unsafe { ctx.device.update_descriptor_sets(&writes, &[]) };

        let (old_layout, img_src_access, src_stage) = if target.initialized {
            (
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::AccessFlags::SHADER_READ,
                vk::PipelineStageFlags::FRAGMENT_SHADER
                    | vk::PipelineStageFlags::COMPUTE_SHADER
                    | vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS,
            )
        } else {
            (
                vk::ImageLayout::UNDEFINED,
                vk::AccessFlags::empty(),
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS,
            )
        };
        let subresource = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };
        let pre_barriers = [
            vk::ImageMemoryBarrier::default()
                .src_access_mask(img_src_access)
                .dst_access_mask(vk::AccessFlags::SHADER_WRITE)
                .old_layout(old_layout)
                .new_layout(vk::ImageLayout::GENERAL)
                .image(target.color_image)
                .subresource_range(subresource),
            vk::ImageMemoryBarrier::default()
                .src_access_mask(img_src_access)
                .dst_access_mask(vk::AccessFlags::SHADER_WRITE)
                .old_layout(old_layout)
                .new_layout(vk::ImageLayout::GENERAL)
                .image(target.depth_image)
                .subresource_range(subresource),
        ];
        let mem_barrier = vk::MemoryBarrier::default()
            .src_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            )
            .dst_access_mask(vk::AccessFlags::SHADER_READ);

        unsafe {
            ctx.device.cmd_pipeline_barrier(
                ctx.cmd,
                src_stage,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                std::slice::from_ref(&mem_barrier),
                &[],
                &pre_barriers,
            );

            ctx.device.cmd_bind_pipeline(
                ctx.cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline.pipeline,
            );
            ctx.device.cmd_bind_descriptor_sets(
                ctx.cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline.layout,
                0,
                std::slice::from_ref(&ds),
                &[],
            );
            ctx.device.cmd_dispatch(
                ctx.cmd,
                extent.width.div_ceil(8),
                extent.height.div_ceil(8),
                1,
            );

            let post_barriers = [
                vk::ImageMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ)
                    .old_layout(vk::ImageLayout::GENERAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .image(target.color_image)
                    .subresource_range(subresource),
                vk::ImageMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ)
                    .old_layout(vk::ImageLayout::GENERAL)
                    .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .image(target.depth_image)
                    .subresource_range(subresource),
            ];
            ctx.device.cmd_pipeline_barrier(
                ctx.cmd,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::COMPUTE_SHADER | vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &post_barriers,
            );
        }

        target.initialized = true;
        resources.set_image_view(VOXEL_COLOR_H, target.color_view);
        resources.set_image(VOXEL_COLOR_H, target.color_image);
        resources.set_image_view(VOXEL_DEPTH_H, target.depth_view);
        resources.set_image(VOXEL_DEPTH_H, target.depth_image);
        Ok(())
    }

    fn graph_info(&self) -> PassInfo {
        PassInfo {
            index: 0,
            name: self.name().into(),
            kind: PassKind::Voxel,
            inputs: vec![FORWARD_COLOR_H, FORWARD_DEPTH_H],
            outputs: vec![VOXEL_COLOR_H, VOXEL_DEPTH_H],
        }
    }
}
