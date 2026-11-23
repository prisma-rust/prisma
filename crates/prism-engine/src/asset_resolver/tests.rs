use super::decoders::{deinterleave_rmes, texture_format_for};
use super::*;
use crate::scene::components::{LocalToWorld, SceneAssetId};
use prism_asset::formats::*;
use prism_render::asset_bridge::AssetResolveResult;
use prism_render::managers::MeshHandle;
use prism_render::managers::TextureFormat;

/// Build a `.pak` + path manifest containing only the given `(path, type, bytes)` (for #583 tests).
fn tiny_pak(entries: &[(&str, prism_asset::core::AssetType, &[u8])]) -> (Vec<u8>, String) {
    use prism_asset::core::id::AssetIdGenerator;
    use prism_asset::package::PackageBuilder;

    let mut builder = PackageBuilder::new();
    let mut ids = AssetIdGenerator::new(1, 0);
    let mut manifest = Vec::new();
    for (path, asset_type, bytes) in entries {
        let id = ids.next();
        builder.add_asset(id, *asset_type, bytes.to_vec(), &[]);
        manifest.push(format!(
            "{{\"id\":\"0x{:016x}\",\"path\":\"{path}\"}}",
            id.into_raw()
        ));
    }
    let pak = builder.build().expect("build pak");
    let manifest = format!("{{\"assets\":[{}]}}", manifest.join(","));
    (pak, manifest)
}

#[test]
fn project_fonts_are_loaded_once_by_path() {
    // #583: fonts referenced by `Text.font` are read from `.pak` and handed to renderer for registration.
    let (pak, manifest) = tiny_pak(&[(
        "fonts/hud.ttf",
        prism_asset::core::AssetType::Font,
        b"not-a-real-ttf",
    )]);
    let mut resolver = GpuAssetResolver::new();
    resolver
        .load_resource_package_bytes(&pak, Some(&manifest))
        .expect("load pak");

    let mut world = World::new();
    let entity = world.spawn();
    world.insert(
        entity,
        crate::ui::components::Text {
            content: "HUD".into(),
            font: "fonts/hud.ttf".into(),
            ..Default::default()
        },
    );

    let fonts = resolver.prepare_ui_fonts(&world);
    assert_eq!(fonts.len(), 1);
    assert_eq!(fonts[0].0, "fonts/hud.ttf");
    assert_eq!(fonts[0].1, b"not-a-real-ttf");

    // Same path not re-read repeatedly; text without a font produces no request.
    assert!(resolver.prepare_ui_fonts(&world).is_empty());
    world
        .get_mut::<crate::ui::components::Text>(entity)
        .unwrap()
        .font = "fonts/nope.ttf".into();
    assert!(
        resolver.prepare_ui_fonts(&world).is_empty(),
        "Missing font reported once, no bytes returned"
    );
    assert!(resolver.prepare_ui_fonts(&world).is_empty());
}

/// #509: optional packages are mounted lazily — an entity parsed before a mount failure must be able to retry after
/// mount, otherwise the "report same bad path only once" failure cache would block them forever.
#[test]
fn failed_entities_retry_after_an_optional_package_is_mounted() {
    let mut resolver = GpuAssetResolver::new();
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(
        entity,
        SpriteRenderer {
            sprite_path: "textures/late.png".to_string(),
            ..Default::default()
        },
    );

    // Simulate the failure before lazy package is mounted.
    resolver.record_failure(entity, AssetSlot::Sprite, "textures/late.png");
    assert_eq!(
        resolver.retry_failed_after_package_change(&mut world),
        0,
        "the path still has no package, so nothing may retry"
    );
    assert!(!resolver.failed.is_empty());

    let (pak, manifest) = tiny_pak(&[(
        "textures/late.png",
        prism_asset::core::AssetType::Texture,
        b"rtex",
    )]);
    resolver
        .load_resource_package_bytes(&pak, Some(&manifest))
        .expect("load optional pak");

    assert_eq!(resolver.retry_failed_after_package_change(&mut world), 1);
    assert!(
        resolver.failed.is_empty(),
        "the now-resolvable path must leave the failure cache"
    );
}

/// #495: handle accounting / release. Returns a sprite entity already "uploaded via callback".
fn resolved_sprite(world: &mut World, resolver: &mut GpuAssetResolver, slot: u32) -> Entity {
    let entity = world.spawn();
    world.insert(
        entity,
        SpriteRenderer {
            sprite_path: "sprites/boat.png".to_string(),
            ..Default::default()
        },
    );
    resolver.apply_results(
        world,
        &[(
            entity,
            AssetResolveResult {
                material_slots: Vec::new(),
                mesh_handle: None,
                material_slot: None,
                texture_slot: Some(slot),
                texture_size: Some([16, 16]),
                shader_handle: None,
            },
        )],
    );
    entity
}

#[test]
fn a_good_upload_does_not_queue_a_release() {
    let mut world = World::new();
    let mut resolver = GpuAssetResolver::new();
    resolved_sprite(&mut world, &mut resolver, 9);
    assert!(resolver.take_releases().is_empty());
}

#[test]
fn re_uploading_to_a_new_slot_releases_the_previous_one() {
    // Path rewritten → new upload gets new slot, old slot must be returned, otherwise each graph switch leaks a texture.
    let mut world = World::new();
    let mut resolver = GpuAssetResolver::new();
    let entity = resolved_sprite(&mut world, &mut resolver, 9);
    resolver.apply_results(
        &mut world,
        &[(
            entity,
            AssetResolveResult {
                material_slots: Vec::new(),
                mesh_handle: None,
                material_slot: None,
                texture_slot: Some(21),
                texture_size: Some([16, 16]),
                shader_handle: None,
            },
        )],
    );
    let releases = resolver.take_releases();
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0].texture_slot, Some(9));
    // Same slot uploaded again (result reused) does not count as invalidation.
    resolver.apply_results(
        &mut world,
        &[(
            entity,
            AssetResolveResult {
                material_slots: Vec::new(),
                mesh_handle: None,
                material_slot: None,
                texture_slot: Some(21),
                texture_size: Some([16, 16]),
                shader_handle: None,
            },
        )],
    );
    assert!(resolver.take_releases().is_empty());
}

#[test]
fn despawning_the_entity_releases_its_gpu_handles() {
    let mut world = World::new();
    let mut resolver = GpuAssetResolver::new();
    let entity = resolved_sprite(&mut world, &mut resolver, 5);
    world.despawn(entity);

    // `prepare_requests` opens with recycling of invalidated resources.
    resolver.prepare_requests(&world);
    let releases = resolver.take_releases();
    assert_eq!(releases.len(), 1, "{releases:?}");
    assert_eq!(releases[0].texture_slot, Some(5));
    // Release only once.
    resolver.prepare_requests(&world);
    assert!(resolver.take_releases().is_empty());
}

#[test]
fn hot_reload_invalidation_releases_every_handle() {
    // Hot reload clears generation; old graph images / meshes must be returned this frame.
    let mut world = World::new();
    let mut resolver = GpuAssetResolver::new();
    let sprite = resolved_sprite(&mut world, &mut resolver, 7);
    let mesh_entity = spawn_renderer(&mut world, "models/cube.gltf", "materials/cube.mat.json");
    let mesh_handle = prism_render::managers::MeshHandle::default();
    resolver.apply_results(
        &mut world,
        &[(
            mesh_entity,
            AssetResolveResult {
                material_slots: Vec::new(),
                mesh_handle: Some(mesh_handle),
                material_slot: Some(4),
                texture_slot: None,
                texture_size: None,
                shader_handle: None,
            },
        )],
    );
    assert!(resolver.take_releases().is_empty());

    resolver.invalidate_assets(&mut world);
    resolver.prepare_requests(&world);
    let mut releases = resolver.take_releases();
    releases.sort_by_key(|r| {
        (
            r.texture_slot.unwrap_or(u32::MAX),
            r.material_slot.unwrap_or(u32::MAX),
        )
    });
    assert_eq!(releases.len(), 3, "{releases:?}");
    assert!(releases.iter().any(|r| r.texture_slot == Some(7)));
    assert!(releases.iter().any(|r| r.material_slot == Some(4)));
    assert!(releases.iter().any(|r| r.mesh_handle == Some(mesh_handle)));
    // sprite entity itself remains, next upload will re-account it.
    assert!(world.get::<SpriteRef>(sprite).is_some());
}

fn spawn_renderer(world: &mut World, mesh_path: &str, material_path: &str) -> Entity {
    let entity = world.spawn();
    world.insert(
        entity,
        MeshRenderer {
            mesh_path: mesh_path.to_string(),
            material_path: material_path.to_string(),
            ..Default::default()
        },
    );
    world.insert(
        entity,
        MeshRef {
            asset_id: SceneAssetId::generate(),
            render_handle: MeshHandle::default(),
            generation: 0,
        },
    );
    world.insert(entity, LocalToWorld(glam::Mat4::IDENTITY));
    entity
}

/// Runtime material handle: `generation == 0` means material not yet parsed.
fn insert_material_ref(world: &mut World, entity: Entity) {
    world.insert(
        entity,
        MaterialRef {
            asset_id: SceneAssetId::generate(),
            material_slot: 0,
            generation: 0,
        },
    );
}

/// sprite only needs `SpriteRenderer`: `SpriteRef` filled by parse result, project does not touch engine internal state.
#[test]
fn sprite_without_ref_is_resolved_and_ref_is_created() {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(
        entity,
        SpriteRenderer {
            sprite_path: "sprites/boat.png".to_string(),
            ..Default::default()
        },
    );
    let mut resolver = GpuAssetResolver::new();

    let first = resolver.prepare_requests(&world);
    // Resource package empty, so texture load will fail; key is "no `SpriteRef` still enqueues".
    assert_eq!(
        first.len(),
        1,
        "Sprites without SpriteRef must still be parsed once"
    );

    resolver.apply_results(
        &mut world,
        &[(
            entity,
            AssetResolveResult {
                material_slots: Vec::new(),
                mesh_handle: None,
                material_slot: None,
                texture_slot: Some(7),
                texture_size: Some([16, 16]),
                shader_handle: None,
            },
        )],
    );
    let sprite = world
        .get::<SpriteRef>(entity)
        .expect("SpriteRef should be filled after parsing");
    assert_eq!(sprite.texture_slot, 7);
    assert_eq!(sprite.generation, 1);

    // Already parsed and path unchanged: not re-submitted.
    assert!(resolver.prepare_requests(&world).is_empty());
}

#[test]
fn sprite_atlas_subslice_resolves_uv_and_size() {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(
        entity,
        SpriteRenderer {
            sprite_path: "atlases/chars.ratlas#hero_run".to_string(),
            ..Default::default()
        },
    );
    let mut resolver = GpuAssetResolver::new();
    resolver.pending_atlas_sprites.insert(
        entity,
        AtlasSpriteMeta {
            uv_rect: [0.1, 0.2, 0.3, 0.4],
            trimmed_offset: [2, 4],
            original_size: [32, 32],
            size: [28, 24],
        },
    );

    resolver.apply_results(
        &mut world,
        &[(
            entity,
            AssetResolveResult {
                material_slots: Vec::new(),
                mesh_handle: None,
                material_slot: None,
                texture_slot: Some(5),
                texture_size: Some([512, 512]),
                shader_handle: None,
            },
        )],
    );

    let sprite = world
        .get::<SpriteRef>(entity)
        .expect("SpriteRef should be filled after parsing");
    assert_eq!(sprite.texture_slot, 5);
    assert_eq!(sprite.generation, 1);
    assert_eq!(sprite.uv_rect, [0.1, 0.2, 0.3, 0.4]);
    assert_eq!(sprite.trimmed_offset, [2, 4]);
    assert_eq!(sprite.original_size, [32, 32]);
    assert_eq!(sprite.size, [28, 24]);
}

#[test]
fn tilemap_tileset_is_staged_once_and_written_back() {
    // #529: tilemap goes through same texture parse path as sprite, but handle written into `TilemapRef`.
    use crate::scene::components::Tilemap;
    use crate::scene::runtime_components::TilemapRef;

    let mut world = World::new();
    let entity = world.spawn();
    world.insert(
        entity,
        Tilemap {
            tileset_path: "sprites/tiles.png".to_string(),
            tiles: vec![1, 0, 2],
            ..Tilemap::default()
        },
    );
    let mut resolver = GpuAssetResolver::new();

    let first = resolver.prepare_requests(&world);
    assert_eq!(first.len(), 1, "Parsed once even without TilemapRef");
    // Resource package empty → failure cache, not retried every frame.
    assert!(resolver.prepare_requests(&world).is_empty());

    resolver.apply_results(
        &mut world,
        &[(
            entity,
            AssetResolveResult {
                material_slots: Vec::new(),
                mesh_handle: None,
                material_slot: None,
                texture_slot: Some(3),
                texture_size: Some([64, 64]),
                shader_handle: None,
            },
        )],
    );
    let tileset = world
        .get::<TilemapRef>(entity)
        .expect("Parse result should be written into TilemapRef");
    assert_eq!(tileset.texture_slot, 3);
    assert_eq!(tileset.size, [64, 64]);
    assert_eq!(tileset.generation, 1);
}

/// #591: `Style::image` goes through texture parse like sprite, result written into `UiImageRef`.
#[test]
fn ui_background_images_are_staged_and_written_back() {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(
        entity,
        crate::ui::components::Style {
            image: "textures/icon.png".to_string(),
            background: [1.0; 4],
            ..Default::default()
        },
    );
    let mut resolver = GpuAssetResolver::new();

    let first = resolver.prepare_requests(&world);
    assert_eq!(first.len(), 1, "Parsed once even without UiImageRef");
    // Empty package → failure cache, not retried every frame.
    assert!(resolver.prepare_requests(&world).is_empty());

    resolver.apply_results(
        &mut world,
        &[(
            entity,
            AssetResolveResult {
                material_slots: Vec::new(),
                mesh_handle: None,
                material_slot: None,
                texture_slot: Some(5),
                texture_size: Some([32, 32]),
                shader_handle: None,
            },
        )],
    );
    let image = world
        .get::<UiImageRef>(entity)
        .expect("Parse result should be written into UiImageRef");
    assert_eq!(image.texture_slot, 5);
    assert_eq!(image.size, [32, 32]);
    assert_eq!(image.generation, 1);
}

/// Changing `sprite_path` is enough: no need to manually zero `generation`.
#[test]
fn sprite_path_change_re_resolves_without_touching_generation() {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(
        entity,
        SpriteRenderer {
            sprite_path: "sprites/bg@day.png".to_string(),
            ..Default::default()
        },
    );
    let mut resolver = GpuAssetResolver::new();
    assert_eq!(resolver.prepare_requests(&world).len(), 1);
    resolver.apply_results(
        &mut world,
        &[(
            entity,
            AssetResolveResult {
                material_slots: Vec::new(),
                mesh_handle: None,
                material_slot: None,
                texture_slot: Some(1),
                texture_size: Some([8, 8]),
                shader_handle: None,
            },
        )],
    );
    assert_eq!(world.get::<SpriteRef>(entity).unwrap().generation, 1);

    world.get_mut::<SpriteRenderer>(entity).unwrap().sprite_path =
        "sprites/bg@night.png".to_string();
    let requests = resolver.prepare_requests(&world);
    assert_eq!(requests.len(), 1, "Path change must re-parse");
    assert_eq!(
        world.get::<SpriteRef>(entity).unwrap().generation,
        1,
        "Host need not (and should not) manually zero generation"
    );
}

/// Mesh without material: empty material path has nothing to parse, must not re-submit every frame
/// (otherwise mesh would be uploaded anew each time).
#[test]
fn mesh_only_renderer_is_enqueued_once() {
    let mut world = World::new();
    let entity = spawn_renderer(&mut world, "models/box.gltf", "");
    let mut resolver = GpuAssetResolver::new();

    let first = resolver.prepare_requests(&world);
    assert_eq!(first.len(), 1, "Mesh path still needs one parse");
    assert!(
        first[0].1.materials.is_empty(),
        "Empty material path should not produce material parse request"
    );

    // Simulate render thread callback: mesh handle parsed, material result empty.
    world.get_mut::<MeshRef>(entity).unwrap().generation = 1;
    resolver.apply_results(
        &mut world,
        &[(
            entity,
            AssetResolveResult {
                material_slots: Vec::new(),
                mesh_handle: None,
                material_slot: None,
                texture_slot: None,
                texture_size: None,
                shader_handle: None,
            },
        )],
    );

    assert!(
        resolver.prepare_requests(&world).is_empty(),
        "No re-submission after parse completes"
    );
}

/// When runtime component missing there is nowhere to write result, submission would only re-enqueue each frame.
#[test]
fn renderer_without_runtime_refs_is_not_enqueued() {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(
        entity,
        MeshRenderer {
            mesh_path: "models/box.gltf".to_string(),
            material_path: "materials/box.mat".to_string(),
            ..Default::default()
        },
    );
    let mut resolver = GpuAssetResolver::new();

    assert!(resolver.prepare_requests(&world).is_empty());
    assert!(resolver.prepare_requests(&world).is_empty());
}

/// Partial parse: mesh upload succeeds, material path bad — must not re-upload already-parsed mesh every frame
/// just because material still in `generation == 0` (VRAM / bindless slot leak).
#[test]
fn failed_material_is_latched_and_the_resolved_mesh_is_not_reuploaded() {
    let mut world = World::new();
    let entity = spawn_renderer(&mut world, "models/box.gltf", "materials/missing.mat");
    insert_material_ref(&mut world, entity);
    let mut resolver = GpuAssetResolver::new();

    let first = resolver.prepare_requests(&world);
    assert_eq!(
        first.len(),
        1,
        "Both slots not yet parsed, need one submission"
    );

    // Render thread callback: mesh upload success, material no handle.
    resolver.apply_results(
        &mut world,
        &[(
            entity,
            AssetResolveResult {
                material_slots: Vec::new(),
                mesh_handle: Some(MeshHandle::default()),
                material_slot: None,
                texture_slot: None,
                texture_size: None,
                shader_handle: None,
            },
        )],
    );
    assert_eq!(world.get::<MeshRef>(entity).unwrap().generation, 1);

    assert!(
        resolver.prepare_requests(&world).is_empty(),
        "Failed material path cached, parsed mesh must not be re-submitted"
    );
    assert!(
        resolver.prepare_requests(&world).is_empty(),
        "Cache must persist across frames (old implementation re-requested every frame)"
    );
}

/// Failure cache keyed by path: after author replaces bad path (or hot reload swaps package) must retry,
/// and only retry that side.
#[test]
fn changing_a_failed_path_retries_only_that_half() {
    let mut world = World::new();
    let entity = spawn_renderer(&mut world, "models/box.gltf", "materials/missing.mat");
    insert_material_ref(&mut world, entity);
    let mut resolver = GpuAssetResolver::new();

    assert_eq!(resolver.prepare_requests(&world).len(), 1);
    resolver.apply_results(
        &mut world,
        &[(
            entity,
            AssetResolveResult {
                material_slots: Vec::new(),
                mesh_handle: Some(MeshHandle::default()),
                material_slot: None,
                texture_slot: None,
                texture_size: None,
                shader_handle: None,
            },
        )],
    );

    world.get_mut::<MeshRenderer>(entity).unwrap().material_path =
        "materials/fixed.mat".to_string();
    let retry = resolver.prepare_requests(&world);
    assert_eq!(retry.len(), 1, "Should retry after path change");
    assert!(
        retry[0].1.mesh.is_none(),
        "Mesh already parsed, retry must not load/upload it again"
    );
}

/// Build vertex area per real RMXI / RMES layout: segmented, not interleaved.
fn planar_mesh(
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    uvs: &[[f32; 2]],
    indices: &[u32],
) -> prism_asset::formats::RmesInfo {
    let mut vertex_data = Vec::new();
    for p in positions {
        for c in p {
            vertex_data.extend_from_slice(&c.to_le_bytes());
        }
    }
    for n in normals {
        for c in n {
            vertex_data.extend_from_slice(&c.to_le_bytes());
        }
    }
    for t in uvs {
        for c in t {
            vertex_data.extend_from_slice(&c.to_le_bytes());
        }
    }
    let float_stride =
        3 + if normals.is_empty() { 0 } else { 3 } + if uvs.is_empty() { 0 } else { 2 };
    prism_asset::formats::RmesInfo {
        vert_count: positions.len() as u32,
        idx_count: indices.len() as u32,
        uv_channels: if uvs.is_empty() { 0 } else { 1 },
        stride_bytes: (float_stride * 4) as u32,
        vertex_data,
        index_data: indices.iter().flat_map(|i| i.to_le_bytes()).collect(),
        submeshes: Vec::new(),
    }
}

#[test]
fn planar_rmes_splits_into_channels_instead_of_rows() {
    // Two vertex counts differ: any "read segments as interleaved" would mix channels.
    let info = planar_mesh(
        &[[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]],
        &[[0.0, 0.0, 1.0], [0.0, 1.0, 0.0]],
        &[[0.25, 0.5], [0.75, 1.0]],
        &[0, 1, 0],
    );

    let mesh = deinterleave_rmes("test/planar.rmes", &info)
        .expect("valid RMES must decode")
        .0;

    assert_eq!(mesh.positions, vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]);
    assert_eq!(mesh.normals, vec![[0.0, 0.0, 1.0], [0.0, 1.0, 0.0]]);
    assert_eq!(mesh.uvs, vec![[0.25, 0.5], [0.75, 1.0]]);
    assert_eq!(mesh.indices, vec![0, 1, 0]);
}

#[test]
fn planar_rmes_without_normals_keeps_positions_and_uvs() {
    // Without normals stride is 3 + 2 floats, uv segment tightly follows positions segment.
    let info = planar_mesh(
        &[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        &[],
        &[[0.1, 0.2], [0.3, 0.4]],
        &[0, 1, 1],
    );

    let mesh = deinterleave_rmes("test/uv-only.rmes", &info)
        .expect("valid RMES must decode")
        .0;

    assert_eq!(mesh.positions, vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
    assert_eq!(mesh.normals, vec![[0.0, 0.0, 1.0], [0.0, 0.0, 1.0]]);
    assert_eq!(mesh.uvs, vec![[0.1, 0.2], [0.3, 0.4]]);
}

#[test]
fn planar_rmes_positions_only_falls_back_to_a_fixed_normal() {
    let info = planar_mesh(&[[7.0, 8.0, 9.0], [10.0, 11.0, 12.0]], &[], &[], &[0, 1, 0]);

    let mesh = deinterleave_rmes("test/pos-only.rmes", &info)
        .expect("valid RMES must decode")
        .0;

    assert_eq!(mesh.positions, vec![[7.0, 8.0, 9.0], [10.0, 11.0, 12.0]]);
    assert_eq!(mesh.normals, vec![[0.0, 0.0, 1.0], [0.0, 0.0, 1.0]]);
    assert_eq!(mesh.uvs, vec![[0.0, 0.0], [0.0, 0.0]]);
}

#[test]
fn rmes_v2_submeshes_reach_the_runtime_component() {
    // Hand-write an RMES v2: 3 vertices, 6 indices, two submeshes.
    let mut blob = Vec::new();
    blob.extend_from_slice(b"RMES");
    blob.push(2);
    blob.extend_from_slice(&3u32.to_le_bytes());
    blob.extend_from_slice(&6u32.to_le_bytes());
    blob.extend_from_slice(&0u32.to_le_bytes()); // uv channels
    blob.extend_from_slice(&12u32.to_le_bytes()); // stride bytes
    blob.extend_from_slice(&33u32.to_le_bytes()); // positions offset
    blob.extend_from_slice(&0u32.to_le_bytes()); // normals offset
    blob.extend_from_slice(&0u32.to_le_bytes()); // uv0 offset
    for position in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
        for value in position {
            blob.extend_from_slice(&value.to_le_bytes());
        }
    }
    for index in [0u32, 1, 2, 0, 2, 1] {
        blob.extend_from_slice(&index.to_le_bytes());
    }
    blob.extend_from_slice(&2u32.to_le_bytes());
    for (start, count, material) in [(0u32, 3u32, 0u32), (3, 3, u32::MAX)] {
        blob.extend_from_slice(&start.to_le_bytes());
        blob.extend_from_slice(&count.to_le_bytes());
        blob.extend_from_slice(&material.to_le_bytes());
    }

    let info = prism_asset::formats::decode_rmes(&blob).expect("RMES v2 must decode");
    assert_eq!(info.submeshes.len(), 2);
    let (_mesh, submeshes) =
        deinterleave_rmes("test/submeshes.rmes", &info).expect("valid RMES must decode");
    assert_eq!(
        submeshes,
        vec![
            MeshSubmesh {
                index_start: 0,
                index_count: 3,
                material_index: Some(0),
            },
            MeshSubmesh {
                index_start: 3,
                index_count: 3,
                material_index: None,
            },
        ]
    );
}

#[test]
fn rtex_rgba8_follows_the_slot_colour_space() {
    assert_eq!(
        texture_format_for(RTEX_FORMAT_RGBA8, true),
        Some(TextureFormat::Rgba8Srgb)
    );
    assert_eq!(
        texture_format_for(RTEX_FORMAT_RGBA8, false),
        Some(TextureFormat::Rgba8),
        "linear slots must not be sRGB-decoded"
    );
}

#[test]
fn rtex_compressed_formats_keep_linear_semantics() {
    assert_eq!(
        texture_format_for(RTEX_FORMAT_BC5, true),
        Some(TextureFormat::Bc5Unorm),
        "BC5 is a linear 2-channel normal map regardless of the slot"
    );
    assert_eq!(
        texture_format_for(RTEX_FORMAT_BC6H, false),
        Some(TextureFormat::Bc6HUfloat)
    );
}

#[test]
fn bc1_and_bc3_follow_the_slot_colour_space() {
    // Color texture needs sRGB sampling, normal / MR stay linear (#519).
    assert_eq!(
        texture_format_for(RTEX_FORMAT_BC1, true),
        Some(TextureFormat::Bc1Srgb)
    );
    assert_eq!(
        texture_format_for(RTEX_FORMAT_BC1, false),
        Some(TextureFormat::Bc1Unorm)
    );
    assert_eq!(
        texture_format_for(RTEX_FORMAT_BC3, true),
        Some(TextureFormat::Bc3Srgb)
    );
    assert_eq!(
        texture_format_for(RTEX_FORMAT_BC3, false),
        Some(TextureFormat::Bc3Unorm)
    );
    assert_eq!(
        texture_format_for(RTEX_FORMAT_BC7, true),
        Some(TextureFormat::Bc7Srgb)
    );
    assert_eq!(
        texture_format_for(RTEX_FORMAT_BC7, false),
        Some(TextureFormat::Bc7Unorm)
    );
}

#[test]
fn astc_and_etc2_follow_the_slot_colour_space() {
    assert_eq!(
        texture_format_for(RTEX_FORMAT_ASTC_4X4, true),
        Some(TextureFormat::Astc4x4Srgb)
    );
    assert_eq!(
        texture_format_for(RTEX_FORMAT_ASTC_4X4, false),
        Some(TextureFormat::Astc4x4Unorm)
    );
    assert_eq!(
        texture_format_for(RTEX_FORMAT_ASTC_6X6, true),
        Some(TextureFormat::Astc6x6Srgb)
    );
    assert_eq!(
        texture_format_for(RTEX_FORMAT_ASTC_8X8, false),
        Some(TextureFormat::Astc8x8Unorm)
    );
    assert_eq!(
        texture_format_for(RTEX_FORMAT_ASTC_12X12, true),
        Some(TextureFormat::Astc12x12Srgb)
    );
    assert_eq!(
        texture_format_for(RTEX_FORMAT_ASTC_12X12, false),
        Some(TextureFormat::Astc12x12Unorm)
    );
    assert_eq!(
        texture_format_for(RTEX_FORMAT_ETC2_RGBA8, true),
        Some(TextureFormat::Etc2R8G8B8A8Srgb)
    );
    assert_eq!(
        texture_format_for(RTEX_FORMAT_ETC2_RGBA8, false),
        Some(TextureFormat::Etc2R8G8B8A8Unorm)
    );
}

#[test]
fn unsupported_rtex_formats_fall_back() {
    assert_eq!(texture_format_for(99, false), None);
    assert_eq!(texture_format_for(99, true), None);
}

fn make_test_rtex(width: u32, height: u32, format: u8, mips: &[&[u8]]) -> Vec<u8> {
    let levels = mips.len() as u32;
    let header_size = 4 + 1 + 4 + 4 + 4 + 1 + (levels as usize * 4);
    let data_size: usize = mips.iter().map(|m| m.len()).sum();
    let mut buf = Vec::with_capacity(header_size + data_size);

    buf.extend_from_slice(b"RTEX");
    buf.push(1);
    buf.extend_from_slice(&width.to_le_bytes());
    buf.extend_from_slice(&height.to_le_bytes());
    buf.extend_from_slice(&levels.to_le_bytes());
    buf.push(format);

    let offset_pos = buf.len();
    buf.resize(offset_pos + levels as usize * 4, 0);

    let mut mip_start = header_size as u32;
    for (i, mip) in mips.iter().enumerate() {
        let off = &mut buf[offset_pos + i * 4..offset_pos + (i + 1) * 4];
        off.copy_from_slice(&mip_start.to_le_bytes());
        buf.extend_from_slice(mip);
        mip_start += mip.len() as u32;
    }
    buf
}

#[test]
fn load_texture_data_preserves_prebaked_mip_chain() {
    let mip0 = vec![10u8; 4 * 4 * 4];
    let mip1 = vec![20u8; 2 * 2 * 4];
    let mip2 = vec![30u8; 4];
    let rtex = make_test_rtex(4, 4, RTEX_FORMAT_RGBA8, &[&mip0, &mip1, &mip2]);

    let (pak, manifest) = tiny_pak(&[(
        "textures/terrain.rtex",
        prism_asset::core::AssetType::Texture,
        &rtex,
    )]);
    let mut resolver = GpuAssetResolver::new();
    resolver
        .load_resource_package_bytes(&pak, Some(&manifest))
        .expect("load pak");

    let input = resolver
        .load_texture_by_path(Entity::from_raw(u32::MAX, 0), "textures/terrain.rtex")
        .expect("load texture");

    assert_eq!(input.width, 4);
    assert_eq!(input.height, 4);
    assert_eq!(input.mip_levels, 3);
    assert_eq!(input.pixels.len(), 64 + 16 + 4);
    assert_eq!(&input.pixels[0..64], &mip0[..]);
    assert_eq!(&input.pixels[64..80], &mip1[..]);
    assert_eq!(&input.pixels[80..84], &mip2[..]);
}

#[test]
fn load_texture_data_single_mip_keeps_zero_mip_levels_fallback() {
    let mip0 = vec![10u8; 4 * 4 * 4];
    let rtex = make_test_rtex(4, 4, RTEX_FORMAT_RGBA8, &[&mip0]);

    let (pak, manifest) = tiny_pak(&[(
        "textures/single.rtex",
        prism_asset::core::AssetType::Texture,
        &rtex,
    )]);
    let mut resolver = GpuAssetResolver::new();
    resolver
        .load_resource_package_bytes(&pak, Some(&manifest))
        .expect("load pak");

    let input = resolver
        .load_texture_by_path(Entity::from_raw(u32::MAX, 0), "textures/single.rtex")
        .expect("load texture");

    assert_eq!(input.width, 4);
    assert_eq!(input.height, 4);
    assert_eq!(input.mip_levels, 0);
    assert_eq!(input.pixels, mip0);
}

#[test]
fn load_texture_data_corrupt_mip_falls_back_to_mip0() {
    let mip0 = vec![10u8; 4 * 4 * 4];
    let bad_mip1 = vec![20u8; 10];
    let rtex = make_test_rtex(4, 4, RTEX_FORMAT_RGBA8, &[&mip0, &bad_mip1]);

    let (pak, manifest) = tiny_pak(&[(
        "textures/bad_chain.rtex",
        prism_asset::core::AssetType::Texture,
        &rtex,
    )]);
    let mut resolver = GpuAssetResolver::new();
    resolver
        .load_resource_package_bytes(&pak, Some(&manifest))
        .expect("load pak");

    let input = resolver
        .load_texture_by_path(Entity::from_raw(u32::MAX, 0), "textures/bad_chain.rtex")
        .expect("load texture");

    assert_eq!(input.width, 4);
    assert_eq!(input.height, 4);
    assert_eq!(input.mip_levels, 0);
    assert_eq!(input.pixels, mip0);
}

#[cfg(test)]
mod mesh_bounds_tests {
    use super::*;
    use crate::scene::systems::render::MeshBounds;
    use prism_render::managers::MeshHandle;
    use prism_scene::SceneAssetId;

    #[test]
    fn the_mesh_bounds_land_in_the_registry_with_the_handle() {
        // built-in cube needs no `.pak`, but goes through same prepare → upload → writeback path.
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(
            entity,
            MeshRenderer {
                mesh_path: "__builtin/box".into(),
                ..Default::default()
            },
        );
        world.insert(
            entity,
            MeshRef {
                asset_id: SceneAssetId::generate(),
                render_handle: MeshHandle::default(),
                generation: 0,
            },
        );

        let mut resolver = GpuAssetResolver::new();
        let requests = resolver.prepare_requests(&world);
        assert_eq!(requests.len(), 1);
        assert!(requests[0].1.mesh.is_some());

        let handle = MeshHandle::default();
        resolver.apply_results(
            &mut world,
            &[(
                entity,
                prism_render::asset_bridge::AssetResolveResult {
                    material_slots: Vec::new(),
                    mesh_handle: Some(handle),
                    material_slot: None,
                    texture_slot: None,
                    texture_size: None,
                    shader_handle: None,
                },
            )],
        );

        let registry = world
            .get_resource::<MeshBounds>()
            .expect("bounds registry must be created on the first upload");
        let bounds = registry.get(handle).expect("box bounds must be registered");
        // Unit cube (edge 1, centered at origin).
        assert!((bounds.min[0] + 0.5).abs() < 1e-5);
        assert!((bounds.max[1] - 0.5).abs() < 1e-5);

        // Another submission will not re-upload: handle already written back (`generation = 1`).
        assert!(resolver.prepare_requests(&world).is_empty());
    }
}

#[cfg(test)]
mod hot_reload_tests {
    use super::*;
    use prism_render::managers::MeshHandle;
    use prism_scene::SceneAssetId;

    fn spawn_mesh(world: &mut World, path: &str, generation: u32) -> Entity {
        let entity = world.spawn();
        world.insert(
            entity,
            MeshRenderer {
                mesh_path: path.into(),
                ..Default::default()
            },
        );
        world.insert(
            entity,
            MeshRef {
                asset_id: SceneAssetId::generate(),
                render_handle: Default::default(),
                generation,
            },
        );
        entity
    }

    #[test]
    fn invalidating_everything_re_uploads_the_meshes() {
        let mut world = World::new();
        let entity = spawn_mesh(&mut world, "__builtin/box", 1);

        let mut resolver = GpuAssetResolver::new();
        assert!(
            resolver.prepare_requests(&world).is_empty(),
            "a resolved mesh must not be re-uploaded on its own"
        );

        assert_eq!(resolver.invalidate_assets(&mut world), 1);
        assert_eq!(world.get::<MeshRef>(entity).unwrap().generation, 0);
        assert_eq!(
            resolver.prepare_requests(&world).len(),
            1,
            "a hot reload must put the entity back in the upload queue"
        );
    }

    #[test]
    fn invalidating_a_path_only_touches_its_users() {
        let mut world = World::new();
        let boxed = spawn_mesh(&mut world, "__builtin/box", 1);
        let sphere = spawn_mesh(&mut world, "__builtin/sphere", 1);

        let mut resolver = GpuAssetResolver::new();
        assert_eq!(resolver.invalidate_path(&mut world, "__builtin/box"), 1);
        assert_eq!(world.get::<MeshRef>(boxed).unwrap().generation, 0);
        assert_eq!(
            world.get::<MeshRef>(sphere).unwrap().generation,
            1,
            "an unrelated path must keep its resolved handles"
        );
        assert_eq!(
            resolver.invalidate_path(&mut world, "assets/nothing.rmes"),
            0
        );
    }

    #[test]
    fn invalidation_clears_the_failure_cache() {
        // A path not in any package: first prepare fails and enters failure cache,
        // then not retried. Hot reload must allow one retry (new package may have added it).
        let mut world = World::new();
        let entity = spawn_mesh(&mut world, "assets/new_mesh.rmes", 0);

        let mut resolver = GpuAssetResolver::new();
        let first = resolver.prepare_requests(&world);
        assert_eq!(first.len(), 1);
        assert!(first[0].1.mesh.is_none(), "the path is not in any package");
        // Upload failure also counts as "having a result": clear queued flag, only failure cache blocks retry.
        resolver.apply_results(
            &mut world,
            &[(
                entity,
                prism_render::asset_bridge::AssetResolveResult {
                    material_slots: Vec::new(),
                    mesh_handle: None,
                    material_slot: None,
                    texture_slot: None,
                    texture_size: None,
                    shader_handle: None,
                },
            )],
        );
        assert!(
            resolver.prepare_requests(&world).is_empty(),
            "a failed path must not be retried until something changes"
        );

        assert_eq!(
            resolver.invalidate_path(&mut world, "assets/new_mesh.rmes"),
            1
        );
        assert_eq!(
            resolver.prepare_requests(&world).len(),
            1,
            "hot reload must clear the failed-path cache"
        );
        assert_eq!(world.get::<MeshRef>(entity).unwrap().generation, 0);
    }

    #[test]
    fn multiple_material_paths_map_to_submesh_slots() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(
            entity,
            MeshRenderer {
                mesh_path: "models/multi.gltf".into(),
                material_paths: vec!["materials/a.mat".into(), "materials/b.mat".into()],
                ..MeshRenderer::default()
            },
        );
        // mesh already parsed (this frame only fills material), submesh table from runtime cache.
        world.insert(
            entity,
            MeshRef {
                asset_id: SceneAssetId::generate(),
                render_handle: MeshHandle::default(),
                generation: 1,
            },
        );
        world.insert(
            entity,
            MaterialRef {
                asset_id: SceneAssetId::generate(),
                material_slot: 0,
                generation: 0,
            },
        );
        world.insert(
            entity,
            MeshSubmeshes {
                submeshes: vec![
                    MeshSubmesh {
                        index_start: 0,
                        index_count: 3,
                        material_index: Some(0),
                    },
                    MeshSubmesh {
                        index_start: 3,
                        index_count: 3,
                        material_index: Some(1),
                    },
                    MeshSubmesh {
                        index_start: 6,
                        index_count: 3,
                        material_index: Some(2),
                    },
                ],
            },
        );

        // Both paths not in package, but requests still expanded by unique path (upload phase will fail).
        let mut resolver = GpuAssetResolver::new();
        let requests = resolver.prepare_requests(&world);
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].1.materials.len(),
            2,
            "a/b two unique materials only requested twice, third submesh uses default material"
        );

        resolver.apply_results(
            &mut world,
            &[(
                entity,
                prism_render::asset_bridge::AssetResolveResult {
                    material_slots: vec![Some(11), Some(12)],
                    material_slot: Some(11),
                    ..Default::default()
                },
            )],
        );

        let slots = world.get::<MaterialSlots>(entity).expect("slot table");
        assert_eq!(slots.slots, vec![Some(11), Some(12), None]);
        assert_eq!(
            world.get::<MaterialRef>(entity).unwrap().material_slot,
            11,
            "First material still written to legacy field, for code that only reads it"
        );
        assert_eq!(world.get::<MaterialRef>(entity).unwrap().generation, 1);
    }

    #[test]
    fn rcld_virtual_geometry_mesh_resolves_and_streams_pages() {
        use prism_asset::formats_virtual_geo::*;

        let aabb_min = [-1.0, -1.0, 0.0];
        let aabb_max = [1.0, 1.0, 0.0];
        let verts = vec![
            QuantizedClusterVertex {
                pos_q: QuantizedClusterVertex::quantize_position(
                    [-1.0, -1.0, 0.0],
                    aabb_min,
                    aabb_max,
                ),
                normal_oct: encode_octahedral_normal([0.0, 0.0, 1.0]),
                tangent_angle: 0,
                bone_local_pair: 0,
                uv_f16: [f32_to_f16(0.0), f32_to_f16(0.0)],
                geomorph_delta_q: [0, 0, 0],
            },
            QuantizedClusterVertex {
                pos_q: QuantizedClusterVertex::quantize_position(
                    [1.0, -1.0, 0.0],
                    aabb_min,
                    aabb_max,
                ),
                normal_oct: encode_octahedral_normal([0.0, 0.0, 1.0]),
                tangent_angle: 0,
                bone_local_pair: 0,
                uv_f16: [f32_to_f16(1.0), f32_to_f16(0.0)],
                geomorph_delta_q: [0, 0, 0],
            },
            QuantizedClusterVertex {
                pos_q: QuantizedClusterVertex::quantize_position(
                    [0.0, 1.0, 0.0],
                    aabb_min,
                    aabb_max,
                ),
                normal_oct: encode_octahedral_normal([0.0, 0.0, 1.0]),
                tangent_angle: 0,
                bone_local_pair: 0,
                uv_f16: [f32_to_f16(0.5), f32_to_f16(1.0)],
                geomorph_delta_q: [0, 0, 0],
            },
        ];
        let asset = RcldAsset {
            header: RcldHeader {
                version: RCLD_VERSION,
                flags: 0,
                max_lod_level: 1,
                page_count: 2,
                group_count: 2,
                cluster_count: 2,
                root_group_index: 0,
                bounds_min: aabb_min,
                bounds_max: aabb_max,
                page_table_offset: 64,
                group_table_offset: 64 + 40,
                payload_offset: 64 + 40 + 96,
            },
            pages: vec![
                RcldPageDescriptor {
                    page_index: 0,
                    byte_offset: 0,
                    byte_size: 0,
                    first_cluster: 0,
                    cluster_count: 1,
                    lod_level: 1,
                    flags: RCLD_PAGE_FLAG_RESIDENT_ROOT,
                },
                RcldPageDescriptor {
                    page_index: 1,
                    byte_offset: 0,
                    byte_size: 0,
                    first_cluster: 1,
                    cluster_count: 1,
                    lod_level: 0,
                    flags: 0,
                },
            ],
            groups: vec![
                RcldClusterGroup {
                    group_id: 0,
                    lod_level: 1,
                    primitive_kind: VirtualGeoPrimitiveKind::Triangles,
                    child_group_count: 1,
                    page_index: 0,
                    child_group_first: 1,
                    first_cluster: 0,
                    cluster_count: 1,
                    parent_error: 100.0,
                    self_error: 0.25,
                    bounding_sphere: [0.0, 0.0, 0.0, 1.5],
                },
                RcldClusterGroup {
                    group_id: 1,
                    lod_level: 0,
                    primitive_kind: VirtualGeoPrimitiveKind::Triangles,
                    child_group_count: 0,
                    page_index: 1,
                    child_group_first: 0,
                    first_cluster: 1,
                    cluster_count: 1,
                    parent_error: 0.25,
                    self_error: 0.0,
                    bounding_sphere: [0.0, 0.0, 0.0, 1.5],
                },
            ],
            clusters: vec![
                RcldCluster {
                    cluster_id: 0,
                    group_id: 0,
                    lod_level: 1,
                    primitive_kind: VirtualGeoPrimitiveKind::Triangles,
                    material_index: 0,
                    aabb_min,
                    aabb_max,
                    normal_cone: [0.0, 0.0, 1.0, 0.2],
                    parent_error: 100.0,
                    self_error: 0.25,
                    geomorph_scale: 0.0,
                    bone_palette: [0; 8],
                    bone_count: 0,
                    max_skin_inflation: 0.0,
                    vertices: verts.clone(),
                    triangles: vec![[0, 1, 2]],
                    dmm_displacements: Vec::new(),
                    splat_surrogate: None,
                },
                RcldCluster {
                    cluster_id: 1,
                    group_id: 1,
                    lod_level: 0,
                    primitive_kind: VirtualGeoPrimitiveKind::Triangles,
                    material_index: 1,
                    aabb_min,
                    aabb_max,
                    normal_cone: [0.0, 0.0, 1.0, 0.2],
                    parent_error: 0.25,
                    self_error: 0.0,
                    geomorph_scale: 0.0,
                    bone_palette: [0; 8],
                    bone_count: 0,
                    max_skin_inflation: 0.0,
                    vertices: verts,
                    triangles: vec![[0, 1, 2]],
                    dmm_displacements: vec![15, -30, 45],
                    splat_surrogate: None,
                },
            ],
        };
        let rcld_bytes = encode_rcld_bytes(&asset).expect("encode RCLD");
        let (pak, manifest) = tiny_pak(&[(
            "models/statue.rcld",
            prism_asset::core::AssetType::Mesh,
            &rcld_bytes,
        )]);

        let mut resolver = GpuAssetResolver::new();
        resolver
            .load_resource_package_bytes(&pak, Some(&manifest))
            .expect("load pak");

        let mut world = World::new();
        let entity = spawn_renderer(&mut world, "models/statue.rcld", "");
        let requests = resolver.prepare_requests(&world);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, entity);
        let mesh = requests[0]
            .1
            .mesh
            .as_ref()
            .expect("RCLD root fallback must upload MeshUploadInput");
        assert_eq!(mesh.positions.len(), 3);
        assert_eq!(mesh.indices, vec![0, 1, 2]);

        let desc = resolver
            .load_virtual_geo_descriptor("models/statue.rcld")
            .expect("RCLD runtime descriptor");
        assert_eq!(desc.pages.len(), 2);
        assert_eq!(desc.clusters.len(), 2);

        let id = resolver
            .resource_manager
            .id_by_path("models/statue.rcld")
            .expect("asset id");
        let page1 = resolver
            .stream_virtual_geo_page(id, &desc.pages[1])
            .expect("stream page 1 clusters");
        assert_eq!(page1.len(), 1);
        assert_eq!(page1[0].material_index, 1);
        assert_eq!(page1[0].dmm_displacements, vec![15, -30, 45]);
    }
}
