//! 2D virtual camera unit test (#1160).

use prism_ecs::World;
use prism_scene::components::{Camera, Camera2D, LocalTransform};

use super::camera2d::camera_2d_system;

#[test]
fn test_camera_2d_instant_follow() {
    let mut world = World::new();

    let target = world.spawn();
    world.insert(
        target,
        LocalTransform {
            position: glam::vec3(10.0, 5.0, 0.0),
            ..Default::default()
        },
    );

    let camera = world.spawn();
    world.insert(
        camera,
        LocalTransform {
            position: glam::vec3(0.0, 0.0, -10.0),
            ..Default::default()
        },
    );
    world.insert(
        camera,
        Camera2D {
            target_entity: target.id() as u64,
            damping: 0.0,
            ..Default::default()
        },
    );

    camera_2d_system(&mut world, 0.016);

    let cam_tf = world.get::<LocalTransform>(camera).unwrap();
    assert!((cam_tf.position.x - 10.0).abs() < 1e-4);
    assert!((cam_tf.position.y - 5.0).abs() < 1e-4);
}

#[test]
fn test_camera_2d_deadzone() {
    let mut world = World::new();

    let target = world.spawn();
    world.insert(
        target,
        LocalTransform {
            position: glam::vec3(1.0, 1.0, 0.0),
            ..Default::default()
        },
    );

    let camera = world.spawn();
    world.insert(
        camera,
        LocalTransform {
            position: glam::vec3(0.0, 0.0, -10.0),
            ..Default::default()
        },
    );
    world.insert(
        camera,
        Camera2D {
            target_entity: target.id() as u64,
            damping: 0.0,
            deadzone: [2.0, 2.0],
            ..Default::default()
        },
    );

    camera_2d_system(&mut world, 0.016);
    let cam_tf = world.get::<LocalTransform>(camera).unwrap();
    assert_eq!(cam_tf.position.x, 0.0);
    assert_eq!(cam_tf.position.y, 0.0);

    world.get_mut::<LocalTransform>(target).unwrap().position = glam::vec3(5.0, 0.0, 0.0);
    camera_2d_system(&mut world, 0.016);
    let cam_tf = world.get::<LocalTransform>(camera).unwrap();
    assert!((cam_tf.position.x - 3.0).abs() < 1e-4);
}

#[test]
fn test_camera_2d_confiner() {
    let mut world = World::new();

    let target = world.spawn();
    world.insert(
        target,
        LocalTransform {
            position: glam::vec3(100.0, 100.0, 0.0),
            ..Default::default()
        },
    );

    let camera = world.spawn();
    world.insert(
        camera,
        Camera {
            orthographic: true,
            orthographic_size: 5.0,
            aspect: 2.0,
            ..Default::default()
        },
    );
    world.insert(
        camera,
        LocalTransform {
            position: glam::vec3(0.0, 0.0, -10.0),
            ..Default::default()
        },
    );
    world.insert(
        camera,
        Camera2D {
            target_entity: target.id() as u64,
            damping: 0.0,
            confiner_enabled: true,
            confiner_bounds: [-20.0, -10.0, 20.0, 10.0],
            ..Default::default()
        },
    );

    camera_2d_system(&mut world, 0.016);
    let cam_tf = world.get::<LocalTransform>(camera).unwrap();
    assert!((cam_tf.position.x - 10.0).abs() < 1e-4);
    assert!((cam_tf.position.y - 5.0).abs() < 1e-4);
}

#[test]
fn test_camera_2d_pixel_snapping() {
    let mut world = World::new();

    let target = world.spawn();
    world.insert(
        target,
        LocalTransform {
            position: glam::vec3(1.033, 2.088, 0.0),
            ..Default::default()
        },
    );

    let camera = world.spawn();
    world.insert(
        camera,
        LocalTransform {
            position: glam::vec3(0.0, 0.0, -10.0),
            ..Default::default()
        },
    );
    world.insert(
        camera,
        Camera2D {
            target_entity: target.id() as u64,
            damping: 0.0,
            pixel_snapping: true,
            pixels_per_unit: 16.0,
            ..Default::default()
        },
    );

    camera_2d_system(&mut world, 0.016);
    let cam_tf = world.get::<LocalTransform>(camera).unwrap();
    assert!((cam_tf.position.x - 1.0625).abs() < 1e-4);
    assert!((cam_tf.position.y - 2.0625).abs() < 1e-4);
}
