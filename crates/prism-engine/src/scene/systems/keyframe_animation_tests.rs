//! `keyframe_animation_system` tests.

use super::*;
use crate::scene::components::KeyframeAnimation;
use prism_scene::animation::{AnimationEvent, Vec3Key};

fn clip(position: &[(f32, [f32; 3])], euler: &[(f32, [f32; 3])]) -> KeyframeAnimation {
    KeyframeAnimation {
        position: position
            .iter()
            .map(|(time, value)| Vec3Key::new(*time, *value))
            .collect(),
        euler_xyz: euler
            .iter()
            .map(|(time, value)| Vec3Key::new(*time, *value))
            .collect(),
        ..KeyframeAnimation::default()
    }
}

fn spawn(world: &mut World, animation: KeyframeAnimation) -> Entity {
    let entity = world.spawn();
    world.insert(entity, LocalTransform::default());
    world.insert(entity, animation);
    entity
}

fn transform_of(world: &World, entity: Entity) -> LocalTransform {
    world.get::<LocalTransform>(entity).unwrap().clone()
}

#[test]
fn playback_writes_the_sampled_position_into_the_transform() {
    let mut world = World::new();
    let entity = spawn(
        &mut world,
        clip(&[(0.0, [0.0; 3]), (2.0, [4.0, 0.0, 0.0])], &[]),
    );

    keyframe_animation_system(&mut world, 1.0);
    assert!((transform_of(&world, entity).position.x - 2.0).abs() < 1e-5);
}

#[test]
fn a_paused_clip_keeps_the_current_pose() {
    let mut world = World::new();
    let entity = spawn(
        &mut world,
        KeyframeAnimation {
            playing: false,
            ..clip(&[(0.0, [1.0, 0.0, 0.0]), (4.0, [5.0, 0.0, 0.0])], &[])
        },
    );

    for _ in 0..10 {
        keyframe_animation_system(&mut world, 1.0);
    }
    assert!((transform_of(&world, entity).position.x - 1.0).abs() < 1e-5);
}

#[test]
fn a_looped_clip_wraps_the_playhead() {
    let mut world = World::new();
    let entity = spawn(
        &mut world,
        clip(&[(0.0, [0.0; 3]), (1.0, [10.0, 0.0, 0.0])], &[]),
    );

    // 1.5 seconds → one full loop + half.
    keyframe_animation_system(&mut world, 1.5);
    let head = *world.get::<ClipPlayhead>(entity).unwrap();
    assert!(head.time > 1.5 - 1e-4);
    assert!(!head.finished);
    assert!((transform_of(&world, entity).position.x - 5.0).abs() < 1e-4);
}

#[test]
fn a_single_shot_clip_holds_the_last_key_and_reports_finished() {
    let mut world = World::new();
    let entity = spawn(
        &mut world,
        KeyframeAnimation {
            looped: false,
            ..clip(&[(0.0, [0.0; 3]), (1.0, [10.0, 0.0, 0.0])], &[])
        },
    );

    for _ in 0..5 {
        keyframe_animation_system(&mut world, 1.0);
    }
    let head = *world.get::<ClipPlayhead>(entity).unwrap();
    assert!(head.finished);
    assert!((head.time - 1.0).abs() < 1e-5, "playhead clamps at the end");
    assert!((transform_of(&world, entity).position.x - 10.0).abs() < 1e-5);
}

#[test]
fn speed_scales_the_playhead() {
    let mut world = World::new();
    let entity = spawn(
        &mut world,
        KeyframeAnimation {
            speed: 2.0,
            phase: 0.5,
            ..clip(&[(0.0, [0.0; 3]), (2.0, [2.0, 0.0, 0.0])], &[])
        },
    );

    // dt 0.25 × speed 2 = 0.5 seconds, plus phase 0.5 → sample 1.0 seconds = intermediate value.
    keyframe_animation_system(&mut world, 0.25);
    assert!((transform_of(&world, entity).position.x - 1.0).abs() < 1e-4);
}

#[test]
fn channels_without_keys_keep_the_authored_transform() {
    let mut world = World::new();
    let entity = spawn(
        &mut world,
        clip(&[(0.0, [3.0, 0.0, 0.0]), (1.0, [3.0, 0.0, 0.0])], &[]),
    );
    {
        let transform = world.get_mut::<LocalTransform>(entity).unwrap();
        transform.position = glam::Vec3::new(-1.0, 0.0, 0.0);
        transform.scale = glam::Vec3::new(2.0, 3.0, 4.0);
    }

    keyframe_animation_system(&mut world, 0.5);
    let transform = transform_of(&world, entity);
    // Position taken over by track...
    assert!((transform.position.x - 3.0).abs() < 1e-5);
    // ...rotation / scale have no key frames, keep author value.
    assert_eq!(transform.rotation, glam::Quat::IDENTITY);
    assert_eq!(transform.scale, glam::Vec3::new(2.0, 3.0, 4.0));
}

#[test]
fn euler_keys_are_degrees_and_rotate_the_transform() {
    let mut world = World::new();
    // Looping clip wraps to 0 at `time == duration`, single play used to check last frame.
    let entity = spawn(
        &mut world,
        KeyframeAnimation {
            looped: false,
            ..clip(&[], &[(0.0, [0.0; 3]), (1.0, [0.0, 90.0, 0.0])])
        },
    );

    keyframe_animation_system(&mut world, 1.0);
    let rotation = transform_of(&world, entity).rotation;
    let forward = rotation * glam::Vec3::Z;
    // +90° around Y: +Z becomes +X.
    assert!((forward.x - 1.0).abs() < 1e-5);
    assert!(forward.z.abs() < 1e-5);
}

#[test]
fn an_empty_clip_attaches_no_playhead() {
    let mut world = World::new();
    let entity = spawn(&mut world, KeyframeAnimation::default());

    keyframe_animation_system(&mut world, 1.0);
    assert!(world.get::<ClipPlayhead>(entity).is_none());
    let transform = transform_of(&world, entity);
    assert_eq!(transform.position, glam::Vec3::ZERO);
    assert_eq!(transform.rotation, glam::Quat::IDENTITY);
    assert_eq!(transform.scale, glam::Vec3::ONE);
}

#[test]
fn an_entity_without_a_transform_is_left_alone() {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, clip(&[(0.0, [0.0; 3]), (1.0, [1.0; 3])], &[]));

    keyframe_animation_system(&mut world, 1.0);
    assert!(world.get::<ClipPlayhead>(entity).is_none());
}

fn fired(world: &World) -> Vec<(String, f32)> {
    world
        .get_resource::<AnimationEventQueue>()
        .map(|queue| {
            queue
                .events
                .iter()
                .map(|event| (event.name.clone(), event.time))
                .collect()
        })
        .unwrap_or_default()
}

/// #490: enqueue when playhead crosses event time; frames that do not cross emit nothing.
#[test]
fn events_fire_when_the_playhead_crosses_them() {
    let mut world = World::new();
    let entity = spawn(
        &mut world,
        KeyframeAnimation {
            looped: false,
            events: vec![
                AnimationEvent::new(0.5, "step"),
                AnimationEvent::new(1.5, "land"),
            ],
            ..clip(&[(0.0, [0.0; 3]), (2.0, [10.0; 3])], &[])
        },
    );

    keyframe_animation_system(&mut world, 0.4);
    assert!(fired(&world).is_empty());

    keyframe_animation_system(&mut world, 0.2);
    assert_eq!(fired(&world), vec![("step".to_string(), 0.5)]);
    assert_eq!(
        world.get_resource::<AnimationEventQueue>().unwrap().events[0].entity,
        entity
    );

    keyframe_animation_system(&mut world, 0.8);
    assert!(fired(&world).is_empty(), "1.5 has not been crossed yet");

    keyframe_animation_system(&mut world, 0.3);
    assert_eq!(fired(&world), vec![("land".to_string(), 1.5)]);

    // No re-fire after clamping to last frame.
    keyframe_animation_system(&mut world, 0.5);
    assert!(fired(&world).is_empty());
}

#[test]
fn a_zero_time_event_fires_on_the_first_advance() {
    let mut world = World::new();
    spawn(
        &mut world,
        KeyframeAnimation {
            looped: false,
            events: vec![AnimationEvent::new(0.0, "start")],
            ..clip(&[(0.0, [0.0; 3]), (1.0, [1.0; 3])], &[])
        },
    );

    keyframe_animation_system(&mut world, 0.1);
    assert_eq!(fired(&world), vec![("start".to_string(), 0.0)]);
}

#[test]
fn a_looped_clip_fires_events_once_per_cycle() {
    let mut world = World::new();
    spawn(
        &mut world,
        KeyframeAnimation {
            events: vec![AnimationEvent::new(0.25, "tick")],
            ..clip(&[(0.0, [0.0; 3]), (1.0, [10.0; 3])], &[])
        },
    );

    keyframe_animation_system(&mut world, 0.3);
    assert_eq!(fired(&world), vec![("tick".to_string(), 0.25)]);
    keyframe_animation_system(&mut world, 0.3);
    assert!(fired(&world).is_empty());
    keyframe_animation_system(&mut world, 0.3);
    assert!(fired(&world).is_empty());
    // 0.9 → 1.2: wraps to 0.2, not yet 0.25.
    keyframe_animation_system(&mut world, 0.3);
    assert!(fired(&world).is_empty());
    // 0.2 → 0.5: second loop tick.
    keyframe_animation_system(&mut world, 0.3);
    assert_eq!(fired(&world), vec![("tick".to_string(), 0.25)]);
}

#[test]
fn paused_clips_do_not_fire_events_and_the_queue_is_cleared() {
    let mut world = World::new();
    spawn(
        &mut world,
        KeyframeAnimation {
            playing: false,
            events: vec![AnimationEvent::new(0.1, "never")],
            ..clip(&[(0.0, [0.0; 3]), (1.0, [1.0; 3])], &[])
        },
    );

    keyframe_animation_system(&mut world, 1.0);
    assert!(fired(&world).is_empty());

    // Event present this frame, absent next frame must also be cleared.
    let stale_entity = world.spawn();
    world.insert_resource(AnimationEventQueue {
        events: vec![FiredAnimationEvent {
            entity: stale_entity,
            name: "stale".to_string(),
            time: 99.0,
        }],
    });
    keyframe_animation_system(&mut world, 1.0);
    assert!(fired(&world).is_empty());
}

/// #490: when playing backwards, crossing event time downward must also enqueue.
#[test]
fn reverse_playback_fires_events_when_the_playhead_crosses_backwards() {
    let mut world = World::new();
    let entity = spawn(
        &mut world,
        KeyframeAnimation {
            looped: false,
            speed: -1.0,
            events: vec![AnimationEvent::new(0.5, "back")],
            ..clip(&[(0.0, [0.0; 3]), (1.0, [10.0; 3])], &[])
        },
    );
    world.insert(
        entity,
        ClipPlayhead {
            time: 0.9,
            finished: false,
        },
    );

    // 0.9 → 0.7: not yet 0.5.
    keyframe_animation_system(&mut world, 0.2);
    assert!(fired(&world).is_empty());
    // 0.7 → 0.4: crossing 0.5 downward.
    keyframe_animation_system(&mut world, 0.3);
    assert_eq!(fired(&world), vec![("back".to_string(), 0.5)]);
}

#[test]
fn reverse_looped_playback_fires_events_after_wrapping_past_zero() {
    let mut world = World::new();
    let entity = spawn(
        &mut world,
        KeyframeAnimation {
            looped: true,
            speed: -1.0,
            events: vec![AnimationEvent::new(0.8, "tail")],
            ..clip(&[(0.0, [0.0; 3]), (1.0, [10.0; 3])], &[])
        },
    );
    world.insert(
        entity,
        ClipPlayhead {
            time: 0.1,
            finished: false,
        },
    );

    // 0.1 → -0.2 wraps to 0.8: reverse crossing 0 continues from end, hits event at 0.8.
    keyframe_animation_system(&mut world, 0.3);
    assert_eq!(fired(&world), vec![("tail".to_string(), 0.8)]);
}

#[test]
fn a_clip_with_only_events_still_advances_and_fires() {
    let mut world = World::new();
    spawn(
        &mut world,
        KeyframeAnimation {
            looped: false,
            events: vec![AnimationEvent::new(0.2, "cue")],
            position: Vec::new(),
            euler_xyz: Vec::new(),
            scale: Vec::new(),
            ..KeyframeAnimation::default()
        },
    );

    keyframe_animation_system(&mut world, 0.3);
    assert_eq!(fired(&world), vec![("cue".to_string(), 0.2)]);
}
