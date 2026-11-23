//! Scene-state persistence: save/restore inspector-editable parameters
//! (transforms, lights, camera) to/from a JSON file.
//!
//! **Nothing saves or loads automatically.** The host (or the project module via
//! `GameWorld::save_scene_state` / `load_scene_state`) decides when: the engine
//! used to document "saved on Ctrl+S, loaded on startup", which was never true
//! and made the API look dead (#484).
//!
//! The format is hand-rolled JSON (no serde dependency).
//!
//! **Format version 2:** Point-light position and active state are serialized
//! together with the light. This preserves the entity/component relationship
//! instead of pairing independent component arrays by iteration order.
//!
//! **Format version 4:** every saved record carries the entity's stable
//! `.scene.json` id (`SceneEntityId`), and restore matches by id instead of by
//! pool order, so inserting or deleting an entity no longer shifts every saved
//! pose (#118). Files without ids still restore positionally.
//!
//! **Format version 5:** Clock entities are serialized too, so a project that
//! advances `TimeOfDay` does not need its own save path just to keep the hour
//! (#1051). Only `hours` is restored: the rest of the component stays authored
//! data from the scene.
//!
//! **Format version 3:** Camera state is sourced from the data components
//! (`Camera` + `FlyCameraController` + `LocalTransform`) instead of the old
//! `crate::camera::Camera` enum. `fov_y` is still serialized in radians for
//! backward-compat with v2 files.

use glam::{Quat, Vec3};

use prism_ecs::{Entity, World};

use crate::scene::components::{
    Active, Camera, DirectionalLight as SceneDirLight, FlyCameraController, LocalTransform,
    PointLight as ScenePtLight, TimeOfDay,
};
use crate::scene::runtime_components::SceneEntityId;
use crate::scene::systems::camera;

const SCENE_STATE_VERSION: u32 = 5;

/// v1 wrote point-light positions in a separate array; v2+ keeps them with the
/// light, so the positional back-fill below only applies to v1 files.
const LEGACY_INDEPENDENT_LIGHTS_VERSION: u32 = 2;

/// Save directory (`World` resource). Host inserts real project config directory at launch.
///
/// Previously `scene_state_path()` used [`crate::config::app_data_dir`] directly, which used
/// `AppConfig::default()` — all projects wrote to
/// `%APPDATA%/Excurs1ons/Prisma/scene_state.json`, overwriting each other (#484).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveDirectory(pub std::path::PathBuf);

/// Default save directory (`config::app_data_dir()`, fallback when real config unavailable).
pub fn default_save_dir() -> Option<std::path::PathBuf> {
    crate::config::app_data_dir()
}

/// Save filename under `dir`: `scene_state.json` when `slot` is `None`,
/// otherwise `scene_state-<slot>.json`.
pub fn scene_state_path_in(dir: Option<&std::path::Path>, slot: Option<u32>) -> std::path::PathBuf {
    let dir = dir
        .map(|dir| dir.to_path_buf())
        .or_else(default_save_dir)
        .unwrap_or_default();
    let name = match slot {
        Some(slot) => format!("scene_state-{slot}.json"),
        None => "scene_state.json".to_string(),
    };
    if dir.as_os_str().is_empty() {
        std::path::PathBuf::from(name)
    } else {
        dir.join(name)
    }
}

/// Save path of current world: prefers [`SaveDirectory`] resource, then default directory.
pub fn scene_state_path(world: &World) -> std::path::PathBuf {
    scene_state_path_in(
        world
            .get_resource::<SaveDirectory>()
            .map(|dir| dir.0.as_path()),
        None,
    )
}

/// Save path of specified slot (also honors [`SaveDirectory`]).
pub fn scene_state_path_for_slot(world: &World, slot: u32) -> std::path::PathBuf {
    scene_state_path_in(
        world
            .get_resource::<SaveDirectory>()
            .map(|dir| dir.0.as_path()),
        Some(slot),
    )
}

#[derive(Clone, Debug)]
pub struct CameraState {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub fov_y: f32,
    pub move_speed: f32,
    pub look_sensitivity: f32,
    pub znear: f32,
    pub zfar: f32,
}

#[derive(Clone, Debug)]
pub struct SceneState {
    pub camera: Option<CameraState>,
    pub directional_light: Option<SceneDirLight>,
    pub point_lights: Vec<ScenePtLight>,
    pub transforms: Vec<SavedTransform>,
}

/// One save transform record: stable id + pose.
///
/// `id` from [`SceneEntityId`] written at load (`.scene.json` v3+ `id`). Saves before v4 lack
/// this field, restore falls back to order-based matching (#118).
#[derive(Clone, Debug)]
pub struct SavedTransform {
    pub id: Option<u32>,
    pub transform: LocalTransform,
}

#[derive(Clone, Debug)]
struct SavedPointLight {
    id: Option<u32>,
    light: ScenePtLight,
    position: Option<Vec3>,
    active: bool,
}

/// A saved clock: the stable id plus the advanced hour.
///
/// Everything else on [`TimeOfDay`] (day length, sunrise, colors, …) is
/// authoring data owned by the scene, so restoring it would undo scene edits.
#[derive(Clone, Debug)]
struct SavedTimeOfDay {
    id: Option<u32>,
    hours: f32,
}

/// Get entity stable id (returns `None` when no `.scene.json` id).
fn scene_entity_id(world: &World, entity: Entity) -> Option<u32> {
    world.get::<SceneEntityId>(entity).map(|id| id.0)
}

fn fmt3(a: Vec3) -> String {
    format!("{},{},{}", a.x, a.y, a.z)
}
fn fmt4(a: Quat) -> String {
    format!("{},{},{},{}", a.x, a.y, a.z, a.w)
}

impl CameraState {
    fn to_json(&self) -> String {
        format!(
            "{{\"position\":[{}],\"yaw\":{},\"pitch\":{},\"fov_y\":{},\"move_speed\":{},\"look_sensitivity\":{},\"znear\":{},\"zfar\":{}}}",
            fmt3(self.position),
            self.yaw,
            self.pitch,
            self.fov_y,
            self.move_speed,
            self.look_sensitivity,
            self.znear,
            self.zfar,
        )
    }

    fn from_json(s: &str) -> Option<Self> {
        let pos = find_array_f32(s, "position")?;
        if pos.len() != 3 {
            return None;
        }
        Some(Self {
            position: Vec3::new(pos[0], pos[1], pos[2]),
            yaw: find_field_f32(s, "yaw")?,
            pitch: find_field_f32(s, "pitch")?,
            fov_y: find_field_f32(s, "fov_y").unwrap_or(std::f32::consts::FRAC_PI_4),
            move_speed: find_field_f32(s, "move_speed").unwrap_or(5.0),
            look_sensitivity: find_field_f32(s, "look_sensitivity").unwrap_or(0.005),
            znear: find_field_f32(s, "znear").unwrap_or(0.01),
            zfar: find_field_f32(s, "zfar").unwrap_or(1000.0),
        })
    }
}

fn dir_light_to_json(light: &SceneDirLight) -> String {
    format!(
        "{{\"euler_xyz\":[{}],\"intensity\":{},\"color\":[{}],\"ambient\":{}}}",
        fmt3(light.euler_xyz),
        light.intensity,
        fmt3(light.color),
        light.ambient,
    )
}

fn dir_light_from_json(s: &str) -> Option<SceneDirLight> {
    let euler = find_array_f32(s, "euler_xyz")?;
    let col = find_array_f32(s, "color")?;
    if euler.len() != 3 || col.len() != 3 {
        return None;
    }
    Some(SceneDirLight {
        euler_xyz: Vec3::new(euler[0], euler[1], euler[2]),
        intensity: find_field_f32(s, "intensity")?,
        color: Vec3::new(col[0], col[1], col[2]),
        ambient: find_field_f32(s, "ambient").unwrap_or(1.0),
    })
}

fn point_light_from_json(s: &str) -> Option<ScenePtLight> {
    let col = find_array_f32(s, "color").unwrap_or_default();
    Some(ScenePtLight {
        range: find_field_f32(s, "range").unwrap_or(12.0),
        color: if col.len() == 3 {
            Vec3::new(col[0], col[1], col[2])
        } else {
            Vec3::ONE
        },
        intensity: find_field_f32(s, "intensity").unwrap_or(1.0),
    })
}

fn saved_transform_to_json(saved: &SavedTransform) -> String {
    let id = saved
        .id
        .map(|id| format!("\"id\":{id},"))
        .unwrap_or_default();
    format!(
        "{{{}\"position\":[{}],\"rotation\":[{}],\"scale\":[{}]}}",
        id,
        fmt3(saved.transform.position),
        fmt4(saved.transform.rotation),
        fmt3(saved.transform.scale),
    )
}

fn saved_time_of_day_to_json(saved: &SavedTimeOfDay) -> String {
    match saved.id {
        Some(id) => format!("{{\"id\":{id},\"hours\":{}}}", saved.hours),
        None => format!("{{\"hours\":{}}}", saved.hours),
    }
}

fn local_transform_from_fields(position: Vec3, rotation: Quat, scale: Vec3) -> LocalTransform {
    LocalTransform {
        position,
        rotation,
        scale,
    }
}

impl SavedPointLight {
    fn to_json(&self) -> String {
        let id = self
            .id
            .map(|id| format!("\"id\":{id},"))
            .unwrap_or_default();
        let position = self
            .position
            .map(|value| format!("\"position\":[{}],", fmt3(value)))
            .unwrap_or_default();
        format!(
            "{{{}{}\"range\":{},\"color\":[{}],\"intensity\":{},\"active\":{}}}",
            id,
            position,
            self.light.range,
            fmt3(self.light.color),
            self.light.intensity,
            self.active,
        )
    }

    fn from_json(s: &str) -> Option<Self> {
        let position = find_array_f32(s, "position")
            .and_then(|value| (value.len() == 3).then(|| Vec3::new(value[0], value[1], value[2])));
        Some(Self {
            id: find_field_u32(s, "id"),
            light: point_light_from_json(s)?,
            position,
            active: find_field_bool(s, "active")
                .or_else(|| find_field_bool(s, "enabled"))
                .unwrap_or(true),
        })
    }
}

/// Camera entity used for save / restore: **the one actually rendered**.
///
/// Shared with render, aspect writeback, fly input via
/// [`scene::systems::camera::active_camera`] (`#45`): with multiple cameras picks max `depth`
/// enabled camera, not first in ECS insert order, otherwise save is A but player sees B.
///
/// `active_camera` requires camera already has `LocalToWorld`; when restoring scene state hierarchy propagation may not have run,
/// so fall back to `main_camera` (max depth enabled camera), finally to first camera,
/// preserving legacy fallback when "only disabled cameras in world".
fn state_camera(world: &World) -> Option<Entity> {
    camera::active_camera(world)
        .or_else(|| camera::main_camera(world))
        .or_else(|| world.query::<Camera>().next().map(|(entity, _)| entity))
}

/// Snapshot [`CameraState`] from **rendered camera**.
///
/// Position from `LocalTransform`, yaw/pitch/move_speed/look_sensitivity from
/// `FlyCameraController`, fov/near/far from `Camera` (position written to transform by controller rebuild,
/// see `scene::systems::camera`). Returns `None` when selected camera lacks any component.
fn snapshot_camera_state(world: &World) -> Option<CameraState> {
    let entity = state_camera(world)?;
    let cam = world.get::<Camera>(entity)?;
    let ctrl = world.get::<FlyCameraController>(entity)?;
    let lt = world.get::<LocalTransform>(entity)?;
    Some(CameraState {
        position: lt.position,
        yaw: ctrl.yaw,
        pitch: ctrl.pitch,
        fov_y: cam.fov_y_degrees.to_radians(),
        move_speed: ctrl.move_speed,
        look_sensitivity: ctrl.look_sensitivity,
        znear: cam.near,
        zfar: cam.far,
    })
}

/// Query ECS world + camera and write JSON to `path`.
pub fn save_scene_state_to(world: &World, path: &std::path::Path) {
    use std::fmt::Write;

    let camera_state = snapshot_camera_state(world);

    let dir_light = world.query::<SceneDirLight>().next().map(|(_, dl)| *dl);
    let point_lights: Vec<SavedPointLight> = world
        .query::<ScenePtLight>()
        .map(|(entity, light)| SavedPointLight {
            id: scene_entity_id(world, entity),
            light: *light,
            position: world.get::<LocalTransform>(entity).map(|t| t.position),
            active: world.get::<Active>(entity).map(|a| a.0).unwrap_or(true),
        })
        .collect();
    let transforms: Vec<SavedTransform> = world
        .query::<LocalTransform>()
        .filter(|(entity, _)| world.get::<ScenePtLight>(*entity).is_none())
        .map(|(entity, transform)| SavedTransform {
            id: scene_entity_id(world, entity),
            transform: transform.clone(),
        })
        .collect();
    let clocks: Vec<SavedTimeOfDay> = world
        .query::<TimeOfDay>()
        .map(|(entity, clock)| SavedTimeOfDay {
            id: scene_entity_id(world, entity),
            hours: clock.normalized_hours(),
        })
        .collect();

    let mut json = String::new();
    json.push_str("{\n");
    let _ = writeln!(json, "\"version\":{SCENE_STATE_VERSION},");

    if let Some(cs) = &camera_state {
        let _ = writeln!(json, "\"camera\":{},", cs.to_json());
    }

    if let Some(dl) = &dir_light {
        let _ = writeln!(json, "\"directionalLight\":{},", dir_light_to_json(dl));
    }

    json.push_str("\"pointLights\":[\n");
    for (i, pl) in point_lights.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        let _ = write!(json, "{}", pl.to_json());
    }
    json.push_str("],\n");

    json.push_str("\"transforms\":[\n");
    for (i, t) in transforms.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        let _ = write!(json, "{}", saved_transform_to_json(t));
    }
    json.push_str("],\n");

    json.push_str("\"timeOfDay\":[\n");
    for (i, clock) in clocks.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        let _ = write!(json, "{}", saved_time_of_day_to_json(clock));
    }
    json.push_str("]\n");
    json.push_str("}\n");

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(path, &json) {
        Ok(_) => log::info!("saved scene state to {:?}", path),
        Err(e) => log::warn!("failed to save scene state to {:?}: {e}", path),
    }
}

/// Write to world default save path ([`scene_state_path`]).
pub fn save_scene_state(world: &World) {
    save_scene_state_to(world, &scene_state_path(world));
}

/// Write to specified slot (`scene_state-<slot>.json`).
pub fn save_scene_state_slot(world: &World, slot: u32) {
    save_scene_state_to(world, &scene_state_path_for_slot(world, slot));
}

/// Read the JSON file and apply saved values to the ECS world (camera
/// lives as a resource inside the world).
/// Returns `true` if a state was loaded (so callers can skip default placement).
pub fn load_scene_state_from(world: &mut World, path: &std::path::Path) -> bool {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return false,
    };

    log::info!("restoring scene state from {:?}", path);
    apply_scene_state(world, &text);
    true
}

/// Restore from world default save path.
pub fn load_scene_state(world: &mut World) -> bool {
    load_scene_state_from(world, &scene_state_path(world))
}

/// Restore from specified slot.
pub fn load_scene_state_slot(world: &mut World, slot: u32) -> bool {
    load_scene_state_from(world, &scene_state_path_for_slot(world, slot))
}

/// Apply already-read scene-state text. Kept separate from file I/O so state
/// reconciliation can be tested without touching the executable directory.
fn apply_scene_state(world: &mut World, text: &str) {
    use std::collections::HashSet;

    let version = find_field_f32(text, "version").unwrap_or(1.0) as u32;
    let parsed_transforms = extract_array(text, "transforms")
        .map(|array| parse_transform_array(&array))
        .unwrap_or_default();

    if let Some(cs) = extract_object(text, "camera").and_then(|json| CameraState::from_json(&json))
    {
        if let Some(cam_entity) = state_camera(world) {
            if let Some(cam) = world.get_mut::<Camera>(cam_entity) {
                cam.fov_y_degrees = cs.fov_y.to_degrees();
                cam.near = cs.znear;
                cam.far = cs.zfar;
            }
            if let Some(ctrl) = world.get_mut::<FlyCameraController>(cam_entity) {
                ctrl.yaw = cs.yaw;
                ctrl.pitch = cs.pitch;
                ctrl.move_speed = cs.move_speed;
                ctrl.look_sensitivity = cs.look_sensitivity;
            }
            if let Some(lt) = world.get_mut::<LocalTransform>(cam_entity) {
                lt.position = cs.position;
            }
        }
    }

    if let Some(dl_json) = extract_object(text, "directionalLight") {
        if let Some(dl) = dir_light_from_json(&dl_json) {
            for (_, existing) in world.query_mut::<SceneDirLight>() {
                *existing = dl;
            }
        }
    }

    let old_point_entities: Vec<Entity> = world
        .query::<ScenePtLight>()
        .map(|(entity, _)| entity)
        .collect();
    let mut reserved_transform_entities: HashSet<Entity> =
        old_point_entities.iter().copied().collect();
    let mut legacy_point_transform_count = 0;

    if let Some(pl_array) = extract_array(text, "pointLights") {
        let mut parsed = parse_pt_light_array(&pl_array);

        if version < LEGACY_INDEPENDENT_LIGHTS_VERSION {
            legacy_point_transform_count = parsed.len().min(parsed_transforms.len());
            for (saved, transform) in parsed
                .iter_mut()
                .zip(parsed_transforms.iter().take(legacy_point_transform_count))
            {
                if saved.position.is_none() {
                    saved.position = Some(transform.transform.position);
                }
            }
        }

        for entity in old_point_entities.iter().copied() {
            world.remove::<ScenePtLight>(entity);
        }

        if parsed.len() > old_point_entities.len() {
            log::warn!(
                "scene state contains {} point light(s), but the loaded scene has only {}; ignoring unmatched saved lights",
                parsed.len(),
                old_point_entities.len(),
            );
        }

        let mut unmatched: Vec<SavedPointLight> = Vec::new();
        let mut remaining: Vec<Entity> = old_point_entities.clone();
        for saved in parsed {
            let matched = saved.id.and_then(|id| {
                let index = remaining
                    .iter()
                    .position(|entity| scene_entity_id(world, *entity) == Some(id))?;
                Some(remaining.remove(index))
            });
            match matched {
                Some(entity) => {
                    apply_saved_light(world, entity, saved, &mut reserved_transform_entities)
                }
                None => unmatched.push(saved),
            }
        }
        for (entity, saved) in remaining.into_iter().zip(unmatched) {
            apply_saved_light(world, entity, saved, &mut reserved_transform_entities);
        }
    }

    if let Some(clock_array) = extract_array(text, "timeOfDay") {
        let parsed = parse_time_of_day_array(&clock_array);
        let parsed_count = parsed.len();
        let mut remaining: Vec<Entity> = world
            .query::<TimeOfDay>()
            .map(|(entity, _)| entity)
            .collect();
        let scene_clocks = remaining.len();
        let mut unmatched: Vec<SavedTimeOfDay> = Vec::new();
        for saved in parsed {
            let matched = saved.id.and_then(|id| {
                let index = remaining
                    .iter()
                    .position(|entity| scene_entity_id(world, *entity) == Some(id))?;
                Some(remaining.remove(index))
            });
            match matched {
                Some(entity) => apply_saved_time_of_day(world, entity, saved.hours),
                None => unmatched.push(saved),
            }
        }
        if parsed_count > scene_clocks {
            log::warn!(
                "scene state contains {parsed_count} clock(s), but the loaded scene has only {scene_clocks}; ignoring unmatched saved clocks"
            );
        }
        for (entity, saved) in remaining.into_iter().zip(unmatched) {
            apply_saved_time_of_day(world, entity, saved.hours);
        }
    }

    let mut matched_transforms: HashSet<Entity> = HashSet::new();
    let mut legacy_transforms: Vec<SavedTransform> = Vec::new();
    for saved in parsed_transforms
        .into_iter()
        .skip(legacy_point_transform_count)
    {
        let entity = saved.id.and_then(|id| {
            world
                .query::<SceneEntityId>()
                .find(|(entity, stable)| {
                    stable.0 == id
                        && !matched_transforms.contains(entity)
                        && world.get::<LocalTransform>(*entity).is_some()
                })
                .map(|(entity, _)| entity)
        });
        match (entity, saved.id) {
            (Some(entity), _) => {
                if let Some(existing) = world.get_mut::<LocalTransform>(entity) {
                    *existing = saved.transform;
                }
                matched_transforms.insert(entity);
            }
            (None, None) => legacy_transforms.push(saved),
            (None, Some(id)) => log::warn!(
                "scene state holds a transform for entity id {id}, which is not in the loaded scene; ignoring it"
            ),
        }
    }

    let transform_entities: Vec<Entity> = world
        .query::<LocalTransform>()
        .filter(|(entity, _)| {
            !reserved_transform_entities.contains(entity)
                && !matched_transforms.contains(entity)
                && world.get::<ScenePtLight>(*entity).is_none()
        })
        .map(|(entity, _)| entity)
        .collect();
    for (entity, new) in transform_entities.into_iter().zip(legacy_transforms) {
        if let Some(existing) = world.get_mut::<LocalTransform>(entity) {
            *existing = new.transform;
        }
    }
}

/// Restores the saved hour on an existing clock entity. The other fields stay
/// as authored, so a scene edit is never undone by an old save.
fn apply_saved_time_of_day(world: &mut World, entity: Entity, hours: f32) {
    if !hours.is_finite() {
        return;
    }
    if let Some(clock) = world.get_mut::<TimeOfDay>(entity) {
        clock.hours = hours.rem_euclid(24.0);
    }
}

/// Apply one save light to an already existing entity (does not create new entity).
fn apply_saved_light(
    world: &mut World,
    entity: Entity,
    saved: SavedPointLight,
    reserved_transform_entities: &mut std::collections::HashSet<Entity>,
) {
    world.insert(entity, saved.light);
    if let Some(position) = saved.position {
        if let Some(transform) = world.get_mut::<LocalTransform>(entity) {
            transform.position = position;
        } else {
            world.insert(
                entity,
                LocalTransform {
                    position,
                    ..Default::default()
                },
            );
        }
    }
    world.insert(entity, Active(saved.active));
    reserved_transform_entities.insert(entity);
}

/// Find `[a,b,c]` following `key` in a JSON-ish string.
fn find_array_f32(s: &str, key: &str) -> Option<Vec<f32>> {
    let after = s.find(key)? + key.len();
    let rest = &s[after..];
    let open = rest.find('[')?;
    let close = rest[open..].find(']')?;
    let inner = &rest[open + 1..open + close];
    let mut out = Vec::new();
    for part in inner.split(',') {
        out.push(part.trim().parse::<f32>().ok()?);
    }
    Some(out)
}

/// Find a bare `f32` following `"key":` in a JSON-ish string.
fn find_field_f32(s: &str, key: &str) -> Option<f32> {
    let needle = format!("\"{key}\":");
    let pos = s.find(&needle)? + needle.len();
    let rest = s[pos..].trim_start();
    let end = rest
        .find(|c: char| [',', '}', ']'].contains(&c))
        .unwrap_or(rest.len());
    rest[..end].trim().parse::<f32>().ok()
}

/// Find a bare boolean following `"key":` in a JSON-ish string.
fn find_field_bool(s: &str, key: &str) -> Option<bool> {
    let needle = format!("\"{key}\":");
    let pos = s.find(&needle)? + needle.len();
    let rest = s[pos..].trim_start();
    let end = rest
        .find(|c: char| [',', '}', ']'].contains(&c))
        .unwrap_or(rest.len());
    rest[..end].trim().parse::<bool>().ok()
}

/// Find a bare `u32` following a key in a JSON-ish string
fn find_field_u32(s: &str, key: &str) -> Option<u32> {
    let needle = format!("\"{key}\":");
    let pos = s.find(&needle)? + needle.len();
    let rest = s[pos..].trim_start();
    let end = rest
        .find(|c: char| [',', '}', ']'].contains(&c))
        .unwrap_or(rest.len());
    rest[..end].trim().parse::<u32>().ok()
}

/// Extract the JSON object `{...}` for a top-level key. Returns the inner
/// content (without the outer braces) so `from_json` can parse it.
fn extract_object(s: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":");
    let start = s.find(&needle)? + needle.len();
    let rest = &s[start..];
    let brace_open = rest.find('{')?;
    let inner_start = brace_open + 1;
    let mut depth = 1u32;
    let mut pos = inner_start;
    for (i, ch) in rest[inner_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    pos = i;
                    break;
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    Some(rest[inner_start..inner_start + pos].to_string())
}

/// Extract a JSON array `[...]` for a key.
fn extract_array(s: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":");
    let start = s.find(&needle)? + needle.len();
    let rest = &s[start..];
    let bracket_open = rest.find('[')?;
    let inner_start = bracket_open + 1;
    let mut depth = 1u32;
    let mut pos = 0;
    for (i, ch) in rest[inner_start..].char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    pos = i;
                    break;
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    Some(rest[inner_start..inner_start + pos].to_string())
}

/// Parse point lights from version 2 or the legacy position/enabled format.
fn parse_pt_light_array(s: &str) -> Vec<SavedPointLight> {
    let mut out = Vec::new();
    let mut rest = s.trim();
    while !rest.is_empty() {
        let open = match rest.find('{') {
            Some(i) => i,
            None => break,
        };
        let obj_str = match extract_object_nested(&rest[open..]) {
            Some((obj, consumed)) => {
                rest = &rest[open + consumed..];
                obj
            }
            None => break,
        };
        if let Some(light) = SavedPointLight::from_json(&obj_str) {
            out.push(light);
        }
    }
    out
}

/// Parse the `timeOfDay` array: `{ "id": n, "hours": h }` records.
fn parse_time_of_day_array(s: &str) -> Vec<SavedTimeOfDay> {
    let mut out = Vec::new();
    let mut rest = s.trim();
    while !rest.is_empty() {
        let open = match rest.find('{') {
            Some(i) => i,
            None => break,
        };
        let obj_str = match extract_object_nested(&rest[open..]) {
            Some((obj, consumed)) => {
                rest = &rest[open + consumed..];
                obj
            }
            None => break,
        };
        if let Some(hours) = find_field_f32(&obj_str, "hours") {
            out.push(SavedTimeOfDay {
                id: find_field_u32(&obj_str, "id"),
                hours,
            });
        }
    }
    out
}

/// Parse a JSON array of `{...}` objects into Vec<LocalTransform>.
/// Backward-compatible: reads `enabled` from old format (ignored).
fn parse_transform_array(s: &str) -> Vec<SavedTransform> {
    let mut out = Vec::new();
    let mut rest = s.trim();
    while !rest.is_empty() {
        let open = match rest.find('{') {
            Some(i) => i,
            None => break,
        };
        let obj_str = match extract_object_nested(&rest[open..]) {
            Some((obj, consumed)) => {
                rest = &rest[open + consumed..];
                obj
            }
            None => break,
        };
        let t = find_array_f32(&obj_str, "position").unwrap_or_default();
        let r = find_array_f32(&obj_str, "rotation").unwrap_or_default();
        let s = find_array_f32(&obj_str, "scale").unwrap_or_default();
        out.push(SavedTransform {
            id: find_field_u32(&obj_str, "id"),
            transform: local_transform_from_fields(
                if t.len() == 3 {
                    glam::Vec3::new(t[0], t[1], t[2])
                } else {
                    glam::Vec3::ZERO
                },
                if r.len() == 4 {
                    glam::Quat::from_xyzw(r[0], r[1], r[2], r[3])
                } else {
                    glam::Quat::IDENTITY
                },
                if s.len() == 3 {
                    glam::Vec3::new(s[0], s[1], s[2])
                } else {
                    glam::Vec3::ONE
                },
            ),
        });
    }
    out
}

/// Extract a single `{...}` object from the start of a string, returning
/// (inner_content, bytes_consumed) including the braces.
fn extract_object_nested(s: &str) -> Option<(String, usize)> {
    let s = s.trim();
    if !s.starts_with('{') {
        return None;
    }
    let mut depth = 1u32;
    for (i, ch) in s[1..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((s[1..=i].to_string(), i + 2));
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
#[path = "scene_state_tests.rs"]
mod tests;
