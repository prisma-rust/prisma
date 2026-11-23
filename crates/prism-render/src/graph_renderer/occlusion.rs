//! Coarse depth-tile occlusion culling state and readback helpers (#1007).

use ash::vk;

use crate::forward_pass::ForwardPass;

use super::{GraphRenderer, OCCLUSION_TILE_SIZE};

/// Depth readback buffer corresponding to one swapchain image (#1007).
pub(super) struct OcclusionTarget {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    size: vk::DeviceSize,
}

/// GPU-side state for occlusion culling (#1007, path A: previous-frame coarse depth + proxy boxes).
#[derive(Default)]
pub(super) struct OcclusionState {
    pub(super) enabled: bool,
    /// How many depth blocks to expand outward when testing, compensating one-frame latency edge error.
    pub(super) margin_tiles: u32,
    pub(super) targets: Vec<Option<OcclusionTarget>>,
    /// Most recent successfully read coarse depth and its camera matrix (previous frame).
    pub(super) tiles: Option<crate::culling::DepthTiles>,
    pub(super) tiles_view_proj: Option<[[f32; 4]; 4]>,
    /// Camera matrix per swapchain image with "copy recorded, waiting readback".
    pub(super) pending_view_proj: Vec<Option<[[f32; 4]; 4]>>,
    pub(super) counts: crate::culling::OcclusionCounts,
}

impl GraphRenderer {
    /// Toggle occlusion culling (#1007).
    ///
    /// When disabled, immediately discard already-read coarse depth and stop depth copy, next frame returns to old behavior.
    pub fn set_occlusion_culling(&mut self, enabled: bool) {
        if self.occlusion.enabled == enabled {
            return;
        }
        self.occlusion.enabled = enabled;
        if !enabled {
            self.occlusion.tiles = None;
            self.occlusion.tiles_view_proj = None;
            self.occlusion.pending_view_proj.fill(None);
            self.occlusion.counts = Default::default();
            if let Some(forward) = self.graph.pass_mut::<ForwardPass>() {
                forward.set_depth_readback(None);
            }
        }
    }

    pub fn occlusion_culling(&self) -> bool {
        self.occlusion.enabled
    }

    /// Statistics of most recent frame occlusion culling (for reports / debug).
    pub fn occlusion_counts(&self) -> crate::culling::OcclusionCounts {
        self.occlusion.counts
    }

    /// Previous-frame coarse depth and camera matrix that generated it; returns `None` when no data available.
    pub fn occlusion_view(&self) -> Option<(&crate::culling::DepthTiles, [[f32; 4]; 4])> {
        if !self.occlusion.enabled {
            return None;
        }
        Some((
            self.occlusion.tiles.as_ref()?,
            self.occlusion.tiles_view_proj?,
        ))
    }

    /// Use previous-frame coarse depth to flag culled draw items as not visible in main pass, record stats.
    ///
    /// Caller should invoke before handing draw list to `prepare` / `execute`; when no depth available
    /// (first frame, disabled, readback failure) returns as-is.
    pub fn cull_occluded_draws(
        &mut self,
        items: &mut [crate::render_graph::DrawItem],
    ) -> crate::culling::OcclusionCounts {
        let Some((tiles, view_proj)) = self.occlusion_view() else {
            self.occlusion.counts = Default::default();
            return self.occlusion.counts;
        };
        let counts = crate::culling::mark_occluded_draws(
            items,
            tiles,
            &view_proj,
            self.occlusion.margin_tiles,
        );
        self.occlusion.counts = counts;
        counts
    }

    /// Ensure current image has a writable depth readback buffer under `extent`.
    pub(super) fn ensure_occlusion_target(
        &mut self,
        image_index: u32,
        extent: vk::Extent2D,
    ) -> Option<vk::Buffer> {
        let size = u64::from(extent.width) * u64::from(extent.height) * 4;
        let image_count = self
            .swapchain
            .as_ref()
            .map(|swapchain| swapchain.images.len())
            .unwrap_or(1);
        if self.occlusion.targets.len() < image_count {
            self.occlusion.targets.resize_with(image_count, || None);
        }
        if self.occlusion.pending_view_proj.len() < image_count {
            self.occlusion.pending_view_proj.resize(image_count, None);
        }
        if size == 0 || image_index as usize >= self.occlusion.targets.len() {
            return None;
        }
        let slot = &mut self.occlusion.targets[image_index as usize];
        if let Some(target) = slot.as_ref() {
            if target.size == size {
                return Some(target.buffer);
            }
        }
        if let Some(old) = slot.take() {
            unsafe {
                self.runtime.context.device.destroy_buffer(old.buffer, None);
                self.runtime.context.device.free_memory(old.memory, None);
            }
        }
        let (buffer, memory) = crate::buffer::create_buffer(
            &self.runtime.context,
            size,
            vk::BufferUsageFlags::TRANSFER_DST,
            crate::buffer::MemoryProperties::HOST_VISIBLE
                | crate::buffer::MemoryProperties::HOST_COHERENT,
        )
        .ok()?;
        *slot = Some(OcclusionTarget {
            buffer,
            memory,
            size,
        });
        Some(buffer)
    }

    pub(super) fn destroy_occlusion_targets(&mut self) {
        for slot in &mut self.occlusion.targets {
            if let Some(target) = slot.take() {
                unsafe {
                    self.runtime
                        .context
                        .device
                        .destroy_buffer(target.buffer, None);
                    self.runtime.context.device.free_memory(target.memory, None);
                }
            }
        }
        self.occlusion.targets.clear();
    }

    /// Read back depth written last frame and generate coarse depth. `begin_frame` already waited on this image
    /// fence, so mapping memory here will not race with GPU writes.
    pub(super) fn collect_occlusion_depth(&mut self, image_index: u32, extent: vk::Extent2D) {
        if !self.occlusion.enabled {
            return;
        }
        let Some(vp) = self
            .occlusion
            .pending_view_proj
            .get_mut(image_index as usize)
            .and_then(|slot| slot.take())
        else {
            return;
        };
        let Some(target) = self
            .occlusion
            .targets
            .get(image_index as usize)
            .and_then(|slot| slot.as_ref())
        else {
            return;
        };
        let size = target.size;
        let memory = target.memory;
        let sample_count = (extent.width as usize).saturating_mul(extent.height as usize);
        if sample_count == 0 || size != sample_count as u64 * 4 {
            return;
        }
        let device = &self.runtime.context.device;
        let ptr = unsafe { device.map_memory(memory, 0, size, vk::MemoryMapFlags::empty()) };
        let Ok(ptr) = ptr else {
            log::warn!("occlusion: map depth readback failed");
            return;
        };
        let samples = unsafe { std::slice::from_raw_parts(ptr as *const f32, sample_count) };
        let tiles = crate::culling::DepthTiles::from_samples(
            [extent.width, extent.height],
            OCCLUSION_TILE_SIZE,
            samples,
        );
        unsafe { device.unmap_memory(memory) };
        self.occlusion.tiles_view_proj = tiles.as_ref().map(|_| vp);
        self.occlusion.tiles = tiles;
    }
}
