//! Multi-layer pose stack (the BlendStack foundation layer of #560 P1).
//!
//! A [`BlendStack`] starts from a base pose and stacks several [`BlendLayer`] on top in
//! **addition order**. Each layer can:
//!
//! - be a normal layer (blending the current result toward the layer's pose) or an additive
//!   layer (applying the increment relative to the reference pose, reusing
//!   [`crate::blend::additive_joint`]);
//! - carry a per-bone mask (0 = don't move this bone, 1 = fully apply), for layered cases such
//!   as "only the upper body breathes" and "only the right hand aims";
//! - start only after an `activation_delay` of seconds, then fade in over `blend_in_time` seconds;
//! - limit the number of simultaneously active layers via `max_active_blends` (extra layers
//!   don't participate in evaluation).
//!
//! This layer only does math and does not depend on ECS / rendering / asset formats; the
//! animation graph evaluator, editor preview, and offline tools can all share it. Weight
//! semantics match UE's layered blend: later-added layers continue blending **on top of the
//! current result**, and `weight = 1` fully overrides the previous layer's result on affected
//! bones.
//!
//! Convention consistent with the rest of `prism-anim`: quaternion `xyzw`, right-handed +Y up,
//! pose is per-bone local TRS.

use crate::blend::{additive_joint, blend_joint};
use crate::pose::Pose;
use crate::skeleton::{JointTransform, Skeleton};

/// Blend weight per bone, `0` = completely still, `1` = fully applied.
///
/// An empty mask means "all bones are 1", which is the default value; no all-`1` `Vec` needs
/// to be allocated for full-body layers. When non-empty, bones beyond the mask length are treated
/// as `0`: when skeleton versions mismatch, prefer not moving unknown bones rather than mistakenly
/// treating them as full-body weight.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoneMask {
    weights: Vec<f32>,
}

impl BoneMask {
    /// Full-body mask (equivalent to no mask).
    pub fn all() -> Self {
        Self::default()
    }

    /// Construct from per-bone weights; NaN is treated as `0`, the rest are clamped to `[0, 1]`.
    pub fn from_weights(weights: Vec<f32>) -> Self {
        Self {
            weights: weights
                .into_iter()
                .map(|weight| {
                    if weight.is_nan() {
                        0.0
                    } else {
                        weight.clamp(0.0, 1.0)
                    }
                })
                .collect(),
        }
    }

    /// Construct from bone names: listed bones are 1, the rest are 0; names not found are ignored.
    ///
    /// The result length equals the skeleton's joint count, so index order matches the pose.
    pub fn from_names(skeleton: &Skeleton, names: &[&str]) -> Self {
        let mut weights = vec![0.0; skeleton.joint_count()];
        for name in names {
            if let Some(index) = skeleton.joint_index(name) {
                weights[index] = 1.0;
            }
        }
        Self { weights }
    }

    /// Whether it applies to all bones.
    pub fn is_all(&self) -> bool {
        self.weights.is_empty()
    }

    /// Weight of a given bone. Empty mask = 1; out of range = 0.
    pub fn weight(&self, joint: usize) -> f32 {
        if self.weights.is_empty() {
            1.0
        } else {
            self.weights.get(joint).copied().unwrap_or(0.0)
        }
    }

    /// Per-bone weights (empty = full body).
    pub fn weights(&self) -> &[f32] {
        &self.weights
    }
}

/// A layer stacked on top of [`BlendStack`].
#[derive(Clone, Debug)]
pub struct BlendLayer {
    /// The pose of this layer for the current frame (written in by the caller after sampling from a clip / blend space).
    pose: Pose,
    /// Base weight, `0..=1`; NaN / negative treated as 0.
    weight: f32,
    /// Per-bone mask.
    mask: BoneMask,
    /// `true` = incremental layer relative to the reference pose.
    additive: bool,
    /// Wait at least this many seconds after pushing before taking effect.
    activation_delay: f32,
    /// After taking effect, fade from 0 to `weight` over this many seconds; `0` = immediately full weight.
    blend_in_time: f32,
    /// Elapsed time since pushing (advanced by [`BlendStack::advance`]).
    elapsed: f32,
}

impl BlendLayer {
    /// Normal layer, default full body, weight 1, no delay, takes effect immediately.
    pub fn new(pose: Pose) -> Self {
        Self {
            pose,
            weight: 1.0,
            mask: BoneMask::all(),
            additive: false,
            activation_delay: 0.0,
            blend_in_time: 0.0,
            elapsed: 0.0,
        }
    }

    pub fn with_weight(mut self, weight: f32) -> Self {
        self.weight = weight;
        self
    }

    pub fn with_mask(mut self, mask: BoneMask) -> Self {
        self.mask = mask;
        self
    }

    pub fn with_additive(mut self, additive: bool) -> Self {
        self.additive = additive;
        self
    }

    /// Set delay and fade-in duration (negative values treated as 0).
    pub fn with_timing(mut self, activation_delay: f32, blend_in_time: f32) -> Self {
        self.activation_delay = sanitize_seconds(activation_delay);
        self.blend_in_time = sanitize_seconds(blend_in_time);
        self
    }

    pub fn pose(&self) -> &Pose {
        &self.pose
    }

    pub fn pose_mut(&mut self) -> &mut Pose {
        &mut self.pose
    }

    pub fn weight(&self) -> f32 {
        self.weight
    }

    pub fn set_weight(&mut self, weight: f32) {
        self.weight = weight;
    }

    pub fn mask(&self) -> &BoneMask {
        &self.mask
    }

    pub fn is_additive(&self) -> bool {
        self.additive
    }

    pub fn elapsed(&self) -> f32 {
        self.elapsed
    }

    /// Reset timing from scratch (e.g. when a layer is reactivated).
    pub fn reset_elapsed(&mut self) {
        self.elapsed = 0.0;
    }

    /// Current effective weight: base weight × delay/fade-in factor × mask (multiplied at evaluation time).
    fn effective_weight(&self) -> f32 {
        let weight = sanitize_weight(self.weight);
        if weight <= 0.0 || self.elapsed < self.activation_delay {
            return 0.0;
        }
        if self.blend_in_time <= 0.0 {
            return weight;
        }
        let t = ((self.elapsed - self.activation_delay) / self.blend_in_time).clamp(0.0, 1.0);
        weight * t
    }
}

/// Multi-layer pose stack.
#[derive(Clone, Debug)]
pub struct BlendStack {
    base: Pose,
    /// Reference (bind) pose used by additive layers.
    reference: Pose,
    layers: Vec<BlendLayer>,
    /// Upper limit of normal + additive layers participating in evaluation at the same time;
    /// `0` = unlimited.
    max_active_blends: usize,
}

impl BlendStack {
    /// Build a stack with a base pose and an additive reference pose.
    pub fn new(base: Pose, reference: Pose) -> Self {
        Self {
            base,
            reference,
            layers: Vec::new(),
            max_active_blends: 0,
        }
    }

    pub fn base(&self) -> &Pose {
        &self.base
    }

    pub fn base_mut(&mut self) -> &mut Pose {
        &mut self.base
    }

    pub fn reference(&self) -> &Pose {
        &self.reference
    }

    pub fn set_reference(&mut self, reference: Pose) {
        self.reference = reference;
    }

    pub fn layers(&self) -> &[BlendLayer] {
        &self.layers
    }

    pub fn layers_mut(&mut self) -> &mut Vec<BlendLayer> {
        &mut self.layers
    }

    /// Push a layer, returning its index.
    pub fn push(&mut self, layer: BlendLayer) -> usize {
        let index = self.layers.len();
        self.layers.push(layer);
        index
    }

    /// Remove a layer; returns `None` if out of bounds.
    pub fn remove(&mut self, index: usize) -> Option<BlendLayer> {
        (index < self.layers.len()).then(|| self.layers.remove(index))
    }

    pub fn clear(&mut self) {
        self.layers.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// Limit the number of simultaneously active layers; `0` means unlimited. Extra layers
    /// do not participate in **addition order**.
    pub fn set_max_active_blends(&mut self, max_active: usize) {
        self.max_active_blends = max_active;
    }

    pub fn max_active_blends(&self) -> usize {
        self.max_active_blends
    }

    /// Advance each layer's timing (`dt` non-finite or negative treated as 0).
    pub fn advance(&mut self, dt: f32) {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        for layer in &mut self.layers {
            layer.elapsed = (layer.elapsed + dt).min(86_400.0);
        }
    }

    /// Evaluate into `out`: starting from base, stack all active layers in order.
    ///
    /// On failure (base is empty) `out` is left untouched. The caller can reuse the same `out`
    /// every frame to avoid allocating a new pose per character.
    pub fn evaluate_into(&self, out: &mut Pose) {
        if self.base.is_empty() {
            return;
        }
        out.copy_from(&self.base);
        let joint_count = out.len();
        let mut active = 0usize;

        for layer in &self.layers {
            let weight = layer.effective_weight();
            if weight <= 0.0 {
                continue;
            }
            if self.max_active_blends > 0 && active >= self.max_active_blends {
                break;
            }
            active += 1;

            let shared = joint_count.min(layer.pose.len());
            for joint in 0..shared {
                let mask = layer.mask.weight(joint);
                let weight = weight * mask;
                if weight <= 0.0 {
                    continue;
                }
                let (Some(current), Some(target)) = (out.local(joint), layer.pose.local(joint))
                else {
                    continue;
                };
                if layer.additive {
                    let reference = self
                        .reference
                        .local(joint)
                        .unwrap_or(JointTransform::IDENTITY);
                    out.set_local(joint, additive_joint(&current, &target, &reference, weight));
                } else {
                    out.set_local(joint, blend_joint(&current, &target, weight));
                }
            }
        }
    }
}

fn sanitize_weight(weight: f32) -> f32 {
    if weight.is_finite() {
        weight.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn sanitize_seconds(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

#[cfg(test)]
#[path = "blend_stack_tests.rs"]
mod tests;
