//! User project module for Game View: compile, hot-reload, per-frame tick.
//!
//! Editor itself only links engine; user project gameplay code compiles into **separate cdylib**
//! (`preview-module` feature in `projects/<p>/Cargo.toml`), loaded by
//! `prism-runtime-host` via stable C ABI. Project module doesn't link engine,
//! so no second `World` / `renderer`.
//!
//! Compile on background thread, load / unload must on UI thread (`ProjectSession` callback pointers
//! cannot cross threads). Pacing follows **module compilation** (no script layer, so no domain reload):
//!
//! - Source changes **settle** 350ms before compiling, consecutive saves won't trigger multiple cargos;
//! - During Play won't auto-swap module, avoid interrupting playtest; `Ctrl+R` explicitly refreshes instance;
//! - Swapping module is **whole-module reload**: old session dropped, `create` rerun, project static state zeroed;
//! - Compile failure keeps previous successful module running, diagnostics go to Console and Problems panel;
//! - Build artifact first copied to unique name then loaded (Windows can't overwrite already-loaded dll).

use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime};

use prism_editor_core::LogLevel;
use prism_engine::input::InputManager;

use crate::diagnostics::{diagnostics_from_cargo, Diagnostic};
use prism_runtime_host::{
    AssetIo, InputSnapshot, ProjectModule, ProjectSession, RaycastResult, SceneRequest,
    UiQuad as HostUiQuad, WorldCommands,
};
use prism_scene::ComponentRegistry;

/// How long to wait after source change before compiling (engine-style debounce).
const COMPILE_DEBOUNCE: Duration = Duration::from_millis(350);
/// Can point directly to precompiled module (tests / non-Cargo projects).
const MODULE_ENV: &str = "PRISMA_PROJECT_MODULE";

/// One project module event to write into Console.
#[derive(Clone, Debug)]
pub struct ProjectEvent {
    pub level: LogLevel,
    pub text: String,
}

/// Module compile state. Shared by status bar, Problems panel and MCP `editor_status`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleState {
    /// Project has no `preview-module` (Play only runs engine systems).
    Unavailable,
    /// Loaded, source has no uncompiled changes.
    Ready,
    /// Source has changes, not yet compiled (in debounce, or Auto Refresh off).
    Pending,
    /// Cargo currently running.
    Compiling,
    /// Last compile / load failed (won't auto-retry until source changes again).
    Failed,
}

impl ModuleState {
    pub fn as_str(self) -> &'static str {
        match self {
            ModuleState::Unavailable => "unavailable",
            ModuleState::Ready => "ready",
            ModuleState::Pending => "pending",
            ModuleState::Compiling => "compiling",
            ModuleState::Failed => "failed",
        }
    }
}

/// Pollable status (status bar / Problems / MCP `editor_status`).
#[derive(Clone, Debug)]
pub struct ProjectModuleStatus {
    pub available: bool,
    pub loaded: bool,
    pub building: bool,
    pub auto_refresh: bool,
    pub state: ModuleState,
    /// Source mtime later than last compile attempt: has uncompiled changes.
    pub dirty: bool,
    pub manifest: Option<PathBuf>,
    pub error: Option<String>,
    /// Number of diagnostics from most recent build (details via `ProjectModuleDriver::diagnostics`).
    pub problems: usize,
}

enum BuildOutcome {
    Built {
        path: PathBuf,
        diagnostics: Vec<Diagnostic>,
    },
    Failed {
        message: String,
        diagnostics: Vec<Diagnostic>,
    },
}

/// Only scan source mtimes when no compile task; offset 350ms debounce, don't stat every frame.
const DIRTY_SCAN_INTERVAL: Duration = Duration::from_millis(500);

/// Project save file (#984): editor Play saves go under project directory, don't touch real user data.
///
/// Player side uses `prism_engine::user_data` (`%APPDATA%/Prisma/<project>` style),
/// editor here fixes `<project>/target/play-data/player_data.json` — deleting
/// `target/` equals clearing Play save, won't pollute player's real progress.
pub struct ProjectSave {
    store: prism_engine::user_data::UserStore,
}

impl ProjectSave {
    /// Returns `None` when save directory can't be created (feature not declared).
    fn new(root: &Path) -> Option<Self> {
        let dir = root.join("target").join("play-data");
        if let Err(error) = std::fs::create_dir_all(&dir) {
            log::warn!("project save unavailable ({}): {error}", dir.display());
            return None;
        }
        Some(Self {
            store: prism_engine::user_data::UserStore::open(
                dir.join(prism_engine::user_data::FILE_NAME),
            ),
        })
    }
}

impl prism_runtime_host::SaveIo for ProjectSave {
    fn save_value(&mut self, key: &str, value: &[u8]) -> bool {
        let Ok(value) = std::str::from_utf8(value) else {
            log::warn!("project save '{key}': value is not UTF-8");
            return false;
        };
        self.store.set(key, value);
        match self.store.flush() {
            Ok(_) => true,
            Err(error) => {
                log::warn!("project save '{key}': {error}");
                false
            }
        }
    }

    fn load_value(&mut self, key: &str) -> Option<Vec<u8>> {
        self.store.get(key).map(|value| value.as_bytes().to_vec())
    }
}

/// Host-side driver of project module.
/// Mixer for one-shot project audio (#987).
///
/// In editor Play, module and host are **two** engine instances, module can't get `App`'s audio device,
/// so here gives driver its own `AudioEngine`: module calls `GameWorld::play_sound` → ABI
/// callback → here decode + play. Voice ids allocated here (from 1, `0` means failure).
pub struct ProjectAudio {
    engine: prism_audio::AudioEngine,
    voices: std::collections::HashMap<u32, ProjectVoice>,
    next_id: u32,
}

struct ProjectVoice {
    handle: prism_audio::PlaybackHandle,
    /// Keep segment for `repeat` (`AudioEngine` has no loop flag, replay after finishing).
    clip: prism_audio::AudioData,
    volume: f32,
    repeat: bool,
}

impl ProjectAudio {
    /// Returns `None` when no sound card / audio init fails (feature not declared).
    fn new() -> Option<Self> {
        match prism_audio::AudioEngine::new(prism_audio::AudioConfig::default()) {
            Ok(engine) => Some(Self {
                engine,
                voices: std::collections::HashMap::new(),
                next_id: 0,
            }),
            Err(error) => {
                log::warn!("project module audio unavailable: {error}");
                None
            }
        }
    }

    /// Advance mixer each frame and release loop sounds.
    fn update(&mut self) {
        self.engine.update();
        for voice in self.voices.values_mut() {
            if !voice.repeat || self.engine.is_playing(&voice.handle) {
                continue;
            }
            let handle = self.engine.play(&voice.clip);
            if handle.is_valid() {
                self.engine.set_volume(&handle, voice.volume);
                voice.handle = handle;
            }
        }
        // Release voices that finished and aren't looping, avoid id table growing unbounded.
        self.voices
            .retain(|_, voice| voice.repeat || self.engine.is_playing(&voice.handle));
    }
}

impl prism_runtime_host::AudioIo for ProjectAudio {
    fn play_sound(&mut self, path: &str, volume: f32, repeat: bool) -> u32 {
        let path = path.to_string();
        let Some(file) = prism_engine::asset_paths::asset_path(&path) else {
            log::warn!("project audio: '{path}' has no asset root");
            return 0;
        };
        if !file.is_file() {
            log::warn!("project audio: '{path}' not found");
            return 0;
        }
        let clip = match prism_audio::decoder::decode_file(&file) {
            Ok(clip) => clip,
            Err(error) => {
                log::warn!("project audio: decode '{path}' failed: {error}");
                return 0;
            }
        };
        let handle = self.engine.play(&clip);
        if !handle.is_valid() {
            return 0;
        }
        let volume = volume.clamp(0.0, 1.0);
        self.engine.set_volume(&handle, volume);
        log::debug!(
            "project audio: '{path}' started as id {} (volume {volume:.2}, repeat {repeat})",
            self.next_id.wrapping_add(1).max(1)
        );
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let id = self.next_id;
        self.voices.insert(
            id,
            ProjectVoice {
                handle,
                clip,
                volume,
                repeat,
            },
        );
        id
    }

    fn stop_sound(&mut self, id: u32) {
        if let Some(voice) = self.voices.remove(&id) {
            self.engine.stop(&voice.handle);
        }
    }
}

// Note: `ProjectAudio` must be created when **constructing** driver, cannot create in module tick.
// `AudioEngine::new` inside tick (opens cpal stream) would freeze editor — world is
// borrowed by module, once audio callback thread starts UI thread can't get control back. Create once at construction is safest.
pub struct ProjectModuleDriver {
    manifest: Option<PathBuf>,
    target_dir: Option<PathBuf>,
    components: ComponentRegistry,
    assets: ProjectAssets,
    session: Option<ProjectSession>,
    /// Current loaded copy path (build artifact will be overwritten by next cargo).
    staged: Option<PathBuf>,
    /// Source mtime at most recent compile attempt (success / failure both recorded, failure won't retry until change).
    compiled_stamp: Option<SystemTime>,
    /// Source changed after `compiled_stamp`: status bar shows "uncompiled changes".
    dirty: bool,
    /// `dirty` scan interval (`DIRTY_SCAN_INTERVAL`).
    last_scan: Option<Instant>,
    /// Compiler diagnostics from most recent build (warnings only on success).
    diagnostics: Vec<Diagnostic>,
    /// Moment change first observed, for debounce.
    pending_since: Option<Instant>,
    build: Option<Receiver<BuildOutcome>>,
    building: bool,
    /// `Ctrl+R`: next `maybe_compile` forces rebuild, ignoring Auto Refresh / Play.
    refresh_requested: bool,
    auto_refresh: bool,
    /// Whether project provides `preview-module` (or prebuilt module given).
    available: bool,
    /// Last tick judged module faulty (panic / returned error): pause calls, wait for reload.
    faulted: bool,
    /// Prebuilt module specified by `PRISMA_PROJECT_MODULE` (only tried once).
    prebuilt: Option<PathBuf>,
    events: Vec<ProjectEvent>,
    /// One-shot project audio (#987): editor Play module hands sounds here via ABI callback.
    audio: Option<ProjectAudio>,
    last_error: Option<String>,
    last_tick_error: Option<String>,
    /// Scene change requested by project code on one tick (#482), taken by host at tick end.
    pending_scene: Option<SceneRequest>,
    /// Project save file (#984): Play save under `<project>/target/play-data/`.
    save: Option<ProjectSave>,
    /// Key edges captured once per render frame and consumed by the next fixed step.
    pending_fixed_key_edges: [u64; 8],
    interpolation_alpha: f32,
}

impl ProjectModuleDriver {
    pub fn new(root: &Path) -> Self {
        let manifest = crate::build::game_manifest_path(root);
        let manifest = manifest.is_file().then_some(manifest);
        let available_feature = manifest
            .as_deref()
            .is_some_and(manifest_declares_preview_module);
        let target_dir = manifest.as_deref().and_then(|manifest| {
            manifest
                .parent()
                .map(|dir| dir.join("target").join("preview-module"))
        });

        let prebuilt = std::env::var_os(MODULE_ENV)
            .map(PathBuf::from)
            .filter(|path| path.is_file());

        let available = prebuilt.is_some() || available_feature;
        let mut driver = Self {
            manifest,
            target_dir,
            components: builtin_component_registry(),
            assets: ProjectAssets {
                root: root.to_path_buf(),
            },
            session: None,
            staged: None,
            compiled_stamp: None,
            dirty: available,
            last_scan: None,
            diagnostics: Vec::new(),
            pending_since: None,
            build: None,
            building: false,
            refresh_requested: false,
            auto_refresh: true,
            available,
            faulted: false,
            prebuilt,
            events: Vec::new(),
            audio: ProjectAudio::new(),
            last_error: None,
            last_tick_error: None,
            pending_scene: None,
            save: ProjectSave::new(root),
            pending_fixed_key_edges: [0; 8],
            interpolation_alpha: 0.0,
        };

        if driver.prebuilt.is_some() {
            driver.push(
                LogLevel::Info,
                format!("project module: using {MODULE_ENV}"),
            );
        } else if driver.manifest.is_none() {
            driver.push(
                LogLevel::Info,
                "project module: no Cargo.toml in project root; Play runs the engine only"
                    .to_string(),
            );
        } else if !available_feature {
            driver.push(
                LogLevel::Warn,
                "project module: Cargo.toml has no `preview-module` feature; \
                 Play runs the engine only"
                    .to_string(),
            );
        }

        driver
    }

    pub fn status(&self) -> ProjectModuleStatus {
        ProjectModuleStatus {
            available: self.available(),
            loaded: self.session.is_some(),
            building: self.building,
            auto_refresh: self.auto_refresh,
            state: self.state(),
            dirty: self.dirty,
            manifest: self.manifest.clone(),
            error: self.last_error.clone(),
            problems: self.diagnostics.len(),
        }
    }

    /// Problems panel reads this (not cloned every frame via `status()`).
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Captures frame input before the host runs zero or more fixed steps.
    /// Held keys remain in every step; edge bits are consumed by the first one.
    pub fn begin_frame(&mut self, input: &InputManager, screen: [f32; 2], alpha: f32) {
        let snapshot = input_snapshot(input, screen);
        for (pending, current) in self
            .pending_fixed_key_edges
            .iter_mut()
            .zip(snapshot.key_edges)
        {
            *pending |= current;
        }
        self.interpolation_alpha = if alpha.is_finite() {
            alpha.clamp(0.0, 1.0)
        } else {
            0.0
        };
    }

    pub fn set_interpolation_alpha(&mut self, alpha: f32) {
        self.interpolation_alpha = if alpha.is_finite() {
            alpha.clamp(0.0, 1.0)
        } else {
            0.0
        };
    }

    fn state(&self) -> ModuleState {
        if !self.available() {
            return ModuleState::Unavailable;
        }
        if self.building {
            return ModuleState::Compiling;
        }
        // Source changed but not compiled (when Auto Refresh off hint user press Ctrl+R);
        // only stable Failed if source hasn't changed again after failure.
        if self.dirty {
            return ModuleState::Pending;
        }
        if self.last_error.is_some() {
            return ModuleState::Failed;
        }
        ModuleState::Ready
    }

    pub fn available(&self) -> bool {
        self.available
    }

    pub fn auto_refresh(&self) -> bool {
        self.auto_refresh
    }

    pub fn set_auto_refresh(&mut self, enabled: bool) {
        if self.auto_refresh == enabled {
            return;
        }
        self.auto_refresh = enabled;
        self.pending_since = None;
        self.push(
            LogLevel::Info,
            format!(
                "project module: auto refresh {}",
                if enabled { "on" } else { "off" }
            ),
        );
    }

    /// Ctrl+R: rebuild immediately. Request kept until next `maybe_compile` (not lost even during build).
    pub fn request_refresh(&mut self) {
        self.refresh_requested = true;
        self.pending_since = None;
    }

    pub fn drain_events(&mut self) -> Vec<ProjectEvent> {
        std::mem::take(&mut self.events)
    }

    /// Called each frame (UI thread): collect compile result, trigger compile as needed.
    ///
    /// `playing` = Game view currently simulating; auto compile waits until Play ends.
    pub fn poll(&mut self, playing: bool) {
        self.poll_build();
        self.scan_sources();
        self.maybe_compile(playing);
    }

    /// Whether source has uncompiled changes (throttled scan, must still show when Auto Refresh off).
    fn scan_sources(&mut self) {
        if self.manifest.is_none() || self.prebuilt.is_some() {
            self.dirty = false;
            return;
        }
        let now = Instant::now();
        if let Some(last) = self.last_scan {
            if now.duration_since(last) < DIRTY_SCAN_INTERVAL {
                return;
            }
        }
        self.last_scan = Some(now);
        let Some(stamp) = self.source_stamp() else {
            return;
        };
        self.dirty = self.compiled_stamp.is_none_or(|compiled| compiled < stamp);
    }

    /// Called each frame during Game View Play: advance project code before engine systems.
    ///
    /// UI quads submitted by project go into `ExternalUiQuads`, same frame's `ui::render`
    /// merges them into `UiDrawList`.
    pub fn tick(
        &mut self,
        world: &mut prism_engine::ecs::World,
        input: &InputManager,
        dt: f32,
        scene_generation: u64,
        screen: [f32; 2],
    ) {
        // Module already judged faulty this frame: wait for reload, don't jump into same hole again.
        if self.faulted {
            return;
        }
        // Field-level borrows: session and components/assets don't overlap.
        let Some(session) = self.session.as_mut() else {
            return;
        };

        let mut commands = WorldCommands::new(world, &self.components, scene_generation)
            .with_assets(&self.assets)
            .with_scene_switch()
            .with_raycast(engine_raycast);
        // Project save (#984): Play save under `<project>/target/play-data/`,
        // separate from player's real save; don't declare feature when no writable dir.
        if let Some(save) = self.save.as_mut() {
            commands = commands.with_save(save);
        }

        // One-shot audio (#987): when no sound card `ProjectAudio::new` returns `None`, feature
        // not declared, project code gets `play_sound() == None` rather than silent success.
        if let Some(audio) = self.audio.as_mut() {
            audio.update();
            commands = commands.with_audio(audio);
        }
        // Project code can call `GameWorld::save_scene_state()` (#484); engine is here,
        // runtime-host only holds function pointer.
        commands = commands.with_scene_state(
            prism_engine::scene_state::save_scene_state,
            prism_engine::scene_state::load_scene_state,
        );
        let mut snapshot = input_snapshot(input, screen);
        snapshot.interpolation_alpha = self.interpolation_alpha;
        commands.set_input(snapshot);
        let host = commands.api();
        let result = session.tick(&host, dt.max(0.000_5));
        let quads = commands.take_quads();
        // Scene change request can't execute in callback (world borrowed), first store for host (#482).
        self.pending_scene = commands.take_scene_request();
        queue_project_quads(world, quads);

        let error = result.err().map(|error| error.to_string());
        // Stop here after one crash: project panicking each frame would burn stderr and GPU time,
        // only reloading module (save source / Ctrl+R) makes it try again.
        self.faulted = error.is_some();
        if error != self.last_tick_error {
            match &error {
                Some(error) => self.push(LogLevel::Error, format!("project tick failed: {error}")),
                None => self.push(LogLevel::Info, "project tick recovered".to_string()),
            }
            self.last_tick_error = error;
        }
    }

    /// Runs the optional project fixed-step callback with the same host world
    /// capabilities as the render-frame callback.
    pub fn fixed_update(
        &mut self,
        world: &mut prism_engine::ecs::World,
        input: &InputManager,
        dt: f32,
        scene_generation: u64,
        screen: [f32; 2],
    ) {
        if self.faulted {
            return;
        }
        let Some(session) = self.session.as_mut() else {
            return;
        };

        let mut commands = WorldCommands::new(world, &self.components, scene_generation)
            .with_assets(&self.assets)
            .with_scene_switch()
            .with_raycast(engine_raycast);
        if let Some(save) = self.save.as_mut() {
            commands = commands.with_save(save);
        }
        if let Some(audio) = self.audio.as_mut() {
            audio.update();
            commands = commands.with_audio(audio);
        }
        commands = commands.with_scene_state(
            prism_engine::scene_state::save_scene_state,
            prism_engine::scene_state::load_scene_state,
        );
        let mut snapshot = input_snapshot(input, screen);
        snapshot.key_edges = self.pending_fixed_key_edges;
        snapshot.interpolation_alpha = self.interpolation_alpha;
        self.pending_fixed_key_edges = [0; 8];
        commands.set_input(snapshot);
        let host = commands.api();
        let result = session.fixed_update(&host, dt.max(0.000_5));
        let quads = commands.take_quads();
        if let Some(request) = commands.take_scene_request() {
            self.pending_scene = Some(request);
        }
        queue_project_quads(world, quads);

        let error = result.err().map(|error| error.to_string());
        self.faulted = error.is_some();
        if error != self.last_tick_error {
            match &error {
                Some(error) => self.push(
                    LogLevel::Error,
                    format!("project fixed_update failed: {error}"),
                ),
                None => self.push(LogLevel::Info, "project fixed_update recovered".to_string()),
            }
            self.last_tick_error = error;
        }
    }

    /// Take scene change requested by project code; host executes after this tick's engine update.
    pub fn take_scene_request(&mut self) -> Option<SceneRequest> {
        self.pending_scene.take()
    }

    // ------------------------------------------------------------------
    // compile  + load
    // ------------------------------------------------------------------

    fn maybe_compile(&mut self, playing: bool) {
        if self.building || !self.available() {
            return;
        }

        // Prebuilt module preferred: only try once, on failure stay in error state (Ctrl+R won't reinstall).
        if let Some(path) = self.prebuilt.take() {
            match self.install(&path, Vec::new()) {
                Ok(()) => self.push(
                    LogLevel::Info,
                    format!(
                        "project module loaded from {MODULE_ENV}: {}",
                        path.display()
                    ),
                ),
                Err(error) => self.fail(error, Vec::new()),
            }
            return;
        }

        let manual = std::mem::take(&mut self.refresh_requested);
        if !manual && !self.auto_refresh {
            return;
        }
        // During Play don't auto-swap module: whole reload would drop project state (no script layer to migrate).
        // Placed before source scan, avoid traversing `src/` every frame.
        if !manual && playing {
            return;
        }

        let Some(stamp) = self.source_stamp() else {
            return;
        };
        if !manual
            && self
                .compiled_stamp
                .is_some_and(|compiled| compiled >= stamp)
        {
            self.pending_since = None;
            return;
        }

        let Some((manifest, target_dir)) = self.build_paths() else {
            return;
        };
        let now = Instant::now();
        let pending_since = *self.pending_since.get_or_insert(now);
        if now.duration_since(pending_since) < COMPILE_DEBOUNCE {
            return;
        }
        self.pending_since = None;

        // Record timestamp regardless of outcome: syntax error shouldn't make editor rerun cargo every 350ms.
        self.compiled_stamp = Some(stamp);
        self.building = true;
        self.last_error = None;
        self.push(
            LogLevel::Info,
            format!(
                "project module: compiling `{}` (preview-module)",
                manifest.display()
            ),
        );

        let (tx, rx) = channel();
        self.build = Some(rx);
        std::thread::spawn(move || {
            let outcome = build_module(&manifest, &target_dir);
            let _ = tx.send(outcome);
        });
    }

    fn poll_build(&mut self) {
        let Some(rx) = self.build.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(outcome) => {
                self.build = None;
                self.building = false;
                match outcome {
                    BuildOutcome::Built { path, diagnostics } => {
                        let warnings = diagnostics.len();
                        match self.install(&path, diagnostics) {
                            Ok(()) => {
                                self.push(
                                    LogLevel::Info,
                                    if warnings == 0 {
                                        format!("project module loaded: {}", path.display())
                                    } else {
                                        format!(
                                            "project module loaded: {} ({warnings} warnings, see Problems)",
                                            path.display()
                                        )
                                    },
                                );
                            }
                            Err(error) => self.fail(error, Vec::new()),
                        }
                    }
                    BuildOutcome::Failed {
                        message,
                        diagnostics,
                    } => self.fail(message, diagnostics),
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.build = None;
                self.building = false;
                self.fail(
                    "project module build thread exited without a result".to_string(),
                    Vec::new(),
                );
            }
        }
    }

    fn install(&mut self, built: &Path, diagnostics: Vec<Diagnostic>) -> Result<(), String> {
        let staged = stage_module(built)?;
        let session = match ProjectModule::load(&staged)
            .map_err(|error| error.to_string())
            .and_then(|module| module.start().map_err(|error| error.to_string()))
        {
            Ok(session) => session,
            Err(error) => {
                remove_staged_file(&staged);
                return Err(error);
            }
        };

        // First swap session then delete old copy: old code pointers must stay valid until new module takes effect.
        let previous = self.session.replace(session);
        drop(previous);
        if let Some(old) = self.staged.replace(staged) {
            remove_staged_file(&old);
        }
        self.last_error = None;
        self.last_tick_error = None;
        self.diagnostics = diagnostics;
        // Make next frame rescan immediately: source changed during compile shouldn't be treated as already compiled.
        self.last_scan = None;
        self.faulted = false;
        Ok(())
    }

    fn fail(&mut self, error: String, diagnostics: Vec<Diagnostic>) {
        let count = diagnostics.len();
        self.diagnostics = diagnostics;
        self.last_scan = None;
        self.last_error = Some(error.clone());
        self.push(LogLevel::Error, error);
        if count > 0 {
            self.push(
                LogLevel::Warn,
                format!("project module: {count} problem(s) — see the Problems panel"),
            );
        }
    }

    fn build_paths(&self) -> Option<(PathBuf, PathBuf)> {
        Some((self.manifest.clone()?, self.target_dir.clone()?))
    }

    fn source_stamp(&self) -> Option<SystemTime> {
        let manifest = self.manifest.as_deref()?;
        let project = manifest.parent()?;
        let mut newest = file_modified(manifest);
        collect_newest_source(&project.join("src"), &mut newest);
        newest
    }

    fn push(&mut self, level: LogLevel, text: String) {
        self.events.push(ProjectEvent { level, text });
    }
}

// ---------------------------------------------------------------------------
// input / UI quads
// ---------------------------------------------------------------------------

/// Pack host input into ABI snapshot.
///
/// Mapping body in [`prism_engine::input::input_snapshot`]: static Player's
/// `GameWorld::input()` reads same implementation (#589), two sides won't drift.
fn input_snapshot(input: &InputManager, screen: [f32; 2]) -> InputSnapshot {
    prism_engine::input::input_snapshot(input, screen)
}

/// Connect project module's raycast query to engine's pure geometry implementation (#522).
///
/// runtime-host doesn't depend on engine, only receives function pointers; editor is host, so adapt here.
fn engine_raycast(
    world: &prism_engine::ecs::World,
    origin: [f32; 3],
    direction: [f32; 3],
    max_distance: f32,
    layer_mask: u32,
) -> Option<RaycastResult> {
    prism_engine::physics::raycast(world, origin, direction, max_distance, layer_mask).map(|hit| {
        RaycastResult {
            entity: hit.entity,
            distance: hit.distance,
            point: hit.point,
            normal: hit.normal,
            is_trigger: hit.is_trigger,
        }
    })
}

/// Project module submitted quads → engine's `ExternalUiQuads`.
///
/// Must write before `engine.update()`: same frame's `ui::render` merges and clears them.
fn queue_project_quads(world: &mut prism_engine::ecs::World, quads: Vec<HostUiQuad>) {
    if quads.is_empty() {
        return;
    }
    let mut external = world
        .remove_resource::<prism_engine::ui::ExternalUiQuads>()
        .unwrap_or_default();
    external
        .0
        .extend(quads.into_iter().map(|quad| prism_engine::ui::UiQuad {
            rect: quad.rect,
            color: quad.color,
            border_radius: quad.border_radius,
            // Quads submitted by project module are solid-color only, no background image (#591).
            texture: None,
            layer: quad.layer,
            order: prism_engine::ui::EXTERNAL_UI_ORDER,
        }));
    world.insert_resource(external);
}

fn builtin_component_registry() -> ComponentRegistry {
    let mut registry = ComponentRegistry::new();
    prism_scene::register_builtin_components(&mut registry);
    registry
}

// ---------------------------------------------------------------------------
// Loose assets
// ---------------------------------------------------------------------------

/// Project's own `assets/` (not repo-root engine assets).
struct ProjectAssets {
    root: PathBuf,
}

impl ProjectAssets {
    fn resolve(&self, path: &str) -> Option<PathBuf> {
        let relative = Path::new(path);
        if relative.is_absolute()
            || relative.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return None;
        }
        Some(self.root.join("assets").join(relative))
    }
}

impl AssetIo for ProjectAssets {
    fn exists(&self, path: &str) -> bool {
        self.resolve(path).is_some_and(|path| path.is_file())
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        let path = self.resolve(path).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("asset path escapes the project root: {path}"),
            )
        })?;
        std::fs::read(&path)
    }
}

// ---------------------------------------------------------------------------
// compile
// ---------------------------------------------------------------------------

fn build_module(manifest: &Path, target_dir: &Path) -> BuildOutcome {
    let mut command = std::process::Command::new("cargo");
    command
        .arg("build")
        .arg("--manifest-path")
        .arg(manifest)
        .args(["--no-default-features", "--features", "preview-module"])
        .arg("--target-dir")
        .arg(target_dir)
        // Structured diagnostics: Problems panel needs file:line:col, not a colored stderr blob.
        .args(["--message-format", "json"]);
    if let Some(dir) = manifest.parent() {
        command.current_dir(dir);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let output = match command.output() {
        Ok(output) => output,
        Err(error) => {
            return BuildOutcome::Failed {
                message: format!("project module: cannot spawn cargo: {error}"),
                diagnostics: Vec::new(),
            }
        }
    };
    let root = manifest.parent().unwrap_or_else(|| Path::new("."));
    let diagnostics = diagnostics_from_cargo(&String::from_utf8_lossy(&output.stdout), root);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // First error is what user needs to fix; only fall back to stderr when no compiler diagnostics (link errors etc).
        let summary = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.level == LogLevel::Error)
            .map(|diagnostic| match diagnostic.location() {
                Some(location) => format!("{location}: {}", diagnostic.message),
                None => diagnostic.message.clone(),
            })
            .unwrap_or_else(|| tail_lines(stderr.trim(), 12));
        return BuildOutcome::Failed {
            message: format!("project module compile failed: {summary}"),
            diagnostics,
        };
    }

    let library = target_dir
        .join("debug")
        .join(library_file_name(&module_library_name(manifest)));
    if library.is_file() {
        BuildOutcome::Built {
            path: library,
            diagnostics,
        }
    } else {
        BuildOutcome::Failed {
            message: format!(
                "project module compile succeeded but `{}` is missing",
                library.display()
            ),
            diagnostics,
        }
    }
}

/// Copy build artifact to unique name first: next cargo will overwrite original, and Windows can't
/// overwrite already-loaded dll. Old copy deleted after session swap.
fn stage_module(built: &Path) -> Result<PathBuf, String> {
    let parent = built
        .parent()
        .ok_or_else(|| "project module path has no parent directory".to_string())?;
    let staging = parent.join("staged");
    std::fs::create_dir_all(&staging)
        .map_err(|error| format!("create module staging dir failed: {error}"))?;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let stem = built
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("project");
    // Uniqueness on **directory**, filename must stay: dbghelp finds symbol file by name recorded in PE
    // (`fishing.pdb`) in module directory, won't infer from `fishing-<nanos>.dll`.
    // Renamed dll makes project frame symbols degrade to export names (`prism_project_get_api`),
    // panic call stack would be wasted.
    let unique = staging.join(format!("{stem}-{nanos}"));
    std::fs::create_dir_all(&unique)
        .map_err(|error| format!("create module staging dir failed: {error}"))?;
    let Some(file_name) = built.file_name() else {
        return Err(format!(
            "project module path has no file name: {}",
            built.display()
        ));
    };
    let staged = unique.join(file_name);
    std::fs::copy(built, &staged)
        .map_err(|error| format!("copy project module failed: {error}"))?;

    // Symbol file in same directory as dll: without it project frames only have addresses.
    let pdb = built.with_extension("pdb");
    if pdb.is_file() {
        if let Some(pdb_name) = pdb.file_name() {
            if let Err(error) = std::fs::copy(&pdb, unique.join(pdb_name)) {
                log::warn!("copy project module symbols failed (stack will lack frames): {error}");
            }
        }
    } else {
        log::debug!(
            "project module has no .pdb ({}), stack frames will be addresses",
            pdb.display()
        );
    }
    Ok(staged)
}

/// Remove staging directory from one load (dll + pdb). Module dll must already be unloaded.
fn remove_staged_file(staged: &Path) {
    let Some(dir) = staged.parent() else {
        return;
    };
    if let Err(error) = std::fs::remove_dir_all(dir) {
        log::debug!(
            "remove project module staging dir failed {}: {error}",
            dir.display()
        );
    }
}

fn library_file_name(package: &str) -> String {
    let crate_name = package.replace('-', "_");
    if cfg!(target_os = "windows") {
        format!("{crate_name}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{crate_name}.dylib")
    } else {
        format!("lib{crate_name}.so")
    }
}

fn tail_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(count);
    lines[start..].join("\n")
}

// ---------------------------------------------------------------------------
// Manifest / source mtime
// ---------------------------------------------------------------------------

fn manifest_declares_preview_module(manifest: &Path) -> bool {
    std::fs::read_to_string(manifest)
        .map(|text| text.contains("preview-module"))
        .unwrap_or(false)
}

/// Read package name from `Cargo.toml` `[package]` section (build artifact name derived from it).
fn package_name(manifest: &Path) -> Option<String> {
    let text = std::fs::read_to_string(manifest).ok()?;
    let mut in_package = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        let Some(rest) = line.strip_prefix("name") else {
            continue;
        };
        let rest = rest.trim_start().strip_prefix('=')?.trim();
        let name = rest.trim_matches('"').trim_matches('\'');
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    None
}

/// Read `[lib] name` from a project manifest.
///
/// The lib target name decides the cdylib filename. A project can keep a
/// user-facing bin name and use a different internal library name, so the Game
/// View loader must follow the lib target instead of assuming the package name.
fn lib_name(manifest: &Path) -> Option<String> {
    let text = std::fs::read_to_string(manifest).ok()?;
    let mut in_lib = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_lib = line == "[lib]";
            continue;
        }
        if !in_lib {
            continue;
        }
        let Some(rest) = line.strip_prefix("name") else {
            continue;
        };
        let rest = rest.trim_start().strip_prefix('=')?.trim();
        let name = rest.trim_matches('"').trim_matches('\'');
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    None
}

/// Library name for the Game View module: `[lib] name`, else the package name.
fn module_library_name(manifest: &Path) -> String {
    lib_name(manifest)
        .or_else(|| package_name(manifest))
        .unwrap_or_else(|| "game".to_string())
}

fn file_modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn collect_newest_source(dir: &Path, newest: &mut Option<SystemTime>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_newest_source(&path, newest);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            if let Some(modified) = file_modified(&path) {
                if newest.is_none_or(|current| modified > current) {
                    *newest = Some(modified);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_engine::input::MouseButton;
    use prism_engine::input::{ElementState, KeyCode};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "prism-editor-project-module-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn package_name_comes_from_the_package_section() {
        let dir = temp_dir("package");
        let manifest = dir.join("Cargo.toml");
        std::fs::write(
            &manifest,
            "[workspace]\nname = \"not-this\"\n\n[package]\nname = \"fishing-game\"\n",
        )
        .unwrap();
        assert_eq!(package_name(&manifest), Some("fishing-game".to_string()));
    }

    #[test]
    fn preview_feature_is_detected_from_the_manifest() {
        let dir = temp_dir("feature");
        let manifest = dir.join("Cargo.toml");
        std::fs::write(&manifest, "[package]\nname = \"game\"\n").unwrap();
        assert!(!manifest_declares_preview_module(&manifest));
        std::fs::write(
            &manifest,
            "[package]\nname = \"game\"\n\n[features]\npreview-module = []\n",
        )
        .unwrap();
        assert!(manifest_declares_preview_module(&manifest));
    }

    #[test]
    fn library_name_uses_the_package_name() {
        let name = library_file_name("my-game");
        assert!(name.contains("my_game"));
        assert!(name.ends_with(".so") || name.ends_with(".dll") || name.ends_with(".dylib"));
    }

    #[test]
    fn lib_section_name_wins_over_the_package_name() {
        let dir = temp_dir("lib-name");
        let manifest = dir.join("Cargo.toml");
        std::fs::write(
            &manifest,
            "[package]\nname = \"my-game\"\n\n[lib]\nname = \"my_game_core\"\n",
        )
        .unwrap();
        assert_eq!(module_library_name(&manifest), "my_game_core");
        assert!(library_file_name(&module_library_name(&manifest)).contains("my_game_core"));
    }

    #[test]
    fn module_library_name_falls_back_to_the_package_name() {
        let dir = temp_dir("lib-fallback");
        let manifest = dir.join("Cargo.toml");
        std::fs::write(&manifest, "[package]\nname = \"my-game\"\n").unwrap();
        assert_eq!(module_library_name(&manifest), "my-game");
    }

    #[test]
    fn held_keys_are_reported_even_after_begin_frame() {
        let mut input = InputManager::new();
        input.handle_keyboard(KeyCode::Space, ElementState::Pressed);
        // `begin_frame` clears just_pressed, but held state must remain:
        // fishing uses `input.key(keys::SPACE)` to detect continuous line retrieval.
        input.begin_frame();
        let snapshot = input_snapshot(&input, [1280.0, 720.0]);
        assert!(snapshot.key(KeyCode::Space.bit_index().expect("named key")));
        assert_eq!(snapshot.screen, [1280.0, 720.0]);
    }

    #[test]
    fn click_is_reported_as_just_pressed() {
        let mut input = InputManager::new();
        input.handle_mouse_button(MouseButton::Left, ElementState::Pressed);
        let snapshot = input_snapshot(&input, [100.0, 50.0]);
        assert!(snapshot.left_clicked);
        assert!(snapshot.left_held);
    }

    #[test]
    fn right_and_middle_buttons_reach_the_snapshot() {
        let mut input = InputManager::new();
        input.handle_mouse_button(MouseButton::Right, ElementState::Pressed);
        let snapshot = input_snapshot(&input, [800.0, 600.0]);
        assert!(snapshot.right_clicked, "Right-click edge must be visible");
        assert!(snapshot.right_held);
        assert!(!snapshot.middle_held);

        let mut input = InputManager::new();
        input.handle_mouse_button(MouseButton::Middle, ElementState::Pressed);
        let snapshot = input_snapshot(&input, [800.0, 600.0]);
        assert!(snapshot.middle_clicked);
        assert!(snapshot.middle_held);
        assert!(!snapshot.right_held);
    }

    #[test]
    fn key_edges_are_reported_separately_from_held_keys() {
        let mut input = InputManager::new();
        input.handle_keyboard(KeyCode::Space, ElementState::Pressed);
        let snapshot = input_snapshot(&input, [1.0, 1.0]);
        assert!(
            snapshot.key(KeyCode::Space.bit_index().expect("named key")),
            "Held bitmap"
        );
        assert!(
            snapshot.key_pressed(KeyCode::Space.bit_index().expect("named key")),
            "Pressed edge"
        );
        assert!(!snapshot.key_released(KeyCode::Space.bit_index().expect("named key")));

        // On frame window opens: held bitmap cleared, release edge set.
        // Host calls `begin_frame` at start of each frame, do same here, else previous frame's press-below edge remains.
        input.begin_frame();
        input.handle_keyboard(KeyCode::Space, ElementState::Released);
        let snapshot = input_snapshot(&input, [1.0, 1.0]);
        assert!(!snapshot.key(KeyCode::Space.bit_index().expect("named key")));
        assert!(!snapshot.key_pressed(KeyCode::Space.bit_index().expect("named key")));
        assert!(snapshot.key_released(KeyCode::Space.bit_index().expect("named key")));
    }

    #[test]
    fn scroll_and_pointer_lock_are_forwarded() {
        let mut input = InputManager::new();
        input.handle_scroll(2.0);
        input.pointer_locked = true;
        let snapshot = input_snapshot(&input, [1.0, 1.0]);
        assert!((snapshot.scroll - 2.0).abs() < 1e-6);
        assert!(snapshot.pointer_locked);
    }

    #[test]
    fn touches_keep_their_id_and_phase() {
        use prism_engine::input::TouchPhase;
        let mut input = InputManager::new();
        input.handle_touch(7, TouchPhase::Started, [10.0, 20.0]);
        input.handle_touch(9, TouchPhase::Moved, [30.0, 40.0]);
        let snapshot = input_snapshot(&input, [100.0, 100.0]);

        assert_eq!(snapshot.touch_count, 2);
        let touches = snapshot.touch_slice();
        assert_eq!(touches.len(), 2);
        assert_eq!(touches[0].id, 7);
        assert_eq!(touches[0].position, [10.0, 20.0]);
        assert_eq!(touches[0].phase, prism_runtime_host::TOUCH_PHASE_STARTED);
        assert_eq!(touches[1].id, 9);
        assert_eq!(touches[1].phase, prism_runtime_host::TOUCH_PHASE_MOVED);
        assert!(snapshot.any_touch());

        // When fully "ended" don't count as having touch point pressed again.
        input.begin_frame();
        input.handle_touch(7, TouchPhase::Ended, [10.0, 20.0]);
        let snapshot = input_snapshot(&input, [100.0, 100.0]);
        assert!(!snapshot.any_touch());
    }

    #[test]
    fn project_assets_are_read_below_the_project_assets_dir() {
        let dir = temp_dir("assets");
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("assets/lake_layers.json"), br#"{"layers":[]}"#).unwrap();
        let assets = ProjectAssets { root: dir.clone() };
        assert!(assets.exists("lake_layers.json"));
        assert_eq!(
            assets.read("lake_layers.json").unwrap(),
            br#"{"layers":[]}"#
        );
        assert!(!assets.exists("../Cargo.toml"));
        assert!(assets.read("../Cargo.toml").is_err());
    }

    #[test]
    fn source_stamp_tracks_rust_sources() {
        let dir = temp_dir("stamp");
        std::fs::create_dir_all(dir.join("src/game")).unwrap();
        let manifest = dir.join("Cargo.toml");
        std::fs::write(&manifest, "[package]\nname = \"game\"\n").unwrap();
        std::fs::write(dir.join("src/game/mod.rs"), "fn main() {}\n").unwrap();

        let driver = ProjectModuleDriver::new(&dir);
        assert!(driver.source_stamp().is_some());
    }

    /// End-to-end: drive `projects/template` preview module with real `.scene.json`.
    ///
    /// Need to build module first and set `PRISMA_TEST_PROJECT_MODULE`; skip when unset,
    /// consistent with `prism-runtime-host` env test of same name.
    #[test]
    fn preview_module_drives_the_intro_scene_when_configured() {
        let Some(module_path) = std::env::var_os("PRISMA_TEST_PROJECT_MODULE") else {
            return;
        };
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let scene = repo.join("assets/scenes/intro.scene.json");
        let mut engine = prism_engine::Engine::empty();
        engine
            .load_scene_json_file(&scene)
            .expect("intro scene should load");

        let module = ProjectModule::load(&module_path).expect("preview module should load");
        let mut session = module.start().expect("preview session should start");
        let components = builtin_component_registry();
        let assets = ProjectAssets { root: repo };
        let mut last_quads = 0;
        for _ in 0..90 {
            let mut commands =
                WorldCommands::new(engine.world_mut(), &components, 1).with_assets(&assets);
            let host = commands.api();
            session
                .tick(&host, 1.0 / 60.0)
                .expect("project tick should succeed");
            last_quads = commands.take_quads().len();
        }
        // Intro raindrops submitted via `GameWorld::draw_quad`; able to come back across ABI proves
        // component read/write and UI quad channel both work.
        assert!(last_quads > 0, "intro should submit UI quads");
    }

    #[test]
    fn poll_loads_a_prebuilt_module_when_configured() {
        let Some(module_path) = std::env::var_os("PRISMA_TEST_PROJECT_MODULE") else {
            return;
        };
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let mut driver = ProjectModuleDriver::new(&repo.join("projects/template"));
        driver.prebuilt = Some(PathBuf::from(module_path));
        assert!(!driver.status().loaded);
        driver.poll(false);
        assert!(
            driver.status().loaded,
            "poll should install the prebuilt module"
        );
    }

    #[test]
    fn driver_stages_and_reloads_a_prebuilt_module_when_configured() {
        let Some(module_path) = std::env::var_os("PRISMA_TEST_PROJECT_MODULE") else {
            return;
        };
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let mut driver = ProjectModuleDriver::new(&repo.join("projects/template"));
        assert!(driver.available());
        driver
            .install(Path::new(&module_path), Vec::new())
            .expect("prebuilt module should install");
        assert!(driver.status().loaded);
        let first = driver.staged.clone().expect("first staged copy");

        driver
            .install(Path::new(&module_path), Vec::new())
            .expect("prebuilt module should reload");
        let second = driver.staged.clone().expect("second staged copy");
        assert_ne!(first, second, "each load gets a unique file name");
        assert!(!first.exists(), "old copy is removed after the swap");
        assert!(second.exists());
    }

    #[test]
    fn driver_without_preview_feature_is_unavailable() {
        let dir = temp_dir("unavailable");
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"game\"\n").unwrap();
        let driver = ProjectModuleDriver::new(&dir);
        assert!(!driver.available());
        assert!(!driver.status().loaded);
        assert_eq!(driver.status().state, ModuleState::Unavailable);
    }

    /// State machine: changes-not-compiled = Pending, error = Failed, success = Ready.
    #[test]
    fn module_state_reports_pending_and_failed() {
        let dir = temp_dir("state");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"game\"\n\n[features]\npreview-module = []\n",
        )
        .unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn create() {}\n").unwrap();
        let mut driver = ProjectModuleDriver::new(&dir);
        driver.scan_sources();
        assert_eq!(driver.status().state, ModuleState::Pending);
        assert!(driver.status().dirty);

        // Failure also records compile timestamp: source unchanged shouldn't rerun cargo every 350 ms.
        driver.compiled_stamp = driver.source_stamp();
        driver.last_scan = None;
        driver.scan_sources();
        driver.fail("boom".to_string(), Vec::new());
        assert_eq!(driver.status().state, ModuleState::Failed);
        assert!(!driver.status().dirty);

        // After changing back source (or manual rebuild), next compile period starts fresh.
        driver.last_error = None;
        assert_eq!(driver.status().state, ModuleState::Ready);
    }
}
