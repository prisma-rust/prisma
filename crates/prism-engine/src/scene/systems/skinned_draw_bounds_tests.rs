use super::*;
use prism_render::culling::Aabb;
use prism_scene::components::SkinnedMeshRenderer;
use prism_scene::ShadowCastingMode;

fn unit_bind() -> Aabb {
    Aabb::new([-0.5, -0.5, -0.5], [0.5, 0.5, 0.5])
}

fn identity_array() -> [[f32; 4]; 4] {
    glam::Mat4::IDENTITY.to_cols_array_2d()
}

fn translated(x: f32) -> [[f32; 4]; 4] {
    glam::Mat4::from_translation(glam::vec3(x, 0.0, 0.0)).to_cols_array_2d()
}

#[test]
fn an_identity_palette_leaves_the_bind_box_untouched() {
    let bounds = skinned_local_bounds(&unit_bind(), &[identity_array()]).expect("bounds");
    assert_eq!(bounds.min, unit_bind().min);
    assert_eq!(bounds.max, unit_bind().max);
}

#[test]
fn a_moving_bone_stretches_the_box_along_its_travel() {
    // One bone stays in place, another moves +X 4m: skinned vertices fall between these two boxes,
    // so bounding box must cover both (−0.5..0.5) and (3.5..4.5).
    let bounds =
        skinned_local_bounds(&unit_bind(), &[identity_array(), translated(4.0)]).expect("bounds");
    assert!(
        (bounds.min[0] + 0.5).abs() < 1e-5,
        "min.x = {}",
        bounds.min[0]
    );
    assert!(
        (bounds.max[0] - 4.5).abs() < 1e-5,
        "max.x = {}",
        bounds.max[0]
    );
    // Other axes unchanged.
    assert!((bounds.min[1] + 0.5).abs() < 1e-5);
    assert!((bounds.max[2] - 0.5).abs() < 1e-5);
}

#[test]
fn a_broken_palette_is_ignored_and_an_empty_one_has_no_bounds() {
    assert!(skinned_local_bounds(&unit_bind(), &[]).is_none());
    let nan = glam::Mat4::from_translation(glam::vec3(f32::NAN, 0.0, 0.0)).to_cols_array_2d();
    assert!(
        skinned_local_bounds(&unit_bind(), &[nan]).is_none(),
        "Non-finite matrix must not produce a fake small box"
    );
    // Good/bad blend: only take the valid one.
    let bounds = skinned_local_bounds(&unit_bind(), &[nan, translated(2.0)]).expect("bounds");
    assert!((bounds.max[0] - 2.5).abs() < 1e-5);
}

#[test]
fn the_cull_uses_the_skinned_box_not_the_bind_pose() {
    // Camera at origin looking −Z, object at (0,0,-5). Bind box very small near origin — here
    // place entity off-screen then bone pulls it back to screen: looking only at bind box will miscull.
    let mut world = prism_ecs::World::new();
    let entity = world.spawn();
    world.insert(entity, LocalToWorld(glam::Mat4::IDENTITY));
    world.insert(
        entity,
        SkinnedMeshRenderer {
            shadow_casting_mode: ShadowCastingMode::Off,
            ..Default::default()
        },
    );
    world.insert(
        entity,
        crate::scene::runtime_components::MeshRef {
            asset_id: prism_scene::components::SceneAssetId::generate(),
            render_handle: prism_render::managers::MeshHandle::default(),
            generation: 1,
        },
    );
    let mut bounds = MeshBounds::default();
    bounds.insert(
        prism_render::managers::MeshHandle::default(),
        Aabb::new([-0.5, -0.5, -0.5], [0.5, 0.5, 0.5]),
    );
    world.insert_resource(bounds);

    // Bind box position far at x = +1000 (off-screen); palette pulls mesh back in front of camera.
    world.insert(
        entity,
        BonePalette::from_matrices(&[glam::Mat4::from_translation(glam::vec3(1000.0, 0.0, 0.0))]),
    );

    let projection = prism_scene::components::Camera {
        fov_y_degrees: 60.0,
        near: 0.1,
        far: 100.0,
        ..Default::default()
    }
    .projection_matrix(1.0);
    let frustum = prism_render::culling::Frustum::from_view_proj(&projection.to_cols_array_2d())
        .expect("usable frustum");

    // Bind pose box at (0,0,-5) (in frustum), after skinning moves to x=1000 (outside).
    // With palette cull must disappear; without palette (bind pose) still appears.
    let mut items: Vec<DrawItem> = Vec::new();
    let mut palette: Vec<[[f32; 4]; 4]> = Vec::new();
    let added = append_skinned_draw_items(&world, Some(&frustum), &mut items, &mut palette);
    assert_eq!(added, 0, "Skinned box off-screen must be culled");

    // Return bone to original position: same entity appears in draw list again.
    world.insert(
        entity,
        BonePalette::from_matrices(&[glam::Mat4::from_translation(glam::vec3(0.0, 0.0, -5.0))]),
    );
    let added = append_skinned_draw_items(&world, Some(&frustum), &mut items, &mut palette);
    assert_eq!(added, 1, "Bone putting mesh back on screen should be drawn");
}
