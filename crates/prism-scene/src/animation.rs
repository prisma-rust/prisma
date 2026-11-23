//! Keyframe animation: track data and sampling.
//!
//! Engine originally only had 2D sprite frame animation ([`crate::components::SpriteAnimation`], converting
//! progress into `SpriteRenderer.frame_index`). This module adds minimal viable
//! face for **curve** animation: position / rotation / scale tracks, linear interpolation, endpoint hold (no extrapolation), loop and one-shot.
//!
//! Here only pure "time → value" functions, no ECS; playhead and writing back `Transform` on engine side
//! (`prism_engine::scene::systems::animation::keyframe_animation_system`).
//! Clip events ([`AnimationEvent`]) also here: just `(time, name)` data, fired on engine side.
//! Skeleton
//! Skinning not in this layer, but skeletal animation clip sampling fully consistent — skeleton only writes sample result to
//! bone node `Transform`.
//!
//! Conventions:
//!
//! - Keyframes sorted by `time` ascending (editor sorts on write); sampling uses binary search.
//! - Time axis both ends **hold endpoint values** (Unity `AnimationClip` default same no extrapolation),
//!   so before first frame / after last frame are constant.
//! - Rotation uses XYZ Euler angles (**degrees**), matching [`crate::components::DirectionalLight`]
//!   `euler_xyz` convention: hand-written JSON and editor both readable.

use prism_reflect::Reflect;
use serde::{Deserialize, Serialize};

/// Single keyframe on position / scale / Euler-angle track.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
#[reflect(name = "prism_engine::animation::Vec3Key")]
pub struct Vec3Key {
    /// Keyframe time (seconds, relative to clip start).
    pub time: f32,
    /// Value; rotation track holds XYZ Euler angles (degrees).
    pub value: [f32; 3],
}

impl Default for Vec3Key {
    fn default() -> Self {
        Self {
            time: 0.0,
            value: [0.0; 3],
        }
    }
}

impl Vec3Key {
    pub fn new(time: f32, value: [f32; 3]) -> Self {
        Self { time, value }
    }
}

/// One event on clip (#490): fired by engine when playhead crosses `time`.
///
/// Event name is just string, engine does not interpret semantics; gameplay maps name to sound / VFX / state switch.
/// Time semantics same as keyframes (seconds, relative to clip start); non-finite `time` events ignored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
#[reflect(name = "prism_engine::animation::AnimationEvent")]
pub struct AnimationEvent {
    /// Trigger time (seconds, relative to clip start).
    pub time: f32,
    /// Event name (gameplay custom).
    pub name: String,
}

impl Default for AnimationEvent {
    fn default() -> Self {
        Self {
            time: 0.0,
            name: String::new(),
        }
    }
}

impl AnimationEvent {
    pub fn new(time: f32, name: impl Into<String>) -> Self {
        Self {
            time,
            name: name.into(),
        }
    }
}

/// One sampling result.
///
/// `None` = that channel **has no keyframes**, caller must preserve author-set value (cannot let empty track zero out
/// author rotation). Three channels independent, which is prerequisite for common "position only" usage
/// case.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ClipSample {
    pub position: Option<[f32; 3]>,
    /// XYZ Euler angles, **degrees**.
    pub euler_xyz: Option<[f32; 3]>,
    pub scale: Option<[f32; 3]>,
}

impl ClipSample {
    /// Whether any channel was sampled.
    pub fn is_empty(&self) -> bool {
        self.position.is_none() && self.euler_xyz.is_none() && self.scale.is_none()
    }
}

/// Time of last keyframe in a track (empty track `0.0`).
pub fn max_key_time(keys: &[Vec3Key]) -> f32 {
    keys.iter()
        .map(|key| key.time)
        .filter(|time| time.is_finite())
        .fold(0.0f32, f32::max)
}

/// Sample value at `time` on one track.
///
/// Empty track returns `None`; with single keyframe or time before first / after last returns endpoint
/// value; otherwise linearly interpolates between adjacent frames. Non-finite `time` treated as `0`.
pub fn sample_vec3(keys: &[Vec3Key], time: f32) -> Option<[f32; 3]> {
    let first = keys.first()?;
    let time = if time.is_finite() { time } else { 0.0 };
    if time <= first.time || keys.len() == 1 {
        return Some(first.value);
    }
    // Last index with `time <= keys[i].time` (`time > keys[0].time` guarantees exists).
    let mut low = 0usize;
    let mut high = keys.len();
    while low < high {
        let mid = (low + high) / 2;
        if keys[mid].time <= time {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    let index = low - 1;
    let start = &keys[index];
    let Some(end) = keys.get(index + 1) else {
        return Some(start.value);
    };
    let span = end.time - start.time;
    if span <= f32::EPSILON {
        // Two keyframes at same moment (or out of order): use first one, avoid division by zero amplification.
        return Some(start.value);
    }
    let t = ((time - start.time) / span).clamp(0.0, 1.0);
    Some([
        start.value[0] + (end.value[0] - start.value[0]) * t,
        start.value[1] + (end.value[1] - start.value[1]) * t,
        start.value[2] + (end.value[2] - start.value[2]) * t,
    ])
}

/// Sort by time ascending (stable), keyframes at same moment keep insertion order.
pub fn sort_keys(keys: &mut [Vec3Key]) {
    keys.sort_by(|a, b| a.time.total_cmp(&b.time));
}

#[cfg(test)]
#[path = "animation_tests.rs"]
mod tests;
