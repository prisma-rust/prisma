use prism_ecs::{Entity, World};
use prism_scene::components::{Camera, Child, LocalToWorld, LocalTransform, LodGroup, Parent};

use super::lod_system;
use crate::scene::components::LodHidden;
use crate::scene::systems::hierarchy::is_active_in_hierarchy;

fn spawn_camera(world: &mut World, position: glam::Vec3) -> Entity {
    let entity = world.spawn();
    world.insert(
        entity,
        LocalTransform {
            position,
            ..Default::default()
        },
    );
    world.insert(entity, LocalToWorld(glam::Mat4::from_translation(position)));
    world.insert(entity, Camera::default());
    entity
}

fn spawn_group(
    world: &mut World,
    distances: Vec<f32>,
    child_count: usize,
) -> (Entity, Vec<Entity>) {
    spawn_group_with(
        world,
        LodGroup {
            distances,
            ..LodGroup::default()
        },
        child_count,
    )
}

fn spawn_group_with(
    world: &mut World,
    group: LodGroup,
    child_count: usize,
) -> (Entity, Vec<Entity>) {
    let parent = world.spawn();
    world.insert(parent, LocalTransform::default());
    world.insert(parent, LocalToWorld(glam::Mat4::IDENTITY));
    world.insert(parent, group);

    let children: Vec<Entity> = (0..child_count)
        .map(|index| {
            let child = world.spawn();
            world.insert(child, Parent(parent));
            // Child entity position does not affect level selection: distance measured from parent entity.
            world.insert(
                child,
                LocalTransform {
                    position: glam::Vec3::new(index as f32, 0.0, 0.0),
                    ..Default::default()
                },
            );
            child
        })
        .collect();
    world.insert(parent, Child(children.clone()));
    (parent, children)
}

fn hidden(world: &World, entity: Entity) -> bool {
    world
        .get::<LodHidden>(entity)
        .map(|hidden| hidden.0)
        .unwrap_or(false)
}

#[test]
fn the_nearest_camera_distance_selects_the_level() {
    let mut world = World::new();
    let camera = spawn_camera(&mut world, glam::Vec3::ZERO);
    let (_parent, children) = spawn_group(&mut world, vec![10.0, 50.0], 3);

    // distance  0 -> LOD0
    lod_system(&mut world);
    assert!(!hidden(&world, children[0]));
    assert!(hidden(&world, children[1]));
    assert!(hidden(&world, children[2]));
    assert!(is_active_in_hierarchy(&world, children[0]));
    assert!(!is_active_in_hierarchy(&world, children[1]));

    // distance  20 -> LOD1
    world.insert(
        camera,
        LocalToWorld(glam::Mat4::from_translation(glam::Vec3::new(
            20.0, 0.0, 0.0,
        ))),
    );
    lod_system(&mut world);
    assert!(hidden(&world, children[0]));
    assert!(!hidden(&world, children[1]));
    assert!(hidden(&world, children[2]));

    // Distance 100 -> beyond all thresholds -> last level.
    world.insert(
        camera,
        LocalToWorld(glam::Mat4::from_translation(glam::Vec3::new(
            100.0, 0.0, 0.0,
        ))),
    );
    lod_system(&mut world);
    assert!(hidden(&world, children[0]));
    assert!(hidden(&world, children[1]));
    assert!(!hidden(&world, children[2]));
}

#[test]
fn multiple_cameras_use_the_closest_one() {
    let mut world = World::new();
    spawn_camera(&mut world, glam::Vec3::new(100.0, 0.0, 0.0));
    spawn_camera(&mut world, glam::Vec3::new(5.0, 0.0, 0.0));
    let (_, children) = spawn_group(&mut world, vec![10.0, 50.0], 2);

    lod_system(&mut world);
    assert!(!hidden(&world, children[0]), "the near camera should win");
    assert!(hidden(&world, children[1]));
}

#[test]
fn a_disabled_camera_is_ignored() {
    let mut world = World::new();
    let camera = spawn_camera(&mut world, glam::Vec3::new(100.0, 0.0, 0.0));
    world.get_mut::<Camera>(camera).unwrap().enabled = false;
    let (_, children) = spawn_group(&mut world, vec![10.0, 50.0], 2);

    lod_system(&mut world);
    // No enabled camera -> conservatively display LOD0.
    assert!(!hidden(&world, children[0]));
    assert!(hidden(&world, children[1]));
}

#[test]
fn removing_the_group_clears_the_runtime_hidden_flags() {
    let mut world = World::new();
    spawn_camera(&mut world, glam::Vec3::new(100.0, 0.0, 0.0));
    let (parent, children) = spawn_group(&mut world, vec![10.0, 50.0], 2);

    lod_system(&mut world);
    assert!(hidden(&world, children[0]));
    assert!(!hidden(&world, children[1]));

    world.remove::<LodGroup>(parent);
    lod_system(&mut world);
    assert!(!hidden(&world, children[0]));
    assert!(!hidden(&world, children[1]));
}

#[test]
fn level_for_distance_handles_degenerate_input() {
    let group = LodGroup {
        distances: vec![10.0, 50.0],
        ..LodGroup::default()
    };
    assert_eq!(group.level_for_distance(0.0, 0), None);
    assert_eq!(group.level_for_distance(0.0, 3), Some(0));
    assert_eq!(group.level_for_distance(10.0, 3), Some(0));
    assert_eq!(group.level_for_distance(10.1, 3), Some(1));
    assert_eq!(group.level_for_distance(50.0, 3), Some(1));
    assert_eq!(group.level_for_distance(50.1, 3), Some(2));
    // Non-limited / negative distance treated as 0, not jumping to last level.
    assert_eq!(group.level_for_distance(f32::NAN, 3), Some(0));
    assert_eq!(group.level_for_distance(f32::INFINITY, 3), Some(0));
    assert_eq!(group.level_for_distance(-5.0, 3), Some(0));
    // No thresholds -> always LOD0.
    assert_eq!(
        LodGroup {
            distances: Vec::new(),
            ..LodGroup::default()
        }
        .level_for_distance(999.0, 3),
        Some(0)
    );
}

#[test]
fn level_for_screen_ratio_uses_descending_thresholds() {
    let group = LodGroup {
        screen_ratios: vec![0.5, 0.1],
        ..LodGroup::default()
    };
    assert_eq!(group.level_for_screen_ratio(0.8, 3), Some(0));
    assert_eq!(group.level_for_screen_ratio(0.5, 3), Some(0));
    assert_eq!(group.level_for_screen_ratio(0.49, 3), Some(1));
    assert_eq!(group.level_for_screen_ratio(0.1, 3), Some(1));
    assert_eq!(group.level_for_screen_ratio(0.09, 3), Some(2));
    // Non-limited ratio treated as 0, not jumping to last level.
    assert_eq!(group.level_for_screen_ratio(f32::NAN, 3), Some(2));
    // No child entities / no valid threshold returns None, caller falls back to distance mode.
    assert_eq!(group.level_for_screen_ratio(1.0, 0), None);
    assert_eq!(
        LodGroup {
            screen_ratios: vec![f32::NAN, f32::INFINITY],
            ..LodGroup::default()
        }
        .level_for_screen_ratio(1.0, 3),
        None
    );
}

#[test]
fn screen_ratio_mode_selects_by_projected_height() {
    let mut world = World::new();
    let camera = spawn_camera(&mut world, glam::Vec3::new(0.0, 0.0, 10.0));
    let (_, children) = spawn_group_with(
        &mut world,
        LodGroup {
            // Distance 10m would select LOD2, but screen ratio only reaches LOD1.
            distances: vec![1.0, 2.0],
            screen_ratios: vec![0.5, 0.1],
            bounds_radius: 1.0,
        },
        3,
    );

    // Default 60° FOV: 1 / tan(30°) ≈ 1.732; ratio = 1 * 1.732 / 10 ≈ 0.173.
    lod_system(&mut world);
    assert!(hidden(&world, children[0]));
    assert!(!hidden(&world, children[1]));
    assert!(hidden(&world, children[2]));

    // Distance 2m -> ratio ≈ 0.866, back to LOD0.
    world.insert(
        camera,
        LocalToWorld(glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.0, 2.0))),
    );
    lod_system(&mut world);
    assert!(!hidden(&world, children[0]));
    assert!(hidden(&world, children[1]));
    assert!(hidden(&world, children[2]));
}

#[test]
fn an_invalid_screen_radius_falls_back_to_distance() {
    let mut world = World::new();
    spawn_camera(&mut world, glam::Vec3::new(100.0, 0.0, 0.0));
    let (_, children) = spawn_group_with(
        &mut world,
        LodGroup {
            distances: vec![10.0, 50.0],
            screen_ratios: vec![0.9],
            bounds_radius: 0.0,
        },
        3,
    );

    // When radius illegal cannot hide whole group by screen ratio, should fall back to distance: 100m -> LOD2.
    lod_system(&mut world);
    assert!(hidden(&world, children[0]));
    assert!(hidden(&world, children[1]));
    assert!(!hidden(&world, children[2]));
}
