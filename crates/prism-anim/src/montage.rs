//! Montage playback queue (#560 P1).
//!
//! Montage is a one-shot / looping overlay animation (hit reaction, vault, fire). This layer
//! only handles playback state: immediate play, FIFO queueing, auto-advance to the next segment
//! on single-shot end, `stop` / `stop_after_current`, and cross-frame Notify. Sampling still
//! reuses [`Clip`]; pose composition is decided by the caller (graph Slot node or engine).
//!
//! Unlike `Animator`'s regular clips, Montage carries its own start time / loop / speed / weight /
//! blend-in / blend-out; the playhead belongs only to this player and does not overwrite authored
//! scene data.

use std::collections::VecDeque;
use std::sync::Arc;

use crate::clip::{Clip, ClipSection, ClipWarpWindow};
use crate::pose::Pose;

/// Queue upper bound: bad logic (enqueueing every frame) should not eat memory without limit.
pub const MAX_MONTAGE_QUEUE: usize = 32;

/// Montage BranchIn Notify prefix: `BranchIn.<section>` jumps to the same-named segment's
/// start when the event is crossed (#560 P4). If the segment doesn't exist, the event is still
/// emitted but no jump occurs.
pub const BRANCH_IN_PREFIX: &str = "BranchIn.";

/// A queued-up Montage waiting to be played.
#[derive(Debug, Clone)]
pub struct Montage {
    clip: Arc<Clip>,
    start_time: f32,
    /// Which Montage segment of the clip to start from; falls back to `start_time` if not found.
    start_section: Option<String>,
    looped: bool,
    speed: f32,
    weight: f32,
    /// Starting fade-in duration (seconds); 0 = immediately full weight.
    blend_in: f32,
    /// Fade-out duration (seconds) before a single-shot Montage ends; not used for looped Montages.
    blend_out: f32,
}

impl Montage {
    /// Default: start from 0, looped, speed 1, weight 1.
    pub fn new(clip: Arc<Clip>) -> Self {
        Self {
            clip,
            start_time: 0.0,
            start_section: None,
            looped: true,
            speed: 1.0,
            weight: 1.0,
            blend_in: 0.0,
            blend_out: 0.0,
        }
    }

    pub fn with_start_time(mut self, start_time: f32) -> Self {
        self.start_time = start_time;
        self
    }

    /// Start playback from the specified Montage segment; falls back to `start_time` if the segment
    /// doesn't exist.
    pub fn with_start_section(mut self, section: impl Into<String>) -> Self {
        let section = section.into();
        self.start_section = (!section.trim().is_empty()).then_some(section);
        self
    }

    pub fn with_looping(mut self, looped: bool) -> Self {
        self.looped = looped;
        self
    }

    pub fn with_speed(mut self, speed: f32) -> Self {
        self.speed = speed;
        self
    }

    pub fn with_weight(mut self, weight: f32) -> Self {
        self.weight = weight;
        self
    }

    /// Starting fade-in duration (seconds); negative / non-finite values treated as 0.
    pub fn with_blend_in(mut self, seconds: f32) -> Self {
        self.blend_in = sanitize_duration(seconds);
        self
    }

    /// Fade-out duration (seconds) before a single-shot Montage ends; looped Montages do not use it.
    pub fn with_blend_out(mut self, seconds: f32) -> Self {
        self.blend_out = sanitize_duration(seconds);
        self
    }

    pub fn clip(&self) -> &Arc<Clip> {
        &self.clip
    }
}

/// A crossed Montage Notify.
#[derive(Debug, Clone, PartialEq)]
pub struct MontageNotify {
    /// Name of the clip the event belongs to (`Clip.name`, for mapping back to assets by upper layers).
    pub clip_name: String,
    pub name: String,
    /// Local time of the event within the clip (seconds).
    pub time: f32,
}

#[derive(Debug, Clone)]
struct ActiveMontage {
    montage: Montage,
    time: f32,
    /// Absolute duration this segment has played (seconds), used for blend-in; does not advance while paused.
    elapsed: f32,
    /// Baseline weight set by the author; actual weight is further multiplied by fade-in / fade-out factors.
    weight: f32,
}

impl ActiveMontage {
    /// Weight actually used for pose blending this frame.
    fn effective_weight(&self) -> f32 {
        let fade_in = if self.montage.blend_in > 0.0 {
            (self.elapsed / self.montage.blend_in).clamp(0.0, 1.0)
        } else {
            1.0
        };
        // Looped Montages return to the end every cycle; fading out by remaining time would
        // flash periodically, so blend-out is applied only on single-shot Montages.
        let fade_out = if !self.montage.looped && self.montage.blend_out > 0.0 {
            let local = local_time(self.montage.clip.duration, self.montage.looped, self.time);
            let remaining = (self.montage.clip.duration - local).max(0.0);
            (remaining / self.montage.blend_out).clamp(0.0, 1.0)
        } else {
            1.0
        };
        (self.weight * fade_in * fade_out).clamp(0.0, 1.0)
    }
}

/// Montage player: one active segment + a FIFO queue.
#[derive(Debug, Clone, Default)]
pub struct MontagePlayer {
    active: Option<ActiveMontage>,
    queue: VecDeque<Montage>,
    finished: bool,
    paused: bool,
    pending_notifies: Vec<MontageNotify>,
}

impl MontagePlayer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Play immediately (replaces the current segment, clears the queue). Returns whether accepted.
    pub fn play_now(&mut self, montage: Montage) -> bool {
        self.active = Some(start(montage));
        self.queue.clear();
        self.finished = false;
        self.paused = false;
        self.pending_notifies.clear();
        true
    }

    /// Queue; starts immediately when idle. Returns `false` and drops when the queue is full.
    pub fn enqueue(&mut self, montage: Montage) -> bool {
        if self.active.is_none() {
            self.active = Some(start(montage));
            self.finished = false;
            return true;
        }
        if self.queue.len() >= MAX_MONTAGE_QUEUE {
            return false;
        }
        self.queue.push_back(montage);
        true
    }

    /// Stop and clear the queue.
    pub fn stop(&mut self) {
        self.active = None;
        self.queue.clear();
        self.finished = false;
        self.paused = false;
        self.pending_notifies.clear();
    }

    /// Stop after the current segment finishes; do not take the next queued segment.
    pub fn stop_after_current(&mut self) {
        self.queue.clear();
    }

    pub fn is_playing(&self) -> bool {
        self.active.is_some()
    }

    /// Pause / resume the active segment (#560 Montage playback control). While paused, `tick`
    /// does not advance time, does not sample new Notifies; queue and weight remain unchanged.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Time already played by the active segment (seconds); returns `None` when there is no active segment.
    pub fn active_time(&self) -> Option<f32> {
        self.active.as_ref().map(|active| active.time)
    }

    /// The Montage segment at the current time; returns `None` when there is no active segment / not in any segment.
    pub fn active_section(&self) -> Option<&ClipSection> {
        let active = self.active.as_ref()?;
        let local = local_time(
            active.montage.clip.duration,
            active.montage.looped,
            active.time,
        );
        active
            .montage
            .clip
            .sections
            .iter()
            .find(|section| section.contains(local))
    }

    /// The MotionWarping window at the current time; returns `None` when there is no active segment / not in any window.
    pub fn active_warp_window(&self) -> Option<&ClipWarpWindow> {
        let active = self.active.as_ref()?;
        let local = local_time(
            active.montage.clip.duration,
            active.montage.looped,
            active.time,
        );
        active
            .montage
            .clip
            .warp_windows
            .iter()
            .find(|window| window.contains(local))
    }

    /// Normalized progress (0..=1) within the current warp window; returns `None` when not in a window.
    pub fn warp_window_progress(&self) -> Option<f32> {
        let active = self.active.as_ref()?;
        let local = local_time(
            active.montage.clip.duration,
            active.montage.looped,
            active.time,
        );
        active
            .montage
            .clip
            .warp_windows
            .iter()
            .find_map(|window| window.progress(local))
    }

    /// Jump to the start of the named Montage segment; returns `false` if the segment doesn't exist
    /// or there is no active segment.
    ///
    /// The paused state is unchanged: jumping segments while paused still stops at the segment start,
    /// and only continues advancing after resume.
    pub fn jump_to_section(&mut self, name: &str) -> bool {
        let Some(active) = self.active.as_mut() else {
            return false;
        };
        let Some(section) = active
            .montage
            .clip
            .sections
            .iter()
            .find(|section| section.name == name)
        else {
            return false;
        };
        active.time = section.start;
        self.pending_notifies.clear();
        true
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    pub fn active_clip(&self) -> Option<&Arc<Clip>> {
        self.active.as_ref().map(|active| &active.montage.clip)
    }

    /// Current actual blend weight (baseline weight × blend-in / blend-out factor).
    pub fn active_weight(&self) -> f32 {
        self.active
            .as_ref()
            .map_or(0.0, ActiveMontage::effective_weight)
    }

    pub fn set_active_weight(&mut self, weight: f32) {
        if let Some(active) = self.active.as_mut() {
            active.weight = sanitize_weight(weight);
        }
    }

    /// Advance time; a single-shot Montage auto-advances to the next segment when done. Returns
    /// whether still in a segment.
    pub fn tick(&mut self, dt: f32) -> bool {
        if self.paused {
            return self.active.is_some();
        }
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let Some(active) = self.active.as_mut() else {
            return false;
        };
        let previous_local = local_time(
            active.montage.clip.duration,
            active.montage.looped,
            active.time,
        );
        let speed = sanitize_speed(active.montage.speed);
        active.time += dt * speed;
        // blend-in is wall-clock duration, not scaled by playback speed; paused tick returns early
        // so it stays frozen.
        active.elapsed += dt;
        let local = local_time(
            active.montage.clip.duration,
            active.montage.looped,
            active.time,
        );
        let mut branch_target = None;
        if dt > 0.0 && speed >= 0.0 {
            for notify in active.montage.clip.notifies_between(previous_local, local) {
                if let Some(target_name) = notify.name.strip_prefix(BRANCH_IN_PREFIX) {
                    if let Some(target) = active
                        .montage
                        .clip
                        .sections
                        .iter()
                        .find(|section| section.name == target_name)
                    {
                        branch_target = Some(target.start);
                    }
                }
                self.pending_notifies.push(MontageNotify {
                    clip_name: active.montage.clip.name.clone(),
                    name: notify.name.clone(),
                    time: notify.time,
                });
            }
        }
        if let Some(start) = branch_target {
            // BranchIn is an explicit segment jump: no longer loop-wrapping, and not scanning
            // Notify in the target segment within the same frame.
            active.time = start;
        } else if active.montage.looped {
            active.time = local;
        }

        let finished =
            !active.montage.looped && speed > 0.0 && active.time >= active.montage.clip.duration;
        if finished {
            if let Some(next) = self.queue.pop_front() {
                self.active = Some(start(next));
                self.finished = false;
            } else {
                self.active = None;
                self.finished = true;
            }
            return self.active.is_some();
        }
        true
    }

    /// Sample the current Montage into `out` (resetting to reference first). Returns `false` when
    /// there is no active segment.
    pub fn sample_into(&self, reference: &Pose, out: &mut Pose) -> bool {
        let Some(active) = self.active.as_ref() else {
            return false;
        };
        out.copy_from(reference);
        let local = local_time(
            active.montage.clip.duration,
            active.montage.looped,
            active.time,
        );
        active.montage.clip.sample_into(local, out);
        true
    }

    /// Drain the Notify events crossed this frame (whole drain).
    pub fn take_notifies(&mut self) -> Vec<MontageNotify> {
        std::mem::take(&mut self.pending_notifies)
    }
}

fn start(montage: Montage) -> ActiveMontage {
    let section_start = montage
        .start_section
        .as_deref()
        .and_then(|name| {
            montage
                .clip
                .sections
                .iter()
                .find(|section| section.name == name)
        })
        .map(|section| section.start);
    let start_time = section_start.unwrap_or_else(|| {
        if montage.start_time.is_finite() {
            montage.start_time.max(0.0)
        } else {
            0.0
        }
    });
    ActiveMontage {
        weight: sanitize_weight(montage.weight),
        time: local_time(montage.clip.duration, montage.looped, start_time),
        elapsed: 0.0,
        montage,
    }
}

fn sanitize_weight(weight: f32) -> f32 {
    if weight.is_finite() {
        weight.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn sanitize_speed(speed: f32) -> f32 {
    if speed.is_finite() {
        speed
    } else {
        1.0
    }
}

fn sanitize_duration(seconds: f32) -> f32 {
    if seconds.is_finite() {
        seconds.max(0.0)
    } else {
        0.0
    }
}

fn local_time(duration: f32, looped: bool, time: f32) -> f32 {
    if duration <= 0.0 {
        return 0.0;
    }
    if looped {
        time.rem_euclid(duration)
    } else {
        time.clamp(0.0, duration)
    }
}

#[cfg(test)]
#[path = "montage_tests.rs"]
mod tests;
