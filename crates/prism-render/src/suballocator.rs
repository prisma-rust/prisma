//! VRAM suballocator and staging transfer arena.
//!
//! Provides Chunk-based Vulkan VRAM suballocation management (#1030),
//! reducing direct `vkAllocateMemory` calls and avoiding hitting driver `maxMemoryAllocationCount` limit.
//!
//! Core structures:
//! - `MemorySuballocator`: large `VkDeviceMemory` manager per memory type index,
//!   performs aligned suballocation and coalescing of adjacent free ranges on release (free-list coalescing).
//! - `StagingArena`: chunked staging area dedicated to batched uploads (`crate::batch::BatchUploader`),
//!   merges hundreds of per-resource staging `vkAllocateMemory` into large staging buffer allocations, and explicitly unmaps
//!   before submit to guarantee correct synchronization across drivers (including llvmpipe).
//! - `SuballocatorStats`: stats for reserved VRAM, active VRAM, chunk count and allocation count.

use anyhow::{Context as _, Result};
use ash::vk;
use serde::Serialize;

use crate::buffer::{create_buffer, BufferUsage, MemoryProperties};
use crate::context::VulkanContext;

/// Round value up to `alignment`.
pub fn align_up(value: vk::DeviceSize, alignment: vk::DeviceSize) -> vk::DeviceSize {
    if alignment <= 1 {
        return value;
    }
    let remainder = value % alignment;
    if remainder == 0 {
        value
    } else {
        value + (alignment - remainder)
    }
}

/// Contiguous free range in VRAM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryRange {
    pub offset: vk::DeviceSize,
    pub size: vk::DeviceSize,
}

/// VRAM suballocation handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Suballocation {
    pub memory: vk::DeviceMemory,
    pub offset: vk::DeviceSize,
    pub size: vk::DeviceSize,
    pub chunk_id: usize,
    pub memory_type_index: u32,
}

/// VRAM suballocator runtime stats.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuballocatorStats {
    pub total_reserved_bytes: u64,
    pub active_used_bytes: u64,
    pub chunk_count: usize,
    pub allocation_count: usize,
}

/// Chunk owning one whole `VkDeviceMemory`, maintains internal free range list.
pub struct MemoryChunk {
    pub memory: vk::DeviceMemory,
    pub size: vk::DeviceSize,
    pub memory_type_index: u32,
    free_ranges: Vec<MemoryRange>,
    active_allocations: usize,
    allocated_bytes: vk::DeviceSize,
}

impl MemoryChunk {
    /// Allocate and initialize a new VRAM Chunk.
    pub fn new(device: &ash::Device, size: vk::DeviceSize, memory_type_index: u32) -> Result<Self> {
        let alloc_info = vk::MemoryAllocateInfo::default()
            .allocation_size(size)
            .memory_type_index(memory_type_index);
        let memory = unsafe { device.allocate_memory(&alloc_info, None) }
            .context("MemoryChunk: allocate device memory")?;
        Ok(Self {
            memory,
            size,
            memory_type_index,
            free_ranges: vec![MemoryRange { offset: 0, size }],
            active_allocations: 0,
            allocated_bytes: 0,
        })
    }

    /// Construct a Mock Chunk for pure algorithm tests (no Vulkan device calls).
    #[cfg(test)]
    pub(crate) fn new_mock(size: vk::DeviceSize, memory_type_index: u32) -> Self {
        Self {
            memory: vk::DeviceMemory::null(),
            size,
            memory_type_index,
            free_ranges: vec![MemoryRange { offset: 0, size }],
            active_allocations: 0,
            allocated_bytes: 0,
        }
    }

    /// Allocate a range of given size and alignment in current Chunk.
    pub fn allocate(
        &mut self,
        size: vk::DeviceSize,
        alignment: vk::DeviceSize,
    ) -> Option<vk::DeviceSize> {
        if size == 0 {
            return Some(0);
        }
        let alignment = alignment.max(1);
        for i in 0..self.free_ranges.len() {
            let range = self.free_ranges[i];
            let aligned_offset = align_up(range.offset, alignment);
            let end_offset = aligned_offset.checked_add(size)?;
            let range_end = range.offset.checked_add(range.size)?;

            if end_offset <= range_end {
                self.free_ranges.remove(i);
                let mut insert_idx = i;
                if aligned_offset > range.offset {
                    self.free_ranges.insert(
                        insert_idx,
                        MemoryRange {
                            offset: range.offset,
                            size: aligned_offset - range.offset,
                        },
                    );
                    insert_idx += 1;
                }
                if end_offset < range_end {
                    self.free_ranges.insert(
                        insert_idx,
                        MemoryRange {
                            offset: end_offset,
                            size: range_end - end_offset,
                        },
                    );
                }
                self.active_allocations += 1;
                self.allocated_bytes += size;
                return Some(aligned_offset);
            }
        }
        None
    }

    /// Release range and automatically coalesce adjacent free ranges.
    pub fn free(&mut self, offset: vk::DeviceSize, size: vk::DeviceSize) {
        if size == 0 {
            return;
        }
        let free_range = MemoryRange { offset, size };
        let pos = match self.free_ranges.binary_search_by_key(&offset, |r| r.offset) {
            Ok(idx) => idx,
            Err(idx) => idx,
        };
        self.free_ranges.insert(pos, free_range);
        self.active_allocations = self.active_allocations.saturating_sub(1);
        self.allocated_bytes = self.allocated_bytes.saturating_sub(size);

        let mut i = 0;
        while i + 1 < self.free_ranges.len() {
            let current = self.free_ranges[i];
            let next = self.free_ranges[i + 1];
            if current.offset + current.size == next.offset {
                self.free_ranges[i].size += next.size;
                self.free_ranges.remove(i + 1);
            } else {
                i += 1;
            }
        }
    }

    /// Whether current Chunk has no active allocations.
    pub fn is_empty(&self) -> bool {
        self.active_allocations == 0
    }

    /// Release underlying Vulkan DeviceMemory.
    pub fn destroy(&mut self, device: &ash::Device) {
        if self.memory != vk::DeviceMemory::null() {
            unsafe {
                device.free_memory(self.memory, None);
            }
            self.memory = vk::DeviceMemory::null();
        }
        self.free_ranges.clear();
        self.active_allocations = 0;
        self.allocated_bytes = 0;
    }
}

/// Vulkan VRAM suballocator.
pub struct MemorySuballocator {
    chunks: Vec<Option<MemoryChunk>>,
    default_chunk_size: vk::DeviceSize,
}

impl MemorySuballocator {
    /// Default 64 MiB single Chunk size.
    pub const DEFAULT_CHUNK_SIZE: vk::DeviceSize = 64 * 1024 * 1024;

    pub fn new(default_chunk_size: vk::DeviceSize) -> Self {
        Self {
            chunks: Vec::new(),
            default_chunk_size: default_chunk_size.max(1024 * 1024),
        }
    }

    /// Allocate VRAM range of given size, alignment and memory type.
    pub fn allocate(
        &mut self,
        device: &ash::Device,
        size: vk::DeviceSize,
        alignment: vk::DeviceSize,
        memory_type_index: u32,
    ) -> Result<Suballocation> {
        for (chunk_id, chunk_opt) in self.chunks.iter_mut().enumerate() {
            if let Some(chunk) = chunk_opt {
                if chunk.memory_type_index == memory_type_index {
                    if let Some(offset) = chunk.allocate(size, alignment) {
                        return Ok(Suballocation {
                            memory: chunk.memory,
                            offset,
                            size,
                            chunk_id,
                            memory_type_index,
                        });
                    }
                }
            }
        }

        let chunk_size = self
            .default_chunk_size
            .max(align_up(size, alignment).next_power_of_two());
        let mut new_chunk = MemoryChunk::new(device, chunk_size, memory_type_index)?;
        let offset = new_chunk
            .allocate(size, alignment)
            .ok_or_else(|| anyhow::anyhow!("failed to allocate from newly created memory chunk"))?;
        let memory = new_chunk.memory;

        let chunk_id = if let Some(empty_idx) = self.chunks.iter().position(|c| c.is_none()) {
            self.chunks[empty_idx] = Some(new_chunk);
            empty_idx
        } else {
            let id = self.chunks.len();
            self.chunks.push(Some(new_chunk));
            id
        };

        Ok(Suballocation {
            memory,
            offset,
            size,
            chunk_id,
            memory_type_index,
        })
    }

    /// Release suballocation range. If multiple Chunks of same memory type and current Chunk free, destroy and return to driver.
    pub fn free(&mut self, device: &ash::Device, suballoc: &Suballocation) {
        if let Some(Some(chunk)) = self.chunks.get_mut(suballoc.chunk_id) {
            chunk.free(suballoc.offset, suballoc.size);
            if chunk.is_empty() {
                let same_type_count = self
                    .chunks
                    .iter()
                    .filter_map(|c| c.as_ref())
                    .filter(|c| c.memory_type_index == suballoc.memory_type_index)
                    .count();
                if same_type_count > 1 {
                    let mut removed = self.chunks[suballoc.chunk_id].take().unwrap();
                    removed.destroy(device);
                }
            }
        }
    }

    /// Query current suballocation stats.
    pub fn stats(&self) -> SuballocatorStats {
        let mut total_reserved_bytes = 0;
        let mut active_used_bytes = 0;
        let mut chunk_count = 0;
        let mut allocation_count = 0;

        for chunk in self.chunks.iter().flatten() {
            total_reserved_bytes += chunk.size;
            active_used_bytes += chunk.allocated_bytes;
            chunk_count += 1;
            allocation_count += chunk.active_allocations;
        }

        SuballocatorStats {
            total_reserved_bytes,
            active_used_bytes,
            chunk_count,
            allocation_count,
        }
    }

    /// Destroy all Chunks and release underlying VRAM.
    pub fn destroy(&mut self, device: &ash::Device) {
        for chunk_opt in self.chunks.iter_mut() {
            if let Some(mut chunk) = chunk_opt.take() {
                chunk.destroy(device);
            }
        }
        self.chunks.clear();
    }
}

/// One Buffer + Memory Chunk for staging uploads.
pub struct StagingChunk {
    pub buffer: vk::Buffer,
    pub memory: vk::DeviceMemory,
    pub capacity: vk::DeviceSize,
    pub cursor: vk::DeviceSize,
    pub mapped_ptr: *mut u8,
}

unsafe impl Send for StagingChunk {}
unsafe impl Sync for StagingChunk {}

impl StagingChunk {
    pub fn new(context: &VulkanContext, capacity: vk::DeviceSize) -> Result<Self> {
        let (buffer, memory) = create_buffer(
            context,
            capacity,
            BufferUsage::TRANSFER_SRC,
            MemoryProperties::HOST_VISIBLE | MemoryProperties::HOST_COHERENT,
        )
        .context("StagingChunk: create staging buffer")?;

        let mapped_ptr = unsafe {
            context
                .device
                .map_memory(memory, 0, capacity, vk::MemoryMapFlags::empty())
        }
        .context("StagingChunk: map staging memory")? as *mut u8;

        Ok(Self {
            buffer,
            memory,
            capacity,
            cursor: 0,
            mapped_ptr,
        })
    }

    pub fn can_fit(&self, size: vk::DeviceSize, alignment: vk::DeviceSize) -> bool {
        let offset = align_up(self.cursor, alignment);
        match offset.checked_add(size) {
            Some(end) => end <= self.capacity,
            None => false,
        }
    }

    pub fn stage(
        &mut self,
        data: &[u8],
        alignment: vk::DeviceSize,
    ) -> (vk::Buffer, vk::DeviceSize) {
        let offset = align_up(self.cursor, alignment);
        if !data.is_empty() && !self.mapped_ptr.is_null() {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    self.mapped_ptr.add(offset as usize),
                    data.len(),
                );
            }
        }
        self.cursor = offset + data.len() as vk::DeviceSize;
        (self.buffer, offset)
    }

    pub fn unmap(&mut self, device: &ash::Device) {
        if !self.mapped_ptr.is_null() {
            unsafe {
                device.unmap_memory(self.memory);
            }
            self.mapped_ptr = std::ptr::null_mut();
        }
    }

    pub fn destroy(&mut self, device: &ash::Device) {
        self.unmap(device);
        if self.buffer != vk::Buffer::null() {
            unsafe {
                device.destroy_buffer(self.buffer, None);
            }
            self.buffer = vk::Buffer::null();
        }
        if self.memory != vk::DeviceMemory::null() {
            unsafe {
                device.free_memory(self.memory, None);
            }
            self.memory = vk::DeviceMemory::null();
        }
    }
}

/// Batched upload staging arena.
///
/// Replaces per-resource `create_buffer` + `vkAllocateMemory`, aggregating hundreds of small allocations during batch load into
/// a few 16~64 MiB Chunk allocations. Before submit calls `unmap_all` explicitly to ensure llvmpipe
/// software rasterizer and hardware drivers render consistently.
pub struct StagingArena {
    chunks: Vec<StagingChunk>,
    default_chunk_size: vk::DeviceSize,
    allocation_count: usize,
}

impl StagingArena {
    /// Default single Chunk size (16 MiB).
    pub const DEFAULT_CHUNK_SIZE: vk::DeviceSize = 16 * 1024 * 1024;

    pub fn new(default_chunk_size: vk::DeviceSize) -> Self {
        Self {
            chunks: Vec::new(),
            default_chunk_size: default_chunk_size.max(1024 * 1024),
            allocation_count: 0,
        }
    }

    /// Copy data into staging area, return corresponding `(VkBuffer, offset)`.
    pub fn stage(
        &mut self,
        context: &VulkanContext,
        data: &[u8],
        alignment: vk::DeviceSize,
    ) -> Result<(vk::Buffer, vk::DeviceSize)> {
        let size = data.len() as vk::DeviceSize;
        let alignment = alignment.max(1);
        self.allocation_count += 1;

        if let Some(last_chunk) = self.chunks.last_mut() {
            if last_chunk.can_fit(size, alignment) {
                return Ok(last_chunk.stage(data, alignment));
            }
        }

        let needed = align_up(size, alignment);
        let chunk_size = self.default_chunk_size.max(needed.next_power_of_two());
        let mut chunk = StagingChunk::new(context, chunk_size)?;
        let result = chunk.stage(data, alignment);
        self.chunks.push(chunk);
        Ok(result)
    }

    /// Explicitly unmap all Chunks before submitting commands (for llvmpipe driver timing requirements).
    pub fn unmap_all(&mut self, device: &ash::Device) {
        for chunk in &mut self.chunks {
            chunk.unmap(device);
        }
    }

    /// Release all staging Chunks and VRAM.
    pub fn destroy(&mut self, device: &ash::Device) {
        for mut chunk in self.chunks.drain(..) {
            chunk.destroy(device);
        }
        self.allocation_count = 0;
    }

    /// Get staging area stats.
    pub fn stats(&self) -> SuballocatorStats {
        let mut total_reserved_bytes = 0;
        let mut active_used_bytes = 0;
        for chunk in &self.chunks {
            total_reserved_bytes += chunk.capacity;
            active_used_bytes += chunk.cursor;
        }
        SuballocatorStats {
            total_reserved_bytes,
            active_used_bytes,
            chunk_count: self.chunks.len(),
            allocation_count: self.allocation_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_align_up() {
        assert_eq!(align_up(0, 16), 0);
        assert_eq!(align_up(1, 16), 16);
        assert_eq!(align_up(16, 16), 16);
        assert_eq!(align_up(17, 16), 32);
        assert_eq!(align_up(300, 256), 512);
        assert_eq!(align_up(512, 256), 512);
        assert_eq!(align_up(10, 1), 10);
        assert_eq!(align_up(10, 0), 10);
    }

    #[test]
    fn test_chunk_allocation_and_coalescing() {
        let mut chunk = MemoryChunk::new_mock(1024, 0);
        assert_eq!(chunk.free_ranges.len(), 1);
        assert_eq!(
            chunk.free_ranges[0],
            MemoryRange {
                offset: 0,
                size: 1024
            }
        );

        let a1 = chunk.allocate(100, 16).unwrap();
        assert_eq!(a1, 0);

        let a2 = chunk.allocate(200, 16).unwrap();
        assert_eq!(a2, 112);

        let a3 = chunk.allocate(100, 64).unwrap();
        assert_eq!(a3, 320);

        assert_eq!(chunk.active_allocations, 3);
        assert_eq!(chunk.allocated_bytes, 400);

        chunk.free(a2, 200);
        assert_eq!(chunk.active_allocations, 2);

        chunk.free(a1, 100);
        assert_eq!(chunk.active_allocations, 1);

        chunk.free(a3, 100);
        assert_eq!(chunk.active_allocations, 0);
        assert_eq!(chunk.allocated_bytes, 0);
        assert_eq!(chunk.free_ranges.len(), 1);
        assert_eq!(
            chunk.free_ranges[0],
            MemoryRange {
                offset: 0,
                size: 1024
            }
        );
    }

    #[test]
    fn test_chunk_out_of_memory() {
        let mut chunk = MemoryChunk::new_mock(500, 0);
        assert!(chunk.allocate(600, 16).is_none());
        let a = chunk.allocate(500, 16).unwrap();
        assert_eq!(a, 0);
        assert!(chunk.allocate(1, 1).is_none());
    }

    #[test]
    fn test_suballocator_stats() {
        let suballoc = MemorySuballocator::new(64 * 1024 * 1024);
        let s = suballoc.stats();
        assert_eq!(s.total_reserved_bytes, 0);
        assert_eq!(s.active_used_bytes, 0);
        assert_eq!(s.chunk_count, 0);
        assert_eq!(s.allocation_count, 0);
    }
}
