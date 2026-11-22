use super::*;
use crate::skeleton::JointTransform;

#[test]
fn a_pose_is_indexed_and_bounds_checked() {
    let mut pose = Pose::from_locals(vec![JointTransform::IDENTITY; 2]);
    assert_eq!(pose.len(), 2);
    assert!(pose.set_local(1, JointTransform::IDENTITY.translation_only(1.0)));
    assert!(
        !pose.set_local(2, JointTransform::IDENTITY),
        "out-of-bounds write must fail"
    );
    assert!(pose.local(5).is_none());
    assert_eq!(pose.local(1).unwrap().translation.x, 1.0);
}

#[test]
fn identity_resets_every_joint() {
    let mut pose = Pose::from_locals(vec![JointTransform::IDENTITY.translation_only(3.0); 3]);
    pose.identity();
    assert!(pose
        .locals()
        .iter()
        .all(|local| *local == JointTransform::IDENTITY));
}

impl JointTransform {
    /// Convenience constructor with translation only (test use).
    fn translation_only(self, x: f32) -> Self {
        let _ = self;
        Self {
            translation: glam::Vec3::new(x, 0.0, 0.0),
            ..Self::IDENTITY
        }
    }
}
