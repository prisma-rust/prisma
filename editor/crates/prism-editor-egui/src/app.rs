//!

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::egui_host::{self, App, AshRenderState, CreationContext, ExitSignal, HandleRedraw};
use anyhow::Context as _;
use egui::{Align, Color32, CursorIcon, Layout, Rect, RichText, Sense, Stroke, Vec2};

use prism_editor_core::dock::split_side_length;
use prism_editor_core::{Command, DockDirection, DockNode, Locale, Preferences, Workspace};
use serde_json::json;

use crate::engine_host::ViewKind;
use crate::gpu::SharedVulkan;
use crate::logging;
use crate::panels::SkeletonJointRow;
use crate::panels::{self, UiState};
use crate::project_module::ModuleState;
use crate::selection_history::{SelectionHistory, SelectionSnapshot};
use crate::theme;
use crate::viewport::{GameResolution, ImageBridge, ViewportState};
use egui_phosphor::regular as icons;

#[path = "app/chrome.rs"]
mod chrome;
#[path = "app/dock_ui.rs"]
mod dock_ui;
#[path = "app/mcp_control.rs"]
mod mcp_control;
#[path = "app/shortcuts.rs"]
mod shortcuts;

use chrome::*;
use mcp_control::*;
use shortcuts::*;

/// Stable English tab titles for the feature map and agent-facing tests.
///
/// The user-visible tab label comes from [`Workspace::panel_title`] and follows the locale, so this
/// table must never be used for rendering.
#[cfg(test)]
pub(super) fn tab_title(tab: &str) -> &str {
    match tab {
        "sceneView" => "Scene",
        "gameView" => "Game",
        "hierarchy" => "Hierarchy",
        "inspector" => "Inspector",
        "assets" => "Assets",
        "docs" => "Documents",
        "log" => "Console",
        "history" => "History",
        "renderGraph" => "Render Graph",
        "projectSettings" => "Project Settings",
        "preferences" => "Preferences",
        "theme" => "Theme",
        "build" => "Build",
        "problems" => "Problems",
        "prefabOverrides" => "Prefab Overrides",
        "material" => "Material",
        "animGraph" => "Animation Graph",
        "gaspDebug" => "GASP Debug",
        "tools" => "Tools",
        "shortcuts" => "Shortcuts",
        "about" => "About",
        other => other,
    }
}

pub(super) struct AudioPreview {
    engine: prism_audio::AudioEngine,
    handle: Option<prism_audio::PlaybackHandle>,

    started: bool,
}

pub struct EditorApp {
    pub workspace: Workspace,
    pub state: UiState,
    pub preferences: Preferences,
    pub(super) project_root: Option<PathBuf>,
    pub(super) preview_scene: PathBuf,

    pub(super) gpu: SharedVulkan,
    pub(super) bridge: ImageBridge,
    pub(super) scene_view: Option<ViewportState>,
    pub(super) game_view: Option<ViewportState>,

    pub(super) exit_signal: ExitSignal,
    /// Last `(doc.uid, doc.revision, doc.transform_revision)` synced to `scene_view`.
    pub(super) synced_doc_state: Option<(u64, u64, u64)>,
    /// Document state captured on Play entry to detect edits discarded at Stop.
    pub(super) play_start_doc_state: Option<(u64, u64, u64)>,
    /// Selection hash so Scene view gizmos and outlines stay responsive.
    pub(super) selection_hash: u64,
    /// Visibility hash (`hidden_entities` + `isolate`) forcing viewport redraws.
    pub(super) visibility_hash: u64,
    pub(super) selection_history: SelectionHistory,
    pub(super) last_frame: Instant,

    pub(super) was_focused: bool,
    pub(super) playing: bool,
    pub(super) paused: bool,

    pub(super) force_quit: bool,

    pub(super) audio_preview: Option<AudioPreview>,
    pub(super) render_report: Option<String>,

    pub(super) render_report_mtime: Option<std::time::SystemTime>,

    pub(super) named_layouts: std::collections::BTreeMap<String, serde_json::Value>,
    pub(super) layouts_path: PathBuf,

    pub(super) build: Option<crate::build::BuildJob>,

    pub(super) loose_assets: Option<crate::loose_assets::LooseAssetWatcher>,

    pub(super) loose_scan_at: Instant,

    pub(super) build_diagnostics: Vec<crate::diagnostics::Diagnostic>,

    pub(super) render_settings: prism_engine::render_settings::RenderSettings,

    pub(super) theme_signature: String,
    pub(super) state_path: PathBuf,
    pub(super) autosave_at: Instant,

    pub(super) hwnd: Option<isize>,

    pub(super) control: Option<crate::control::ControlServer>,

    pub(super) control_cli_disabled: bool,

    pub(super) control_wanted: bool,

    pub(super) control_error: Option<String>,

    pub(super) frame_count: u64,

    pub(super) pending_captures: Vec<PendingCapture>,

    pub(super) panel_rects: std::collections::BTreeMap<String, egui::Rect>,

    pub(super) dock_leaves: Vec<(String, String, Vec<String>)>,

    pub(super) palette: crate::palette::Palette,

    pub(super) pending_input: std::collections::VecDeque<InjectedInput>,
}

/// The inspector schema has exactly one source: the reflection registry of
/// `prism-scene`, projected by `prism_editor_api::build_schema`.
/// The fixture used to carry a frozen snapshot of it, which silently went stale
/// (19 components and 23 nested types missing by the time it was noticed), so
/// new components never showed up in Add Component or the inspector.
pub(super) fn runtime_schema() -> prism_editor_core::Schema {
    let value = serde_json::to_value(prism_editor_api::build_schema())
        .expect("the editor schema is serializable");
    prism_editor_core::Schema::from_value(&value)
        .expect("the editor schema matches the UI-side model")
}

pub(super) fn load_workspace() -> Workspace {
    let candidates: Vec<PathBuf> = std::env::var("PRISMA_EDITOR_DEMO")
        .ok()
        .map(PathBuf::from)
        .into_iter()
        .chain([
            PathBuf::from("editor/public/demo.json"),
            PathBuf::from("../editor/public/demo.json"),
            PathBuf::from("../../editor/public/demo.json"),
        ])
        .collect();

    for path in candidates {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let mut workspace = Workspace::load_fixture(&value);
        workspace.log(
            prism_editor_core::LogSource::Editor,
            prism_editor_core::LogLevel::Info,
            format!("fixture: {}", path.display()),
        );
        install_schema(&mut workspace);
        attach_project(&mut workspace);
        return workspace;
    }

    let mut workspace = Workspace::sample();
    install_schema(&mut workspace);
    attach_project(&mut workspace);
    workspace
}

/// Install the live schema into the workspace (both the fixture and the
/// no-fixture startup path go through here).
pub(super) fn install_schema(workspace: &mut Workspace) {
    workspace.set_schema(runtime_schema());
    workspace.log(
        prism_editor_core::LogSource::Editor,
        prism_editor_core::LogLevel::Info,
        format!(
            "schema: {} types ({} components)",
            workspace.schema.types.len(),
            workspace.schema.components().len()
        ),
    );
}

pub(super) fn attach_project(workspace: &mut Workspace) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let Some(root) = prism_editor_core::find_project_root(&cwd) else {
        workspace.log(
            prism_editor_core::LogSource::Editor,
            prism_editor_core::LogLevel::Warn,
            "no project root found; running with the fixture only",
        );
        return;
    };
    match workspace.load_project(root.clone()) {
        Ok(count) => workspace.log(
            prism_editor_core::LogSource::Editor,
            prism_editor_core::LogLevel::Info,
            format!("project {} ({count} documents)", root.display()),
        ),
        Err(error) => workspace.log(
            prism_editor_core::LogSource::Editor,
            prism_editor_core::LogLevel::Error,
            format!("load project failed: {error}"),
        ),
    }
}

pub(super) fn read_named_layouts(
    path: &Path,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return std::collections::BTreeMap::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub(super) fn read_editor_state(path: &Path) -> Option<prism_editor_core::EditorState> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub(super) fn render_settings_json(
    settings: &prism_engine::render_settings::RenderSettings,
) -> serde_json::Value {
    use prism_engine::render_settings::RenderMode;
    serde_json::json!({
        "debugFlags": settings.debug_flags,
        "tonemapMode": settings.tonemap_mode,
        "aaMode": settings.aa_mode,
        "contrast": settings.contrast,
        "saturation": settings.saturation,
        "vignette": settings.vignette,
        "bloom": settings.bloom,
        "bloomIntensity": settings.bloom_intensity,
        "bloomThreshold": settings.bloom_threshold,
        "chromaticAberration": settings.chromatic_aberration,
        "filmGrain": settings.film_grain,
        "temperature": settings.temperature,
        "tint": settings.tint,
        "dofFocusDistance": settings.dof_focus_distance,
        "dofAperture": settings.dof_aperture,
        "dofMaxBlur": settings.dof_max_blur,
        "lut": settings.lut,
        "lutStrength": settings.lut_strength,
        "motionBlur": settings.motion_blur,
        "motionBlurMaxPixels": settings.motion_blur_max_pixels,
        "taa": settings.taa,
        "taaStrength": settings.taa_strength,

        "occlusionCulling": settings.occlusion_culling,
        "shadowDistance": settings.shadow_distance,
        "shadowCascadeCount": settings.shadow_cascade_count,
        "debugRt": settings.debug_rt,
        "wireframe": settings.wireframe,
        "shadedWireframe": settings.shaded_wireframe,
        "pathTrace": settings.render_mode == RenderMode::PathTrace,
        "ptMaxBounces": settings.pt_max_bounces,
        "ptRayMaxDistance": settings.pt_ray_max_distance,
        "ptMaxIterations": settings.pt_max_iterations,
        "showGrid": settings.show_grid,
    })
}

pub(super) fn render_settings_from_json(
    value: &serde_json::Value,
    fallback_grid: bool,
) -> prism_engine::render_settings::RenderSettings {
    use prism_engine::render_settings::RenderMode;
    let base = prism_engine::render_settings::RenderSettings::default();
    let u32_field = |key: &str, default: u32| {
        value
            .get(key)
            .and_then(|value| value.as_u64())
            .map(|value| value as u32)
            .unwrap_or(default)
    };
    let f32_field = |key: &str, default: f32| {
        value
            .get(key)
            .and_then(|value| value.as_f64())
            .map(|value| value as f32)
            .unwrap_or(default)
    };
    let path_trace = value
        .get("pathTrace")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    prism_engine::render_settings::RenderSettings {
        debug_flags: u32_field("debugFlags", base.debug_flags),
        tonemap_mode: u32_field("tonemapMode", base.tonemap_mode),
        aa_mode: u32_field("aaMode", base.aa_mode),
        occlusion_culling: value
            .get("occlusionCulling")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(base.occlusion_culling),
        contrast: f32_field("contrast", base.contrast),
        saturation: f32_field("saturation", base.saturation),
        vignette: f32_field("vignette", base.vignette),
        bloom: value
            .get("bloom")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(base.bloom),
        bloom_intensity: f32_field("bloomIntensity", base.bloom_intensity),
        bloom_threshold: f32_field("bloomThreshold", base.bloom_threshold),
        chromatic_aberration: f32_field("chromaticAberration", base.chromatic_aberration),
        film_grain: f32_field("filmGrain", base.film_grain),
        temperature: f32_field("temperature", base.temperature),
        tint: f32_field("tint", base.tint),
        dof_focus_distance: f32_field("dofFocusDistance", base.dof_focus_distance),
        dof_aperture: f32_field("dofAperture", base.dof_aperture),
        dof_max_blur: f32_field("dofMaxBlur", base.dof_max_blur),
        lut: value
            .get("lut")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .unwrap_or(base.lut),
        lut_strength: f32_field("lutStrength", base.lut_strength),
        motion_blur: f32_field("motionBlur", base.motion_blur),
        motion_blur_max_pixels: f32_field("motionBlurMaxPixels", base.motion_blur_max_pixels),
        taa: value
            .get("taa")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(base.taa),
        taa_strength: f32_field("taaStrength", base.taa_strength),
        shadow_distance: f32_field("shadowDistance", base.shadow_distance),

        shadow_map_size: u32_field("shadowMapSize", base.shadow_map_size),
        shadow_cascade_count: u32_field("shadowCascadeCount", base.shadow_cascade_count)
            .clamp(1, 2),
        debug_rt: u32_field("debugRt", base.debug_rt),
        wireframe: value
            .get("wireframe")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(base.wireframe),
        shaded_wireframe: value
            .get("shadedWireframe")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(base.shaded_wireframe),
        render_mode: if path_trace {
            RenderMode::PathTrace
        } else {
            RenderMode::Raster
        },
        pt_max_bounces: u32_field("ptMaxBounces", base.pt_max_bounces),
        pt_ray_max_distance: f32_field("ptRayMaxDistance", base.pt_ray_max_distance),
        pt_max_iterations: u32_field("ptMaxIterations", base.pt_max_iterations),
        show_grid: value
            .get("showGrid")
            .and_then(|value| value.as_bool())
            .unwrap_or(fallback_grid),

        internal_resolution: base.internal_resolution,
        internal_scale_mode: base.internal_scale_mode,

        ..Default::default()
    }
}

pub(super) fn selection_hash(workspace: &Workspace) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for index in &workspace.selection {
        hash ^= *index as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
pub(super) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub(super) fn refresh_render_report(app: &mut EditorApp) {
    let Some(root) = app.project_root.clone() else {
        return;
    };
    let path = root.join("target/prism-render-graph.json");
    let Ok(modified) = std::fs::metadata(&path).and_then(|meta| meta.modified()) else {
        return;
    };
    if app.render_report_mtime == Some(modified) {
        return;
    }
    if let Ok(text) = std::fs::read_to_string(&path) {
        app.render_report = Some(text);
        app.render_report_mtime = Some(modified);
    }
}

pub(super) fn read_report(root: Option<&Path>) -> Option<String> {
    let root = root?;
    let path = root.join("target/prism-render-graph.json");
    std::fs::read_to_string(path).ok()
}

pub(super) fn initial_view_settings(
    kind: ViewKind,
    editor: &prism_engine::render_settings::RenderSettings,
) -> prism_engine::render_settings::RenderSettings {
    match kind {
        ViewKind::Scene => editor.clone(),

        ViewKind::Game => prism_engine::render_settings::RenderSettings {
            ..Default::default()
        },
    }
}

pub(super) fn project_window_resolution(project_root: Option<&Path>) -> Option<(u32, u32)> {
    let root = project_root?;
    let path = root.join("assets").join("settings.ini");
    let text = std::fs::read_to_string(path).ok()?;
    let config = prism_engine::config::AppConfig::from_ini(&text);
    (config.window.width > 0 && config.window.height > 0)
        .then_some((config.window.width, config.window.height))
}

pub(super) fn seeded_render_settings(
    restored: Option<&serde_json::Value>,
    project: Option<prism_engine::render_settings::RenderSettings>,
    show_grid: bool,
) -> prism_engine::render_settings::RenderSettings {
    if let Some(value) = restored {
        return render_settings_from_json(value, show_grid);
    }
    let (mut settings, source) = match project {
        Some(settings) => (settings, "project settings.ini"),
        None => (
            prism_engine::render_settings::RenderSettings::default(),
            "engine defaults",
        ),
    };
    settings.show_grid = show_grid;
    log::info!("scene view render defaults: {source}");
    settings
}

pub(super) fn project_render_settings(
    root: Option<&Path>,
) -> Option<prism_engine::render_settings::RenderSettings> {
    let root = root?;
    let path = root.join("assets").join("settings.ini");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            log::debug!("no project render settings at {}: {error}", path.display());
            return None;
        }
    };
    let config = prism_engine::config::AppConfig::from_ini(&text);
    let mut settings = prism_engine::render_settings::RenderSettings::default();
    config.render.apply(&mut settings);
    Some(settings)
}

pub(super) fn apply_project_render_defaults(
    host: &mut crate::engine_host::EngineHost,
    project_root: Option<&Path>,
) {
    let Some(settings) = project_render_settings(project_root) else {
        return;
    };
    *host.settings_mut() = settings;
    log::info!(
        "game view render defaults: {}",
        project_root
            .map(|root| root.join("assets/settings.ini").display().to_string())
            .unwrap_or_default()
    );
}

impl EditorApp {
    pub fn new(cc: &CreationContext<'_>) -> anyhow::Result<Self> {
        let gpu = SharedVulkan::create(cc).context("create shared Vulkan device")?;
        let sampler = unsafe {
            gpu.context.device.create_sampler(
                &ash::vk::SamplerCreateInfo::default()
                    .mag_filter(ash::vk::Filter::LINEAR)
                    .min_filter(ash::vk::Filter::LINEAR)
                    .mipmap_mode(ash::vk::SamplerMipmapMode::NEAREST)
                    .address_mode_u(ash::vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(ash::vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(ash::vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .unnormalized_coordinates(false),
                None,
            )
        }
        .context("create viewport sampler")?;

        let bridge = ImageBridge {
            registry: cc.image_registry.clone(),
            sampler,
        };

        theme::install_fonts(&cc.context);

        cc.context
            .send_viewport_cmd(egui::ViewportCommand::Title("Prisma Editor".to_owned()));
        cc.context
            .send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1440.0, 900.0)));

        let mut workspace = load_workspace();
        theme::apply(&cc.context, &workspace.colors());

        let project_root = workspace.project_root.clone();
        let loose_assets = project_root
            .as_deref()
            .map(crate::loose_assets::LooseAssetWatcher::new);
        let loose_scan_at = Instant::now();
        let preview_dir = project_root
            .clone()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("target/editor-egui")
            .join("preview");
        std::fs::create_dir_all(&preview_dir).ok();
        let preview_scene = preview_dir.join("scene.json");
        let state_path = project_root
            .clone()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("target/editor-egui/state.json");

        let layouts_path = state_path
            .parent()
            .map(|dir| dir.join("layouts.json"))
            .unwrap_or_else(|| PathBuf::from("layouts.json"));
        let named_layouts = read_named_layouts(&layouts_path);

        let saved_state = read_editor_state(&state_path);
        if let Some(state) = &saved_state {
            let layout = prism_editor_core::DockLayout::from_json(
                &state.dock,
                &workspace.known_panels(),
                1440.0,
                900.0,
            );
            workspace.dock = layout.dock;
            workspace.floating = layout.floating;

            workspace.maximized_tab = state.maximized.clone().filter(|tab| {
                prism_editor_core::dock::collect_tabs(&workspace.dock)
                    .iter()
                    .any(|existing| existing == tab)
            });
            workspace.set_locale(Locale::parse(&state.locale));
            if !state.theme_preset.is_empty() {
                workspace.set_theme_preset(&state.theme_preset);
            }
        }

        let restored_scene = saved_state
            .as_ref()
            .map(|state| state.scene.clone())
            .unwrap_or_default();
        let restored_tool = match restored_scene.gizmo_tool.as_str() {
            "Rotate" => crate::viewport::GizmoTool::Rotate,
            "Scale" => crate::viewport::GizmoTool::Scale,
            _ => crate::viewport::GizmoTool::Translate,
        };
        let render_settings = seeded_render_settings(
            restored_scene.render.as_ref(),
            project_render_settings(project_root.as_deref()),
            workspace.show_grid,
        );
        workspace.show_grid = render_settings.show_grid;

        let control_cli_disabled = crate::control::cli_disabled();
        let control_enabled = !control_cli_disabled
            && saved_state
                .as_ref()
                .map(|state| state.preferences.control_channel)
                .unwrap_or(true);
        let (control_server, control_error) = match crate::control::ControlServer::start_if(
            control_enabled,
            state_path.parent().unwrap_or_else(|| Path::new(".")),
        ) {
            Ok(server) => (server, None),
            Err(error) => {
                log::warn!("control channel disabled: {error:#}");
                (None, Some(format!("{error:#}")))
            }
        };

        let mut app = Self {
            synced_doc_state: None,
            play_start_doc_state: None,
            selection_hash: 0,
            visibility_hash: 0,
            selection_history: SelectionHistory::default(),
            last_frame: Instant::now(),
            was_focused: false,
            playing: false,
            paused: false,
            force_quit: false,
            audio_preview: None,
            render_report: None,
            render_report_mtime: None,
            palette: crate::palette::Palette::new(),
            named_layouts,
            layouts_path,
            build: None,
            loose_assets,
            loose_scan_at,
            build_diagnostics: Vec::new(),

            render_settings,
            theme_signature: String::new(),
            autosave_at: Instant::now(),
            workspace,
            state: UiState::new(),
            preferences: saved_state
                .as_ref()
                .map(|state| state.preferences.clone())
                .unwrap_or_default(),
            project_root,
            preview_scene,
            gpu,
            bridge,
            scene_view: None,
            game_view: None,
            exit_signal: cc.exit_signal.clone(),
            state_path: state_path.clone(),
            hwnd: main_window_hwnd(cc.main_window),
            control: control_server,
            control_cli_disabled,
            control_wanted: control_enabled,
            control_error,
            frame_count: 0,
            pending_captures: Vec::new(),
            panel_rects: std::collections::BTreeMap::new(),
            dock_leaves: Vec::new(),
            pending_input: std::collections::VecDeque::new(),
        };

        app.state.pending_recovery = match app.workspace.recovery_pending() {
            Ok(count) => count,
            Err(error) => {
                log::warn!("failed to read editor recovery snapshot: {error:#}");
                0
            }
        };

        app.workspace
            .log
            .set_capacity(app.preferences.max_log_lines as usize);

        if !app.workspace.document().doc.entities.is_empty() {
            app.workspace.frame_all();
        }
        app.write_preview_scene();
        app.ensure_view(ViewKind::Scene);

        if let Some(view) = app.scene_view.as_mut() {
            view.tool = restored_tool;
        }

        let started = Instant::now();
        app.ensure_view(ViewKind::Game);
        log::info!(
            "game view engine ready in {:.0} ms",
            started.elapsed().as_secs_f32() * 1000.0
        );
        app.synced_doc_state = Some(app.doc_sync_key());
        app.render_report = read_report(app.project_root.as_deref());
        app.workspace.log(
            prism_editor_core::LogSource::Editor,
            prism_editor_core::LogLevel::Info,
            "Prisma editor ready — Scene/Game viewports share the egui VkDevice (zero-copy)",
        );
        Ok(app)
    }

    pub(super) fn ensure_view(&mut self, kind: ViewKind) -> bool {
        let exists = match kind {
            ViewKind::Scene => self.scene_view.is_some(),
            ViewKind::Game => self.game_view.is_some(),
        };
        if exists {
            return true;
        }
        let size = (1280u32, 720u32);
        let created = ViewportState::new(
            kind,
            self.gpu.context.clone(),
            size.0,
            size.1,
            self.render_settings.shadow_map_size,
            self.render_settings.shadow_cascade_count,
        );
        match created {
            Ok(mut view) => {
                *view.host.settings_mut() = initial_view_settings(kind, &self.render_settings);

                if kind == ViewKind::Game {
                    apply_project_render_defaults(&mut view.host, self.project_root.as_deref());
                    view.project_resolution =
                        project_window_resolution(self.project_root.as_deref());

                    if let Some((width, height)) = view.project_resolution {
                        view.target = GameResolution::Fixed { width, height };
                    }
                }
                view.host.set_project_root(self.project_root.clone());
                view.host.set_show_skybox(self.workspace.show_skybox);

                if kind == ViewKind::Scene {
                    view.report_path = self
                        .project_root
                        .as_ref()
                        .map(|root| root.join("target/prism-render-graph.json"));
                }

                if kind == ViewKind::Game {
                    if let Some(root) = self.project_root.as_deref() {
                        view.host.attach_project_module(root);
                    }
                }
                if let Err(error) = view.host.load_scene(&self.preview_scene) {
                    let message = format!("load scene failed: {error}");
                    self.workspace.log(
                        prism_editor_core::LogSource::Editor,
                        prism_editor_core::LogLevel::Error,
                        message.clone(),
                    );
                    view.error = Some(message);
                }
                match kind {
                    ViewKind::Scene => self.scene_view = Some(view),
                    ViewKind::Game => self.game_view = Some(view),
                }
                true
            }
            Err(error) => {
                self.workspace.log(
                    prism_editor_core::LogSource::Editor,
                    prism_editor_core::LogLevel::Error,
                    format!("create {kind:?} view failed: {error:#}"),
                );
                false
            }
        }
    }

    pub(super) fn write_preview_scene(&mut self) {
        let text = self.workspace.document().doc.to_json_string();
        if let Err(error) = std::fs::write(&self.preview_scene, text) {
            log::warn!("write preview scene failed: {error}");
        }
    }

    pub(super) fn doc_sync_key(&self) -> (u64, u64, u64) {
        let document = self.workspace.document();
        (document.uid, document.revision, document.transform_revision)
    }

    /// Synchronize active workspace document to Scene view immediately (0ms).
    /// Transform-only revisions patch live ECS entities in place even during Play mode;
    /// structural or document switches reload the Scene World immediately without debouncing.
    pub(super) fn sync_document(&mut self) {
        let current = self.doc_sync_key();
        if self.synced_doc_state == Some(current) {
            return;
        }
        let structure_changed = match self.synced_doc_state {
            Some((uid, rev, _)) => uid != current.0 || rev != current.1,
            None => true,
        };
        if !structure_changed {
            let doc = &self.workspace.document().doc;
            if let Some(view) = self.scene_view.as_mut() {
                if view.host.sync_transforms_from_doc(doc) {
                    view.mark_dirty();
                    self.synced_doc_state = Some(current);
                    return;
                }
            }
        }
        if self.playing {
            return;
        }
        self.synced_doc_state = Some(current);
        self.write_preview_scene();
        if let Some(view) = self.scene_view.as_mut() {
            if let Err(error) = view.host.load_scene(&self.preview_scene) {
                view.error = Some(format!("{error}"));
            }
            view.mark_dirty();
        }
    }

    pub(super) fn refresh_object_bounds(&mut self) {
        let Some(view) = self.scene_view.as_ref() else {
            return;
        };
        let bounds = crate::viewport::bounds_table(&self.workspace, Some(&view.host));
        self.workspace.set_object_bounds(bounds);
    }

    pub(super) fn handle_audio_preview(&mut self) {
        if let Some(action) = self.state.pending_audio_preview.take() {
            match action {
                panels::AudioPreviewAction::Play(path) => self.play_audio_preview(&path),
                panels::AudioPreviewAction::Stop => self.stop_audio_preview(),
            }
        }

        let mut finished = false;
        if let Some(preview) = self.audio_preview.as_mut() {
            preview.engine.update();
            if let Some(handle) = preview.handle {
                if preview.engine.is_playing(&handle) {
                    preview.started = true;
                } else if preview.started {
                    preview.engine.stop(&handle);
                    preview.handle = None;
                    finished = true;
                }
            }
        }
        if finished {
            self.state.audio_preview_path = None;
            self.state.status = self.t("desktop.status.audioFinished");
        }
    }

    pub(super) fn play_audio_preview(&mut self, relative: &str) {
        let Some(root) = self.project_root.clone() else {
            self.state.status = self.t("desktop.status.audioNeedsRoot");
            return;
        };
        if self.audio_preview.is_none() {
            match prism_audio::AudioEngine::new(prism_audio::AudioConfig::default()) {
                Ok(engine) => {
                    self.audio_preview = Some(AudioPreview {
                        engine,
                        handle: None,
                        started: false,
                    });
                }
                Err(error) => {
                    self.state.status = self.tf(
                        "desktop.status.audioUnavailable",
                        &[("error", &error.to_string())],
                    );
                    return;
                }
            }
        }

        let result = {
            let preview = self.audio_preview.as_mut().expect("created above");
            if let Some(handle) = preview.handle.take() {
                preview.engine.stop(&handle);
            }
            let path = root.join(relative);
            match prism_audio::decoder::decode_file(&path) {
                Ok(data) => {
                    let handle = preview.engine.play(&data);
                    if handle.is_valid() {
                        preview.handle = Some(handle);
                        preview.started = false;
                        Ok(())
                    } else {
                        Err(format!("play {relative} failed"))
                    }
                }
                Err(error) => Err(format!("decode {relative} failed: {error}")),
            }
        };

        match result {
            Ok(()) => {
                self.state.audio_preview_path = Some(relative.to_string());
                self.state.status = self.tf("desktop.status.playing", &[("path", &relative)]);
            }
            Err(error) => {
                self.state.audio_preview_path = None;
                self.state.status = error;
            }
        }
    }

    pub(super) fn stop_audio_preview(&mut self) {
        if let Some(preview) = self.audio_preview.as_mut() {
            if let Some(handle) = preview.handle.take() {
                preview.engine.stop(&handle);
            }
            preview.started = false;
        }
        self.state.audio_preview_path = None;
        self.state.status = self.t("desktop.status.audioStopped");
    }

    pub(super) fn project_module_status(
        &self,
    ) -> Option<crate::project_module::ProjectModuleStatus> {
        self.game_view
            .as_ref()
            .and_then(|view| view.host.project_module_status())
    }

    pub(super) fn project_module_json(&self) -> serde_json::Value {
        use serde_json::json;
        match self.project_module_status() {
            Some(status) => json!({
                "available": status.available,
                "loaded": status.loaded,
                "building": status.building,
                "auto_refresh": status.auto_refresh,
                "state": status.state.as_str(),
                "dirty": status.dirty,
                "problems": status.problems,
                "manifest": status.manifest.as_ref().map(|path| path.display().to_string()),
                "error": status.error,
            }),
            None => serde_json::Value::Null,
        }
    }

    pub(super) fn poll_project_module(&mut self) {
        let playing = self.playing;
        let events = if let Some(view) = self.game_view.as_mut() {
            view.host.poll_project_module(playing);
            view.host.drain_project_events()
        } else {
            Vec::new()
        };
        for event in events {
            self.workspace.log(
                prism_editor_core::LogSource::System,
                event.level,
                event.text,
            );
        }
    }

    pub(super) fn refresh_project_module(&mut self) {
        let available = if let Some(view) = self.game_view.as_mut() {
            let available = view
                .host
                .project_module_status()
                .is_some_and(|status| status.available);
            if available {
                view.host.refresh_project_module();
            }
            available
        } else {
            false
        };
        if available {
            self.workspace.log(
                prism_editor_core::LogSource::System,
                prism_editor_core::LogLevel::Info,
                "project module: compile requested",
            );
            self.state.status = self.t("desktop.status.compilingModule");
        } else {
            self.state.status = self.t("desktop.status.noPreviewModule");
        }
    }

    pub(super) fn project_auto_refresh(&self) -> bool {
        self.game_view
            .as_ref()
            .map(|view| view.host.project_auto_refresh())
            .unwrap_or(true)
    }

    pub(super) fn set_project_auto_refresh(&mut self, enabled: bool) {
        if let Some(view) = self.game_view.as_mut() {
            view.host.set_project_auto_refresh(enabled);
        }
        self.workspace.log(
            prism_editor_core::LogSource::System,
            prism_editor_core::LogLevel::Info,
            format!(
                "project module: auto refresh {}",
                if enabled { "on" } else { "off" }
            ),
        );
    }

    pub(super) fn build_job_running(&self) -> bool {
        self.build
            .as_ref()
            .map(|job| job.running())
            .unwrap_or(false)
    }

    pub(super) fn cancel_build(&mut self) {
        let Some(job) = self.build.as_ref() else {
            return;
        };
        job.cancel();
        let label = job.label().to_string();
        self.state.status = self.tf("desktop.status.cancellingBuild", &[("label", &label)]);
        self.workspace.log(
            prism_editor_core::LogSource::System,
            prism_editor_core::LogLevel::Warn,
            format!("build cancel requested: {label}"),
        );
    }

    pub(super) fn start_build(&mut self, kind: crate::build::BuildKind) {
        let Some(root) = self.project_root.clone() else {
            self.state.status = self.t("desktop.status.noRootBuild");
            return;
        };

        if let Err(error) = self.save_all_documents() {
            self.state.status = self.tf(
                "desktop.status.buildAborted",
                &[("error", &error.to_string())],
            );
            self.workspace.log(
                prism_editor_core::LogSource::System,
                prism_editor_core::LogLevel::Error,
                format!("build aborted: save failed ({error})"),
            );
            return;
        }
        let label = kind.label(&self.workspace.i18n);
        self.workspace.log(
            prism_editor_core::LogSource::System,
            prism_editor_core::LogLevel::Info,
            format!("build started: {label}"),
        );
        if kind.is_player_build() {
            let settings = prism_editor_core::settings::ProjectSettings::read(&root)
                .unwrap_or_else(|error| {
                    self.workspace.log(
                        prism_editor_core::LogSource::System,
                        prism_editor_core::LogLevel::Warn,
                        format!("cannot read build settings ({error}); using defaults"),
                    );
                    Default::default()
                });
            let plan = prism_editor_core::build::plan_build(&root, &settings);
            self.workspace.log(
                prism_editor_core::LogSource::System,
                prism_editor_core::LogLevel::Info,
                format!(
                    "build plan: profile={} output_dir={}",
                    settings.build.profile,
                    plan.output_dir.display()
                ),
            );
        }
        self.build_diagnostics.clear();
        self.build = Some(crate::build::BuildJob::spawn(kind, &root, label));
    }

    pub(super) fn cook_active_scene(&mut self) {
        let Some(root) = self.project_root.clone() else {
            self.state.status = self.t("desktop.status.noRootCook");
            return;
        };

        let _ = self.workspace.save_active_if_dirty();
        let Some(path) = self.workspace.document().path.clone() else {
            self.state.status = self.t("desktop.status.docNoPath");
            return;
        };
        self.start_build(crate::build::BuildKind::Cook(root.join(&path)));
    }

    pub(super) fn autosave_state(&mut self) {
        if self.preferences.autosave
            && self.autosave_at.elapsed() > std::time::Duration::from_secs(5)
        {
            self.autosave_at = Instant::now();
            let mut state =
                prism_editor_core::EditorState::capture(&self.workspace, &self.preferences);
            if let Some(view) = self.scene_view.as_ref() {
                state.scene.gizmo_tool = match view.tool {
                    crate::viewport::GizmoTool::Translate => "Translate".to_string(),
                    crate::viewport::GizmoTool::Rotate => "Rotate".to_string(),
                    crate::viewport::GizmoTool::Scale => "Scale".to_string(),
                };
            }
            state.scene.render = Some(render_settings_json(&self.render_settings));
            if let Ok(text) = serde_json::to_string_pretty(&state) {
                if let Some(parent) = self.state_path.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                if let Err(error) = std::fs::write(&self.state_path, text) {
                    log::warn!("write editor state failed: {error}");
                }
            }
        }
    }

    pub(super) fn current_layout_json(&self) -> serde_json::Value {
        let layout = prism_editor_core::DockLayout {
            version: prism_editor_core::DockLayout::VERSION,
            dock: self.workspace.dock.clone(),
            floating: self.workspace.floating.clone(),
        };
        layout.to_json()
    }

    pub(super) fn write_named_layouts(&mut self) {
        if let Some(dir) = self.layouts_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let value = serde_json::json!(self.named_layouts);
        match std::fs::write(
            &self.layouts_path,
            serde_json::to_string_pretty(&value).unwrap_or_default(),
        ) {
            Ok(()) => {
                self.state.status = self.tf(
                    "desktop.status.layoutsSaved",
                    &[("path", &self.layouts_path.display().to_string())],
                )
            }
            Err(error) => {
                self.state.status = self.tf(
                    "desktop.status.layoutsSaveFailed",
                    &[("error", &error.to_string())],
                )
            }
        }
    }

    pub(super) fn apply_layout_json(&mut self, value: &serde_json::Value) {
        let known: Vec<String> = self
            .workspace
            .panels
            .iter()
            .map(|panel| panel.id.clone())
            .collect();
        let layout = prism_editor_core::DockLayout::from_json(value, &known, 1440.0, 900.0);
        self.workspace.dock = layout.dock;
        self.workspace.floating = layout.floating;

        self.workspace.maximized_tab = None;
    }

    pub(super) fn save_named_layout(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let value = self.current_layout_json();
        self.named_layouts.insert(name.to_string(), value);
        self.write_named_layouts();
    }

    pub(super) fn poll_loose_assets(&mut self) {
        if !self.preferences.hot_reload_assets {
            return;
        }
        let Some(root) = self.project_root.clone() else {
            return;
        };
        let now = Instant::now();
        if now < self.loose_scan_at {
            return;
        }
        self.loose_scan_at = now + std::time::Duration::from_secs(1);
        if self.build_job_running() {
            return;
        }
        let Some(watcher) = self.loose_assets.as_mut() else {
            return;
        };
        if !watcher.poll(&root, now) {
            return;
        }
        self.workspace.log(
            prism_editor_core::LogSource::System,
            prism_editor_core::LogLevel::Info,
            "loose assets changed — rebuilding the scene pack",
        );
        self.state.status = self.t("desktop.status.assetsChanged");
        self.start_build(crate::build::BuildKind::ScenePack);
    }

    pub(super) fn poll_build(&mut self) {
        let Some(job) = self.build.as_mut() else {
            return;
        };
        let (lines, running) = job.poll();
        let new_diagnostics = job.take_diagnostics();
        for line in lines {
            self.workspace.log(
                prism_editor_core::LogSource::System,
                prism_editor_core::LogLevel::Info,
                line,
            );
        }
        if !running {
            let cancelled = job.was_cancelled();

            let failed = job.failed() && !cancelled;
            let label = job.label().to_string();
            if cancelled {
                self.state.status = self.tf("desktop.status.buildCancelled", &[("label", &label)]);
                self.workspace.log(
                    prism_editor_core::LogSource::System,
                    prism_editor_core::LogLevel::Warn,
                    format!("build cancelled: {label}"),
                );
            } else if failed {
                self.state.status = self.tf("desktop.status.buildFailed", &[("label", &label)]);
                self.workspace.log(
                    prism_editor_core::LogSource::System,
                    prism_editor_core::LogLevel::Error,
                    format!("build failed: {label} (see Console)"),
                );
            } else {
                self.state.status = self.tf("desktop.status.buildFinished", &[("label", &label)]);
                self.workspace.log(
                    prism_editor_core::LogSource::System,
                    prism_editor_core::LogLevel::Info,
                    format!("build finished: {label}"),
                );
                if let Some(view) = self.scene_view.as_mut() {
                    view.invalidate_probe_cache();
                    view.host.invalidate_gi_cache();
                    view.mark_dirty();
                }
                if let Some(view) = self.game_view.as_mut() {
                    view.invalidate_probe_cache();
                    view.host.invalidate_gi_cache();
                    view.mark_dirty();
                }
            }
            self.build = None;

            if let (Some(root), Some(watcher)) =
                (self.project_root.as_deref(), self.loose_assets.as_mut())
            {
                watcher.resync(root);
            }
        }
        self.build_diagnostics.extend(new_diagnostics);
    }

    pub(super) fn drain_engine_logs(&mut self) {
        let entries = logging::drain();
        let mut saw_error = false;
        for (source, level, text) in entries {
            if matches!(
                level,
                prism_editor_core::LogLevel::Error | prism_editor_core::LogLevel::Panic
            ) {
                saw_error = true;
            }
            self.workspace.log.push(source, level, text);
        }

        if saw_error
            && self.state.log_error_pause
            && self.workspace.playing
            && !self.workspace.paused
        {
            self.apply_command(Command::Pause);
            self.state.status = self.t("desktop.status.errorPause");
            self.workspace.log(
                prism_editor_core::LogSource::Editor,
                prism_editor_core::LogLevel::Warn,
                "Error Pause: paused because an error was logged while playing",
            );
        }
    }

    pub(super) fn apply_render_settings(
        &mut self,
        settings: prism_engine::render_settings::RenderSettings,
    ) {
        self.render_settings = settings;

        if let Some(view) = self.scene_view.as_mut() {
            *view.host.settings_mut() = self.render_settings.clone();
            view.mark_dirty();
        }
    }

    pub fn render_state(
        &self,
    ) -> AshRenderState<Arc<std::sync::Mutex<gpu_allocator::vulkan::Allocator>>> {
        self.gpu.render_state()
    }

    pub(super) fn sync_animation_preview(&mut self) {
        use prism_engine::scene::systems::animator::SkeletonLibrary;

        let mut wanted_clips: Vec<String> = Vec::new();
        let mut wanted_skeletons: Vec<String> = Vec::new();
        for entity in &self.workspace.document().doc.entities {
            let components = &entity.components;
            if let Some(serde_json::Value::Object(animator)) =
                components.get("prism_engine::scene::Animator")
            {
                if let Some(path) = animator
                    .get("clip_path")
                    .and_then(serde_json::Value::as_str)
                {
                    if !path.is_empty() {
                        wanted_clips.push(path.to_string());
                    }
                }
            }
            if let Some(serde_json::Value::Object(root)) =
                components.get("prism_engine::scene::SkeletonRoot")
            {
                if let Some(path) = root
                    .get("skeleton_path")
                    .and_then(serde_json::Value::as_str)
                {
                    if !path.is_empty() {
                        wanted_skeletons.push(path.to_string());
                    }
                }
            }
        }
        wanted_clips.sort();
        wanted_clips.dedup();
        wanted_skeletons.sort();
        wanted_skeletons.dedup();

        let Some(library) = self
            .scene_view
            .as_ref()
            .and_then(|view| view.host.world().get_resource::<SkeletonLibrary>())
        else {
            return;
        };

        let durations: std::collections::HashMap<String, f32> = wanted_clips
            .iter()
            .filter_map(|path| library.clip(path).map(|clip| (path.clone(), clip.duration)))
            .collect();

        let keys: std::collections::HashMap<String, Vec<f32>> = wanted_clips
            .iter()
            .map(|path| (path.clone(), library.clip_key_times(path)))
            .collect();

        let notifies: std::collections::HashMap<String, Vec<(f32, String)>> = wanted_clips
            .into_iter()
            .map(|path| {
                let list = library.clip_notifies(&path);
                (path, list)
            })
            .collect();
        let skeletons: std::collections::HashMap<String, Vec<SkeletonJointRow>> = wanted_skeletons
            .into_iter()
            .filter_map(|path| {
                let skeleton = library.skeleton(&path)?;
                let joints = skeleton
                    .joints()
                    .iter()
                    .map(|joint| SkeletonJointRow {
                        name: joint.name.clone(),
                        parent: joint.parent,
                    })
                    .collect();
                Some((path, joints))
            })
            .collect();
        self.state.clip_durations = durations;
        self.state.clip_keys = keys;
        self.state.clip_notifies = notifies;
        self.state.skeleton_joints = skeletons;
    }
}

impl App for EditorApp {
    fn handle_event(&mut self, event: egui_host::event::Event<'_>) {
        let egui_host::event::Event::AppEvent {
            event: egui_host::event::AppEvent::LoopExiting,
        } = event
        else {
            return;
        };

        if self.force_quit {
            return;
        }
        match self.workspace.write_recovery() {
            Ok(0) => {}
            Ok(count) => {
                log::warn!("wrote recovery snapshot for {count} unsaved document(s) before exit")
            }
            Err(error) => log::error!("failed to write recovery snapshot: {error:#}"),
        }
    }

    fn ui(&mut self, ctx: &egui::Context) {
        self.frame_count += 1;
        self.state.build_running = self.build_job_running();

        self.refresh_object_bounds();

        if let Some(view) = self.scene_view.as_ref() {
            let (kept, total) = view.host.local_light_stats();
            self.state.lights_kept = kept;
            self.state.lights_total = total;
            let dropped = view.host.dropped_light_ids();
            if self.state.dropped_light_ids.as_slice() != dropped {
                self.state.dropped_light_ids = dropped.to_vec();
            }
        }

        if ctx.input(|input| input.pointer.any_released()) {
            self.workspace.end_interaction();
        }
        if let Some(action) = self.palette.show(ctx, &self.workspace) {
            self.run_palette_action(action);
        }
        self.poll_control(ctx);
        self.drain_engine_logs();

        if let Some(index) = self.workspace.pending_save_conflict.take() {
            self.state.pending_save_conflict = Some(index);
        }

        let focused = ctx.input(|input| input.focused);
        if focused && !self.was_focused {
            let changed = self.workspace.check_external_changes();
            if changed > 0 {
                self.state.status = format!("{changed} document(s) changed on disk");
            }

            self.workspace.refresh_project_files();
        }
        self.was_focused = focused;
        self.poll_build();
        self.poll_loose_assets();
        self.poll_project_module();
        self.sync_document();
        self.handle_shortcuts(ctx);

        if let Some(event) = self.pending_input.pop_front() {
            if let Some(view) = self.game_view.as_mut() {
                event.apply(view.host.input_mut());
            }
        }

        let mut re_resolved = 0usize;
        for view in [self.scene_view.as_mut(), self.game_view.as_mut()]
            .into_iter()
            .flatten()
        {
            let count = view.host.poll_asset_changes();
            if count > 0 {
                view.mark_dirty();
                re_resolved += count;
            }
        }
        if re_resolved > 0 {
            self.state.status = self.tf(
                "desktop.status.hotReloaded",
                &[("count", &re_resolved.to_string())],
            );
        }

        self.sync_animation_preview();

        if self.playing {
            if let Some(view) = self.game_view.as_mut() {
                view.host.tick();
                view.mark_dirty();
            }
        }

        let signature = format!(
            "{}:{}",
            self.workspace.theme.preset,
            self.workspace.theme.overrides.len()
        );
        if signature != self.theme_signature {
            self.theme_signature = signature;
            theme::apply(ctx, &self.workspace.colors());
        }

        let colors = self.workspace.colors();
        let chrome_fill = theme::chrome_fill(&colors);
        let panel_fill = theme::panel_fill(&colors);
        let bg_fill = theme::color32(&colors, prism_editor_core::ThemeToken::Bg);

        egui::TopBottomPanel::top("menu-bar")
            .exact_height(22.0)
            .frame(
                egui::Frame::new()
                    .fill(chrome_fill)
                    .inner_margin(egui::Margin::symmetric(4, 1)),
            )
            .show(ctx, |ui| self.menu_bar(ui, ctx));

        egui::TopBottomPanel::top("toolbar")
            .exact_height(34.0)
            .frame(
                egui::Frame::new()
                    .fill(panel_fill)
                    .inner_margin(egui::Margin::symmetric(8, 3)),
            )
            .show(ctx, |ui| self.toolbar(ui));

        egui::TopBottomPanel::bottom("status-bar")
            .exact_height(22.0)
            .frame(
                egui::Frame::new()
                    .fill(chrome_fill)
                    .inner_margin(egui::Margin::symmetric(6, 1)),
            )
            .show(ctx, |ui| self.status_bar(ui));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(bg_fill)
                    .inner_margin(egui::Margin::same(4)),
            )
            .show(ctx, |ui| self.dock_tree(ui));

        if let Some(kind) = self.state.pending_build.take() {
            self.start_build(kind);
        }

        self.handle_audio_preview();

        if self.playing {
            let color = panels::play_mode_tint(self.paused);
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("play-mode-frame"),
            ));
            painter.rect_stroke(
                ctx.viewport_rect().shrink(1.5),
                0.0,
                egui::Stroke::new(3.0_f32, color),
                egui::StrokeKind::Inside,
            );
        }

        self.finish_tab_drag(ctx);
        self.floating_panels(ctx);

        self.prompt_dialog(ctx);

        self.state.compact = self.preferences.compact;
        self.state.confirm_delete = self.preferences.confirm_delete;
        panels::entity_delete_confirm(ctx, &mut self.workspace, &mut self.state);
        self.confirm_dialog(ctx);
        self.close_confirm_dialog(ctx);
        self.save_conflict_dialog(ctx);
        self.recovery_dialog(ctx);
        panels::doc_delete_confirm(ctx, &mut self.workspace, &mut self.state);

        let mut about = self.state.about_open;
        let about_title = self.t("menu.about");
        panels::boolean_window(ctx, &mut about, &about_title, |ui| {
            panels::about(ui, &self.workspace)
        });
        self.state.about_open = about;

        let mut shortcuts = self.state.shortcuts_open;
        let shortcuts_title = self.t("menu.shortcuts");
        panels::boolean_window(ctx, &mut shortcuts, &shortcuts_title, |ui| {
            panels::shortcuts(ui, &self.workspace, &mut self.preferences, &mut self.state);
        });
        self.state.shortcuts_open = shortcuts;
        if !self.state.shortcuts_open {
            self.state.shortcut_capture = None;
        }

        if ctx.input(|input| input.viewport().close_requested()) {
            if !self.force_quit
                && self
                    .workspace
                    .docs
                    .iter()
                    .any(|document| document.is_dirty())
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.state.pending_close = Some(panels::CloseTarget::App);
            } else {
                self.exit_signal.send(std::process::ExitCode::SUCCESS);
            }
        }

        self.track_selection_history();
        self.autosave_state();
        let _ = self.last_frame;
        ctx.request_repaint_after(std::time::Duration::from_millis(if self.playing {
            16
        } else {
            250
        }));
    }

    fn request_redraw(&mut self, _viewport_id: egui::ViewportId) -> HandleRedraw {
        HandleRedraw::Auto
    }
}

#[cfg(test)]
fn document_hash(workspace: &Workspace) -> u64 {
    let text = workspace.document().doc.to_json_string();
    fnv1a(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::dock_ui::*;

    #[test]
    fn stable_tab_titles_keep_the_feature_map_in_sync() {
        assert_eq!(tab_title("sceneView"), "Scene");
        assert_eq!(tab_title("renderGraph"), "Render Graph");
        assert_eq!(tab_title("prefabOverrides"), "Prefab Overrides");
        assert_eq!(tab_title("unknown"), "unknown");
    }
    use prism_editor_core::{KeyChord, SHORTCUTS, SHORTCUT_ALIASES};

    /// The inspector must see every component the scene registry knows about.
    /// A frozen schema snapshot used to hide 19 of them from Add Component.
    #[test]
    fn runtime_schema_covers_every_registered_component() {
        let mut registry = prism_scene::ComponentRegistry::new();
        prism_scene::register_builtin_components(&mut registry);
        let schema = runtime_schema();
        let missing: Vec<&str> = registry
            .component_names()
            .into_iter()
            .filter(|name| schema.component(name).is_none())
            .collect();
        assert!(missing.is_empty(), "not in the editor schema: {missing:?}");
    }

    #[test]
    fn first_open_seeds_scene_view_from_project_settings() {
        let mut project = prism_engine::render_settings::RenderSettings::default();
        project.tonemap_mode = 2;
        project.bloom_intensity = 0.75;

        let seeded = seeded_render_settings(None, Some(project.clone()), true);
        assert_eq!(seeded.tonemap_mode, 2);
        assert!((seeded.bloom_intensity - 0.75).abs() < 1e-6);
        assert!(seeded.show_grid, "grid still editor switch");

        let restored = serde_json::json!({ "tonemapMode": 0, "showGrid": false });
        let seeded = seeded_render_settings(Some(&restored), Some(project.clone()), true);
        assert_eq!(seeded.tonemap_mode, 0);
        assert!(!seeded.show_grid);

        let seeded = seeded_render_settings(None, None, false);
        assert_eq!(
            seeded.tonemap_mode,
            prism_engine::render_settings::RenderSettings::default().tonemap_mode
        );
    }

    #[test]
    fn project_render_settings_reads_the_ini_section() {
        let dir = std::env::temp_dir().join(format!("prism-app-979-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(
            dir.join("assets/settings.ini"),
            "[render]
tonemap_mode = 2
bloom_intensity = 0.5
",
        )
        .unwrap();
        let settings = project_render_settings(Some(&dir)).expect("settings.ini");
        assert_eq!(settings.tonemap_mode, 2);
        assert!((settings.bloom_intensity - 0.5).abs() < 1e-6);

        let empty =
            std::env::temp_dir().join(format!("prism-app-979-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&empty);
        std::fs::create_dir_all(&empty).unwrap();
        assert!(project_render_settings(Some(&empty)).is_none());
        assert!(project_render_settings(None).is_none());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn the_scene_overlay_stays_inside_the_viewport() {
        let viewport = Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(800.0, 600.0));
        let expanded = scene_overlay_rect(viewport, false);
        assert_eq!(expanded.min, egui::pos2(18.0, 28.0));
        assert!(expanded.width() > 200.0);
        assert!(viewport.contains(expanded.min) && viewport.contains(expanded.max));

        let collapsed = scene_overlay_rect(viewport, true);
        assert!(collapsed.width() < expanded.width());

        let tiny = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(40.0, 40.0));
        let rect = scene_overlay_rect(tiny, false);
        assert!(rect.width() <= tiny.width());
        assert!(tiny.contains(rect.min) && tiny.contains(rect.max));
    }

    fn rect() -> Rect {
        Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(400.0, 300.0))
    }

    #[test]
    fn shortcut_overrides_change_the_consumed_binding() {
        let mut overrides = std::collections::BTreeMap::new();
        overrides.insert(Command::Play.id().to_string(), "Ctrl+Shift+P".to_string());
        let table = editor_shortcuts(&overrides);
        let (play, _) = table
            .iter()
            .find(|(_, command)| *command == Command::Play)
            .expect("play binding");
        assert_eq!(play.logical_key, egui::Key::P);
        assert!(play.modifiers.ctrl && play.modifiers.shift);

        assert!(!table.iter().any(|(shortcut, command)| {
            *command == Command::Play
                && shortcut.logical_key == egui::Key::P
                && shortcut.modifiers == egui::Modifiers::CTRL
        }));

        assert!(table.iter().any(|(_, command)| *command == Command::Save));
    }

    #[test]
    fn every_listed_shortcut_maps_to_a_key() {
        for entry in SHORTCUTS.iter().chain(SHORTCUT_ALIASES.iter()) {
            let chord = KeyChord::parse(entry.keys)
                .unwrap_or_else(|| panic!("{:?} key {:?} cannot parse", entry.command, entry.keys));
            assert!(
                key_for(&chord.key).is_some(),
                "{:?} key name {:?} has no corresponding key in egui",
                entry.command,
                chord.key
            );
        }
        assert_eq!(
            editor_shortcuts(&Default::default()).len(),
            SHORTCUTS.len() + SHORTCUT_ALIASES.len(),
            "shortcut table should cover whole table (incl. aliases)"
        );
    }

    #[test]
    fn shifted_chords_do_not_fall_back_to_their_base_key() {
        let table = editor_shortcuts(&Default::default());

        for pair in table.windows(2) {
            let modifiers = |shortcut: &egui::KeyboardShortcut| {
                u8::from(shortcut.modifiers.ctrl)
                    + u8::from(shortcut.modifiers.shift)
                    + u8::from(shortcut.modifiers.alt)
            };
            assert!(
                modifiers(&pair[0].0) >= modifiers(&pair[1].0),
                "sort must be by modifier count descending, otherwise Ctrl+Shift+S eaten by Ctrl+S"
            );
        }

        let mut input = egui::InputState::default();
        input.events.push(egui::Event::Key {
            key: egui::Key::S,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
        });
        let save = egui::KeyboardShortcut::new(egui::Modifiers::CTRL, egui::Key::S);
        let save_as = egui::KeyboardShortcut::new(
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            egui::Key::S,
        );
        assert!(
            !consume_shortcut_exact(&mut input, &save),
            "Ctrl+Shift+S cannot trigger Save"
        );
        assert!(
            consume_shortcut_exact(&mut input, &save_as),
            "Ctrl+Shift+S must trigger Save As"
        );
        assert!(input.events.is_empty(), "events consumed after hit");

        input.events.push(egui::Event::Key {
            key: egui::Key::S,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL,
        });
        assert!(!consume_shortcut_exact(&mut input, &save_as));
        assert!(consume_shortcut_exact(&mut input, &save));
    }

    #[test]
    fn the_shortcut_table_has_no_duplicate_commands() {
        let mut seen: Vec<Command> = Vec::new();
        for entry in SHORTCUTS {
            assert!(
                !seen.contains(&entry.command),
                "{:?} appears twice in SHORTCUTS",
                entry.command
            );
            seen.push(entry.command);
        }
    }

    #[test]
    fn every_table_command_is_dispatched() {
        let table = editor_shortcuts(&Default::default());
        for entry in SHORTCUTS {
            assert!(
                table.iter().any(|(_, command)| *command == entry.command),
                "{:?} in table but not in shortcut table",
                entry.command
            );
        }
    }

    #[test]
    fn typing_in_a_text_field_keeps_the_bare_shortcuts() {
        assert!(plain_shortcut(Command::FrameSelection), "F is bare key");
        assert!(
            plain_shortcut(Command::DeleteSelection),
            "Delete is bare key"
        );
        assert!(plain_shortcut(Command::SelectAll));

        assert!(!plain_shortcut(Command::Save));
        assert!(!plain_shortcut(Command::Play));
    }

    #[test]
    fn select_all_is_ctrl_a() {
        let table = editor_shortcuts(&Default::default());
        let (shortcut, _) = table
            .iter()
            .find(|(_, command)| *command == Command::SelectAll)
            .expect("Select All must be bound");
        assert_eq!(shortcut.modifiers, egui::Modifiers::CTRL);
        assert_eq!(shortcut.logical_key, egui::Key::A);
        assert!(
            !table
                .iter()
                .any(|(entry, _)| entry.modifiers == egui::Modifiers::NONE
                    && entry.logical_key == egui::Key::A),
            "bare A turns typed chars in input box into select-all-entities"
        );
    }

    #[test]
    fn a_crowded_tab_bar_falls_back_to_overflow() {
        assert!(!tab_bar_needs_overflow(120.0, 400.0));
        assert!(!tab_bar_needs_overflow(399.0, 400.0));

        assert!(tab_bar_needs_overflow(400.5, 400.0));
        assert!(tab_bar_needs_overflow(900.0, 300.0));
    }

    #[test]
    fn the_active_tab_stays_visible_in_a_crowded_bar() {
        let widths = [100.0_f32; 8];

        assert_eq!(visible_tab_range(&widths, 900.0, 5), (0, 8));

        assert_eq!(visible_tab_range(&widths, 320.0, 1), (0, 3));

        let (from, to) = visible_tab_range(&widths, 320.0, 5);
        assert_eq!(to, 6, "active tab must be inside the window");
        assert!(from <= 5);
        assert_eq!(visible_tab_range(&widths, 250.0, 7), (6, 8));

        assert_eq!(visible_tab_range(&widths, 10.0, 4), (4, 5));

        assert_eq!(visible_tab_range(&[], 100.0, 0), (0, 0));
    }

    #[test]
    fn game_view_never_inherits_the_editor_debug_toggles() {
        use prism_engine::render_settings::{RenderMode, RenderSettings};
        let editor = RenderSettings {
            show_grid: true,
            debug_flags: 0b1010,
            debug_rt: 2,
            wireframe: true,
            shaded_wireframe: true,
            render_mode: RenderMode::PathTrace,
            tonemap_mode: 2,
            ..Default::default()
        };

        let game = initial_view_settings(ViewKind::Game, &editor);
        assert!(!game.show_grid, "grid is a scene-view helper");
        assert_eq!(game.debug_flags, 0, "PBR isolate is a scene-view helper");
        assert_eq!(game.debug_rt, 0, "debug RT is a scene-view helper");
        assert!(!game.wireframe, "wireframe is a scene-view helper");
        assert!(
            !game.shaded_wireframe,
            "shaded wireframe is a scene-view helper"
        );

        let scene = initial_view_settings(ViewKind::Scene, &editor);
        assert!(scene.show_grid);
        assert_eq!(scene.debug_flags, 0b1010);
        assert_eq!(scene.debug_rt, 2);
        assert!(scene.wireframe);
        assert!(scene.shaded_wireframe);
    }

    #[test]
    fn dock_zone_uses_quarter_bands() {
        use prism_editor_core::dock::DockZone;
        let rect = rect();
        assert_eq!(dock_zone(rect, egui::pos2(110.0, 250.0)), DockZone::Left);
        assert_eq!(dock_zone(rect, egui::pos2(490.0, 250.0)), DockZone::Right);
        assert_eq!(dock_zone(rect, egui::pos2(300.0, 110.0)), DockZone::Top);
        assert_eq!(dock_zone(rect, egui::pos2(300.0, 390.0)), DockZone::Bottom);
        assert_eq!(dock_zone(rect, egui::pos2(300.0, 250.0)), DockZone::Center);
    }

    #[test]
    fn transform_paths_split_only_supported_fields_and_axes() {
        assert_eq!(split_axis("position[2]"), Some(("position", 2)));
        assert_eq!(split_axis("rotation"), Some(("rotation", 0)));
        assert_eq!(split_axis("scale[bad]"), None);
        assert_eq!(split_axis("name"), None);
    }

    #[test]
    fn render_settings_round_trip_and_old_state_uses_grid_fallback() {
        let mut settings = prism_engine::render_settings::RenderSettings::default();
        settings.show_grid = true;
        settings.pt_max_bounces = 7;
        settings.pt_ray_max_distance = 123.5;

        settings.contrast = 1.4;
        settings.saturation = 0.6;
        settings.vignette = 0.3;
        settings.bloom = true;
        settings.bloom_intensity = 0.4;
        settings.bloom_threshold = 2.5;
        settings.chromatic_aberration = 0.25;
        settings.film_grain = 0.45;
        settings.temperature = -0.35;
        settings.tint = 0.15;
        settings.dof_focus_distance = 4.5;
        settings.dof_aperture = 0.55;
        settings.dof_max_blur = 10.0;
        settings.lut = "luts/grade.cube".to_string();
        settings.lut_strength = 0.8;
        settings.motion_blur = 0.65;
        settings.motion_blur_max_pixels = 20.0;
        settings.taa = true;
        settings.taa_strength = 0.85;
        settings.shadow_distance = 90.0;
        settings.shadow_cascade_count = 2;
        settings.shaded_wireframe = true;
        let value = render_settings_json(&settings);
        let loaded = render_settings_from_json(&value, false);
        assert_eq!(loaded.show_grid, true);
        assert_eq!(loaded.pt_max_bounces, 7);
        assert_eq!(loaded.pt_ray_max_distance, 123.5);
        assert_eq!(loaded.contrast, 1.4);
        assert_eq!(loaded.saturation, 0.6);
        assert_eq!(loaded.vignette, 0.3);
        assert!(loaded.bloom);
        assert_eq!(loaded.bloom_intensity, 0.4);
        assert_eq!(loaded.bloom_threshold, 2.5);
        assert_eq!(loaded.chromatic_aberration, 0.25);
        assert_eq!(loaded.film_grain, 0.45);
        assert_eq!(loaded.temperature, -0.35);
        assert_eq!(loaded.tint, 0.15);
        assert_eq!(loaded.dof_focus_distance, 4.5);
        assert_eq!(loaded.dof_aperture, 0.55);
        assert_eq!(loaded.dof_max_blur, 10.0);
        assert_eq!(loaded.lut, "luts/grade.cube");
        assert_eq!(loaded.lut_strength, 0.8);
        assert_eq!(loaded.motion_blur, 0.65);
        assert_eq!(loaded.motion_blur_max_pixels, 20.0);
        assert!(loaded.taa);
        assert_eq!(loaded.taa_strength, 0.85);
        assert_eq!(loaded.shadow_distance, 90.0);
        assert_eq!(loaded.shadow_cascade_count, 2);
        assert!(loaded.shaded_wireframe);
        assert_eq!(
            render_settings_from_json(&serde_json::json!({}), true).show_grid,
            true
        );
    }

    #[test]
    fn workspace_hashes_change_with_document_and_selection() {
        let mut workspace = Workspace::sample();
        let document_before = document_hash(&workspace);
        let selection_before = selection_hash(&workspace);
        workspace.add_entity("Changed");
        assert_ne!(document_hash(&workspace), document_before);
        workspace.select(0, false);
        assert_ne!(selection_hash(&workspace), selection_before);
    }

    #[test]
    fn game_view_reads_the_project_window_resolution() {
        let root = std::env::temp_dir().join(format!(
            "prisma-editor-window-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(
            root.join("assets/settings.ini"),
            "[window]\nwidth = 1600\nheight = 900\n",
        )
        .unwrap();
        assert_eq!(project_window_resolution(Some(&root)), Some((1600, 900)));
        std::fs::remove_dir_all(root).unwrap();
        assert_eq!(project_window_resolution(None), None);
    }

    #[test]
    fn audio_preview_can_decode_and_start_a_wav() {
        let sample_rate = 44_100u32;
        let samples: Vec<i16> = (0..64)
            .map(|index| if index % 2 == 0 { 1200 } else { -1200 })
            .collect();
        let data_len = (samples.len() * 2) as u32;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); 
        wav.extend_from_slice(&1u16.to_le_bytes()); 
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes()); 
        wav.extend_from_slice(&16u16.to_le_bytes()); 
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for sample in samples {
            wav.extend_from_slice(&sample.to_le_bytes());
        }

        let path = std::env::temp_dir().join(format!(
            "prism-editor-audio-preview-{}.wav",
            std::process::id()
        ));
        std::fs::write(&path, &wav).unwrap();
        let clip = prism_audio::decoder::decode_file(&path).expect("wav decodes");
        assert_eq!(clip.channels, 1);
        assert!(clip.duration > std::time::Duration::ZERO);

        let mut engine =
            prism_audio::AudioEngine::new(prism_audio::AudioConfig::default()).expect("engine");
        let handle = engine.play(&clip);
        assert!(handle.is_valid(), "preview should start a real voice");
        engine.stop(&handle);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn dock_compass_zones_place_five_directional_pads_around_center() {
        use prism_editor_core::dock::DockZone;
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0));
        let zones = dock_compass_zones(rect);
        assert_eq!(zones.len(), 5);
        assert_eq!(zones[0].0, DockZone::Center);
        assert!(zones[0].1.contains(rect.center()));
        for i in 0..zones.len() {
            for j in (i + 1)..zones.len() {
                assert!(
                    !zones[i].1.intersects(zones[j].1),
                    "compass pads {:?} and {:?} must not overlap",
                    zones[i].0,
                    zones[j].0
                );
            }
        }
        assert!(zones[1].1.center().x < zones[0].1.center().x);
        assert!(zones[2].1.center().x > zones[0].1.center().x);
        assert!(zones[3].1.center().y < zones[0].1.center().y);
        assert!(zones[4].1.center().y > zones[0].1.center().y);
    }

    #[test]
    fn studio_theme_preset_maintains_dark_contrast() {
        use prism_editor_core::{ThemeColors, ThemeToken};
        let studio = ThemeColors::preset("studio");
        let sunken = studio.color(ThemeToken::BgSunken);
        let panel = studio.color(ThemeToken::Panel);
        let panel_alt = studio.color(ThemeToken::PanelAlt);
        let text = studio.color(ThemeToken::Text);
        let accent = studio.color(ThemeToken::Accent);
        assert!(sunken.r < panel.r && sunken.g < panel.g && sunken.b < panel.b);
        assert!(panel.r < panel_alt.r && panel.g < panel_alt.g && panel.b < panel_alt.b);
        assert!(text.r > 180 && text.g > 180 && text.b > 180);
        assert_eq!((accent.r, accent.g, accent.b), (59, 130, 246));
    }

    #[test]
    fn panel_icon_maps_core_tabs() {
        let icons_list = [
            panel_icon("sceneView"),
            panel_icon("gameView"),
            panel_icon("hierarchy"),
            panel_icon("inspector"),
            panel_icon("assets"),
            panel_icon("log"),
        ];
        assert!(icons_list.iter().all(|glyph| !glyph.is_empty()));
        assert_ne!(panel_icon("sceneView"), panel_icon("hierarchy"));
        assert_ne!(panel_icon("gameView"), panel_icon("inspector"));
    }
}
