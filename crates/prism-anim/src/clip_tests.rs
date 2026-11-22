use super::*;
use glam::Vec3;

fn v3(time: f32, value: Vec3) -> Vec3Key {
    Vec3Key { time, value }
}

fn q(time: f32, value: Quat) -> QuatKey {
    QuatKey { time, value }
}

#[test]
fn sampling_interpolates_between_keys_and_holds_the_ends() {
    let keys = [v3(0.0, Vec3::ZERO), v3(1.0, Vec3::new(10.0, 0.0, 0.0))];
    assert_eq!(sample_vec3(&keys, -5.0), Some(Vec3::ZERO));
    let middle = sample_vec3(&keys, 0.5).unwrap();
    assert!((middle.x - 5.0).abs() < 1e-6, "got {middle:?}");
    assert_eq!(sample_vec3(&keys, 9.0), Some(Vec3::new(10.0, 0.0, 0.0)));
    assert!(sample_vec3(&[], 0.0).is_none());
}

#[test]
fn a_single_key_is_constant() {
    let keys = [v3(0.5, Vec3::new(1.0, 2.0, 3.0))];
    assert_eq!(sample_vec3(&keys, 0.0), Some(Vec3::new(1.0, 2.0, 3.0)));
    assert_eq!(sample_vec3(&keys, 99.0), Some(Vec3::new(1.0, 2.0, 3.0)));
}

#[test]
fn three_keys_pick_the_right_segment() {
    let keys = [
        v3(0.0, Vec3::ZERO),
        v3(1.0, Vec3::new(2.0, 0.0, 0.0)),
        v3(2.0, Vec3::new(0.0, 4.0, 0.0)),
    ];
    // 1.5 falls in the middle of the second segment ((2,0,0) → (0,4,0)).
    let mid = sample_vec3(&keys, 1.5).unwrap();
    assert!((mid.x - 1.0).abs() < 1e-6, "got {mid:?}");
    assert!((mid.y - 2.0).abs() < 1e-6, "got {mid:?}");
    // First segment.
    let first = sample_vec3(&keys, 0.25).unwrap();
    assert!((first.x - 0.5).abs() < 1e-6, "got {first:?}");
}

#[test]
fn rotations_slerp_the_short_way() {
    let keys = [
        q(0.0, Quat::IDENTITY),
        q(1.0, Quat::from_rotation_y(std::f32::consts::PI)),
    ];
    let half = sample_quat(&keys, 0.5).unwrap();
    let rotated = half * Vec3::X;
    assert!(
        rotated.z.abs() > 0.99,
        "half of 180° is 90°, got {rotated:?}"
    );
    // Endpoints hold.
    assert!(sample_quat(&keys, -1.0)
        .unwrap()
        .abs_diff_eq(Quat::IDENTITY, 1e-6));
}

#[test]
fn looping_times_wrap_and_once_clips_clamp() {
    let mut clip = Clip {
        name: "walk".into(),
        duration: 2.0,
        fps: 30.0,
        looped: true,
        additive: false,
        tracks: Vec::new(),
        ..Default::default()
    };
    assert!((clip.wrap_time(2.5) - 0.5).abs() < 1e-6);
    assert!((clip.wrap_time(-0.5) - 1.5).abs() < 1e-6, "negative time also wraps around");
    assert_eq!(clip.wrap_time(f32::NAN), 0.0);

    clip.looped = false;
    assert_eq!(clip.wrap_time(2.5), 2.0);
    assert_eq!(clip.wrap_time(-1.0), 0.0);

    clip.duration = 0.0;
    assert_eq!(clip.wrap_time(3.0), 0.0);
}

#[test]
fn sampling_writes_only_the_channels_that_have_keys() {
    let skeleton = crate::skeleton::tests::two_bone_skeleton();
    let mut pose = skeleton.rest_pose();
    let clip = Clip {
        name: "bend".into(),
        duration: 1.0,
        fps: 30.0,
        looped: false,
        additive: false,
        tracks: vec![Track {
            joint: 1,
            translation: Vec::new(),
            rotation: vec![
                q(0.0, Quat::IDENTITY),
                q(1.0, Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)),
            ],
            scale: Vec::new(),
        }],
        ..Default::default()
    };
    assert!(clip.sample_into(0.0, &mut pose));
    // Position was not written (the clip doesn't move the position channel).
    assert_eq!(pose.local(1).unwrap().translation, Vec3::new(2.0, 0.0, 0.0));
    clip.sample_into(1.0, &mut pose);
    let rotated = pose.local(1).unwrap().rotation * Vec3::X;
    assert!(rotated.y.abs() > 0.99, "got {rotated:?}");
}

#[test]
fn a_track_for_a_missing_joint_is_ignored() {
    let skeleton = crate::skeleton::tests::two_bone_skeleton();
    let mut pose = skeleton.rest_pose();
    let clip = Clip {
        name: "bad".into(),
        duration: 1.0,
        fps: 30.0,
        looped: false,
        additive: false,
        tracks: vec![Track {
            joint: 99,
            translation: vec![v3(0.0, Vec3::ONE)],
            ..Default::default()
        }],
        ..Default::default()
    };
    let before = pose.clone();
    clip.sample_into(0.0, &mut pose);
    assert_eq!(pose, before, "track pointing to a nonexistent bone should not modify the pose");
}

#[test]
fn max_key_time_sees_every_channel() {
    let clip = Clip {
        name: "c".into(),
        duration: 5.0,
        fps: 30.0,
        looped: false,
        additive: false,
        tracks: vec![
            Track {
                joint: 0,
                translation: vec![v3(0.0, Vec3::ZERO), v3(3.0, Vec3::ONE)],
                ..Default::default()
            },
            Track {
                joint: 1,
                scale: vec![v3(1.0, Vec3::ONE), v3(7.5, Vec3::ONE)],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    assert!((clip.max_key_time() - 7.5).abs() < 1e-6);
    assert!(!clip.is_empty());
    assert!(Clip {
        tracks: vec![Track {
            joint: 0,
            ..Default::default()
        }],
        ..clip
    }
    .is_empty());
}

#[test]
fn key_times_merge_every_track_and_drop_duplicates() {
    let clip = Clip {
        name: "c".into(),
        duration: 2.0,
        fps: 30.0,
        looped: true,
        additive: false,
        tracks: vec![
            Track {
                joint: 0,
                translation: vec![v3(0.0, Vec3::ZERO), v3(1.0, Vec3::ONE)],
                rotation: vec![q(1.0, Quat::IDENTITY)],
                ..Default::default()
            },
            Track {
                joint: 1,
                scale: vec![v3(0.5, Vec3::ONE), v3(f32::NAN, Vec3::ONE)],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    // 0.0 / 1.0 each appear twice → keep one each; NaN dropped.
    assert_eq!(clip.key_times(), vec![0.0, 0.5, 1.0]);
}

#[test]
fn a_clip_without_keys_has_no_key_times() {
    let clip = Clip {
        name: "empty".into(),
        duration: 0.0,
        fps: 30.0,
        looped: false,
        additive: false,
        tracks: Vec::new(),
        ..Default::default()
    };
    assert!(clip.key_times().is_empty());
}

fn notified_clip(duration: f32, looped: bool, notifies: &[(f32, &str)]) -> Clip {
    Clip {
        name: "notified".into(),
        duration,
        fps: 30.0,
        looped,
        additive: false,
        tracks: Vec::new(),
        notifies: notifies
            .iter()
            .map(|(time, name)| ClipNotify {
                time: *time,
                name: (*name).to_string(),
            })
            .collect(),
        sync_markers: Vec::new(),
        sections: Vec::new(),
        warp_windows: Vec::new(),
    }
}

fn fired(clip: &Clip, from: f32, to: f32) -> Vec<&str> {
    clip.notifies_between(from, to)
        .into_iter()
        .map(|notify| notify.name.as_str())
        .collect()
}

#[test]
fn notifies_fire_in_a_half_open_interval() {
    let clip = notified_clip(2.0, false, &[(0.5, "a"), (1.5, "b")]);
    assert_eq!(fired(&clip, 0.4, 0.6), vec!["a"]);
    // Start is an open interval: 0.5 already fired that frame, paused on it shouldn't replay.
    assert!(fired(&clip, 0.5, 0.6).is_empty());
    // One frame crossing two doesn't miss either.
    assert_eq!(fired(&clip, 0.0, 2.0), vec!["a", "b"]);
    // No events / non-finite / zero duration are all safe.
    assert!(fired(&notified_clip(2.0, false, &[]), 0.0, 2.0).is_empty());
    assert!(fired(&clip, f32::NAN, 1.0).is_empty());
    assert!(fired(&notified_clip(0.0, false, &[(0.5, "a")]), 0.0, 1.0).is_empty());
}

#[test]
fn a_looped_clip_fires_notifies_on_both_sides_of_the_wrap() {
    let clip = notified_clip(1.0, true, &[(0.05, "start"), (0.95, "end")]);
    // 0.9 → 1.0 → 0.1: each segment hits one.
    assert_eq!(fired(&clip, 0.9, 0.1), vec!["start", "end"]);
    // Only cross the end.
    assert_eq!(fired(&clip, 0.9, 0.98), vec!["end"]);
    // Only cross the start (point 0 itself must also be able to trigger).
    assert_eq!(fired(&clip, 0.98, 0.05), vec!["start"]);
}

#[test]
fn sync_markers_expose_a_normalized_marker_set() {
    let clip = Clip {
        sync_markers: vec![0.5, 0.0, 0.5, 1.25],
        ..Default::default()
    };
    let markers = clip.sync_marker_set().expect("markers");
    assert_eq!(markers.phases(), &[0.0, 0.25, 0.5]);

    let empty = Clip::default();
    assert!(empty.sync_marker_set().is_none());
}

#[test]
fn sample_joint_into_only_touches_the_requested_joint() {
    let clip = Clip {
        name: "two".into(),
        duration: 1.0,
        fps: 30.0,
        looped: false,
        additive: false,
        tracks: vec![
            Track {
                joint: 0,
                translation: vec![Vec3Key {
                    time: 0.0,
                    value: Vec3::new(7.0, 0.0, 0.0),
                }],
                rotation: Vec::new(),
                scale: Vec::new(),
            },
            Track {
                joint: 1,
                translation: Vec::new(),
                rotation: vec![crate::QuatKey {
                    time: 0.0,
                    value: glam::Quat::from_rotation_z(1.0),
                }],
                scale: Vec::new(),
            },
        ],
        ..Default::default()
    };
    let mut pose = Pose::from_locals(vec![
        crate::skeleton::JointTransform::IDENTITY,
        crate::skeleton::JointTransform::IDENTITY,
    ]);
    assert!(clip.sample_joint_into(0.0, 1, &mut pose));
    assert_eq!(pose.local(0).unwrap().translation, Vec3::ZERO, "root didn't move");
    assert!(
        (pose.local(1).unwrap().rotation.z - 0.5f32.sin()).abs() < 1e-6,
        "joint 1 sampled"
    );
    assert!(!clip.sample_joint_into(0.0, 9, &mut pose), "out-of-range joint");
}
