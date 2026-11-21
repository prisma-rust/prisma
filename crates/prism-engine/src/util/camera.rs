//! Optional convenience helpers for host-owned cameras.
//!
//! Nothing here runs by default. The engine renders with
//! [`fallback_camera_output`](crate::scene::systems::camera::fallback_camera_output)
//! when a world has no `Camera` entity, so a host that wants a free-fly camera has
//! to ask for one explicitly:
//!
//! ```ignore
//! prism_engine::util::camera::spawn_default_camera(engine.world_mut());
//! ```
//!
//! This used to happen inside `Engine::runtime_initialize`, which meant every
//! engine instance — editor, headless tool, test — silently gained a camera entity
//! it never asked for. Scene documents already carry their own cameras, so the
//! implicit one mostly showed up as noise in the hierarchy and in saved state.

use crate::scene::components::{Camera, FlyCameraController, LocalToWorld, LocalTransform};
use prism_ecs::World;

/// Spawn a free-fly camera (`Camera` + `FlyCameraController` + transforms).
///
/// Returns the new entity. The caller owns it from then on; nothing else in the
/// engine references it, and it is not a `SceneMember`, so loading a scene leaves
/// it in place.
pub fn spawn_default_camera(world: &mut World) -> prism_ecs::Entity {
    let entity = world.spawn();
    world.insert(
        entity,
        LocalTransform {
            position: glam::Vec3::new(0.0, 2.0, 5.0),
            rotation: glam::Quat::IDENTITY,
            scale: glam::Vec3::ONE,
        },
    );
    world.insert(entity, LocalToWorld(glam::Mat4::IDENTITY));
    world.insert(
        entity,
        Camera {
            fov_y_degrees: 60.0,
            near: 0.1,
            far: 1000.0,
            exposure: 1.0,
            ..Camera::default()
        },
    );
    world.insert(
        entity,
        FlyCameraController {
            yaw: 0.0,
            pitch: 0.0,
            move_speed: 5.0,
            look_sensitivity: 0.005,
        },
    );
    entity
}
