//! Scene system — owns scene loading, switching and "current scene" state.
//!
//! # Responsibilities
//!
//! [`SceneManager`] is engine's scene subsystem: owns "current scene name", uniformly responsible for
//! loading scenes from `assets/scenes.json` manifest into ECS world. Scene **content** only loaded by
//! [`prism_asset::core::AssetId`] from asset packages (`.pak`): `scenes.json`'s
//! `path` only lookup key in path manifest (`.pak.meta.json`), first parsed as `AssetId`,
//! then RSCN bytes read by [`ResourceManager`] — no disk file fallback.
//! Earlier versions had `Engine` directly scan manifest to load scenes, now consolidated here
//! (`Engine::init_scene` only forwards) — this implements plan "scene load should be
//! handled by scene system".
//!
//! # Save files
//!
//! Save / load is **not** engine / scene system's responsibility — `Engine` no longer
//! auto-reads `scene_state.json` at init, only exposes [`crate::scene_state`] primitives via
//! `Engine::save_scene_state` / `Engine::load_scene_state`, user project decides
//! when to call.

use std::path::PathBuf;

use prism_asset::core::AssetId;
use prism_asset::runtime::{ResourceManager, SceneAsset};
use prism_ecs::{Entity, World};

use super::component_registry::ComponentRegistry;
use super::loader::{SceneInstance, SceneLoader, SceneSource};

/// Scene system: owns current scene state and handles scene loading.
pub struct SceneManager {
    /// Currently loaded scene name (`scenes[].name` in manifest).
    current_scene_name: Option<String>,
    /// Current scene's path inside asset package (e.g. `scenes/default.rscn`),
    /// used to resolve author-relative paths into package-internal paths.
    current_scene_path: Option<String>,
    environment_bytes: Option<Vec<u8>>,
    /// Scene generation: incremented on every successful switch / unload. Project module uses it to tell whether cached entity handles
    /// are invalidated (`prism-project-api`'s `scene_generation`).
    generation: u64,
    /// At startup manifest gave no loadable scene (`.pak` not packed, or none in manifest).
    /// Only set by manifest load path; explicit `unload` / editor JSON load doesn't count.
    missing_scene: bool,
}

/// Read-only interface for current scene's environment textures.
pub trait EnvironmentProvider {
    fn load_environment(&mut self, asset_path: &str) -> Option<Vec<u8>>;
}

/// Read-only ECS scene view for render / asset phase.
pub struct SceneReadView<'a> {
    world: &'a World,
}

impl<'a> SceneReadView<'a> {
    pub fn new(world: &'a World) -> Self {
        Self { world }
    }

    /// Lend underlying ECS World read-only reference.
    pub fn world(&self) -> &'a World {
        self.world
    }

    /// Extract enabled camera entity output (contains matrices and exposure params).
    pub fn active_camera_output(
        &self,
        surface_rotation: &glam::Mat4,
    ) -> Option<super::systems::camera::CameraOutput> {
        super::systems::camera::compute_camera_output(self.world, surface_rotation)
    }

    /// Extract all camera layers participating in composition (already sorted ascending by Camera.depth).
    pub fn cameras(
        &self,
        surface_rotation: &glam::Mat4,
        display_aspect: f32,
    ) -> Vec<prism_render::render_graph::CameraView> {
        let camera_entities = super::systems::camera::all_cameras(self.world);
        let mut list: Vec<prism_render::render_graph::CameraView> = camera_entities
            .iter()
            .filter_map(|entity| {
                let out = super::systems::camera::compute_camera_output_for(
                    self.world,
                    *entity,
                    surface_rotation,
                )?;
                let flags = super::systems::camera::clear_flags(self.world, *entity);
                Some(prism_render::render_graph::CameraView {
                    view_proj: out.view_proj.to_cols_array_2d(),
                    view: out.view.to_cols_array_2d(),
                    inv_view: out.view.inverse().to_cols_array_2d(),
                    eye: out.eye.to_array(),
                    clear_color: super::systems::camera::clear_color(self.world, *entity),
                    clear_color_enabled: flags.clears_color(),
                    clear_depth: flags.clears_depth(),
                    draw_skybox: flags.draws_skybox(),
                    exposure: out.exposure,
                    has_camera: true,
                })
            })
            .collect();
        if list.is_empty() {
            let fallback =
                super::systems::camera::fallback_camera_output(surface_rotation, display_aspect);
            list.push(prism_render::render_graph::CameraView {
                view_proj: fallback.view_proj.to_cols_array_2d(),
                view: fallback.view.to_cols_array_2d(),
                inv_view: fallback.view.inverse().to_cols_array_2d(),
                eye: fallback.eye.to_array(),
                clear_color: super::systems::camera::DEFAULT_CLEAR_COLOR,
                clear_color_enabled: true,
                clear_depth: true,
                draw_skybox: false,
                exposure: fallback.exposure,
                has_camera: false,
            });
        }
        list
    }

    /// Extract light samples collected in scene.
    pub fn light_samples(&self) -> Vec<super::systems::lights::LightSample> {
        super::systems::lights::collect_light_samples(self.world)
    }

    /// Collect light samples into caller buffer.
    pub fn light_samples_into(&self, out: &mut Vec<super::systems::lights::LightSample>) {
        super::systems::lights::collect_light_samples_into(self.world, out);
    }

    pub fn environment_asset_path(&self) -> Option<String> {
        let mut has_skybox = false;
        for (_, skybox) in self.world.query::<super::components::Skybox>() {
            has_skybox = true;
            if skybox.enabled && !skybox.hdr_path.is_empty() {
                return Some(skybox.hdr_path.clone());
            }
        }
        if has_skybox {
            return None;
        }

        self.world
            .query::<super::components::EnvironmentLighting>()
            .next()
            .and_then(|(_, env)| env.env_map.clone())
    }

    /// Per-frame skybox parameters (rotation/intensity) from the first enabled
    /// Skybox component.
    pub fn skybox_settings(&self) -> Option<prism_render::SkyboxSettings> {
        self.world
            .query::<super::components::Skybox>()
            .find(|(_, skybox)| skybox.enabled)
            .map(|(_, skybox)| prism_render::SkyboxSettings {
                rotation: skybox.rotation_degrees.to_radians(),
                intensity: skybox.intensity.max(0.0),
            })
    }

    /// One gizmo instance per enabled `Gizmo` entity, using its world
    /// transform multiplied by the component's additional scale.
    pub fn gizmo_instances(&self) -> Vec<prism_render::GizmoInstance> {
        let mut out = Vec::new();
        self.gizmo_instances_into(&mut out);
        out
    }

    /// [`Self::gizmo_instances`] into a caller-owned buffer (frame-packet reuse).
    pub fn gizmo_instances_into(&self, out: &mut Vec<prism_render::GizmoInstance>) {
        out.clear();
        out.extend(
            self.world
                .query::<super::components::Gizmo>()
                .filter(|(_, gizmo)| gizmo.enabled)
                .filter_map(|(entity, gizmo)| {
                    let transform = self.world.get::<super::components::LocalToWorld>(entity)?;
                    let model = transform.0
                        * glam::Mat4::from_scale(glam::Vec3::splat(gizmo.scale.max(0.0)));
                    Some(prism_render::GizmoInstance {
                        model: model.to_cols_array_2d(),
                        mode: match gizmo.mode {
                            super::components::GizmoMode::Translate => 0,
                            super::components::GizmoMode::Rotate => 1,
                            super::components::GizmoMode::Scale => 2,
                        },
                    })
                }),
        );
    }

    /// Extract 2D draw list (sprite + polyline, already sorted by `sorting_layer` →
    /// `order_in_layer` → world `z` → entity id, far drawn first, near drawn later).
    pub fn sprite_items(&self) -> Vec<prism_render::SpriteDrawItem> {
        super::systems::render::scene_2d_system(self.world)
    }

    /// [`Self::sprite_items`] into a caller-owned buffer (frame-packet reuse).
    pub fn sprite_items_into(&self, out: &mut Vec<prism_render::SpriteDrawItem>) {
        super::systems::render::scene_2d_system_into(self.world, out)
    }

    /// [`Self::sprite_items_into`] plus main camera frustum (tilemap clipped per cell, #529).
    pub fn sprite_items_with_frustum_into(
        &self,
        frustum: Option<&prism_render::culling::Frustum>,
        out: &mut Vec<prism_render::SpriteDrawItem>,
    ) {
        self.sprite_items_with_frustum_and_line_scale_into(frustum, None, out)
    }

    /// [`Self::sprite_items_with_frustum_into`] plus main-line camera pixel ratio (#978):
    /// sub-pixel-wide polylines drawn at 1 render pixel, avoid breaking into dots.
    pub fn sprite_items_with_frustum_and_line_scale_into(
        &self,
        frustum: Option<&prism_render::culling::Frustum>,
        line_scale: Option<super::systems::render::LineScreenScale>,
        out: &mut Vec<prism_render::SpriteDrawItem>,
    ) {
        super::systems::render::scene_2d_system_into_with_frustum_and_line_scale(
            self.world, frustum, line_scale, out,
        )
    }

    /// Extract `Light2D` (#611), already pruned to `SPRITE_LIGHT_MAX` by contribution.
    pub fn sprite_lights_into(&self, out: &mut Vec<prism_render::SpriteLight>) {
        out.clear();
        out.extend(super::systems::render::sprite_light_entries(self.world));
    }

    /// Extract 3D particle billboards (already sorted **far to near** by distance to `eye`).
    pub fn particle_items_into(
        &self,
        eye: glam::Vec3,
        out: &mut Vec<prism_render::ParticleDrawItem>,
    ) {
        super::systems::particles::scene_particles_into(self.world, eye, out)
    }

    /// Extract flat draw list needed for render, no mutable ECS access exposed.
    ///
    /// Delegates to [`crate::scene::systems::render::scene_render_system`]: this used to be
    /// a copy-pasted implementation, two query conditions (especially whether `MaterialRef` required) would drift.
    pub fn draw_items(&self) -> Vec<prism_render::DrawItem> {
        crate::scene::systems::render::scene_render_system(self.world)
    }

    /// [`Self::draw_items`] into a caller-owned buffer (frame-packet reuse).
    pub fn draw_items_into(&self, out: &mut Vec<prism_render::DrawItem>) {
        crate::scene::systems::render::scene_render_system_into(self.world, out)
    }

    /// [`Self::draw_items_into`] with camera frustum culling (#491).
    ///
    /// `frustum == None` equivalent to [`Self::draw_items_into`].
    pub fn draw_items_with_frustum_into(
        &self,
        frustum: Option<&prism_render::culling::Frustum>,
        out: &mut Vec<prism_render::DrawItem>,
    ) {
        crate::scene::systems::render::scene_render_system_into_with_frustum(
            self.world, frustum, out,
        )
    }

    /// [`Self::draw_items_with_frustum_into`] for **all** composited cameras
    /// (#461 / #491). Empty slice = no cull.
    pub fn draw_items_with_frustums_into(
        &self,
        frustums: &[prism_render::culling::Frustum],
        out: &mut Vec<prism_render::DrawItem>,
    ) {
        crate::scene::systems::render::scene_render_system_into_with_frustums(
            self.world, frustums, out,
        )
    }
}

/// Resolve relative asset paths in scene into package-internal paths.
///
/// Example: scene `scenes/default.rscn` + `../valley.hdr` -> `valley.hdr`;
/// `assets/valley.hdr` -> `valley.hdr`.
fn normalize_scene_asset_path(scene_path: Option<&str>, asset_path: &str) -> String {
    let asset_path = asset_path.replace('\\', "/");
    if let Some(rest) = asset_path.strip_prefix("assets/") {
        return rest.to_string();
    }
    if asset_path.starts_with('/') {
        return asset_path;
    }

    let mut parts: Vec<&str> = Vec::new();
    if let Some(dir) = scene_path.and_then(|path| path.rsplit_once('/').map(|(dir, _)| dir)) {
        parts.extend(dir.split('/').filter(|part| !part.is_empty()));
    }
    for part in asset_path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

impl Default for SceneManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SceneManager {
    /// Create an empty scene system (no current scene yet).
    pub fn new() -> Self {
        Self {
            current_scene_name: None,
            current_scene_path: None,
            environment_bytes: None,
            missing_scene: false,
            generation: 0,
        }
    }

    /// At startup no loadable scene (usually `assets/*.pak` not packed).
    ///
    /// Render side uses it to decide whether to draw "no scene" placeholder image; clear color semantics unaffected.
    pub fn is_missing_scene(&self) -> bool {
        self.missing_scene
    }

    /// Current scene name; `None` when no scene loaded from manifest yet.
    pub fn current_scene_name(&self) -> Option<&str> {
        self.current_scene_name.as_deref()
    }

    /// Scene generation (from 0, +1 on each switch / unload).
    ///
    /// Host (editor / player) passes it to project module, module discards cached entity handles:
    /// after scene switch entities pointed by old handles no longer exist.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Increment generation and return new value.
    pub fn bump_generation(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1).max(1);
        self.generation
    }

    /// Adopt just-spawned **author-format** scene (`Engine::load_scene_json_file`).
    ///
    /// `additive = false` (`LoadSceneMode::Single`): unload old scene entities, clear
    /// current_scene / env cache and increment generation — loading again won't double entities.
    /// `additive = true`: only load without unload, `Persistent` and old scene both stay.
    ///
    /// Must call **after new scene spawned successfully**: on failure keep old scene unchanged.
    pub fn adopt_author_scene(
        &mut self,
        world: &mut World,
        inst: &SceneInstance,
        additive: bool,
    ) -> usize {
        if additive {
            log::info!(
                "scene: additive load kept {} existing scene entity(ies)",
                world.query::<super::components::SceneMember>().count()
            );
            return 0;
        }
        let unloaded = unload_scene_members(world, Some(inst.scene_id));
        self.current_scene_name = None;
        self.current_scene_path = None;
        self.environment_bytes = None;
        self.bump_generation();
        unloaded
    }

    /// Unload current scene (except `Persistent` subtree), return number of entities unloaded.
    ///
    /// Difference from [`Self::adopt_scene`]: here no new scene loaded, for
    /// "return to main menu" / "restart a round" cases that only need cleanup. Engine-owned entities
    /// (without `SceneMember`, e.g. `runtime_initialize`'s fallback camera) not deleted.
    pub fn unload_scene(&mut self, world: &mut World) -> usize {
        let unloaded = unload_scene_members(world, None);
        let had_scene = self.current_scene_name.is_some();
        self.current_scene_name = None;
        self.current_scene_path = None;
        self.environment_bytes = None;
        self.bump_generation();
        if had_scene || unloaded > 0 {
            log::info!(
                "scene unloaded: {unloaded} entity(ies) removed (generation {})",
                self.generation
            );
        }
        unloaded
    }

    /// Inject environment texture bytes for current scene's declarative lighting. Bytes provided by runtime asset service, scene system
    /// not responsible for file reads or format decode.
    pub fn set_environment_bytes(&mut self, bytes: Option<Vec<u8>>) {
        self.environment_bytes = bytes;
    }

    /// Load **first** scene already registered in asset package from `assets/scenes.json` manifest.
    ///
    /// Engine init (`Engine::init_scene`) delegates here: scenes always loaded by `AssetId`
    /// from [`ResourceManager`] (`scenes.json` `path` → path manifest → `AssetId`).
    /// Returns successfully loaded scene name and records as current; returns `None` when no manifest / no available scene.
    pub fn load_first_from_manifest(
        &mut self,
        rm: &mut ResourceManager,
        world: &mut World,
        registry: &ComponentRegistry,
    ) -> Option<String> {
        self.load_from_manifest(rm, world, registry, None)
    }

    /// Load specified scene by name from `assets/scenes.json`.
    ///
    /// When name not found in manifest or load fails, keep "current scene" unchanged and return `None`.
    pub fn load_named_from_manifest(
        &mut self,
        rm: &mut ResourceManager,
        world: &mut World,
        registry: &ComponentRegistry,
        name: &str,
    ) -> Option<String> {
        self.load_from_manifest(rm, world, registry, Some(name))
    }

    /// Load scene from caller-provided in-memory manifest.
    ///
    /// Runtime host can inject manifest from `.pak`, Android assets or network resources, avoiding
    /// scene system depending on current working directory. File entry kept only as compatibility layer.
    pub fn load_from_manifest_text(
        &mut self,
        rm: &mut ResourceManager,
        world: &mut World,
        registry: &ComponentRegistry,
        text: &str,
        scene_name: Option<&str>,
    ) -> Option<String> {
        let manifest: SceneManifest = match SceneManifest::parse(text) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("scene manifest parse error: {e}");
                self.missing_scene = true;
                return None;
            }
        };
        self.load_entries(rm, world, registry, &manifest, scene_name)
    }

    /// Resolve environment texture bytes needed for "current scene" declarative lighting.
    ///
    /// Replaces old [`load_env_bytes_from_manifest`], which scanned all `.rscn` in manifest,
    /// causing UI startup screens to also load 3D environment textures. New logic determines scene by `current_scene_name`:
    /// - `.rscn`: reuse `read_env_path_from_rscn` auto-derivation (backward compatible, default / sponza
    ///   IBL preserved without scene changes).
    /// - `.scene.json`: query `EnvironmentLighting` component from already-loaded ECS world;
    ///   no such component or `env_map = None` → return `None` (startup screens without lighting don't build IBL).
    #[cfg(feature = "legacy-disk-scenes")]
    #[deprecated(
        note = "use current_scene_env_bytes_with_provider; this fallback reads scene files"
    )]
    pub fn current_scene_env_bytes(&self, world: &World) -> Option<Vec<u8>> {
        if let Some(bytes) = &self.environment_bytes {
            return Some(bytes.clone());
        }
        let name = self.current_scene_name.as_ref()?;
        let (manifest_dir, manifest) = find_and_parse_manifest()?;
        let entry = manifest.scenes.iter().find(|e| &e.name == name)?;
        let path = manifest_dir.join(&entry.path);

        if entry.path.ends_with(".rscn") {
            if let Some(hdr_rel) = super::loader::read_env_path_from_rscn(&path) {
                let hdr_path = path
                    .parent()
                    .map(|d| d.join(&hdr_rel))
                    .unwrap_or_else(|| PathBuf::from(&hdr_rel));
                match std::fs::read(&hdr_path) {
                    Ok(bytes) => {
                        log::info!(
                            "scene '{}' environment (rscn): {}",
                            name,
                            hdr_path.display()
                        );
                        return Some(bytes);
                    }
                    Err(e) => log::warn!("env map {} not readable: {e}", hdr_path.display()),
                }
            }
            return None;
        }

        if entry.path.ends_with(".scene.json") {
            for (_, env) in world.query::<super::components::EnvironmentLighting>() {
                if let Some(env_map) = &env.env_map {
                    let hdr_path = path
                        .parent()
                        .map(|d| d.join(env_map))
                        .unwrap_or_else(|| PathBuf::from(env_map));
                    match std::fs::read(&hdr_path) {
                        Ok(bytes) => {
                            log::info!(
                                "scene '{}' environment (component): {}",
                                name,
                                hdr_path.display()
                            );
                            return Some(bytes);
                        }
                        Err(e) => {
                            log::warn!("env map {} not readable: {e}", hdr_path.display())
                        }
                    }
                }
                return None;
            }
        }
        None
    }

    /// Resolve environment textures using caller-provided asset reader, no filesystem access.
    pub fn current_scene_env_bytes_with_provider<P: EnvironmentProvider>(
        &self,
        view: SceneReadView<'_>,
        provider: &mut P,
    ) -> Option<Vec<u8>> {
        if let Some(bytes) = &self.environment_bytes {
            return Some(bytes.clone());
        }
        if let Some(path) = view.environment_asset_path() {
            let normalized = normalize_scene_asset_path(self.current_scene_path.as_deref(), &path);
            return provider.load_environment(&normalized);
        }
        None
    }

    fn fatal_scene_pak_error(msg: impl std::fmt::Display) -> ! {
        log::error!("{msg}");
        eprintln!("FATAL: {msg}");
        std::process::exit(1);
    }

    fn read_packaged_manifest(rm: &mut ResourceManager) -> Result<SceneManifest, String> {
        let id = rm
            .id_by_path("scenes.json")
            .ok_or_else(|| "missing `scenes.json` inside resource package".to_string())?;
        let bytes = rm
            .load_with_deps_raw(id)
            .map_err(|e| format!("failed to read packaged `scenes.json`: {e}"))?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|e| format!("packaged `scenes.json` is not valid UTF-8: {e}"))?;
        SceneManifest::parse(text).map_err(|e| format!("packaged `scenes.json` parse error: {e}"))
    }

    /// Resolve and load scenes one by one from manifest; `scene_name = None` means first loadable scene.
    fn load_from_manifest(
        &mut self,
        rm: &mut ResourceManager,
        world: &mut World,
        registry: &ComponentRegistry,
        scene_name: Option<&str>,
    ) -> Option<String> {
        let strict = crate::asset_paths::strict_pak_only();
        if strict {
            let manifest = match Self::read_packaged_manifest(rm) {
                Ok(m) => m,
                Err(err) => Self::fatal_scene_pak_error(err),
            };
            log::info!(
                "scene manifest (packaged): {} scene(s) listed",
                manifest.scenes.len()
            );
            let loaded = self.load_entries(rm, world, registry, &manifest, scene_name);
            if loaded.is_none() {
                Self::fatal_scene_pak_error(
                    "no loadable enabled scene found in packaged `scenes.json`",
                );
            }
            return loaded;
        }

        let manifest_path = find_manifest_path();
        let manifest_path = match manifest_path {
            Some(p) => p,
            None => {
                if let Ok(manifest) = Self::read_packaged_manifest(rm) {
                    log::info!(
                        "scene manifest (packaged fallback): {} scene(s) listed",
                        manifest.scenes.len()
                    );
                    return self.load_entries(rm, world, registry, &manifest, scene_name);
                }
                #[expect(
                    clippy::disallowed_methods,
                    reason = "diagnostic log only when scene manifest is missing"
                )]
                let cwd = std::env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| "<unknown>".into());
                log::warn!("no assets/scenes.json found (cwd={cwd}); no scene will be loaded");
                self.missing_scene = true;
                return None;
            }
        };
        let text = match std::fs::read_to_string(&manifest_path) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("failed to read scene manifest {:?}: {e}", manifest_path);
                self.missing_scene = true;
                return None;
            }
        };
        log::info!("scene manifest: {:?} ({} bytes)", manifest_path, text.len());
        let mut manifest: SceneManifest = match SceneManifest::parse(&text) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("scene manifest parse error: {e}");
                self.missing_scene = true;
                return None;
            }
        };
        if let Some(assets_root) = manifest_path.parent() {
            resolve_manifest_guids(&mut manifest, assets_root);
        }
        log::info!(
            "scene manifest parsed: {} scene(s) listed",
            manifest.scenes.len()
        );
        self.load_entries(rm, world, registry, &manifest, scene_name)
    }

    fn load_entries(
        &mut self,
        rm: &mut ResourceManager,
        world: &mut World,
        registry: &ComponentRegistry,
        manifest: &SceneManifest,
        scene_name: Option<&str>,
    ) -> Option<String> {
        for entry in &manifest.scenes {
            if !entry.enabled {
                continue;
            }
            if let Some(name) = scene_name {
                if entry.name != name {
                    continue;
                }
            }
            let Some(id) = rm.id_by_path(&entry.path) else {
                log::warn!(
                    "scene '{}' -> '{}' not registered in resource package; skipping",
                    entry.name,
                    entry.path
                );
                continue;
            };
            match load_scene_by_id(rm, world, id, registry) {
                Ok(inst) => {
                    let unloaded = self.adopt_scene(world, &entry.name, &entry.path, &inst);
                    log::info!(
                        "scene '{}' loaded: {} entities ({} roots), unloaded {unloaded} entity(ies) from the previous scene",
                        entry.name,
                        inst.all_entities.len(),
                        inst.root_entities.len()
                    );
                    return Some(entry.name.clone());
                }
                Err(e) => {
                    log::warn!("scene '{}' failed to load: {e}", entry.name);
                    continue;
                }
            }
        }
        self.missing_scene = true;
        let command = match crate::asset_paths::asset_dir() {
            Some(dir) => crate::asset_paths::build_scenes_command(&dir),
            None => "cargo run -q -p prism-build-pipeline -- build-scenes".to_string(),
        };
        log::error!(
            "no resolvable scene: the window shows the no-scene placeholder. Scene bytes are \
             read from `.pak` only (no disk fallback). Build the package, then restart: {command}"
        );
        if let Some(name) = scene_name {
            log::error!("requested scene '{name}' was not found in the manifest");
        }
        None
    }

    /// Adopt just-successfully-spawned new scene: unload old scene entities, clear environment texture cache,
    /// and switch "current scene" to new scene.
    ///
    /// Returns number of old scene entities unloaded. Entities not belonging to any scene (e.g.
    /// fallback camera generated by `Engine::runtime_initialize`) kept — owned by engine not
    /// scene.
    fn adopt_scene(
        &mut self,
        world: &mut World,
        name: &str,
        path: &str,
        inst: &SceneInstance,
    ) -> usize {
        let unloaded = unload_scene_members(world, Some(inst.scene_id));
        self.current_scene_name = Some(name.to_owned());
        self.missing_scene = false;
        self.current_scene_path = Some(path.to_owned());
        self.environment_bytes = None;
        self.bump_generation();
        unloaded
    }
}

/// Load environment map table bytes from **first** scene in `scenes.json`.
///
/// Legacy manual entry — already replaced by [`SceneManager::current_scene_env_bytes`]
/// (latter only resolves **current** scene, avoids UI startup screens loading 3D environment textures).
#[cfg(feature = "legacy-disk-scenes")]
#[deprecated(note = "load environment data through EnvironmentProvider")]
pub fn load_env_bytes_from_manifest() -> Option<Vec<u8>> {
    let (manifest_dir, manifest) = find_and_parse_manifest()?;
    for entry in &manifest.scenes {
        let path = manifest_dir.join(&entry.path);
        if !path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("rscn"))
            .unwrap_or(false)
            || !path.exists()
        {
            continue;
        }
        if let Some(hdr_rel) = super::loader::read_env_path_from_rscn(&path) {
            let hdr_path = path
                .parent()
                .map(|d| d.join(&hdr_rel))
                .unwrap_or_else(|| PathBuf::from(&hdr_rel));
            match std::fs::read(&hdr_path) {
                Ok(bytes) => {
                    log::info!("loaded environment map from scene: {}", hdr_path.display());
                    return Some(bytes);
                }
                Err(e) => log::warn!("env map HDR {} not readable: {e}", hdr_path.display()),
            }
        }
    }
    log::info!("no environment map in scene manifest; using procedural fallback");
    None
}

pub use prism_scene::{SceneManifest, SceneManifestEntry};

/// Resolve `scenes.json` GUID references against the source `assets/` tree.
///
/// The manifest stores logical pak paths (`.rscn`), while `.rmeta` sidecars
/// belong to authoring documents (`.scene.json`). Preserve the `.rscn` style
/// when a GUID resolves to its source document so the runtime lookup key keeps
/// pointing at the cooked asset.
fn resolve_manifest_guids(manifest: &mut SceneManifest, assets_root: &std::path::Path) {
    let index = prism_asset::GuidIndex::scan(assets_root);
    for entry in &mut manifest.scenes {
        let Some(guid) = entry.guid.as_deref() else {
            continue;
        };
        let Some(resolved) = index.path_for_guid(guid) else {
            continue;
        };
        entry.path = manifest_style_path(&entry.path, resolved);
    }
}

fn manifest_style_path(current: &str, resolved: &str) -> String {
    if current.ends_with(".rscn") && resolved.ends_with(".scene.json") {
        format!("{}.rscn", resolved.trim_end_matches(".scene.json"))
    } else {
        resolved.to_string()
    }
}

/// Locate scene manifest without depending on current working directory:
/// in order of [`crate::asset_paths::search_roots`] (`PRISMA_ASSET_ROOT`,
/// current dir and parents, executable dir and parents), first find
/// `assets/scenes.json`, then fall back to old project's `assets/scenes.toml`.
///
/// So whether starting from repo root or `projects/template/` subdirectory,
/// can reliably find asset manifest — previously starting from user project directory caused
/// manifest not found, fallback to empty scene and black screen.
fn find_manifest_path() -> Option<PathBuf> {
    const NAMES: [&str; 2] = ["scenes.json", "scenes.toml"];
    const DIRS: [&str; 2] = ["assets", "crates/prism-engine/assets"];
    for base in crate::asset_paths::search_roots() {
        for name in NAMES {
            for dir in DIRS {
                let candidate = base.join(dir).join(name);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

#[cfg(feature = "legacy-disk-scenes")]
fn find_and_parse_manifest() -> Option<(PathBuf, SceneManifest)> {
    let manifest_path = find_manifest_path()?;
    let manifest_dir = manifest_path.parent()?.to_path_buf();
    let text = std::fs::read_to_string(&manifest_path).ok()?;
    let mut manifest = SceneManifest::parse(&text).ok()?;
    resolve_manifest_guids(&mut manifest, &manifest_dir);
    log::info!(
        "scene manifest: {:?} ({} entries)",
        manifest_path,
        manifest.scenes.len()
    );
    Some((manifest_dir, manifest))
}

fn load_scene_by_id(
    rm: &mut ResourceManager,
    world: &mut World,
    id: AssetId,
    registry: &ComponentRegistry,
) -> Result<SceneInstance, anyhow::Error> {
    use anyhow::Context;
    let handle = rm
        .load_with_deps::<SceneAsset>(id)
        .with_context(|| format!("load scene {id}"))?;
    let asset = rm
        .get::<SceneAsset>(handle)
        .with_context(|| format!("get scene {id}"))?;
    let mut loader = SceneLoader::new();
    loader
        .load_and_spawn(world, SceneSource::RawCooked(asset.bytes.clone()), registry)
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Unload all scene entities not belonging to `keep`, return number removed.
///
/// Ownership determined by [`SceneMember`](super::components::SceneMember): entities without that
/// component (engine-owned entities, user runtime-spawned objects) all kept.
/// Unload all entities belonging to "non-`keep` scenes", return unload count.
///
/// `keep = None` means unload **all** scene entities (`Engine::unload_scene`).
/// Following two classes of entities always kept:
/// - entities without `SceneMember` (engine-owned: fallback camera, host-injected objects);
/// - entities with [`super::components::Persistent`] and whole subtree (Unity's
///   `DontDestroyOnLoad`).
fn unload_scene_members(world: &mut World, keep: Option<super::components::SceneAssetId>) -> usize {
    let candidates: Vec<Entity> = world
        .query::<super::components::SceneMember>()
        .filter(|(_, member)| Some(member.0) != keep)
        .map(|(entity, _)| entity)
        .collect();
    if candidates.is_empty() {
        return 0;
    }

    let keep_set = persistent_subtrees(world);
    let mut unloaded = 0;
    for entity in candidates {
        if keep_set.contains(&entity) {
            continue;
        }
        if world.is_alive(entity) {
            world.despawn(entity);
            unloaded += 1;
        }
    }
    warn_dangling_parents(world);
    unloaded
}

/// Entities with `Persistent` flag and all descendants (retention list).
fn persistent_subtrees(world: &World) -> std::collections::HashSet<Entity> {
    let mut keep = std::collections::HashSet::new();
    let roots: Vec<Entity> = world
        .query::<super::components::Persistent>()
        .map(|(entity, _)| entity)
        .collect();
    if roots.is_empty() {
        return keep;
    }
    let mut stack = roots;
    while let Some(entity) = stack.pop() {
        if !keep.insert(entity) {
            continue;
        }
        if let Some(children) = world.get::<super::components::Child>(entity) {
            stack.extend(children.0.iter().copied());
        }
    }
    keep
}

/// Check broken parent-child relationships after unload and warn once.
///
/// Typical case: parent belonged to old scene deleted, child with `Persistent` stayed —
/// layer level already has no parent node, treating as "has parent" would compute wrong world matrix.
/// Here only warn, don't change structure for user.
fn warn_dangling_parents(world: &World) {
    let dangling: Vec<Entity> = world
        .query::<super::components::Parent>()
        .filter(|(entity, parent)| world.is_alive(*entity) && !world.is_alive(parent.0))
        .map(|(entity, _)| entity)
        .collect();
    if dangling.is_empty() {
        return;
    }
    log::warn!(
        "{} entity(ies) kept across the scene switch have a dead parent; \
         reparent them or clear the hierarchy (dangling: {:?})",
        dangling.len(),
        dangling
    );
}

#[cfg(test)]
#[path = "scene_system_tests.rs"]
mod tests;
