//! Gameplay regression tests (host backend, no Vulkan / window needed).
//!
//! These tests run the **same** `game::*::tick` code as in Play, just swapping host from
//! "editor process" to a plain `prism_ecs::World`: `GameWorld::host` directly borrows the world,
//! so asserted component state is what module writes through ABI during Play.
//!
//! Run (in project dir): `cargo test`
//!
//! `world::tick` reads `assets/data/lake_layers.json`, uses `prism_engine::asset_paths`
//! to search upward from cwd for `assets/`, so must run under `projects/demos/fishing`.

use prism_game_api::{keys, EntityHandle, GameWorld, InputSnapshot};
use prism_scene::components::{Camera, LineRenderer, LocalTransform, Name, SpriteRenderer};

/// Build a minimal world: boat + bobber + camera (same entity names as `lake.scene.json`).
fn scene_world() -> prism_ecs::World {
    let mut world = prism_ecs::World::new();
    let camera = world.spawn();
    world.insert(camera, Camera::default());

    let boat = world.spawn();
    world.insert(boat, Name("boat".to_string()));
    world.insert(boat, LocalTransform::default());

    let bobber = world.spawn();
    world.insert(bobber, Name("bobber".to_string()));
    let mut transform = LocalTransform::default();
    transform.position.x = 44.0;
    transform.position.y = 24.0;
    world.insert(bobber, transform);
    world.insert(bobber, SpriteRenderer::default());

    world
}

fn click() -> InputSnapshot {
    InputSnapshot {
        left_clicked: true,
        ..InputSnapshot::default()
    }
}

/// All 8 texture layers spawned, each with transform and texture path.
///
/// This assertion watches the bug where `prism-runtime-host` once dropped `LocalTransform`:
/// texture layer created but no transform, screen would be empty.
#[test]
fn lake_world_spawns_every_layer_with_transform() {
    let mut world = scene_world();
    let mut api = GameWorld::host(&mut world);
    let mut state = None;
    fishing::game::world::tick(&mut state, &mut api, 0.016);

    let lake = state.expect("lake world should be created on the first tick");
    assert_eq!(
        lake.layer_count(),
        8,
        "expected the 8 layers from assets/data/lake_layers.json (cwd must be projects/demos/fishing)"
    );

    let layers: Vec<EntityHandle> = api.query::<SpriteRenderer>();
    let background: Vec<EntityHandle> = layers
        .into_iter()
        .filter(|entity| {
            api.get::<SpriteRenderer>(*entity)
                .map(|sprite| sprite.sprite_path.starts_with("sprites/bg/"))
                .unwrap_or(false)
        })
        .collect();
    assert!(
        background.len() >= 8,
        "every background layer needs a SpriteRenderer, got {}",
        background.len()
    );
    for entity in background {
        assert!(
            api.get::<LocalTransform>(entity).is_some(),
            "layer entity without LocalTransform never renders"
        );
    }
}

/// Cast → splash → bite → reel → catch a fish.
#[test]
fn fishing_state_machine_runs_cast_to_catch() {
    use fishing::game::fishing::Phase;

    let mut world = scene_world();
    let mut api = GameWorld::host(&mut world);
    let mut state = None;

    api.set_input(InputSnapshot::default());
    fishing::game::fishing::tick(&mut state, &mut api, 0.016);
    let state_ref = state.as_ref().expect("state created");
    assert_eq!(state_ref.phase(), Phase::Idle);
    assert_eq!(state_ref.caught(), 0);

    api.set_input(click());
    fishing::game::fishing::tick(&mut state, &mut api, 0.016);
    assert_eq!(state.as_ref().unwrap().phase(), Phase::Casting);

    api.set_input(InputSnapshot::default());
    fishing::game::fishing::tick(&mut state, &mut api, 0.6);
    assert_eq!(state.as_ref().unwrap().phase(), Phase::Waiting);

    let mut phase = Phase::Waiting;
    for _ in 0..300 {
        fishing::game::fishing::tick(&mut state, &mut api, 0.016);
        phase = state.as_ref().unwrap().phase();
        if phase == Phase::Biting {
            break;
        }
    }
    assert_eq!(phase, Phase::Biting, "bite should arrive within ~5s");

    api.set_input(click());
    fishing::game::fishing::tick(&mut state, &mut api, 0.016);
    assert_eq!(state.as_ref().unwrap().phase(), Phase::Reeling);

    api.set_input(InputSnapshot::default());
    for _ in 0..120 {
        fishing::game::fishing::tick(&mut state, &mut api, 0.016);
        if state.as_ref().unwrap().phase() == Phase::Caught {
            break;
        }
    }
    let state_ref = state.as_ref().unwrap();
    assert_eq!(state_ref.phase(), Phase::Caught);
    assert_eq!(state_ref.caught(), 1);
}

/// Holding Space counts as one input: cast and hook-set are press edges, not held state.
#[test]
fn holding_space_does_not_repeat_the_cast_or_set_the_hook() {
    use fishing::game::fishing::Phase;

    /// Space held down, without a fresh press edge this frame.
    fn space_held() -> InputSnapshot {
        let mut snapshot = InputSnapshot::default();
        snapshot.set_key(keys::SPACE, true);
        snapshot
    }

    /// Space held down **and** pressed this frame.
    fn space_pressed() -> InputSnapshot {
        let mut snapshot = space_held();
        snapshot.set_key_edge(keys::SPACE, true);
        snapshot
    }

    let mut world = scene_world();
    let mut api = GameWorld::host(&mut world);
    let mut state = None;
    fishing::game::fishing::tick(&mut state, &mut api, 0.016);

    api.set_input(space_held());
    for _ in 0..10 {
        fishing::game::fishing::tick(&mut state, &mut api, 0.016);
    }
    assert_eq!(
        state.as_ref().unwrap().phase(),
        Phase::Idle,
        "a held Space must not start a cast"
    );

    api.set_input(space_pressed());
    fishing::game::fishing::tick(&mut state, &mut api, 0.016);
    assert_eq!(state.as_ref().unwrap().phase(), Phase::Casting);

    api.set_input(space_held());
    fishing::game::fishing::tick(&mut state, &mut api, 0.6);
    assert_eq!(state.as_ref().unwrap().phase(), Phase::Waiting);

    let mut phase = Phase::Waiting;
    for _ in 0..300 {
        fishing::game::fishing::tick(&mut state, &mut api, 0.016);
        phase = state.as_ref().unwrap().phase();
        if phase == Phase::Biting {
            break;
        }
    }
    assert_eq!(phase, Phase::Biting, "bite should arrive within ~5s");

    for _ in 0..200 {
        fishing::game::fishing::tick(&mut state, &mut api, 0.016);
        phase = state.as_ref().unwrap().phase();
        assert_ne!(phase, Phase::Reeling, "a held Space must not set the hook");
        if phase == Phase::Waiting {
            break;
        }
    }
    assert_eq!(
        phase,
        Phase::Waiting,
        "missed bite should return to Waiting"
    );

    for _ in 0..300 {
        fishing::game::fishing::tick(&mut state, &mut api, 0.016);
        if state.as_ref().unwrap().phase() == Phase::Biting {
            break;
        }
    }
    assert_eq!(state.as_ref().unwrap().phase(), Phase::Biting);
    api.set_input(space_pressed());
    fishing::game::fishing::tick(&mut state, &mut api, 0.016);
    assert_eq!(state.as_ref().unwrap().phase(), Phase::Reeling);
}

/// Fishing line visible in non-Idle/Caught phases, and endpoint follows bobber.
#[test]
fn rod_line_follows_the_bobber() {
    use fishing::game::fishing::Phase;

    let mut world = scene_world();
    let mut api = GameWorld::host(&mut world);
    let mut state = None;
    fishing::game::fishing::tick(&mut state, &mut api, 0.016);

    let line = api
        .query::<LineRenderer>()
        .first()
        .copied()
        .expect("fishing spawns its own LineRenderer");
    let idle_points = api.get::<LineRenderer>(line).unwrap().points.len();
    assert!(
        idle_points <= 1,
        "idle line should be hidden, got {idle_points} points"
    );

    api.set_input(click());
    fishing::game::fishing::tick(&mut state, &mut api, 0.016);
    assert_eq!(state.as_ref().unwrap().phase(), Phase::Casting);
    fishing::game::fishing::tick(&mut state, &mut api, 0.6);

    let points = api.get::<LineRenderer>(line).unwrap().points;
    assert!(
        points.len() >= 3,
        "an active cast draws a polyline, got {} points",
        points.len()
    );
    assert!(
        state.as_ref().unwrap().phase() == Phase::Waiting,
        "cast should have landed"
    );
}

/// The reel reaches the same point at the same elapsed time whatever the frame rate is.
///
/// Drives the state machine up to `Reeling` with a shared step sequence, then
/// advances half of `REEL_SECONDS` at 30 FPS and at 60 FPS and compares the
/// bobber the engine ends up rendering.
#[test]
fn reel_position_does_not_depend_on_the_frame_rate() {
    use fishing::game::fishing::Phase;

    fn bobber_after_reeling(dt: f32, steps: usize) -> [f32; 2] {
        let mut world = scene_world();
        let mut api = GameWorld::host(&mut world);
        let mut state = None;

        api.set_input(InputSnapshot::default());
        fishing::game::fishing::tick(&mut state, &mut api, 0.016);
        api.set_input(click());
        fishing::game::fishing::tick(&mut state, &mut api, 0.016);
        api.set_input(InputSnapshot::default());
        fishing::game::fishing::tick(&mut state, &mut api, 0.6);
        for _ in 0..300 {
            fishing::game::fishing::tick(&mut state, &mut api, 0.016);
            if state.as_ref().unwrap().phase() == Phase::Biting {
                break;
            }
        }
        assert_eq!(state.as_ref().unwrap().phase(), Phase::Biting);

        api.set_input(click());
        fishing::game::fishing::tick(&mut state, &mut api, 0.016);
        assert_eq!(state.as_ref().unwrap().phase(), Phase::Reeling);
        api.set_input(InputSnapshot::default());
        for _ in 0..steps {
            fishing::game::fishing::tick(&mut state, &mut api, dt);
        }

        let bobber = api
            .query::<Name>()
            .into_iter()
            .find(|entity| {
                api.get::<Name>(*entity)
                    .map(|name| name.0 == "bobber")
                    .unwrap_or(false)
            })
            .expect("scene fixture has a bobber");
        let position = api.get::<LocalTransform>(bobber).unwrap().position;
        [position.x, position.y]
    }

    let slow = bobber_after_reeling(1.0 / 30.0, 15);
    let fast = bobber_after_reeling(1.0 / 60.0, 30);

    assert!(
        (slow[0] - fast[0]).abs() < 0.5 && (slow[1] - fast[1]).abs() < 0.5,
        "reel position depends on the frame rate: 30 FPS {slow:?} vs 60 FPS {fast:?}"
    );
}

/// Reflections are mirror images across the water line, not sprites parked on it (#1052).
///
/// The mirror's top edge has to follow `2 * water_line - source bottom`, and the
/// band above the line (the texture edge the art buries under the water) is cut by
/// `clip_rect` instead of being hidden by clamping the position.
#[test]
fn reflections_mirror_across_the_water_line_and_clip_above_it() {
    let mut world = scene_world();
    let mut api = GameWorld::host(&mut world);
    let mut state = None;
    fishing::game::world::tick(&mut state, &mut api, 0.016);
    let water_line = state.as_ref().expect("lake world").water_line();

    let sprites: Vec<EntityHandle> = api.query::<SpriteRenderer>();
    let mut mirrors = 0;
    for mirror in sprites.iter().copied() {
        let sprite = api.get::<SpriteRenderer>(mirror).expect("sprite");
        if !sprite.flip_y || !sprite.sprite_path.starts_with("sprites/bg/") {
            continue;
        }
        mirrors += 1;

        assert!(
            sprite.clip_enabled,
            "reflection {} must be clipped at the water line",
            sprite.sprite_path
        );
        assert_eq!(
            sprite.clip_rect[3], water_line,
            "the clip rect has to end at the water line"
        );
        assert!(
            sprite.clip_rect[1] < water_line,
            "the clip rect has to keep the area below the water line"
        );

        let source = sprites
            .iter()
            .copied()
            .find(|entity| {
                *entity != mirror
                    && api
                        .get::<SpriteRenderer>(*entity)
                        .map(|other| !other.flip_y && other.sprite_path == sprite.sprite_path)
                        .unwrap_or(false)
            })
            .expect("every mirror needs its source layer");
        let source_y = api
            .get::<LocalTransform>(source)
            .expect("source transform")
            .position
            .y;
        let mirror_y = api
            .get::<LocalTransform>(mirror)
            .expect("mirror transform")
            .position
            .y;
        assert!(
            (mirror_y - (2.0 * water_line - source_y)).abs() < 1e-3,
            "mirror top edge {mirror_y} should be 2 * water_line - source bottom ({}), not clamped to the line",
            2.0 * water_line - source_y
        );
    }
    assert!(mirrors > 0, "the reflected layers need mirror sprites");
}
