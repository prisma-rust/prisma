//! Embedded engine instance in editor (**zero-copy** host).
//!
//! Unlike `prism-app` hosted mode, here **no render thread created**:
//! editor drives `Engine` + `GraphRenderer` synchronously on egui draw thread,
//! render result stays in `GraphRenderer`'s offscreen image, then `VkImageView` handed
//! to egui for sampling ([`crate::viewport`]). Therefore:
//!
//! - only one queue, one thread → no cross-thread semaphore / fence needed;
//! - no `vkCmdCopyImageToBuffer`, no CPU pixel readback.
//!
//! Asset upload also done synchronously: `GpuAssetResolver::prepare_requests` → (renderer)
//! `apply_asset_requests` → `apply_results` writes back ECS, two sections originally on separate threads
//! chained together here.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use glam::{Mat4, Quat, Vec3};
use prism_engine::asset_resolver::GpuAssetResolver;
use prism_engine::ecs::{Entity, World};
use prism_engine::input::InputManager;
use prism_engine::render_settings::RenderSettings;
use prism_engine::render_system::{
    extract_frame_packet_into_with_shadow_cascades_and_render_height, FramePacket,
};
use prism_engine::scene::components::{
    Active, Animator, Camera, Hidden, LocalToWorld, LocalTransform, Parent, SceneEntityId,
};
use prism_engine::ui::ComputedLayout;
use prism_engine::Engine;
use prism_render::descriptor::FrameUBOData;
use prism_render::{FrameInput, GraphRenderer, VulkanContext};

use crate::diagnostics::Diagnostic;
use crate::project_module::{ProjectEvent, ProjectModuleDriver, ProjectModuleStatus};

/// Viewport kind: Scene view uses editor orbit camera, Game view uses authored camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewKind {
    Scene,
    Game,
}

#[allow(dead_code)]
impl ViewKind {
    pub fn icon(self) -> &'static str {
        match self {
            ViewKind::Scene => "🎬",
            ViewKind::Game => "🎮",
        }
    }
}

/// Engine instance behind one viewport.
pub struct EngineHost {
    kind: ViewKind,
    /// User project module (only Game view attaches).
    ///
    /// **Must be declared before `engine`**: project session callback pointers point to host world,
    /// unload order must be "stop project code first, then tear down engine".
    project: Option<ProjectModuleDriver>,
    /// Scene rebuild generation: project module uses it to check entity handle invalidation.
    scene_generation: u64,
    engine: Engine,
    asset_resolver: GpuAssetResolver,
    renderer: GraphRenderer,
    input: InputManager,
    packet: FramePacket,
    settings: RenderSettings,
    /// Entity list in `.scene.json` document order (= layer level / checker subscript).
    entities: Vec<Entity>,
    editor_camera: Option<Entity>,
    width: u32,
    height: u32,
    /// Whether Scene view uses scene skybox as background (engine default on).
    show_skybox: bool,
    /// Entity ids hidden at session level in Scene view (#417). Game view always empty.
    hidden_entities: HashSet<u32>,
    /// Whether Scene view currently 2D (orthographic) view (#641). Synced each frame by
    /// [`Self::place_editor_camera`], used to disable XZ mesh seen edge-on.
    view_2d: bool,
    /// Path to write offscreen image back after next frame (editor screenshot).
    capture_path: Option<PathBuf>,
    simulate: bool,
    paused: bool,
    step_pending: bool,
    accumulator: Duration,
    last_tick: Instant,
    scene_loaded: bool,
    /// Path of environment texture already convolved into renderer (`None` = no IBL).
    ///
    /// Reconing IBL just for a rename is most expensive part of whole reload (#435), so only
    /// re-run `set_environment` when scene-declared environment texture actually changed.
    env_path: Option<String>,
    /// Modify time of `assets/scenes.pak`: only re-read it when pak itself rebuilt.
    pak_mtime: Option<std::time::SystemTime>,
    /// Time of last asset change check (500ms throttled, see [`EngineHost::poll_asset_changes`]).
    asset_check_at: Instant,
    /// Project root used to resolve loose scene asset paths such as GI probe volumes.
    project_root: Option<PathBuf>,
    /// Last GI probe volume state applied to the renderer `(path, mtime, enabled)`.
    loaded_gi: Option<(Option<PathBuf>, Option<std::time::SystemTime>, bool)>,
    /// Previous frame view-projection matrix for motion vectors and TAA.
    prev_view_proj: Option<[[f32; 4]; 4]>,
}

/// Editor ground mesh only appears in Scene view.
///
/// Game view represents independent build: mesh is editor helper object, not part of game image
/// (#454). Another layer of guarding here, avoid it leaking in via other paths.
fn editor_grid_visible(
    kind: ViewKind,
    settings: &prism_engine::render_settings::RenderSettings,
    view_2d: bool,
) -> bool {
    // In 2D view XZ mesh is a line seen edge-on (XY mesh see #602), don't draw first.
    kind == ViewKind::Scene && settings.show_grid && !view_2d
}

impl EngineHost {
    pub fn new(
        kind: ViewKind,
        context: Arc<VulkanContext>,
        width: u32,
        height: u32,
        shadow_map_size: u32,
        shadow_cascade_count: u32,
    ) -> anyhow::Result<Self> {
        let mut engine = Engine::empty();
        engine.runtime_initialize();

        let mut asset_resolver = GpuAssetResolver::new();
        asset_resolver.load_resource_package();

        let width = width.max(1);
        let height = height.max(1);
        let renderer = GraphRenderer::offscreen_new_with_context_and_shadow_size(
            context,
            width,
            height,
            None,
            shadow_map_size,
            shadow_cascade_count,
        )
        .context("create offscreen renderer")?;

        Ok(Self {
            kind,
            project: None,
            scene_generation: 0,
            engine,
            asset_resolver,
            renderer,
            input: InputManager::new(),
            packet: FramePacket::default(),
            settings: RenderSettings::default(),
            entities: Vec::new(),
            editor_camera: None,
            width,
            height,
            show_skybox: true,
            hidden_entities: HashSet::new(),
            view_2d: false,
            capture_path: None,
            simulate: false,
            paused: false,
            step_pending: false,
            accumulator: Duration::ZERO,
            last_tick: Instant::now(),
            scene_loaded: false,
            env_path: None,
            pak_mtime: resource_package_mtime(),
            asset_check_at: Instant::now(),
            project_root: None,
            loaded_gi: None,
            prev_view_proj: None,
        })
    }

    /// Set the workspace project root so loose relative paths resolve reliably.
    pub fn set_project_root(&mut self, root: Option<PathBuf>) {
        self.project_root = root;
        self.loaded_gi = None;
    }

    /// Attach driver for user project module to Game view (Scene view doesn't run project code).
    pub fn attach_project_module(&mut self, root: &Path) {
        self.project_root = Some(root.to_path_buf());
        if self.kind != ViewKind::Game || self.project.is_some() {
            return;
        }
        self.project = Some(ProjectModuleDriver::new(root));
    }

    pub fn project_module_status(&self) -> Option<ProjectModuleStatus> {
        self.project.as_ref().map(ProjectModuleDriver::status)
    }

    /// Problems panel details (status only holds count, avoid cloning diagnostics each frame).
    pub fn project_diagnostics(&self) -> &[Diagnostic] {
        self.project
            .as_ref()
            .map(ProjectModuleDriver::diagnostics)
            .unwrap_or_default()
    }

    /// Poll each frame: collect compile result / compile as needed. Doesn't block UI thread.
    pub fn poll_project_module(&mut self, playing: bool) {
        if let Some(project) = self.project.as_mut() {
            project.poll(playing);
        }
    }

    pub fn drain_project_events(&mut self) -> Vec<ProjectEvent> {
        self.project
            .as_mut()
            .map(ProjectModuleDriver::drain_events)
            .unwrap_or_default()
    }

    pub fn refresh_project_module(&mut self) {
        if let Some(project) = self.project.as_mut() {
            project.request_refresh();
        }
    }

    pub fn set_project_auto_refresh(&mut self, enabled: bool) {
        if let Some(project) = self.project.as_mut() {
            project.set_auto_refresh(enabled);
        }
    }

    pub fn project_auto_refresh(&self) -> bool {
        self.project
            .as_ref()
            .map(ProjectModuleDriver::auto_refresh)
            .unwrap_or(true)
    }

    pub fn kind(&self) -> ViewKind {
        self.kind
    }

    pub fn input_mut(&mut self) -> &mut InputManager {
        &mut self.input
    }

    /// Map selected subscript in editor document to ECS entity id, hand to renderer for outline (#487).
    ///
    /// `enabled == false` or subscript out of range clears list; projection wireframe of
    /// parent / child still handled by viewport overlay layer, here only directly selected meshes.
    pub fn set_selection_outline(&mut self, selected: &[usize], enabled: bool) {
        let ids = selection_entity_ids(&self.entities, selected, enabled);
        self.renderer.set_outline_entities(&ids);
    }

    /// Sync Scene view's session-level hidden set to ECS (#417).
    ///
    /// Engine only reads [`Hidden`] in render-type systems, so physics / scripts / documents unaffected.
    /// Return early when set unchanged; after scene rebuild re-stamp via [`Self::apply_scene_hidden`].
    pub fn set_scene_hidden(&mut self, hidden: &[u32]) {
        let next: HashSet<u32> = hidden.iter().copied().collect();
        if self.hidden_entities == next {
            return;
        }
        self.hidden_entities = next;
        self.apply_scene_hidden();
    }

    fn apply_scene_hidden(&mut self) {
        let hidden = self.hidden_entities.clone();
        let entities = self.entities.clone();
        let world = self.engine.world_mut();
        for entity in entities {
            let Some(id) = world.get::<SceneEntityId>(entity).map(|id| id.0) else {
                continue;
            };
            if hidden.contains(&id) {
                world.insert(entity, Hidden(true));
            } else {
                world.remove::<Hidden>(entity);
            }
        }
    }

    /// Cooked mesh local AABB for document subscript (precise Scene picking, #600).
    ///
    /// Returns `None` when scene hasn't finished asset upload, caller falls back to unit cube.
    pub fn mesh_bounds(&self, index: usize) -> Option<([f32; 3], [f32; 3])> {
        let entity = *self.entities.get(index)?;
        let world = self.engine.world();
        let mesh = world.get::<prism_engine::scene::runtime_components::MeshRef>(entity)?;
        let bounds = world
            .get_resource::<prism_engine::scene::systems::render::MeshBounds>()?
            .get(mesh.render_handle)?;
        Some((bounds.min, bounds.max))
    }

    /// Selected entity's joints positions and parent-child lines in world space (#560 skeleton overlay).
    ///
    /// Need three things present: entity `BonePalette` (written each frame by animation system),
    /// skeleton path on `SkeletonRoot`, and skeleton already loaded in `SkeletonLibrary`.
    /// Missing any returns empty — shouldn't draw a pile of origins before skinned mesh uploaded.
    pub fn skeleton_overlay(&self, index: usize) -> crate::skeleton_overlay::SkeletonOverlay {
        let empty = || crate::skeleton_overlay::SkeletonOverlay {
            joints: Vec::new(),
            segments: Vec::new(),
        };
        let Some(entity) = self.entities.get(index).copied() else {
            return empty();
        };
        let world = self.engine.world();
        let Some(palette) =
            world.get::<prism_engine::scene::systems::animator::BonePalette>(entity)
        else {
            return empty();
        };
        let Some(root) = world.get::<prism_engine::scene::components::SkeletonRoot>(entity) else {
            return empty();
        };
        let Some(library) =
            world.get_resource::<prism_engine::scene::systems::animator::SkeletonLibrary>()
        else {
            return empty();
        };
        let Some(skeleton) = library.skeleton(&root.skeleton_path) else {
            return empty();
        };

        let parents: Vec<Option<usize>> =
            skeleton.joints().iter().map(|joint| joint.parent).collect();
        let inverse_binds: Vec<[[f32; 4]; 4]> = skeleton
            .joints()
            .iter()
            .map(|joint| joint.inverse_bind.to_cols_array_2d())
            .collect();
        let local_to_world = world
            .get::<LocalToWorld>(entity)
            .map(|ltw| ltw.0.to_cols_array_2d())
            .unwrap_or_else(|| Mat4::IDENTITY.to_cols_array_2d());
        crate::skeleton_overlay::bone_overlay(
            &palette.matrices,
            &parents,
            &inverse_binds,
            &local_to_world,
        )
    }

    /// Sprite actual texture size (so Scene picking can know visible range even with
    /// `frame_size = [0,0]`, rather than falling back to unit cube, #600).
    pub fn sprite_texture_size(&self, index: usize) -> Option<[f32; 2]> {
        let entity = *self.entities.get(index)?;
        let sprite = self
            .engine
            .world()
            .get::<prism_engine::scene::runtime_components::SpriteRef>(entity)?;
        Some([sprite.size[0] as f32, sprite.size[1] as f32])
    }

    pub fn world(&self) -> &World {
        self.engine.world()
    }

    /// After editor brush changes heightmap, drop runtime `TerrainData` so terrain system next frame
    /// regenerates mesh from disk (#517).
    pub fn reload_terrain_heightmap(&mut self, scene_id: u32) {
        let world = self.engine.world_mut();
        let entity = world
            .query::<SceneEntityId>()
            .find(|(_, id)| id.0 == scene_id)
            .map(|(entity, _)| entity);
        if let Some(entity) = entity {
            world.remove::<prism_engine::scene::systems::terrain::TerrainData>(entity);
        }
    }

    /// UI screen rect for document subscript and parent rect used to resolve it (#429).
    ///
    /// Both in offscreen target pixel space, same coordinates as engine `ComputedLayout`.
    /// When parent isn't UI node (no `ComputedLayout`) fall back to whole screen; root node same.
    pub fn ui_rect(&self, index: usize) -> Option<([f32; 4], [f32; 4])> {
        let world = self.engine.world();
        let entity = *self.entities.get(index)?;
        let layout = world.get::<ComputedLayout>(entity)?;
        let root = [0.0, 0.0, self.width as f32, self.height as f32];
        let parent = world
            .get::<Parent>(entity)
            .and_then(|parent| world.get::<ComputedLayout>(parent.0))
            .map(|layout| layout.rect)
            .unwrap_or(root);
        Some((layout.rect, parent))
    }

    /// Offscreen render target size ("drawing area pixels" of Game view, differs from panel logical pixels).
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn renderer(&self) -> &GraphRenderer {
        &self.renderer
    }

    /// Export current render graph to `target/prism-render-graph.json` (analysis panel reads it).
    ///
    /// Previously editor only **read** this file, write side in `prism-app` (game process), so panel
    /// was always empty (#451). Now viewport writes it itself: graph and settings at hand, no env var needed.
    pub fn write_render_graph_report(
        &self,
        path: &std::path::Path,
        frame: u64,
    ) -> anyhow::Result<()> {
        let report =
            prism_render::render_graph_report::RenderGraphReport::from_snapshot_with_timings(
                &self.renderer.graph().snapshot(),
                self.settings.to_report_settings(),
                frame,
                Some(self.renderer.gpu_timings()),
            )
            .with_stats(self.renderer.render_stats())
            .with_draw_calls(self.renderer.draw_call_report(
                &self.packet.draw_items,
                &self.packet.sprite_items,
                &self.packet.particle_items,
            ))
            .with_gpu_history(self.renderer.gpu_history().samples());
        report.write_json(path)?;
        Ok(())
    }

    pub fn settings_mut(&mut self) -> &mut RenderSettings {
        &mut self.settings
    }

    /// Local light budget `(kept, total)`: actually drawn / enabled in scene.
    ///
    /// `total > kept` means some lights squeezed out by forward `LIGHT_MAX` (#468),
    /// viewport corner badge and checker hint accordingly.
    pub fn local_light_stats(&self) -> (usize, usize) {
        (
            self.packet.scene.local_lights_kept,
            self.packet.scene.local_lights_total,
        )
    }

    /// Local light entities squeezed out by `LIGHT_MAX` (`.scene.json` ids).
    pub fn dropped_light_ids(&self) -> &[u32] {
        &self.packet.scene.dropped_light_ids
    }

    /// Rebuild world from `.scene.json` (same path as Web editor).
    pub fn load_scene(&mut self, path: &Path) -> Result<usize, String> {
        // All entity handles of old world invalidated: project module discards cached handles by this generation.
        self.scene_generation = self.scene_generation.wrapping_add(1).max(1);
        // All entity handles of old world invalidated, helper entities need recreating.
        self.editor_camera = None;
        self.engine = Engine::empty();
        // Asset parser (with already-resolved `.pak` and uploaded handles) reused across reloads: previously each
        // edit called `GpuAssetResolver::new()` + re-read pak (#435). Only re-read manifest when pak file
        // itself was rebuilt.
        let mtime = resource_package_mtime();
        if mtime != self.pak_mtime {
            self.asset_resolver.load_resource_package();
            self.pak_mtime = mtime;
        }
        let entities = self.engine.load_scene_json_file(path)?;
        self.engine.runtime_initialize();
        self.engine
            .world_mut()
            .insert_resource(prism_engine::ui::ScreenSize::new(self.width, self.height));

        self.refresh_scene_environment();
        self.refresh_scene_gi();

        let count = entities.len();
        self.entities = entities;
        self.scene_loaded = true;
        self.prev_view_proj = None;

        // Loader only guarantees root `LocalToWorld`; propagate full hierarchy before first frame.
        prism_engine::scene::systems::hierarchy::hierarchy_system(self.engine.world_mut());
        prism_engine::scene::systems::hierarchy::active_hierarchy_system(self.engine.world_mut());

        self.ensure_helper_entities();
        self.isolate_scene_cameras();
        self.apply_scene_hidden();
        self.accumulator = Duration::ZERO;
        self.last_tick = Instant::now();
        Ok(count)
    }

    /// Synchronize transforms and active flags from the editor document in place.
    ///
    /// Returns `false` when entity count or identity no longer matches the loaded ECS world,
    /// signaling that a full `load_scene` rebuild is required.
    pub fn sync_transforms_from_doc(&mut self, doc: &prism_editor_core::doc::SceneDoc) -> bool {
        if !self.scene_loaded || self.entities.len() != doc.entities.len() {
            return false;
        }
        {
            let world = self.engine.world_mut();
            for (index, &entity) in self.entities.iter().enumerate() {
                let doc_ent = &doc.entities[index];
                if let Some(doc_id) = doc_ent.id {
                    if world.get::<SceneEntityId>(entity).map(|id| id.0) != Some(doc_id) {
                        return false;
                    }
                }
                if world.get::<LocalTransform>(entity).is_none() {
                    return false;
                }
                let r = doc_ent.transform.rotation;
                let quat = Quat::from_xyzw(r[0], r[1], r[2], r[3]);
                let rotation = if quat.is_finite() && quat.length_squared() > 1e-8 {
                    quat.normalize()
                } else {
                    Quat::IDENTITY
                };
                let local = LocalTransform {
                    position: Vec3::from(doc_ent.transform.position),
                    rotation,
                    scale: Vec3::from(doc_ent.transform.scale),
                };
                let world_mat = local.to_model_matrix();
                world.insert(entity, local);
                world.insert(entity, LocalToWorld(world_mat));
                world.insert(entity, Active(doc_ent.active()));
            }
        }
        prism_engine::scene::systems::hierarchy::hierarchy_system(self.engine.world_mut());
        prism_engine::scene::systems::hierarchy::active_hierarchy_system(self.engine.world_mut());
        true
    }

    fn isolate_scene_cameras(&mut self) {
        if self.kind != ViewKind::Scene {
            return;
        }
        let entities = self.entities.clone();
        let world = self.engine.world_mut();
        for entity in entities {
            if let Some(cam) = world.get_mut::<Camera>(entity) {
                cam.enabled = false;
            }
        }
    }

    /// Editor camera for Scene view + background policy.
    ///
    /// Engine Scene view default uses scene skybox as background (`SceneViewState.showSkybox`),
    /// so editor camera here defaults to `Skybox` not solid color.
    fn editor_clear_flags(&self) -> prism_scene::CameraClearFlags {
        if self.show_skybox {
            prism_scene::CameraClearFlags::Skybox
        } else {
            prism_scene::CameraClearFlags::SolidColor
        }
    }

    /// Toggle Scene view background (skybox / solid), immediately applies to editor camera.
    pub fn set_show_skybox(&mut self, on: bool) {
        if self.show_skybox == on {
            return;
        }
        self.show_skybox = on;
        let Some(entity) = self.editor_camera else {
            return;
        };
        let flags = self.editor_clear_flags();
        if let Some(camera) = self.engine.world_mut().get_mut::<Camera>(entity) {
            camera.clear_flags = flags;
        }
    }

    /// Editor camera + selection flag (only created in Scene view).
    fn ensure_helper_entities(&mut self) {
        if self.kind == ViewKind::Scene {
            if self.editor_camera.is_none() {
                let camera = Camera {
                    depth: 1000.0,
                    far: 5000.0,
                    aspect: self.width as f32 / self.height.max(1) as f32,
                    clear_flags: self.editor_clear_flags(),
                    background_color: [0.11, 0.12, 0.14, 1.0],
                    ..Camera::default()
                };
                let world = self.engine.world_mut();
                let entity = world.spawn();
                world.insert(entity, camera);
                world.insert(entity, LocalTransform::default());
                world.insert(entity, LocalToWorld(Mat4::IDENTITY));
                self.editor_camera = Some(entity);
            }
        }
    }

    /// Write orbit camera into editor camera entity (Scene view calls each frame).
    ///
    /// Consistent with Tauri editor: `LocalTransform` and `LocalToWorld` written together,
    /// because extract only reads `LocalToWorld`, and camera entity has no parent, no propagation.
    pub fn place_editor_camera(&mut self, orbit: &prism_editor_core::workspace::OrbitCamera) {
        let Some(entity) = self.editor_camera else {
            return;
        };
        let (yaw, pitch, distance) = (orbit.yaw, orbit.pitch, orbit.distance);
        self.view_2d = orbit.orthographic;
        let target = Vec3::from_array(orbit.target);
        // Keep the rendered camera on the same orbit as `OrbitCamera::eye()`:
        // yaw is world-Y, while positive pitch raises the eye toward +Y.
        let rotation = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(-pitch);
        let eye = target + rotation * Vec3::new(0.0, 0.0, distance.max(0.05));
        let look = rotation;

        let world = self.engine.world_mut();
        world.insert(
            entity,
            LocalTransform {
                position: eye,
                rotation: look,
                scale: Vec3::ONE,
            },
        );
        world.insert(
            entity,
            LocalToWorld(Mat4::from_rotation_translation(look, eye)),
        );
        if let Some(camera) = world.get_mut::<Camera>(entity) {
            camera.aspect = self.width as f32 / self.height.max(1) as f32;
            // 2D view: orthographic half height directly reuses `distance`, wheel zoom feel unchanged (#641).
            camera.orthographic = orbit.orthographic;
            if orbit.orthographic {
                camera.orthographic_size = orbit.distance.max(0.05);
            }
            // Editor "View..." settings (#416): FOV and clip planes apply to both render and picking,
            // otherwise gizmos / point selection on screen won't match image.
            camera.fov_y_degrees = orbit.fov_y_degrees.clamp(10.0, 170.0);
            camera.near = orbit.near;
            camera.far = orbit.far;
        }
    }

    pub fn set_simulate(&mut self, simulate: bool) {
        if self.simulate != simulate {
            // Animation graph debug snapshot only maintained during Play (#560 editor-runtime bridge).
            if simulate {
                self.engine.world_mut().insert_resource(
                    prism_engine::scene::systems::animator::AnimGraphDebug::default(),
                );
            } else {
                self.engine
                    .world_mut()
                    .remove_resource::<prism_engine::scene::systems::animator::AnimGraphDebug>();
            }
            self.accumulator = Duration::ZERO;
            self.last_tick = Instant::now();
            self.paused = false;
            self.step_pending = false;
            if !simulate {
                // On stopping Play discard transient key / mouse state: otherwise next Play
                // would inherit previous "just pressed".
                self.input = InputManager::new();
            }
        }
        self.simulate = simulate;
    }

    pub fn is_simulating(&self) -> bool {
        self.simulate
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    pub fn step(&mut self) {
        self.paused = true;
        self.step_pending = true;
    }

    pub fn resize(&mut self, width: u32, height: u32) -> anyhow::Result<()> {
        let width = width.max(1);
        let height = height.max(1);
        if (width, height) == (self.width, self.height) {
            return Ok(());
        }
        self.width = width;
        self.height = height;
        self.renderer.resize_offscreen(width, height)?;
        if self.kind == ViewKind::Scene {
            // After size change editor camera aspect must follow.
            if let Some(entity) = self.editor_camera {
                if let Some(camera) = self.engine.world_mut().get_mut::<Camera>(entity) {
                    camera.aspect = width as f32 / height as f32;
                }
            }
        }
        Ok(())
    }

    /// Scene declarative lighting: only reconvolve IBL when declared environment texture truly changed.
    fn refresh_scene_environment(&mut self) {
        let env_path =
            prism_engine::scene::SceneReadView::new(&self.engine.world()).environment_asset_path();
        if env_path == self.env_path {
            return;
        }
        let env_bytes = self
            .engine
            .current_scene_env_bytes_with_provider(&mut self.asset_resolver.resource_manager);
        if let Err(error) = self.renderer.set_environment(env_bytes) {
            log::warn!("set scene environment failed: {error:#}");
        }
        self.env_path = env_path;
    }

    /// Force-reload baked GI probe volumes from disk (e.g. after running GI Bake).
    pub fn invalidate_gi_cache(&mut self) {
        self.loaded_gi = None;
        self.refresh_scene_gi();
    }

    fn resolve_gi_volume_path(&self, raw: Option<&str>) -> Option<PathBuf> {
        let rel = raw
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("assets/gi/probe_volume.bin");
        let path = PathBuf::from(rel);
        if path.is_absolute() {
            return Some(path);
        }
        let mut candidates = Vec::new();
        if let Some(root) = self.project_root.as_ref() {
            candidates.push(root.join(&path));
            candidates.push(root.join("assets").join(&path));
        }
        candidates.push(path);
        for candidate in &candidates {
            if candidate.is_file() {
                return Some(candidate.clone());
            }
        }
        candidates.into_iter().next()
    }

    /// Synchronize scene `GiConfig` probe volume into `GraphRenderer` (when enabled).
    fn refresh_scene_gi(&mut self) {
        let gi = self
            .engine
            .world()
            .query::<prism_scene::GiConfig>()
            .next()
            .map(|(_, cfg)| (cfg.enabled, cfg.volume.clone()));
        let (enabled, raw_volume) = gi.unwrap_or((false, None));
        let resolved = if enabled {
            self.resolve_gi_volume_path(raw_volume.as_deref())
        } else {
            None
        };
        let mtime = resolved
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok().and_then(|m| m.modified().ok()));
        let key = (resolved.clone(), mtime, enabled);
        if self.loaded_gi.as_ref() == Some(&key) {
            return;
        }
        if enabled {
            if let Some(ref path) = resolved {
                if path.is_file() {
                    let loaded = self.renderer.load_probe_volume_file(path, None);
                    if loaded {
                        log::info!("editor host loaded GI probe volume: {}", path.display());
                    }
                }
            }
        }
        self.loaded_gi = Some(key);
    }

    /// Execute scene change requested by project code (#482). Called after this tick's engine update.
    ///
    /// Reuses same `Engine` (`Single` keeps `Persistent` subtree), advances
    /// `scene_generation` so project module drops old world's entity handles; old asset GPU slots
    /// accounted by parser then returned to renderer for recycling (#495). On failure keep old scene.
    fn apply_scene_request(&mut self, request: prism_runtime_host::SceneRequest) {
        match request {
            prism_runtime_host::SceneRequest::Load { path, additive } => {
                let mode = if additive {
                    prism_engine::engine::LoadSceneMode::Additive
                } else {
                    prism_engine::engine::LoadSceneMode::Single
                };
                match self
                    .engine
                    .load_scene_json_file_with_mode(Path::new(&path), mode)
                {
                    Ok(entities) => {
                        self.scene_generation = self.scene_generation.wrapping_add(1).max(1);
                        log::info!(
                            "scene switched to {path} ({} entities, additive={additive})",
                            entities.len()
                        );
                    }
                    Err(error) => {
                        log::error!(
                            "scene switch to {path} failed: {error}; keeping the current scene"
                        );
                        return;
                    }
                }
            }
            prism_runtime_host::SceneRequest::Unload => {
                let count = self.engine.unload_scene();
                self.scene_generation = self.scene_generation.wrapping_add(1).max(1);
                log::info!("scene unloaded ({count} entities)");
            }
        }
        self.engine.runtime_initialize();
        self.asset_resolver
            .forget_missing_entities(self.engine.world_mut());
        self.refresh_scene_environment();
        self.refresh_scene_gi();
        self.pump_assets();
    }

    /// Advance simulation (Play) or only propagate layer level (edit mode), finish asset upload sync.
    pub fn tick(&mut self) {
        // ECS UI layout depends on `ScreenSize`; offscreen host has no winit window, writes it itself.
        self.engine
            .world_mut()
            .insert_resource(prism_engine::ui::ScreenSize::new(self.width, self.height));
        let now = Instant::now();
        let elapsed = now
            .saturating_duration_since(self.last_tick)
            .min(Duration::from_millis(100));
        self.last_tick = now;

        let mut run_fixed = 0u32;
        if self.simulate && fixed_tick_requested(self.paused, self.step_pending) {
            const FIXED_STEP: Duration = Duration::from_nanos(16_666_667);
            if self.step_pending {
                // A paused step advances exactly one simulation tick. Do not
                // consume wall-clock time accumulated while the editor was idle.
                self.accumulator = FIXED_STEP;
            } else {
                self.accumulator += elapsed;
            }
            if let Some(project) = self.project.as_mut() {
                project.begin_frame(&self.input, [self.width as f32, self.height as f32], 0.0);
            }
            while self.accumulator >= FIXED_STEP && run_fixed < 8 {
                if let Some(project) = self.project.as_mut() {
                    project.fixed_update(
                        self.engine.world_mut(),
                        &self.input,
                        FIXED_STEP.as_secs_f32(),
                        self.scene_generation,
                        [self.width as f32, self.height as f32],
                    );
                }
                self.engine
                    .fixed_update(FIXED_STEP.as_secs_f32(), &self.input);
                self.accumulator -= FIXED_STEP;
                run_fixed += 1;
            }
            let alpha = self.accumulator.as_secs_f32() / FIXED_STEP.as_secs_f32();
            self.engine.set_interpolation_alpha(alpha);
            if let Some(project) = self.project.as_mut() {
                project.set_interpolation_alpha(alpha);
            }
        }

        if self.simulate && (!self.paused || self.step_pending) {
            let dt = if self.step_pending {
                xstep_dt()
            } else {
                elapsed.as_secs_f32()
            };
            self.step_pending = false;
            // User project gameplay code runs before engine systems: components it writes
            // and UI quads seen by hierarchy / ui::render in same frame.
            let mut scene_request = None;
            if let Some(project) = self.project.as_mut() {
                project.tick(
                    self.engine.world_mut(),
                    &self.input,
                    dt,
                    self.scene_generation,
                    [self.width as f32, self.height as f32],
                );
                scene_request = project.take_scene_request();
            }
            // Scene change requested by project code executed after world borrow ends (#482).
            if let Some(request) = scene_request {
                self.apply_scene_request(request);
            }
            self.engine.update(dt, &self.input);
            self.engine.late_update();
        } else {
            // Edit mode (or paused): don't run game systems, only propagate layer level.
            self.accumulator = Duration::ZERO;
            prism_engine::scene::systems::hierarchy::hierarchy_system(self.engine.world_mut());
            prism_engine::scene::systems::hierarchy::active_hierarchy_system(
                self.engine.world_mut(),
            );
        }

        self.pump_assets();
        self.input.begin_frame();
    }

    /// Poll `assets/scenes.pak`: if rebuilt swap reader, mark already-uploaded assets for re-parse (#495).
    ///
    /// Check at most once every 500ms (same cadence as `prism_asset` watcher). Returns number of
    /// re-parsed entities; 0 means nothing changed this frame. Upload done synchronously here (same path as
    /// [`Self::pump_assets`]), so next frame during Play sees new texture / mesh without exit & re-enter.
    ///
    /// Only handles `.pak`: runtime reads assets only from package, changing a loose texture won't enter memory.
    pub fn poll_asset_changes(&mut self) -> usize {
        const INTERVAL: Duration = Duration::from_millis(500);
        if self.asset_check_at.elapsed() < INTERVAL {
            return 0;
        }
        self.asset_check_at = Instant::now();

        let Some(pak) = prism_engine::asset_paths::asset_path("scenes.pak") else {
            return 0;
        };
        let mtime = std::fs::metadata(&pak)
            .and_then(|meta| meta.modified())
            .ok();
        if mtime == self.pak_mtime {
            return 0;
        }
        // On open failure (pak being written) wait next round, don't swap reader to half file.
        if let Err(error) = self.asset_resolver.resource_manager.on_pak_changed(&pak) {
            log::warn!(
                "hot reload: reopening {} failed: {error}; will retry",
                pak.display()
            );
            return 0;
        }
        self.pak_mtime = mtime;
        if let Some(manifest) = prism_engine::asset_paths::asset_path("scenes.pak.meta.json") {
            if let Err(error) = self
                .asset_resolver
                .resource_manager
                .load_path_manifest(&manifest)
            {
                log::warn!(
                    "hot reload: path manifest {} failed: {error}",
                    manifest.display()
                );
            }
        }
        let count = self
            .asset_resolver
            .invalidate_assets(self.engine.world_mut());
        self.pump_assets();
        log::info!(
            "hot reload: {count} entity/entities re-resolved after {} was rebuilt",
            pak.display()
        );
        count
    }

    /// Asset parsing: CPU stage → GPU upload → results write back ECS, all synchronous.
    fn pump_assets(&mut self) {
        // Project-bundled fonts (#583): read bytes from `.pak` hand to UI atlas. Only on new path
        // read once, same failure-cache strategy as textures / meshes.
        let fonts = self.asset_resolver.prepare_ui_fonts(self.engine.world());
        for (name, bytes) in fonts {
            self.renderer.register_ui_font(name, bytes);
        }

        // Skeletal animation assets (#560 P0-2): idempotent, only new `.rskel` / `.ranim` read from disk.
        self.asset_resolver
            .load_animation_assets(self.engine.world_mut());

        self.asset_resolver
            .resolve_cached_sprites(self.engine.world_mut());

        let requests = self
            .asset_resolver
            .prepare_requests(self.engine.world_mut());
        // Invalidated GPU resources released synchronously (editor has no render thread, #495).
        let releases = self.asset_resolver.take_releases();
        if !releases.is_empty() {
            self.renderer.release_assets(&releases);
        }
        if !requests.is_empty() {
            let (entities, stripped): (Vec<Entity>, Vec<_>) = requests.into_iter().unzip();
            let results = self.renderer.apply_asset_requests(stripped);
            let paired: Vec<(Entity, _)> = entities.into_iter().zip(results).collect();
            self.asset_resolver
                .apply_results(self.engine.world_mut(), &paired);
        }
    }

    /// Render one frame to offscreen image, transition image to samplable
    /// `SHADER_READ_ONLY_OPTIMAL`. Returns `(image_view, extent)`.
    pub fn render(&mut self) -> anyhow::Result<Option<(ash::vk::ImageView, ash::vk::Extent2D)>> {
        // Edit mode doesn't run `tick()` (only called during Play), but producing a frame also needs
        // those two "end of simulation step" steps: layer propagation + asset upload. Otherwise `LocalToWorld` stays at load value,
        // and `SpriteRef::generation == 0` makes 2D render system filter out all sprites —
        // edit-mode Scene / Game view would show only clear color (#579).
        self.engine
            .world_mut()
            .insert_resource(prism_engine::ui::ScreenSize::new(self.width, self.height));
        if !self.simulate {
            prism_engine::scene::systems::hierarchy::hierarchy_system(self.engine.world_mut());
            prism_engine::scene::systems::hierarchy::active_hierarchy_system(
                self.engine.world_mut(),
            );
            let animated: Vec<(Entity, f32)> = {
                let world = self.engine.world();
                self.entities
                    .iter()
                    .filter_map(|&entity| {
                        world
                            .get::<Animator>(entity)
                            .map(|anim| (entity, anim.time))
                    })
                    .collect()
            };
            if !animated.is_empty() {
                {
                    let world = self.engine.world_mut();
                    for (entity, time) in animated {
                        if let Some(playhead) = world.get_mut::<
                            prism_engine::scene::systems::animator::AnimatorPlayhead,
                        >(entity) {
                            playhead.time = if time.is_finite() { time } else { 0.0 };
                        }
                    }
                }
                prism_engine::scene::systems::animator::animator_system(
                    self.engine.world_mut(),
                    0.0,
                );
            }
            // Edit mode does not run update(); recompute UI layout for Scene anchor handles.
            prism_engine::ui::ui_layout_system(self.engine.world_mut());
        }
        self.pump_assets();
        // #613: internal render resolution follows same path as game process, pixel ratio seen by
        // editor Game view is actual device ratio.
        let aspect = self
            .settings
            .render_aspect(self.width as f32 / self.height.max(1) as f32);
        extract_frame_packet_into_with_shadow_cascades_and_render_height(
            self.engine.world_mut(),
            aspect,
            &Mat4::IDENTITY,
            self.settings.shadow_distance,
            self.settings.shadow_cascade_count,
            // #978: sub-pixel lines widened to 1 pixel by actual render height.
            self.settings.render_height(self.width, self.height),
            &mut self.packet,
        );
        self.packet.settings = self.settings.clone();
        // Local post-processing volume (#502 Phase 2): shares same parse function with player path.
        self.packet.grade = Some(
            prism_engine::scene::systems::post_process::resolve_post_grade(
                self.engine.world(),
                self.packet.scene.eye,
                self.packet.settings.post_grade(),
            ),
        );

        let extent = self.renderer.extent();
        self.renderer
            .set_taa(self.settings.taa, self.settings.taa_strength);
        let scene = &self.packet.scene;
        let current_view_proj = scene.view_proj.to_cols_array_2d();
        let prev_view_proj = self.prev_view_proj.unwrap_or(current_view_proj);
        let mut frame_data = FrameUBOData {
            view_proj: current_view_proj,
            camera_position: [
                scene.eye.x,
                scene.eye.y,
                scene.eye.z,
                scene.lights.len() as f32,
            ],
            light_direction: scene.light_direction.to_array(),
            light_color: scene.light_color.to_array(),
            view: scene.view.to_cols_array_2d(),
            light_view_proj: scene.light_view_proj.to_cols_array_2d(),
            light_view_proj1: scene.light_view_proj1.to_cols_array_2d(),
            shadow_split: scene.shadow_split,
            shadow_cascade_count: scene.shadow_cascade_count,
            tonemap_mode: self.settings.tonemap_mode,
            viewport_w: extent.width as f32,
            viewport_h: extent.height as f32,
            exposure: scene.exposure,
            shadow_strength: scene.shadow_strength,
            prev_view_proj,
            ..Default::default()
        };
        self.renderer.apply_taa_jitter(&mut frame_data);

        // #502: LUT strength follows global setting, path resolved under assets/.
        let mut resolved_grade = self
            .packet
            .grade
            .unwrap_or_else(|| self.settings.post_grade());
        resolved_grade.lut_strength = self.settings.lut_strength;
        let lut_path = if self.settings.lut.trim().is_empty() {
            None
        } else {
            prism_engine::asset_paths::asset_path(self.settings.lut.trim())
        };
        let input = FrameInput {
            draw_items: &self.packet.draw_items,
            gizmos: &self.packet.gizmos,
            sprite_items: &self.packet.sprite_items,
            sprite_lights: &self.packet.sprite_lights,
            particle_items: &self.packet.particle_items,
            bone_matrices: &self.packet.bone_matrices,
            skybox: self.packet.skybox,
            frame_data: &frame_data,
            cameras: &scene.cameras,
            light_view_proj: scene.light_view_proj.to_cols_array_2d(),
            light_view_proj1: scene.light_view_proj1.to_cols_array_2d(),
            shadow_split: scene.shadow_split,
            shadow_cascade_count: scene.shadow_cascade_count,
            inv_projection: scene.inv_projection.to_cols_array_2d(),
            debug_flags: self.settings.debug_flags,
            wireframe: self.settings.wireframe,
            shaded_wireframe: self.settings.shaded_wireframe,
            tonemap_mode: self.settings.tonemap_mode,
            aa_mode: self.settings.aa_mode,
            debug_rt: self.settings.debug_rt,
            grade: resolved_grade,
            lut_path: lut_path.as_deref(),
            proj22: scene.proj22,
            proj32: scene.proj32,
            lights: &scene.lights,
            render_mode: self.settings.render_mode,
            pt_max_bounces: self.settings.pt_max_bounces,
            pt_ray_max_distance: self.settings.pt_ray_max_distance,
            pt_max_iterations: self.settings.pt_max_iterations,
            exposure: scene.exposure,
            pt_lights: &scene.pt_lights,
            // Editor only redraws on "changes", so each render treated as fresh accumulation start.
            pt_accum_dirty: true,
            has_camera: scene.has_camera,
            camera_orthographic: scene.camera_orthographic,
            clear_color: scene.clear_color,
            shadows_enabled: scene.shadows_enabled,
            ui_overlay: if self.kind == ViewKind::Game || self.view_2d {
                Some(&self.packet.ui_overlay)
            } else {
                None
            },
            show_grid: editor_grid_visible(self.kind, &self.settings, self.view_2d),
            dispatch_plan: None,
        };

        self.renderer.set_internal_resolution(
            self.settings.internal_resolution,
            self.settings.internal_scale_mode,
        );
        self.renderer
            .set_voxel_scene(self.packet.voxel_scene.clone());
        let Some(ctx) = self.renderer.begin_frame()? else {
            return Ok(None);
        };
        let result = self
            .renderer
            .prepare(&ctx, &input)
            .and_then(|_| self.renderer.execute(&ctx, &input))
            .inspect_err(|_| {
                // Keep headless/window fence signaled cleanly when a render pass aborts.
                let _ = self.renderer.present(&ctx);
                let _ = self.renderer.end_frame(&ctx, false);
            })
            .and_then(|_| self.renderer.present(&ctx))
            .and_then(|_| self.renderer.end_frame(&ctx, false));
        if let Err(error) = result {
            return Err(error);
        }
        self.prev_view_proj = Some(current_view_proj);

        // Screenshot must read offscreen image current layout (frame end is `COLOR_ATTACHMENT_OPTIMAL`),
        // so must do before transitioning to sample layout.
        if let Some(path) = self.capture_path.take() {
            match self.capture_offscreen(&path) {
                Ok(path) => log::info!("viewport capture: {}", path.display()),
                Err(error) => log::warn!("viewport capture failed: {error:#}"),
            }
        }

        // COLOR_ATTACHMENT_OPTIMAL → SHADER_READ_ONLY_OPTIMAL: egui sampling descriptor requires
        // latter, and step happens on same queue, no extra semaphore.
        self.renderer.offscreen_present_for_sampling()?;

        Ok(self
            .renderer
            .offscreen_image_view()
            .map(|view| (view, extent)))
    }

    /// Request **next frame** to read offscreen image back and write PNG (editor screenshot).
    pub fn request_capture(&mut self, path: PathBuf) {
        self.capture_path = Some(path);
    }

    /// Offscreen image → host-visible buffer → RGBA8 → PNG.
    fn capture_offscreen(&mut self, path: &Path) -> anyhow::Result<PathBuf> {
        let extent = self
            .renderer
            .offscreen_extent()
            .context("no offscreen target to capture")?;
        self.renderer.copy_offscreen_to_readback()?;
        let pixels = self.renderer.readback_pixels()?;
        prism_render::screenshot::save_rgba_png(path, extent.width, extent.height, &pixels, None)
    }
}

fn fixed_tick_requested(paused: bool, step_pending: bool) -> bool {
    !paused || step_pending
}

/// Fixed step size for single-step simulation.
fn xstep_dt() -> f32 {
    1.0 / 60.0
}

/// Document subscript → ECS entity id; skip out-of-range / disabled. Pure function for easy tests (#487).
fn selection_entity_ids(entities: &[Entity], selected: &[usize], enabled: bool) -> Vec<u32> {
    if !enabled {
        return Vec::new();
    }
    selected
        .iter()
        .filter_map(|index| entities.get(*index))
        .map(|entity| entity.id())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_kind_metadata_is_stable() {
        assert_eq!(ViewKind::Scene.icon(), "🎬");
        assert_eq!(ViewKind::Game.icon(), "🎮");
        assert_ne!(ViewKind::Scene, ViewKind::Game);
    }

    /// #454: mesh is Scene view helper object, Game view never draws it.
    #[test]
    fn the_editor_grid_never_reaches_the_game_view() {
        let mut settings = prism_engine::render_settings::RenderSettings::default();
        settings.show_grid = true;
        assert!(editor_grid_visible(ViewKind::Scene, &settings, false));
        assert!(!editor_grid_visible(ViewKind::Game, &settings, false));

        settings.show_grid = false;
        assert!(!editor_grid_visible(ViewKind::Scene, &settings, false));
        assert!(!editor_grid_visible(ViewKind::Game, &settings, false));
    }

    /// #641: XZ mesh in 2D view is edge-on line, must disable.
    #[test]
    fn the_grid_is_hidden_in_the_2d_view() {
        let mut settings = prism_engine::render_settings::RenderSettings::default();
        settings.show_grid = true;
        assert!(!editor_grid_visible(ViewKind::Scene, &settings, true));
    }

    #[test]
    fn simulation_step_uses_fixed_sixtieth_second() {
        assert!((xstep_dt() - 1.0 / 60.0).abs() < f32::EPSILON);
    }

    #[test]
    fn paused_step_requests_one_fixed_tick() {
        assert!(fixed_tick_requested(true, true));
        assert!(fixed_tick_requested(false, false));
        assert!(!fixed_tick_requested(true, false));
    }

    /// #487: only map truly existing selected subscripts in document to entity ids; clear when disabled.
    #[test]
    fn selection_outline_maps_document_indices_to_entity_ids() {
        let entities = [
            Entity::from_raw(10, 0),
            Entity::from_raw(42, 0),
            Entity::from_raw(7, 0),
        ];
        assert_eq!(selection_entity_ids(&entities, &[0, 2], true), vec![10, 7]);
        assert_eq!(selection_entity_ids(&entities, &[0, 99], true), vec![10]);
        assert!(selection_entity_ids(&entities, &[0, 1], false).is_empty());
    }

    #[test]
    fn editor_camera_rotation_matches_orbit_camera_eye_convention() {
        let yaw = 0.0;
        let pitch = 0.0;
        let distance = 12.0;
        let rotation = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(-pitch);
        let eye_offset = rotation * Vec3::new(0.0, 0.0, distance);
        let (sin_yaw, cos_yaw) = yaw.sin_cos();
        let (sin_pitch, cos_pitch) = pitch.sin_cos();
        let expected = Vec3::new(
            distance * cos_pitch * sin_yaw,
            distance * sin_pitch,
            distance * cos_pitch * cos_yaw,
        );
        assert!((eye_offset - expected).length() < 1e-5);
    }
}

/// Modify time of `assets/scenes.pak`; `None` when no pak or metadata unreadable.
fn resource_package_mtime() -> Option<std::time::SystemTime> {
    let path = prism_engine::asset_paths::asset_path("scenes.pak")?;
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}
