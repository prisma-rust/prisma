//! Pose blending and 1D blend space (foundation of #560 P1).
//!
//! The animation graph, BlendStack, and BlendSpace all need the same thing: **blend two poses by weight**.
//! This layer only does math and does not depend on engine / render / ECS, so editor preview,
//! offline tools, and runtime can share the same implementation.
//!
//! Convention consistent with the rest of `prism-anim`: quaternion `xyzw`, right-handed +Y up,
//! `Pose` is per-joint **local** TRS (parent space).
//!
//! Two deliberate design choices:
//!
//! - Rotation uses `slerp` (shortest path), not component-wise lerp on quaternions — the latter
//!   makes bones "twitch" near 180°, which is exactly the most common failure mode of motion blending;
//! - When lengths differ, **blend only the shorter joint count**; extra joints keep `a`'s pose.
//!   When skeleton versions mismatch, prefer blending one fewer bone over panicking or collapsing
//!   missing joints to the origin.

use std::sync::Arc;

use glam::{Quat, Vec3};

use crate::clip::Clip;
use crate::pose::Pose;
use crate::skeleton::JointTransform;
use crate::sync::SyncGroup;

/// Clamp `t` to `[0, 1]` (NaN → 0).
fn clamp_weight(t: f32) -> f32 {
    if t.is_nan() {
        0.0
    } else {
        t.clamp(0.0, 1.0)
    }
}

/// Blend two joint transforms by weight: linear interpolation of translation/scale, shortest-path slerp of rotation.
pub fn blend_joint(a: &JointTransform, b: &JointTransform, t: f32) -> JointTransform {
    JointTransform {
        translation: a.translation.lerp(b.translation, t),
        rotation: a.rotation.slerp(b.rotation, t),
        scale: a.scale.lerp(b.scale, t),
    }
}

/// Write the blend of `a` → `b` into `out`, returning the actual number of joints blended.
///
/// `out` is resized to `a`'s length: extra joints (parts `b` does not have) keep `a`'s
/// pose, and joints originally extra in `out` are discarded. The returned joint count is the shorter of the two.
pub fn blend_into(out: &mut Pose, a: &Pose, b: &Pose, t: f32) -> usize {
    let t = clamp_weight(t);
    let shared = a.len().min(b.len());
    // `out`, `a`, and `b` are guaranteed to be distinct poses (borrow checker guarantees it),
    // so we can compute and write in place without an intermediate buffer.
    out.resize_with(a.len(), |index| {
        let from = a.local(index).unwrap_or(JointTransform::IDENTITY);
        match b.local(index).filter(|_| index < shared) {
            Some(to) => blend_joint(&from, &to, t),
            None => from,
        }
    });
    shared
}

/// Apply an overlay (additive) pose onto the base pose by weight, returning the actual number of joints processed.
///
/// Semantics match UE's "local space additive": the increment is the difference of the overlay clip's local transform relative to the **reference pose** —
///
/// - Translation: `base + (additive - reference) * t`
/// - Rotation: `base * slerp(identity, reference⁻¹ * additive, t)`
/// - Scale: `base * lerp(1, additive / reference, t)` (reference scale of 0 treated as 1)
///
/// At `t = 0` the result matches `base` exactly; at `t = 1` it is the full overlay result. `base` is overwritten in place,
/// without an intermediate buffer: when the overlay layer only changes some joints, the rest stay put.
///
/// This is the foundation of "Layer + Additive": clips that only affect some bones, like breathing,
/// hit flinch, aim offset, don't need a whole separate base clip (#560 P1).
pub fn apply_additive(base: &mut Pose, additive: &Pose, reference: &Pose, weight: f32) -> usize {
    let t = clamp_weight(weight);
    if t <= 0.0 {
        return 0;
    }
    let shared = base.len().min(additive.len()).min(reference.len());
    for index in 0..shared {
        let (Some(current), Some(layer), Some(rest)) = (
            base.local(index),
            additive.local(index),
            reference.local(index),
        ) else {
            continue;
        };
        base.set_local(index, additive_joint(&current, &layer, &rest, t));
    }
    shared
}

/// Single joint's overlay result (see [`apply_additive`]).
pub fn additive_joint(
    base: &JointTransform,
    additive: &JointTransform,
    reference: &JointTransform,
    weight: f32,
) -> JointTransform {
    let t = clamp_weight(weight);
    if t <= 0.0 {
        return *base;
    }
    // Inverse of the reference pose: rotation takes conjugate, scale takes reciprocal.
    let rest_rotation = reference.rotation.normalize();
    let delta_rotation = rest_rotation.inverse() * additive.rotation.normalize();
    let rotation =
        (base.rotation.normalize() * Quat::IDENTITY.slerp(delta_rotation, t)).normalize();

    let delta_translation = additive.translation - reference.translation;
    let translation = base.translation + delta_translation * t;

    let scale_ratio = |additive: f32, reference: f32| -> f32 {
        if reference.abs() <= 1e-6 {
            1.0
        } else {
            additive / reference
        }
    };
    let scale = Vec3::new(
        scale_ratio(additive.scale.x, reference.scale.x),
        scale_ratio(additive.scale.y, reference.scale.y),
        scale_ratio(additive.scale.z, reference.scale.z),
    );
    let scale = base.scale * scale.lerp(Vec3::ONE, 1.0 - t);

    JointTransform {
        translation,
        rotation,
        scale,
    }
}

/// Allocating version of [`blend_into`], convenient for tools and tests.
pub fn blend_poses(a: &Pose, b: &Pose, t: f32) -> Pose {
    let mut out = Pose::from_locals(Vec::new());
    blend_into(&mut out, a, b, t);
    out
}

/// One sample point of the 1D blend space: `position` is the position on the parameter axis, `clip` is the corresponding clip.
#[derive(Debug, Clone)]
pub struct BlendSample {
    pub position: f32,
    pub clip: Arc<Clip>,
}

/// 1D blend space (velocity 0 → walk → run, and so on).
///
/// The `phase` of `evaluate_into` is **normalized time** (0..1, looping per the clip's own duration):
/// clip lengths in a blend space usually differ, syncing by seconds makes two animations chase each other,
/// normalized phase is the correct approach for "stride alignment".
///
/// Each clip resets the pose to `reference` (generally the skeleton's reference pose) before sampling:
/// when a clip only covers some tracks, uncovered joints keep reference instead of collapsing to
/// identity — consistent with `Animator`'s evaluation order. [`Clip::sample_into`] only writes existing
/// joints, so this step is necessary.
#[derive(Debug, Clone, Default)]
pub struct BlendSpace1D {
    samples: Vec<BlendSample>,
    /// The position axis of `samples` (sorted in sync with `samples`), not recomputed every frame during evaluation.
    positions: Vec<f32>,
    reference: Pose,
    /// When all samples have sync markers, use the first sample as the leading clip for phase mapping.
    sync: Option<SyncGroup>,
    scratch: [Pose; 2],
}

impl BlendSpace1D {
    /// Build a blend space: drop non-finite positions and sort by position (author's fill order does not matter).
    pub fn new(samples: Vec<BlendSample>, reference: Pose) -> Self {
        let mut samples: Vec<BlendSample> = samples
            .into_iter()
            .filter(|sample| sample.position.is_finite())
            .collect();
        samples.sort_by(|a, b| a.position.total_cmp(&b.position));
        let positions = samples.iter().map(|sample| sample.position).collect();
        let sync = samples
            .iter()
            .map(|sample| sample.clip.sync_marker_set())
            .collect::<Option<Vec<_>>>()
            .and_then(SyncGroup::new)
            .filter(|group| group.len() == samples.len());
        Self {
            samples,
            positions,
            reference,
            sync,
            scratch: [Pose::default(), Pose::default()],
        }
    }

    /// The normalized phase actually used for sample `index`.
    ///
    /// When a sync group exists, use the current highest-weight sample (`leader`) as the leading clip, mapping phase
    /// to each sample's own stride; without markers / inconsistent marker structure, all samples share the same phase.
    fn phase_for(&self, leader: usize, index: usize, phase: f32) -> f32 {
        self.sync
            .as_ref()
            .filter(|group| leader < group.len() && index < group.len())
            .map_or(phase, |group| group.map_phase(leader, index, phase))
    }

    /// Pose used for reset before sampling.
    pub fn reference(&self) -> &Pose {
        &self.reference
    }

    pub fn samples(&self) -> &[BlendSample] {
        &self.samples
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Range of the parameter axis; empty blend space returns `None`.
    pub fn range(&self) -> Option<(f32, f32)> {
        let first = self.samples.first()?.position;
        let last = self.samples.last()?.position;
        Some((first, last))
    }

    /// Index of the highest-weight sample at the current parameter (used by Notify's Dominant strategy).
    ///
    /// Takes the right side when exactly at the midpoint between left and right samples; returns it when there is only one sample.
    pub fn dominant_sample(&self, at: f32) -> Option<usize> {
        let (left, right, t) = blend_space_weights(&self.positions, at)?;
        Some(if t < 0.5 { left } else { right })
    }

    /// Evaluate at `at` into `out`. Returns whether a pose was actually written (empty space / sampling failure → `false`).
    ///
    /// On failure `out` is **not touched**: the caller can keep using the previous frame's pose instead of getting an empty skeleton.
    pub fn evaluate_into(&mut self, at: f32, phase: f32, out: &mut Pose) -> bool {
        let Some((left, right, t)) = blend_space_weights(&self.positions, at) else {
            return false;
        };
        let leader = self.dominant_sample(at).unwrap_or(0);

        if left == right {
            self.scratch[0].copy_from(&self.reference);
            let phase = self.phase_for(leader, left, phase);
            if !sample_at_phase(&self.samples[left].clip, phase, &mut self.scratch[0]) {
                return false;
            }
            out.copy_from(&self.scratch[0]);
            return true;
        }

        self.scratch[0].copy_from(&self.reference);
        self.scratch[1].copy_from(&self.reference);
        let left_phase = self.phase_for(leader, left, phase);
        let right_phase = self.phase_for(leader, right, phase);
        if !sample_at_phase(&self.samples[left].clip, left_phase, &mut self.scratch[0]) {
            return false;
        }
        if !sample_at_phase(&self.samples[right].clip, right_phase, &mut self.scratch[1]) {
            return false;
        }
        blend_into(out, &self.scratch[0], &self.scratch[1], t);
        true
    }

    /// Evaluate each sample with **its own phase** (independent playheads, used when samples have different speeds).
    ///
    /// `phases[i]` is the normalized phase of sample `i`; missing samples are sampled at 0. This entry does not do
    /// SyncGroup phase mapping — the caller has already computed its own time for each sample.
    pub fn evaluate_into_with_phases(&mut self, at: f32, phases: &[f32], out: &mut Pose) -> bool {
        let Some((left, right, t)) = blend_space_weights(&self.positions, at) else {
            return false;
        };
        let phase = |index: usize| phases.get(index).copied().unwrap_or(0.0);

        if left == right {
            self.scratch[0].copy_from(&self.reference);
            if !sample_at_phase(&self.samples[left].clip, phase(left), &mut self.scratch[0]) {
                return false;
            }
            out.copy_from(&self.scratch[0]);
            return true;
        }

        self.scratch[0].copy_from(&self.reference);
        self.scratch[1].copy_from(&self.reference);
        if !sample_at_phase(&self.samples[left].clip, phase(left), &mut self.scratch[0]) {
            return false;
        }
        if !sample_at_phase(
            &self.samples[right].clip,
            phase(right),
            &mut self.scratch[1],
        ) {
            return false;
        }
        blend_into(out, &self.scratch[0], &self.scratch[1], t);
        true
    }
}

/// Position interpolation of the 1D blend space: returns `(left sample index, right sample index, t)`.
///
/// Pure function, convenient for unit tests. `positions` must be sorted ascending (guaranteed by [`BlendSpace1D::new`]).
/// Out-of-range clamped to both ends (`t = 0`); with only one sample both left and right point to it,
/// empty table / non-finite query returns `None`.
pub fn blend_space_weights(positions: &[f32], at: f32) -> Option<(usize, usize, f32)> {
    if positions.is_empty() || !at.is_finite() {
        return None;
    }
    let last = positions.len() - 1;
    if at <= positions[0] {
        return Some((0, 0, 0.0));
    }
    if at >= positions[last] {
        return Some((last, last, 0.0));
    }
    // Linear scan: the sample count of a blend space is hand-filled by the author (single digits to a few tens), binary search not worth it.
    for index in 0..last {
        let (low, high) = (positions[index], positions[index + 1]);
        if at >= low && at <= high {
            let span = high - low;
            let t = if span <= f32::EPSILON {
                0.0
            } else {
                (at - low) / span
            };
            return Some((index, index + 1, t));
        }
    }
    // Unsorted / NaN-containing input: clamp to the last sample, instead of returning an out-of-bounds index.
    Some((last, last, 0.0))
}

/// Sample a clip by normalized phase (looping out of range, `phase` non-finite treated as 0).
pub(crate) fn sample_at_phase(clip: &Clip, phase: f32, out: &mut Pose) -> bool {
    let phase = if phase.is_finite() { phase } else { 0.0 };
    let wrapped = phase - phase.floor();
    clip.sample_wrapped_into(clip.duration * wrapped, out)
}

#[cfg(test)]
#[path = "blend_tests.rs"]
mod tests;
