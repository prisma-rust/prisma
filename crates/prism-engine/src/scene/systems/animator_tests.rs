use super::*;
use glam::{Mat4, Quat, Vec3};
use prism_anim::{
    AnimState, AnimTransition, ClipNotify, ClipSection, Joint, JointTransform, Pose, StateMachine,
    Track, TransitionCondition, Vec3Key,
};
use prism_asset::formats_anim_graph::{
    AnimGraphInfo, GraphInputInfo, GraphNodeInfo, GraphNodeKind,
};
use prism_scene::components::{AnimLayer, SkeletonRoot};

fn two_bone_skeleton() -> Skeleton {
    Skeleton::new(vec![
        Joint {
            name: "root".into(),
            parent: None,
            reference_local: JointTransform::IDENTITY,
            inverse_bind: Mat4::IDENTITY,
        },
        Joint {
            name: "child".into(),
            parent: Some(0),
            reference_local: JointTransform::new(
                Vec3::new(2.0, 0.0, 0.0),
                Quat::IDENTITY,
                Vec3::ONE,
            ),
            inverse_bind: Mat4::from_translation(Vec3::new(2.0, 0.0, 0.0)).inverse(),
        },
    ])
    .expect("two bones")
}

/// Child bone rotates 0 → 90° about Z over 1s.
fn bend_clip() -> Clip {
    Clip {
        name: "bend".into(),
        duration: 1.0,
        fps: 30.0,
        looped: false,
        additive: false,
        tracks: vec![Track {
            joint: 1,
            translation: Vec::new(),
            rotation: vec![
                prism_anim::QuatKey {
                    time: 0.0,
                    value: Quat::IDENTITY,
                },
                prism_anim::QuatKey {
                    time: 1.0,
                    value: Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
                },
            ],
            scale: Vec::new(),
        }],
        ..Default::default()
    }
}

fn world_with_animator(clip_path: &str) -> (World, Entity) {
    let mut world = World::new();
    let mut library = SkeletonLibrary::new();
    library.insert_skeleton("rig.rskel", two_bone_skeleton());
    library.insert_clip("bend.ranim", bend_clip());
    world.insert_resource(library);

    let entity = world.spawn();
    world.insert(
        entity,
        SkeletonRoot {
            skeleton_path: "rig.rskel".into(),
            default_clip_path: "bend.ranim".into(),
        },
    );
    world.insert(
        entity,
        Animator {
            clip_path: clip_path.into(),
            ..Default::default()
        },
    );
    (world, entity)
}

fn add_camera(world: &mut World, position: Vec3) -> Entity {
    let entity = world.spawn();
    world.insert(entity, Camera::default());
    world.insert(
        entity,
        LocalTransform {
            position,
            ..Default::default()
        },
    );
    entity
}

/// Root bone moves +X from 0 to 2 over 1s, used to validate root-motion consumption.
fn forward_clip() -> Clip {
    Clip {
        name: "forward".into(),
        duration: 1.0,
        fps: 30.0,
        looped: false,
        additive: false,
        tracks: vec![Track {
            joint: 0,
            translation: vec![
                Vec3Key {
                    time: 0.0,
                    value: Vec3::ZERO,
                },
                Vec3Key {
                    time: 1.0,
                    value: Vec3::new(2.0, 0.0, 0.0),
                },
            ],
            rotation: Vec::new(),
            scale: Vec::new(),
        }],
        ..Default::default()
    }
}

fn install_forward(world: &mut World) {
    world
        .get_resource_mut::<SkeletonLibrary>()
        .unwrap()
        .insert_clip("forward.ranim", forward_clip());
}

fn root_translation(world: &World, entity: Entity) -> Vec3 {
    let palette = world.get::<BonePalette>(entity).expect("palette");
    Mat4::from_cols_array_2d(&palette.matrices[0])
        .w_axis
        .truncate()
}

#[test]
fn root_motion_consumption_moves_the_entity_and_zeroes_the_bone_offset() {
    let (mut world, entity) = world_with_animator("");
    install_forward(&mut world);
    world.insert(entity, LocalTransform::default());
    {
        let animator = world.get_mut::<Animator>(entity).unwrap();
        animator.clip_path = "forward.ranim".into();
        animator.looped = false;
        animator.root_motion_consume = true;
    }

    animator_system(&mut world, 0.0);
    animator_system(&mut world, 0.5);

    let position = world.get::<LocalTransform>(entity).unwrap().position;
    assert!(
        (position.x - 1.0).abs() < 1e-4,
        "Entity moved partway, position {position:?}"
    );
    assert!(
        root_translation(&world, entity).length() < 1e-4,
        "Bone root translation deducted to avoid double movement"
    );
    let head = world.get::<AnimatorPlayhead>(entity).unwrap();
    assert!((head.root_motion.translation.x - 1.0).abs() < 1e-4);
}

#[test]
fn root_motion_is_not_consumed_by_default() {
    let (mut world, entity) = world_with_animator("");
    install_forward(&mut world);
    world.insert(entity, LocalTransform::default());
    world.get_mut::<Animator>(entity).unwrap().clip_path = "forward.ranim".into();
    world.get_mut::<Animator>(entity).unwrap().looped = false;

    animator_system(&mut world, 0.0);
    animator_system(&mut world, 0.5);

    let position = world.get::<LocalTransform>(entity).unwrap().position;
    assert_eq!(
        position,
        Vec3::ZERO,
        "Default only records, doesn't move Transform"
    );
    assert!((root_translation(&world, entity).x - 1.0).abs() < 1e-4);
}

#[test]
fn distant_root_only_lod_keeps_bones_frozen_while_time_advances() {
    let (mut world, entity) = world_with_animator("");
    {
        let animator = world.get_mut::<Animator>(entity).unwrap();
        animator.lod_enabled = true;
        animator.lod_root_only = true;
    }
    add_camera(&mut world, Vec3::new(0.0, 0.0, 100.0));

    animator_system(&mut world, 0.0);
    let tip = child_tip(&world, entity);
    assert!(
        (tip.x - 2.0).abs() < 1e-4,
        "Full evaluation on first frame {tip:?}"
    );

    animator_system(&mut world, 0.5);
    let head = world.get::<AnimatorPlayhead>(entity).unwrap();
    assert!((head.time - 0.5).abs() < 1e-5, "Time advances as usual");
    let tip = child_tip(&world, entity);
    assert!(
        (tip.x - 2.0).abs() < 1e-4 && (tip.y - 1.0).abs() < 1e-4,
        "Only root bone sampled, child keeps previous frame {tip:?}"
    );
}

#[test]
fn distant_bone_subset_lod_updates_only_the_listed_bones() {
    let (mut world, entity) = world_with_animator("");
    {
        let animator = world.get_mut::<Animator>(entity).unwrap();
        animator.lod_enabled = true;
        animator.lod_bones = vec!["child".into(), "missing".into()];
    }
    add_camera(&mut world, Vec3::new(0.0, 0.0, 100.0));

    animator_system(&mut world, 0.0);
    let tip = child_tip(&world, entity);
    assert!(
        (tip.x - 2.0).abs() < 1e-4,
        "Full evaluation on first frame {tip:?}"
    );

    animator_system(&mut world, 0.5);
    let head = world.get::<AnimatorPlayhead>(entity).unwrap();
    assert!((head.time - 0.5).abs() < 1e-5, "Time advances as usual");
    let tip = child_tip(&world, entity);
    assert!(
        (tip.x - (2.0 - std::f32::consts::FRAC_PI_4.sin())).abs() < 1e-3,
        "Child in list, rotated to 45°; {tip:?}"
    );
}

#[test]
fn root_only_lod_still_consumes_root_motion() {
    let (mut world, entity) = world_with_animator("");
    install_forward(&mut world);
    world.insert(entity, LocalTransform::default());
    {
        let animator = world.get_mut::<Animator>(entity).unwrap();
        animator.clip_path = "forward.ranim".into();
        animator.looped = false;
        animator.lod_enabled = true;
        animator.lod_root_only = true;
        animator.root_motion_consume = true;
    }
    add_camera(&mut world, Vec3::new(0.0, 0.0, 100.0));

    animator_system(&mut world, 0.0);
    animator_system(&mut world, 0.5);

    let position = world.get::<LocalTransform>(entity).unwrap().position;
    assert!(
        (position.x - 1.0).abs() < 1e-4,
        "Root-only tier still consumes root motion, position {position:?}"
    );
    assert!(root_translation(&world, entity).length() < 1e-4);
}

#[test]
fn far_animators_freeze_and_resume_when_the_camera_comes_back() {
    let (mut world, entity) = world_with_animator("");
    world.get_mut::<Animator>(entity).unwrap().lod_enabled = true;
    let camera = add_camera(&mut world, Vec3::new(0.0, 0.0, 100.0));

    animator_system(&mut world, 0.2);
    assert!(
        world.get::<BonePalette>(entity).is_none(),
        "Frozen entity doesn't count as pose"
    );
    assert!(
        world.get::<AnimatorPlayhead>(entity).is_none(),
        "Frozen entity doesn't advance time"
    );

    world.get_mut::<LocalTransform>(camera).unwrap().position = Vec3::new(0.0, 0.0, 5.0);
    animator_system(&mut world, 0.25);
    let head = world
        .get::<AnimatorPlayhead>(entity)
        .expect("Resumes after returning to range");
    assert!(
        (head.time - 0.25).abs() < 1e-5,
        "No time backfill while frozen"
    );
    assert!(world.get::<BonePalette>(entity).is_some());
}

#[test]
fn lod_is_off_by_default_and_bad_distances_fall_back_to_fifty_metres() {
    let (mut world, entity) = world_with_animator("");
    add_camera(&mut world, Vec3::new(0.0, 0.0, 400.0));
    animator_system(&mut world, 0.25);
    let head = world
        .get::<AnimatorPlayhead>(entity)
        .expect("No default LOD");
    assert!((head.time - 0.25).abs() < 1e-5);

    {
        let animator = world.get_mut::<Animator>(entity).unwrap();
        animator.lod_enabled = true;
        animator.lod_far = -1.0;
    }
    animator_system(&mut world, 0.5);
    let head = world
        .get::<AnimatorPlayhead>(entity)
        .expect("Pose computed");
    assert!(
        (head.time - 0.25).abs() < 1e-5,
        "Bad distance treated as 50m, frozen beyond 400m"
    );
}

#[test]
fn near_animators_update_every_other_frame_with_double_dt() {
    let (mut world, entity) = world_with_animator("");
    world.insert(
        entity,
        LocalTransform {
            position: Vec3::new(0.0, 0.0, 30.0),
            ..Default::default()
        },
    );
    {
        let animator = world.get_mut::<Animator>(entity).unwrap();
        animator.lod_enabled = true;
        animator.lod_near = 25.0;
        animator.lod_far = 50.0;
    }
    let camera = add_camera(&mut world, Vec3::ZERO);

    animator_system(&mut world, 0.1);
    let time = world.get::<AnimatorPlayhead>(entity).unwrap().time;
    assert!(
        (time - 0.2).abs() < 1e-5,
        "Update frame advances 2*dt, time {time}"
    );
    animator_system(&mut world, 0.1);
    let time = world.get::<AnimatorPlayhead>(entity).unwrap().time;
    assert!((time - 0.2).abs() < 1e-5, "Next frame skipped, time {time}");
    animator_system(&mut world, 0.1);
    let time = world.get::<AnimatorPlayhead>(entity).unwrap().time;
    assert!(
        (time - 0.4).abs() < 1e-5,
        "Frame after continues, time {time}"
    );

    // Back near: next frame immediately resumes normal dt, no leftover skip flag.
    world.get_mut::<LocalTransform>(camera).unwrap().position = Vec3::new(0.0, 0.0, 30.0);
    animator_system(&mut world, 0.1);
    let time = world.get::<AnimatorPlayhead>(entity).unwrap().time;
    assert!(
        (time - 0.5).abs() < 1e-5,
        "Near uses per-frame dt, time {time}"
    );
}

#[test]
fn a_near_threshold_past_the_far_threshold_disables_the_middle_tier() {
    let (mut world, entity) = world_with_animator("");
    world.insert(
        entity,
        LocalTransform {
            position: Vec3::new(0.0, 0.0, 30.0),
            ..Default::default()
        },
    );
    {
        let animator = world.get_mut::<Animator>(entity).unwrap();
        animator.lod_enabled = true;
        animator.lod_near = 80.0;
        animator.lod_far = 50.0;
    }
    add_camera(&mut world, Vec3::ZERO);
    animator_system(&mut world, 0.1);
    let time = world.get::<AnimatorPlayhead>(entity).unwrap().time;
    assert!((time - 0.1).abs() < 1e-5, "No downgrade tier, time {time}");
    animator_system(&mut world, 0.1);
    let time = world.get::<AnimatorPlayhead>(entity).unwrap().time;
    assert!(
        (time - 0.2).abs() < 1e-5,
        "Updated every frame, time {time}"
    );
}

/// Clip where root joint translates +2m and rotates 90° about Y over 1s.
fn root_motion_clip() -> Clip {
    Clip {
        name: "root_motion".into(),
        duration: 1.0,
        fps: 30.0,
        looped: false,
        additive: false,
        tracks: vec![Track {
            joint: 0,
            translation: vec![
                Vec3Key {
                    time: 0.0,
                    value: Vec3::ZERO,
                },
                Vec3Key {
                    time: 1.0,
                    value: Vec3::new(2.0, 0.0, 0.0),
                },
            ],
            rotation: vec![
                prism_anim::QuatKey {
                    time: 0.0,
                    value: Quat::IDENTITY,
                },
                prism_anim::QuatKey {
                    time: 1.0,
                    value: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
                },
            ],
            scale: Vec::new(),
        }],
        ..Default::default()
    }
}

#[test]
fn root_motion_records_the_root_joint_delta_per_frame() {
    let (mut world, entity) = world_with_animator("root.ranim");
    world
        .get_resource_mut::<SkeletonLibrary>()
        .unwrap()
        .insert_clip("root.ranim", root_motion_clip());

    animator_system(&mut world, 0.0);
    let delta = world.get::<AnimatorPlayhead>(entity).unwrap().root_motion;
    assert_eq!(
        delta,
        RootMotionDelta::default(),
        "First frame has no previous to compare"
    );

    animator_system(&mut world, 0.25);
    let delta = world.get::<AnimatorPlayhead>(entity).unwrap().root_motion;
    assert!((delta.translation.x - 0.5).abs() < 1e-4, "delta {delta:?}");
    assert!(
        (delta.yaw - std::f32::consts::FRAC_PI_8).abs() < 1e-4,
        "delta {delta:?}"
    );

    animator_system(&mut world, 0.25);
    let delta = world.get::<AnimatorPlayhead>(entity).unwrap().root_motion;
    assert!((delta.translation.x - 0.5).abs() < 1e-4, "delta {delta:?}");
    assert!(
        (delta.yaw - std::f32::consts::FRAC_PI_8).abs() < 1e-4,
        "delta {delta:?}"
    );
}

#[test]
fn switching_clips_resets_the_recorded_root_delta() {
    let (mut world, entity) = world_with_animator("root.ranim");
    world
        .get_resource_mut::<SkeletonLibrary>()
        .unwrap()
        .insert_clip("root.ranim", root_motion_clip());
    animator_system(&mut world, 0.0);
    animator_system(&mut world, 0.5);
    assert!(
        world
            .get::<AnimatorPlayhead>(entity)
            .unwrap()
            .root_motion
            .translation
            .length()
            > 0.5
    );

    world.get_mut::<Animator>(entity).unwrap().clip_path = "bend.ranim".into();
    animator_system(&mut world, 0.1);
    let delta = world.get::<AnimatorPlayhead>(entity).unwrap().root_motion;
    assert_eq!(
        delta,
        RootMotionDelta::default(),
        "First frame after clip switch clears"
    );
}

/// Transform a bind-pose vertex with distance **from joint** (local `(2, 1, 0)`) to world.
///
/// Joint origin itself doesn't move under rotation, so need offset vertex to see animation.
fn child_tip(world: &World, entity: Entity) -> Vec3 {
    let palette = world.get::<BonePalette>(entity).expect("palette");
    let matrix = Mat4::from_cols_array_2d(&palette.matrices[1]);
    matrix.transform_point3(Vec3::new(2.0, 1.0, 0.0))
}

#[test]
fn the_animator_produces_a_palette_and_advances_time() {
    let (mut world, entity) = world_with_animator("");
    assert_eq!(
        animator_system(&mut world, 0.0),
        1,
        "Static pose computed on first frame"
    );

    let palette = world.get::<BonePalette>(entity).unwrap();
    assert_eq!(palette.len(), 2);

    // At 0.5s child rotated 45°: endpoint should leave +X axis.
    animator_system(&mut world, 0.5);
    let head = world.get::<AnimatorPlayhead>(entity).unwrap();
    assert!((head.time - 0.5).abs() < 1e-5);
    // (2,1,0) rotated 45° about joint: x = 2 - sin45, y = cos45.
    let tip = child_tip(&world, entity);
    let half_sqrt2 = std::f32::consts::FRAC_1_SQRT_2;
    assert!((tip.x - (2.0 - half_sqrt2)).abs() < 1e-4, "tip {tip:?}");
    assert!((tip.y - half_sqrt2).abs() < 1e-4, "tip {tip:?}");
}

#[test]
fn a_paused_animator_keeps_its_pose() {
    let (mut world, entity) = world_with_animator("");
    animator_system(&mut world, 0.0);
    world.get_mut::<Animator>(entity).unwrap().playing = false;
    animator_system(&mut world, 5.0);
    let head = world.get::<AnimatorPlayhead>(entity).unwrap();
    assert_eq!(head.time, 0.0, "Time doesn't advance when paused");
    assert!(world.get::<BonePalette>(entity).is_some());
}

#[test]
fn a_once_clip_stops_at_the_end_and_flags_finished() {
    let (mut world, entity) = world_with_animator("");
    world.get_mut::<Animator>(entity).unwrap().looped = false;
    animator_system(&mut world, 10.0);
    let head = world.get::<AnimatorPlayhead>(entity).unwrap();
    assert!(
        (head.time - 1.0).abs() < 1e-5,
        "One-shot clamped at duration"
    );
    assert!(head.finished);
    // Final pose: child rotated 90°, (2,1,0) about joint becomes (1,0,0).
    let tip = child_tip(&world, entity);
    assert!((tip.x - 1.0).abs() < 1e-4, "tip {tip:?}");
    assert!(tip.y.abs() < 1e-4, "tip {tip:?}");
}

#[test]
fn a_looping_clip_wraps_the_playhead() {
    let (mut world, entity) = world_with_animator("");
    world.get_mut::<Animator>(entity).unwrap().looped = true;
    world.get_mut::<Animator>(entity).unwrap().speed = 1.0;
    animator_system(&mut world, 2.25);
    let head = world.get::<AnimatorPlayhead>(entity).unwrap();
    assert!(!head.finished, "Looping clip never finishes");
    // 2.25s / 1.0s loop → samples 0.25s (about 22.5°).
    let angle = 0.25 * std::f32::consts::FRAC_PI_2;
    let tip = child_tip(&world, entity);
    assert!((tip.x - (2.0 - angle.sin())).abs() < 1e-3, "tip {tip:?}");
    assert!((tip.y - angle.cos()).abs() < 1e-3, "tip {tip:?}");
}

#[test]
fn the_default_clip_from_the_skeleton_root_is_used_when_none_is_set() {
    let (mut world, entity) = world_with_animator("");
    animator_system(&mut world, 0.0);
    assert_eq!(
        world.get::<AnimatorPlayhead>(entity).unwrap().clip_path,
        "bend.ranim"
    );
}

#[test]
fn an_animator_without_its_assets_is_left_alone() {
    let (mut world, entity) = world_with_animator("");
    world.get_resource_mut::<SkeletonLibrary>().unwrap().clear();
    assert_eq!(animator_system(&mut world, 0.5), 0);
    assert!(world.get::<BonePalette>(entity).is_none());
    assert!(world.get::<AnimatorPlayhead>(entity).is_none());
}

#[test]
fn changing_the_clip_restarts_from_the_authored_time() {
    let (mut world, entity) = world_with_animator("");
    animator_system(&mut world, 0.5);
    let mut other = bend_clip();
    other.name = "other".into();
    world
        .get_resource_mut::<SkeletonLibrary>()
        .unwrap()
        .insert_clip("other.ranim", other);

    let animator = world.get_mut::<Animator>(entity).unwrap();
    animator.clip_path = "other.ranim".into();
    animator.time = 0.25;
    animator.playing = false;
    animator_system(&mut world, 0.5);

    let head = world.get::<AnimatorPlayhead>(entity).unwrap();
    assert_eq!(head.clip_path, "other.ranim");
    assert!(
        (head.time - 0.25).abs() < 1e-5,
        "Clip switch restarts at authored time"
    );
}

/// Clip with child bone constantly +90° (about Z): clearly different from `bend_clip` start pose (0°).
fn bent_90_clip(name: &str) -> Clip {
    Clip {
        name: name.into(),
        duration: 1.0,
        fps: 30.0,
        looped: true,
        additive: false,
        tracks: vec![Track {
            joint: 1,
            translation: Vec::new(),
            rotation: vec![prism_anim::QuatKey {
                time: 0.0,
                value: Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            }],
            scale: Vec::new(),
        }],
        ..Default::default()
    }
}

/// Switch entity to `path`, set `blend_duration` / `playing` (without running system).
fn switch_clip(world: &mut World, entity: Entity, path: &str, blend_duration: f32) {
    let animator = world.get_mut::<Animator>(entity).unwrap();
    animator.clip_path = path.into();
    animator.time = 0.0;
    animator.blend_duration = blend_duration;
    animator.playing = true;
}

#[test]
fn switching_clips_crossfades_instead_of_popping() {
    let (mut world, entity) = world_with_animator("");
    world
        .get_resource_mut::<SkeletonLibrary>()
        .unwrap()
        .insert_clip("bent90.ranim", bent_90_clip("bent90"));
    // Start: bend at 0s (child 0°).
    animator_system(&mut world, 0.0);
    let start = child_tip(&world, entity);
    assert!((start.y - 1.0).abs() < 1e-4, "start tip {start:?}");

    switch_clip(&mut world, entity, "bent90.ranim", 1.0);
    // First frame: weight still near 0 → still old pose (this is "no hard cut").
    animator_system(&mut world, 0.0);
    let faded_in = child_tip(&world, entity);
    assert!(
        (faded_in.y - 1.0).abs() < 1e-4,
        "Transition start should be old pose, got {faded_in:?}"
    );

    // Halfway: weight 0.5 → about 45°.
    animator_system(&mut world, 0.5);
    let half = child_tip(&world, entity);
    let angle = std::f32::consts::FRAC_PI_4;
    assert!(
        (half.x - (2.0 - angle.sin())).abs() < 1e-3,
        "Halfway should be 45°, got {half:?}"
    );
    assert!((half.y - angle.cos()).abs() < 1e-3, "half {half:?}");

    // Done: weight 1 → full new clip (90°), source cleared.
    animator_system(&mut world, 0.5);
    let done = child_tip(&world, entity);
    assert!(
        (done.x - 1.0).abs() < 1e-3,
        "End should be 90°, got {done:?}"
    );
    assert!(done.y.abs() < 1e-3, "End should be 90°, got {done:?}");
    let head = world.get::<AnimatorPlayhead>(entity).unwrap();
    assert!(
        head.previous_pose.is_none(),
        "Should not hold source after transition"
    );
}

#[test]
fn a_zero_blend_duration_switches_immediately() {
    let (mut world, entity) = world_with_animator("");
    world
        .get_resource_mut::<SkeletonLibrary>()
        .unwrap()
        .insert_clip("bent90.ranim", bent_90_clip("bent90"));
    animator_system(&mut world, 0.0);

    switch_clip(&mut world, entity, "bent90.ranim", 0.0);
    animator_system(&mut world, 0.0);

    let tip = child_tip(&world, entity);
    assert!(
        (tip.x - 1.0).abs() < 1e-3,
        "0 duration = direct switch, got {tip:?}"
    );
    assert!(world
        .get::<AnimatorPlayhead>(entity)
        .unwrap()
        .previous_pose
        .is_none());
}

#[test]
fn a_paused_animator_does_not_advance_the_crossfade() {
    let (mut world, entity) = world_with_animator("");
    world
        .get_resource_mut::<SkeletonLibrary>()
        .unwrap()
        .insert_clip("bent90.ranim", bent_90_clip("bent90"));
    animator_system(&mut world, 0.0);

    switch_clip(&mut world, entity, "bent90.ranim", 1.0);
    world.get_mut::<Animator>(entity).unwrap().playing = false;
    animator_system(&mut world, 0.4);
    animator_system(&mut world, 0.4);

    let head = world.get::<AnimatorPlayhead>(entity).unwrap();
    assert_eq!(
        head.blend_elapsed, 0.0,
        "Transition shouldn't advance while paused"
    );
    assert!(
        head.previous_pose.is_some(),
        "Snapshot still held while paused"
    );
    let tip = child_tip(&world, entity);
    assert!(
        (tip.y - 1.0).abs() < 1e-4,
        "Still shows old pose (0° tip at y=1), got {tip:?}"
    );
}

#[test]
fn losing_the_source_clip_keeps_the_snapshot_and_still_completes() {
    // Transition source is **pose snapshot**, so removing old clip from library (asset delete / hot reload) doesn't affect fade-out:
    // previously this hard-cut, now must still finish transition.
    let (mut world, entity) = world_with_animator("");
    world
        .get_resource_mut::<SkeletonLibrary>()
        .unwrap()
        .insert_clip("bent90.ranim", bent_90_clip("bent90"));
    animator_system(&mut world, 0.0);

    switch_clip(&mut world, entity, "bent90.ranim", 1.0);
    // Remove old clip from library on same frame transition starts.
    {
        let library = world.get_resource_mut::<SkeletonLibrary>().unwrap();
        library.clear();
        library.insert_skeleton("rig.rskel", two_bone_skeleton());
        library.insert_clip("bent90.ranim", bent_90_clip("bent90"));
    }
    animator_system(&mut world, 0.0);

    let head = world.get::<AnimatorPlayhead>(entity).unwrap();
    assert!(
        head.previous_pose.is_some(),
        "Snapshot held, transition shouldn't be abandoned"
    );
    let tip = child_tip(&world, entity);
    assert!(
        (tip.y - 1.0).abs() < 1e-4,
        "Transition start still old pose, got {tip:?}"
    );

    // Done: pure new clip, snapshot dropped.
    animator_system(&mut world, 1.0);
    let tip = child_tip(&world, entity);
    assert!(
        (tip.x - 1.0).abs() < 1e-3,
        "After transition should be 90°, got {tip:?}"
    );
    assert!(world
        .get::<AnimatorPlayhead>(entity)
        .unwrap()
        .previous_pose
        .is_none());
}

#[test]
fn blend_weights_are_clamped_and_need_both_a_source_and_a_duration() {
    let mut head = AnimatorPlayhead {
        clip_path: "new".into(),
        time: 0.0,
        finished: false,
        previous_pose: Some(Pose::default()),
        pose: Pose::default(),
        blend_elapsed: 0.0,
        blend_total: None,
        root_motion: RootMotionDelta::default(),
        root_previous: None,
        smoothed_blend: None,
    };
    assert_eq!(head.blend_weight(1.0), Some(0.0));
    head.blend_elapsed = 0.5;
    assert_eq!(head.blend_weight(1.0), Some(0.5));
    head.blend_elapsed = 99.0;
    assert_eq!(head.blend_weight(1.0), Some(1.0), "Weight clamped to 1");
    assert_eq!(head.blend_weight(0.0), None, "0 duration = no transition");
    head.previous_pose = None;
    assert_eq!(head.blend_weight(1.0), None, "No source means no weight");
}

#[test]
fn bad_blend_durations_are_treated_as_zero() {
    assert_eq!(sanitize_blend_duration(f32::NAN), 0.0);
    assert_eq!(sanitize_blend_duration(-1.0), 0.0);
    assert_eq!(sanitize_blend_duration(f32::INFINITY), 0.0);
    assert_eq!(sanitize_blend_duration(0.0), 0.0);
    assert_eq!(sanitize_blend_duration(0.2), 0.2);
}

fn install_bent90(world: &mut World) {
    world
        .get_resource_mut::<SkeletonLibrary>()
        .unwrap()
        .insert_clip("bent90.ranim", bent_90_clip("bent90"));
}

/// Switch entity to 1D blend space between "bend ↔ bent90", `parameter` is position on axis.
fn use_blend_space(world: &mut World, entity: Entity, parameter: f32) {
    let animator = world.get_mut::<Animator>(entity).unwrap();
    animator.blend_clip_paths = vec!["bend.ranim".into(), "bent90.ranim".into()];
    animator.blend_positions = vec![0.0, 1.0];
    animator.blend_parameter = parameter;
    animator.time = 0.0;
    animator.playing = false;
}

#[test]
fn blend_parameter_smoothing_eases_towards_the_target() {
    let (mut world, entity) = world_with_animator("");
    install_bent90(&mut world);
    use_blend_space(&mut world, entity, 0.0);
    world.get_mut::<Animator>(entity).unwrap().blend_smoothing = 0.5;

    animator_system(&mut world, 0.0);
    assert_eq!(
        world
            .get::<AnimatorPlayhead>(entity)
            .unwrap()
            .smoothed_blend,
        Some([0.0, 0.0]),
        "First frame lands directly on target"
    );

    world.get_mut::<Animator>(entity).unwrap().blend_parameter = 1.0;
    animator_system(&mut world, 0.1);
    let smoothed = world
        .get::<AnimatorPlayhead>(entity)
        .unwrap()
        .smoothed_blend
        .expect("smoothing on");
    assert!(
        smoothed[0] > 0.0 && smoothed[0] < 1.0,
        "One frame only partial, smoothed {smoothed:?}"
    );
    let tip = child_tip(&world, entity);
    assert!(tip.x > 1.5, "Not yet 90°, tip {tip:?}");

    for _ in 0..120 {
        animator_system(&mut world, 0.1);
    }
    let smoothed = world
        .get::<AnimatorPlayhead>(entity)
        .unwrap()
        .smoothed_blend
        .expect("smoothing on");
    assert!(
        (smoothed[0] - 1.0).abs() < 0.01,
        "Approaching target, {smoothed:?}"
    );
    let tip = child_tip(&world, entity);
    assert!((tip.x - 1.0).abs() < 0.05, "Finally 90°, tip {tip:?}");
}

#[test]
fn smoothing_zero_uses_the_written_parameter_immediately() {
    let (mut world, entity) = world_with_animator("");
    install_bent90(&mut world);
    use_blend_space(&mut world, entity, 1.0);
    animator_system(&mut world, 0.1);
    assert_eq!(
        world
            .get::<AnimatorPlayhead>(entity)
            .unwrap()
            .smoothed_blend,
        None,
        "0 = no smoothing"
    );
    let tip = child_tip(&world, entity);
    assert!(
        (tip.x - 1.0).abs() < 1e-3,
        "Parameter applies directly, tip {tip:?}"
    );
}

#[test]
fn a_one_dimensional_blend_space_is_sampled_from_the_parameter() {
    let (mut world, entity) = world_with_animator("");
    install_bent90(&mut world);

    // parameter 0 → pure bend.ranim (0° at 0s).
    use_blend_space(&mut world, entity, 0.0);
    animator_system(&mut world, 0.0);
    let tip = child_tip(&world, entity);
    assert!(
        (tip.y - 1.0).abs() < 1e-4,
        "Parameter 0 should be 0°, got {tip:?}"
    );
    assert_eq!(
        world.get::<AnimatorPlayhead>(entity).unwrap().clip_path,
        BLEND_SPACE_KEY,
        "Blend space uses its own playback key"
    );

    // parameter 1 → pure bent90 (90°).
    use_blend_space(&mut world, entity, 1.0);
    animator_system(&mut world, 0.0);
    let tip = child_tip(&world, entity);
    assert!(
        (tip.x - 1.0).abs() < 1e-3,
        "Parameter 1 should be 90°, got {tip:?}"
    );

    // parameter 0.5 → 45°.
    use_blend_space(&mut world, entity, 0.5);
    animator_system(&mut world, 0.0);
    let tip = child_tip(&world, entity);
    let angle = std::f32::consts::FRAC_PI_4;
    assert!(
        (tip.x - (2.0 - angle.sin())).abs() < 1e-3,
        "Parameter 0.5 should be 45°, got {tip:?}"
    );
}

#[test]
fn a_blend_space_needs_two_loaded_clips_and_falls_back_to_the_single_one() {
    let (mut world, entity) = world_with_animator("");
    install_bent90(&mut world);

    // Two paths but one not in library → only one sample, falls back to single clip.
    {
        let animator = world.get_mut::<Animator>(entity).unwrap();
        animator.blend_clip_paths = vec!["bend.ranim".into(), "missing.ranim".into()];
        animator.blend_positions = vec![0.0, 1.0];
        animator.playing = false;
    }
    animator_system(&mut world, 0.0);
    let head = world.get::<AnimatorPlayhead>(entity).unwrap();
    assert_eq!(
        head.clip_path, "bend.ranim",
        "Falls back to single clip when missing a sample"
    );

    // Only one path + one position: not a blend space.
    {
        let animator = world.get_mut::<Animator>(entity).unwrap();
        animator.blend_clip_paths = vec!["bend.ranim".into()];
        animator.blend_positions = vec![0.0, 1.0];
    }
    animator_system(&mut world, 0.0);
    assert_eq!(
        world.get::<AnimatorPlayhead>(entity).unwrap().clip_path,
        "bend.ranim"
    );
}

#[test]
fn a_blend_space_clamps_out_of_range_parameters() {
    let (mut world, entity) = world_with_animator("");
    install_bent90(&mut world);
    for (parameter, expect_90) in [(-5.0, false), (50.0, true)] {
        use_blend_space(&mut world, entity, parameter);
        animator_system(&mut world, 0.0);
        let tip = child_tip(&world, entity);
        if expect_90 {
            assert!(
                (tip.x - 1.0).abs() < 1e-3,
                "Parameter 50 clamped to 1, got {tip:?}"
            );
        } else {
            assert!(
                (tip.y - 1.0).abs() < 1e-4,
                "Parameter -5 clamped to 0, got {tip:?}"
            );
        }
    }
}

#[test]
fn entering_a_blend_space_crossfades_from_the_single_clip() {
    let (mut world, entity) = world_with_animator("");
    install_bent90(&mut world);
    // First run single clip one frame, establishing playhead on bend.ranim.
    animator_system(&mut world, 0.0);

    use_blend_space(&mut world, entity, 1.0);
    world.get_mut::<Animator>(entity).unwrap().playing = true;
    world.get_mut::<Animator>(entity).unwrap().blend_duration = 1.0;
    // Transition start: weight 0 → still bend pose.
    animator_system(&mut world, 0.0);
    let tip = child_tip(&world, entity);
    assert!(
        (tip.y - 1.0).abs() < 1e-4,
        "First frame still old pose, got {tip:?}"
    );
    let head = world.get::<AnimatorPlayhead>(entity).unwrap();
    assert!(head.previous_pose.is_some(), "Old pose frozen on entry");
    assert_eq!(head.clip_path, BLEND_SPACE_KEY);

    // Transition done: pure blend-space pose (90°), source cleared.
    animator_system(&mut world, 1.0);
    let tip = child_tip(&world, entity);
    assert!(
        (tip.x - 1.0).abs() < 1e-3,
        "After transition should be 90°, got {tip:?}"
    );
    assert!(world
        .get::<AnimatorPlayhead>(entity)
        .unwrap()
        .previous_pose
        .is_none());
}

#[test]
fn the_sanitizer_rejects_bad_times() {
    assert_eq!(sanitize_time(f32::NAN), 0.0);
    assert_eq!(sanitize_time(f32::INFINITY), 0.0);
    assert_eq!(sanitize_time(2.5), 2.5);
}

#[test]
fn the_library_reports_what_it_holds() {
    let mut library = SkeletonLibrary::new();
    assert!(library.insert_skeleton("a", two_bone_skeleton()));
    assert!(
        !library.insert_skeleton("a", two_bone_skeleton()),
        "Duplicate insert"
    );
    assert!(library.insert_clip("c", bend_clip()));
    assert!(library.has_skeleton("a"));
    assert!(library.has_clip("c"));
    assert!(library.skeleton("a").is_some());
    assert!(library.clip("c").is_some());
    assert_eq!(library.skeleton_count(), 1);
    assert_eq!(library.clip_count(), 1);
    library.clear();
    assert!(library.skeleton("a").is_none());
}

#[test]
fn a_track_pointing_at_a_missing_joint_does_not_stop_the_palette() {
    let (mut world, entity) = world_with_animator("");
    let mut clip = bend_clip();
    clip.tracks.push(Track {
        joint: 42,
        translation: vec![Vec3Key {
            time: 0.0,
            value: Vec3::ONE,
        }],
        rotation: Vec::new(),
        scale: Vec::new(),
    });
    world
        .get_resource_mut::<SkeletonLibrary>()
        .unwrap()
        .insert_clip("bend.ranim", clip);
    assert_eq!(animator_system(&mut world, 0.1), 1);
    assert_eq!(world.get::<BonePalette>(entity).unwrap().len(), 2);
}

#[test]
fn the_library_reports_clip_key_times_for_the_timeline() {
    let mut library = SkeletonLibrary::new();
    let clip = Clip {
        name: "walk".into(),
        duration: 2.0,
        fps: 30.0,
        looped: true,
        additive: false,
        tracks: vec![Track {
            joint: 1,
            translation: vec![
                Vec3Key {
                    time: 0.0,
                    value: Vec3::ZERO,
                },
                Vec3Key {
                    time: 1.25,
                    value: Vec3::X,
                },
                Vec3Key {
                    time: 2.0,
                    value: Vec3::Y,
                },
            ],
            ..Default::default()
        }],
        ..Default::default()
    };
    library.insert_clip("char/walk.ranim", clip);
    assert_eq!(
        library.clip_key_times("char/walk.ranim"),
        vec![0.0, 1.25, 2.0]
    );
    assert!(library.clip_key_times("char/missing.ranim").is_empty());
}

fn clip_with_notify(time: f32, name: &str) -> Clip {
    let mut clip = bend_clip();
    clip.duration = 1.0;
    clip.looped = true;
    clip.notifies = vec![ClipNotify {
        time,
        name: name.to_string(),
    }];
    clip
}

#[test]
fn a_notify_fires_once_when_the_playhead_crosses_it() {
    let (mut world, entity) = world_with_animator("");
    world
        .get_resource_mut::<SkeletonLibrary>()
        .unwrap()
        .insert_clip("bend.ranim", clip_with_notify(0.5, "Footstep.L"));

    // First frame only establishes playhead: shouldn't fire whole clip events at once.
    animator_system(&mut world, 0.0);
    assert!(world.get_resource::<AnimEvents>().unwrap().is_empty());

    // 0 → 0.4: not yet.
    animator_system(&mut world, 0.4);
    assert!(world.get_resource::<AnimEvents>().unwrap().is_empty());

    // 0.4 → 0.8: crosses 0.5, fires once.
    animator_system(&mut world, 0.4);
    let events = world.get_resource::<AnimEvents>().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events.events[0].entity, entity);
    assert_eq!(events.events[0].name, "Footstep.L");
    assert!((events.events[0].time - 0.5).abs() < 1e-6);
    assert_eq!(
        events.names_for(entity).collect::<Vec<_>>(),
        vec!["Footstep.L"]
    );

    // One more frame (no time point crossed): event table rebuilt empty, won't refire.
    animator_system(&mut world, 0.1);
    assert!(world.get_resource::<AnimEvents>().unwrap().is_empty());
}

#[test]
fn a_notify_at_the_wrap_point_fires_on_the_next_loop() {
    let (mut world, _entity) = world_with_animator("");
    world
        .get_resource_mut::<SkeletonLibrary>()
        .unwrap()
        .insert_clip("bend.ranim", clip_with_notify(0.95, "Footstep.R"));

    animator_system(&mut world, 0.0);
    animator_system(&mut world, 0.9); // advance to 0.9
    assert!(world.get_resource::<AnimEvents>().unwrap().is_empty());

    // 0.9 → 1.0 → wrap to 0.0: this segment crosses 0.95.
    animator_system(&mut world, 0.1);
    let names: Vec<String> = world
        .get_resource::<AnimEvents>()
        .unwrap()
        .events
        .iter()
        .map(|event| event.name.clone())
        .collect();
    assert_eq!(names, vec!["Footstep.R".to_string()]);
}

#[test]
fn a_paused_animator_does_not_fire_notifies() {
    let (mut world, entity) = world_with_animator("");
    world
        .get_resource_mut::<SkeletonLibrary>()
        .unwrap()
        .insert_clip("bend.ranim", clip_with_notify(0.5, "Footstep.L"));
    animator_system(&mut world, 0.0);
    animator_system(&mut world, 0.4);

    world.get_mut::<Animator>(entity).unwrap().playing = false;
    animator_system(&mut world, 0.4);
    assert!(
        world.get_resource::<AnimEvents>().unwrap().is_empty(),
        "Time doesn't advance when paused, so no events either"
    );
}

#[test]
fn a_reversed_animator_does_not_fire_notifies() {
    let (mut world, entity) = world_with_animator("");
    world
        .get_resource_mut::<SkeletonLibrary>()
        .unwrap()
        .insert_clip("bend.ranim", clip_with_notify(0.5, "Footstep.L"));
    animator_system(&mut world, 0.0);
    animator_system(&mut world, 0.9);

    world.get_mut::<Animator>(entity).unwrap().speed = -1.0;
    // 0.9 → 0.5 crossing 0.5 backwards: reverse playback doesn't fire (prefer missing one over replaying backwards).
    animator_system(&mut world, 0.4);
    assert!(world.get_resource::<AnimEvents>().unwrap().is_empty());
}

#[test]
fn leaving_a_blend_space_crossfades_from_the_frozen_pose() {
    // Previously "blend space → single clip" was hard cut (source is placeholder key, no sample in library).
    // Now transition source is **pose snapshot**, so here must truly fade out.
    let (mut world, entity) = world_with_animator("");
    install_bent90(&mut world);
    use_blend_space(&mut world, entity, 0.5);
    world.get_mut::<Animator>(entity).unwrap().playing = false;
    animator_system(&mut world, 0.0);
    let mixed = child_tip(&world, entity);
    assert!(
        (mixed.x - (2.0 - std::f32::consts::FRAC_PI_4.sin())).abs() < 1e-3,
        "Blend space should be 45°, got {mixed:?}"
    );

    // Switch to single clip (90°), 1 second transition.
    {
        let animator = world.get_mut::<Animator>(entity).unwrap();
        animator.blend_clip_paths.clear();
        animator.blend_positions.clear();
        animator.clip_path = "bent90.ranim".into();
        animator.blend_duration = 1.0;
        animator.playing = true;
        animator.time = 0.0;
    }
    animator_system(&mut world, 0.0);
    let tip = child_tip(&world, entity);
    assert!(
        (tip.x - (2.0 - std::f32::consts::FRAC_PI_4.sin())).abs() < 1e-3,
        "Transition start still old 45° pose, got {tip:?}"
    );

    // Done: pure 90°.
    animator_system(&mut world, 1.0);
    let tip = child_tip(&world, entity);
    assert!(
        (tip.x - 1.0).abs() < 1e-3,
        "After transition should be 90°, got {tip:?}"
    );
}

/// Additive layer test: base clip stays 0°, additive clip rotates another 90° on top of 0°.
fn additive_setup() -> (World, Entity) {
    let (mut world, entity) = world_with_animator("");
    world
        .get_resource_mut::<SkeletonLibrary>()
        .unwrap()
        .insert_clip("bent90.ranim", bent_90_clip("bent90"));
    {
        let animator = world.get_mut::<Animator>(entity).unwrap();
        animator.playing = false;
        animator.time = 0.0;
        animator.additive_clip_path = "bent90.ranim".into();
        animator.additive_weight = 1.0;
    }
    (world, entity)
}

#[test]
fn an_additive_layer_rotates_on_top_of_the_base_clip() {
    let (mut world, entity) = additive_setup();
    animator_system(&mut world, 0.0);
    let tip = child_tip(&world, entity);
    // Base 0° + additive 90° = 90°.
    assert!(
        (tip.x - 1.0).abs() < 1e-3 && tip.y.abs() < 1e-3,
        "Additive layer should rotate child to 90°, got {tip:?}"
    );
}

#[test]
fn the_additive_weight_scales_the_delta() {
    let (mut world, entity) = additive_setup();
    world.get_mut::<Animator>(entity).unwrap().additive_weight = 0.5;
    animator_system(&mut world, 0.0);
    let tip = child_tip(&world, entity);
    assert!(
        (tip.x - (2.0 - std::f32::consts::FRAC_PI_4.sin())).abs() < 1e-3,
        "Half delta = 45°, got {tip:?}"
    );
}

#[test]
fn a_missing_or_disabled_additive_layer_leaves_the_base_pose_alone() {
    // Bad path (clip not in library).
    let (mut world, entity) = additive_setup();
    world
        .get_mut::<Animator>(entity)
        .unwrap()
        .additive_clip_path = "missing.ranim".into();
    animator_system(&mut world, 0.0);
    let tip = child_tip(&world, entity);
    assert!(
        (tip.y - 1.0).abs() < 1e-4,
        "Without additive layer stays 0°, got {tip:?}"
    );

    // Weight 0 / NaN.
    let (mut world, entity) = additive_setup();
    world.get_mut::<Animator>(entity).unwrap().additive_weight = 0.0;
    animator_system(&mut world, 0.0);
    assert!((child_tip(&world, entity).y - 1.0).abs() < 1e-4);

    let (mut world, entity) = additive_setup();
    world.get_mut::<Animator>(entity).unwrap().additive_weight = f32::NAN;
    animator_system(&mut world, 0.0);
    assert!((child_tip(&world, entity).y - 1.0).abs() < 1e-4);
}

#[test]
fn the_library_reports_clip_notifies_for_the_timeline() {
    let mut library = SkeletonLibrary::new();
    let mut clip = bent_90_clip("with-notify");
    clip.notifies = vec![
        prism_anim::ClipNotify {
            time: 0.25,
            name: "Footstep.L".into(),
        },
        prism_anim::ClipNotify {
            time: 0.5,
            name: "Footstep.R".into(),
        },
    ];
    library.insert_clip("char/walk.ranim", clip);
    assert_eq!(
        library.clip_notifies("char/walk.ranim"),
        vec![
            (0.25, "Footstep.L".to_string()),
            (0.5, "Footstep.R".to_string())
        ]
    );
    assert!(library.clip_notifies("char/missing.ranim").is_empty());
}

/// Two-state machine for "idle → run": transition when `speed > 0.5`.
fn speed_machine(duration: f32, min_state_fraction: f32) -> StateMachine {
    StateMachine {
        states: vec![
            AnimState {
                name: "idle".into(),
                clip_path: "bend.ranim".into(),
                speed: 1.0,
                looped: false,
            },
            AnimState {
                name: "run".into(),
                clip_path: "bent90.ranim".into(),
                speed: 1.0,
                looped: false,
            },
        ],
        transitions: vec![AnimTransition {
            from: 0,
            to: 1,
            min_state_fraction,
            duration,
            conditions: vec![TransitionCondition::FloatGreater {
                name: "speed".into(),
                threshold: 0.5,
            }],
        }],
    }
}

fn install_machine(world: &mut World, machine: StateMachine) {
    let mut library = StateMachineLibrary::new();
    library.insert("hero.rasm", machine);
    world.insert_resource(library);
}

#[test]
fn a_state_machine_drives_the_clip_and_writes_the_state_back() {
    let (mut world, entity) = world_with_animator("");
    install_bent90(&mut world);
    install_machine(&mut world, speed_machine(0.0, 0.0));
    {
        let animator = world.get_mut::<Animator>(entity).unwrap();
        animator.state_machine_path = "hero.rasm".into();
        animator.playing = false;
    }

    // Parameter 0: stay in idle (bend.ranim 0°).
    animator_system(&mut world, 0.0);
    assert_eq!(world.get::<Animator>(entity).unwrap().state, "idle");
    let tip = child_tip(&world, entity);
    assert!((tip.y - 1.0).abs() < 1e-4, "Idle should be 0°, got {tip:?}");

    // Parameter past threshold: next frame switch to run (bent90.ranim 90°).
    let mut params = prism_anim::AnimParameters::default();
    params.set_float("speed", 1.0);
    world.insert_resource(params);
    animator_system(&mut world, 0.0);
    assert_eq!(world.get::<Animator>(entity).unwrap().state, "run");
    let tip = child_tip(&world, entity);
    assert!((tip.x - 1.0).abs() < 1e-3, "Run should be 90°, got {tip:?}");
    // State change restarts from 0, doesn't continue idle's time.
    assert!(
        world.get::<AnimatorPlayhead>(entity).unwrap().time.abs() < 1e-6,
        "Time should reset after state change"
    );
}

#[test]
fn a_transition_duration_overrides_the_component_blend_duration() {
    let (mut world, entity) = world_with_animator("");
    install_bent90(&mut world);
    // Transition carries own 2s duration; component set to 0 (hard cut) must defer to transition.
    install_machine(&mut world, speed_machine(2.0, 0.0));
    {
        let animator = world.get_mut::<Animator>(entity).unwrap();
        animator.state_machine_path = "hero.rasm".into();
        animator.blend_duration = 0.0;
        animator.playing = false;
    }
    animator_system(&mut world, 0.0);
    let mut params = prism_anim::AnimParameters::default();
    params.set_float("speed", 1.0);
    world.insert_resource(params);

    // First frame of transition: weight 0 → still idle 0° pose.
    animator_system(&mut world, 0.0);
    let tip = child_tip(&world, entity);
    assert!(
        (tip.y - 1.0).abs() < 1e-4,
        "Transition start still old pose, got {tip:?}"
    );

    // After 2 seconds: 90°.
    world.get_mut::<Animator>(entity).unwrap().playing = true;
    animator_system(&mut world, 2.0);
    let tip = child_tip(&world, entity);
    assert!(
        (tip.x - 1.0).abs() < 1e-3,
        "After transition should be 90°, got {tip:?}"
    );
}

#[test]
fn min_state_fraction_holds_the_transition_until_the_state_has_played() {
    let (mut world, entity) = world_with_animator("");
    install_bent90(&mut world);
    install_machine(&mut world, speed_machine(0.0, 0.5));
    {
        let animator = world.get_mut::<Animator>(entity).unwrap();
        animator.state_machine_path = "hero.rasm".into();
        animator.playing = true;
    }
    let mut params = prism_anim::AnimParameters::default();
    params.set_float("speed", 1.0);
    world.insert_resource(params);

    // Only 0.2s played (<50%): condition true still doesn't transition.
    animator_system(&mut world, 0.2);
    assert_eq!(world.get::<Animator>(entity).unwrap().state, "idle");

    // Criterion uses **previous frame** playback time (that frame actually reached there), so frame crossing 50% is
    // still blocked, next frame allows it.
    animator_system(&mut world, 0.5);
    assert_eq!(world.get::<Animator>(entity).unwrap().state, "idle");

    animator_system(&mut world, 0.0);
    assert_eq!(world.get::<Animator>(entity).unwrap().state, "run");
}

#[test]
fn a_missing_state_machine_falls_back_to_the_component_clip() {
    let (mut world, entity) = world_with_animator("bend.ranim");
    world
        .get_mut::<Animator>(entity)
        .unwrap()
        .state_machine_path = "missing.rasm".into();
    assert_eq!(
        animator_system(&mut world, 0.0),
        1,
        "Bad path shouldn't make entity disappear"
    );
    let tip = child_tip(&world, entity);
    assert!(
        (tip.y - 1.0).abs() < 1e-4,
        "Falls back to clip_path, got {tip:?}"
    );
}

/// Switch entity to 2D blend space: `(0,0)` = bend (0°), `(1,0)` / `(0,1)` = bent90 (90°).
fn use_blend_space_2d(world: &mut World, entity: Entity, x: f32, y: f32) {
    let animator = world.get_mut::<Animator>(entity).unwrap();
    animator.blend_2d_clip_paths = vec![
        "bend.ranim".into(),
        "bent90.ranim".into(),
        "bent90.ranim".into(),
    ];
    animator.blend_2d_positions = vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
    animator.blend_parameter = x;
    animator.blend_parameter_y = y;
    animator.time = 0.0;
    animator.playing = false;
}

#[test]
fn a_two_dimensional_blend_space_interpolates_on_the_parameter_plane() {
    let (mut world, entity) = world_with_animator("");
    install_bent90(&mut world);

    // Origin sample = bend at 0s (0°).
    use_blend_space_2d(&mut world, entity, 0.0, 0.0);
    animator_system(&mut world, 0.0);
    assert_eq!(
        world.get::<AnimatorPlayhead>(entity).unwrap().clip_path,
        BLEND_SPACE_2D_KEY,
        "2D blend space uses its own playback key"
    );
    let tip = child_tip(&world, entity);
    assert!(
        (tip.y - 1.0).abs() < 1e-4,
        "Sample origin should be 0°, got {tip:?}"
    );

    // Midpoint along x axis: 0° and 90° half each → 45°.
    use_blend_space_2d(&mut world, entity, 0.5, 0.0);
    animator_system(&mut world, 0.0);
    let tip = child_tip(&world, entity);
    let angle = std::f32::consts::FRAC_PI_4;
    assert!(
        (tip.x - (2.0 - angle.sin())).abs() < 1e-3,
        "X-axis midpoint should be 45°, got {tip:?}"
    );

    // Edge between two 90° samples: still 90°.
    use_blend_space_2d(&mut world, entity, 0.5, 0.5);
    animator_system(&mut world, 0.0);
    let tip = child_tip(&world, entity);
    assert!(
        (tip.x - 1.0).abs() < 1e-3,
        "Opposite edge should be 90°, got {tip:?}"
    );
}

#[test]
fn a_two_dimensional_blend_space_needs_an_area_and_falls_back() {
    let (mut world, entity) = world_with_animator("");
    install_bent90(&mut world);

    // Only two samples: no triangle, falls back to single clip.
    {
        let animator = world.get_mut::<Animator>(entity).unwrap();
        animator.blend_2d_clip_paths = vec!["bend.ranim".into(), "bent90.ranim".into()];
        animator.blend_2d_positions = vec![[0.0, 0.0], [1.0, 0.0]];
        animator.playing = false;
    }
    animator_system(&mut world, 0.0);
    assert_eq!(
        world.get::<AnimatorPlayhead>(entity).unwrap().clip_path,
        "bend.ranim"
    );

    // Three collinear samples: triangulation fails, same fallback to single clip.
    {
        let animator = world.get_mut::<Animator>(entity).unwrap();
        animator.blend_2d_clip_paths = vec![
            "bend.ranim".into(),
            "bent90.ranim".into(),
            "bend.ranim".into(),
        ];
        animator.blend_2d_positions = vec![[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]];
    }
    animator_system(&mut world, 0.0);
    assert_eq!(
        world.get::<AnimatorPlayhead>(entity).unwrap().clip_path,
        "bend.ranim"
    );
}

/// Animation graph layer test: base clip stays 0°, graph layer is bent90 at 90°.
fn layered_setup() -> (World, Entity) {
    let (mut world, entity) = world_with_animator("");
    world
        .get_resource_mut::<SkeletonLibrary>()
        .unwrap()
        .insert_clip("bent90.ranim", bent_90_clip("bent90"));
    {
        let animator = world.get_mut::<Animator>(entity).unwrap();
        animator.playing = false;
        animator.time = 0.0;
    }
    (world, entity)
}

fn push_layer(world: &mut World, entity: Entity, layer: AnimLayer) {
    world
        .get_mut::<Animator>(entity)
        .unwrap()
        .layers
        .push(layer);
}

#[test]
fn an_animation_layer_blends_over_the_base_pose() {
    let (mut world, entity) = layered_setup();
    push_layer(
        &mut world,
        entity,
        AnimLayer {
            clip_path: "bent90.ranim".into(),
            weight: 1.0,
            ..Default::default()
        },
    );
    animator_system(&mut world, 0.0);
    let tip = child_tip(&world, entity);
    assert!(
        (tip.x - 1.0).abs() < 1e-3 && tip.y.abs() < 1e-3,
        "Full layer 90° should override 0° base pose, got {tip:?}"
    );

    // Weight 0.5: only rotate 45°.
    world.get_mut::<Animator>(entity).unwrap().layers[0].weight = 0.5;
    animator_system(&mut world, 0.0);
    let tip = child_tip(&world, entity);
    assert!(
        (tip.x - (2.0 - std::f32::consts::FRAC_PI_4.sin())).abs() < 1e-3,
        "Half weight should be 45°, got {tip:?}"
    );
}

#[test]
fn a_layer_bone_mask_limits_which_joints_move() {
    let (mut world, entity) = layered_setup();
    push_layer(
        &mut world,
        entity,
        AnimLayer {
            clip_path: "bent90.ranim".into(),
            weight: 1.0,
            bone_mask: vec!["root".into()],
            ..Default::default()
        },
    );
    animator_system(&mut world, 0.0);
    let tip = child_tip(&world, entity);
    assert!(
        (tip.y - 1.0).abs() < 1e-4 && (tip.x - 2.0).abs() < 1e-4,
        "Mask only root, child shouldn't rotate, got {tip:?}"
    );
}

#[test]
fn an_additive_layer_uses_the_reference_delta() {
    let (mut world, entity) = layered_setup();
    push_layer(
        &mut world,
        entity,
        AnimLayer {
            clip_path: "bent90.ranim".into(),
            weight: 1.0,
            additive: true,
            ..Default::default()
        },
    );
    animator_system(&mut world, 0.0);
    let tip = child_tip(&world, entity);
    assert!(
        (tip.x - 1.0).abs() < 1e-3 && tip.y.abs() < 1e-3,
        "Additive 90° should add to 0° base pose, got {tip:?}"
    );
}

#[test]
fn a_missing_or_zero_weight_layer_is_ignored() {
    let (mut world, entity) = layered_setup();
    push_layer(
        &mut world,
        entity,
        AnimLayer {
            clip_path: "missing.ranim".into(),
            weight: 1.0,
            ..Default::default()
        },
    );
    push_layer(
        &mut world,
        entity,
        AnimLayer {
            clip_path: "bent90.ranim".into(),
            weight: f32::NAN,
            ..Default::default()
        },
    );
    animator_system(&mut world, 0.0);
    let tip = child_tip(&world, entity);
    assert!(
        (tip.y - 1.0).abs() < 1e-4,
        "Invalid layer shouldn't change base pose, got {tip:?}"
    );
}

/// Single-clip animation graph: ClipPlayer -> Output.
fn single_clip_graph(path: &str) -> AnimGraphInfo {
    let clip = GraphNodeInfo {
        name: "clip".into(),
        kind: GraphNodeKind::ClipPlayer,
        looped: false,
        additive: false,
        speed: 1.0,
        weight: 1.0,
        clips: vec![path.into()],
        positions: Vec::new(),
        sample_speeds: Vec::new(),
        parameter_x: String::new(),
        parameter_y: String::new(),
        state_machine_path: String::new(),
        inputs: Vec::new(),
    };
    let mut output = GraphNodeInfo {
        name: "out".into(),
        kind: GraphNodeKind::Output,
        looped: false,
        additive: false,
        speed: 1.0,
        weight: 1.0,
        clips: Vec::new(),
        positions: Vec::new(),
        sample_speeds: Vec::new(),
        parameter_x: String::new(),
        parameter_y: String::new(),
        state_machine_path: String::new(),
        inputs: vec![GraphInputInfo {
            node: 0,
            weight: 1.0,
            bone_mask: Vec::new(),
        }],
    };
    output.name = "out".into();
    AnimGraphInfo {
        parameters: Vec::new(),
        nodes: vec![clip, output],
        output: 1,
    }
}

#[test]
fn an_animator_uses_its_graph_instead_of_the_clip_path() {
    let (mut world, entity) = world_with_animator("bend.ranim");
    world.insert_resource(AnimGraphLibrary::new());
    world
        .get_resource_mut::<AnimGraphLibrary>()
        .unwrap()
        .insert("hero.rag", single_clip_graph("bend.ranim"));
    {
        let animator = world.get_mut::<Animator>(entity).unwrap();
        animator.anim_graph_path = "hero.rag".into();
        animator.time = 0.0;
    }

    animator_system(&mut world, 0.5);
    let head = world.get::<AnimatorPlayhead>(entity).unwrap();
    assert_eq!(head.clip_path, ANIM_GRAPH_KEY);
    let tip = child_tip(&world, entity);
    assert!(
        (tip.x - (2.0 - std::f32::consts::FRAC_PI_4.sin())).abs() < 1e-3,
        "tip {tip:?}"
    );

    // Evaluator persists per entity, second half-second continues to 90°.
    animator_system(&mut world, 0.5);
    let tip = child_tip(&world, entity);
    assert!((tip.x - 1.0).abs() < 1e-3, "tip {tip:?}");
}

#[test]
fn graph_debug_snapshots_are_opt_in_and_track_node_time() {
    let (mut world, entity) = world_with_animator("bend.ranim");
    world.insert_resource(AnimGraphLibrary::new());
    world
        .get_resource_mut::<AnimGraphLibrary>()
        .unwrap()
        .insert("hero.rag", single_clip_graph("bend.ranim"));
    world.get_mut::<Animator>(entity).unwrap().anim_graph_path = "hero.rag".into();

    animator_system(&mut world, 0.25);
    assert!(
        world.get_resource::<AnimGraphDebug>().is_none(),
        "No snapshot by default"
    );

    world.insert_resource(AnimGraphDebug::default());
    animator_system(&mut world, 0.25);
    let debug = world.get_resource::<AnimGraphDebug>().unwrap();
    let states = debug.states(entity).expect("Graph entity has snapshot");
    assert_eq!(states.len(), 2);
    assert_eq!(states[0].name, "clip");
    assert_eq!(states[0].kind, GraphNodeKind::ClipPlayer);
    assert!(
        (states[0].time - 0.5).abs() < 1e-4,
        "time {}",
        states[0].time
    );
    assert_eq!(states[1].kind, GraphNodeKind::Output);
    assert!(
        states[0].state_name.is_empty(),
        "Non-state-machine node has no state name"
    );
}

#[test]
fn a_frozen_graph_animator_keeps_its_evaluator_and_resumes() {
    let (mut world, entity) = world_with_animator("bend.ranim");
    world.insert_resource(AnimGraphLibrary::new());
    world
        .get_resource_mut::<AnimGraphLibrary>()
        .unwrap()
        .insert("hero.rag", single_clip_graph("bend.ranim"));
    {
        let animator = world.get_mut::<Animator>(entity).unwrap();
        animator.anim_graph_path = "hero.rag".into();
        animator.lod_enabled = true;
    }
    let camera = add_camera(&mut world, Vec3::new(0.0, 0.0, 100.0));

    animator_system(&mut world, 0.25);
    assert!(
        world.get::<BonePalette>(entity).is_none(),
        "Graph entity not evaluated even when far away"
    );

    world.get_mut::<LocalTransform>(camera).unwrap().position = Vec3::new(0.0, 0.0, 5.0);
    animator_system(&mut world, 0.5);
    let head = world
        .get::<AnimatorPlayhead>(entity)
        .expect("Evaluation resumed");
    assert_eq!(head.clip_path, ANIM_GRAPH_KEY);
    // 0.25s during freeze not backfilled: after resume only advances this frame's 0.5s.
    let tip = child_tip(&world, entity);
    assert!(
        (tip.x - (2.0 - std::f32::consts::FRAC_PI_4.sin())).abs() < 1e-3,
        "tip {tip:?}"
    );
}

#[test]
fn a_throttled_graph_animator_keeps_its_playback_speed() {
    let (mut world, entity) = world_with_animator("bend.ranim");
    world.insert_resource(AnimGraphLibrary::new());
    world
        .get_resource_mut::<AnimGraphLibrary>()
        .unwrap()
        .insert("hero.rag", single_clip_graph("bend.ranim"));
    world.insert(
        entity,
        LocalTransform {
            position: Vec3::new(0.0, 0.0, 30.0),
            ..Default::default()
        },
    );
    {
        let animator = world.get_mut::<Animator>(entity).unwrap();
        animator.anim_graph_path = "hero.rag".into();
        animator.lod_enabled = true;
        animator.lod_near = 25.0;
        animator.lod_far = 50.0;
    }
    add_camera(&mut world, Vec3::ZERO);

    // Two frames of 0.1s: first frame 0.2s, second skipped, total still 0.2s.
    animator_system(&mut world, 0.1);
    animator_system(&mut world, 0.1);
    let tip = child_tip(&world, entity);
    let angle = 0.2 * std::f32::consts::FRAC_PI_2;
    assert!(
        (tip.x - (2.0 - angle.sin())).abs() < 1e-3,
        "tip {tip:?} should correspond to 0.2s"
    );
}

#[test]
fn a_missing_graph_falls_back_to_the_clip_path() {
    let (mut world, entity) = world_with_animator("bend.ranim");
    world.insert_resource(AnimGraphLibrary::new());
    world.get_mut::<Animator>(entity).unwrap().anim_graph_path = "missing.rag".into();
    animator_system(&mut world, 0.5);
    let head = world.get::<AnimatorPlayhead>(entity).unwrap();
    assert_ne!(
        head.clip_path, ANIM_GRAPH_KEY,
        "missing graph must fall back"
    );
    let tip = child_tip(&world, entity);
    assert!(
        (tip.x - (2.0 - std::f32::consts::FRAC_PI_4.sin())).abs() < 1e-3,
        "tip {tip:?}"
    );
}

#[test]
fn graph_notifies_reach_the_engine_event_queue() {
    let (mut world, entity) = world_with_animator("");
    let mut foot = bend_clip();
    foot.notifies.push(ClipNotify {
        time: 0.5,
        name: "Footstep.L".into(),
    });
    world
        .get_resource_mut::<SkeletonLibrary>()
        .unwrap()
        .insert_clip("foot.ranim", foot);
    world.insert_resource(AnimGraphLibrary::new());
    world
        .get_resource_mut::<AnimGraphLibrary>()
        .unwrap()
        .insert("hero.rag", single_clip_graph("foot.ranim"));
    {
        let animator = world.get_mut::<Animator>(entity).unwrap();
        animator.anim_graph_path = "hero.rag".into();
        animator.time = 0.0;
    }

    animator_system(&mut world, 0.6);
    let events = world.get_resource::<AnimEvents>().expect("events resource");
    assert_eq!(events.len(), 1, "{:?}", events.events);
    assert_eq!(events.events[0].name, "Footstep.L");
    assert!((events.events[0].time - 0.5).abs() < 1e-5);
}

#[test]
fn many_graph_entities_evaluate_independently() {
    let (mut world, first) = world_with_animator("");
    world.insert_resource(AnimGraphLibrary::new());
    world
        .get_resource_mut::<AnimGraphLibrary>()
        .unwrap()
        .insert("hero.rag", single_clip_graph("bend.ranim"));
    let mut entities = vec![first];
    for _ in 1..6 {
        let entity = world.spawn();
        world.insert(
            entity,
            SkeletonRoot {
                skeleton_path: "rig.rskel".into(),
                default_clip_path: "bend.ranim".into(),
            },
        );
        world.insert(
            entity,
            Animator {
                anim_graph_path: "hero.rag".into(),
                ..Default::default()
            },
        );
        entities.push(entity);
    }
    {
        let animator = world.get_mut::<Animator>(first).unwrap();
        animator.anim_graph_path = "hero.rag".into();
        animator.time = 0.0;
    }

    assert_eq!(
        animator_system(&mut world, 0.5),
        entities.len(),
        "Every entity should be updated"
    );
    let tips: Vec<Vec3> = entities.iter().map(|e| child_tip(&world, *e)).collect();
    for (entity, tip) in entities.iter().zip(&tips) {
        let head = world.get::<AnimatorPlayhead>(*entity).expect("playhead");
        assert_eq!(head.clip_path, ANIM_GRAPH_KEY);
        assert!(
            (tip.x - tips[0].x).abs() < 1e-4 && (tip.y - tips[0].y).abs() < 1e-4,
            "{entity:?} pose inconsistent with other entities: {tip:?} vs {:?}",
            tips[0]
        );
    }

    // Evaluator persists across frames: second frame continues to 90°, parallel get/put didn't lose state.
    assert_eq!(animator_system(&mut world, 0.5), entities.len());
    for entity in &entities {
        let tip = child_tip(&world, *entity);
        assert!((tip.x - 1.0).abs() < 1e-3, "{entity:?} tip {tip:?}");
    }
}

#[test]
fn graph_and_clip_entities_update_in_the_same_tick() {
    let (mut world, graph_entity) = world_with_animator("");
    let clip_entity = world.spawn();
    world.insert(
        clip_entity,
        SkeletonRoot {
            skeleton_path: "rig.rskel".into(),
            default_clip_path: "bend.ranim".into(),
        },
    );
    world.insert(
        clip_entity,
        Animator {
            clip_path: "bend.ranim".into(),
            ..Default::default()
        },
    );
    world.insert_resource(AnimGraphLibrary::new());
    world
        .get_resource_mut::<AnimGraphLibrary>()
        .unwrap()
        .insert("hero.rag", single_clip_graph("bend.ranim"));
    world
        .get_mut::<Animator>(graph_entity)
        .unwrap()
        .anim_graph_path = "hero.rag".into();

    assert_eq!(animator_system(&mut world, 0.0), 2);
    assert_eq!(
        world
            .get::<AnimatorPlayhead>(graph_entity)
            .unwrap()
            .clip_path,
        ANIM_GRAPH_KEY
    );
    assert_eq!(
        world
            .get::<AnimatorPlayhead>(clip_entity)
            .unwrap()
            .clip_path,
        "bend.ranim"
    );
}

/// Slot graph: ClipPlayer(base) -> Slot -> Output.
fn slot_graph(base: &str) -> AnimGraphInfo {
    let base_node = GraphNodeInfo {
        name: "base".into(),
        kind: GraphNodeKind::ClipPlayer,
        looped: false,
        additive: false,
        speed: 1.0,
        weight: 1.0,
        clips: vec![base.into()],
        positions: Vec::new(),
        sample_speeds: Vec::new(),
        parameter_x: String::new(),
        parameter_y: String::new(),
        state_machine_path: String::new(),
        inputs: Vec::new(),
    };
    let slot = GraphNodeInfo {
        name: "slot".into(),
        kind: GraphNodeKind::Slot,
        looped: false,
        additive: false,
        speed: 1.0,
        weight: 1.0,
        clips: Vec::new(),
        positions: Vec::new(),
        sample_speeds: Vec::new(),
        parameter_x: String::new(),
        parameter_y: String::new(),
        state_machine_path: String::new(),
        inputs: vec![GraphInputInfo {
            node: 0,
            weight: 1.0,
            bone_mask: Vec::new(),
        }],
    };
    let output = GraphNodeInfo {
        name: "out".into(),
        kind: GraphNodeKind::Output,
        looped: false,
        additive: false,
        speed: 1.0,
        weight: 1.0,
        clips: Vec::new(),
        positions: Vec::new(),
        sample_speeds: Vec::new(),
        parameter_x: String::new(),
        parameter_y: String::new(),
        state_machine_path: String::new(),
        inputs: vec![GraphInputInfo {
            node: 1,
            weight: 1.0,
            bone_mask: Vec::new(),
        }],
    };
    AnimGraphInfo {
        parameters: Vec::new(),
        nodes: vec![base_node, slot, output],
        output: 2,
    }
}

fn setup_slot_world() -> (World, Entity) {
    let (mut world, entity) = world_with_animator("");
    world.insert_resource(AnimGraphLibrary::new());
    world
        .get_resource_mut::<AnimGraphLibrary>()
        .unwrap()
        .insert("slot.rag", slot_graph("bend.ranim"));
    world.get_mut::<Animator>(entity).unwrap().anim_graph_path = "slot.rag".into();
    world.insert_resource(AnimMontageRequests::new());
    (world, entity)
}

fn montage_clip(notify: &str, time: f32) -> Clip {
    let mut clip = bend_clip();
    clip.name = "montage".into();
    clip.notifies.push(ClipNotify {
        time,
        name: notify.into(),
    });
    clip
}

#[test]
fn graph_debug_lists_montage_slots() {
    let (mut world, entity) = setup_slot_world();
    world
        .get_resource_mut::<SkeletonLibrary>()
        .unwrap()
        .insert_clip("montage.ranim", montage_clip("MontageStep", 0.5));
    world.insert_resource(AnimGraphDebug::default());
    world
        .get_resource_mut::<AnimMontageRequests>()
        .unwrap()
        .push(MontageRequest::play(entity, "slot", "montage.ranim"));

    animator_system(&mut world, 0.2);
    let debug = world.get_resource::<AnimGraphDebug>().unwrap();
    let montages = debug.montages(entity).expect("Graph entity");
    assert_eq!(montages.len(), 1);
    assert_eq!(montages[0].slot, "slot");
    assert_eq!(montages[0].clip.as_deref(), Some("montage"));
    assert!((montages[0].time.unwrap() - 0.2).abs() < 1e-5);
    assert!(montages[0].playing && !montages[0].paused);

    world
        .get_resource_mut::<AnimMontageRequests>()
        .unwrap()
        .push(MontageRequest::pause(entity, "slot"));
    animator_system(&mut world, 0.2);
    let debug = world.get_resource::<AnimGraphDebug>().unwrap();
    assert!(debug.montages(entity).unwrap()[0].paused);
}

#[test]
fn a_pause_request_freezes_the_montage_until_resumed() {
    let (mut world, entity) = setup_slot_world();
    world
        .get_resource_mut::<SkeletonLibrary>()
        .unwrap()
        .insert_clip("montage.ranim", montage_clip("MontageStep", 0.5));
    world
        .get_resource_mut::<AnimMontageRequests>()
        .unwrap()
        .push(MontageRequest::play(entity, "slot", "montage.ranim"));
    animator_system(&mut world, 0.2);
    assert!(world.get_resource::<AnimEvents>().unwrap().is_empty());

    world
        .get_resource_mut::<AnimMontageRequests>()
        .unwrap()
        .push(MontageRequest::pause(entity, "slot"));
    animator_system(&mut world, 0.5);
    assert!(
        world.get_resource::<AnimEvents>().unwrap().is_empty(),
        "No advance or Notify while paused"
    );

    world
        .get_resource_mut::<AnimMontageRequests>()
        .unwrap()
        .push(MontageRequest::resume(entity, "slot"));
    animator_system(&mut world, 0.5);
    assert_eq!(
        world
            .get_resource::<AnimEvents>()
            .unwrap()
            .names_for(entity)
            .collect::<Vec<_>>(),
        ["MontageStep"],
        "Advances to Notify after resume"
    );
}

#[test]
fn a_montage_request_carries_blend_in_and_blend_out() {
    let (mut world, entity) = setup_slot_world();
    world
        .get_resource_mut::<SkeletonLibrary>()
        .unwrap()
        .insert_clip("montage.ranim", montage_clip("MontageStep", 0.5));
    world.insert_resource(AnimGraphDebug::default());
    world
        .get_resource_mut::<AnimMontageRequests>()
        .unwrap()
        .push(
            MontageRequest::play(entity, "slot", "montage.ranim")
                .with_looping(false)
                .with_blend_in(0.2)
                .with_blend_out(0.4),
        );

    animator_system(&mut world, 0.1);
    let debug = world.get_resource::<AnimGraphDebug>().unwrap();
    assert!((debug.montages(entity).unwrap()[0].weight - 0.5).abs() < 1e-5);

    // 0.1 → 0.8: fade-in already full, remaining 0.2s / blend_out 0.4s → 0.5 weight.
    animator_system(&mut world, 0.7);
    let debug = world.get_resource::<AnimGraphDebug>().unwrap();
    assert!((debug.montages(entity).unwrap()[0].weight - 0.5).abs() < 1e-5);
}

#[test]
fn montage_debug_reports_the_active_warp_window() {
    let (mut world, entity) = setup_slot_world();
    let mut clip = montage_clip("MontageStep", 0.5);
    clip.warp_windows = vec![prism_anim::ClipWarpWindow {
        name: "VaultWarp".into(),
        start: 0.3,
        end: 0.8,
    }];
    world
        .get_resource_mut::<SkeletonLibrary>()
        .unwrap()
        .insert_clip("montage.ranim", clip);
    world.insert_resource(AnimGraphDebug::default());
    world
        .get_resource_mut::<AnimMontageRequests>()
        .unwrap()
        .push(MontageRequest::play(entity, "slot", "montage.ranim"));

    animator_system(&mut world, 0.5);
    let debug = world.get_resource::<AnimGraphDebug>().unwrap();
    let montage = &debug.montages(entity).expect("Graph entity")[0];
    assert_eq!(montage.warp_window.as_deref(), Some("VaultWarp"));
    assert!((montage.warp_window_progress.unwrap() - 0.4).abs() < 1e-5);
}

#[test]
fn a_montage_request_can_start_at_a_named_section() {
    let (mut world, entity) = setup_slot_world();
    let mut clip = montage_clip("MontageStep", 0.5);
    clip.sections = vec![
        ClipSection {
            name: "Start".into(),
            start: 0.0,
            end: 0.3,
        },
        ClipSection {
            name: "Impact".into(),
            start: 0.6,
            end: 1.0,
        },
    ];
    world
        .get_resource_mut::<SkeletonLibrary>()
        .unwrap()
        .insert_clip("montage.ranim", clip);
    world.insert_resource(AnimGraphDebug::default());
    world
        .get_resource_mut::<AnimMontageRequests>()
        .unwrap()
        .play_section(entity, "slot", "montage.ranim", "Impact");

    animator_system(&mut world, 0.1);
    let debug = world.get_resource::<AnimGraphDebug>().unwrap();
    let montage = &debug.montages(entity).expect("Graph entity")[0];
    assert!((montage.time.unwrap() - 0.7).abs() < 1e-5);
    assert_eq!(montage.section.as_deref(), Some("Impact"));
}

#[test]
fn a_montage_request_plays_on_the_named_slot_and_emits_notifies() {
    let (mut world, entity) = setup_slot_world();
    world
        .get_resource_mut::<SkeletonLibrary>()
        .unwrap()
        .insert_clip("montage.ranim", montage_clip("MontageStep", 0.5));
    world
        .get_resource_mut::<AnimMontageRequests>()
        .unwrap()
        .push(MontageRequest::play(entity, "slot", "montage.ranim"));

    animator_system(&mut world, 0.6);
    let events = world.get_resource::<AnimEvents>().expect("events");
    assert_eq!(
        events.names_for(entity).collect::<Vec<_>>(),
        ["MontageStep"]
    );
    assert_eq!(
        world
            .get_resource::<AnimMontageRequests>()
            .expect("requests")
            .len(),
        0,
        "the system consumes requests"
    );
}

#[test]
fn a_stop_request_clears_the_slot_montage() {
    let (mut world, entity) = setup_slot_world();
    world
        .get_resource_mut::<SkeletonLibrary>()
        .unwrap()
        .insert_clip("montage.ranim", montage_clip("MontageStep", 0.5));
    {
        let requests = world.get_resource_mut::<AnimMontageRequests>().unwrap();
        requests.push(MontageRequest::play(entity, "slot", "montage.ranim").with_looping(true));
    }
    animator_system(&mut world, 0.1);
    {
        let requests = world.get_resource_mut::<AnimMontageRequests>().unwrap();
        requests.stop(entity, "slot");
    }
    animator_system(&mut world, 0.6);
    let events = world.get_resource::<AnimEvents>().expect("events");
    assert!(events.is_empty(), "{:?}", events.events);
}

#[test]
fn an_enqueue_request_waits_for_the_current_montage_to_finish() {
    let (mut world, entity) = setup_slot_world();
    let library = world.get_resource_mut::<SkeletonLibrary>().unwrap();
    library.insert_clip("a.ranim", bend_clip());
    library.insert_clip("b.ranim", montage_clip("SecondStep", 0.1));
    world
        .get_resource_mut::<AnimMontageRequests>()
        .unwrap()
        .push(MontageRequest::play(entity, "slot", "a.ranim").with_looping(false));
    world
        .get_resource_mut::<AnimMontageRequests>()
        .unwrap()
        .push(MontageRequest::enqueue(entity, "slot", "b.ranim"));

    animator_system(&mut world, 0.6);
    animator_system(&mut world, 0.6);
    let events = world.get_resource::<AnimEvents>().expect("events");
    assert!(
        events.is_empty(),
        "B starts at time 0; its 0.1s notify has not been crossed: {:?}",
        events.events
    );
    animator_system(&mut world, 0.2);
    let events = world.get_resource::<AnimEvents>().expect("events");
    assert_eq!(events.names_for(entity).collect::<Vec<_>>(), ["SecondStep"]);
}

#[test]
fn a_montage_request_with_an_unloaded_clip_is_ignored() {
    let (mut world, entity) = setup_slot_world();
    world
        .get_resource_mut::<AnimMontageRequests>()
        .unwrap()
        .play(entity, "slot", "missing.ranim");
    animator_system(&mut world, 0.6);
    let head = world.get::<AnimatorPlayhead>(entity).expect("playhead");
    assert_eq!(head.clip_path, ANIM_GRAPH_KEY);
}
