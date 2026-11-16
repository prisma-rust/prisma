//! Coarse depth-tile occlusion culling state and readback helpers (#1007).

use ash::vk;

use crate::forward_pass::ForwardPass;

use super::{GraphRenderer, OCCLUSION_TILE_SIZE};

/// 一个交换链图像对应的深度回读缓冲（#1007）。
pub(super) struct OcclusionTarget {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    size: vk::DeviceSize,
}

/// 遮挡剔除的 GPU 侧状态（#1007，路径 A：上一帧粗深度 + 代理盒）。
#[derive(Default)]
pub(super) struct OcclusionState {
    pub(super) enabled: bool,
    /// 判定时向外扩的深度块数，抵消一帧延迟带来的边缘误差。
    pub(super) margin_tiles: u32,
    pub(super) targets: Vec<Option<OcclusionTarget>>,
    /// 最近一次成功回读的粗深度及其相机矩阵（上一帧）。
    pub(super) tiles: Option<crate::culling::DepthTiles>,
    pub(super) tiles_view_proj: Option<[[f32; 4]; 4]>,
    /// 每个交换链图像上「已记录拷贝、等待读回」的相机矩阵。
    pub(super) pending_view_proj: Vec<Option<[[f32; 4]; 4]>>,
    pub(super) counts: crate::culling::OcclusionCounts,
}

impl GraphRenderer {
    /// 开关遮挡剔除（#1007）。
    ///
    /// 关闭时立即丢弃已回读的粗深度并停止深度拷贝，下一帧回到旧行为。
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

    /// 最近一帧遮挡剔除的统计（报告 / 调试用）。
    pub fn occlusion_counts(&self) -> crate::culling::OcclusionCounts {
        self.occlusion.counts
    }

    /// 上一帧的粗深度与生成它的相机矩阵；没有可用数据时返回 `None`。
    pub fn occlusion_view(&self) -> Option<(&crate::culling::DepthTiles, [[f32; 4]; 4])> {
        if !self.occlusion.enabled {
            return None;
        }
        Some((
            self.occlusion.tiles.as_ref()?,
            self.occlusion.tiles_view_proj?,
        ))
    }

    /// 用上一帧粗深度把关掉的绘制项标记成主通道不可见，并记录统计。
    ///
    /// 调用方在把绘制列表交给 `prepare` / `execute` 之前调用；没有可用深度
    /// （第一帧、关闭、读回失败）时原样返回。
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

    /// 确保当前图像在 `extent` 下有一个可写的深度回读缓冲。
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

    /// 读回上一帧写入的深度并生成粗深度。`begin_frame` 已经等过该图像的
    /// 围栏，所以这里映射内存不会和 GPU 抢写。
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
