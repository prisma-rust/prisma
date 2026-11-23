// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

use super::*;
use crate::scene::helpers::HierarchyHelper;
use prism_ecs::World;

#[test]
fn root_gets_identity_world() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, LocalTransform::default());
    world.insert(e, LocalToWorld(glam::Mat4::ZERO)); // dummy

    hierarchy_system(&mut world);

    let wt = world.get::<LocalToWorld>(e).unwrap().0;
    assert_eq!(wt.x_axis[0], 1.0);
    assert_eq!(wt.y_axis[1], 1.0);
    assert_eq!(wt.z_axis[2], 1.0);
    assert_eq!(wt.w_axis[3], 1.0);
}

#[test]
fn child_world_inherits_parent_translation() {
    let mut world = World::new();
    let parent = world.spawn();
    let child = world.spawn();

    world.insert(
        parent,
        LocalTransform {
            position: glam::Vec3::new(2.0, 0.0, 0.0),
            ..Default::default()
        },
    );
    world.insert(parent, LocalToWorld(glam::Mat4::ZERO));
    world.insert(
        child,
        LocalTransform {
            position: glam::Vec3::new(0.0, 3.0, 0.0),
            ..Default::default()
        },
    );
    world.insert(child, LocalToWorld(glam::Mat4::ZERO));

    HierarchyHelper::reparent(&mut world, child, Some(parent));
    hierarchy_system(&mut world);

    let cw = world.get::<LocalToWorld>(child).unwrap().0;
    // Expect translation  = [2, 3, 0]
    assert!(
        (cw.col(3)[0] - 2.0).abs() < 1e-6,
        "child x = {}",
        cw.col(3)[0]
    );
    assert!(
        (cw.col(3)[1] - 3.0).abs() < 1e-6,
        "child y = {}",
        cw.col(3)[1]
    );
    assert!(
        (cw.col(3)[2] - 0.0).abs() < 1e-6,
        "child z = {}",
        cw.col(3)[2]
    );
    assert!(
        (cw.col(3)[3] - 1.0).abs() < 1e-6,
        "child w = {}",
        cw.col(3)[3]
    );
}

#[test]
fn nested_hierarchy() {
    let mut world = World::new();
    let gp = world.spawn(); // grandparent
    let p = world.spawn(); // parent
    let c = world.spawn(); // child

    world.insert(
        gp,
        LocalTransform {
            position: glam::Vec3::new(1.0, 0.0, 0.0),
            ..Default::default()
        },
    );
    world.insert(gp, LocalToWorld(glam::Mat4::ZERO));
    world.insert(
        p,
        LocalTransform {
            position: glam::Vec3::new(0.0, 2.0, 0.0),
            ..Default::default()
        },
    );
    world.insert(p, LocalToWorld(glam::Mat4::ZERO));
    world.insert(
        c,
        LocalTransform {
            position: glam::Vec3::new(0.0, 0.0, 3.0),
            ..Default::default()
        },
    );
    world.insert(c, LocalToWorld(glam::Mat4::ZERO));

    HierarchyHelper::reparent(&mut world, p, Some(gp));
    HierarchyHelper::reparent(&mut world, c, Some(p));
    hierarchy_system(&mut world);

    // gp: [1,0,0], p: [1,2,0], c: [1,2,3]
    let pw = world.get::<LocalToWorld>(p).unwrap().0;
    assert!((pw.col(3)[0] - 1.0).abs() < 1e-6, "p.x = {}", pw.col(3)[0]);
    assert!((pw.col(3)[1] - 2.0).abs() < 1e-6, "p.y = {}", pw.col(3)[1]);

    let cw = world.get::<LocalToWorld>(c).unwrap().0;
    assert!((cw.col(3)[0] - 1.0).abs() < 1e-6, "c.x = {}", cw.col(3)[0]);
    assert!((cw.col(3)[1] - 2.0).abs() < 1e-6, "c.y = {}", cw.col(3)[1]);
    assert!((cw.col(3)[2] - 3.0).abs() < 1e-6, "c.z = {}", cw.col(3)[2]);
}

#[test]
fn dead_parent_makes_the_child_a_root() {
    // Parent entity despawned after child entity's `Parent` still points to dead entity (despawn only deletes own
    // component). Before such child node neither root nor reachable, `LocalToWorld` will forever freeze in
    // old value; now promoted to root by unified semantics, equal to its own `LocalTransform`.
    let mut world = World::new();
    let parent = world.spawn();
    let child = world.spawn();

    world.insert(parent, LocalTransform::default());
    world.insert(parent, LocalToWorld(glam::Mat4::ZERO));
    world.insert(
        child,
        LocalTransform {
            position: glam::Vec3::new(5.0, 0.0, 0.0),
            ..Default::default()
        },
    );
    world.insert(child, LocalToWorld(glam::Mat4::ZERO));

    HierarchyHelper::reparent(&mut world, child, Some(parent));
    world.despawn(parent);
    hierarchy_system(&mut world);

    let cw = world.get::<LocalToWorld>(child).unwrap().0;
    assert!(
        (cw.w_axis[0] - 5.0).abs() < 1e-6,
        "child.x = {}",
        cw.w_axis[0]
    );
}

#[test]
fn parent_without_child_link_treats_the_child_as_a_root() {
    // Only has `Parent`, parent's `Child` inside but doesn't list it: tree inconsistent,
    // cannot let it forever wait for a parent node that will never be traversed.
    let mut world = World::new();
    let parent = world.spawn();
    let child = world.spawn();

    world.insert(
        parent,
        LocalTransform {
            position: glam::Vec3::new(100.0, 0.0, 0.0),
            ..Default::default()
        },
    );
    world.insert(
        child,
        LocalTransform {
            position: glam::Vec3::new(3.0, 0.0, 0.0),
            ..Default::default()
        },
    );
    world.insert(child, Parent(parent));

    hierarchy_system(&mut world);

    let cw = world.get::<LocalToWorld>(child).unwrap().0;
    assert!(
        (cw.w_axis[0] - 3.0).abs() < 1e-6,
        "child.x = {}",
        cw.w_axis[0]
    );
}

#[test]
fn transformless_intermediate_node_does_not_block_its_subtree() {
    // root(x=10) → mid(no  LocalTransform) → leaf(x=0)
    // Mid itself should not get `LocalToWorld`, but leaf must inherit root's translation.
    let mut world = World::new();
    let root = world.spawn();
    let mid = world.spawn();
    let leaf = world.spawn();

    world.insert(
        root,
        LocalTransform {
            position: glam::Vec3::new(10.0, 0.0, 0.0),
            ..Default::default()
        },
    );
    world.insert(
        leaf,
        LocalTransform {
            position: glam::Vec3::new(0.0, 0.0, 0.0),
            ..Default::default()
        },
    );

    HierarchyHelper::reparent(&mut world, mid, Some(root));
    HierarchyHelper::reparent(&mut world, leaf, Some(mid));
    hierarchy_system(&mut world);

    assert!(
        world.get::<LocalToWorld>(mid).is_none(),
        "transform-less nodes stay without LocalToWorld"
    );
    let lw = world.get::<LocalToWorld>(leaf).unwrap().0;
    assert!(
        (lw.w_axis[0] - 10.0).abs() < 1e-6,
        "leaf.x = {}",
        lw.w_axis[0]
    );
}

#[test]
fn parent_cycle_does_not_recurse_forever() {
    // Dirty data forming cycle cannot let DFS recurse infinitely: A is root and lists B, B lists A again.
    let mut world = World::new();
    let a = world.spawn();
    let b = world.spawn();

    for entity in [a, b] {
        world.insert(
            entity,
            LocalTransform {
                position: glam::Vec3::new(1.0, 0.0, 0.0),
                ..Default::default()
            },
        );
    }
    world.insert(a, Child(vec![b]));
    world.insert(b, Child(vec![a]));
    world.insert(b, Parent(a));

    hierarchy_system(&mut world);

    // A is root (no Parent) → [1,0,0]; B is child → [2,0,0].
    assert!((world.get::<LocalToWorld>(a).unwrap().0.w_axis[0] - 1.0).abs() < 1e-6);
    assert!((world.get::<LocalToWorld>(b).unwrap().0.w_axis[0] - 2.0).abs() < 1e-6);
}

#[test]
fn no_panics_on_empty_world() {
    let mut world = World::new();
    hierarchy_system(&mut world); // should not panic
}

#[test]
fn single_entity_no_transform() {
    // entity  without LocalTransform — should be skipped silently.
    let mut world = World::new();
    let e = world.spawn();
    // No LocalTransform
    hierarchy_system(&mut world);
    assert!(world.is_alive(e));
}

#[test]
fn multiple_roots() {
    let mut world = World::new();
    let r1 = world.spawn();
    let r2 = world.spawn();

    world.insert(
        r1,
        LocalTransform {
            position: glam::Vec3::new(10.0, 0.0, 0.0),
            ..Default::default()
        },
    );
    world.insert(r1, LocalToWorld(glam::Mat4::ZERO));
    world.insert(
        r2,
        LocalTransform {
            position: glam::Vec3::new(0.0, 20.0, 0.0),
            ..Default::default()
        },
    );
    world.insert(r2, LocalToWorld(glam::Mat4::ZERO));

    hierarchy_system(&mut world);

    let wt1 = world.get::<LocalToWorld>(r1).unwrap().0;
    assert!((wt1.w_axis[0] - 10.0).abs() < 1e-6);
    let wt2 = world.get::<LocalToWorld>(r2).unwrap().0;
    assert!((wt2.w_axis[1] - 20.0).abs() < 1e-6);
}

#[test]
fn deactivating_a_parent_deactivates_the_whole_subtree() {
    let mut world = World::new();
    let parent = world.spawn();
    let child = world.spawn();
    let grandchild = world.spawn();
    for entity in [parent, child, grandchild] {
        world.insert(entity, Active(true));
    }
    HierarchyHelper::reparent(&mut world, child, Some(parent));
    HierarchyHelper::reparent(&mut world, grandchild, Some(child));

    world.insert(parent, Active(false));
    active_hierarchy_system(&mut world);

    assert!(!world.get::<ActiveInHierarchy>(parent).unwrap().0);
    assert!(!world.get::<ActiveInHierarchy>(child).unwrap().0);
    assert!(!world.get::<ActiveInHierarchy>(grandchild).unwrap().0);
    assert!(!is_active_in_hierarchy(&world, grandchild));
    // Own `Active` not changed: checker displays `activeSelf`.
    assert!(world.get::<Active>(child).unwrap().0);

    // Parent heavy new opened, child tree comes back.
    world.insert(parent, Active(true));
    active_hierarchy_system(&mut world);
    assert!(is_active_in_hierarchy(&world, grandchild));
}

#[test]
fn a_hidden_child_does_not_hide_its_siblings_or_parent() {
    let mut world = World::new();
    let parent = world.spawn();
    let hidden = world.spawn();
    let shown = world.spawn();
    for entity in [parent, hidden, shown] {
        world.insert(entity, Active(true));
    }
    HierarchyHelper::reparent(&mut world, hidden, Some(parent));
    HierarchyHelper::reparent(&mut world, shown, Some(parent));
    world.insert(hidden, Active(false));

    active_hierarchy_system(&mut world);

    assert!(is_active_in_hierarchy(&world, parent));
    assert!(is_active_in_hierarchy(&world, shown));
    assert!(!is_active_in_hierarchy(&world, hidden));
}

#[test]
fn scene_visibility_hides_rendering_without_disabling_the_entity() {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, Active(true));
    world.insert(entity, Hidden(true));

    active_hierarchy_system(&mut world);

    assert!(
        world.get::<ActiveInHierarchy>(entity).unwrap().0,
        "ActiveInHierarchy must preserve author semantics"
    );
    assert!(
        !is_active_in_hierarchy(&world, entity),
        "Hidden only filters rendering systems"
    );

    world.remove::<Hidden>(entity);
    active_hierarchy_system(&mut world);
    assert!(is_active_in_hierarchy(&world, entity));
}

#[test]
fn active_hierarchy_survives_a_parent_cycle() {
    let mut world = World::new();
    let a = world.spawn();
    let b = world.spawn();
    world.insert(a, Active(true));
    world.insert(b, Active(true));
    // Bypass `HierarchyHelper` directly write `Parent`, simulate corrupted save.
    world.insert(a, Parent(b));
    world.insert(b, Parent(a));

    active_hierarchy_system(&mut world);

    assert!(is_active_in_hierarchy(&world, a));
    assert!(is_active_in_hierarchy(&world, b));
}
