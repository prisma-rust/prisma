//! Schema and math-convention tests for `Camera` / `Light` (test module of `components.rs`).

use super::*;

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

fn assert_mat_eq(a: glam::Mat4, b: glam::Mat4) {
    let (a, b) = (a.to_cols_array(), b.to_cols_array());
    for i in 0..16 {
        assert!(approx(a[i], b[i]), "element {i}: {} != {}", a[i], b[i]);
    }
}

#[test]
fn perspective_matches_vulkan_convention() {
    let cam = Camera {
        fov_y_degrees: 90.0,
        near: 0.1,
        far: 100.0,
        ..Camera::default()
    };
    let aspect = 2.0;
    let p = cam.projection_matrix(aspect);
    let inv_tan = 1.0 / (45.0_f32.to_radians()).tan();

    assert!(approx(p.col(0).x, inv_tan / aspect));
    // Vulkan y-flip: p[1][1] is negative.
    assert!(approx(p.col(1).y, -inv_tan));
    assert!(approx(p.col(2).z, cam.far / (cam.near - cam.far)));
    assert!(approx(p.col(2).w, -1.0));
    assert!(approx(
        p.col(3).z,
        cam.near * cam.far / (cam.near - cam.far)
    ));
}

#[test]
fn perspective_maps_near_to_zero_and_far_to_one() {
    let cam = Camera {
        near: 0.5,
        far: 50.0,
        ..Camera::default()
    };
    let p = cam.projection_matrix(1.0);
    for (dist, expected) in [(cam.near, 0.0), (cam.far, 1.0)] {
        let clip = p * glam::vec4(0.0, 0.0, -dist, 1.0);
        let ndc_z = clip.z / clip.w;
        assert!((ndc_z - expected).abs() < 1e-4, "z={dist} → {ndc_z}");
    }
}

#[test]
fn orthographic_maps_half_sizes_and_depth() {
    let cam = Camera {
        orthographic: true,
        orthographic_size: 2.0,
        near: 0.5,
        far: 9.5,
        ..Camera::default()
    };
    let p = cam.projection_matrix(2.0);

    // half height 2 → NDC 1/2; after y-flip the top edge (y=+2) lands at NDC y = -1.
    assert!(approx(p.col(1).y, -0.5));
    // half width = half height × aspect = 4.
    assert!(approx(p.col(0).x, 0.25));

    let top = p * glam::vec4(0.0, 2.0, -1.0, 1.0);
    assert!(approx(top.y / top.w, -1.0));
    let right = p * glam::vec4(4.0, 0.0, -1.0, 1.0);
    assert!(approx(right.x / right.w, 1.0));

    for (dist, expected) in [(cam.near, 0.0), (cam.far, 1.0)] {
        let clip = p * glam::vec4(0.0, 0.0, -dist, 1.0);
        let ndc_z = clip.z / clip.w;
        assert!((ndc_z - expected).abs() < 1e-4, "z={dist} → {ndc_z}");
    }
}

#[test]
fn degenerate_aspect_falls_back_to_one() {
    let cam = Camera::default();
    let sane = cam.projection_matrix(1.0);
    assert_mat_eq(cam.projection_matrix(0.0), sane);
    assert_mat_eq(cam.projection_matrix(f32::NAN), sane);
}

#[test]
fn world_to_camera_inverts_camera_to_world() {
    let camera_to_world = glam::Mat4::from_translation(glam::Vec3::new(1.0, 2.0, 3.0));
    let world_to_camera = Camera::world_to_camera_matrix(camera_to_world);
    assert_mat_eq(camera_to_world * world_to_camera, glam::Mat4::IDENTITY);
    assert_mat_eq(
        Camera::camera_to_world_matrix(world_to_camera),
        camera_to_world,
    );
}

#[test]
fn camera_defaults() {
    let cam = Camera::default();
    assert_eq!(cam.clear_flags, CameraClearFlags::Skybox);
    assert!(!cam.orthographic);
    assert!(approx(cam.orthographic_size, 5.0));
    assert!(approx(cam.depth, 0.0));
    assert!(approx(cam.background_color[3], 1.0));
}

#[test]
fn json_round_trips_new_fields() {
    let cam = Camera {
        clear_flags: CameraClearFlags::SolidColor,
        background_color: [0.1, 0.2, 0.3, 1.0],
        orthographic: true,
        orthographic_size: 7.5,
        depth: 2.0,
        ..Camera::default()
    };
    let json = serde_json::to_string(&cam).unwrap();
    let back: Camera = serde_json::from_str(&json).unwrap();
    assert_eq!(back.clear_flags, CameraClearFlags::SolidColor);
    assert!(back.orthographic);
    assert!(approx(back.orthographic_size, 7.5));
    assert!(approx(back.depth, 2.0));
    assert!(approx(back.background_color[1], 0.2));
}

#[test]
fn older_json_without_new_fields_uses_defaults() {
    // Document with only old fields (pre-v2 fast-path camera) must still load — new fields use serde defaults.
    let json = r#"{"fov_y_degrees":72.0,"near":0.2,"far":500.0,"exposure":1.5,"aspect":1.0,"enabled":true}"#;
    let cam: Camera = serde_json::from_str(json).unwrap();
    assert!(approx(cam.fov_y_degrees, 72.0));
    assert_eq!(cam.clear_flags, CameraClearFlags::Skybox);
    assert!(!cam.orthographic);
}

#[test]
fn virtual_camera_json_round_trips_and_fills_defaults() {
    let camera = VirtualCamera2D {
        target_name: "player".to_string(),
        damping: [8.0, 4.0],
        deadzone: [2.0, 1.0],
        confiner: Some([0.0, 0.0, 320.0, 180.0]),
        follow_offset: [0.0, 3.0],
        pixel_snap: true,
        pixels_per_unit: 16.0,
    };
    let json = serde_json::to_string(&camera).unwrap();
    let back: VirtualCamera2D = serde_json::from_str(&json).unwrap();
    assert_eq!(back.target_name, "player");
    assert_eq!(back.confiner, Some([0.0, 0.0, 320.0, 180.0]));
    assert!(back.pixel_snap);
    let old: VirtualCamera2D = serde_json::from_str("{}").unwrap();
    assert!(old.target_name.is_empty());
    assert_eq!(old.damping, [0.0, 0.0]);
}

// ---------------------------------------------------------------------------
// Light
// ---------------------------------------------------------------------------

#[test]
fn light_defaults() {
    let light = Light::default();
    assert!(light.enabled);
    // `new Light()` defaults to point light; cone angles and color temperature use engine defaults.
    assert_eq!(light.light_type, LightType::Point);
    assert_eq!(light.shadows, LightShadows::None);
    assert_eq!(light.render_mode, LightRenderMode::Auto);
    assert!(approx(light.spot_angle, 30.0));
    assert!(approx(light.inner_spot_angle, 21.8));
    assert!(approx(light.color_temperature, 6570.0));
    assert!(!light.use_color_temperature);
    assert!(approx(light.shadow_strength, 1.0));
    assert!(!light.is_shadow_caster());
}

#[test]
fn light_json_round_trips_and_fills_missing_fields() {
    let light = Light {
        light_type: LightType::Spot,
        shadows: LightShadows::Soft,
        use_color_temperature: true,
        color_temperature: 3200.0,
        ..Light::default()
    };
    let json = serde_json::to_string(&light).unwrap();
    let back: Light = serde_json::from_str(&json).unwrap();
    assert_eq!(back.light_type, LightType::Spot);
    assert_eq!(back.shadows, LightShadows::Soft);
    assert!(back.use_color_temperature);

    // Document containing only known fields must also serialize correctly (`#[serde(default)]`).
    let partial: Light =
        serde_json::from_str(r#"{"light_type":"Directional","intensity":3.0}"#).unwrap();
    assert_eq!(partial.light_type, LightType::Directional);
    assert!(approx(partial.intensity, 3.0));
    assert_eq!(partial.shadows, LightShadows::None);
    assert!(approx(partial.range, 10.0));
}

#[test]
fn light_2d_defaults_and_round_trips() {
    // #611: 2D lighting uses "artistic units" — intensity 1.0 = original color.
    let light = Light2D::default();
    assert_eq!(light.kind, Light2DKind::Point);
    assert_eq!(light.color, [1.0, 1.0, 1.0]);
    assert!(approx(light.intensity, 1.0));
    assert!(approx(light.radius, 5.0));
    assert_eq!(light.direction, [0.0, -1.0]);
    assert!(light.enabled);
    assert!(light.contributes());

    let authored: Light2D =
        serde_json::from_str(r#"{"kind":"Ambient","color":[0.2,0.25,0.5],"intensity":0.35}"#)
            .unwrap();
    assert_eq!(authored.kind, Light2DKind::Ambient);
    assert_eq!(authored.color, [0.2, 0.25, 0.5]);
    assert!(approx(authored.intensity, 0.35));
    assert_eq!(authored.radius, 5.0, "Unwritten fields use default");

    let json = serde_json::to_string(&authored).unwrap();
    let back: Light2D = serde_json::from_str(&json).unwrap();
    assert_eq!(back.kind, Light2DKind::Ambient);
    assert!(approx(back.intensity, 0.35));
}

#[test]
fn light_2d_ignores_disabled_or_degenerate_values() {
    let mut light = Light2D {
        enabled: false,
        ..Light2D::default()
    };
    assert!(!light.contributes(), "Disabled light does not contribute");

    light.enabled = true;
    light.intensity = 0.0;
    assert!(!light.contributes(), "0 intensity = no contribution");
    assert_eq!(light.strength(), 0.0);

    light.intensity = f32::NAN;
    assert!(!light.contributes());
    assert_eq!(light.strength(), 0.0);

    light.intensity = 2.0;
    light.color = [0.0, 0.0, 0.0];
    assert!(
        !light.contributes(),
        "Black light does not illuminate anything"
    );

    light.color = [1.0, 1.0, 1.0];
    light.radius = 0.0;
    assert!(light.contributes());
    assert_eq!(light.falloff_radius(), 0.0, "Radius 0 = no falloff");
    light.radius = f32::INFINITY;
    assert_eq!(light.falloff_radius(), 0.0);
    light.radius = -3.0;
    assert_eq!(light.falloff_radius(), 0.0);
    light.radius = 2.5;
    assert!(approx(light.falloff_radius(), 2.5));
}

#[test]
fn light_2d_kind_exposes_reflect_variants() {
    use prism_reflect::ReflectType;
    assert_eq!(
        Light2DKind::enum_variants(),
        Some(&["Ambient", "Point", "Directional"][..])
    );
    for (index, kind) in [
        Light2DKind::Ambient,
        Light2DKind::Point,
        Light2DKind::Directional,
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(kind.to_value(), prism_reflect::Value::U64(index as u64));
        assert_eq!(Light2DKind::from_value(kind.to_value()).unwrap(), kind);
    }
    assert!(Light2DKind::from_value(prism_reflect::Value::U64(9)).is_err());
}

#[test]
fn sprites_stay_unlit_unless_receive_lights_is_enabled() {
    // #611: default does not participate in lighting, old scenes' palette unchanged.
    let legacy: SpriteRenderer = serde_json::from_str(r#"{"sprite_path":"a.png"}"#).unwrap();
    assert!(!legacy.receive_lights);

    let lit = SpriteRenderer {
        sprite_path: "a.png".into(),
        receive_lights: true,
        ..SpriteRenderer::default()
    };
    let json = serde_json::to_string(&lit).unwrap();
    let back: SpriteRenderer = serde_json::from_str(&json).unwrap();
    assert!(back.receive_lights);
}

#[test]
fn light_effective_color_applies_temperature_only_when_enabled() {
    let mut light = Light {
        color: glam::Vec3::new(1.0, 0.5, 0.25),
        color_temperature: 2000.0,
        ..Light::default()
    };
    // Color temperature disabled: return as-is.
    assert_eq!(light.effective_color(), light.color);

    light.use_color_temperature = true;
    let tinted = light.effective_color();
    // 2000 K is warm: red component kept, blue visibly reduced.
    assert!(approx(tinted.x, light.color.x));
    assert!(
        tinted.z < light.color.z,
        "Color temperature filter should reduce blue: {tinted:?}"
    );
}

#[test]
fn light_cone_half_angles_are_derived_from_full_angles() {
    let light = Light {
        light_type: LightType::Spot,
        // Engine's 60° full cone = 30° half cone.
        spot_angle: 60.0,
        inner_spot_angle: 20.0,
        ..Light::default()
    };
    let (inner, outer) = light.cone_half_angles_radians();
    assert!(approx(outer, 30.0_f32.to_radians()));
    assert!(approx(inner, 10.0_f32.to_radians()));

    // When inner cone larger than outer cone, clamp to outer cone, no negative cosine interval.
    let broken = Light {
        spot_angle: 20.0,
        inner_spot_angle: 90.0,
        ..Light::default()
    };
    let (inner, outer) = broken.cone_half_angles_radians();
    assert!(inner <= outer);
}

#[test]
fn light_shadow_caster_requires_enabled_and_shadow_mode() {
    let light = Light {
        shadows: LightShadows::Hard,
        ..Light::default()
    };
    assert!(light.is_shadow_caster());

    let disabled = Light {
        enabled: false,
        ..light
    };
    assert!(!disabled.is_shadow_caster());
}

// ---------------------------------------------------------------------------
// SpriteRenderer (2D sprite)
// ---------------------------------------------------------------------------

#[test]
fn sprite_defaults() {
    let sprite = SpriteRenderer::default();
    assert!(!sprite.has_sprite());
    assert_eq!(sprite.frame_size, [0, 0]);
    assert_eq!(sprite.frame_index, 0);
    assert!(approx(sprite.pixels_per_unit, 100.0));
    assert_eq!(sprite.pivot, [0.5, 0.5]);
    assert_eq!(sprite.color, [1.0, 1.0, 1.0, 1.0]);
    assert!(!sprite.flip_x && !sprite.flip_y);
    assert_eq!(sprite.sorting_layer, 0);
    assert_eq!(sprite.order_in_layer, 0);
}

#[test]
fn sprite_json_round_trips_and_fills_missing_fields() {
    let sprite = SpriteRenderer {
        sprite_path: "sprites/boat.png".into(),
        frame_size: [32, 16],
        sheet_columns: 4,
        sheet_row: 1,
        frame_index: 3,
        pixels_per_unit: 16.0,
        pivot: [0.0, 1.0],
        color: [0.5, 1.0, 0.25, 0.75],
        flip_x: true,
        flip_y: false,
        sorting_layer: 2,
        order_in_layer: -1,
        src_blend: BlendMode::SrcAlpha,
        dst_blend: BlendMode::One,
        blend_op: BlendOp::Add,
        clip_enabled: true,
        clip_rect: [-4.0, -2.0, 4.0, 2.0],
        receive_lights: true,
    };
    let json = serde_json::to_string(&sprite).unwrap();
    let back: SpriteRenderer = serde_json::from_str(&json).unwrap();
    assert_eq!(back.sprite_path, "sprites/boat.png");
    assert_eq!(back.frame_size, [32, 16]);
    assert_eq!(back.sheet_columns, 4);
    assert_eq!(back.sheet_row, 1);
    assert_eq!(back.frame_index, 3);
    assert!(approx(back.pixels_per_unit, 16.0));
    assert_eq!(back.pivot, [0.0, 1.0]);
    assert!(back.flip_x);
    assert_eq!(back.sorting_layer, 2);
    assert_eq!(back.order_in_layer, -1);
    assert_eq!(back.src_blend, BlendMode::SrcAlpha);
    assert_eq!(back.dst_blend, BlendMode::One);
    assert_eq!(back.blend_op, BlendOp::Add);
    assert!(back.clip_enabled);
    assert_eq!(back.clip_rect, [-4.0, -2.0, 4.0, 2.0]);
    assert!(back.receive_lights);

    // Old document with only path: remaining fields use serde defaults.
    let legacy: SpriteRenderer = serde_json::from_str(r#"{"sprite_path":"a.png"}"#).unwrap();
    assert_eq!(legacy.sprite_path, "a.png");
    assert!(approx(legacy.pixels_per_unit, 100.0));
    assert_eq!(legacy.frame_size, [0, 0]);
    assert_eq!(
        legacy.sheet_columns, 0,
        "Old document treated as single-row atlas"
    );
    assert_eq!(legacy.sheet_row, 0);
    assert_eq!(
        legacy.src_blend,
        BlendMode::SrcAlpha,
        "Old document defaults to alpha blending"
    );

    // #582: clipping disabled by default, old documents not suddenly clipped.
    assert!(!legacy.clip_enabled);
    assert_eq!(legacy.clip_rect, [0.0; 4]);
}

#[test]
fn sprite_clip_rect_round_trips() {
    let sprite = SpriteRenderer {
        sprite_path: "sprites/reflection.png".into(),
        clip_enabled: true,
        clip_rect: [-100.0, -50.0, 100.0, 12.5],
        ..Default::default()
    };
    let json = serde_json::to_string(&sprite).unwrap();
    let back: SpriteRenderer = serde_json::from_str(&json).unwrap();
    assert!(back.clip_enabled);
    assert_eq!(back.clip_rect, [-100.0, -50.0, 100.0, 12.5]);
}

#[test]
fn tilemap_json_defaults_and_round_trips() {
    // #529: old documents have no Tilemap, new fields all use serde defaults.
    let map: Tilemap = serde_json::from_str(
        r#"{"tileset_path":"sprites/tiles.png","columns":2,"tiles":[0,3,0,1]}"#,
    )
    .unwrap();
    assert_eq!(map.tile_size, [16, 16], "Default tile 16x16");
    assert_eq!(map.cell_size, [1.0, 1.0]);
    assert_eq!(map.color, [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(map.sorting_layer, 0);
    assert!(map.has_tiles());

    let json = serde_json::to_string(&map).unwrap();
    let back: Tilemap = serde_json::from_str(&json).unwrap();
    assert_eq!(back.tiles, vec![0, 3, 0, 1]);
    assert_eq!(back.columns, 2);

    assert_eq!(Tilemap::default().columns, 0, "Default single row");
    assert!(
        !Tilemap::default().has_tiles(),
        "No tileset path means no drawing"
    );
}

#[test]
fn tilemap_grid_and_uv_follow_the_tileset_layout() {
    let mut map = Tilemap {
        tiles: vec![1, 0, 2, 3, 0],
        columns: 2,
        tile_size: [16, 16],
        ..Tilemap::default()
    };
    // 2 columns → 3 rows (last row has only one cell).
    assert_eq!(map.grid(), (2, 3));

    map.columns = 0;
    assert_eq!(map.grid(), (5, 1), "columns=0 arranged in one row");

    // tileset 64x32 + 16x16 tile = 4 columns 2 rows.
    assert_eq!(map.tileset_grid([64, 32]), (4, 2));
    assert_eq!(map.tile_uv(0, [64, 32]), [0.0, 0.0, 0.25, 0.5]);
    assert_eq!(map.tile_uv(5, [64, 32]), [0.25, 0.5, 0.5, 1.0]);
    // Index beyond tileset clamped to last tile.
    assert_eq!(map.tile_uv(99, [64, 32]), [0.75, 0.5, 1.0, 1.0]);

    // Row 0 at top: larger row number means smaller y.
    assert_eq!(map.cell_center(0, 0, 2), [0.5, 1.5]);
    assert_eq!(map.cell_center(1, 1, 2), [1.5, 0.5]);
}

#[test]
fn tilemap_rejects_invalid_cell_sizes() {
    let map = Tilemap {
        tileset_path: "tiles.png".into(),
        tiles: vec![1],
        cell_size: [0.0, 1.0],
        ..Tilemap::default()
    };
    assert!(!map.has_tiles(), "cell_size 0 cannot draw");
}

#[test]
fn sprite_world_size_uses_frame_or_texture_and_ppu() {
    // Entire texture 64x32, PPU 16 -> 4x2 world units.
    let whole = SpriteRenderer {
        pixels_per_unit: 16.0,
        ..SpriteRenderer::default()
    };
    assert_eq!(whole.world_size([64, 32]), [4.0, 2.0]);

    // sprite sheet single frame takes priority over texture size.
    let sheet = SpriteRenderer {
        frame_size: [32, 32],
        pixels_per_unit: 32.0,
        ..SpriteRenderer::default()
    };
    assert_eq!(sheet.frame_size_or([256, 32]), [32, 32]);
    assert_eq!(sheet.world_size([256, 32]), [1.0, 1.0]);

    // Invalid PPU falls back to 100, no NaN/Inf.
    let broken = SpriteRenderer {
        pixels_per_unit: 0.0,
        ..SpriteRenderer::default()
    };
    assert_eq!(broken.world_size([100, 100]), [1.0, 1.0]);
}

#[test]
fn sprite_and_collider_bounds_share_pivot_and_transform_scale() {
    let transform = LocalTransform {
        position: glam::Vec3::new(3.0, 4.0, 0.0),
        scale: glam::Vec3::new(2.0, 3.0, 1.0),
        ..LocalTransform::default()
    };
    let sprite = SpriteRenderer {
        frame_size: [10, 20],
        pixels_per_unit: 1.0,
        pivot: [0.0, 0.0],
        ..SpriteRenderer::default()
    };
    assert_eq!(
        sprite.world_bounds(&transform, [10, 20]).unwrap(),
        prism_physics2d::Aabb2::new(3.0, 4.0, 23.0, 64.0)
    );

    let collider = Collider2D {
        offset: [1.0, -2.0],
        half_extents: [2.0, 3.0],
        ..Collider2D::default()
    };
    assert_eq!(
        collider.world_bounds(&transform).unwrap(),
        prism_physics2d::Aabb2::new(1.0, -11.0, 9.0, 7.0)
    );
}

#[test]
fn collider_bounds_accept_a_composed_parent_world_matrix() {
    let parent = glam::Mat4::from_scale_rotation_translation(
        glam::Vec3::new(2.0, 1.0, 1.0),
        glam::Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
        glam::Vec3::new(10.0, 20.0, 0.0),
    );
    let child = LocalTransform {
        position: glam::Vec3::new(3.0, 4.0, 0.0),
        ..LocalTransform::default()
    };
    let collider = Collider2D {
        offset: [1.0, -2.0],
        half_extents: [2.0, 1.0],
        ..Collider2D::default()
    };

    let bounds = collider
        .world_bounds_with_matrix(parent * child.to_model_matrix())
        .unwrap();
    for (actual, expected) in bounds
        .min
        .into_iter()
        .chain(bounds.max)
        .zip([7.0, 24.0, 9.0, 32.0])
    {
        assert!((actual - expected).abs() < 1e-5);
    }
}

#[test]
fn mesh_renderer_state_defaults_and_round_trips() {
    // Old document with only mesh/material: all new fields must fall back to engine's opaque default.
    let legacy: MeshRenderer =
        serde_json::from_str(r#"{"mesh_path":"m.mesh","material_path":"m.mat"}"#)
            .expect("Old JSON missing render state fields is still readable");
    assert_eq!(legacy.cull_mode, CullMode::Back);
    assert_eq!(legacy.shadow_casting_mode, ShadowCastingMode::On);
    assert_eq!(legacy.render_queue.value(), RenderQueue::GEOMETRY);
    assert!(legacy.z_write);
    assert_eq!(legacy.src_blend, BlendMode::One);
    assert_eq!(legacy.dst_blend, BlendMode::Zero);
    assert_eq!(legacy.blend_op, BlendOp::Add);
    // One / Zero / Add = opaque.
    assert!(!legacy.blends());
    assert!(legacy.material_paths.is_empty());
    assert_eq!(
        legacy.material_path_for_submesh(3, 4, None),
        Some("m.mat"),
        "Fall back to singular field when material_paths missing"
    );

    // Author format uses variant names (consistent with `CameraClearFlags` / `TextAlign`),
    // queue is numeric (`Material.renderQueue` int form).
    let json = serde_json::to_value(MeshRenderer {
        mesh_path: "m.mesh".into(),
        material_path: "m.mat".into(),
        material_paths: Vec::new(),
        shader_path: String::new(),
        shadow_casting_mode: ShadowCastingMode::Off,
        cull_mode: CullMode::Off,
        render_queue: RenderQueue::new(RenderQueue::TRANSPARENT),
        z_write: false,
        src_blend: BlendMode::SrcAlpha,
        dst_blend: BlendMode::OneMinusSrcAlpha,
        blend_op: BlendOp::Add,
    })
    .unwrap();
    assert_eq!(json["cull_mode"], serde_json::json!("Off"));
    assert_eq!(json["render_queue"], serde_json::json!(3000));
    assert_eq!(json["src_blend"], serde_json::json!("SrcAlpha"));
    assert_eq!(json["z_write"], serde_json::json!(false));

    let back: MeshRenderer = serde_json::from_value(json).unwrap();
    assert_eq!(back.cull_mode, CullMode::Off);
    assert_eq!(back.shadow_casting_mode, ShadowCastingMode::Off);
    assert_eq!(back.render_queue.value(), RenderQueue::TRANSPARENT);
    assert!(!back.z_write);
    assert_eq!(back.dst_blend, BlendMode::OneMinusSrcAlpha);
    // Standard alpha blend = transparent.
    assert!(back.blends());
}

#[test]
fn mesh_renderer_resolves_material_paths_per_submesh() {
    let renderer = MeshRenderer {
        material_path: "materials/fallback.mat".into(),
        material_paths: vec![
            "materials/skin.mat".into(),
            String::new(),
            "materials/metal.mat".into(),
        ],
        ..MeshRenderer::default()
    };

    // One-to-one correspondence uses submesh lower bound.
    assert_eq!(
        renderer.material_path_for_submesh(0, 3, Some(2)),
        Some("materials/skin.mat")
    );
    // Empty string does not masquerade as material, continue matching by glTF material index.
    assert_eq!(
        renderer.material_path_for_submesh(1, 3, Some(2)),
        Some("materials/metal.mat")
    );
    // When not found fall back to singular field.
    assert_eq!(
        renderer.material_path_for_submesh(1, 3, None),
        Some("materials/fallback.mat")
    );

    // When length is not submesh count, match by source material index (Unity shared-material usage).
    let shared = MeshRenderer {
        material_paths: vec!["materials/a.mat".into(), "materials/b.mat".into()],
        ..MeshRenderer::default()
    };
    assert_eq!(
        shared.material_path_for_submesh(0, 8, Some(1)),
        Some("materials/b.mat")
    );
    assert!(shared.has_material());
}

#[test]
fn particle_system_round_trips_with_variant_names() {
    let system = ParticleSystem {
        emission_rate: 12.0,
        burst: 3,
        duration: 2.0,
        lifetime: [0.5, 1.5],
        shape: ParticleShape::Sphere,
        shape_radius: 0.75,
        ..ParticleSystem::default()
    };

    // Author format uses variant names (consistent with `CameraClearFlags` / `TextAlign`).
    let json = serde_json::to_value(&system).unwrap();
    assert_eq!(json["shape"], serde_json::json!("Sphere"));
    assert_eq!(json["emission_rate"], serde_json::json!(12.0));

    let back: ParticleSystem = serde_json::from_value(json).unwrap();
    assert_eq!(back.shape, ParticleShape::Sphere);
    assert_eq!(back.burst, 3);
    assert_eq!(back.lifetime, [0.5, 1.5]);
}

#[test]
fn a_particle_system_without_fields_falls_back_to_the_defaults() {
    // Missing fields keep defaults (cross-version compatibility).
    let system: ParticleSystem = serde_json::from_str("{}").unwrap();
    assert_eq!(system.shape, ParticleShape::Cone);
    assert!(system.playing);
    assert_eq!(system.particle_cap(), 256);
    assert_eq!(system.emission_duration(), None);
    assert_eq!(ParticleSystem::random_range([2.0, 1.0], 0.25, 0.0), 1.25);
    assert_eq!(
        ParticleSystem::random_range([f32::NAN, 1.0], 0.25, 7.0),
        7.0
    );
}

#[test]
fn audio_sources_decay_with_distance_and_blend_toward_2d() {
    let source = AudioSource {
        sound_path: "audio/footstep.ogg".into(),
        volume: 0.8,
        spatial_blend: 1.0,
        min_distance: 2.0,
        max_distance: 10.0,
        ..AudioSource::default()
    };
    assert!(source.has_sound());
    assert_eq!(
        source.volume_at(0.0),
        0.8,
        "No falloff within near distance"
    );
    assert_eq!(source.volume_at(2.0), 0.8, "min_distance still full volume");
    assert!(
        (source.volume_at(6.0) - 0.4).abs() < 1e-6,
        "Linear falloff at midpoint"
    );
    assert_eq!(
        source.volume_at(10.0),
        0.0,
        "Silent from max_distance onward"
    );
    assert_eq!(source.volume_at(1_000.0), 0.0);
    assert_eq!(source.volume_at(f32::NAN), 0.0);

    // Pure 2D: distance has no effect.
    let flat = AudioSource {
        spatial_blend: 0.0,
        ..source.clone()
    };
    assert_eq!(flat.volume_at(0.0), 0.8);
    assert_eq!(flat.volume_at(9_999.0), 0.8);

    // Half blend: falloff only applies halfway.
    let half = AudioSource {
        spatial_blend: 0.5,
        ..source.clone()
    };
    assert!((half.volume_at(6.0) - 0.6).abs() < 1e-6);

    // Degenerate range / out-of-range volume will not produce >1 or NaN.
    let broken = AudioSource {
        volume: 4.0,
        min_distance: 5.0,
        max_distance: 1.0,
        ..source
    };
    assert_eq!(broken.volume_at(3.0), 1.0);
    assert_eq!(AudioSource::default().volume_at(0.0), 1.0);
    assert!(!AudioSource::default().has_sound());
}

#[test]
fn line_json_defaults_to_miter_join_and_butt_caps() {
    // #604: old document has only points / width, new fields must be serde defaults.
    let line: LineRenderer =
        serde_json::from_str(r#"{"points":[[0,0,0],[1,0,0]],"width":2}"#).unwrap();
    assert_eq!(line.join, LineJoin::Miter);
    assert_eq!(line.cap, LineCap::Butt);
    assert!((line.miter_limit - 4.0).abs() < 1e-6);

    let text = serde_json::to_string(&LineRenderer {
        join: LineJoin::Bevel,
        cap: LineCap::Round,
        ..line
    })
    .unwrap();
    assert!(text.contains("\"Bevel\""), "{text}");
    assert!(text.contains("\"Round\""), "{text}");
}

#[test]
fn particle_emitter_2d_json_round_trips_and_defaults() {
    let emitter = ParticleEmitter2D {
        emission_rate: 120.0,
        burst: 3,
        shape: ParticleShape2D::Box,
        shape_size: [4.0, 0.5],
        direction: [1.0, -1.0],
        spread_degrees: 25.0,
        sorting_layer: 7,
        start_color: [0.2, 0.4, 1.0, 0.9],
        end_color: [0.2, 0.4, 1.0, 0.0],
        ..ParticleEmitter2D::default()
    };
    let json = serde_json::to_string(&emitter).unwrap();
    let back: ParticleEmitter2D = serde_json::from_str(&json).unwrap();
    assert!(approx(back.emission_rate, 120.0));
    assert_eq!(back.burst, 3);
    assert_eq!(back.shape, ParticleShape2D::Box);
    assert_eq!(back.shape_size, [4.0, 0.5]);
    assert_eq!(back.sorting_layer, 7);

    // Old document / only one field written: rest use serde defaults.
    let legacy: ParticleEmitter2D = serde_json::from_str(r#"{"emission_rate": 5.0}"#).unwrap();
    assert!(approx(legacy.emission_rate, 5.0));
    assert_eq!(legacy.shape, ParticleShape2D::Point);
    assert!(legacy.playing);
    assert!(approx(legacy.direction[1], -1.0), "Default emits downward");
    assert_eq!(legacy.particle_cap(), 256, "0 = built-in cap");

    // Degenerate direction / non-finite value falls back to downward, not NaN velocity.
    let broken = ParticleEmitter2D {
        direction: [0.0, 0.0],
        ..ParticleEmitter2D::default()
    };
    assert_eq!(broken.direction_or_down(), [0.0, -1.0]);
    assert!(!broken.rate().is_nan());
}

#[test]
fn particle_shape_2d_exposes_enum_variants_for_the_editor() {
    use prism_reflect::ReflectType;
    assert_eq!(
        ParticleShape2D::enum_variants(),
        Some(&["Point", "Line", "Box", "Circle"][..])
    );
    for (index, shape) in [
        ParticleShape2D::Point,
        ParticleShape2D::Line,
        ParticleShape2D::Box,
        ParticleShape2D::Circle,
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(shape.to_value(), prism_reflect::Value::U64(index as u64));
        assert_eq!(
            ParticleShape2D::from_value(shape.to_value()).unwrap(),
            shape
        );
    }
    assert!(ParticleShape2D::from_value(prism_reflect::Value::U64(9)).is_err());
}
