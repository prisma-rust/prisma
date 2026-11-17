//! Hardware Ray Tracing acceleration structure (BLAS/TLAS) for three-tier sparse voxels.
//!
//! Each active voxel chunk builds a procedural AABB BLAS containing one `VkAabbPositionsKHR`
//! per non-empty L1 cell (`1m x 1m x 1m`), and all active chunks are combined into a single
//! TLAS for `VK_KHR_ray_query` traversal.

use std::collections::HashMap;

use anyhow::Context as _;
use ash::vk;

use crate::acceleration_structure::{Tlas, TlasInstance};
use crate::buffer::{self, BufferUsage, MemoryProperties};
use crate::context::VulkanContext;
use crate::voxel_dda_pass::VoxelChunkUpload;

/// Procedural AABB bottom-level acceleration structure for a single voxel chunk.
pub struct VoxelChunkBlas {
    pub handle: vk::AccelerationStructureKHR,
    pub device_address: vk::DeviceAddress,
    pub origin_l1: [i32; 3],
    pub brick_count: u32,
    device: ash::Device,
    as_fn: ash::khr::acceleration_structure::Device,
    as_buffer: vk::Buffer,
    as_memory: vk::DeviceMemory,
    aabb_buffer: vk::Buffer,
    aabb_memory: vk::DeviceMemory,
}

impl Drop for VoxelChunkBlas {
    fn drop(&mut self) {
        unsafe {
            self.as_fn.destroy_acceleration_structure(self.handle, None);
            self.device.destroy_buffer(self.as_buffer, None);
            self.device.free_memory(self.as_memory, None);
            self.device.destroy_buffer(self.aabb_buffer, None);
            self.device.free_memory(self.aabb_memory, None);
        }
    }
}

impl VoxelChunkBlas {
    /// Build a procedural AABB BLAS from a chunk's non-empty L1 cells.
    pub fn build(
        context: &VulkanContext,
        command_pool: vk::CommandPool,
        chunk: &VoxelChunkUpload,
    ) -> anyhow::Result<Option<Self>> {
        let as_fn = context
            .acceleration_structure_fn
            .as_ref()
            .context("VoxelChunkBlas: VK_KHR_acceleration_structure not available")?
            .clone();

        let dx = chunk.dims_l1[0].max(1);
        let dy = chunk.dims_l1[1].max(1);

        let mut aabbs: Vec<vk::AabbPositionsKHR> = Vec::new();
        for (idx, l1) in chunk.l1_cells.iter().enumerate() {
            if l1[0] == 0 {
                continue;
            }
            let lx = (idx as u32) % dx;
            let ly = ((idx as u32) / dx) % dy;
            let lz = (idx as u32) / (dx * dy);
            let min_x = lx as f32;
            let min_y = ly as f32;
            let min_z = lz as f32;
            aabbs.push(vk::AabbPositionsKHR {
                min_x,
                min_y,
                min_z,
                max_x: min_x + 1.0,
                max_y: min_y + 1.0,
                max_z: min_z + 1.0,
            });
        }

        if aabbs.is_empty() {
            return Ok(None);
        }

        let brick_count = aabbs.len() as u32;
        let aabb_byte_len =
            (aabbs.len() * std::mem::size_of::<vk::AabbPositionsKHR>()) as vk::DeviceSize;

        let (aabb_buffer, aabb_memory) = buffer::create_buffer(
            context,
            aabb_byte_len,
            BufferUsage::STORAGE_BUFFER
                | BufferUsage::SHADER_DEVICE_ADDRESS
                | BufferUsage::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR,
            MemoryProperties::HOST_VISIBLE | MemoryProperties::HOST_COHERENT,
        )
        .context("VoxelChunkBlas: create AABB buffer")?;

        unsafe {
            let ptr = context
                .device
                .map_memory(aabb_memory, 0, aabb_byte_len, vk::MemoryMapFlags::empty())
                .context("VoxelChunkBlas: map AABB buffer")?;
            std::ptr::copy_nonoverlapping(
                aabbs.as_ptr() as *const u8,
                ptr as *mut u8,
                aabb_byte_len as usize,
            );
            context.device.unmap_memory(aabb_memory);
        }

        let aabb_addr = unsafe {
            context
                .device
                .get_buffer_device_address(&vk::BufferDeviceAddressInfo::default().buffer(aabb_buffer))
        };

        let geom_data = vk::AccelerationStructureGeometryDataKHR {
            aabbs: vk::AccelerationStructureGeometryAabbsDataKHR::default()
                .data(vk::DeviceOrHostAddressConstKHR {
                    device_address: aabb_addr,
                })
                .stride(std::mem::size_of::<vk::AabbPositionsKHR>() as vk::DeviceSize),
        };

        let geometry = vk::AccelerationStructureGeometryKHR::default()
            .geometry_type(vk::GeometryTypeKHR::AABBS)
            .flags(vk::GeometryFlagsKHR::OPAQUE)
            .geometry(geom_data);

        let build_info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
            .ty(vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL)
            .flags(vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_TRACE)
            .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
            .geometries(std::slice::from_ref(&geometry));

        let mut size_info = vk::AccelerationStructureBuildSizesInfoKHR::default();
        unsafe {
            as_fn.get_acceleration_structure_build_sizes(
                vk::AccelerationStructureBuildTypeKHR::DEVICE,
                &build_info,
                &[brick_count],
                &mut size_info,
            );
        }

        let (as_buffer, as_memory) = buffer::create_buffer(
            context,
            size_info.acceleration_structure_size,
            BufferUsage::ACCELERATION_STRUCTURE_STORAGE_KHR | BufferUsage::SHADER_DEVICE_ADDRESS,
            MemoryProperties::DEVICE_LOCAL,
        )
        .context("VoxelChunkBlas: create AS buffer")?;

        let create_info = vk::AccelerationStructureCreateInfoKHR::default()
            .buffer(as_buffer)
            .size(size_info.acceleration_structure_size)
            .ty(vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL);

        let handle = unsafe { as_fn.create_acceleration_structure(&create_info, None) }
            .context("VoxelChunkBlas: create acceleration structure")?;

        let device_address = unsafe {
            as_fn.get_acceleration_structure_device_address(
                &vk::AccelerationStructureDeviceAddressInfoKHR::default()
                    .acceleration_structure(handle),
            )
        };

        let (scratch_buffer, scratch_memory) = buffer::create_buffer(
            context,
            size_info.build_scratch_size.max(256),
            BufferUsage::STORAGE_BUFFER | BufferUsage::SHADER_DEVICE_ADDRESS,
            MemoryProperties::DEVICE_LOCAL,
        )
        .context("VoxelChunkBlas: create scratch buffer")?;

        let scratch_addr = unsafe {
            context
                .device
                .get_buffer_device_address(&vk::BufferDeviceAddressInfo::default().buffer(scratch_buffer))
        };

        let mut exec_build_info = build_info;
        exec_build_info.dst_acceleration_structure = handle;
        exec_build_info.scratch_data = vk::DeviceOrHostAddressKHR {
            device_address: scratch_addr,
        };

        let range_info = vk::AccelerationStructureBuildRangeInfoKHR::default()
            .primitive_count(brick_count)
            .primitive_offset(0)
            .first_vertex(0)
            .transform_offset(0);

        let alloc_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let cmd = unsafe { context.device.allocate_command_buffers(&alloc_info) }
            .context("VoxelChunkBlas: allocate command buffer")?[0];

        unsafe {
            context
                .device
                .begin_command_buffer(
                    cmd,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .context("VoxelChunkBlas: begin command buffer")?;
            as_fn.cmd_build_acceleration_structures(
                cmd,
                std::slice::from_ref(&exec_build_info),
                std::slice::from_ref(&std::slice::from_ref(&range_info)),
            );
            context
                .device
                .end_command_buffer(cmd)
                .context("VoxelChunkBlas: end command buffer")?;

            let submit_info =
                vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&cmd));
            context
                .device
                .queue_submit(context.graphics_queue, &[submit_info], vk::Fence::null())
                .context("VoxelChunkBlas: submit BLAS build")?;
            context
                .device
                .queue_wait_idle(context.graphics_queue)
                .context("VoxelChunkBlas: wait BLAS build")?;
            context.device.free_command_buffers(command_pool, &[cmd]);

            context.device.destroy_buffer(scratch_buffer, None);
            context.device.free_memory(scratch_memory, None);
        }

        Ok(Some(Self {
            handle,
            device_address,
            origin_l1: chunk.origin_l1,
            brick_count,
            device: context.device.clone(),
            as_fn,
            as_buffer,
            as_memory,
            aabb_buffer,
            aabb_memory,
        }))
    }
}

/// Manages per-chunk procedural AABB BLASes and the combined voxel TLAS.
pub struct VoxelRtAccel {
    chunk_blases: HashMap<[i32; 3], VoxelChunkBlas>,
    tlas: Option<Tlas>,
}

impl Default for VoxelRtAccel {
    fn default() -> Self {
        Self::new()
    }
}

impl VoxelRtAccel {
    pub fn new() -> Self {
        Self {
            chunk_blases: HashMap::new(),
            tlas: None,
        }
    }

    pub fn tlas_handle(&self) -> Option<vk::AccelerationStructureKHR> {
        self.tlas.as_ref().map(|t| t.handle)
    }

    /// Synchronize active voxel chunks into BLASes and rebuild the TLAS.
    pub fn sync_chunks(
        &mut self,
        context: &VulkanContext,
        command_pool: vk::CommandPool,
        chunks: &[VoxelChunkUpload],
    ) -> anyhow::Result<()> {
        if !context.rt_caps.acceleration_structure {
            return Ok(());
        }

        self.chunk_blases.clear();
        let mut instances: Vec<TlasInstance> = Vec::with_capacity(chunks.len());
        let mut blas_addresses: Vec<vk::DeviceAddress> = Vec::with_capacity(chunks.len());

        for (idx, chunk) in chunks.iter().enumerate() {
            if let Some(blas) = VoxelChunkBlas::build(context, command_pool, chunk)? {
                let tx = chunk.origin_l1[0] as f32;
                let ty = chunk.origin_l1[1] as f32;
                let tz = chunk.origin_l1[2] as f32;
                let transform = [1.0, 0.0, 0.0, tx, 0.0, 1.0, 0.0, ty, 0.0, 0.0, 1.0, tz];
                blas_addresses.push(blas.device_address);
                instances.push(TlasInstance {
                    transform,
                    custom_index: idx as u32,
                    mask: 0xFF,
                    instance_shader_binding_table_record_offset: 0,
                    flags: vk::GeometryInstanceFlagsKHR::TRIANGLE_FACING_CULL_DISABLE,
                });
                self.chunk_blases.insert(chunk.origin_l1, blas);
            }
        }

        if instances.is_empty() {
            self.tlas = None;
            return Ok(());
        }

        self.tlas = Some(Tlas::build(
            context,
            command_pool,
            &instances,
            &blas_addresses,
        )?);
        Ok(())
    }
}
