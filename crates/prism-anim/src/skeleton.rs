//! Skeleton: hierarchy, reference pose, and `bone_palette`.

use glam::{Mat4, Quat, Vec3};

use crate::pose::Pose;

/// A bone's local transform (one-to-one with the cooked format's 10 f32s).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JointTransform {
    pub translation: Vec3,
    /// Quaternion, `xyzw` layout (`glam::Quat`).
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for JointTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl JointTransform {
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    pub fn new(translation: Vec3, rotation: Quat, scale: Vec3) -> Self {
        Self {
            translation,
            rotation: rotation.normalize(),
            scale,
        }
    }

    /// Unpack from cooked layout (`t.xyz, r.xyzw, s.xyz`).
    pub fn from_array(values: [f32; 10]) -> Self {
        let rotation = Quat::from_xyzw(values[3], values[4], values[5], values[6]);
        Self {
            translation: Vec3::new(values[0], values[1], values[2]),
            rotation: normalize_quat(rotation),
            scale: Vec3::new(values[7], values[8], values[9]),
        }
    }

    /// Pack back to cooked layout.
    pub fn to_array(self) -> [f32; 10] {
        [
            self.translation.x,
            self.translation.y,
            self.translation.z,
            self.rotation.x,
            self.rotation.y,
            self.rotation.z,
            self.rotation.w,
            self.scale.x,
            self.scale.y,
            self.scale.z,
        ]
    }

    /// Local matrix `T * R * S`.
    pub fn matrix(self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }

    /// Linear interpolation (rotation uses slerp). `t = 0` returns `self`.
    pub fn lerp(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        Self {
            translation: self.translation.lerp(other.translation, t),
            rotation: self.rotation.slerp(other.rotation, t),
            scale: self.scale.lerp(other.scale, t),
        }
    }

    /// Decompose from matrix (scale takes column length; with negative scale, flip onto the X axis
    /// by the smallest component).
    pub fn from_matrix(matrix: Mat4) -> Self {
        let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
        Self {
            translation,
            rotation: normalize_quat(rotation),
            scale,
        }
    }
}

impl From<prism_asset::formats_skinned::JointTransform> for JointTransform {
    fn from(value: prism_asset::formats_skinned::JointTransform) -> Self {
        Self::from_array(value.to_array())
    }
}

fn normalize_quat(rotation: Quat) -> Quat {
    if rotation.length_squared() > 1e-12 {
        rotation.normalize()
    } else {
        Quat::IDENTITY
    }
}

/// A bone.
#[derive(Debug, Clone, PartialEq)]
pub struct Joint {
    pub name: String,
    /// Parent bone index (cooked data guarantees parent comes before child).
    pub parent: Option<usize>,
    /// Parent-space transform in the reference (bind) pose.
    pub reference_local: JointTransform,
    /// Skinning matrix: transforms vertices from reference pose space to bone space.
    pub inverse_bind: Mat4,
}

/// Skeleton hierarchy.
#[derive(Debug, Clone, PartialEq)]
pub struct Skeleton {
    joints: Vec<Joint>,
}

impl Skeleton {
    /// Build from cooked `SkeletonInfo`, also validating topological order (parent index must be less than self).
    pub fn from_info(info: &prism_asset::formats_skinned::SkeletonInfo) -> Option<Self> {
        if info.joints.is_empty() || !info.is_topologically_sorted() {
            return None;
        }
        let joints = info
            .joints
            .iter()
            .enumerate()
            .map(|(index, joint)| {
                let parent = joint.parent.map(usize::from).filter(|p| *p < index);
                Joint {
                    name: joint.name.clone(),
                    parent,
                    reference_local: JointTransform::from(joint.reference_local),
                    inverse_bind: Mat4::from_cols_array(&joint.inverse_bind),
                }
            })
            .collect();
        Some(Self { joints })
    }

    /// Construct directly from a joint list (for tests and offline tools).
    pub fn new(joints: Vec<Joint>) -> Option<Self> {
        let mut valid = !joints.is_empty();
        for (index, joint) in joints.iter().enumerate() {
            if joint.parent.is_some_and(|parent| parent >= index) {
                valid = false;
            }
        }
        valid.then_some(Self { joints })
    }

    pub fn joints(&self) -> &[Joint] {
        &self.joints
    }

    pub fn joint_count(&self) -> usize {
        self.joints.len()
    }

    pub fn joint_index(&self, name: &str) -> Option<usize> {
        self.joints.iter().position(|joint| joint.name == name)
    }

    /// Reference pose (each bone's `reference_local`).
    pub fn rest_pose(&self) -> Pose {
        Pose::from_locals(
            self.joints
                .iter()
                .map(|joint| joint.reference_local)
                .collect(),
        )
    }

    /// World matrices of bones in a pose (parent-space product, parent guaranteed before child).
    pub fn world_matrices(&self, pose: &Pose) -> Vec<Mat4> {
        let mut world = Vec::with_capacity(self.joints.len());
        for (index, joint) in self.joints.iter().enumerate() {
            let local = pose
                .local(index)
                .map(JointTransform::matrix)
                .unwrap_or(Mat4::IDENTITY);
            let matrix = match joint.parent {
                Some(parent) => world[parent] * local,
                None => local,
            };
            world.push(matrix);
        }
        world
    }

    /// `bone_palette = world(pose) * inverse_bind` (GPU uploads this directly).
    pub fn bone_palette(&self, pose: &Pose) -> Vec<Mat4> {
        self.world_matrices(pose)
            .into_iter()
            .zip(&self.joints)
            .map(|(world, joint)| world * joint.inverse_bind)
            .collect()
    }
}

#[cfg(test)]
#[path = "skeleton_tests.rs"]
pub(crate) mod tests;
