//! [`App`] — platform application layer, owns event loop.
//!
//! Window backend decided by feature (see crate docs): `winit-backend` uses
//! [`ApplicationHandler`], `win32-native` uses [`App::run_win32`]'s
//! Win32 message loop. Both share the same engine/render/shutdown logic
//! ([`App::tick_frame`] / [`App::shutdown`]), difference only in window and event source.
//!
//! # Architecture (render thread = background thread)
//!
//! ```text
//! Main thread (window events)           Render thread
//!   ──────────────────────────         ──────────────
//!   about_to_wait:                       loop:
//!     engine.fixed_update × N             take_packet()
//!     engine.update                       begin_frame()
//!     engine.late_update                  execute(packet)
//!     audio.update                        present()
//!     extract_frame_packet ──packet──►
//!     frame_hook.on_tick ──overlay_msg──►
//! ```
//!
//! Render thread runs independently of window events. VSync only blocks render thread --- main thread continues executing.
//!
//! **Initialization** (window quickly created on main thread, renderer asynchronously constructed on render thread):
//! ```text
//!   App::new:
//!     Engine::empty → pre_init → init_core → init_config → init_resources
//!     → init_scene → runtime_initialize
//!   [resumed]:
//!     PlatformContext::create_window (main thread, ~few ms) → compute extension name / scene lighting
//!     → create render thread (returns on window start, events not blocked)
//!   [render thread]:
//!     GraphRenderer::new (~few hundred ms, not blocking main thread) → warmup → set_environment
//!     → enter frame loop; resource resolution completed asynchronously via asset_requests/asset_results channel
//! ```

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[cfg(feature = "audio")]
use prism_audio::AudioEngine;
use prism_engine::asset_resolver::GpuAssetResolver;
use prism_engine::config::AppConfig;
use prism_engine::input::InputManager;
use prism_engine::render_settings::RenderSettings;
use prism_engine::render_system::extract_frame_packet_into_with_shadow_cascades_and_render_height;
use prism_engine::Engine;
use prism_platform::{
    required_vulkan_extensions, Key, Modifiers, PlatformContext, PlatformEvent, PlatformWindow,
    WindowConfig,
};

/// Mapping from "window pixels → UI layout space" (#988).
///
/// When no `internal_resolution` it is identity mapping (old behavior). When effective UI and PostPass target rectangle
/// target rectangle coincide, scale ratio = rect width / window width, offset = rect centered offset.
fn ui_layout_mapping(
    surface_w: u32,
    surface_h: u32,
    internal: [u32; 2],
    mode: prism_render::InternalScaleMode,
) -> prism_engine::ui::UiScreenMapping {
    if internal[0] == 0 || internal[1] == 0 || surface_w == 0 || surface_h == 0 {
        return prism_engine::ui::UiScreenMapping::default();
    }
    let (offset, extent) =
        prism_render::render_graph::post_destination_px([surface_w, surface_h], internal, mode);
    let scale = extent[0] as f32 / surface_w as f32;
    if !(scale.is_finite() && scale > 0.0) {
        return prism_engine::ui::UiScreenMapping::default();
    }
    prism_engine::ui::UiScreenMapping {
        scale,
        offset: [offset[0] as f32, offset[1] as f32],
    }
}

#[cfg(feature = "winit-backend")]
use winit::application::ApplicationHandler;
#[cfg(feature = "winit-backend")]
use winit::event::{DeviceEvent, WindowEvent};
#[cfg(feature = "winit-backend")]
use winit::event_loop::{ActiveEventLoop, ControlFlow};
#[cfg(feature = "winit-backend")]
use winit::window::WindowId;

#[cfg(feature = "engine-debug")]
use crate::debug_hud::DebugHud;
use crate::hook::FrameHook;
use crate::render_runner::{render_thread_main, OverlayFactory, RenderTarget};
use crate::render_shared::{FrameImage, RenderShared, RenderStats};

// ===========================================================================
// Subsystem enum
// ===========================================================================

/// Engine subsystem, explicitly declared by user project via [`App::with_subsystem`].
mod hot_reload;
mod input;
#[cfg(test)]
mod tests;

use hot_reload::spawn_pak_watcher;
use input::{
    apply_device_mouse_motion, apply_key_event, apply_window_cursor_move, engine_touch_phase,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subsystem {
    /// Render subsystem (window + Vulkan + render thread).
    Render,
    /// Audio engine (AudioEngine + audio decode thread).
    Audio,
    /// Physics simulation (Rapier background thread).
    Physics,
    /// Scene system
    Scene,
    /// Asset system
    Asset,
    UI,
    Animation,
}

impl Subsystem {
    fn bit(self) -> u32 {
        match self {
            Subsystem::Render => 1 << 0,
            Subsystem::Audio => 1 << 1,
            Subsystem::Physics => 1 << 2,
            Subsystem::Scene => 1 << 3,
            Subsystem::Asset => 1 << 4,
            Subsystem::UI => 1 << 5,
            Subsystem::Animation => 1 << 6,
        }
    }
}

// ===========================================================================
// App
// ===========================================================================

/// Application shell: owns engine, window and render thread.
/// Who owns the cursor: the app (FPS-style games) or the host (editors, tools).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PointerLockPolicy {
    /// The app grabs the pointer on the first left click, releases it on Esc, and
    /// re-grabs when the window regains focus. The engine camera reads
    /// `pointer_locked`, so this is the behaviour every FPS project expects.
    #[default]
    Auto,
    /// The host owns the cursor. The app forwards mouse input but never grabs or
    /// releases the pointer, so the cursor stays visible and usable.
    Manual,
}

pub struct App {
    // ---------- Startup config ----------
    config: AppConfig,

    /// Enabled subsystem bitmask.
    subsystems: u32,

    // ---------- Engine (main thread) ----------
    engine: Option<Engine>,
    asset_resolver: GpuAssetResolver,

    // ---------- Render thread ----------
    render_shared: Option<Arc<RenderShared>>,
    render_running: Option<Arc<AtomicBool>>,
    render_thread: Option<JoinHandle<()>>,

    // ---------- Window context ----------
    /// Window held by main thread (`Arc` shared with render thread). `None` means not yet created.
    platform: Option<PlatformContext>,

    // ---------- Frame hook (injected by editor/host etc) ----------
    frame_hook: Option<Box<dyn FrameHook>>,
    render_settings: RenderSettings,
    /// Runtime diagnostics HUD (F3; no UI framework dependency).
    /// Runtime diagnostics HUD (F3). Only exists when engine debug layer compiled.
    #[cfg(feature = "engine-debug")]
    debug_hud: DebugHud,

    // ---------- Per-frame state ----------
    display_aspect: f32,
    /// Dev-time asset hot reload (#495): polls `.pak` when `PRISMA_HOT_RELOAD` enabled.
    ///
    /// Release default off (no thread, no stats); after opening, rebuild `.pak` without exiting
    /// and re-entering game. Old GPU handles not immediately released (renderer has no per-handle recycle interface yet),
    /// so only suitable for dev-time repeated iteration.
    pak_watcher: Option<prism_engine::scene::hot_reload::HotReloadWatcher>,
    surface_rotation: glam::Mat4,

    // ---------- Input (main thread) ----------
    input: InputManager,
    /// Whether the app grabs the pointer on its own (FPS-style games) or leaves
    /// cursor ownership to the host (editors, tools).
    pointer_lock: PointerLockPolicy,

    // ---------- Audio (main thread, `audio` feature) ----------
    #[cfg(feature = "audio")]
    audio: Option<AudioEngine>,
    #[cfg(feature = "audio")]
    audio_decode_thread: Option<JoinHandle<()>>,
    #[cfg(feature = "audio")]
    audio_decode_tx: Option<flume::Sender<crate::audio_decode_runner::DecodeRequest>>,
    #[cfg(feature = "audio")]
    audio_decode_rx: Option<flume::Receiver<crate::audio_decode_runner::DecodeResult>>,
    /// Scene audio driver: connects ECS `AudioSource` to mixer (#508).
    #[cfg(feature = "audio")]
    scene_audio: crate::scene_audio::SceneAudio,
    /// `SceneAudio` voice id → real playback handle.
    #[cfg(feature = "audio")]
    audio_voices: crate::scene_audio::VoiceHandles,
    /// `GameWorld::play_sound` queued one-shot sounds (#987): id → playback handle + clip.
    #[cfg(feature = "audio")]
    one_shot_sounds: std::collections::HashMap<u32, crate::one_shot::OneShotSound>,

    // ---------- Physics (main thread + independent step thread, `physics` feature) ----------
    /// Each fixed step sends ECS `Rigidbody` / `Collider` to rapier, and writes results back to
    /// `LocalTransform` (#522). `None` = not enabled / build without physics feature.
    #[cfg(feature = "physics")]
    physics: Option<crate::physics_runner::PhysicsRuntime>,

    // ---------- Window resize ----------
    needs_resize: bool,

    /// Offscreen render size in host (editor) mode.
    ///
    /// `Some` means this instance has no winit window: `tick_sim` uses it to write `ScreenSize`/aspect ratio,
    /// render thread runs `RenderTarget::Offscreen`, frames read back via [`RenderShared::take_frame`].
    host_extent: Option<(u32, u32)>,
    /// Optional native window surface supplied by an embedding host.
    host_surface: Option<(prism_platform::SendWindowHandles, Vec<String>)>,
    host_native_surface: bool,

    /// Latest modifier key snapshot (winit `ModifiersChanged` / Win32 `GetKeyState`).
    ///
    /// Keyboard event itself does not carry modifier keys, PBR component debug hotkeys
    /// (Shift+1..5 = component bit 10..14) can only judge whether Shift is pressed from here,
    /// otherwise those 5 bits are forever unreachable.
    modifiers: Modifiers,

    // ---------- lifecycle ----------
    fatal_error: Option<String>,
    /// Background suspend flag: `suspended()` set, `resumed()` cleared. When set,
    /// `about_to_wait` skips `tick_sim`, preventing background idle spinning burning battery (T4).
    suspended: bool,
    /// The window was created hidden and must be shown once a frame exists.
    ///
    /// Renderer construction and pipeline/font warmup take hundreds of
    /// milliseconds; showing the window before that paints an empty rectangle
    /// the user reads as a hang.
    pending_show: bool,

    // ---------- timing ----------
    last_frame: Option<Instant>,
    /// Fixed-step accumulator in nanoseconds. Keeping this as an integer
    /// avoids float drift and makes simulation time independent of event rate.
    fixed_accumulator: Duration,

    // ---------- plugins (#1165) ----------
    pub(crate) plugin_registry: crate::plugin::PluginRegistry,
}

/// Normalize asset paths written in scenes to relative paths inside pak.
///
/// Scene authors often write `assets/` prefix or relative paths (`../x.hdr`), while pak keys are
/// `assets/` stripped and already-resolved `.`/`..` relative paths; without normalization cannot look up.
fn normalize_asset_path(path: &str) -> String {
    let unified = path.replace('\\', "/");
    let stripped = unified.strip_prefix("assets/").unwrap_or(unified.as_str());
    let mut parts: Vec<&str> = Vec::new();
    for segment in stripped.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// Project state parked in the world while its gameplay system is not running.
///
/// The wrapper keeps a project resource from colliding with an engine or another
/// project's resource of the same type.
struct GameplayState<S>(Option<S>);

/// Run one gameplay tick with the project state temporarily taken out of the world.
fn tick_gameplay_state<S, F>(world: &mut prism_engine::ecs::World, tick: &mut F, dt: f32)
where
    S: Send + Sync + 'static,
    F: FnMut(&mut Option<S>, &mut prism_game_api::GameWorld<'_>, f32),
{
    let mut state = world
        .remove_resource::<GameplayState<S>>()
        .map(|held| held.0)
        .unwrap_or(None);
    {
        let mut api = prism_game_api::GameWorld::host(world);
        tick(&mut state, &mut api, dt);
    }
    world.insert_resource(GameplayState(state));
}

impl App {
    /// Parameterless constructor = complete engine with default config (no demo content).
    pub fn new() -> Self {
        Self::with_config(crate::load_config())
    }

    /// Create app with specified config, run all engine initialization stages.
    ///
    /// User projects then register their own ECS content via [`Self::engine_mut`] / [`Self::add_system`] /
    /// [`Self::insert_resource`] to register their own ECS content, finally call
    /// [`Self::run`] (desktop) or [`Self::run_on`] (Android) to start event loop.
    pub fn with_config(config: AppConfig) -> Self {
        // Bring the logger up before `build`, which mounts the asset packages and
        // loads the startup scene; their warnings must not be dropped (#1013).
        crate::logging::init_player_logging();
        Self::build(config, true)
    }

    /// Host (editor / tool) mode: **no startup scene load**.
    ///
    /// The only difference from [`Self::with_config`] is that `init_scene_named`
    /// is never called, so the process does not try to resolve `assets/scenes.json`
    /// and is therefore never marked "scene missing" (which would paint the
    /// full-window `ui::no_scene` placeholder). The host loads documents itself
    /// through [`Self::load_scene_json_file`].
    ///
    /// The window depends on how the app is then driven: [`Self::run`] /
    /// [`Self::run_on`] still create a winit window (the self-hosted editor takes
    /// this path), while [`Self::start_hosted`] renders offscreen and lets the host
    /// pull frames at its own pace.
    pub fn hosted(config: AppConfig) -> Self {
        Self::build(config, false)
    }

    /// Hosted engine backed by a host-owned native window surface.
    ///
    /// The host window must outlive the App and its render thread.
    pub fn hosted_surface(
        config: AppConfig,
        handles: prism_platform::SendWindowHandles,
        extensions: Vec<String>,
    ) -> Self {
        let mut app = Self::build(config, false);
        app.host_surface = Some((handles, extensions));
        app
    }

    fn build(config: AppConfig, load_scene: bool) -> Self {
        let mut build_span = prism_engine::util::timing::span("app::build");
        let mut engine = Engine::empty();

        // Phase 1 – Subsystem registration

        // Resource loading: ResourceManager held by app layer and injected into engine.
        let mut asset_resolver = GpuAssetResolver::new();
        asset_resolver.load_resource_package();

        // The launch config can declare that this process wants no scene at all
        // (editor / tool host). Skip manifest resolution entirely: a missing
        // `assets/scenes.json` would otherwise be recorded as "scene missing" and
        // `update` would paint the full-window placeholder.
        let launch = prism_engine::launch_config::LaunchConfig::from_env();
        if load_scene && !launch.no_scene {
            // Scene loading — launch config can specify scene name (editor/launcher preview);
            // when not specified follows default behavior "manifest first parseable scene".
            engine.init_scene_named(
                &mut asset_resolver.resource_manager,
                launch.scene.as_deref(),
            );
        }

        // Save directory follows project `[app] company/name` (AppConfig already read from settings).
        // Previously `scene_state_path()` used default config, all projects shared one directory (#484).
        let _save_dir_span = prism_engine::util::timing::span("app::open_save_directory");
        if let Some(dir) = prism_engine::config::app_data_dir_for(&config) {
            // Player save (#984) and scene save same directory: both follow `[app] company/name`.
            // when `PRISMA_USER_DATA` set use it instead (portable / Android host).
            let player_data = if std::env::var_os(prism_engine::user_data::ENV_OVERRIDE).is_some() {
                prism_engine::user_data::save_path(&config.app.name)
            } else {
                dir.join(prism_engine::user_data::FILE_NAME)
            };
            engine.set_save_directory(dir);
            engine
                .world_mut()
                .insert_resource(prism_engine::user_data::UserStore::open(player_data));
        }

        // Runtime startup callbacks
        engine.runtime_initialize();

        // Project-level render defaults (`[render]` section).
        let mut render_settings = RenderSettings::default();
        config.render.apply(&mut render_settings);
        build_span.detail(format!(
            "load_scene={} assets={}",
            load_scene,
            asset_resolver.resource_manager.asset_count()
        ));

        Self {
            config,
            subsystems: 0,
            engine: Some(engine),
            asset_resolver,
            render_shared: None,
            render_running: None,
            render_thread: None,
            platform: None,
            frame_hook: None,
            render_settings,
            #[cfg(feature = "engine-debug")]
            debug_hud: DebugHud::default(),
            display_aspect: 16.0 / 9.0,
            pak_watcher: {
                let _watcher_span = prism_engine::util::timing::span("app::spawn_pak_watcher");
                spawn_pak_watcher()
            },
            surface_rotation: glam::Mat4::IDENTITY,
            input: InputManager::new(),
            #[cfg(feature = "audio")]
            audio: None,
            #[cfg(feature = "audio")]
            audio_decode_thread: None,
            #[cfg(feature = "audio")]
            audio_decode_tx: None,
            #[cfg(feature = "audio")]
            audio_decode_rx: None,
            #[cfg(feature = "audio")]
            scene_audio: crate::scene_audio::SceneAudio::new(),
            #[cfg(feature = "audio")]
            audio_voices: crate::scene_audio::VoiceHandles::default(),
            #[cfg(feature = "audio")]
            one_shot_sounds: std::collections::HashMap::new(),
            #[cfg(feature = "physics")]
            physics: None,
            pointer_lock: PointerLockPolicy::Auto,
            needs_resize: false,
            host_extent: None,
            host_surface: None,
            host_native_surface: false,
            modifiers: Modifiers::empty(),
            fatal_error: None,
            suspended: false,
            pending_show: false,
            last_frame: None,
            fixed_accumulator: Duration::ZERO,
            plugin_registry: crate::plugin::PluginRegistry::new(),
        }
    }

    // -----------------------------------------------------------------------
    // ECS integration (user projects)
    // -----------------------------------------------------------------------

    /// Full ECS access — world / schedule / timer etc entry points all via `Engine`.
    /// Current pointer-ownership policy (diagnostics / tests).
    pub fn pointer_lock_policy(&self) -> PointerLockPolicy {
        self.pointer_lock
    }

    /// Read-only engine access (diagnostics / tests).
    pub fn engine(&self) -> &Engine {
        self.engine.as_ref().expect("engine alive")
    }

    pub fn engine_mut(&mut self) -> &mut Engine {
        self.engine.as_mut().expect("engine alive")
    }

    /// Set gain of one audio bus (#508); `Master` multiplies all other buses.
    ///
    /// No-op without `audio` feature, convenient for unconditional project code calls.
    pub fn set_audio_bus_volume(
        &mut self,
        bus: prism_engine::scene::components::AudioBus,
        volume: f32,
    ) -> &mut Self {
        #[cfg(feature = "audio")]
        self.scene_audio.set_bus_volume(bus, volume);
        #[cfg(not(feature = "audio"))]
        let _ = (bus, volume);
        self
    }

    /// Read one audio bus's own gain (excluding `Master`); 1 without `audio` feature.
    pub fn audio_bus_volume(&self, bus: prism_engine::scene::components::AudioBus) -> f32 {
        #[cfg(feature = "audio")]
        {
            self.scene_audio.bus_volume(bus)
        }
        #[cfg(not(feature = "audio"))]
        {
            let _ = bus;
            1.0
        }
    }

    /// Register an ECS system (equivalent to `engine_mut().schedule_mut().add_system`).
    ///
    /// Engine default schedule (UI infrastructure) already merged at construction, systems registered here
    /// run every frame after in registration order.
    pub fn add_system<F>(&mut self, label: &str, f: F) -> &mut Self
    where
        F: FnMut(&mut prism_engine::ecs::World, f32) + 'static,
    {
        self.engine_mut().schedule_mut().add_system(label, f);
        self
    }

    /// Register an ECS system that runs once per fixed simulation step.
    pub fn add_fixed_system<F>(&mut self, label: &str, f: F) -> &mut Self
    where
        F: FnMut(&mut prism_engine::ecs::World, f32) + 'static,
    {
        self.engine_mut().fixed_schedule_mut().add_system(label, f);
        self
    }

    /// Register a gameplay system whose project state lives in an ECS resource.
    ///
    /// `tick` gets the same signature the preview module uses, so gameplay code is
    /// written once: the state arrives as `Option<&mut S>`, and `None` means it could
    /// not be bound yet (for example right after a scene reload). The state is put
    /// back before the system returns and after a scene switch, so a project cannot
    /// silently drop it by forgetting `insert_resource` (#1054).
    pub fn add_game_system<S, F>(&mut self, label: &str, mut tick: F) -> &mut Self
    where
        S: Send + Sync + 'static,
        F: FnMut(&mut Option<S>, &mut prism_game_api::GameWorld<'_>, f32) + 'static,
    {
        self.engine_mut()
            .world_mut()
            .insert_resource(GameplayState::<S>(None));
        self.engine_mut()
            .schedule_mut()
            .add_system(label, move |world, dt| {
                tick_gameplay_state::<S, _>(world, &mut tick, dt);
            });
        self
    }

    /// Register deterministic gameplay on the engine's fixed simulation schedule.
    pub fn add_fixed_game_system<S, F>(&mut self, label: &str, mut tick: F) -> &mut Self
    where
        S: Send + Sync + 'static,
        F: FnMut(&mut Option<S>, &mut prism_game_api::GameWorld<'_>, f32) + 'static,
    {
        self.engine_mut()
            .world_mut()
            .insert_resource(GameplayState::<S>(None));
        self.engine_mut()
            .fixed_schedule_mut()
            .add_system(label, move |world, dt| {
                tick_gameplay_state::<S, _>(world, &mut tick, dt);
            });
        self
    }

    /// Insert an ECS resource (equivalent to `engine_mut().world_mut().insert_resource`).
    pub fn insert_resource<R: 'static + Send + Sync>(&mut self, resource: R) -> &mut Self {
        self.engine_mut().world_mut().insert_resource(resource);
        self
    }

    /// Modify app-level render settings; user projects can choose Raster or PathTrace at startup,
    /// without depending on editor or direct renderer access.
    pub fn render_settings_mut(&mut self) -> &mut RenderSettings {
        &mut self.render_settings
    }

    /// Configure app-level render settings via closure.
    pub fn with_render_settings(mut self, configure: impl FnOnce(&mut RenderSettings)) -> Self {
        configure(&mut self.render_settings);
        self
    }

    /// Stop the app from grabbing or releasing the pointer on its own.
    ///
    /// The default (`PointerLockPolicy::Auto`) is the FPS behaviour: the first
    /// left click grabs the cursor, Esc releases it, and regaining focus re-grabs.
    /// That is wrong for a host that owns the cursor itself — an editor needs a
    /// visible pointer for selecting and dragging, and the auto-grab hides it on
    /// the first click anywhere in the window.
    ///
    /// With `Manual` the app still forwards mouse motion and button events to the
    /// engine; it just never calls `grab_pointer` / `release_pointer`.
    pub fn with_pointer_lock_policy(mut self, policy: PointerLockPolicy) -> Self {
        self.pointer_lock = policy;
        self
    }

    /// Inject frame hook (editor etc host). Hook asked for external overlay factory before render thread starts,
    /// overlay factory, then receives tick and window event forwarding each frame.
    pub fn with_frame_hook(mut self, hook: impl FrameHook + 'static) -> Self {
        self.frame_hook = Some(Box::new(hook));
        self
    }

    /// Explicitly declare need to enable an engine subsystem.
    ///
    /// If not called, corresponding subsystem not created (no window, no audio, no physics, etc.).
    pub fn with_subsystem(mut self, subsystem: Subsystem) -> Self {
        self.enable_subsystem(subsystem);
        self
    }

    /// Explicitly enable an engine subsystem.
    pub fn enable_subsystem(&mut self, subsystem: Subsystem) -> &mut Self {
        let bit = subsystem.bit();
        if self.subsystems & bit != 0 {
            return self; // already enabled, skip
        }
        self.subsystems |= bit;

        match subsystem {
            Subsystem::Render => {
                // render conditionally created at resumed()
            }
            Subsystem::Physics => {
                #[cfg(feature = "physics")]
                {
                    // #522: physics thread integrates `Rigidbody` each fixed step, results written back to
                    // `LocalTransform`; `Collider` continues serving raycast queries.
                    self.physics = Some(crate::physics_runner::PhysicsRuntime::spawn());
                }
                #[cfg(not(feature = "physics"))]
                log::warn!(
                    "physics subsystem requested but this build has no `physics` feature — \
                     colliders remain query-only; rebuild with `--features physics` to step bodies"
                );
            }
            Subsystem::Audio => {
                #[cfg(feature = "audio")]
                {
                    match AudioEngine::new(prism_audio::AudioConfig::default()) {
                        Ok(engine) => self.audio = Some(engine),
                        // do not stay silent when no sound card / device is exclusive: one-shot sounds would all be discarded.
                        Err(error) => log::warn!(
                            "audio subsystem requested but failed to initialize: {error}"
                        ),
                    }
                    self.start_audio_decode_thread();
                }
                #[cfg(not(feature = "audio"))]
                log::warn!(
                    "audio subsystem requested but this build has no `audio` feature — \
                     rebuild with `--features audio` to enable sound"
                );
            }
            Subsystem::Scene | Subsystem::Asset | Subsystem::UI | Subsystem::Animation => {
                // Scene/asset/UI/animation are always-on engine capabilities, no separate switch to flip.
                log::debug!("{:?} subsystem is always on — nothing to start", subsystem);
            }
        }
        self
    }

    /// Disable an engine subsystem and release its background runtime (if exists).
    pub fn disable_subsystem(&mut self, subsystem: Subsystem) -> &mut Self {
        let bit = subsystem.bit();
        if self.subsystems & bit == 0 {
            return self;
        }
        self.subsystems &= !bit;
        match subsystem {
            Subsystem::Physics => {
                #[cfg(feature = "physics")]
                {
                    self.physics = None;
                }
            }
            Subsystem::Audio => {
                #[cfg(feature = "audio")]
                {
                    self.audio = None;
                    if let Some(handle) = self.audio_decode_thread.take() {
                        let _ = handle.join();
                    }
                    self.audio_decode_tx = None;
                    self.audio_decode_rx = None;
                }
            }
            _ => {}
        }
        self
    }

    // -----------------------------------------------------------------------
    // Plugin system (#1165)
    // -----------------------------------------------------------------------

    /// Register an app plugin (customize/replace subsystems, add resources or inject ECS logic).
    pub fn add_plugin<P: crate::plugin::AppPlugin>(&mut self, plugin: P) -> &mut Self {
        self.plugin_registry.add(plugin);
        self
    }

    /// Register an app plugin via chained call.
    pub fn with_plugin<P: crate::plugin::AppPlugin>(mut self, plugin: P) -> Self {
        self.add_plugin(plugin);
        self
    }

    /// Register a group of plugins.
    pub fn add_plugins<G: crate::plugin::PluginGroup>(&mut self, group: G) -> &mut Self {
        group.build(self);
        self
    }

    /// Register a group of plugins via chained call.
    pub fn with_plugins<G: crate::plugin::PluginGroup>(mut self, group: G) -> Self {
        group.build(&mut self);
        self
    }

    /// Disable a specific type of plugin (its `build` method will be skipped).
    pub fn disable_plugin<P: crate::plugin::AppPlugin>(&mut self) -> &mut Self {
        self.plugin_registry.disable::<P>();
        self
    }

    /// Check whether specified type of plugin is currently enabled.
    pub fn is_plugin_enabled<P: crate::plugin::AppPlugin>(&self) -> bool {
        self.plugin_registry.is_enabled::<P>()
    }

    /// Immediately build and apply all registered pending plugins.
    pub fn build_plugins(&mut self) -> &mut Self {
        crate::plugin::PluginRegistry::apply_pending(self);
        self
    }

    /// Check whether specified subsystem is enabled.
    pub fn has_subsystem(&self, subsystem: Subsystem) -> bool {
        self.subsystems & subsystem.bit() != 0
    }

    // -----------------------------------------------------------------------
    // Host (editor) mode
    // -----------------------------------------------------------------------

    /// Current render surface logical size (pixels).
    ///
    /// Host mode uses [`Self::host_extent`], game mode takes window client area size.
    fn surface_size(&self) -> (u32, u32) {
        if let Some(size) = self.host_extent {
            return size;
        }
        self.platform
            .as_ref()
            .map(|platform| platform.window().inner_size())
            .unwrap_or((1920, 1080))
    }

    /// Start render thread with **offscreen target** (editor viewport).
    ///
    /// Mutually exclusive with [`Self::run`]/[`Self::run_on`]: host mode does not create winit window,
    /// host calls [`Self::host_tick`] at its own pace and takes
    /// [`Self::take_hosted_frame`] returned pixels.
    pub fn start_hosted(&mut self, width: u32, height: u32) {
        // Every entry point that starts delivering frames must drain the pending
        // plugin list first, or a host driving the app itself (`start_hosted` +
        // `host_tick`) silently loses every plugin it registered.
        self.build_plugins();
        let width = width.max(1);
        let height = height.max(1);
        self.host_extent = Some((width, height));
        self.display_aspect = width as f32 / height as f32;
        let scene_env = self.engine.as_ref().and_then(|engine| {
            engine.current_scene_env_bytes_with_provider(&mut self.asset_resolver.resource_manager)
        });
        let target = match self.host_surface.take() {
            Some((handles, extensions)) => {
                self.host_native_surface = true;
                RenderTarget::NativeSurface {
                    handles,
                    extensions,
                    vsync: false,
                }
            }
            None => RenderTarget::Offscreen { width, height },
        };
        self.spawn_render_thread_with(target, scene_env, None);
    }

    /// Adjust offscreen render size (panel resize); takes effect from next frame.
    pub fn host_resize(&mut self, width: u32, height: u32) {
        let size = (width.max(1), height.max(1));
        if self.host_extent == Some(size) {
            return;
        }
        self.host_extent = Some(size);
        self.display_aspect = size.0 as f32 / size.1 as f32;
        if let Some(shared) = self.render_shared.as_ref() {
            let (w, h) = size;
            shared.send_renderer_message(Box::new(move |renderer| {
                let result = if renderer.is_headless() {
                    renderer.resize_offscreen(w, h)
                } else {
                    renderer.recreate_swapchain()
                };
                if let Err(e) = result {
                    log::warn!("host resize ({w}x{h}) failed: {e:#}");
                }
            }));
        }
    }

    /// Host-driven one frame: simulate (optional) + extract + submit render + request readback.
    ///
    /// `simulate = false` is editor "edit mode": only propagates hierarchy, does not run game systems.
    pub fn host_tick(&mut self, simulate: bool) {
        self.tick_sim(simulate);
        if let Some(shared) = self.render_shared.as_ref() {
            if self.host_native_surface {
                shared.wake_renderer();
            } else {
                shared.request_frame();
            }
        }
    }

    /// Take the most recent frame read back by render thread (RGBA8).
    pub fn take_hosted_frame(&self) -> Option<FrameImage> {
        self.render_shared
            .as_ref()
            .and_then(|shared| shared.take_frame())
    }

    /// Render thread fatal error (init failure / thread exit). Host mode uses it to report errors to panel.
    pub fn render_failure(&self) -> Option<String> {
        self.render_shared
            .as_ref()
            .and_then(|shared| shared.read_render_failure())
    }

    /// Render thread real-time stats (FPS / frame time).
    pub fn render_stats(&self) -> Option<RenderStats> {
        self.render_shared
            .as_ref()
            .map(|shared| shared.read_render_stats())
    }

    /// Input handle: host writes mouse/keyboard events into same [`InputManager`].
    pub fn input_manager_mut(&mut self) -> &mut InputManager {
        &mut self.input
    }

    /// Stop host render thread and cleanup (audio / engine pre/post shutdown).
    pub fn shutdown_hosted(&mut self) {
        if let Some(watcher) = self.pak_watcher.as_mut() {
            watcher.stop();
        }
        self.pak_watcher = None;
        #[cfg(feature = "audio")]
        self.stop_audio_decode_thread();
        self.stop_render_thread();
        if let Some(engine) = self.engine.as_mut() {
            engine.pre_shutdown();
        }
        #[cfg(feature = "audio")]
        self.audio.take();
        if let Some(engine) = self.engine.as_mut() {
            engine.post_shutdown();
        }
    }

    /// Sync once with render thread: wait for it to process **already queued** asset uploads, then discard results.
    ///
    /// Reloading scene replaces entire world, while `prism-ecs` entity ids re-allocated from 0;
    /// old scene in-flight upload results if late will attach to new scene same-id entities (wrong mesh).
    /// render thread processes `asset_requests` first each frame, then `renderer_messages`, so
    /// one message returned means previous requests have all been applied to result queue.
    fn sync_render_thread(&self) {
        let Some(shared) = self.render_shared.as_ref() else {
            return;
        };
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        shared.send_renderer_message(Box::new(move |_| {
            let _ = done_tx.send(());
        }));
        if done_rx.recv_timeout(Duration::from_millis(500)).is_err() {
            log::warn!("scene reload: render thread sync timed out");
        }
        let _ = shared.take_asset_results();
    }

    /// Load / switch scene from loose `.scene.json` (host / editor).
    ///
    /// **Reuse same `Engine`** (#482): `Engine::load_scene_json_file` is `Single`
    /// mode --- load new scene first, unload old scene after success; `Persistent` entities (equivalent
    /// `DontDestroyOnLoad`) survive scene switch along with subtree; registered systems/schedules will not
    /// be flushed out.
    ///
    /// Old scene GPU handles recycled by asset resolver: unloaded entities no longer appear in world,
    /// `prepare_requests` will return their texture / mesh / material slots to renderer (#495).
    /// Only first load creates new engine and asset resolver (`.pak` also mounted only once).
    ///
    /// Returns entity table in **document order** (one-to-one with `.scene.json` `entities` index).
    pub fn load_scene_json_file(
        &mut self,
        path: &Path,
    ) -> Result<Vec<prism_engine::ecs::Entity>, String> {
        self.sync_render_thread();
        if self.engine.is_none() {
            self.engine = Some(Engine::empty());
            self.asset_resolver = GpuAssetResolver::new();
            self.asset_resolver.load_resource_package();
            self.pak_watcher = spawn_pak_watcher();
        }
        let engine = self.engine.as_mut().expect("engine present");
        let entities = engine.load_scene_json_file(path)?;
        engine.runtime_initialize();

        self.last_frame = None;
        self.fixed_accumulator = Duration::ZERO;
        self.reset_persistent_play_state();
        self.apply_scene_environment();
        Ok(entities)
    }

    /// #509: Mount optional packages under `assets/optional/` on demand (`optional_*.pak`).
    ///
    /// Auto-mount at startup only scans `assets/` top level, so optional package resources do not occupy resident memory.
    /// Call when entering levels/gameplay needing them; already-mounted packages skipped, repeat calls safe.
    /// After mounting, entities previously failed due to "path not in package" will be re-resolved (otherwise failure cache will
    /// block them). Returns **newly mounted** package count this time.
    pub fn load_optional_packages(&mut self) -> usize {
        let count = self.asset_resolver.load_optional_packages();
        if count > 0 {
            if let Some(engine) = self.engine.as_mut() {
                self.asset_resolver
                    .retry_failed_after_package_change(engine.world_mut());
            }
        }
        count
    }

    /// Clear playback state only meaningful for "previous scene" after scene switch.
    ///
    /// Asset resolver accounts by `(entity, slot)` (#495); old entity already unloaded, here lets
    /// cache immediately discard their entries, instead of waiting until next `prepare_requests` to clean up.
    fn reset_persistent_play_state(&mut self) {
        let Some(engine) = self.engine.as_mut() else {
            return;
        };
        self.asset_resolver.forget_missing_entities(engine.world());
    }

    /// Hand scene `Skybox`-pointed HDR to render thread to rebuild IBL.
    ///
    /// IBL under manifest path comes from environment bytes cooked into `.rscn`; loose `.scene.json`
    /// has no manifest entry, so host fetches and sends on the spot by `hdr_path` in component.
    fn apply_scene_environment(&mut self) {
        let Some(shared) = self.render_shared.clone() else {
            return;
        };
        let Some(engine) = self.engine.as_ref() else {
            return;
        };
        let Some(path) =
            prism_engine::scene::SceneReadView::new(engine.world()).environment_asset_path()
        else {
            return;
        };
        let normalized = normalize_asset_path(&path);
        let Some(id) = self
            .asset_resolver
            .resource_manager
            .id_by_path(normalized.as_str())
        else {
            log::warn!("hosted: environment `{path}` not found in package");
            return;
        };
        let bytes = match self.asset_resolver.resource_manager.load_with_deps_raw(id) {
            Ok(bytes) => bytes,
            Err(e) => {
                log::warn!("hosted: load environment `{path}` failed: {e}");
                return;
            }
        };
        shared.send_renderer_message(Box::new(move |renderer| {
            if let Err(e) = renderer.set_environment(Some(bytes)) {
                log::warn!("hosted: set_environment failed: {e:#}");
            }
        }));
    }

    // -----------------------------------------------------------------------
    // Launch config
    // -----------------------------------------------------------------------

    /// Construct window parameters for this launch (shared by both backends).
    fn window_config(&self) -> WindowConfig {
        WindowConfig {
            title: self.config.window.title.clone(),
            width: self.config.window.width,
            height: self.config.window.height,
            min_width: self.config.window.min_width,
            min_height: self.config.window.min_height,
            max_width: self.config.window.max_width,
            max_height: self.config.window.max_height,
            position_x: self.config.window.position_x,
            position_y: self.config.window.position_y,
            resizable: self.config.window.resizable,
            fullscreen: self.config.window.fullscreen,
            maximized: self.config.window.maximized,
            visible: self.config.window.visible,
            decorations: self.config.window.decorations,
            vsync: self.config.window.vsync,
        }
    }

    /// Start event loop (desktop entry, self-built winit EventLoop).
    ///
    /// Internally construct EventLoop and hand to [`Self::run_on`]; fatal errors panic directly.
    /// If env var `PRISMA_HEADLESS` set, directly go pure background offscreen loop, no window created.
    #[cfg(feature = "winit-backend")]
    pub fn run(mut self) {
        prism_engine::util::timing::init();
        // Fallback: construction paths outside `App::hosted` also need logger before event loop (idempotent).
        crate::logging::init_player_logging();
        Self::log_asset_root();
        prism_engine::util::timing::event("app::event_loop_ready", "");

        if let Some(frames) = Self::headless_frames_from_env() {
            log::info!("headless mode requested via PRISMA_HEADLESS ({frames} frames)");
            let (w, h) = self.default_headless_extent();
            self.run_headless(frames, w, h)
                .expect("fatal headless application error");
            return;
        }

        let loop_span = prism_engine::util::timing::span("app::create_event_loop");
        let event_loop = winit::event_loop::EventLoop::new().expect("failed to create event loop");
        drop(loop_span);
        self.run_on(event_loop).expect("fatal application error");
    }

    /// Run on already-built winit EventLoop (Android entry used).
    ///
    /// Parse launch config before startup (`PRISMA_LAUNCH_CONFIG` env) and set log level.
    ///
    /// Android `android_main` should inject before calling this function **by itself**
    /// `PRISMA_LAUNCH_CONFIG` env (read from files directory).
    #[cfg(feature = "winit-backend")]
    pub fn run_on(mut self, event_loop: winit::event_loop::EventLoop<()>) -> anyhow::Result<()> {
        crate::logging::init_player_logging();
        self.build_plugins();
        event_loop.run_app(&mut self)?;
        Ok(())
    }

    /// Start application (desktop entry, Win32 native window + own message loop).
    ///
    /// Equivalent to winit entry --- window, render thread, simulation loop, shutdown order exactly same,
    /// only window and messages come from `prism-platform` `win32-native` backend.
    #[cfg(all(windows, feature = "win32-native", not(feature = "winit-backend")))]
    pub fn run(mut self) {
        crate::logging::init_player_logging();
        Self::log_asset_root();

        if let Some(frames) = Self::headless_frames_from_env() {
            log::info!("headless mode requested via PRISMA_HEADLESS ({frames} frames)");
            let (w, h) = self.default_headless_extent();
            self.run_headless(frames, w, h)
                .expect("fatal headless application error");
            return;
        }

        self.build_plugins();
        self.run_win32().expect("fatal application error");
    }

    fn headless_frames_from_env() -> Option<u32> {
        let env_val = std::env::var("PRISMA_HEADLESS").ok()?;
        let val = env_val.trim();
        if val.is_empty() || val == "0" || val.eq_ignore_ascii_case("false") {
            return None;
        }
        if let Ok(n) = val.parse::<u32>() {
            return Some(n.max(1));
        }
        let explicit = std::env::var("PRISMA_HEADLESS_FRAMES")
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok());
        if let Some(n) = explicit {
            return Some(n.max(1));
        }
        let pass_dump = std::env::var("PRISMA_PASS_DUMP_FRAME")
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .map(|f| f + 5);
        if let Some(n) = pass_dump {
            return Some(n);
        }
        let screenshot = std::env::var("PRISMA_SCREENSHOT_FRAME")
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .map(|f| f + 5);
        if let Some(n) = screenshot {
            return Some(n);
        }
        Some(60)
    }

    fn default_headless_extent(&self) -> (u32, u32) {
        if let Ok(size_str) = std::env::var("PRISMA_SCREENSHOT_SIZE") {
            let normalized = size_str.trim().replace(['X', ',', '*'], "x");
            if let Some((w, h)) = normalized.split_once('x') {
                if let (Ok(w), Ok(h)) = (w.trim().parse::<u32>(), h.trim().parse::<u32>()) {
                    if w > 0 && h > 0 {
                        return (w, h);
                    }
                }
            }
        }
        let win_cfg = self.window_config();
        (win_cfg.width.max(1), win_cfg.height.max(1))
    }

    /// Run the application headlessly for `frames` ticks without creating a window.
    pub fn run_headless(&mut self, frames: u32, width: u32, height: u32) -> anyhow::Result<()> {
        // `start_hosted` drains pending plugins itself.
        self.start_hosted(width, height);
        for index in 0..frames.max(1) {
            self.host_tick(true);
            for _ in 0..100 {
                if let Some(failure) = self.render_failure() {
                    anyhow::bail!("render thread failed: {failure}");
                }
                if self.take_hosted_frame().is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if (index + 1) % 50 == 0 || index + 1 == frames {
                log::info!("headless tick: {} / {frames} frames", index + 1);
            }
        }
        self.shutdown_hosted();
        Ok(())
    }

    /// Print one line of asset root after logging available (#985).
    ///
    /// `asset_paths::asset_root()` called for every asset query, can only leave `trace` there;
    /// one line in startup log enough to locate "where assets/ actually resolved to", no need to let editor
    /// log panel be swamped by hundreds of same path lines.
    fn log_asset_root() {
        if let Some(root) = prism_engine::asset_paths::asset_root() {
            log::debug!("asset root: {}", root.display());
        }
    }

    /// Restart subsystems released with suspend on resume (audio stream / decode thread).
    fn resume_subsystems(&mut self) {
        #[cfg(feature = "audio")]
        if self.has_subsystem(Subsystem::Audio) {
            if let Some(audio) = self.audio.as_mut() {
                audio.resume_stream();
            }
            // `suspended()` shuts the decode thread down; restart it so
            // playback queued after resume is decoded again.
            self.start_audio_decode_thread();
        }
    }

    /// Per-frame main thread work: render thread survival check → simulate → host hook.
    ///
    /// Returns `Some(message)` meaning render thread already died (init failure / panic),
    /// caller should pop fatal error and close window, instead of spinning a blank window.
    fn tick_frame(&mut self) -> Option<String> {
        let _frame_span = prism_engine::util::timing::span("app::tick_frame");
        // The render thread exclusively owns the Vulkan context. If it exited
        // (renderer init failure or panic), stop instead of spinning a blank
        // window at 100% CPU.
        if let Some(handle) = self.render_thread.as_ref() {
            if handle.is_finished() {
                return Some(
                    self.render_shared
                        .as_ref()
                        .and_then(|shared| shared.take_render_failure())
                        .unwrap_or_else(|| {
                            "render thread exited unexpectedly (see log)".to_string()
                        }),
                );
            }
        }

        self.tick_sim(true);

        // Frame hook (host UI etc) — main thread. All use field-level borrows (not borrow self as a whole).
        let window = self.platform.as_ref().map(PlatformContext::window_arc);
        if let (Some(hook), Some(shared), Some(engine), Some(window)) = (
            self.frame_hook.as_mut(),
            self.render_shared.as_ref(),
            self.engine.as_mut(),
            window,
        ) {
            let stats = shared.read_render_stats();
            hook.on_tick(
                &window,
                engine.world_mut(),
                &mut self.render_settings,
                &stats,
                shared,
            );
        }

        // Reveal the window once the render thread has finished building and
        // warming up. Everything expensive is behind us at that point, so the
        // first frame follows within one frame interval and the window never
        // appears as an empty rectangle.
        if self.pending_show {
            let ready = self
                .render_shared
                .as_ref()
                .is_some_and(|shared| shared.is_renderer_ready());
            if ready {
                if let Some(platform) = self.platform.as_ref() {
                    platform.window().set_visible(true);
                }
                self.pending_show = false;
                log::debug!(
                    target: "startup",
                    "[startup] window shown (first frame already presented)"
                );
            }
        }

        None
    }

    /// Shutdown order (shared by both backends) — order matters.
    fn shutdown(&mut self) {
        // 1. Stop audio decode thread
        #[cfg(feature = "audio")]
        self.stop_audio_decode_thread();
        // 2. Stop render thread
        self.stop_render_thread();
        // 4. Engine pre-shutdown
        if let Some(ref mut engine) = self.engine {
            engine.pre_shutdown();
        }
        // 5. Drop audio engine (stop audio stream before platform/engine)
        #[cfg(feature = "audio")]
        self.audio.take();
        // 6. Drop window (renderer owned by render thread, destroyed with render thread stop)
        self.platform = None;
        // 7. Engine post-shutdown
        if let Some(ref mut engine) = self.engine {
            engine.post_shutdown();
        }
    }

    /// Take fatal error text (dialog content shown before closing window).
    fn take_fatal_message(&mut self) -> String {
        self.fatal_error
            .take()
            .unwrap_or_else(|| "An unknown fatal error occurred.".to_string())
    }

    /// Win32 native window main loop (`win32-native` built `run_app` equivalent).
    ///
    /// Message loop uses `PeekMessageW` pump + `MsgWaitForMultipleObjectsEx` to yield
    /// CPU (see `prism_platform::win32` module docs): main thread must continue advancing simulation
    /// when no window messages; blocking `GetMessageW` freezes static screen.
    #[cfg(all(windows, feature = "win32-native", not(feature = "winit-backend")))]
    fn run_win32(mut self) -> anyhow::Result<()> {
        // Reset suspend flag + restore subsystems (corresponds to winit `resumed`).
        self.suspended = false;
        self.resume_subsystems();

        if !self.has_subsystem(Subsystem::Render) {
            log::info!("render subsystem disabled — headless mode (no window, no GPU)");
            return Ok(());
        }

        let resumed_entry = crate::render_shared::startup_ms();
        let window_config = self.window_config();
        let window_span = prism_engine::util::timing::span("app::create_window_native");
        let platform = match PlatformContext::create_window_native(&window_config) {
            Ok(platform) => platform,
            Err(error) => {
                log::error!("failed to create native window: {error:#}");
                return Err(error);
            }
        };
        let window_built = crate::render_shared::startup_ms();
        drop(window_span);

        self.platform = Some(platform);
        // Render thread asynchronously builds renderer + starts frame loop (heavy init doesn't block main thread).
        let spawn_span = prism_engine::util::timing::span("app::spawn_render_thread");
        self.spawn_render_thread();
        drop(spawn_span);

        let spawned = crate::render_shared::startup_ms();
        if let Some(shared) = self.render_shared.as_ref() {
            shared.set_mark("resumed_entry", resumed_entry);
            shared.set_mark("window_built", window_built);
            shared.set_mark("render_thread_spawned", spawned);
        }

        let window = match self.platform.as_ref() {
            Some(platform) => platform.window_arc(),
            None => return Ok(()),
        };

        let mut exit = false;
        while !exit {
            // 1. Message pump: convert WM_* to neutral events (including user close button click).
            let events = window.poll_events();
            for event in events {
                if self.fatal_error.is_some() {
                    exit = true;
                    break;
                }

                // First send events to frame hook (host UI etc); if consumed skip hotkeys.
                let consumed = match self.frame_hook.as_mut() {
                    Some(hook) => hook.on_platform_event(&window, event),
                    None => false,
                };

                // Route to InputManager; `false` = user closed window.
                if !self.handle_platform_event(&window, event) {
                    exit = true;
                    break;
                }

                if consumed {
                    continue;
                }

                if let PlatformEvent::Key { key, pressed } = event {
                    if pressed && self.handle_shortcut(key) {
                        continue;
                    }
                }
            }

            if exit || window.close_requested() {
                break;
            }

            // 2. Simulate + extract + submit render (same as winit `about_to_wait`).
            if let Some(message) = self.tick_frame() {
                log::error!("{message}");
                self.fatal_error = Some(message);
                break;
            }

            // 3. Yield CPU: return immediately if message, otherwise wait at most 1 ms.
            window.wait_events(1);
        }

        self.shutdown();

        if self.fatal_error.is_some() {
            let message = self.take_fatal_message();
            let _choice = crate::crash_dialog::show_crash_dialog("Prisma - Fatal Error", &message);
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Render thread lifecycle
    // -----------------------------------------------------------------------

    /// Asynchronously build [`GraphRenderer`] and start frame loop on **render thread**.
    ///
    /// Main thread only holds created platform window. This function computes scene lighting,
    /// Vulkan extensions, overlay factory, create shared state, then spawn render thread ---
    /// renderer built inside render thread (~few hundred ms), so after `resumed` call can immediately
    /// return, window events (close/move/scale) not blocked throughout.
    fn spawn_render_thread(&mut self) {
        // Raw handles must be obtained on **window creation thread** before cross-thread transfer (winit's
        // `window_handle()` fails on other threads); raw HWND is process-level value,
        // cross-thread used to create Vulkan surface is safe (see `SendWindowHandles`).
        let (handles, extensions) = match self.platform.as_ref().map(PlatformContext::window) {
            Some(window) => (
                prism_platform::raw_window_handles(window).expect("get raw window handles"),
                // Vulkan instance extensions (depends on window display handle, cheap).
                required_vulkan_extensions(window),
            ),
            None => {
                log::error!("spawn_render_thread: window not created");
                return;
            }
        };

        // Scene declarative lighting: pure CPU computation, no renderer needed (None → cheap empty IBL).
        let scene_env = self.engine.as_ref().and_then(|engine| {
            engine.current_scene_env_bytes_with_provider(&mut self.asset_resolver.resource_manager)
        });

        // External overlay factory (host UI etc) --- only provide factory, render thread built
        // renderer then call once to produce overlay (GPU resources lazily created at record time).
        let overlay_factory: Option<OverlayFactory> =
            self.frame_hook.as_ref().and_then(|h| h.overlay());

        let vsync = self.config.window.vsync;
        self.spawn_render_thread_with(
            RenderTarget::Surface {
                handles,
                extensions,
                vsync,
            },
            scene_env,
            overlay_factory,
        );
    }

    /// Start render thread --- target (swapchain / offscreen image) decided by caller.
    fn spawn_render_thread_with(
        &mut self,
        target: RenderTarget,
        scene_env: Option<Vec<u8>>,
        overlay_factory: Option<OverlayFactory>,
    ) {
        let (shared, running) = RenderShared::new();
        let shared_clone = shared.clone();
        let shadow_map_size = self.render_settings.shadow_map_size;
        let shadow_cascade_count = self.render_settings.shadow_cascade_count;
        let thread = std::thread::Builder::new()
            .name("render".into())
            .spawn(move || {
                if !prism_platform::thread::favour_current() {
                    log::debug!("render thread keeps the default scheduling priority");
                }
                render_thread_main(
                    shared_clone,
                    target,
                    scene_env,
                    true,
                    overlay_factory,
                    shadow_map_size,
                    shadow_cascade_count,
                )
            })
            .expect("failed to spawn render thread");

        self.render_shared = Some(shared);
        self.render_running = Some(running);
        self.render_thread = Some(thread);
    }

    fn stop_render_thread(&mut self) {
        // Notify render thread to stop.
        if let Some(ref running) = self.render_running {
            running.store(false, Ordering::Relaxed);
        }
        if let Some(shared) = self.render_shared.as_ref() {
            shared.packet_ready.notify_all();
        }

        // Wait for thread to end
        if let Some(handle) = self.render_thread.take() {
            if handle.join().is_err() {
                log::error!("render thread panicked");
            }
        }
        self.render_running = None;
        self.render_shared = None;
    }

    // -----------------------------------------------------------------------
    // Audio decode thread lifecycle (`audio` feature)
    // -----------------------------------------------------------------------

    #[cfg(feature = "audio")]
    fn start_audio_decode_thread(&mut self) {
        if self.audio_decode_thread.is_some() {
            return;
        }
        let (tx, rx) = flume::unbounded();
        let (result_tx, result_rx) = flume::bounded(8);

        let thread = std::thread::Builder::new()
            .name("audio-decode".into())
            .spawn(move || {
                if !prism_platform::thread::put_behind_current() {
                    log::debug!("audio decode thread keeps the default scheduling priority");
                }
                crate::audio_decode_runner::audio_decode_thread_main(rx, result_tx)
            })
            .expect("failed to spawn audio decode thread");

        self.audio_decode_tx = Some(tx);
        self.audio_decode_rx = Some(result_rx);
        self.audio_decode_thread = Some(thread);
    }

    #[cfg(feature = "audio")]
    fn stop_audio_decode_thread(&mut self) {
        if let Some(tx) = self.audio_decode_tx.take() {
            let _ = tx.send(crate::audio_decode_runner::DecodeRequest::Shutdown);
        }
        if let Some(handle) = self.audio_decode_thread.take() {
            let _ = handle.join();
        }
    }

    // -----------------------------------------------------------------------
    // Platform context (window + renderer) lifecycle
    // -----------------------------------------------------------------------
    // Note: window created by main thread `PlatformContext::create_window` (fast),
    // renderer by `spawn_render_thread` → render thread internal construction (async, not blocking events).

    // -----------------------------------------------------------------------
    // Game loop (main thread)
    // -----------------------------------------------------------------------

    /// Single-step simulate + extract + submit render.
    ///
    /// `simulate = false` is editor "edit mode": only propagate hierarchy then directly extract, not run
    /// any game system (camera controller / UI / user logic should not touch scene before clicking Play).
    fn tick_sim(&mut self, simulate: bool) {
        let _sim_span = prism_engine::util::timing::span("app::tick_sim");
        // Poll mode may call this at any event rate. Time is measured once from
        // the monotonic clock; input events must never determine simulation speed.
        let now = Instant::now();
        let elapsed = self
            .last_frame
            .map(|previous| now.saturating_duration_since(previous))
            .unwrap_or(Duration::ZERO)
            .min(Duration::from_millis(100));
        self.fixed_accumulator += elapsed;
        let dt = elapsed.as_secs_f32();
        // Advance the frame clock HERE, at the point `elapsed` was measured:
        // the heavy per-frame work (extract + render submit) happens later in
        // this same call, so stamping the clock at the end of `about_to_wait`
        // made `dt` measure only the idle gap between iterations (≈0.5 µs).
        // That froze time-driven simulation and made its speed depend on how
        // often winit woke the loop (i.e. on mouse movement).
        self.last_frame = Some(now);

        // --- Engine simulation (independent borrow self.engine, released at block end) ---
        // Size must be taken before borrowing engine: `surface_size()` borrows entire `&self`.
        let (surface_w, surface_h) = self.surface_size();
        {
            let Some(engine) = self.engine.as_mut() else {
                return;
            };

            // --- Sync screen size (ECS UI layout depends on ScreenSize resource) ---
            //
            // Layout still computed by window pixels; when `internal_resolution` effective UI and scene together
            // drawn in PostPass target rectangle, so tell engine again "window → layout space"
            // conversion, pointer hit test won't be off (#988).
            if surface_w > 0 && surface_h > 0 {
                let mapping = ui_layout_mapping(
                    surface_w,
                    surface_h,
                    self.render_settings.internal_resolution,
                    self.render_settings.internal_scale_mode,
                );
                let world = engine.world_mut();
                world.insert_resource(prism_engine::ui::ScreenSize::new(surface_w, surface_h));
                world.insert_resource(mapping);
            }

            if simulate {
                // Pointer lock has single truth (platform window), copied into InputManager each frame:
                // grab rejected by system / Esc unlock / focus loss release all change window state, after alignment here
                // camera (engine) and panel (project hook) read boolean values won't each say their own thing.
                if let Some(platform) = self.platform.as_ref() {
                    self.input
                        .sync_pointer_lock(platform.window().is_pointer_locked());
                }
                // --- Fixed timestep: integer nanosecond clock, capped catch-up ---
                const FIXED_STEP: Duration = Duration::from_nanos(16_666_667);
                engine.begin_frame(&self.input, [surface_w as f32, surface_h as f32]);
                let mut fixed_steps = 0;
                while self.fixed_accumulator >= FIXED_STEP && fixed_steps < 8 {
                    // Physics uses fixed step (#522): result in below `engine.update()` by
                    // `hierarchy_system` propagated to `LocalToWorld`, render same frame can see it.
                    #[cfg(feature = "physics")]
                    if let Some(physics) = self.physics.as_mut() {
                        physics.step(engine.world_mut());
                    }
                    engine.fixed_update(FIXED_STEP.as_secs_f32(), &self.input);
                    self.fixed_accumulator -= FIXED_STEP;
                    fixed_steps += 1;
                }

                engine.set_interpolation_alpha(
                    self.fixed_accumulator.as_secs_f32() / FIXED_STEP.as_secs_f32(),
                );

                // --- Variable time step update ---
                engine.update(dt, &self.input);

                // --- Late update ---
                engine.late_update();
            } else {
                // Edit mode: does not consume fixed accumulator, avoids catching up big chunk when switching to Play.
                self.fixed_accumulator = Duration::ZERO;
                prism_engine::scene::systems::hierarchy::hierarchy_system(engine.world_mut());
                prism_engine::scene::systems::hierarchy::active_hierarchy_system(
                    engine.world_mut(),
                );
            }

            // --- Audio update (`audio` feature) ---
            #[cfg(feature = "audio")]
            {
                if let Some(audio) = self.audio.as_mut() {
                    audio.update();
                }

                // Process audio decode results: hand to scene audio cache (no longer auto-play,
                // playback decided by `AudioSource` component).
                if let Some(rx) = self.audio_decode_rx.as_ref() {
                    while let Ok(result) = rx.try_recv() {
                        if let crate::audio_decode_runner::DecodeResult::Error { message, .. } =
                            &result
                        {
                            log::warn!("Audio decode error: {message}");
                        }
                        self.scene_audio.receive(result);
                    }
                }

                // Scene audio: ECS `AudioSource` → mixer (#508).
                if let Some(audio) = self.audio.as_mut() {
                    let mut sink = (audio, &mut self.audio_voices);
                    self.scene_audio.update(
                        engine.world(),
                        &mut sink,
                        self.audio_decode_tx.as_ref(),
                    );
                }
                // One-shot sounds (#987): project code queues, here actually play.
                crate::one_shot::drain(
                    engine.world_mut(),
                    self.audio.as_mut(),
                    &mut self.one_shot_sounds,
                    &mut self.asset_resolver.resource_manager,
                );
            }
        } // engine borrow ends here

        // Window events arrive after the previous tick, so transient input must
        // be cleared only after this frame's gameplay / UI have read the
        // snapshot. Otherwise a fresh Space / mouse press edge is dropped
        // before the ECS sees it. Held state stays valid across frames.
        self.input.begin_frame();

        // --- Dev hot reload (#495): after rebuilding `.pak` no need to exit and re-enter ---
        self.poll_asset_changes();

        // --- Asset resolution: main thread CPU segment prepares request → channel → render thread GPU segment ---
        self.pump_asset_requests();

        // --- Extract frame packet → send to render thread (render enabled only) ---
        if let (Some(shared), Some(engine)) = (self.render_shared.as_ref(), self.engine.as_mut()) {
            // Reuse data packet returned by render thread last frame (`Vec` capacity retained, #66).
            let mut packet = shared.take_spare_packet().unwrap_or_default();
            // #613: when internal resolution effective camera projects by **internal** aspect ratio, post-process then integer
            // upscale to window; otherwise 2D pixels stretched into unequal-width rectangles.
            let aspect = self.render_settings.render_aspect(self.display_aspect);
            // #978: subpixel polyline converts "1 pixel" world length by scene actual render height.
            let render_height = self.render_settings.render_height(surface_w, surface_h);
            let mut extract_span = prism_engine::util::timing::span("app::extract_frame_packet");
            extract_frame_packet_into_with_shadow_cascades_and_render_height(
                engine.world_mut(),
                aspect,
                &self.surface_rotation,
                self.render_settings.shadow_distance,
                self.render_settings.shadow_cascade_count,
                render_height,
                &mut packet,
            );
            // Render settings sent to render thread with frame packet (hotkey / project config changes take effect here).
            packet.settings = self.render_settings.clone();
            // Local post-processing volume (#502 Phase 2): parse this frame PostGrade by main camera position.
            packet.grade = Some(
                prism_engine::scene::systems::post_process::resolve_post_grade(
                    engine.world(),
                    packet.scene.eye,
                    packet.settings.post_grade(),
                ),
            );

            // Runtime diagnostics HUD: appended to ECS UI overlay (then uniformly drawn by ui_overlay).
            #[cfg(feature = "engine-debug")]
            if self.debug_hud.show {
                let (width, height) = self.surface_size();
                let (width, height) = (width as f32, height as f32);
                let stats = shared.read_render_stats();
                self.debug_hud.append(
                    &mut packet.ui_overlay,
                    &stats,
                    &self.render_settings,
                    packet.scene.has_camera,
                    width,
                    height,
                );
            }

            extract_span.detail(format!(
                "draws={} sprites={} ui_quads={}",
                packet.draw_items.len(),
                packet.sprite_items.len(),
                packet.ui_overlay.quads.len()
            ));
            shared.send_packet(packet);
        }
    }

    /// Drive cross-thread asset resolution every frame:
    /// 1. **CPU segment** (main thread): scan pending-parse `MeshRenderer` entities, load `.pak`
    ///    + deinterleave vertex/texture pixels, produce pure data upload requests enqueued (not touching renderer).
    /// 2. **Result write-back** (main thread): take handles returned by render thread GPU segment, write into
    ///    `MeshRef`/`MaterialRef` (`generation = 1`).
    ///
    /// Renderer asynchronously built on render thread, so this function safely called every frame before first frame ---
    /// requests wait in channel until renderer ready.
    fn pump_asset_requests(&mut self) {
        let _span = prism_engine::util::timing::span("app::pump_asset_requests");
        let (Some(shared), Some(engine)) = (self.render_shared.as_ref(), self.engine.as_mut())
        else {
            return;
        };

        // Project bundled font (#583): main thread reads bytes, render thread registers it into UI atlas.
        let fonts = self.asset_resolver.prepare_ui_fonts(engine.world());
        for (name, bytes) in fonts {
            shared.send_renderer_message(Box::new(move |renderer| {
                renderer.register_ui_font(name, bytes);
            }));
        }

        // Upload UI RGBA8 icon textures (e.g. rasterized SVGs) into bindless slots.
        if let Some(mut pending) = engine
            .world_mut()
            .remove_resource::<prism_engine::ui::PendingUiImageUploads>()
        {
            if !pending.items.is_empty() {
                let uploaded = engine
                    .world()
                    .get_resource::<prism_engine::ui::UploadedUiImages>()
                    .map(|u| u.slots.clone())
                    .unwrap_or_else(|| {
                        let u = prism_engine::ui::UploadedUiImages::default();
                        let slots = u.slots.clone();
                        engine.world_mut().insert_resource(u);
                        slots
                    });
                let items = std::mem::take(&mut pending.items);
                shared.send_renderer_message(Box::new(move |renderer| {
                    for item in items {
                        let input = prism_render::managers::TextureUploadInput {
                            width: item.width,
                            height: item.height,
                            format: prism_render::managers::TextureFormat::Rgba8,
                            mip_levels: 1,
                            pixels: item.pixels,
                        };
                        if let Ok(handle) = renderer.register_texture(&input) {
                            let slot = renderer.texture_srv(handle).0;
                            if let Ok(mut guard) = uploaded.lock() {
                                guard.insert(item.key, slot);
                            }
                        }
                    }
                }));
            }
        }

        // Skeletal animation assets (#560 P0-2): idempotent, only read disk when new `.rskel` / `.ranim` appears.
        self.asset_resolver
            .load_animation_assets(engine.world_mut());

        // CPU segment: scan pending-parse entities and produce upload requests (pure data, not touching renderer).
        let reqs = self.asset_resolver.prepare_requests(engine.world_mut());
        if !reqs.is_empty() {
            shared.enqueue_asset_requests(reqs);
        }

        // Invalid GPU resources (entity destroyed / hot reload / path rewrite) truly released on render thread (#495).
        let releases = self.asset_resolver.take_releases();
        if !releases.is_empty() {
            shared.send_renderer_message(Box::new(move |renderer| {
                renderer.release_assets(&releases);
            }));
        }

        // GPU segment result write-back: render thread returns handles, main thread writes into components.
        let results = shared.take_asset_results();
        if !results.is_empty() {
            self.asset_resolver
                .apply_results(engine.world_mut(), &results);
        }
    }

    /// Dev asset hot reload: after `.pak` rebuild swap reader and let entities re-parse (#495).
    ///
    /// Only effective when [`spawn_pak_watcher`] built polling thread (`PRISMA_HOT_RELOAD`
    /// enabled). Same as editor three steps: `on_pak_changed` swap file bytes → reload path list
    /// → `invalidate_assets` marks entities back to pending-parse, next frame `pump_asset_requests`
    /// will re-upload.
    ///
    /// Known cost: renderer has no "release by handle" interface, replaced old texture / mesh slots will always
    /// occupy, so this path only used for dev time.
    fn poll_asset_changes(&mut self) {
        let Some(watcher) = self.pak_watcher.as_ref() else {
            return;
        };
        // First drain events then change state: `receiver()` borrows watcher, processing events borrows self.
        let mut changed: Vec<std::path::PathBuf> = Vec::new();
        while let Ok(event) = watcher.receiver().try_recv() {
            match event {
                prism_engine::scene::hot_reload::HotReloadEvent::PakModified(path) => {
                    if !changed.contains(&path) {
                        changed.push(path);
                    }
                }
                prism_engine::scene::hot_reload::HotReloadEvent::WatchError(error) => {
                    log::warn!("hot reload: {error}");
                }
            }
        }
        if changed.is_empty() {
            return;
        }

        let Some(engine) = self.engine.as_mut() else {
            return;
        };
        let mut reloaded = 0usize;
        for pak in &changed {
            if let Err(error) = self.asset_resolver.resource_manager.on_pak_changed(pak) {
                log::warn!("hot reload: reopening {} failed: {error}", pak.display());
                continue;
            }
            let manifest = pak.with_extension("pak.meta.json");
            if manifest.is_file() {
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
            // Also verify build stamp once after repack (#976): when package and document don't match immediately prompt,
            // instead of waiting for player to find screen wrong.
            self.asset_resolver.check_cook_stamp(pak);
            reloaded += 1;
        }
        if reloaded == 0 {
            return;
        }
        let count = self.asset_resolver.invalidate_assets(engine.world_mut());
        log::info!(
            "hot reload: {count} entity/entities re-resolved after {reloaded} package(s) were rebuilt"
        );
    }

    // -----------------------------------------------------------------------
    // PBR debug helpers
    // -----------------------------------------------------------------------

    /// Route a neutral platform event to [`InputManager`] and window state.
    ///
    /// Returns `false` meaning app should exit (user closed window). Both window backends share this one,
    /// so winit and Win32 input semantics won't each be written separately then slowly drift.
    fn handle_platform_event(&mut self, window: &PlatformWindow, event: PlatformEvent) -> bool {
        use prism_engine::input::{ElementState, MouseButton};

        match event {
            PlatformEvent::CloseRequested => return false,
            PlatformEvent::Focused(focused) => {
                // A host that owns the cursor gets no grab/release from us.
                if self.pointer_lock == PointerLockPolicy::Manual {
                    return true;
                }
                if !focused && self.input.pointer_locked {
                    prism_platform::release_pointer(window);
                    self.input.set_locked(false);
                }
                // Regaining focus re-grabs: pointer lock is click-to-enter, so
                // alt-tabbing out and back would otherwise leave the player with
                // dead movement keys until they click again. Esc stays the only
                // way out (see `should_release_pointer_lock`).
                if focused && !self.input.pointer_locked {
                    // Trust the platform result: if the OS refused the grab, do
                    // not record "locked", or the cursor stays visible and free
                    // while the camera keeps eating WASD and mouse deltas.
                    let grabbed = prism_platform::grab_pointer(window);
                    self.input.set_locked(grabbed);
                }
            }
            PlatformEvent::CursorMoved { x, y } => {
                apply_window_cursor_move(&mut self.input, x, y);
            }
            // Raw device increments in locked state (winit: `DeviceEvent::MouseMotion`,
            // Win32: re-center mouse capture offset).
            PlatformEvent::MouseMotion { dx, dy } => {
                apply_device_mouse_motion(&mut self.input, dx, dy);
            }
            PlatformEvent::MouseWheel { delta } => self.input.handle_scroll(delta),
            PlatformEvent::Touch { id, x, y, phase } => {
                // Touch does not go through mouse button simulation: multi-touch must retain touch id and phase,
                // project module via `InputSnapshot::touches` (host fills table, at most
                // `InputSnapshot::MAX_TOUCHES` points) self-interpreted, instead of being compressed to left button.
                self.input
                    .handle_touch(id, engine_touch_phase(phase), [x, y]);
            }
            PlatformEvent::MouseButton { button, pressed } => {
                let button = match button {
                    0 => MouseButton::Left,
                    1 => MouseButton::Right,
                    2 => MouseButton::Middle,
                    3 => MouseButton::Back,
                    4 => MouseButton::Forward,
                    other => MouseButton::Other(other),
                };
                if pressed
                    && button == MouseButton::Left
                    && !self.input.pointer_locked
                    && self.pointer_lock == PointerLockPolicy::Auto
                {
                    let grabbed = prism_platform::grab_pointer(window);
                    self.input.set_locked(grabbed);
                }
                self.input.handle_mouse_button(
                    button,
                    if pressed {
                        ElementState::Pressed
                    } else {
                        ElementState::Released
                    },
                );
            }
            PlatformEvent::Key { key, pressed } => {
                // Key code conversion + Esc unlock + write to InputManager collected in `apply_key_event`,
                // only unit tests can cover "platform key → engine key → key_held" entire chain;
                // only real window-touching release_pointer left outside.
                if apply_key_event(&mut self.input, key, pressed)
                    && self.pointer_lock == PointerLockPolicy::Auto
                {
                    prism_platform::release_pointer(window);
                }
            }
            PlatformEvent::Modifiers(modifiers) => self.modifiers = modifiers,
            // Size change also updates aspect ratio (render thread reads real size from platform context).
            PlatformEvent::Resized { width, height } => {
                self.needs_resize = true;
                if width > 0 && height > 0 {
                    self.display_aspect = width as f32 / height as f32;
                }
            }
            // Unmodeled events: keep explicitly listed, avoid `_ => {}` silently swallowing new variants
            // (`PlatformEvent::Touch` is how entire touch chain was lost).
            PlatformEvent::Other => {}
        }

        true
    }

    /// App-level keyboard hotkeys (F3 / Tab / T) + PBR component isolation.
    ///
    /// Only called when key **pressed**; returns `true` meaning this key consumed.
    fn handle_shortcut(&mut self, key: Key) -> bool {
        // Host hook **not called here**: two event loop entries (platform event pump and winit
        // `window_event`) both already send same key event to hook before calling this function,
        // if consumed skip this function directly. Converting again here equals same key pressed twice --- project side
        // of "press once toggle once" will self-cancel (F1 panel never opens).
        //
        // Binding lookup and "number key + Shift = another PBR component" both decided by engine's
        // `KeyBindings` decide; host here only responsible for applying action to `RenderSettings`.
        // Unbound keys all return false: not consumed, project input available as-is.
        // When debug layer not compiled table is empty: F3 / Tab / T / number keys all left as-is for project.
        let shift = self.modifiers.shift;
        let key_bindings = if cfg!(feature = "engine-debug") {
            prism_engine::KeyBindings::engine_debug(shift)
        } else {
            prism_engine::KeyBindings::empty()
        };
        let Some(action) = key_bindings.action_for(prism_engine::key_code_from_vk(key.to_vk()))
        else {
            return false;
        };

        if let prism_engine::EngineAction::IsolatePbrComponent(bit) = action {
            self.render_settings.debug_flags = if self.render_settings.debug_flags == (1u32 << bit)
            {
                0
            } else {
                1u32 << bit
            };
            log::info!(
                "debug view = {} (flags=0x{:x})",
                self.pbr_flag_label(),
                self.render_settings.debug_flags
            );
            return true;
        }

        match action {
            prism_engine::EngineAction::ToggleHud => {
                #[cfg(feature = "engine-debug")]
                {
                    self.debug_hud.show = !self.debug_hud.show;
                    log::info!(
                        "runtime HUD = {}",
                        if self.debug_hud.show { "on" } else { "off" }
                    );
                }
            }
            prism_engine::EngineAction::CycleDebugTarget => {
                self.render_settings.debug_rt = (self.render_settings.debug_rt + 1) % 3;
                let name = match self.render_settings.debug_rt {
                    0 => "normal (HDR tonemap)",
                    1 => "depth (linearized)",
                    2 => "normal (view-space)",
                    _ => "?",
                };
                log::info!("debug RT = {} ({})", self.render_settings.debug_rt, name);
            }
            prism_engine::EngineAction::CycleTonemap => {
                // 0 = Reinhard, 1 = ACES, 2 = passthrough (2D / pixel art).
                self.render_settings.tonemap_mode = (self.render_settings.tonemap_mode + 1) % 3;
                log::info!("tonemap mode = {}", self.render_settings.tonemap_mode);
            }
            // Already handled above, here just writing branches complete.
            prism_engine::EngineAction::IsolatePbrComponent(_) => {}
        }
        true
    }

    fn pbr_flag_label(&self) -> String {
        prism_engine::render_settings::pbr_flag_label(self.render_settings.debug_flags).to_string()
    }

    #[cfg(feature = "winit-backend")]
    fn show_fatal_dialog(&mut self, event_loop: &ActiveEventLoop) {
        let message = self.take_fatal_message();
        let _choice = crate::crash_dialog::show_crash_dialog("Prisma - Fatal Error", &message);
        event_loop.exit();
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// ApplicationHandler
// ===========================================================================

#[cfg(feature = "winit-backend")]
impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // Drive simulation/extraction continuously. With the default Wait mode
        // winit only re-enters `about_to_wait` after input/window events, which
        // freezes animation when the user is idle.
        event_loop.set_control_flow(ControlFlow::Poll);

        // Reset suspend flag (must precede any subsystem gate --- audio resume doesn't depend on Render).
        self.suspended = false;
        self.resume_subsystems();

        if !self.has_subsystem(Subsystem::Render) {
            log::info!("render subsystem disabled — headless mode (no window, no GPU)");
            return;
        }

        let resumed_entry = crate::render_shared::startup_ms();

        if self.platform.is_none() {
            // Main thread only creates window (fast, ~few ms), then immediately returns --- window events
            // (close/move/scale) from now can be dispatched by winit, not blocked by any init.
            let window_config = self.window_config();
            let window_span = prism_engine::util::timing::span("app::create_window");
            // Create hidden: the first frame is hundreds of milliseconds away
            // (renderer build + pipeline/font warmup), and an empty window in
            // that gap reads as a hang. `tick_frame` shows it once a frame has
            // actually been presented.
            let mut hidden_config = window_config.clone();
            hidden_config.visible = false;
            let platform = PlatformContext::create_window(event_loop, &hidden_config);
            self.pending_show = window_config.visible;
            let window_built = crate::render_shared::startup_ms();
            drop(window_span);

            self.platform = Some(platform);

            // Render thread asynchronously builds renderer + starts frame loop (heavy init doesn't block main thread).
            let spawn_span = prism_engine::util::timing::span("app::spawn_render_thread");
            self.spawn_render_thread();
            drop(spawn_span);

            // Record startup milestone: resumed return time = event_loop_free (event loop unlocked).
            let spawned = crate::render_shared::startup_ms();
            if let Some(shared) = self.render_shared.as_ref() {
                shared.set_mark("resumed_entry", resumed_entry);
                shared.set_mark("window_built", window_built);
                shared.set_mark("render_thread_spawned", spawned);
            }
            return;
        }

        // After Android suspend rebuild surface: this design already stopped render thread at suspended and
        // dropped window, so here re-walk above "first resume" branch. Reserved log only.
        log::info!("resumed (surface already active)");
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        if matches!(event, WindowEvent::CloseRequested) {
            event_loop.exit();
            return;
        }
        if self.fatal_error.is_some() {
            self.show_fatal_dialog(event_loop);
            return;
        }

        let platform_event = prism_platform::to_platform_event(&event);

        // First forward events to frame hook (host UI etc); if consumed no longer walk app
        // own hotkey handling (input still routed to InputManager to keep key state consistent).
        let consumed = match (self.frame_hook.as_mut(), self.platform.as_ref()) {
            (Some(hook), Some(platform)) => {
                let window = platform.window();
                hook.on_platform_event(window, platform_event)
                    || hook.on_window_event(window, &event)
            }
            _ => false,
        };

        // Route window events → InputManager (always route, including hotkey-unhandled repeat events).
        if let Some(window) = self.platform.as_ref().map(PlatformContext::window_arc) {
            if !self.handle_platform_event(&window, platform_event) {
                event_loop.exit();
                return;
            }
        }

        if consumed {
            return;
        }

        // Modifier key state only updates with `ModifiersChanged` (route already written), event itself needs no handling.
        if matches!(event, WindowEvent::ModifiersChanged(_)) {
            return;
        }

        // Keyboard hotkeys (triggered on press only). Here directly reuse already computed above
        // `platform_event`, no longer re-parse keys from `WindowEvent`: two paths each compute once
        // then each uses different conversion (`Key::from_winit` vs `winit_to_vk`), same key under
        // "platform event pump" and "winit event" becomes two different `Key`.
        if let PlatformEvent::Key { key, pressed: true } = platform_event {
            self.handle_shortcut(key);
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        // In pointer lock mode, mouse movement arrives as raw device increments.
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            apply_device_mouse_motion(&mut self.input, dx, dy);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if event_loop.exiting() {
            self.shutdown();
            return;
        }

        // Game loop tick (main thread) --- background suspend pauses simulate and returns early (T4):
        // not feeding dt → simulate frozen → background process CPU zeroed, and audio decode already
        // stopped with suspended, no queue idle spin.
        if self.suspended {
            return;
        }

        if let Some(message) = self.tick_frame() {
            log::error!("{message}");
            self.fatal_error = Some(message);
            self.show_fatal_dialog(event_loop);
        }
    }

    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        // Background no longer needs per-frame polling; `resumed()` will restore `Poll`. Otherwise winit at
        // Android background still keeps spinning idle, CPU usage stays 100%.
        event_loop.set_control_flow(ControlFlow::Wait);

        // Set suspend flag (must precede any subsystem gate --- audio suspend doesn't depend on Render).
        self.suspended = true;

        // --- Audio subsystem: return audio device (T4). drop cpal stream → stop callback thread;
        // Firewheel graph and active playback nodes retained, on resume re-registered with pre-suspend config and resume playback. ---
        #[cfg(feature = "audio")]
        if self.has_subsystem(Subsystem::Audio) {
            if let Some(audio) = self.audio.as_mut() {
                audio.suspend_stream();
            }
        }

        if !self.has_subsystem(Subsystem::Render) {
            return;
        }

        // Stop background thread.
        #[cfg(feature = "audio")]
        self.stop_audio_decode_thread();

        // Stop render thread (renderer owned by render thread, stop releases Vulkan surface).
        self.stop_render_thread();

        self.platform = None;
    }
}
