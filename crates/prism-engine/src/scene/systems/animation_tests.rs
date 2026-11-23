// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

use super::*;
use crate::scene::components::SceneAssetId;
use prism_ecs::World;

fn spawn(world: &mut World, animation: SpriteAnimation, texture: [u32; 2]) -> prism_ecs::Entity {
    let entity = world.spawn();
    world.insert(
        entity,
        SpriteRenderer {
            sprite_path: "sprites/sheet.png".into(),
            frame_size: [16, 16],
            ..SpriteRenderer::default()
        },
    );
    world.insert(entity, animation);
    world.insert(
        entity,
        SpriteRef::new(SceneAssetId::generate(), 1, texture, 1),
    );
    entity
}

fn frame_of(world: &World, entity: prism_ecs::Entity) -> u32 {
    world.get::<SpriteRenderer>(entity).unwrap().frame_index
}

#[test]
fn frame_index_advances_with_time() {
    let mut world = World::new();
    let entity = spawn(
        &mut world,
        SpriteAnimation {
            frames: 4,
            fps: 10.0,
            ..SpriteAnimation::default()
        },
        [64, 16],
    );

    sprite_animation_system(&mut world, 0.05);
    assert_eq!(frame_of(&world, entity), 0);
    sprite_animation_system(&mut world, 0.1); // t = 0.15s -> frame 1
    assert_eq!(frame_of(&world, entity), 1);
}

#[test]
fn looping_wraps_back_to_the_first_frame() {
    let mut world = World::new();
    let entity = spawn(
        &mut world,
        SpriteAnimation {
            frames: 4,
            fps: 10.0,
            looped: true,
            ..SpriteAnimation::default()
        },
        [64, 16],
    );

    // 0.4s = exactly one cycle.
    sprite_animation_system(&mut world, 0.4);
    assert_eq!(frame_of(&world, entity), 0);
    sprite_animation_system(&mut world, 0.05);
    assert_eq!(frame_of(&world, entity), 0);
    sprite_animation_system(&mut world, 0.05); // 0.5s -> frame 1
    assert_eq!(frame_of(&world, entity), 1);
}

#[test]
fn non_looping_stops_on_the_last_frame() {
    let mut world = World::new();
    let entity = spawn(
        &mut world,
        SpriteAnimation {
            frames: 3,
            fps: 10.0,
            looped: false,
            ..SpriteAnimation::default()
        },
        [48, 16],
    );

    sprite_animation_system(&mut world, 5.0);
    assert_eq!(frame_of(&world, entity), 2);
}

#[test]
fn paused_animation_keeps_its_frame() {
    let mut world = World::new();
    let entity = spawn(
        &mut world,
        SpriteAnimation {
            frames: 4,
            fps: 10.0,
            playing: false,
            ..SpriteAnimation::default()
        },
        [64, 16],
    );

    sprite_animation_system(&mut world, 1.0);
    assert_eq!(frame_of(&world, entity), 0);
}

#[test]
fn frame_count_is_derived_from_the_texture_width() {
    let mut world = World::new();
    // 96 px wide sheet with 16 px frames -> 6 frames.
    let entity = spawn(
        &mut world,
        SpriteAnimation {
            frames: 0,
            fps: 10.0,
            ..SpriteAnimation::default()
        },
        [96, 16],
    );

    sprite_animation_system(&mut world, 0.55); // frame 5
    assert_eq!(frame_of(&world, entity), 5);
    sprite_animation_system(&mut world, 0.1); // 0.65 -> wraps to 0
    assert_eq!(frame_of(&world, entity), 0);
}

#[test]
fn one_shot_restarts_when_playing_is_re_enabled() {
    let mut world = World::new();
    let entity = spawn(
        &mut world,
        SpriteAnimation {
            frames: 3,
            fps: 10.0,
            looped: false,
            ..SpriteAnimation::default()
        },
        [48, 16],
    );

    sprite_animation_system(&mut world, 5.0);
    assert_eq!(
        frame_of(&world, entity),
        2,
        "One-shot animation stays at last frame"
    );

    world.get_mut::<SpriteAnimation>(entity).unwrap().playing = false;
    sprite_animation_system(&mut world, 5.0);
    assert_eq!(
        frame_of(&world, entity),
        2,
        "Holds current frame when paused"
    );

    world.get_mut::<SpriteAnimation>(entity).unwrap().playing = true;
    sprite_animation_system(&mut world, 0.05);
    assert_eq!(frame_of(&world, entity), 0, "Replay starts from frame 0");
    sprite_animation_system(&mut world, 0.1);
    assert_eq!(frame_of(&world, entity), 1);
}

#[test]
fn frame_start_offsets_the_written_frame_within_the_row() {
    // #584: same row columns 2..3 as independent action, plays 2 frames and loops.
    let mut world = World::new();
    let entity = spawn(
        &mut world,
        SpriteAnimation {
            frames: 2,
            frame_start: 2,
            fps: 10.0,
            ..SpriteAnimation::default()
        },
        [64, 16],
    );

    sprite_animation_system(&mut world, 0.0);
    assert_eq!(frame_of(&world, entity), 2);
    sprite_animation_system(&mut world, 0.15); // t = 0.15 -> frame 2
    assert_eq!(frame_of(&world, entity), 3);
    sprite_animation_system(&mut world, 0.1); // t = 0.25 -> loop back to frame_start
    assert_eq!(frame_of(&world, entity), 2);
}

#[test]
fn derived_frames_stop_at_the_selected_rows_end() {
    let mut world = World::new();
    let entity = spawn(
        &mut world,
        SpriteAnimation {
            frames: 0,
            frame_start: 2,
            fps: 10.0,
            ..SpriteAnimation::default()
        },
        [64, 16],
    );

    // 4 columns, starting from column 2 -> only 2 frames left.
    sprite_animation_system(&mut world, 0.05);
    assert_eq!(frame_of(&world, entity), 2);
    sprite_animation_system(&mut world, 0.1);
    assert_eq!(frame_of(&world, entity), 3);
    sprite_animation_system(&mut world, 0.1);
    assert_eq!(frame_of(&world, entity), 2);
}

#[test]
fn phase_offsets_the_start_frame() {
    let mut world = World::new();
    let entity = spawn(
        &mut world,
        SpriteAnimation {
            frames: 4,
            fps: 10.0,
            phase: 0.2,
            ..SpriteAnimation::default()
        },
        [64, 16],
    );

    // phase 0.2s at 10 fps -> frame 2 immediately.
    sprite_animation_system(&mut world, 0.0);
    assert_eq!(frame_of(&world, entity), 2);
}
