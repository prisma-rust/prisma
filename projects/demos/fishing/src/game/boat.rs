//! Boat drift + camera follow.
//!
//! In M0 phase boat accepts no input, drifts slowly along sine trajectory, to validate
//! "correct camera + 2D sprite + sorting" whole chain; later `fishing` state machine takes over
//! input (WASD / gamepad) and casting logic.
//!
//! State and write go through [`prism_game_api::GameWorld`]: same logic runs in static Player and
//! dynamic module loaded by editor, no engine types needed.

use prism_game_api::{EntityHandle, GameWorld};
use prism_scene::components::LocalTransform;

/// Camera distance from scene plane (camera at +Z looking −Z).
const CAMERA_Z: f32 = 10.0;
/// Camera height above water surface relative to boat (camera above and diagonal to boat).
const CAMERA_LIFT: f32 = 100.0;
/// Lateral amplitude of boat drift (world units).
const DRIFT_AMPLITUDE: f32 = 40.0;
/// Drift angular speed (radians / sec).
const DRIFT_SPEED: f32 = 0.35;
/// Boat base position.
const BOAT_BASE: [f32; 3] = [0.0, 80.0, 0.0];

/// Drift state — **project-private**, held by caller (Player side puts in ECS resource).
#[derive(Default)]
pub struct BoatState {
    elapsed: f32,
    boat: Option<EntityHandle>,
    camera: Option<EntityHandle>,
}

/// Advance boat and camera each frame.
pub fn tick(state: &mut Option<BoatState>, world: &mut GameWorld<'_>, dt: f32) {
    if state.is_none() {
        *state = Some(BoatState {
            boat: world.find_by_name("boat"),
            camera: world.find_by_name("camera"),
            ..BoatState::default()
        });
    }

    let Some(current) = state.as_mut() else {
        return;
    };
    current.elapsed += dt;

    let drift = (current.elapsed * DRIFT_SPEED).sin() * DRIFT_AMPLITUDE;
    let bob = (current.elapsed * 1.3).sin() * 0.06;

    if let Some(boat) = current.boat {
        if let Some(mut transform) = world.get::<LocalTransform>(boat) {
            transform.position =
                glam::Vec3::new(BOAT_BASE[0] + drift, BOAT_BASE[1] + bob, BOAT_BASE[2]);
            let angle = (current.elapsed * 1.1).sin() * 0.035;
            transform.rotation = glam::Quat::from_rotation_z(angle);
            world.set(boat, transform);
        }
    }

    if let Some(camera) = current.camera {
        if let Some(mut transform) = world.get::<LocalTransform>(camera) {
            transform.position = glam::Vec3::new(drift * 0.5, BOAT_BASE[1] + CAMERA_LIFT, CAMERA_Z);
            world.set(camera, transform);
        }
    }
}
