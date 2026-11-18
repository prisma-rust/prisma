//! Foot locking to suppress foot-sliding across motion-matching cuts (#560 P3).

use glam::Vec3;
use prism_anim::pose::Pose;
use prism_anim::skeleton::Skeleton;

#[derive(Debug, Clone, PartialEq)]
pub struct FootLockSettings {
    pub joints: Vec<usize>,
    pub plant_speed: f32,
    pub release_speed: f32,
    pub max_offset: f32,
    pub weight: f32,
}

impl Default for FootLockSettings {
    fn default() -> Self {
        Self {
            joints: Vec::new(),
            plant_speed: 0.35,
            release_speed: 0.9,
            max_offset: 0.25,
            weight: 1.0,
        }
    }
}

impl FootLockSettings {
    pub fn tracking_all(skeleton: &Skeleton) -> Self {
        Self {
            joints: (0..skeleton.joints().len()).collect(),
            ..Self::default()
        }
    }

    fn sanitized(&self) -> (f32, f32, f32, f32) {
        let positive = |value: f32, fallback: f32| {
            if value.is_finite() && value > 0.0 {
                value
            } else {
                fallback
            }
        };
        let plant = positive(self.plant_speed, 0.35);
        let release = positive(self.release_speed, 0.9).max(plant);
        let max_offset = positive(self.max_offset, 0.25);
        let weight = if self.weight.is_finite() {
            self.weight.clamp(0.0, 1.0)
        } else {
            0.0
        };
        (plant, release, max_offset, weight)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct FootState {
    last: Option<Vec3>,
    planted: Option<Vec3>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FootLockReport {
    pub locked: usize,
    pub planted: usize,
    pub max_correction: f32,
}

impl FootLockReport {
    fn empty() -> Self {
        Self {
            locked: 0,
            planted: 0,
            max_correction: 0.0,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FootLock {
    feet: Vec<FootState>,
}

impl FootLock {
    pub fn new(feet: usize) -> Self {
        Self {
            feet: vec![FootState::default(); feet],
        }
    }

    pub fn reset(&mut self) {
        for foot in &mut self.feet {
            *foot = FootState::default();
        }
    }

    pub fn feet(&self) -> usize {
        self.feet.len()
    }

    pub fn update(
        &mut self,
        skeleton: &Skeleton,
        pose: &mut Pose,
        settings: &FootLockSettings,
        dt: f32,
    ) -> FootLockReport {
        let (plant_speed, release_speed, max_offset, weight) = settings.sanitized();
        if settings.joints.len() != self.feet.len() {
            self.feet = vec![FootState::default(); settings.joints.len()];
        }
        if settings.joints.is_empty() || weight <= 0.0 {
            return FootLockReport::empty();
        }
        let joints = skeleton.joints();
        let world = skeleton.world_matrices(pose);
        let usable = |joint: usize| joint < joints.len() && joint < world.len();
        let valid_dt = dt.is_finite() && dt > 0.0;

        let mut report = FootLockReport::empty();
        for (slot, joint) in settings.joints.iter().copied().enumerate() {
            if !usable(joint) {
                continue;
            }
            let current = world[joint].w_axis.truncate();
            let foot = &mut self.feet[slot];

            // Compute velocity from the un-locked position so locking does not cancel itself.
            let speed = match (foot.last, valid_dt) {
                (Some(last), true) => (current - last).length() / dt,
                _ => 0.0,
            };

            if let Some(anchor) = foot.planted {
                if valid_dt && speed > release_speed {
                    foot.planted = None;
                } else {
                    let delta = anchor - current;
                    let correction = delta.clamp_length_max(max_offset) * weight;
                    if correction.length_squared() > 1e-12 {
                        if let (Some(local), Some(parent_world)) =
                            (pose.local(joint), parent_rotation(&world, joints, joint))
                        {
                            let local_delta = parent_world.inverse() * correction;
                            pose.set_local(
                                joint,
                                prism_anim::skeleton::JointTransform {
                                    translation: local.translation + local_delta,
                                    ..local
                                },
                            );
                        }
                        report.locked += 1;
                        report.max_correction = report.max_correction.max(correction.length());
                    }
                }
            }

            if foot.planted.is_none() && valid_dt && speed <= plant_speed {
                foot.planted = Some(current);
            }

            foot.last = Some(match foot.planted {
                Some(anchor) if report.locked > 0 => anchor,
                _ => current,
            });
            if foot.planted.is_some() {
                report.planted += 1;
            }
        }
        report
    }
}

fn parent_rotation(
    world: &[glam::Mat4],
    joints: &[prism_anim::skeleton::Joint],
    joint: usize,
) -> Option<glam::Quat> {
    let parent = joints.get(joint)?.parent;
    match parent {
        Some(parent) if parent < world.len() => {
            Some(world[parent].to_scale_rotation_translation().1)
        }
        _ => Some(glam::Quat::IDENTITY),
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;
    use prism_anim::pose::Pose;
    use prism_anim::skeleton::{Joint, JointTransform, Skeleton};

    use super::{FootLock, FootLockSettings};

    fn skeleton() -> Skeleton {
        Skeleton::new(vec![
            Joint {
                name: "root".into(),
                parent: None,
                reference_local: JointTransform {
                    translation: Vec3::ZERO,
                    rotation: glam::Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
                inverse_bind: glam::Mat4::IDENTITY,
            },
            Joint {
                name: "thigh".into(),
                parent: Some(0),
                reference_local: JointTransform {
                    translation: Vec3::new(0.0, -0.5, 0.0),
                    rotation: glam::Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
                inverse_bind: glam::Mat4::IDENTITY,
            },
            Joint {
                name: "foot".into(),
                parent: Some(1),
                reference_local: JointTransform {
                    translation: Vec3::new(0.0, -0.5, 0.0),
                    rotation: glam::Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
                inverse_bind: glam::Mat4::IDENTITY,
            },
        ])
        .expect("valid skeleton")
    }

    fn foot_world(skeleton: &Skeleton, pose: &Pose) -> Vec3 {
        skeleton.world_matrices(pose)[2].w_axis.truncate()
    }

    fn settings() -> FootLockSettings {
        FootLockSettings {
            joints: vec![2],
            ..FootLockSettings::default()
        }
    }

    #[test]
    fn a_planted_foot_is_held_through_a_small_shift() {
        let skeleton = skeleton();
        let mut pose = skeleton.rest_pose();
        let mut lock = FootLock::new(1);
        let settings = settings();

        lock.update(&skeleton, &mut pose, &settings, 1.0 / 60.0);
        let anchor = foot_world(&skeleton, &pose);

        let thigh = pose.local(1).unwrap();
        pose.set_local(
            1,
            JointTransform {
                translation: thigh.translation + Vec3::new(0.0, 0.0, -0.01),
                ..thigh
            },
        );
        let report = lock.update(&skeleton, &mut pose, &settings, 1.0 / 60.0);
        assert_eq!(report.locked, 1);
        let held = foot_world(&skeleton, &pose);
        assert!((held - anchor).length() < 1e-4, "{held:?} vs {anchor:?}");
    }

    #[test]
    fn a_stepping_foot_is_released() {
        let skeleton = skeleton();
        let mut pose = skeleton.rest_pose();
        let mut lock = FootLock::new(1);
        let settings = settings();
        lock.update(&skeleton, &mut pose, &settings, 1.0 / 60.0);

        let thigh = pose.local(1).unwrap();
        pose.set_local(
            1,
            JointTransform {
                translation: thigh.translation + Vec3::new(0.0, 0.0, -0.1),
                ..thigh
            },
        );
        let report = lock.update(&skeleton, &mut pose, &settings, 1.0 / 60.0);
        assert_eq!(report.locked, 0);
        assert_eq!(report.planted, 0);
    }

    #[test]
    fn the_correction_is_clamped() {
        let skeleton = skeleton();
        let mut pose = skeleton.rest_pose();
        let mut lock = FootLock::new(1);
        let settings = FootLockSettings {
            joints: vec![2],
            max_offset: 0.01,
            release_speed: 100.0,
            ..FootLockSettings::default()
        };
        lock.update(&skeleton, &mut pose, &settings, 1.0 / 60.0);
        let anchor = foot_world(&skeleton, &pose);
        let thigh = pose.local(1).unwrap();
        pose.set_local(
            1,
            JointTransform {
                translation: thigh.translation + Vec3::new(0.0, 0.0, -0.05),
                ..thigh
            },
        );
        let report = lock.update(&skeleton, &mut pose, &settings, 1.0 / 60.0);
        assert_eq!(report.locked, 1);
        assert!(report.max_correction <= 0.01 + 1e-6, "{report:?}");
        let held = foot_world(&skeleton, &pose);
        assert!((held - anchor).length() > 0.03, "partially recovered");
    }

    #[test]
    fn weight_zero_and_bad_dt_leave_the_pose_alone() {
        let skeleton = skeleton();
        let mut pose = skeleton.rest_pose();
        let before = pose.local(2).unwrap();
        let mut lock = FootLock::new(1);
        let report = lock.update(
            &skeleton,
            &mut pose,
            &FootLockSettings {
                joints: vec![2],
                weight: 0.0,
                ..FootLockSettings::default()
            },
            1.0 / 60.0,
        );
        assert_eq!(report.locked, 0);
        assert_eq!(pose.local(2).unwrap(), before);

        let report = lock.update(&skeleton, &mut pose, &settings(), 0.0);
        assert_eq!(report.planted, 0);
        assert_eq!(pose.local(2).unwrap(), before);
    }

    #[test]
    fn the_settings_are_sanitized_and_out_of_range_joints_are_skipped() {
        let skeleton = skeleton();
        let mut pose = skeleton.rest_pose();
        let mut lock = FootLock::new(2);
        let settings = FootLockSettings {
            joints: vec![2, 99],
            release_speed: f32::NAN,
            plant_speed: -1.0,
            max_offset: f32::INFINITY,
            weight: f32::INFINITY,
        };
        let report = lock.update(&skeleton, &mut pose, &settings, 1.0 / 60.0);
        assert_eq!(report.planted, 0);
        assert_eq!(lock.feet(), 2);

        let sane = FootLockSettings {
            joints: vec![2, 99],
            ..FootLockSettings::default()
        };
        let report = lock.update(&skeleton, &mut pose, &sane, 1.0 / 60.0);
        assert_eq!(report.planted, 1);

        lock.reset();
        assert_eq!(lock.feet(), 2);
        let report = lock.update(&skeleton, &mut pose, &sane, 1.0 / 60.0);
        assert_eq!(report.planted, 1, "reset re-plants on the next frame");
    }
}
