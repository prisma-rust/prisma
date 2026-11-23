//! Viewport skeleton overlay (#560 P0-4 increment).
//!
//! Skinned mesh in Scene view only has one deformed mesh, cannot see what skeleton looks like, which bone moves.
//! Restore selected entity's `BonePalette` to world-space joint positions, draw as lines + dots:
//!
//! - `palette[j] = world_j * inverse_bind_j` (see `prism_anim::Skeleton::bone_palette`),
//!   so `world_j = palette[j] * inverse_bind_j⁻¹`;
//! - Lines drawn between parent -> child, parent bone index in cooked data always smaller than itself, no sort needed.
//!
//! Pure math in [`bone_overlay`], viewport only responsible for projection and drawing.

/// Skeleton in world space: joint positions + parent-child lines.
#[derive(Debug, Clone, PartialEq)]
pub struct SkeletonOverlay {
    /// World position of each joint, order consistent with skeleton `joints`.
    pub joints: Vec<[f32; 3]>,
    /// `(parent index, child index)`, used for drawing lines.
    pub segments: Vec<(usize, usize)>,
}

impl SkeletonOverlay {
    pub fn is_empty(&self) -> bool {
        self.joints.is_empty()
    }
}

/// Restore world-space skeleton from palette + parent table + inverse bind matrix.
///
/// When three inputs length inconsistent, take shortest (`palette` may be truncated by `BONE_CAPACITY`),
/// Non-finite results skipped --- drawing a NaN joint would send whole frame lines flying.
pub fn bone_overlay(
    palette: &[[[f32; 4]; 4]],
    parents: &[Option<usize>],
    inverse_binds: &[[[f32; 4]; 4]],
    local_to_world: &[[f32; 4]; 4],
) -> SkeletonOverlay {
    let count = palette.len().min(parents.len()).min(inverse_binds.len());
    let entity_world = glam::Mat4::from_cols_array_2d(local_to_world);
    let mut joints = Vec::with_capacity(count);
    for index in 0..count {
        let bind = glam::Mat4::from_cols_array_2d(&inverse_binds[index]).inverse();
        let world = entity_world * glam::Mat4::from_cols_array_2d(&palette[index]) * bind;
        let position = world.w_axis.truncate();
        if !position.is_finite() {
            joints.push([f32::NAN; 3]);
            continue;
        }
        joints.push([position.x, position.y, position.z]);
    }

    let mut segments = Vec::with_capacity(count);
    for (index, parent) in parents.iter().enumerate().take(count) {
        let Some(parent) = *parent else {
            continue;
        };
        if parent >= count || parent >= index {
            continue;
        }
        if !joints[index].iter().all(|value| value.is_finite())
            || !joints[parent].iter().all(|value| value.is_finite())
        {
            continue;
        }
        segments.push((parent, index));
    }

    SkeletonOverlay { joints, segments }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity_palette(count: usize) -> Vec<[[f32; 4]; 4]> {
        vec![glam::Mat4::IDENTITY.to_cols_array_2d(); count]
    }

    fn translated_bind(x: f32) -> [[f32; 4]; 4] {
        glam::Mat4::from_translation(glam::vec3(x, 0.0, 0.0))
            .inverse()
            .to_cols_array_2d()
    }

    #[test]
    fn a_rest_pose_palette_puts_every_joint_on_its_bind_position() {
        let identity = glam::Mat4::IDENTITY.to_cols_array_2d();
        let overlay = bone_overlay(
            &identity_palette(2),
            &[None, Some(0)],
            &[translated_bind(0.0), translated_bind(1.0)],
            &identity,
        );
        assert_eq!(overlay.joints.len(), 2);
        assert!((overlay.joints[0][0]).abs() < 1e-5);
        assert!(
            (overlay.joints[1][0] - 1.0).abs() < 1e-5,
            "{:?}",
            overlay.joints
        );
        assert_eq!(overlay.segments, vec![(0, 1)]);
    }

    #[test]
    fn a_translated_palette_moves_the_joint_with_the_animation() {
        let identity = glam::Mat4::IDENTITY.to_cols_array_2d();
        let mut palette = identity_palette(2);
        palette[1] = glam::Mat4::from_translation(glam::vec3(0.0, 2.0, 0.0)).to_cols_array_2d();
        let overlay = bone_overlay(
            &palette,
            &[None, Some(0)],
            &[translated_bind(0.0), translated_bind(1.0)],
            &identity,
        );
        assert!(
            (overlay.joints[1][1] - 2.0).abs() < 1e-5,
            "{:?}",
            overlay.joints
        );
    }

    #[test]
    fn mismatched_inputs_and_broken_matrices_are_dropped_instead_of_panicking() {
        let identity = glam::Mat4::IDENTITY.to_cols_array_2d();
        let overlay = bone_overlay(
            &identity_palette(1),
            &[None, Some(0)],
            &[translated_bind(0.0), translated_bind(1.0)],
            &identity,
        );
        assert_eq!(overlay.joints.len(), 1);
        assert!(overlay.segments.is_empty());

        let mut palette = identity_palette(2);
        palette[1][3][1] = f32::NAN;
        let overlay = bone_overlay(
            &palette,
            &[None, Some(0)],
            &[translated_bind(0.0), translated_bind(1.0)],
            &identity,
        );
        assert!(overlay.segments.is_empty());
    }

    #[test]
    fn a_parent_that_points_at_itself_or_forward_is_ignored() {
        let identity = glam::Mat4::IDENTITY.to_cols_array_2d();
        let overlay = bone_overlay(
            &identity_palette(3),
            &[None, Some(1), Some(3)],
            &[
                translated_bind(0.0),
                translated_bind(1.0),
                translated_bind(2.0),
            ],
            &identity,
        );
        assert!(
            overlay.segments.is_empty(),
            "self or forward parent must not emit segments"
        );
    }

    #[test]
    fn bone_overlay_transforms_joints_by_entity_local_to_world() {
        let entity_world =
            glam::Mat4::from_translation(glam::vec3(10.0, 5.0, -2.0)).to_cols_array_2d();
        let overlay = bone_overlay(
            &identity_palette(2),
            &[None, Some(0)],
            &[translated_bind(0.0), translated_bind(1.0)],
            &entity_world,
        );
        assert!((overlay.joints[0][0] - 10.0).abs() < 1e-5);
        assert!((overlay.joints[0][1] - 5.0).abs() < 1e-5);
        assert!((overlay.joints[0][2] + 2.0).abs() < 1e-5);
        assert!((overlay.joints[1][0] - 11.0).abs() < 1e-5);
        assert!((overlay.joints[1][1] - 5.0).abs() < 1e-5);
        assert!((overlay.joints[1][2] + 2.0).abs() < 1e-5);
    }
}
