//! `SceneManager` scene-switch semantics: on success unload old scene, clear environment cache;
//! on failure keep current scene unchanged.

use super::*;
use crate::scene::components::{SceneAssetId, SceneMember};

/// Spawn an entity belonging to `scene` (minimal replica of loader ownership flag).
fn spawn_scene_entity(world: &mut World, scene: SceneAssetId) -> Entity {
    let entity = world.spawn();
    world.insert(entity, SceneMember(scene));
    entity
}

fn instance(scene_id: SceneAssetId, all_entities: Vec<Entity>) -> SceneInstance {
    SceneInstance {
        scene_id,
        root_entities: all_entities.clone(),
        all_entities,
    }
}

#[test]
fn switching_scenes_unloads_the_previous_scene() {
    let mut world = World::new();
    let mut manager = SceneManager::new();
    let previous = SceneAssetId::generate();
    let current = SceneAssetId::generate();

    let previous_entities: Vec<Entity> = (0..3)
        .map(|_| spawn_scene_entity(&mut world, previous))
        .collect();
    // Engine-owned entities (e.g. `runtime_initialize` fallback camera) have no `SceneMember`,
    // must not be mistakenly deleted on scene switch.
    let engine_owned = world.spawn();
    let current_entities: Vec<Entity> = (0..2)
        .map(|_| spawn_scene_entity(&mut world, current))
        .collect();

    let unloaded = manager.adopt_scene(
        &mut world,
        "second",
        "scenes/second.rscn",
        &instance(current, current_entities.clone()),
    );

    assert_eq!(unloaded, previous_entities.len());
    assert!(previous_entities
        .iter()
        .all(|entity| !world.is_alive(*entity)));
    assert!(current_entities
        .iter()
        .all(|entity| world.is_alive(*entity)));
    assert!(
        world.is_alive(engine_owned),
        "entities without SceneMember are owned by the engine, not the scene"
    );
    assert_eq!(manager.current_scene_name(), Some("second"));
    assert_eq!(
        manager.current_scene_path.as_deref(),
        Some("scenes/second.rscn")
    );
}

#[test]
fn switching_scenes_drops_the_previous_environment_bytes() {
    let mut world = World::new();
    let mut manager = SceneManager::new();
    manager.set_environment_bytes(Some(vec![1, 2, 3]));

    manager.adopt_scene(
        &mut world,
        "next",
        "scenes/next.rscn",
        &instance(SceneAssetId::generate(), Vec::new()),
    );

    assert!(
        manager.environment_bytes.is_none(),
        "IBL must be re-resolved from the new scene instead of reusing the old HDR"
    );
}

#[test]
fn a_missing_scene_keeps_the_current_scene_loaded() {
    let mut world = World::new();
    let mut rm = ResourceManager::new();
    let registry = ComponentRegistry::new();
    let mut manager = SceneManager::new();
    manager.current_scene_name = Some("intro".to_owned());
    manager.current_scene_path = Some("scenes/intro.rscn".to_owned());
    manager.set_environment_bytes(Some(vec![7]));
    let intro_entity = spawn_scene_entity(&mut world, SceneAssetId::generate());

    // Manifest has this scene, but asset pack has no registered corresponding path → load failure.
    let manifest = "\
[[scenes]]
name = \"missing\"
path = \"scenes/missing.rscn\"
";
    let loaded =
        manager.load_from_manifest_text(&mut rm, &mut world, &registry, manifest, Some("missing"));

    assert!(loaded.is_none());
    assert_eq!(manager.current_scene_name(), Some("intro"));
    assert_eq!(
        manager.current_scene_path.as_deref(),
        Some("scenes/intro.rscn")
    );
    assert_eq!(manager.environment_bytes.as_deref(), Some(&[7u8][..]));
    assert!(world.is_alive(intro_entity));
}

#[test]
fn unloading_an_empty_world_is_a_no_op() {
    let mut world = World::new();
    let keep = SceneAssetId::generate();

    assert_eq!(unload_scene_members(&mut world, Some(keep)), 0);

    let entity = spawn_scene_entity(&mut world, keep);
    assert_eq!(unload_scene_members(&mut world, Some(keep)), 0);
    assert!(world.is_alive(entity));
}

// ---------------------------------------------------------------------------
// Manifest parsing: scenes.json priority, legacy scenes.toml fallback
// ---------------------------------------------------------------------------

#[test]
fn parses_the_json_manifest() {
    let manifest = SceneManifest::parse(
        r#"{"scenes":[{"name":"intro","path":"scenes/intro.rscn","enabled":true},
                      {"name":"draft","path":"scenes/draft.rscn","enabled":false}]}"#,
    )
    .expect("json manifest");
    assert_eq!(manifest.scenes.len(), 2);
    assert_eq!(manifest.scenes[0].name, "intro");
    assert!(manifest.scenes[0].enabled);
    assert!(!manifest.scenes[1].enabled);
}

#[test]
fn json_manifest_defaults_enabled_to_true() {
    let manifest =
        SceneManifest::parse(r#"{"scenes":[{"name":"a","path":"scenes/a.rscn"}]}"#).unwrap();
    assert!(manifest.scenes[0].enabled);
}

#[test]
fn json_manifest_preserves_scene_guid() {
    let manifest = SceneManifest::parse(
        r#"{"scenes":[{"name":"a","path":"scenes/a.rscn",
                       "guid":"11111111111111111111111111111111"}]}"#,
    )
    .unwrap();
    assert_eq!(
        manifest.scenes[0].guid.as_deref(),
        Some("11111111111111111111111111111111")
    );
}

#[test]
fn manifest_guid_resolves_to_the_source_scene_path() {
    let temp = tempfile::tempdir().unwrap();
    let assets = temp.path().join("assets");
    std::fs::create_dir_all(assets.join("scenes")).unwrap();
    std::fs::write(assets.join("scenes/renamed.scene.json"), b"{}").unwrap();
    let meta =
        prism_asset::AssetMeta::ensure(&assets, "scenes/renamed.scene.json", "SceneImporter")
            .unwrap();

    let mut manifest = SceneManifest::parse(&format!(
        r#"{{"scenes":[{{"name":"a","path":"scenes/old.rscn","guid":"{}"}}]}}"#,
        meta.guid
    ))
    .unwrap();
    resolve_manifest_guids(&mut manifest, &assets);
    assert_eq!(manifest.scenes[0].path, "scenes/renamed.rscn");
}

#[test]
fn falls_back_to_the_legacy_toml_manifest() {
    let manifest = SceneManifest::parse(
        "# hand written\n[[scenes]]\nname = \"intro\"\npath = \"scenes/intro.rscn\"\n\
         [[scenes]]\nname = \"off\"\npath = \"scenes/off.rscn\"\nenabled = false\n",
    )
    .expect("legacy manifest");
    assert_eq!(manifest.scenes.len(), 2);
    assert_eq!(manifest.scenes[0].name, "intro");
    assert!(
        manifest.scenes[0].enabled,
        "Missing enabled treated as enabled"
    );
    assert!(!manifest.scenes[1].enabled);
}

#[test]
fn garbage_manifest_is_reported_instead_of_silently_empty() {
    assert!(SceneManifest::parse("scenes = 3\n").is_err());
    assert!(SceneManifest::parse("{ not json").is_err());
}

// ---------------------------------------------------------------------------
// Scene unload (#482)
// ---------------------------------------------------------------------------

/// `Persistent` entity and its entire child tree survive one scene switch.
#[test]
fn unloading_a_scene_keeps_persistent_subtrees() {
    use crate::scene::components::{Child, Persistent};
    use prism_scene::HierarchyHelper;

    let mut world = World::new();
    let mut manager = SceneManager::new();
    let scene = SceneAssetId::generate();

    let level_entity = spawn_scene_entity(&mut world, scene);
    let persistent_root = spawn_scene_entity(&mut world, scene);
    world.insert(persistent_root, Persistent);
    let child = spawn_scene_entity(&mut world, scene);
    let grandchild = spawn_scene_entity(&mut world, scene);
    HierarchyHelper::reparent(&mut world, child, Some(persistent_root));
    HierarchyHelper::reparent(&mut world, grandchild, Some(child));

    // A normal intermediate node in scene, to confirm tree structure not wholly preserved.
    assert!(world.get::<Child>(persistent_root).is_some());

    let unloaded = manager.unload_scene(&mut world);

    assert_eq!(unloaded, 1, "Only entity not in Persistent subtree deleted");
    assert!(!world.is_alive(level_entity));
    assert!(world.is_alive(persistent_root));
    assert!(world.is_alive(child), "Persistent descendant kept too");
    assert!(world.is_alive(grandchild));
    assert_eq!(manager.current_scene_name(), None);
    assert!(
        manager.generation() >= 1,
        "Unload must increment scene generation"
    );
}

/// Scene generation must change after unload: project module uses it to discard stale handles.
#[test]
fn every_scene_switch_bumps_the_generation() {
    let mut world = World::new();
    let mut manager = SceneManager::new();
    assert_eq!(manager.generation(), 0);

    let first = SceneAssetId::generate();
    let first_entities: Vec<Entity> = (0..2)
        .map(|_| spawn_scene_entity(&mut world, first))
        .collect();
    manager.adopt_scene(
        &mut world,
        "first",
        "scenes/first.rscn",
        &instance(first, first_entities),
    );
    let after_first = manager.generation();
    assert_eq!(after_first, 1);

    let second = SceneAssetId::generate();
    let second_entities: Vec<Entity> = (0..1)
        .map(|_| spawn_scene_entity(&mut world, second))
        .collect();
    manager.adopt_scene(
        &mut world,
        "second",
        "scenes/second.rscn",
        &instance(second, second_entities),
    );
    assert_eq!(manager.generation(), after_first + 1);
}

/// Author format (`.scene.json`) `Single` takeover: old scene unloaded, not overlaid.
#[test]
fn adopting_an_author_scene_switches_instead_of_stacking() {
    let mut world = World::new();
    let mut manager = SceneManager::new();
    let old = SceneAssetId::generate();
    let old_entities: Vec<Entity> = (0..3)
        .map(|_| spawn_scene_entity(&mut world, old))
        .collect();

    let new = SceneAssetId::generate();
    let new_entities: Vec<Entity> = (0..2)
        .map(|_| spawn_scene_entity(&mut world, new))
        .collect();

    let unloaded =
        manager.adopt_author_scene(&mut world, &instance(new, new_entities.clone()), false);

    assert_eq!(unloaded, old_entities.len());
    assert!(old_entities.iter().all(|entity| !world.is_alive(*entity)));
    assert!(new_entities.iter().all(|entity| world.is_alive(*entity)));
    assert_eq!(
        manager.current_scene_name(),
        None,
        "Author format has no manifest name, must not keep previous scene name"
    );
    assert_eq!(manager.generation(), 1);
}

/// `Additive`: both kept, generation unchanged (old handle still valid).
#[test]
fn additive_loads_keep_both_scenes() {
    let mut world = World::new();
    let mut manager = SceneManager::new();
    let first = SceneAssetId::generate();
    let first_entities: Vec<Entity> = (0..2)
        .map(|_| spawn_scene_entity(&mut world, first))
        .collect();
    let second = SceneAssetId::generate();
    let second_entities: Vec<Entity> = (0..2)
        .map(|_| spawn_scene_entity(&mut world, second))
        .collect();

    let unloaded =
        manager.adopt_author_scene(&mut world, &instance(second, second_entities.clone()), true);

    assert_eq!(unloaded, 0);
    assert!(first_entities.iter().all(|entity| world.is_alive(*entity)));
    assert!(second_entities.iter().all(|entity| world.is_alive(*entity)));
    assert_eq!(manager.generation(), 0);
}

/// Parent belonging to old scene deleted, child `Persistent`: child stays, parent link broken (warn only).
#[test]
fn a_persistent_child_survives_a_dead_parent() {
    use crate::scene::components::Persistent;
    use prism_scene::HierarchyHelper;

    let mut world = World::new();
    let mut manager = SceneManager::new();
    let scene = SceneAssetId::generate();
    let parent = spawn_scene_entity(&mut world, scene);
    let child = spawn_scene_entity(&mut world, scene);
    world.insert(child, Persistent);
    HierarchyHelper::reparent(&mut world, child, Some(parent));

    manager.unload_scene(&mut world);

    assert!(!world.is_alive(parent));
    assert!(world.is_alive(child));
}

#[test]
fn scene_read_view_extracts_cameras_and_lights() {
    let world = World::new();
    let view = SceneReadView::new(&world);
    assert!(view.light_samples().is_empty());
    let cameras = view.cameras(&glam::Mat4::IDENTITY, 16.0 / 9.0);
    assert_eq!(
        cameras.len(),
        1,
        "empty world should produce fallback camera"
    );
    assert!(!cameras[0].has_camera);
}
