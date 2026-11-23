// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

use super::*;
use crate::input::{ElementState, KeyCode};
use prism_ecs::World;

#[test]
fn no_camera_returns_none() {
    let world = World::new();
    assert!(collect_camera(&world).is_none());
}

#[test]
fn finds_first_camera() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(
        e,
        Camera {
            fov_y_degrees: 75.0,
            near: 0.01,
            far: 500.0,
            ..Camera::default()
        },
    );
    let cam = collect_camera(&world);
    assert!(cam.is_some());
    assert_eq!(cam.unwrap().fov_y_degrees, 75.0);
}

#[test]
fn multiple_cameras_returns_first() {
    let mut world = World::new();
    let e1 = world.spawn();
    world.insert(
        e1,
        Camera {
            fov_y_degrees: 60.0,
            ..Camera::default()
        },
    );
    let e2 = world.spawn();
    world.insert(
        e2,
        Camera {
            fov_y_degrees: 90.0,
            near: 0.1,
            far: 100.0,
            ..Camera::default()
        },
    );

    let cam = collect_camera(&world).unwrap();
    // ECS query order is deterministic - first inserted should be first
    assert_eq!(cam.fov_y_degrees, 60.0);
}

/// View and clear must come from same camera (issue #11).
///
/// Two enabled cameras, different `depth`, different background colors: view must come from max `depth`
/// (`Camera.main`), and clear color must be background of **same entity**.
#[test]
fn view_and_clear_color_come_from_the_same_camera() {
    let mut world = World::new();

    let low = world.spawn();
    world.insert(
        low,
        Camera {
            depth: 0.0,
            background_color: [1.0, 0.0, 0.0, 1.0],
            ..Camera::default()
        },
    );
    world.insert(
        low,
        LocalToWorld(Mat4::from_translation(Vec3::new(0.0, 0.0, 5.0))),
    );

    let high = world.spawn();
    world.insert(
        high,
        Camera {
            depth: 1.0,
            background_color: [0.0, 0.0, 1.0, 1.0],
            ..Camera::default()
        },
    );
    world.insert(
        high,
        LocalToWorld(Mat4::from_translation(Vec3::new(0.0, 2.0, 9.0))),
    );

    let out = compute_camera_output(&world, &Mat4::IDENTITY).expect("camera output");
    assert_eq!(out.entity, high, "View should come from max depth camera");
    assert!(
        (out.eye.y - 2.0).abs() < 1e-5,
        "Eye should come from same entity transform, actual {:?}",
        out.eye
    );
    assert_eq!(
        clear_color(&world, out.entity),
        [0.0, 0.0, 1.0, 1.0],
        "Clear color should come from actually rendered camera"
    );
}

/// When main camera lacks `LocalToWorld`, step down to next available camera, not whole fallback.
#[test]
fn camera_without_transform_falls_back_to_next_enabled() {
    let mut world = World::new();

    let usable = world.spawn();
    world.insert(
        usable,
        Camera {
            depth: 0.0,
            ..Camera::default()
        },
    );
    world.insert(
        usable,
        LocalToWorld(Mat4::from_translation(Vec3::new(0.0, 0.0, 3.0))),
    );

    let transform_less = world.spawn();
    world.insert(
        transform_less,
        Camera {
            depth: 5.0,
            ..Camera::default()
        },
    );

    let out = compute_camera_output(&world, &Mat4::IDENTITY).expect("camera output");
    assert_eq!(out.entity, usable);
}

/// `#461`: only max `depth` one will be drawn, others currently ignored —
/// this list is those "added but ineffective" cameras, for hint, not silently swallowed.
#[test]
fn ignored_cameras_lists_everything_but_the_main_camera() {
    let mut world = World::new();

    let low = world.spawn();
    world.insert(
        low,
        Camera {
            depth: -10.0,
            ..Camera::default()
        },
    );

    let main = world.spawn();
    world.insert(
        main,
        Camera {
            depth: 5.0,
            ..Camera::default()
        },
    );

    let middle = world.spawn();
    world.insert(
        middle,
        Camera {
            depth: 0.0,
            ..Camera::default()
        },
    );

    let off = world.spawn();
    world.insert(
        off,
        Camera {
            depth: 50.0,
            enabled: false,
            ..Camera::default()
        },
    );

    // Main camera = max `depth` **enabled** camera.
    assert_eq!(main_camera(&world), Some(main));
    // Disabled camera is neither main nor in ignored list (it should not participate).
    let ignored = ignored_cameras(&world);
    assert_eq!(ignored.len(), 2);
    assert!(ignored.contains(&low));
    assert!(ignored.contains(&middle));
    assert!(!ignored.contains(&main));
    assert!(!ignored.contains(&off));
}

/// Disabled camera neither views nor provides clear color.
#[test]
fn disabled_camera_is_skipped_entirely() {
    let mut world = World::new();

    let active = world.spawn();
    world.insert(active, Camera::default());
    world.insert(active, LocalToWorld(Mat4::IDENTITY));

    let disabled = world.spawn();
    world.insert(
        disabled,
        Camera {
            depth: 9.0,
            enabled: false,
            background_color: [1.0, 0.0, 0.0, 1.0],
            ..Camera::default()
        },
    );
    world.insert(disabled, LocalToWorld(Mat4::IDENTITY));

    assert_eq!(main_camera(&world), Some(active));
    let out = compute_camera_output(&world, &Mat4::IDENTITY).expect("camera output");
    assert_eq!(out.entity, active);
    assert_eq!(
        clear_color(&world, out.entity),
        Camera::default().background_color
    );
}

/// When entity has no `Camera` (or deleted), clear color degrades to `DEFAULT_CLEAR_COLOR`.
#[test]
fn clear_color_for_camera_less_entity_is_default() {
    let mut world = World::new();
    let entity = world.spawn();
    assert_eq!(clear_color(&world, entity), DEFAULT_CLEAR_COLOR);
}

/// Clear mode and clear color same source: reads `clear_flags` of same camera.
#[test]
fn clear_flags_reads_the_camera_component() {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(
        entity,
        Camera {
            clear_flags: CameraClearFlags::SolidColor,
            ..Camera::default()
        },
    );
    assert_eq!(clear_flags(&world, entity), CameraClearFlags::SolidColor);
}

/// When entity has no `Camera`, clear mode degrades to engine default `Skybox`.
#[test]
fn clear_flags_for_camera_less_entity_is_skybox() {
    let world = World::new();
    let entity = prism_ecs::Entity::from_raw(u32::MAX, 0);
    assert_eq!(clear_flags(&world, entity), CameraClearFlags::Skybox);
}

/// Fly input must drive **actually rendering** camera (PR #38 review finding 2).
///
/// First inserted low `depth` camera was old "first enabled" choice; input should target
/// `active_camera` (max depth and renderable one).
#[test]
fn fly_input_drives_the_rendered_camera() {
    let mut world = World::new();

    let low = world.spawn();
    world.insert(
        low,
        Camera {
            depth: 0.0,
            ..Camera::default()
        },
    );
    world.insert(low, FlyCameraController::default());
    world.insert(low, LocalTransform::default());
    world.insert(low, LocalToWorld(Mat4::IDENTITY));

    let high = world.spawn();
    world.insert(
        high,
        Camera {
            depth: 1.0,
            ..Camera::default()
        },
    );
    world.insert(high, FlyCameraController::default());
    world.insert(high, LocalTransform::default());
    world.insert(high, LocalToWorld(Mat4::IDENTITY));

    let mut input = InputManager::new();
    input.handle_keyboard(KeyCode::KeyW, ElementState::Pressed);

    // Shortcut key only returns camera when pointer locked, so here pass `look_active = true`.
    let updated = camera_controller_system(&mut world, &input, 1.0, true);
    assert_eq!(updated, Some(high), "Input should go to rendering camera");

    let moved = world
        .get::<LocalTransform>(high)
        .expect("rendered camera transform")
        .position
        .length();
    assert!(
        moved > 0.0,
        "Rendering camera should be moved, actual {moved}"
    );

    let untouched = world
        .get::<LocalTransform>(low)
        .expect("low camera transform")
        .position
        .length();
    assert!(
        untouched < 1e-6,
        "Low depth camera should not be moved, actual {untouched}"
    );
}

/// With pointer unlocked keyboard belongs to project UI: WASD must never move camera.
#[test]
fn wasd_does_not_move_the_camera_while_unlocked() {
    let mut world = World::new();
    let cam = world.spawn();
    world.insert(cam, Camera::default());
    world.insert(cam, FlyCameraController::default());
    world.insert(cam, LocalTransform::default());
    world.insert(cam, LocalToWorld(Mat4::IDENTITY));

    let mut input = InputManager::new();
    for key in [KeyCode::KeyW, KeyCode::KeyA, KeyCode::KeyS, KeyCode::KeyD] {
        input.handle_keyboard(key, ElementState::Pressed);
    }

    camera_controller_system(&mut world, &input, 1.0, false);
    let moved = world
        .get::<LocalTransform>(cam)
        .expect("camera transform")
        .position
        .length();
    assert!(
        moved < 1e-6,
        "Unlocked camera should not move by WASD, actual {moved}"
    );
}

// ---------------------------------------------------------------------------
// Orientation writeback (issue #121)
// ---------------------------------------------------------------------------

/// `yaw = 0` must look toward −Z (meaning of identity transform in `.scene.json`), not +X.
#[test]
fn yaw_zero_looks_down_negative_z() {
    let f = forward(0.0, 0.0);
    assert!(
        (f - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-6,
        "forward(0, 0) should be −Z, actual {f:?}"
    );
    let r = right(0.0);
    assert!(
        (r - Vec3::X).length() < 1e-6,
        "right(0) should be +X, actual {r:?}"
    );
    assert!(
        fly_rotation(0.0, 0.0).dot(glam::Quat::IDENTITY).abs() > 1.0 - 1e-6,
        "yaw = pitch = 0 should give identity quaternion (= look −Z)"
    );
}

/// View matrix and orientation written to `LocalTransform.rotation` must be same one,
/// otherwise camera child nodes misalign with screen.
#[test]
fn fly_view_matches_fly_rotation() {
    let yaw = 0.7;
    let pitch = -0.3;
    let eye = Vec3::new(1.0, 2.0, -3.0);
    let view = fly_view(eye, yaw, pitch);
    let expected = Mat4::from_rotation_translation(fly_rotation(yaw, pitch), eye).inverse();
    for c in 0..4 {
        for r in 0..4 {
            assert!(
                (view.col(c)[r] - expected.col(c)[r]).abs() < 1e-4,
                "view[{c}][{r}] = {} vs {}",
                view.col(c)[r],
                expected.col(c)[r]
            );
        }
    }
}

/// After looking around, camera child node must follow rotation (core symptom of issue #121).
#[test]
fn children_follow_the_controller_orientation() {
    let mut world = World::new();
    let cam = world.spawn();
    world.insert(cam, Camera::default());
    world.insert(
        cam,
        FlyCameraController {
            yaw: std::f32::consts::FRAC_PI_2,
            ..Default::default()
        },
    );
    world.insert(cam, LocalTransform::default());
    world.insert(cam, LocalToWorld(Mat4::IDENTITY));

    // An identity child node in front of camera (local −Z)
    let child = world.spawn();
    world.insert(
        child,
        LocalTransform {
            position: Vec3::new(0.0, 0.0, -1.0),
            ..Default::default()
        },
    );
    crate::scene::helpers::HierarchyHelper::reparent(&mut world, child, Some(cam));

    camera_controller_system(&mut world, &InputManager::new(), 0.0, false);
    crate::scene::systems::hierarchy::hierarchy_system(&mut world);

    // yaw = π/2 → looking −X, so child world position is (−1, 0, 0)
    let world_pos = world
        .get::<LocalToWorld>(child)
        .expect("child world transform")
        .0
        .col(3)
        .truncate();
    assert!(
        (world_pos - Vec3::new(-1.0, 0.0, 0.0)).length() < 1e-4,
        "Child should rotate to camera front, actual {world_pos:?}"
    );
}

/// When camera under rotated parent: world orientation still yaw/pitch direction, WASD also moves along world axes
/// (cannot add world axis vector directly to local coords).
#[test]
fn parented_camera_keeps_world_orientation_and_movement() {
    let mut world = World::new();

    let parent = world.spawn();
    world.insert(
        parent,
        LocalTransform {
            rotation: glam::Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            ..Default::default()
        },
    );
    world.insert(parent, LocalToWorld(Mat4::IDENTITY));

    let cam = world.spawn();
    world.insert(cam, Camera::default());
    world.insert(cam, FlyCameraController::default()); // yaw = 0 → world −Z
    world.insert(
        cam,
        LocalTransform {
            position: Vec3::new(1.0, 0.0, 0.0),
            ..Default::default()
        },
    );
    world.insert(cam, LocalToWorld(Mat4::IDENTITY));
    crate::scene::helpers::HierarchyHelper::reparent(&mut world, cam, Some(parent));

    // Compute parent world matrix first so controller reads parent rotation.
    crate::scene::systems::hierarchy::hierarchy_system(&mut world);

    let child = world.spawn();
    world.insert(
        child,
        LocalTransform {
            position: Vec3::new(0.0, 0.0, -2.0),
            ..Default::default()
        },
    );
    crate::scene::helpers::HierarchyHelper::reparent(&mut world, child, Some(cam));

    camera_controller_system(&mut world, &InputManager::new(), 0.0, false);
    crate::scene::systems::hierarchy::hierarchy_system(&mut world);

    let cam_pos = world.get::<LocalToWorld>(cam).unwrap().0.col(3).truncate();
    let child_pos = world
        .get::<LocalToWorld>(child)
        .unwrap()
        .0
        .col(3)
        .truncate();
    assert!(
        child_pos.z - cam_pos.z < -1.9,
        "Child should be in camera world −Z direction, actual dz = {}",
        child_pos.z - cam_pos.z
    );

    let before = cam_pos;
    let mut input = InputManager::new();
    input.handle_keyboard(KeyCode::KeyW, ElementState::Pressed);
    camera_controller_system(&mut world, &input, 1.0, true);
    crate::scene::systems::hierarchy::hierarchy_system(&mut world);
    let after = world.get::<LocalToWorld>(cam).unwrap().0.col(3).truncate();

    let delta = after - before;
    assert!(
        delta.z < -0.5,
        "W should move along world −Z, actual {delta:?}"
    );
    assert!(
        delta.x.abs() < 1e-4,
        "Parent rotation should not skew W into X, actual {delta:?}"
    );
}
