//! Pose: local TRS of each bone.

use crate::skeleton::JointTransform;

/// A set of bone local transforms. Length matches the skeleton; out-of-range access returns `None`.
///
/// `Default` is the **empty pose** (zero root bones), not an identity pose — it's only used as
/// an initial value for containers like [`BlendSpace1D`](crate::blend::BlendSpace1D); the caller
/// must sample first before reading.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pose {
    locals: Vec<JointTransform>,
}

impl Pose {
    pub fn from_locals(locals: Vec<JointTransform>) -> Self {
        Self { locals }
    }

    pub fn len(&self) -> usize {
        self.locals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.locals.is_empty()
    }

    pub fn local(&self, index: usize) -> Option<JointTransform> {
        self.locals.get(index).copied()
    }

    pub fn set_local(&mut self, index: usize, transform: JointTransform) -> bool {
        match self.locals.get_mut(index) {
            Some(slot) => {
                *slot = transform;
                true
            }
            None => false,
        }
    }

    pub fn locals(&self) -> &[JointTransform] {
        &self.locals
    }

    /// Reset the pose to `count` joints, each initialized by `fill`.
    ///
    /// No reallocation when capacity suffices: per-frame paths like blending rely on this to
    /// avoid per-frame `Vec` allocation.
    pub fn resize_with(&mut self, count: usize, mut fill: impl FnMut(usize) -> JointTransform) {
        self.locals.clear();
        self.locals.reserve(count);
        for index in 0..count {
            self.locals.push(fill(index));
        }
    }

    /// Overwrite yourself with another pose (keep allocated capacity, no reallocation).
    ///
    /// Each blend space evaluation first resets the scratch pose to the reference pose; per-frame
    /// `clone()` would throw away memory for nothing.
    pub fn copy_from(&mut self, other: &Pose) {
        self.locals.clear();
        self.locals.extend_from_slice(&other.locals);
    }

    /// Reset to identity transform (all bones back to origin, `scale = 1`).
    pub fn identity(&mut self) {
        self.locals.fill(JointTransform::IDENTITY);
    }
}

#[cfg(test)]
#[path = "pose_tests.rs"]
mod tests;
