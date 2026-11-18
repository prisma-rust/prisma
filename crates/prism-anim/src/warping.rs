//! Procedural orientation, stride, and root-motion warping (#560).

use glam::{Quat, Vec3};

use crate::Pose;

/// Distribute a yaw offset (radians around `+Y`) across `spine_joints`,
/// optionally counter-rotating `root_joint` to keep the pelvis aligned.
pub fn apply_orientation_warping(
    pose: &mut Pose,
    spine_joints: &[usize],
    root_joint: Option<usize>,
    yaw_angle_rad: f32,
    weight: f32,
) {
    let w = weight.clamp(0.0, 1.0);
    if w <= 0.0 || !yaw_angle_rad.is_finite() || spine_joints.is_empty() {
        return;
    }

    let total_yaw = yaw_angle_rad * w;
    if let Some(root) = root_joint {
        if let Some(mut jt) = pose.local(root) {
            let rot = Quat::from_rotation_y(-total_yaw);
            jt.rotation = (rot * jt.rotation).normalize();
            pose.set_local(root, jt);
        }
    }

    let valid_count = spine_joints.iter().filter(|&&idx| idx < pose.len()).count();
    if valid_count == 0 {
        return;
    }
    let per_joint_yaw = total_yaw / (valid_count as f32);
    let step_rot = Quat::from_rotation_y(per_joint_yaw);
    for &idx in spine_joints {
        if let Some(mut jt) = pose.local(idx) {
            jt.rotation = (step_rot * jt.rotation).normalize();
            pose.set_local(idx, jt);
        }
    }
}

/// Scale horizontal (`X`, `Z`) local translations of `stride_joints` to match
/// a target stride ratio while preserving vertical (`+Y`) motion.
pub fn apply_stride_warping(pose: &mut Pose, stride_joints: &[usize], stride_scale: f32) {
    if !stride_scale.is_finite() || stride_scale <= 0.0 {
        return;
    }
    for &idx in stride_joints {
        if let Some(mut jt) = pose.local(idx) {
            jt.translation.x *= stride_scale;
            jt.translation.z *= stride_scale;
            pose.set_local(idx, jt);
        }
    }
}

/// Compute a single warped root-motion translation step so that `current_pos`
/// reaches `target_pos` smoothly by the end of a warp window.
///
/// `remaining_fraction` is in `(0.0, 1.0]` (remaining portion of the warp window),
/// and `step_fraction` is the fraction consumed by this tick (`<= remaining_fraction`).
pub fn warp_root_motion_step(
    current_pos: Vec3,
    target_pos: Vec3,
    anim_step_delta: Vec3,
    remaining_fraction: f32,
    step_fraction: f32,
) -> Vec3 {
    if !current_pos.is_finite()
        || !target_pos.is_finite()
        || !anim_step_delta.is_finite()
        || !remaining_fraction.is_finite()
        || !step_fraction.is_finite()
        || remaining_fraction <= 1.0e-5
        || step_fraction <= 0.0
    {
        return anim_step_delta;
    }

    let alpha = (step_fraction / remaining_fraction).clamp(0.0, 1.0);
    (target_pos - current_pos) * alpha
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::JointTransform;

    #[test]
    fn orientation_warping_preserves_unit_quaternions() {
        let mut pose = Pose::from_locals(vec![JointTransform::IDENTITY; 4]);
        apply_orientation_warping(&mut pose, &[1, 2, 3], Some(0), 0.75, 1.0);
        for jt in pose.locals() {
            assert!(jt.rotation.is_normalized());
        }
        let (_axis, angle) = pose.locals()[0].rotation.to_axis_angle();
        assert!((angle - 0.75).abs() < 1.0e-4);
    }

    #[test]
    fn stride_warping_scales_xz_only() {
        let mut pose = Pose::from_locals(vec![JointTransform {
            translation: Vec3::new(1.0, 2.0, -3.0),
            ..JointTransform::IDENTITY
        }]);
        apply_stride_warping(&mut pose, &[0], 1.5);
        assert_eq!(pose.locals()[0].translation, Vec3::new(1.5, 2.0, -4.5));
    }

    #[test]
    fn warp_root_motion_step_hits_target_across_uniform_steps() {
        let mut pos = Vec3::new(0.0, 0.0, 0.0);
        let target = Vec3::new(2.0, 1.0, -4.0);
        let steps = 4;
        for i in 0..steps {
            let rem = (steps - i) as f32 / steps as f32;
            let step = 1.0 / steps as f32;
            pos += warp_root_motion_step(pos, target, Vec3::ZERO, rem, step);
        }
        assert!((pos - target).length() < 1.0e-4);
    }
}
