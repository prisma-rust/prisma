//! Per-pass GPU timing (`vk::QueryPool` + `TIMESTAMP`, #518 Phase 1).
//!
//! Usage ([`crate::graph_renderer::GraphRenderer`] already does this):
//!
//! 1. `begin_frame(device, frame_index)`: clear queries for this frame slot (previous frame fence already waited,
//!    so safe now), and read results left by previous round into [`GpuTimings`];
//! 2. call `begin_pass` / `end_pass` at each pass during recording (writes two timestamps);
//! 3. next frame same slot returns to step 1 — results read back one frame late, no stall.
//!
//! When device does not support timestamps (`timestampPeriod == 0` or graphics queue without `TIMESTAMP`)
//! [`GpuTimer::new`] returns `Ok(None)`, caller treats as "no GPU timing",
//! falls back to CPU wall clock (`frame_time_ms`).

use ash::vk;

/// GPU time of one pass.
#[derive(Debug, Clone, PartialEq)]
pub struct PassTiming {
    pub name: String,
    pub ms: f32,
}

/// GPU timing results for one frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GpuTimings {
    /// Whether device supports timestamp queries (false = entire report empty).
    pub supported: bool,
    /// Whole-frame GPU span (recording start to end, ms); 0 when no span available.
    ///
    /// This span covers all work recorded in command buffer, so it can expose time not covered by any pass
    /// (barriers, out-of-graph dispatch, copy, present transition).
    pub frame_ms: f32,
    /// Sum of all passes' GPU time (ms).
    pub total_ms: f32,
    /// Passes in recording order.
    pub passes: Vec<PassTiming>,
}

impl GpuTimings {
    pub fn is_empty(&self) -> bool {
        self.passes.is_empty()
    }

    /// Whole-frame GPU time: use frame span when available, otherwise fall back to sum of passes.
    ///
    /// Comparisons across runs and baseline reports should use this value, not `total_ms`.
    pub fn frame_or_total_ms(&self) -> f32 {
        if self.frame_ms > 0.0 {
            self.frame_ms
        } else {
            self.total_ms
        }
    }

    /// Portion of whole-frame span not covered by any pass (ms).
    ///
    /// Returns 0 when no frame span; caller should first check [`Self::has_frame_span`],
    /// otherwise 0 would be misread as "no out-of-graph overhead".
    pub fn unmeasured_ms(&self) -> f32 {
        if self.frame_ms > 0.0 {
            (self.frame_ms - self.total_ms).max(0.0)
        } else {
            0.0
        }
    }

    /// Whether a whole-frame span was obtained (possible to miss when device supports but one frame's recording failed).
    pub fn has_frame_span(&self) -> bool {
        self.frame_ms > 0.0
    }

    /// Time of some pass (look up by name, for panel display).
    pub fn pass_ms(&self, name: &str) -> Option<f32> {
        self.passes
            .iter()
            .find(|pass| pass.name == name)
            .map(|pass| pass.ms)
    }
}

/// Default retains a few frames of total GPU time (≈2 seconds @60fps).
pub const DEFAULT_HISTORY_SAMPLES: usize = 120;

/// Ring buffer of total GPU time (#518 Phase 2 history chart).
///
/// Only stores **already read back** frames; when device lacks timestamps caller won't push, so empty buffer
/// itself is the "no GPU timing" signal. Non-finite / negative samples discarded, avoiding drawing
/// uninitialized or exceptional timestamps into the panel.
#[derive(Debug, Clone)]
pub struct GpuHistory {
    capacity: usize,
    samples: std::collections::VecDeque<f32>,
}

impl GpuHistory {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            samples: std::collections::VecDeque::new(),
        }
    }

    /// Append one frame's total GPU time (ms); when over capacity drop oldest frame.
    pub fn push(&mut self, ms: f32) {
        if !ms.is_finite() || ms < 0.0 {
            return;
        }
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(ms);
    }

    /// Iterate from old to new.
    pub fn samples(&self) -> impl ExactSizeIterator<Item = f32> + '_ {
        self.samples.iter().copied()
    }

    pub fn to_vec(&self) -> Vec<f32> {
        self.samples.iter().copied().collect()
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

impl Default for GpuHistory {
    fn default() -> Self {
        Self::new(DEFAULT_HISTORY_SAMPLES)
    }
}

/// Max passes recordable per frame slot; over-limit ones not timed (no panic).
pub const MAX_PASSES_PER_FRAME: usize = 24;

/// Extra queries reserved per frame slot: start and end of whole-frame span.
const SPAN_QUERIES: usize = 2;

/// Queries occupied per frame slot (pass pairs + whole-frame span).
const fn queries_per_slot() -> usize {
    MAX_PASSES_PER_FRAME * 2 + SPAN_QUERIES
}

/// Timestamp query pool.
#[derive(Debug)]
pub struct GpuTimer {
    pool: vk::QueryPool,
    /// `timestampPeriod` (nanoseconds / tick).
    period_ns: f32,
    slots: usize,
    /// Pass names recorded in phase order this frame, read at frame end.
    names: Vec<Vec<String>>,
    /// Number of passes recorded this frame.
    counts: Vec<usize>,
    /// Whether this slot needs `cmd_reset_query_pool` first during this frame's recording.
    ///
    /// Device-level `reset_query_pool` requires Vulkan 1.2 (`hostQueryReset`), so clearing done via
    /// `cmd_reset_query_pool` inside command buffer — available on all targets.
    needs_reset: Vec<bool>,
    /// Whether whole-frame span start already written.
    span_started: Vec<bool>,
    /// Whether whole-frame span fully written (start + end); only then read back.
    span_ended: Vec<bool>,
    /// Warn once when over pass limit, avoid spamming every frame.
    overflow_warned: bool,
    /// Most recently parsed result (one per frame slot).
    last: Vec<GpuTimings>,
}

impl GpuTimer {
    /// Create timer; returns `Ok(None)` when device lacks timestamp support.
    ///
    /// `slots` = frames in flight (each timeline an independent range, avoiding overlap).
    pub fn new(
        instance: &ash::Instance,
        device: &ash::Device,
        physical: vk::PhysicalDevice,
        slots: usize,
    ) -> anyhow::Result<Option<Self>> {
        let slots = slots.max(1);
        let properties = unsafe { instance.get_physical_device_properties(physical) };
        let period_ns = properties.limits.timestamp_period;
        if period_ns <= 0.0 {
            log::info!("GPU timings disabled: timestampPeriod is 0");
            return Ok(None);
        }
        let timestamp_bits =
            unsafe { instance.get_physical_device_queue_family_properties(physical) }
                .iter()
                .map(|family| family.timestamp_valid_bits)
                .max()
                .unwrap_or(0);
        if timestamp_bits == 0 {
            log::info!("GPU timings disabled: no queue family supports timestamps");
            return Ok(None);
        }

        let query_count = (slots * queries_per_slot()) as u32;
        let create_info = vk::QueryPoolCreateInfo::default()
            .query_type(vk::QueryType::TIMESTAMP)
            .query_count(query_count);
        let pool = match unsafe { device.create_query_pool(&create_info, None) } {
            Ok(pool) => pool,
            Err(error) => {
                log::warn!("GPU timings disabled: query pool creation failed ({error})");
                return Ok(None);
            }
        };

        Ok(Some(Self {
            pool,
            period_ns,
            slots,
            names: vec![Vec::new(); slots],
            counts: vec![0; slots],
            needs_reset: vec![true; slots],
            span_started: vec![false; slots],
            span_ended: vec![false; slots],
            overflow_warned: false,
            last: vec![GpuTimings::default(); slots],
        }))
    }

    /// Destroy query pool. Must be explicitly called by owner (`GraphRenderer`'s `Drop`).
    pub fn destroy(&mut self, device: &ash::Device) {
        if self.pool != vk::QueryPool::null() {
            unsafe { device.destroy_query_pool(self.pool, None) };
            self.pool = vk::QueryPool::null();
        }
    }

    /// Most recently parsed result (for some frame slot).
    pub fn timings(&self, frame_index: u32) -> &GpuTimings {
        let slot = (frame_index as usize) % self.slots;
        &self.last[slot]
    }

    /// Start a frame: read back previous round's result, then clear this slot's timestamps.
    ///
    /// Must be called **after fence for this frame slot has been waited** (`begin_frame` does so).
    pub fn begin_frame(&mut self, device: &ash::Device, frame_index: u32) {
        let slot = (frame_index as usize) % self.slots;
        let count = self.counts[slot];
        let has_span = self.span_started[slot] && self.span_ended[slot];
        self.last[slot] = if count > 0 || has_span {
            self.read_results(device, slot, count, has_span)
        } else {
            GpuTimings::default()
        };
        self.names[slot].clear();
        self.counts[slot] = 0;
        self.needs_reset[slot] = true;
        self.span_started[slot] = false;
        self.span_ended[slot] = false;
    }

    /// Record whole-frame span start. Must be called before all commands of the frame.
    ///
    /// Returns `false` meaning this frame already has span start (duplicate call ignored).
    pub fn begin_frame_span(
        &mut self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        frame_index: u32,
    ) -> bool {
        let slot = (frame_index as usize) % self.slots;
        if self.span_started[slot] {
            return false;
        }
        self.reset_slot_queries(device, cmd, slot);
        unsafe {
            device.cmd_write_timestamp(
                cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                self.pool,
                self.span_query_base(slot),
            );
        }
        self.span_started[slot] = true;
        true
    }

    /// Record whole-frame span end. Must be called after all commands of the frame and before `cmd_end_command_buffer`.
    ///
    /// Only writes timestamp when start already written and end not yet written; does nothing without start,
    /// so frames with mid-recording failure won't leave half a span.
    pub fn end_frame_span(
        &mut self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        frame_index: u32,
    ) {
        let slot = (frame_index as usize) % self.slots;
        if !self.span_started[slot] || self.span_ended[slot] {
            return;
        }
        unsafe {
            device.cmd_write_timestamp(
                cmd,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                self.pool,
                self.span_query_base(slot) + 1,
            );
        }
        self.span_ended[slot] = true;
    }

    /// Record a pass's start timestamp. Passes over per-frame limit ignored (returns `false`).
    pub fn begin_pass(
        &mut self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        frame_index: u32,
        name: &str,
    ) -> bool {
        let slot = (frame_index as usize) % self.slots;
        let index = self.counts[slot];
        if index >= MAX_PASSES_PER_FRAME {
            if !self.overflow_warned {
                self.overflow_warned = true;
                log::warn!(
                    "GPU timings: more than {MAX_PASSES_PER_FRAME} passes in one frame; \
                     extra passes are not timed"
                );
            }
            return false;
        }
        self.reset_slot_queries(device, cmd, slot);
        let query = self.slot_base(slot) + (index * 2) as u32;
        unsafe {
            device.cmd_write_timestamp(cmd, vk::PipelineStageFlags::TOP_OF_PIPE, self.pool, query);
        }
        self.names[slot].push(name.to_string());
        self.counts[slot] = index + 1;
        true
    }

    /// Record previous pass's end timestamp.
    pub fn end_pass(&mut self, device: &ash::Device, cmd: vk::CommandBuffer, frame_index: u32) {
        let slot = (frame_index as usize) % self.slots;
        let index = self.counts[slot];
        if index == 0 {
            return;
        }
        let query = self.slot_base(slot) + ((index - 1) * 2 + 1) as u32;
        unsafe {
            device.cmd_write_timestamp(
                cmd,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                self.pool,
                query,
            );
        }
    }

    /// Clear this slot's queries once before first write this frame. Previous round's results already read
    /// in `begin_frame`, so clearing here is safe.
    fn reset_slot_queries(&mut self, device: &ash::Device, cmd: vk::CommandBuffer, slot: usize) {
        if !self.needs_reset[slot] {
            return;
        }
        unsafe {
            device.cmd_reset_query_pool(
                cmd,
                self.pool,
                self.slot_base(slot),
                queries_per_slot() as u32,
            );
        }
        self.needs_reset[slot] = false;
    }

    fn slot_base(&self, slot: usize) -> u32 {
        (slot * queries_per_slot()) as u32
    }

    fn span_query_base(&self, slot: usize) -> u32 {
        self.slot_base(slot) + (MAX_PASSES_PER_FRAME * 2) as u32
    }

    fn read_results(
        &self,
        device: &ash::Device,
        slot: usize,
        count: usize,
        read_span: bool,
    ) -> GpuTimings {
        let base = self.slot_base(slot);
        let names = &self.names[slot];
        let timed = count.min(names.len());

        let mut pass_data = vec![0u64; timed * 2];
        let result = unsafe {
            device.get_query_pool_results(
                self.pool,
                base,
                &mut pass_data,
                vk::QueryResultFlags::TYPE_64,
            )
        };
        if let Err(error) = result {
            log::debug!("GPU timing readback skipped: {error}");
            return GpuTimings {
                supported: true,
                ..Default::default()
            };
        }

        let mut passes = Vec::with_capacity(timed);
        let mut total = 0.0f32;
        for index in 0..timed {
            let begin = pass_data[index * 2];
            let end = pass_data[index * 2 + 1];
            if end <= begin {
                continue;
            }
            let ticks = (end - begin) as f32;
            let ms = ticks * self.period_ns / 1_000_000.0;
            total += ms;
            passes.push(PassTiming {
                name: names[index].clone(),
                ms,
            });
        }

        let mut frame_ms = 0.0f32;
        if read_span {
            let mut span_data = [0u64; 2];
            let span_result = unsafe {
                device.get_query_pool_results(
                    self.pool,
                    self.span_query_base(slot),
                    &mut span_data,
                    vk::QueryResultFlags::TYPE_64,
                )
            };
            if let Err(error) = span_result {
                log::debug!("GPU frame-span readback skipped: {error}");
            } else if span_data[1] > span_data[0] {
                frame_ms = (span_data[1] - span_data[0]) as f32 * self.period_ns / 1_000_000.0;
            }
        }

        GpuTimings {
            supported: true,
            frame_ms,
            total_ms: total,
            passes,
        }
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;

    #[test]
    fn frame_span_prefers_the_whole_frame_and_falls_back_to_the_pass_sum() {
        let mut timings = GpuTimings {
            supported: true,
            frame_ms: 15.0,
            total_ms: 12.0,
            passes: vec![PassTiming {
                name: "forward".into(),
                ms: 12.0,
            }],
        };
        assert_eq!(timings.frame_or_total_ms(), 15.0);
        assert!((timings.unmeasured_ms() - 3.0).abs() < 1e-4);
        assert!(timings.has_frame_span());

        timings.frame_ms = 0.0;
        assert_eq!(timings.frame_or_total_ms(), 12.0);
        assert_eq!(timings.unmeasured_ms(), 0.0);
        assert!(!timings.has_frame_span());
    }

    #[test]
    fn unmeasured_gap_never_goes_negative() {
        let timings = GpuTimings {
            supported: true,
            frame_ms: 10.0,
            total_ms: 10.01,
            passes: Vec::new(),
        };
        assert_eq!(timings.unmeasured_ms(), 0.0);
    }

    #[test]
    fn history_keeps_the_newest_samples_up_to_capacity() {
        let mut history = GpuHistory::new(3);
        for ms in [1.0, 2.0, 3.0, 4.0] {
            history.push(ms);
        }
        assert_eq!(history.to_vec(), vec![2.0, 3.0, 4.0]);
        assert_eq!(history.len(), 3);
        assert_eq!(history.capacity(), 3);
    }

    #[test]
    fn history_ignores_invalid_samples_and_zero_capacity_is_clamped() {
        let mut history = GpuHistory::new(0);
        assert_eq!(history.capacity(), 1);
        history.push(f32::NAN);
        history.push(f32::INFINITY);
        history.push(-1.0);
        assert!(history.is_empty());
        history.push(0.0);
        history.push(1.5);
        assert_eq!(history.to_vec(), vec![1.5]);
        assert_eq!(history.samples().sum::<f32>(), 1.5);
    }
}
