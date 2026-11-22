//! Animation clip: keyframe tracks and sampling.

use glam::{Quat, Vec3};

use crate::pose::Pose;
use crate::sync::SyncMarkerSet;

/// A single channel keyframe for one bone (time in ascending order).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3Key {
    pub time: f32,
    pub value: Vec3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuatKey {
    pub time: f32,
    pub value: Quat,
}

/// One track: the position / rotation / scale channels of one bone.
///
/// An empty channel means "this clip doesn't move this channel"; sampling keeps the value
/// already in the pose.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Track {
    pub joint: usize,
    pub translation: Vec<Vec3Key>,
    pub rotation: Vec<QuatKey>,
    pub scale: Vec<Vec3Key>,
}

impl Track {
    pub fn is_empty(&self) -> bool {
        self.translation.is_empty() && self.rotation.is_empty() && self.scale.is_empty()
    }
}

/// A keyframe event in the clip (#560 P1 Notify).
///
/// `name` is interpreted by the project (`Footstep.L` / `Traversal.Window` / …): the animation
/// layer is only responsible for the fact that "this time point was crossed in this frame",
/// not for what it means.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipNotify {
    /// Trigger time (seconds, clip-local time).
    pub time: f32,
    pub name: String,
}

/// A Montage segment (#560 P4): jump to by name / show while debugging.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipSection {
    pub name: String,
    /// Segment start (seconds, clip-local time).
    pub start: f32,
    /// Segment end (seconds, clip-local time); may equal `start`.
    pub end: f32,
}

impl ClipSection {
    /// Whether this time point falls inside the segment (left-closed right-open; zero-length
    /// segment hits `start`).
    pub fn contains(&self, time: f32) -> bool {
        if !time.is_finite() || !self.start.is_finite() || !self.end.is_finite() {
            return false;
        }
        if self.end <= self.start {
            (time - self.start).abs() <= f32::EPSILON
        } else {
            time >= self.start && time < self.end
        }
    }
}

/// A MotionWarping window (#560 P4): root motion is only allowed to be warped by target
/// point / direction within this time interval.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipWarpWindow {
    pub name: String,
    pub start: f32,
    pub end: f32,
}

impl ClipWarpWindow {
    pub fn contains(&self, time: f32) -> bool {
        if !time.is_finite() || !self.start.is_finite() || !self.end.is_finite() {
            return false;
        }
        if self.end <= self.start {
            (time - self.start).abs() <= f32::EPSILON
        } else {
            time >= self.start && time < self.end
        }
    }

    /// Normalized progress within the window; a zero-length window returns 1 on hit.
    pub fn progress(&self, time: f32) -> Option<f32> {
        if !self.contains(time) {
            return None;
        }
        if self.end <= self.start {
            return Some(1.0);
        }
        Some(((time - self.start) / (self.end - self.start)).clamp(0.0, 1.0))
    }
}

/// An animation clip.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Clip {
    pub name: String,
    pub duration: f32,
    pub fps: f32,
    pub looped: bool,
    pub additive: bool,
    pub tracks: Vec<Track>,
    /// Keyframe events (time ascending guaranteed by `from_info` / encoder, not sorted here).
    pub notifies: Vec<ClipNotify>,
    /// Normalized phases of sync markers (ascending; `from_info` guarantees finite, deduplicated).
    pub sync_markers: Vec<f32>,
    /// Montage segments (ascending by `start`; empty for regular clips).
    pub sections: Vec<ClipSection>,
    /// MotionWarping windows (ascending by `start`; empty for regular clips).
    pub warp_windows: Vec<ClipWarpWindow>,
}

impl Clip {
    pub fn from_info(info: &prism_asset::formats_skinned::AnimClipInfo) -> Self {
        let tracks = info
            .tracks
            .iter()
            .map(|track| Track {
                joint: track.joint as usize,
                translation: track
                    .positions
                    .iter()
                    .filter(|key| key.time.is_finite() && key.value.iter().all(|v| v.is_finite()))
                    .map(|key| Vec3Key {
                        time: key.time,
                        value: Vec3::from_array(key.value),
                    })
                    .collect(),
                rotation: track
                    .rotations
                    .iter()
                    .filter(|key| key.time.is_finite() && key.value.iter().all(|v| v.is_finite()))
                    .map(|key| QuatKey {
                        time: key.time,
                        value: normalize_quat(Quat::from_array(key.value)),
                    })
                    .collect(),
                scale: track
                    .scales
                    .iter()
                    .filter(|key| key.time.is_finite() && key.value.iter().all(|v| v.is_finite()))
                    .map(|key| Vec3Key {
                        time: key.time,
                        value: Vec3::from_array(key.value),
                    })
                    .collect(),
            })
            .collect();
        Self {
            name: info.name.clone(),
            duration: if info.duration.is_finite() && info.duration > 0.0 {
                info.duration
            } else {
                info.clip_duration()
            },
            fps: info.fps,
            looped: info.looped,
            additive: info.additive,
            tracks,
            notifies: info
                .notifies
                .iter()
                .filter(|notify| notify.time.is_finite() && !notify.name.is_empty())
                .map(|notify| ClipNotify {
                    time: notify.time,
                    name: notify.name.clone(),
                })
                .collect(),
            sync_markers: info
                .sync_markers
                .iter()
                .copied()
                .filter(|phase| phase.is_finite())
                .map(|phase| phase.rem_euclid(1.0))
                .collect(),
            sections: info
                .sections
                .iter()
                .filter(|section| {
                    !section.name.trim().is_empty()
                        && section.start.is_finite()
                        && section.end.is_finite()
                        && section.start >= 0.0
                        && section.end >= section.start
                })
                .map(|section| ClipSection {
                    name: section.name.clone(),
                    start: section.start,
                    end: section.end,
                })
                .collect(),
            warp_windows: info
                .warp_windows
                .iter()
                .filter(|window| {
                    !window.name.trim().is_empty()
                        && window.start.is_finite()
                        && window.end.is_finite()
                        && window.start >= 0.0
                        && window.end >= window.start
                })
                .map(|window| ClipWarpWindow {
                    name: window.name.clone(),
                    start: window.start,
                    end: window.end,
                })
                .collect(),
        }
    }

    /// The sync markers of this clip; returns `None` when there are no markers.
    pub fn sync_marker_set(&self) -> Option<SyncMarkerSet> {
        SyncMarkerSet::new(self.sync_markers.iter().copied())
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.iter().all(Track::is_empty)
    }

    /// Events crossed by this frame: interval `(from, to]`; for looped clips crossing the end,
    /// `[0, to]` is also counted.
    ///
    /// `from` / `to` are both **converted local times** (`0..=duration`). Taking the start as an
    /// open interval is intentional: an event that already fired at exactly `from` would replay
    /// repeatedly on a paused frame if a closed interval were used.
    /// Results are sorted by time ascending.
    pub fn notifies_between(&self, from: f32, to: f32) -> Vec<&ClipNotify> {
        if self.notifies.is_empty() || !from.is_finite() || !to.is_finite() {
            return Vec::new();
        }
        let duration = self.duration;
        if !duration.is_finite() || duration <= 0.0 {
            return Vec::new();
        }
        let mut fired: Vec<&ClipNotify> = Vec::new();
        // `to < from` can only be loop wrap-around (after `rem_euclid`), scan in two segments.
        let wrapped = to < from;
        for notify in &self.notifies {
            let hit = if wrapped {
                (notify.time > from && notify.time <= duration)
                    || (notify.time >= 0.0 && notify.time <= to)
            } else {
                notify.time > from && notify.time <= to
            };
            if hit {
                fired.push(notify);
            }
        }
        fired.sort_by(|a, b| a.time.total_cmp(&b.time));
        fired
    }

    /// Convert local time to sampling time: looped clips take the remainder, one-shot clips clamp
    /// to `[0, duration]`.
    pub fn wrap_time(&self, time: f32) -> f32 {
        if !time.is_finite() {
            return 0.0;
        }
        if self.duration <= 0.0 {
            return 0.0;
        }
        if self.looped {
            time.rem_euclid(self.duration)
        } else {
            time.clamp(0.0, self.duration)
        }
    }

    /// Write the clip's sampling at `time` into `pose`, returning whether any channel was written.
    ///
    /// `time` is **local time**: the caller decides whether to first pass through [`Clip::wrap_time`]
    /// (the `Animator` component may override the clip's own loop setting, so we don't wrap it here).
    pub fn sample_into(&self, time: f32, pose: &mut Pose) -> bool {
        let time = if time.is_finite() { time } else { 0.0 };
        let mut touched = false;
        for track in &self.tracks {
            let Some(current) = pose.local(track.joint) else {
                continue;
            };
            let mut next = current;
            if let Some(value) = sample_vec3(&track.translation, time) {
                next.translation = value;
                touched = true;
            }
            if let Some(value) = sample_quat(&track.rotation, time) {
                next.rotation = value;
                touched = true;
            }
            if let Some(value) = sample_vec3(&track.scale, time) {
                next.scale = value;
                touched = true;
            }
            if next != current {
                pose.set_local(track.joint, next);
            }
        }
        touched
    }

    /// Sample only one joint (animation LOD for root bone / root motion only), returning whether
    /// the joint has a track.
    ///
    /// Joints not sampled keep their values in `pose`; when a track is missing a channel
    /// (translation / rotation / scale), that channel doesn't move either, consistent with the
    /// semantics of [`Self::sample_into`]. Out-of-range joints return `false`.
    pub fn sample_joint_into(&self, time: f32, joint: usize, pose: &mut Pose) -> bool {
        let time = if time.is_finite() { time } else { 0.0 };
        let Some(current) = pose.local(joint) else {
            return false;
        };
        let mut touched = false;
        let mut next = current;
        for track in &self.tracks {
            if track.joint != joint {
                continue;
            }
            if let Some(value) = sample_vec3(&track.translation, time) {
                next.translation = value;
                touched = true;
            }
            if let Some(value) = sample_quat(&track.rotation, time) {
                next.rotation = value;
                touched = true;
            }
            if let Some(value) = sample_vec3(&track.scale, time) {
                next.scale = value;
                touched = true;
            }
        }
        if next != current {
            pose.set_local(joint, next);
        }
        touched
    }

    /// First convert time by the clip's own `looped`, then sample (convenient entry point when
    /// using the clip standalone).
    pub fn sample_wrapped_into(&self, time: f32, pose: &mut Pose) -> bool {
        let local = self.wrap_time(time);
        self.sample_into(local, pose)
    }

    /// Upper bound of time covered by tracks (`duration` when there are no keyframes).
    /// All keyframe time points used by tracks (ascending, deduplicated, finite values only).
    ///
    /// Used by editors to draw timeline ticks: keyframes across tracks in a clip are often
    /// staggered, and only after merging is it "the moments this clip changes".
    pub fn key_times(&self) -> Vec<f32> {
        let mut times: Vec<f32> = Vec::new();
        for track in &self.tracks {
            for key in &track.translation {
                times.push(key.time);
            }
            for key in &track.rotation {
                times.push(key.time);
            }
            for key in &track.scale {
                times.push(key.time);
            }
        }
        times.retain(|time| time.is_finite());
        times.sort_by(|a, b| a.total_cmp(b));
        times.dedup_by(|a, b| (*a - *b).abs() <= 1e-6);
        times
    }

    pub fn max_key_time(&self) -> f32 {
        let mut max = 0.0f32;
        for track in &self.tracks {
            for key in &track.translation {
                max = max.max(key.time);
            }
            for key in &track.rotation {
                max = max.max(key.time);
            }
            for key in &track.scale {
                max = max.max(key.time);
            }
        }
        max
    }
}

/// The value at time `time`: hold endpoints before the first / after the last keyframe,
/// linear interpolation in between.
pub fn sample_vec3(keys: &[Vec3Key], time: f32) -> Option<Vec3> {
    let (first, rest) = keys.split_first()?;
    if rest.is_empty() || time <= first.time {
        return Some(first.value);
    }
    let last = rest.last()?;
    if time >= last.time {
        return Some(last.value);
    }
    match find_span(keys, time, |key| key.time) {
        Some((before, after)) => {
            let span = after.time - before.time;
            if span <= f32::EPSILON {
                Some(before.value)
            } else {
                let t = (time - before.time) / span;
                Some(before.value.lerp(after.value, t))
            }
        }
        // Only one keyframe covers this time point.
        None => Some(keys[0].value),
    }
}

/// Quaternion version: slerp (automatically takes the short arc).
pub fn sample_quat(keys: &[QuatKey], time: f32) -> Option<Quat> {
    let (first, rest) = keys.split_first()?;
    if rest.is_empty() || time <= first.time {
        return Some(first.value);
    }
    let last = rest.last()?;
    if time >= last.time {
        return Some(last.value);
    }
    match find_span(keys, time, |key| key.time) {
        Some((before, after)) => {
            let span = after.time - before.time;
            if span <= f32::EPSILON {
                Some(before.value)
            } else {
                let t = (time - before.time) / span;
                Some(normalize_quat(before.value.slerp(after.value, t)))
            }
        }
        None => Some(keys[0].value),
    }
}

/// Binary search for the last frame with `key.time <= time` and its successor.
fn find_span<T>(keys: &[T], time: f32, time_of: impl Fn(&T) -> f32) -> Option<(&T, &T)> {
    let mut low = 0usize;
    let mut high = keys.len() - 1;
    while low + 1 < high {
        let middle = (low + high) / 2;
        if time_of(&keys[middle]) <= time {
            low = middle;
        } else {
            high = middle;
        }
    }
    Some((&keys[low], &keys[high]))
}

fn normalize_quat(rotation: Quat) -> Quat {
    if rotation.length_squared() > 1e-12 {
        rotation.normalize()
    } else {
        Quat::IDENTITY
    }
}

#[cfg(test)]
#[path = "clip_tests.rs"]
mod tests;
