//! [`Engine`] — core simulation engine.
//!
//! Owns ECS world and runs game logic phases (`fixed_update`, `update`,
//! `late_update`). **Does not** contain render, asset parsing or dirty-data routing — window,
//! render thread, audio thread and other platform resources belong to `prism-app`; this engine only does main-thread
//! pure logical simulation.
//!
//! ## Lifecycle (phases actually participating in init)
//!
//! ```text
//! empty / new
//!     ├─ init_scene(rm)          ─── load scene  → world
//!     └─ runtime_initialize()    ─── fallback camera + default ECS resources / schedules
//!
//! [Each frame ]
//! fixed_update → update → late_update
//! ```
//!
//! **Platform lifecycle (hang / resume, thread shutdown) not here.** They are driven
//! by `prism-app`'s [`Subsystem`](https://docs.rs/prism-app/latest/prism_app/trait.Subsystem.html)
//! layer by layer: whoever holds state (thread / resource) receives corresponding hook. Engine itself
//! holds no platform resource needing cleanup, so has no empty thread / disable hooks.

use prism_ecs::World;

impl crate::scene::EnvironmentProvider for prism_asset::runtime::ResourceManager {
    fn load_environment(&mut self, asset_path: &str) -> Option<Vec<u8>> {
        if crate::asset_paths::loose_assets_enabled() {
            if let Some(path) = crate::asset_paths::asset_path(asset_path) {
                if path.is_file() {
                    if let Ok(bytes) = std::fs::read(&path) {
                        log::info!(
                            "environment: loaded loose asset {} (PRISMA_LOOSE_ASSETS)",
                            path.display()
                        );
                        return Some(bytes);
                    }
                }
            }
        }
        let id = self.id_by_path(asset_path)?;
        self.load_with_deps_raw(id).ok()
    }
}

use crate::ecs::schedule::Schedule;
use crate::input::InputManager;

/// Scene load mode (equivalent to Unity `OpenSceneMode`).
///
/// Previous entry only had "keep spawning into current world", so second load made all entities
/// doubled (see #482). Now mode is explicit parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoadSceneMode {
    /// Unload already-loaded scene (except `Persistent` subtree) then load — default.
    #[default]
    Single,
    /// Keep already-loaded scene, overlay new scene.
    Additive,
}

// ===========================================================================
// Engine
// ===========================================================================

/// Simulation engine — owns ECS world and exposes game logic phases.
///
/// All render-related concerns (asset upload, draw list building, GPU submission)
/// handled by application's `RenderContext`.
pub struct Engine {
    world: World,
    /// Scene system — owns "current scene" state and handles scene loading.
    scene: crate::scene::SceneManager,
    /// Ordered ECS system schedule — runs on every [`update`](Self::update).
    schedule: Schedule,
    /// Deterministic simulation schedule — runs only from [`fixed_update`](Self::fixed_update).
    fixed_schedule: Schedule,
    frame_input: prism_project_api::InputSnapshot,
    pending_fixed_key_edges: [u64; 8],
    frame_input_prepared: bool,
    interpolation_alpha: f32,
    /// Main-thread timer service (ticked each frame).
    timer: crate::util::timer::TimerService,
    /// Seconds count, used by "no scene" placeholder scrolling animation. Only accumulates when placeholder displayed.
    no_scene_time: f32,
}

impl Engine {
    // ======================================================================
    // Construction
    // ======================================================================

    /// Create `Engine` and run all boundary init phases (convenience constructor).
    ///
    /// Loads resources from the given [`ResourceManager`] then loads the scene.
    pub fn new(rm: &mut prism_asset::runtime::ResourceManager) -> Self {
        let mut engine = Self::empty();
        engine.init_scene(rm);
        engine.runtime_initialize();
        engine
    }

    /// Create empty engine — runs no init phase.
    pub fn empty() -> Self {
        let _span = crate::util::timing::span("engine::empty");
        let mut world = World::new();
        // One-shot audio queue (#987): project code queues, host takes each frame.
        world.insert_resource(crate::sound::SoundCommands::default());
        Self {
            world,
            scene: crate::scene::SceneManager::new(),
            schedule: Schedule::new(),
            fixed_schedule: Schedule::new(),
            frame_input: prism_project_api::InputSnapshot::default(),
            pending_fixed_key_edges: [0; 8],
            frame_input_prepared: false,
            interpolation_alpha: 0.0,
            timer: crate::util::timer::TimerService::new(),
            no_scene_time: 0.0,
        }
    }

    // ======================================================================
    // Init
    // ======================================================================

    /// Scene load (default loads first loadable scene in manifest).
    ///
    /// Scene load handled by **scene system** (delegates to [`crate::scene::SceneManager::load_first_from_manifest`]),
    /// engine no longer scans manifest itself. When selecting scene by name use
    /// [`Self::init_scene_named`].
    ///
    /// Save files implemented by **user project** — engine doesn't auto-read save at init, only provides
    /// [`Self::save_scene_state`] / [`Self::load_scene_state`] primitives for calls.
    /// Asset parsing (`.pak` → `ResourceManager`) done and passed in by caller, engine doesn't hold it.
    pub fn init_scene(&mut self, rm: &mut prism_asset::runtime::ResourceManager) {
        self.init_scene_named(rm, None);
    }

    /// Load scene by name; `scene_name = None` equivalent to [`Self::init_scene`].
    ///
    /// Name corresponds to `assets/scenes.json` `scenes[].name` (see
    /// [`prism_engine::launch_config::LaunchConfig`]). When name not found
    /// load no scene and log warning.
    pub fn init_scene_named(
        &mut self,
        rm: &mut prism_asset::runtime::ResourceManager,
        scene_name: Option<&str>,
    ) {
        // Runtime prefers reading manifest from asset package, avoid depending on cwd.
        // Old packages packed manifest as `scenes.toml` (TOML format before RSCN), still parseable.
        for name in ["scenes.json", "scenes.toml"] {
            let Some(id) = rm.id_by_path(name) else {
                continue;
            };
            if let Ok(bytes) = rm.load_with_deps_raw(id) {
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    self.init_scene_from_manifest_text(rm, text, scene_name);
                    return;
                }
            }
        }

        // Dev compatibility: when package lacks manifest scene system reads file.
        let mut registry = crate::scene::ComponentRegistry::new();
        crate::scene::register_builtin_components(&mut registry);
        match scene_name {
            Some(name) => {
                self.scene
                    .load_named_from_manifest(rm, &mut self.world, &registry, name);
            }
            None => {
                self.scene
                    .load_first_from_manifest(rm, &mut self.world, &registry);
            }
        }
    }

    /// Init scene from caller-provided in-memory manifest.
    ///
    /// This is preferred entry for Android, packaged runtime and test environments; engine will not
    /// access cwd for this path.
    pub fn init_scene_from_manifest_text(
        &mut self,
        rm: &mut prism_asset::runtime::ResourceManager,
        manifest_text: &str,
        scene_name: Option<&str>,
    ) -> Option<String> {
        let mut registry = crate::scene::ComponentRegistry::new();
        crate::scene::register_builtin_components(&mut registry);
        self.scene.load_from_manifest_text(
            rm,
            &mut self.world,
            &registry,
            manifest_text,
            scene_name,
        )
    }

    /// Load scene from loose `.scene.json` (editor host / dev-time author format).
    ///
    /// Different from manifest / cook path: no `.pak`, directly deserialize author JSON
    /// into ECS components using reflection registry. Returns number of entities spawned.
    pub fn load_scene_json_file(
        &mut self,
        path: &std::path::Path,
    ) -> Result<Vec<crate::ecs::Entity>, String> {
        self.load_scene_json_file_with_mode(path, LoadSceneMode::Single)
    }

    /// [`Self::load_scene_json_file`], but with explicit [`LoadSceneMode`].
    ///
    /// `Single` first unloads previous scene (ask: previously it didn't clear world, second call
    /// doubled all entities with no warning). Unload happens **after new scene spawned successfully** —
    /// on parse failure old scene stays unchanged.
    pub fn load_scene_json_file_with_mode(
        &mut self,
        path: &std::path::Path,
        mode: LoadSceneMode,
    ) -> Result<Vec<crate::ecs::Entity>, String> {
        let mut registry = crate::scene::ComponentRegistry::new();
        crate::scene::register_builtin_components(&mut registry);
        let mut loader = crate::scene::loader::SceneLoader::new();
        let instance = loader.load_and_spawn(
            &mut self.world,
            crate::scene::loader::SceneSource::JsonFile(path.to_path_buf()),
            &registry,
        )?;
        let unloaded = self.scene.adopt_author_scene(
            &mut self.world,
            &instance,
            mode == LoadSceneMode::Additive,
        );
        log::info!(
            "scene: loaded {} entities from {} ({mode:?}, unloaded {unloaded})",
            instance.all_entities.len(),
            path.display()
        );
        // Return entity list in **document order**: `spawn_from_json_text` spawns in document array order,
        // host (editor) uses this subscript to match `.scene.json` `entities[i]`.
        Ok(instance.all_entities)
    }

    /// Unload current scene (except `Persistent` subtree and engine-owned entities), return deleted entity count.
    ///
    /// For "return to main menu" / "restart a round" cases needing only cleanup; combined with
    /// [`Self::load_scene_json_file_with_mode`] can change scene without restarting process.
    pub fn unload_scene(&mut self) -> usize {
        self.scene.unload_scene(&mut self.world)
    }

    /// Scene generation: incremented on each switch / unload. Host passes to project module,
    /// letting it discard invalidated entity handles (`prism-project-api::scene_generation`).
    pub fn scene_generation(&self) -> u64 {
        self.scene.generation()
    }

    /// Save primitive: write current ECS world state to `scene_state.json`.
    ///
    /// Save implemented by **user project** — engine no longer auto-reads save at init; user code calls
    /// this method at appropriate times (checkpoint, pause menu, before exit).
    pub fn save_scene_state(&self) {
        crate::scene_state::save_scene_state(&self.world);
    }

    /// Specify save directory (host derives app data directory from `[app] company/name`).
    ///
    /// When unset falls back to [`crate::config::app_data_dir`], meaning all projects share one
    /// directory — root cause of "projects overwriting each other's saves" in #484.
    pub fn set_save_directory(&mut self, dir: impl Into<std::path::PathBuf>) {
        self.world
            .insert_resource(crate::scene_state::SaveDirectory(dir.into()));
    }

    /// Current save path (respects [`Self::set_save_directory`]).
    pub fn scene_state_path(&self) -> std::path::PathBuf {
        crate::scene_state::scene_state_path(&self.world)
    }

    /// Write to specified slot: `scene_state-<slot>.json`.
    pub fn save_scene_state_slot(&self, slot: u32) {
        crate::scene_state::save_scene_state_slot(&self.world, slot);
    }

    /// Resume from specified slot.
    pub fn load_scene_state_slot(&mut self, slot: u32) -> bool {
        crate::scene_state::load_scene_state_slot(&mut self.world, slot)
    }

    /// Write to arbitrary path (for "save as" / tests).
    pub fn save_scene_state_to(&self, path: &std::path::Path) {
        crate::scene_state::save_scene_state_to(&self.world, path);
    }

    /// Resume from arbitrary path.
    pub fn load_scene_state_from(&mut self, path: &std::path::Path) -> bool {
        crate::scene_state::load_scene_state_from(&mut self.world, path)
    }

    /// Load primitive: restore world state from `scene_state.json` into ECS world.
    ///
    /// Returns whether save file was read. **User project** decides when to call (e.g. at startup,
    /// "continue game" entry).
    pub fn load_scene_state(&mut self) -> bool {
        crate::scene_state::load_scene_state(&mut self.world)
    }

    /// Runtime init: the "everything is wired" finishing hooks.
    ///
    /// Registers the default ECS systems and resources. It deliberately does
    /// **not** spawn a camera: a world without a `Camera` renders through
    /// [`fallback_camera_output`](crate::scene::systems::camera::fallback_camera_output),
    /// and hosts that want a free-fly camera ask for one with
    /// [`crate::util::camera::spawn_default_camera`].
    pub fn runtime_initialize(&mut self) {
        let _span = crate::util::timing::span("engine::runtime_initialize");
        // Default systems (UI infrastructure) are **merged** into the existing
        // schedule rather than replacing it, so systems a user registered before
        // `runtime_initialize` survive.
        self.schedule.merge_if_absent(default_schedule());
    }

    // ======================================================================
    // Timer API
    // ======================================================================

    /// Returns TimerClient reference (Cloneable, for panels etc).
    pub fn timer_client(&self) -> &crate::util::timer::TimerClient {
        &self.timer.client
    }

    /// Register a timer. Returns u32 slot index (timer id).
    pub fn create_timer(
        &self,
        params: crate::util::timer::TimerParams,
    ) -> crate::util::timer::TimerId {
        self.timer.client.create_timer(params)
    }

    /// Pause timer.
    pub fn pause_timer(&self, handle: crate::util::timer::TimerId) {
        self.timer.client.pause(handle);
    }

    /// Resume timer.
    pub fn resume_timer(&self, handle: crate::util::timer::TimerId) {
        self.timer.client.resume(handle);
    }

    // ======================================================================
    // Frame tick phases
    // ======================================================================

    /// Capture render-frame input before running zero or more fixed steps.
    /// Press/release edges are retained until the next fixed step.
    pub fn begin_frame(&mut self, input_manager: &InputManager, screen: [f32; 2]) {
        self.frame_input = crate::input::input_snapshot(input_manager, screen);
        for (pending, current) in self
            .pending_fixed_key_edges
            .iter_mut()
            .zip(self.frame_input.key_edges)
        {
            *pending |= current;
        }
        self.frame_input_prepared = true;
    }

    /// Fixed-timestep update: deterministic gameplay and physics systems.
    pub fn fixed_update(&mut self, fixed_dt: f32, input_manager: &InputManager) {
        if !self.frame_input_prepared {
            let screen = self
                .world
                .get_resource::<crate::ui::ScreenSize>()
                .map(|size| [size.width, size.height])
                .unwrap_or([0.0, 0.0]);
            self.begin_frame(input_manager, screen);
        }

        let mut input = self.frame_input;
        input.key_edges = self.pending_fixed_key_edges;
        self.pending_fixed_key_edges = [0; 8];
        self.world.insert_resource(input);
        crate::scene::systems::interpolation2d::begin_fixed_step(&mut self.world);
        // 2D physics step simulation (#1160): default gravity 9.81 m/s² downward (-Y)
        crate::physics2d::step_2d(&mut self.world, fixed_dt, [0.0, -9.81]);
        self.fixed_schedule.run(&mut self.world, fixed_dt.max(0.0));
        crate::scene::systems::interpolation2d::end_fixed_step(&mut self.world);
        crate::scene::systems::hierarchy::hierarchy_system(&mut self.world);
        crate::scene::systems::hierarchy::active_hierarchy_system(&mut self.world);
    }

    /// Fractional fixed-step remainder used by render-frame interpolation.
    pub fn set_interpolation_alpha(&mut self, alpha: f32) {
        self.interpolation_alpha = if alpha.is_finite() {
            alpha.clamp(0.0, 1.0)
        } else {
            0.0
        };
    }

    /// Variable‑timestep per‑frame update: game logic, camera, input.
    ///
    /// Update · UE `Tick()`.
    pub fn update(&mut self, dt: f32, input_manager: &InputManager) {
        if !self.frame_input_prepared {
            let screen = self
                .world
                .get_resource::<crate::ui::ScreenSize>()
                .map(|size| [size.width, size.height])
                .unwrap_or([0.0, 0.0]);
            self.begin_frame(input_manager, screen);
        }
        let suppressed = self
            .world
            .get_resource::<crate::scene::systems::camera::CameraInputGate>()
            .is_some_and(|gate| gate.suppressed);
        let look_active = input_manager.pointer_locked && !suppressed;
        crate::scene::systems::camera::camera_controller_system(
            &mut self.world,
            input_manager,
            dt,
            look_active,
        );
        crate::scene::systems::camera2d::camera_2d_system(&mut self.world, dt);

        // ── fill UI input state ──────────────────────────────────────
        let pos = input_manager.mouse_position();
        // Window pixels → UI layout space (#988): UI drawn inside PostPass target rect,
        // without conversion clicks would mismatch visible positions.
        let mapping = self
            .world
            .get_resource::<crate::ui::UiScreenMapping>()
            .copied()
            .unwrap_or_default();
        let cursor = mapping.window_to_layout([pos[0] as f32, pos[1] as f32]);
        // Wheel and modifiers fold into layout space here too: host UI never sees
        // platform events, and dropping either deletes an entire interaction path.
        self.world.insert_resource(crate::ui::UiInputState {
            cursor_pos: cursor,
            left_clicked: input_manager.mouse_just_pressed(crate::input::MouseButton::Left),
            left_held: input_manager.mouse_held(crate::input::MouseButton::Left),
            left_released: input_manager.mouse_just_released(crate::input::MouseButton::Left),
            right_clicked: input_manager.mouse_just_pressed(crate::input::MouseButton::Right),
            scroll_delta: [0.0, input_manager.scroll_delta() as f32],
            shift_held: input_manager.key_held(crate::input::KeyCode::ShiftLeft)
                || input_manager.key_held(crate::input::KeyCode::ShiftRight),
            ctrl_held: input_manager.key_held(crate::input::KeyCode::ControlLeft)
                || input_manager.key_held(crate::input::KeyCode::ControlRight),
            pressed_keys: input_manager.pressed_keys().to_vec(),
        });

        // ── same input also given to gameplay code (#589) ──────────────────
        // Static Player's `GameWorld::input()` reads this World resource.
        // Previously only editor plugin backend could get input, `GameWorld::input()` in release
        // was always all-zero. Size from `ScreenSize` (host writes before each frame update).
        let screen = self
            .world
            .get_resource::<crate::ui::ScreenSize>()
            .map(|size| [size.width, size.height])
            .unwrap_or([0.0, 0.0]);
        self.frame_input.screen = screen;
        self.frame_input.interpolation_alpha = self.interpolation_alpha;
        self.world.insert_resource(self.frame_input);

        // Run the ECS system schedule (user‑registered + built‑in systems).
        self.schedule.run(&mut self.world, dt);

        // Quads submitted by user systems via `GameWorld::draw_quad` this tick are
        // ordered after `ui::render`, merge here once, else static Player is one frame late (#1048).
        crate::ui::merge_external_quads(&mut self.world);

        // "No scene" placeholder image: only appears when startup manifest has no loadable scene (usually
        // `.pak` not packed). Goes through existing UI channel, no new pass, clear color untouched.
        if self.scene.is_missing_scene() {
            self.no_scene_time += dt.max(0.0);
            crate::ui::push_no_scene_screen_into(&mut self.world, self.no_scene_time);
        }

        // ── world matrix propagation (end of simulation step) ─────────────────────────────
        // Entities spawned by user systems this tick / modified `LocalTransform` must
        // become `LocalToWorld` before update returns, otherwise consumers below see previous-frame
        // or even missing world matrices:
        // - later in same tick (next frame) game logic / physics / camera;
        // - render extract (`extract_frame_packet` now read-only, no longer computes);
        // - scene state save (`save_scene_state` reads `LocalToWorld`).
        // Placed after schedule rather than inside a system because user systems can register
        // after `runtime_initialize` (App::add_system), can't rely on registration order.
        crate::scene::systems::hierarchy::hierarchy_system(&mut self.world);
        // `Active` propagates along parent-child (`activeInHierarchy`), render / lighting read derived value.
        crate::scene::systems::hierarchy::active_hierarchy_system(&mut self.world);
        crate::scene::systems::interpolation2d::apply_render_interpolation(
            &mut self.world,
            self.interpolation_alpha,
        );
        // 2D virtual cameras follow the frame-interpolated target before the
        // render packet is extracted. Recompute the camera world transform
        // after writing its local position.
        crate::scene::systems::camera2d::virtual_camera2d_system(&mut self.world, dt);
        crate::scene::systems::hierarchy::hierarchy_system(&mut self.world);
        crate::scene::systems::interpolation2d::apply_render_interpolation(
            &mut self.world,
            self.interpolation_alpha,
        );
        // Distance LOD (#491): select level by this frame's world matrix, write runtime `LodHidden`.
        // Must be after `hierarchy_system` so extract sees current-frame distance.
        crate::scene::systems::lod::lod_system(&mut self.world);

        // Tick the main‑thread timer (dispatch expired callbacks).
        self.timer.tick();
        self.frame_input_prepared = false;
    }

    /// Late update: audio sync, IK, camera‑relative effects.
    ///
    /// `LateUpdate()`.
    pub fn late_update(&mut self) {}

    // ======================================================================
    // Shutdown
    // ======================================================================

    /// Pre‑shutdown: save state, flush pending work.
    ///
    /// Platform resources (window, render / audio threads, audio engine) held by `prism-app`'s `App`,
    /// shutdown order orchestrated by `App::about_to_wait`; engine only does logic-side cleanup (currently no state).
    pub fn pre_shutdown(&mut self) {}

    /// Final cleanup after shutdown.
    pub fn post_shutdown(&mut self) {}

    // ======================================================================
    // Suspend / resume (platform lifecycle)
    // ======================================================================

    /// Called when the platform surface is suspended (e.g. Android onPause).
    pub fn on_suspend(&mut self) {}

    /// Called when the platform surface is resumed (e.g. Android onResume).
    pub fn on_resume(&mut self) {}

    // ======================================================================
    // Accessors
    // ======================================================================

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// Current scene name (held by scene system); `None` when no manifest scene loaded.
    /// No resolvable startup scene was loaded (usually `assets/*.pak` is missing).
    ///
    /// The render side uses it to decide whether to draw the "no scene"
    /// placeholder. Host mode (the editor) never attempts a startup load, so this
    /// stays `false` there.
    pub fn is_missing_scene(&self) -> bool {
        self.scene.is_missing_scene()
    }

    pub fn current_scene_name(&self) -> Option<&str> {
        self.scene.current_scene_name()
    }

    /// Resolve environment texture bytes needed for "current scene" declarative lighting.
    ///
    /// Scene system responsibility (determines scene by `current_scene_name`), engine only forwards.
    /// Uses external resource service to read current scene env texture, no filesystem access.
    pub fn current_scene_env_bytes_with_provider<P: crate::scene::EnvironmentProvider>(
        &self,
        provider: &mut P,
    ) -> Option<Vec<u8>> {
        self.scene.current_scene_env_bytes_with_provider(
            crate::scene::SceneReadView::new(&self.world),
            provider,
        )
    }

    /// Inject current scene environment texture for renderer startup or scene switch.
    pub fn set_scene_environment_bytes(&mut self, bytes: Option<Vec<u8>>) {
        self.scene.set_environment_bytes(bytes);
    }

    /// Load current scene environment texture from runtime asset package by path.
    ///
    /// Environment textures usually untyped HDR binary assets, so here only responsible for resolving
    /// path via `ResourceManager` and reading raw bytes, decode remains renderer layer's job.
    pub fn load_scene_environment_from_path(
        &mut self,
        rm: &mut prism_asset::runtime::ResourceManager,
        asset_path: &str,
    ) -> anyhow::Result<()> {
        let bytes = crate::scene::EnvironmentProvider::load_environment(rm, asset_path)
            .ok_or_else(|| anyhow::anyhow!("environment asset not found: {asset_path}"))?;
        self.set_scene_environment_bytes(Some(bytes));
        Ok(())
    }

    /// Mutable access to the ECS system schedule.
    pub fn schedule_mut(&mut self) -> &mut Schedule {
        &mut self.schedule
    }

    /// Mutable access to the fixed simulation schedule.
    pub fn fixed_schedule_mut(&mut self) -> &mut Schedule {
        &mut self.fixed_schedule
    }
}

/// Build the default system schedule.
///
/// Registered systems run in order on every [`Engine::update`] tick.
/// Consumers can extend or replace the schedule via
/// [`Engine::schedule_mut`].
///
/// Contains engine infrastructure only (UI layout / input / render). Demo content
/// is never registered here; entry points add whatever they need.
fn default_schedule() -> Schedule {
    use crate::ecs::schedule::Schedule;

    let mut s = Schedule::new();

    // ── sprite animation ────────────────────────────────────────────
    // Advance `SpriteAnimation` → `SpriteRenderer.frame_index`; before render extract.
    s.add_system("sprite::animation", |world, dt| {
        crate::scene::systems::animation::sprite_animation_system(world, dt)
    });

    // ── keyframe animation ──────────────────────────────────────────
    // Sample `KeyframeAnimation` → `LocalTransform`. `Engine::update` runs `hierarchy_system`
    // after schedule, so same frame's world matrix is post-animation pose.
    s.add_system("clip::animation", |world, dt| {
        crate::scene::systems::animation::keyframe_animation_system(world, dt)
    });

    // ── skeletal animation ────────────────────────────────────────────
    // Sample `Animator` → bone matrices. P0-2 only computes CPU-side palette (GPU skinning in P0-3).
    s.add_system("animator::pose", |world, dt| {
        crate::scene::systems::animator::animator_system(world, dt);
    });

    // ── terrain ────────────────────────────────────────────────
    // `Terrain` is data only: here generate mesh from heightmap and register into `ProceduralMeshes`,
    // asset parser can pick it up in next frame's upload phase (#517 Phase 1).
    s.add_system("terrain::mesh", |world, _dt| {
        crate::scene::systems::terrain::terrain_system(world)
    });

    // ── particle simulation ──────────────────────────────────────────
    // CPU particle integration + emission (#501 Phase 1). Also before `hierarchy_system`,
    // extract sees already-advanced particles this frame.
    s.add_system("fx::particles", |world, dt| {
        crate::scene::systems::particles::particle_system(world, dt);
    });

    // ── 2D particle simulation ─────────────────────────────────
    // CPU particles in world XY plane (#594). Parallel to 3D particles for same reason: consistent with ECS /
    // scene format, extract sees already-advanced particles this frame.
    s.add_system("fx::particles2d", |world, dt| {
        crate::scene::systems::particles2d::particle_emitter_2d_system(world, dt);
    });

    // ── screen-space (UI layer) particles (#1057) ────────────────────────
    // Simulated in pixel space, output merged into `ExternalUiQuads`; must be before this frame's `ui::render`
    // so quads enter same frame's `UiDrawList`.
    s.add_system("fx::ui_particles", |world, dt| {
        crate::scene::systems::ui_particles::ui_particle_emitter_system(world, dt);
    });

    // ── day-night (#983) ─────────────────────────────────────────
    // Advance `TimeOfDay` and refresh ambient light. Placed before particles and `hierarchy_system`:
    // this frame's `sprite_light_entries` must read advanced ambient light, else image is one frame late.
    s.add_system("time::of_day", |world, dt| {
        crate::scene::systems::time_of_day::time_of_day_system(world, dt);
    });

    // ── UI Layout ─────────────────────────────────────────────
    // Recompute screen-space rects of all UI elements each frame.
    s.add_system("ui::layout", |world, _dt| {
        crate::ui::ui_layout_system(world)
    });

    // ── UI Input ──────────────────────────────────────────────
    // Hit test, update `Interaction` component (needs after layout).
    s.add_system("ui::input", |world, _dt| crate::ui::ui_input_system(world));

    // ── UI Render ─────────────────────────────────────────────
    // Collect UI draw commands into UiDrawList resource.
    s.add_system("ui::render", |world, _dt| {
        crate::ui::ui_render_system(world)
    });

    s
}

#[cfg(test)]
#[path = "transform_propagation_tests.rs"]
mod transform_propagation_tests;

#[cfg(test)]
#[path = "engine_scene_tests.rs"]
mod scene_tests;

#[cfg(test)]
mod fixed_update_tests {
    use super::Engine;
    use crate::input::{ElementState, InputManager, KeyCode};
    use prism_project_api::{keys, InputSnapshot};

    #[derive(Default)]
    struct Probe {
        runs: u32,
        pressed_edges: u32,
    }

    #[test]
    fn fixed_schedule_runs_each_step_and_consumes_edges_once() {
        let mut engine = Engine::empty();
        engine.world_mut().insert_resource(Probe::default());
        engine
            .fixed_schedule_mut()
            .add_system("probe", |world, _dt| {
                let snapshot = world
                    .get_resource::<InputSnapshot>()
                    .copied()
                    .expect("fixed_update publishes input first");
                let probe = world
                    .get_resource_mut::<Probe>()
                    .expect("probe resource exists");
                probe.runs += 1;
                if snapshot.key_pressed(keys::SPACE) {
                    probe.pressed_edges += 1;
                }
            });

        let mut input = InputManager::new();
        input.handle_keyboard(KeyCode::Space, ElementState::Pressed);
        engine.begin_frame(&input, [320.0, 180.0]);
        engine.fixed_update(1.0 / 60.0, &input);
        engine.fixed_update(1.0 / 60.0, &input);

        let probe = engine
            .world()
            .get_resource::<Probe>()
            .expect("probe resource exists");
        assert_eq!(probe.runs, 2);
        assert_eq!(probe.pressed_edges, 1);
    }
}
