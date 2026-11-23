use super::shadow::{camera_frustum_corners, shadow_matrix_fitting};
use super::*;
use crate::scene::components::LocalToWorld;

/// Each enabled camera produces one layer, ascending by `Camera.depth`; `clearFlags` maps to
/// color / depth clear bits (#461).
#[test]
fn every_enabled_camera_becomes_a_composited_layer_in_depth_order() {
    use crate::scene::components::{Camera, LocalTransform};
    use prism_scene::CameraClearFlags;

    let mut world = World::new();
    let near = world.spawn();
    world.insert(
        near,
        Camera {
            depth: 1.0,
            clear_flags: CameraClearFlags::Depth,
            ..Default::default()
        },
    );
    world.insert(near, LocalTransform::default());
    world.insert(near, LocalToWorld(Mat4::IDENTITY));

    let far = world.spawn();
    world.insert(
        far,
        Camera {
            depth: 2.0,
            clear_flags: CameraClearFlags::SolidColor,
            background_color: [0.0, 1.0, 0.0, 1.0],
            ..Default::default()
        },
    );
    world.insert(
        far,
        LocalTransform {
            position: Vec3::new(0.0, 0.0, 5.0),
            ..Default::default()
        },
    );
    world.insert(
        far,
        LocalToWorld(Mat4::from_translation(Vec3::new(0.0, 0.0, 5.0))),
    );

    let mut out = SceneChanges::default();
    collect_scene_changes_into(&mut world, 16.0 / 9.0, &Mat4::IDENTITY, 60.0, 1, &mut out);

    assert_eq!(out.cameras.len(), 2, "both cameras must be composited");
    // Depth: clear depth only, keep whatever the previous layer drew.
    assert!(!out.cameras[0].clear_color_enabled);
    assert!(out.cameras[0].clear_depth);
    assert!(!out.cameras[0].draw_skybox);
    // SolidColor: clear colour (and depth), no skybox.
    assert!(out.cameras[1].clear_color_enabled);
    assert!(out.cameras[1].clear_depth);
    assert_eq!(out.cameras[1].clear_color, [0.0, 1.0, 0.0, 1.0]);
    assert!(!out.cameras[1].draw_skybox);
}

/// With no camera still composites one gray fallback layer (no skybox), legacy behavior.
#[test]
fn a_scene_without_cameras_still_composites_one_fallback_layer() {
    let mut world = World::new();
    let mut out = SceneChanges::default();
    collect_scene_changes_into(&mut world, 16.0 / 9.0, &Mat4::IDENTITY, 60.0, 1, &mut out);
    assert_eq!(out.cameras.len(), 1);
    assert!(!out.cameras[0].has_camera);
    assert!(out.cameras[0].clear_color_enabled);
    assert!(!out.cameras[0].draw_skybox);
}

/// #491: objects in second camera view must not be culled by main camera frustum (multi-camera compositing #461).
#[test]
fn extraction_keeps_objects_visible_only_through_a_second_camera() {
    use crate::scene::components::{Camera, LocalTransform, MeshRef};
    use crate::scene::systems::render::MeshBounds;
    use prism_render::managers::MeshHandle;
    use prism_scene::SceneAssetId;

    let mut world = World::new();
    let primary = world.spawn();
    world.insert(primary, Camera::default());
    world.insert(primary, LocalTransform::default());
    world.insert(primary, LocalToWorld(Mat4::IDENTITY));

    let secondary = world.spawn();
    world.insert(
        secondary,
        Camera {
            depth: 1.0,
            ..Camera::default()
        },
    );
    let rotation = glam::Quat::from_rotation_y(std::f32::consts::PI);
    world.insert(
        secondary,
        LocalTransform {
            rotation,
            ..LocalTransform::default()
        },
    );
    world.insert(secondary, LocalToWorld(Mat4::from_quat(rotation)));

    // Main camera looks −Z, so object at +Z only visible in second (rotation 180°) camera.
    let mesh = world.spawn();
    world.insert(
        mesh,
        LocalToWorld(Mat4::from_translation(Vec3::new(0.0, 0.0, 5.0))),
    );
    world.insert(
        mesh,
        MeshRef {
            asset_id: SceneAssetId::generate(),
            render_handle: MeshHandle::default(),
            generation: 1,
        },
    );
    let mut bounds = MeshBounds::default();
    bounds.insert(
        MeshHandle::default(),
        prism_render::culling::Aabb::new([-0.5; 3], [0.5; 3]),
    );
    world.insert_resource(bounds);

    let packet = extract_frame_packet(&mut world, 1.0, &Mat4::IDENTITY);
    assert_eq!(
        packet.scene.cameras.len(),
        2,
        "Both cameras participate in compositing"
    );
    assert_eq!(
        packet.draw_items.len(),
        1,
        "Object visible in second camera must not be culled by main camera"
    );
}

/// A perspective camera looking down -Z from `eye`, plus its inverse VP.
fn test_camera(
    eye: Vec3,
    fov_y_deg: f32,
    aspect: f32,
    near: f32,
    far: f32,
) -> (Mat4, Mat4, Mat4, f32, f32) {
    let view = glam::camera::rh::view::look_at_mat4(eye, eye + Vec3::NEG_Z, Vec3::Y);
    let projection =
        glam::camera::rh::proj::vulkan::perspective(fov_y_deg.to_radians(), aspect, near, far);
    let proj22 = projection.col(2).z;
    let proj32 = projection.col(3).z;
    (
        view,
        projection,
        (projection * view).inverse(),
        proj22,
        proj32,
    )
}

/// The shadow map must cover the camera frustum, and must not be widened by
/// scenery the camera cannot see.
#[test]
fn the_shadow_box_fits_the_camera_frustum() {
    let dir = Vec4::new(0.0, -1.0, 0.0, 0.0);
    let (view, projection, inv, proj22, proj32) =
        test_camera(Vec3::ZERO, 60.0, 16.0 / 9.0, 0.1, 500.0);
    let corners = camera_frustum_corners(&inv, proj22, proj32, false, 0.1, 60.0);
    let light = shadow_matrix_fitting(&dir, &corners, 2048);

    let inside = |point: Vec3| {
        let clip = light * point.extend(1.0);
        clip.z >= -1e-4
            && clip.z <= clip.w + 1e-4
            && clip.x.abs() <= clip.w
            && clip.y.abs() <= clip.w
    };
    // A point in the middle of the visible slab is inside the map.
    assert!(
        inside(Vec3::new(0.0, 0.0, -30.0)),
        "visible slab must be covered"
    );
    // Scenery the camera cannot see must not be inside the map either: a
    // point far off to the side and a point past the shadow distance are out.
    assert!(
        !inside(Vec3::new(500.0, 0.0, -30.0)),
        "a point outside the camera frustum must not be covered"
    );
    assert!(
        !inside(Vec3::new(0.0, 0.0, -400.0)),
        "past the shadow distance is not covered"
    );
    let _ = (view, projection);
}

/// Halving the shadow distance must shrink the light-space box: coverage
/// follows the camera frustum, not a constant world-size box.
#[test]
fn a_longer_shadow_distance_covers_more_world() {
    let dir = Vec4::new(0.0, -1.0, 0.0, 0.0);
    let (_v, _p, inv, proj22, proj32) = test_camera(Vec3::ZERO, 60.0, 16.0 / 9.0, 0.1, 500.0);
    // The x scale is 1/half, so the box half-extent is the reciprocal of the
    // clip-space width of one world unit.
    let half_extent = |distance: f32| {
        let corners = camera_frustum_corners(&inv, proj22, proj32, false, 0.1, distance);
        let m = shadow_matrix_fitting(&dir, &corners, 2048);
        let origin = m * Vec3::ZERO.extend(1.0);
        let unit = m * Vec3::new(1.0, 0.0, 0.0).extend(1.0);
        (unit.x - origin.x).abs().recip()
    };
    assert!(
        half_extent(30.0) < half_extent(120.0),
        "a longer shadow distance must widen the box"
    );
}

/// Camera translation smaller than one texel must not move the projection.
#[test]
fn texel_snapping_freezes_the_shadow_matrix_between_texels() {
    let dir = Vec4::new(-0.4, -1.0, -0.3, 0.0);
    let project = |eye: Vec3, probe: Vec3| {
        let (_v, _p, inv, proj22, proj32) = test_camera(eye, 60.0, 16.0 / 9.0, 0.1, 500.0);
        let corners = camera_frustum_corners(&inv, proj22, proj32, false, 0.1, 60.0);
        let m = shadow_matrix_fitting(&dir, &corners, 2048);
        let clip = m * probe.extend(1.0);
        (clip.x, clip.y)
    };
    let probe = Vec3::new(3.0, 1.5, -25.0);
    let base = Vec3::new(0.137, -0.211, -30.0);
    let (ax, ay) = project(base, probe);
    let (bx, by) = project(base + Vec3::new(0.0002, 0.0, 0.0), probe);
    assert!(
        (ax - bx).abs() < 1e-5 && (ay - by).abs() < 1e-5,
        "sub-texel camera motion must not move the shadow map: {ax},{ay} vs {bx},{by}"
    );
    let (cx, cy) = project(base + Vec3::new(2.0, 0.0, 0.0), probe);
    assert!(
        (ax - cx).abs() > 1e-3 || (ay - cy).abs() > 1e-3,
        "moving several texels must move the projection"
    );
}

/// Two cascades split the shadow distance in half.
#[test]
fn two_cascades_split_the_shadow_distance() {
    let dir = Vec4::new(0.0, -1.0, 0.0, 0.0);
    let (view, _p, inv, proj22, proj32) = test_camera(Vec3::ZERO, 60.0, 1.0, 0.1, 500.0);
    let (near, far, split) =
        shadow_cascade_matrices(&dir, &inv, &view, proj22, proj32, false, 0.1, 60.0, 2);
    assert!((split - 30.0).abs() < 1e-5);
    assert_ne!(near, far, "the two cascades must not share one box");

    let inside = |matrix: Mat4, point: Vec3| {
        let clip = matrix * point.extend(1.0);
        clip.z >= 0.0 && clip.z <= clip.w && clip.x.abs() <= clip.w && clip.y.abs() <= clip.w
    };
    assert!(
        inside(near, Vec3::new(0.0, 0.0, -15.0)),
        "near cascade covers 0..30"
    );
    assert!(
        inside(far, Vec3::new(0.0, 0.0, -45.0)),
        "far cascade covers 30..60"
    );

    let (one_near, one_far, _) =
        shadow_cascade_matrices(&dir, &inv, &view, proj22, proj32, false, 0.1, 60.0, 1);
    assert_eq!(one_near, one_far);
}

/// Multi-cascade culling keeps a caster that only the far cascade covers.
#[test]
fn shadow_culling_keeps_casters_inside_any_cascade() {
    use prism_render::culling::Aabb;

    let dir = Vec4::new(0.0, -1.0, 0.0, 0.0);
    let (view, _p, inv, proj22, proj32) = test_camera(Vec3::ZERO, 60.0, 1.0, 0.1, 500.0);
    let (near, far, _) =
        shadow_cascade_matrices(&dir, &inv, &view, proj22, proj32, false, 0.1, 60.0, 2);
    let mut items = vec![
        shadow_caster(Some(Aabb::new([-1.0; 3], [1.0; 3]))),
        shadow_caster(Some(Aabb::new([-1.0, -1.0, -46.0], [1.0, 1.0, -44.0]))),
        shadow_caster(Some(Aabb::new([499.0, -1.0, -1.0], [501.0, 1.0, 1.0]))),
    ];
    scene::systems::render::cull_shadow_casters_multi(
        &mut items,
        &[near.to_cols_array_2d(), far.to_cols_array_2d()],
    );
    assert!(items[0].cast_shadow, "near caster kept");
    assert!(items[1].cast_shadow, "far-only caster must not be culled");
    assert!(!items[2].cast_shadow, "outside both cascades is skipped");
}

/// Everything the camera can see maps to Vulkan depth in [0, 1].
#[test]
fn the_shadow_projection_stays_inside_vulkan_depth_range() {
    let dir = Vec4::new(-0.4, -1.0, -0.3, 0.0);
    let (_v, _p, inv, proj22, proj32) = test_camera(Vec3::ZERO, 60.0, 16.0 / 9.0, 0.1, 500.0);
    let corners = camera_frustum_corners(&inv, proj22, proj32, false, 0.1, 60.0);
    let light = shadow_matrix_fitting(&dir, &corners, 2048);
    for point in corners {
        let clip = light * point.extend(1.0);
        assert!(
            clip.z >= -1e-4 && clip.z <= clip.w + 1e-4,
            "{point:?} maps to depth {} outside [0,1]",
            clip.z / clip.w
        );
        assert!(clip.x.abs() <= clip.w + 1e-4, "{point:?} maps outside x");
        assert!(clip.y.abs() <= clip.w + 1e-4, "{point:?} maps outside y");
    }
}

fn shadow_caster(bounds: Option<prism_render::culling::Aabb>) -> DrawItem {
    DrawItem {
        mesh: Default::default(),
        entity_id: u32::MAX,
        model: Mat4::IDENTITY.to_cols_array_2d(),
        material: None,
        cast_shadow: true,
        visible_in_main_pass: true,
        cast_shadow_two_sided: false,
        bone_offset: None,
        splat_slots: [u32::MAX; 4],
        splat_tiling: 0.0,
        shader: None,
        state: Default::default(),
        world_bounds: bounds,
        index_start: 0,
        index_count: None,
    }
}

/// Casters outside light frustum no longer write shadow map; entities without AABB conservatively submitted (#526).
#[test]
fn shadow_casters_outside_the_light_box_are_skipped() {
    use prism_render::culling::Aabb;

    let dir = Vec4::new(0.0, -1.0, 0.0, 0.0);
    let (_v, _p, inv, proj22, proj32) = test_camera(Vec3::ZERO, 60.0, 16.0 / 9.0, 0.1, 500.0);
    let corners = camera_frustum_corners(&inv, proj22, proj32, false, 0.1, 60.0);
    let light = shadow_matrix_fitting(&dir, &corners, 2048);
    let mut items = vec![
        shadow_caster(Some(Aabb::new([-1.0; 3], [1.0; 3]))),
        shadow_caster(Some(Aabb::new([499.0, -1.0, -1.0], [501.0, 1.0, 1.0]))),
        shadow_caster(None),
    ];
    scene::systems::render::cull_shadow_casters(&mut items, &light.to_cols_array_2d());
    assert!(
        items[0].cast_shadow,
        "inside the shadow box must still cast"
    );
    assert!(!items[1].cast_shadow, "far outside the box must be skipped");
    assert!(
        items[2].cast_shadow,
        "unknown bounds must stay conservative"
    );
}

/// Degenerate matrix (no frustum available) culls nothing, avoid deleting whole scene shadow.
#[test]
fn an_invalid_light_frustum_disables_shadow_culling() {
    use prism_render::culling::Aabb;

    let mut items = vec![shadow_caster(Some(Aabb::new(
        [499.0, -1.0, -1.0],
        [501.0, 1.0, 1.0],
    )))];
    scene::systems::render::cull_shadow_casters(&mut items, &Mat4::ZERO.to_cols_array_2d());
    assert!(items[0].cast_shadow);
}

/// `LightSample.direction` is light propagation direction, UBO `lightDirection` should "point to light source".
#[test]
fn directional_ubo_direction_points_to_the_light() {
    let ubo = directional_light_ubo_direction(Vec3::Z, 2.0);
    assert!((ubo.truncate() - Vec3::NEG_Z).length() < 1e-6);
    assert_eq!(ubo.w, 2.0);
}

/// PR #38 review finding 1: viewport aspect must be written to **actually rendered** camera.
/// When main camera (higher `depth`) lacks `LocalToWorld`, rendered one is lower `depth`.
#[test]
fn viewport_aspect_lands_on_the_rendered_camera() {
    let mut world = World::new();

    let rendered = world.spawn();
    world.insert(
        rendered,
        Camera {
            depth: 0.0,
            ..Camera::default()
        },
    );
    world.insert(rendered, LocalToWorld(Mat4::IDENTITY));

    let transform_less = world.spawn();
    world.insert(
        transform_less,
        Camera {
            depth: 5.0,
            ..Camera::default()
        },
    );

    let changes = collect_scene_changes(&mut world, 2.5, &Mat4::IDENTITY);

    assert!(changes.has_camera);
    let rendered_aspect = world
        .get::<Camera>(rendered)
        .expect("rendered camera")
        .aspect;
    assert!(
        (rendered_aspect - 2.5).abs() < 1e-6,
        "Rendered camera should get viewport aspect, actual {rendered_aspect}"
    );
    let untouched = world
        .get::<Camera>(transform_less)
        .expect("transform-less camera")
        .aspect;
    assert!(
        (untouched - Camera::default().aspect).abs() < 1e-6,
        "Non-rendered camera should not have aspect overwritten, actual {untouched}"
    );
}

/// issue #10: PT primary ray must distinguish perspective/orthographic, so "orthographic flag" must also come from
/// **actually rendered** camera, not an arbitrary one.
#[test]
fn orthographic_flag_comes_from_the_rendered_camera() {
    let mut world = World::new();

    let rendered = world.spawn();
    world.insert(
        rendered,
        Camera {
            depth: 0.0,
            orthographic: true,
            ..Camera::default()
        },
    );
    world.insert(rendered, LocalToWorld(Mat4::IDENTITY));

    let transform_less = world.spawn();
    world.insert(
        transform_less,
        Camera {
            depth: 5.0,
            orthographic: false,
            ..Camera::default()
        },
    );

    let changes = collect_scene_changes(&mut world, 16.0 / 9.0, &Mat4::IDENTITY);
    assert!(changes.has_camera);
    assert!(
        changes.camera_orthographic,
        "Orthographic flag should follow actually rendered camera (one with LocalToWorld)"
    );

    // With no renderable camera falls back to fallback perspective camera.
    let mut empty = World::new();
    let fallback = collect_scene_changes(&mut empty, 16.0 / 9.0, &Mat4::IDENTITY);
    assert!(!fallback.has_camera);
    assert!(!fallback.camera_orthographic);
}

/// #468: local lights over `LIGHT_MAX` picked by importance, not document order.
///
/// Scene has 5 near bright lights + 5 far dim lights: kept must be 5 bright + 3 dim,
/// dropped only 2 farthest dim lights (ties by document order).
#[test]
fn only_the_most_important_local_lights_reach_the_ssbo() {
    use crate::scene::components::{LocalTransform, PointLight};
    use crate::scene::runtime_components::SceneEntityId;

    let mut world = World::new();
    let camera = world.spawn();
    world.insert(camera, Camera::default());
    world.insert(camera, LocalToWorld(Mat4::IDENTITY));

    for id in 1..=5u32 {
        let light = world.spawn();
        world.insert(
            light,
            PointLight {
                intensity: 100.0,
                range: 10.0,
                ..Default::default()
            },
        );
        world.insert(
            light,
            LocalTransform {
                position: Vec3::new(0.0, 0.0, -1.0),
                ..Default::default()
            },
        );
        world.insert(light, SceneEntityId(id));
    }
    for id in 6..=10u32 {
        let light = world.spawn();
        world.insert(
            light,
            PointLight {
                intensity: 1.0,
                range: 1.0,
                ..Default::default()
            },
        );
        world.insert(
            light,
            LocalTransform {
                position: Vec3::new(0.0, 0.0, -1000.0),
                ..Default::default()
            },
        );
        world.insert(light, SceneEntityId(id));
    }

    let changes = collect_scene_changes(&mut world, 16.0 / 9.0, &Mat4::IDENTITY);

    assert_eq!(
        changes.local_lights_total, 10,
        "All ten lights should be counted"
    );
    assert_eq!(
        changes.local_lights_kept,
        prism_render::LIGHT_MAX as usize,
        "SSBO capacity unchanged"
    );
    assert_eq!(changes.lights.len(), prism_render::LIGHT_MAX as usize);
    assert_eq!(
        changes.dropped_light_ids,
        vec![9, 10],
        "Dropped are farthest dim lights, not last two bright lights in document"
    );
}

/// Unified semantics: only `clearFlags == Skybox` draws skybox background.
///
/// Put an enabled `Skybox` entity in scene, switch camera clear mode one by one; only `Skybox`
/// makes `FramePacket.skybox` `Some`, others all `None` (`ForwardPass`
/// skips skybox on `None`).
#[test]
fn skybox_is_only_drawn_for_skybox_clear_flags() {
    use crate::scene::components::Skybox;

    for (flags, draws) in [
        (CameraClearFlags::Skybox, true),
        (CameraClearFlags::SolidColor, false),
        (CameraClearFlags::Depth, false),
        (CameraClearFlags::Nothing, false),
    ] {
        let mut world = World::new();

        let camera = world.spawn();
        world.insert(
            camera,
            Camera {
                clear_flags: flags,
                ..Camera::default()
            },
        );
        world.insert(camera, LocalToWorld(Mat4::IDENTITY));

        let sky = world.spawn();
        world.insert(
            sky,
            Skybox {
                hdr_path: "env.hdr".to_string(),
                enabled: true,
                ..Skybox::default()
            },
        );

        let packet = extract_frame_packet(&mut world, 1.0, &Mat4::IDENTITY);
        assert_eq!(packet.scene.clear_flags, flags);
        assert_eq!(
            packet.skybox.is_some(),
            draws,
            "Whether skybox is drawn for clear_flags = {flags:?} should be {draws}"
        );
    }
}

/// #66: `extract_frame_packet_into` must reuse caller data pack allocations,
/// otherwise each frame re-`collect()`s.
#[test]
fn extract_into_reuses_the_packet_allocations() {
    use crate::scene::components::Gizmo;

    let mut world = World::new();
    let camera = world.spawn();
    world.insert(camera, Camera::default());
    world.insert(camera, LocalToWorld(Mat4::IDENTITY));
    let gizmo = world.spawn();
    world.insert(
        gizmo,
        Gizmo {
            enabled: true,
            ..Gizmo::default()
        },
    );
    world.insert(gizmo, LocalToWorld(Mat4::IDENTITY));

    let capacities = |packet: &FramePacket| {
        [
            packet.draw_items.capacity(),
            packet.gizmos.capacity(),
            packet.sprite_items.capacity(),
            packet.sprite_lights.capacity(),
            packet.scene.lights.capacity(),
            packet.scene.pt_lights.capacity(),
            packet.ui_overlay.quads.capacity(),
            packet.ui_overlay.texts.capacity(),
            packet.ui_overlay.order.capacity(),
        ]
    };

    let mut packet = FramePacket::default();
    extract_frame_packet_into(&mut world, 1.0, &Mat4::IDENTITY, &mut packet);
    assert!(
        !packet.gizmos.is_empty(),
        "Test case needs at least one draw item"
    );
    let first = capacities(&packet);

    extract_frame_packet_into(&mut world, 1.0, &Mat4::IDENTITY, &mut packet);
    let second = capacities(&packet);
    assert_eq!(
        first, second,
        "Second extract should reuse same Vec capacity, not reallocate"
    );
    assert!(!packet.gizmos.is_empty());
}
