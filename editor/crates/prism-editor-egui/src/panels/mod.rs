//! Editor panels: Hierarchy, Inspector, Assets, AnimGraph, Material, Console, RenderGraph, and Settings.

mod anim_graph;
mod assets;
mod console;
mod hierarchy;
mod inspector;
mod material;
mod render_graph;
mod settings_and_tools;
#[cfg(test)]
mod tests;

pub use anim_graph::{anim_graph, AnimGraphEditState, AnimRuntimeView, StateConditionDraft};
pub use assets::{
    apply_pending_asset_delete, apply_pending_doc_delete, assets, delete_asset_now,
    doc_delete_confirm, docs, request_delete_assets, AssetColumns, AssetView,
};
pub use console::{history, log, problems};
pub use hierarchy::hierarchy;
pub(crate) use hierarchy::reveal_in_hierarchy;
pub use inspector::{inspector, play_mode_tint, prefab_overrides};
pub use material::material;
pub use render_graph::{render_graph, scene_render_mode_combo};
pub use settings_and_tools::{
    about, apply_pending_entity_delete, boolean_window, build, entity_delete_confirm, preferences,
    project_settings, shortcuts, theme, tools,
};

use egui::{Align, Color32, Layout, RichText, Sense, Ui, Vec2};
use prism_editor_core::settings::{
    matching_fields, SettingsCache, PROFILE_CHOICES, TARGET_CHOICES,
};
use prism_editor_core::{
    format_bytes, AssetKind, AssetPreview, EntityJson, Locale, LogLevel, LogSource, Preferences,
    ProjectSettings, PrtDisplayMode, PrtHeaderInfo, PrtProbeVolume, SceneDoc, Workspace,
    THEME_PRESETS,
};
use prism_engine::scene::systems::terrain::SculptMode;
use serde_json::{json, Value};

use crate::viewport::{set_terrain_brush_settings, terrain_brush_settings};
use crate::{diagnostics::Diagnostic, theme, widgets};
use egui_phosphor::regular as icons;

pub fn reveal_asset(workspace: &Workspace, path: &str) -> Result<(), String> {
    let root = workspace
        .project_root
        .clone()
        .ok_or_else(|| "no project root; cannot reveal assets".to_string())?;
    let absolute = root.join(path);
    if !absolute.exists() {
        return Err(format!("{} does not exist", absolute.display()));
    }

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("explorer");
        command.arg(format!("/select,{}", absolute.display()));
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg("-R").arg(&absolute);
        command
    };
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(absolute.parent().unwrap_or(absolute.as_path()));
        command
    };

    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("cannot open file manager: {error}"))
}

pub(super) fn asset_kind_icon(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Model => icons::CUBE,
        AssetKind::Texture => icons::IMAGE,
        AssetKind::Env => icons::GLOBE,
        AssetKind::Scene => icons::TREE_STRUCTURE,
        AssetKind::Prefab => icons::PACKAGE,
        AssetKind::Package => icons::ARCHIVE_BOX,
        AssetKind::Material => icons::PALETTE,
        AssetKind::Audio => icons::MUSIC_NOTES,
        AssetKind::Font => icons::TEXT_AA,
        AssetKind::Data => icons::FILE_TEXT,
        AssetKind::Other => icons::FILE,
    }
}

pub fn tree_row_height(compact: bool) -> f32 {
    if compact {
        15.0
    } else {
        18.0
    }
}

pub fn row_height(compact: bool) -> f32 {
    if compact {
        16.0
    } else {
        20.0
    }
}

pub(super) fn list_row(
    ui: &mut Ui,
    colors: &prism_editor_core::ThemeColors,
    selected: bool,
    compact: bool,
    text: &str,
    thumbnail: Option<(egui::TextureId, [u32; 2])>,
) -> egui::Response {
    use prism_editor_core::ThemeToken;
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), row_height(compact)),
        egui::Sense::click(),
    );
    if ui.is_rect_visible(rect) {
        let bg = if selected {
            Some(theme::color32(colors, ThemeToken::Selection))
        } else if response.hovered() {
            Some(theme::color32(colors, ThemeToken::RowHover))
        } else {
            None
        };
        if let Some(bg) = bg {
            ui.painter().rect_filled(rect, 0.0, bg);
        }
    }
    let text_color = theme::color32(
        colors,
        if selected {
            ThemeToken::OnAccent
        } else {
            ThemeToken::Text
        },
    );

    let mut text_left = rect.left() + 6.0;
    if let Some((texture, [width, height])) = thumbnail {
        let side = (row_height(compact) - 4.0).max(8.0);
        let image_rect = fit_into(
            egui::Rect::from_min_size(
                egui::pos2(rect.left() + 3.0, rect.center().y - side * 0.5),
                egui::vec2(side, side),
            ),
            width,
            height,
        );
        ui.painter().image(
            texture,
            image_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        text_left += side + 2.0;
    }
    ui.painter().text(
        egui::pos2(text_left, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(11.5),
        text_color,
    );
    response
}

pub(super) fn fit_into(bounds: egui::Rect, width: u32, height: u32) -> egui::Rect {
    let width = width.max(1) as f32;
    let height = height.max(1) as f32;
    let scale = (bounds.width() / width)
        .min(bounds.height() / height)
        .max(0.0);
    let size = egui::vec2(width * scale, height * scale);
    egui::Rect::from_center_size(bounds.center(), size)
}

pub(super) fn thumbnail_texture(
    ui: &Ui,
    workspace: &Workspace,
    state: &mut UiState,
    path: &str,
    kind: AssetKind,
) -> Option<(egui::TextureId, [u32; 2])> {
    thumbnail_image(ui, workspace, state, path, kind).map(|image| (image.texture, image.size))
}

pub(super) struct ThumbnailImage {
    texture: egui::TextureId,

    size: [u32; 2],

    source: [u32; 2],
}

pub(super) fn thumbnail_image(
    ui: &Ui,
    workspace: &Workspace,
    state: &mut UiState,
    path: &str,
    kind: AssetKind,
) -> Option<ThumbnailImage> {
    if !matches!(kind, AssetKind::Texture | AssetKind::Env) {
        return None;
    }
    let root = workspace.project_root.as_deref()?;
    if let Some(handle) = state.thumbnail_textures.get(path) {
        let handle_size = handle.size();
        let size = [handle_size[0] as u32, handle_size[1] as u32];
        return Some(ThumbnailImage {
            texture: handle.id(),
            size,
            source: state.thumbnail_sources.get(path).copied().unwrap_or(size),
        });
    }
    if state.thumbnail_budget == 0 {
        return None;
    }
    state.thumbnail_budget = state.thumbnail_budget.saturating_sub(1);
    let thumbnail =
        state
            .thumbnails
            .get(root, path, prism_editor_core::thumbnails::DEFAULT_MAX_SIDE)?;
    let size = [thumbnail.width, thumbnail.height];
    let (source_width, source_height) = thumbnail.source_size();
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [thumbnail.width as usize, thumbnail.height as usize],
        &thumbnail.rgba,
    );
    let handle = ui.ctx().load_texture(
        format!("asset-thumb:{path}"),
        image,
        egui::TextureOptions::LINEAR,
    );
    let id = handle.id();
    state.thumbnail_textures.insert(path.to_string(), handle);
    state
        .thumbnail_sources
        .insert(path.to_string(), [source_width, source_height]);
    Some(ThumbnailImage {
        texture: id,
        size,
        source: [source_width, source_height],
    })
}

pub(super) fn flat_icon(ui: &mut Ui, glyph: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(glyph).size(11.0)).frame(false))
}

pub(super) fn flat_icon_dimmed(ui: &mut Ui, glyph: &str, dimmed: bool) -> egui::Response {
    let mut text = RichText::new(glyph).size(11.0);
    if dimmed {
        text = text.weak();
    }
    ui.add(egui::Button::new(text).frame(false))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AssetSort {
    #[default]
    Name,
    Kind,
    Size,
}

pub fn sort_assets(files: &mut [(String, AssetKind, u64)], sort: AssetSort, descending: bool) {
    files.sort_by(|a, b| {
        let name = |entry: &(String, AssetKind, u64)| {
            entry
                .0
                .rsplit('/')
                .next()
                .unwrap_or(&entry.0)
                .to_lowercase()
        };
        let primary = match sort {
            AssetSort::Name => std::cmp::Ordering::Equal,
            AssetSort::Kind => (a.1 as u8).cmp(&(b.1 as u8)),
            AssetSort::Size => a.2.cmp(&b.2),
        };
        let order = primary
            .then_with(|| name(a).cmp(&name(b)))
            .then_with(|| a.0.cmp(&b.0));
        if descending {
            order.reverse()
        } else {
            order
        }
    });
}

pub(super) fn rename_target(from: &str, file_name: &str) -> Option<String> {
    let file_name = file_name.trim();
    if file_name.is_empty() || file_name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
        return None;
    }
    match from.rsplit_once('/') {
        Some((folder, _)) => Some(format!("{folder}/{file_name}")),
        None => Some(file_name.to_string()),
    }
}

#[derive(Debug, Clone)]
pub struct OfflineToolsState {
    pub gi_output: String,
    pub gi_rays: u32,
    pub gi_bounces: u32,
    pub heightmap_output: String,
    pub heightmap_width: u32,
    pub heightmap_height: u32,
    pub heightmap_seed: u64,
    pub heightmap_iterations: u32,
    pub heightmap_sea_level: f32,
    pub pack_output: String,
    pub pack_compression: u32,
}

impl Default for OfflineToolsState {
    fn default() -> Self {
        Self {
            gi_output: "assets/gi/probe_volume.bin".to_string(),
            gi_rays: 64,
            gi_bounces: 3,
            heightmap_output: "assets/heightmaps/terrain.raw".to_string(),
            heightmap_width: 512,
            heightmap_height: 512,
            heightmap_seed: 0x5EED,
            heightmap_iterations: 100,
            heightmap_sea_level: 0.7,
            pack_output: "assets/scenes.pak".to_string(),
            pack_compression: 3,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HierarchySearchMode {
    #[default]
    Name,

    Component,
}

impl HierarchySearchMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Component => "Type",
        }
    }

    fn matches(self, entity: &EntityJson, filter: &str) -> bool {
        match self {
            Self::Name => entity.display_name().to_lowercase().contains(filter),
            Self::Component => entity.components.keys().any(|component| {
                let short = component.rsplit("::").next().unwrap_or(component);
                short.to_lowercase().contains(filter)
            }),
        }
    }
}

#[derive(Default, Debug, Clone, PartialEq)]
pub struct SkeletonJointRow {
    pub name: String,
    pub parent: Option<usize>,
}

#[derive(Default)]
pub struct UiState {
    pub inspector_search: String,

    pub clip_durations: std::collections::HashMap<String, f32>,

    pub clip_keys: std::collections::HashMap<String, Vec<f32>>,

    pub clip_notifies: std::collections::HashMap<String, Vec<(f32, String)>>,

    pub skeleton_joints: std::collections::HashMap<String, Vec<SkeletonJointRow>>,

    pub inspector_lock: Option<Vec<u32>>,
    pub hierarchy_search: String,

    pub hierarchy_search_mode: HierarchySearchMode,

    pub hierarchy_anchor: Option<usize>,

    pub hierarchy_lock: Option<Vec<u32>>,

    pub hierarchy_reveal: Option<u32>,
    pub asset_search: String,

    pub history_latest_first: bool,

    pub viewport_scale: f32,

    pub tilemap_palette: u32,

    pub scene_overlay_collapsed: bool,

    pub shortcut_capture: Option<prism_editor_core::Command>,

    pub asset_view: AssetView,

    pub asset_columns: AssetColumns,
    pub log_levels: Vec<LogLevel>,
    pub log_sources: Vec<LogSource>,
    pub log_search: String,
    pub log_autoscroll: bool,

    pub log_collapse: bool,

    pub log_clear_on_play: bool,

    pub log_error_pause: bool,

    pub settings_search: String,
    pub settings_cache: Option<SettingsCache>,
    pub rename: Option<(usize, String)>,

    pub rename_base: Option<(SceneDoc, bool)>,

    pub name_edit: Option<(usize, String)>,

    pub rename_focus: bool,
    pub add_component_query: String,
    pub preset_choice: usize,
    pub status: String,
    pub about_open: bool,

    pub prompt: Option<Prompt>,

    pub asset_details: Option<(String, prism_editor_core::AssetDetails)>,
    pub asset_details_at: Option<std::time::Instant>,

    pub thumbnails: prism_editor_core::thumbnails::ThumbnailCache,
    pub thumbnail_textures: std::collections::HashMap<String, egui::TextureHandle>,

    pub thumbnail_sources: std::collections::HashMap<String, [u32; 2]>,

    pub thumbnail_budget: u32,

    pub anim_graph_layout: Option<(String, prism_editor_core::AnimGraphLayout)>,

    pub anim_graph_edit: AnimGraphEditState,

    pub anim_sm_condition: Option<StateConditionDraft>,
    pub asset_selection: Vec<String>,

    pub asset_anchor: Option<String>,

    pub asset_folder: String,
    pub asset_history: Vec<String>,
    pub asset_history_pos: usize,

    pub asset_sort: AssetSort,
    pub asset_sort_desc: bool,

    pub asset_rename: Option<(String, String)>,

    pub asset_rename_focus: bool,

    pub pending_find_references: Option<(String, bool)>,

    pub tab_drag: Option<(String, String)>,

    pub tab_drop: Option<(String, prism_editor_core::dock::DockZone, f32)>,

    pub transform_clip: Option<(String, Vec<f32>)>,

    pub compact: bool,

    pub confirm_delete: bool,

    pub pending_entity_delete: Option<Vec<usize>>,

    pub dragging: Option<usize>,
    pub drop_target: Option<(usize, DropZone)>,
    pub shortcuts_open: bool,

    pub pending_doc_delete: Option<String>,

    pub pending_save_conflict: Option<usize>,

    pub pending_build: Option<crate::build::BuildKind>,

    pub build_running: bool,

    pub offline: OfflineToolsState,

    pub confirm: Option<Confirm>,

    pub focused_panel: Option<String>,

    pub lights_kept: usize,
    pub lights_total: usize,

    pub dropped_light_ids: Vec<u32>,

    pub pending_asset_delete: Option<Vec<String>>,

    pub material_asset: Option<String>,
    pub material_value: Option<serde_json::Value>,

    pub material_saved: Option<serde_json::Value>,
    pub material_error: Option<String>,

    pub pending_close: Option<CloseTarget>,

    pub pending_recovery: usize,

    pub render_graph_paused: bool,

    pub render_graph_frozen: Option<String>,

    pub render_graph_call: Option<usize>,

    pub render_graph_budget_ms: Option<f32>,

    pub pending_audio_preview: Option<AudioPreviewAction>,

    pub audio_preview_path: Option<String>,
    /// GASP-Godot parity debug & sandbox panel state (`#560`).
    pub gasp_debug: crate::gasp_debug::GaspDebugState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioPreviewAction {
    Play(String),
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseTarget {
    Document(usize),
    App,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmKind {
    CleanPlayer,

    CleanEngine,
}

#[derive(Clone)]
pub struct Confirm {
    pub kind: ConfirmKind,
    pub title: String,
    pub message: String,
    pub ok: String,
}

impl Confirm {
    pub fn new(kind: ConfirmKind, title: &str, message: impl Into<String>, ok: &str) -> Self {
        Self {
            kind,
            title: title.to_string(),
            message: message.into(),
            ok: ok.to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    SaveAs,
    NewFolder,
    SavePrefab,
    PrefabVariant,

    LayoutName,

    LayoutFileSave,
    LayoutFileLoad,
}

#[derive(Clone)]
pub struct Prompt {
    pub kind: PromptKind,
    pub title: String,
    pub hint: String,
    pub value: String,
}

impl Prompt {
    pub fn new(kind: PromptKind, title: &str, hint: &str, value: impl Into<String>) -> Self {
        Self {
            kind,
            title: title.to_string(),
            hint: hint.to_string(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropZone {
    Before,
    After,
    Child,
}

impl UiState {
    pub fn asset_primary(&self) -> Option<&String> {
        self.asset_selection.last()
    }

    pub fn asset_selected(&self, path: &str) -> bool {
        self.asset_selection.iter().any(|item| item == path)
    }

    pub fn select_asset_only(&mut self, path: impl Into<String>) {
        let path = path.into();
        self.asset_anchor = Some(path.clone());
        self.asset_selection = vec![path];
    }

    pub fn toggle_asset(&mut self, path: &str) {
        if let Some(index) = self.asset_selection.iter().position(|item| item == path) {
            self.asset_selection.remove(index);
        } else {
            self.asset_selection.push(path.to_string());
        }
        self.asset_anchor = Some(path.to_string());
    }

    pub fn select_asset_range(&mut self, ordered: &[String], path: &str) {
        let Some(target) = ordered.iter().position(|item| item == path) else {
            self.select_asset_only(path);
            return;
        };
        let anchor = self
            .asset_anchor
            .as_ref()
            .and_then(|anchor| ordered.iter().position(|item| item == anchor))
            .unwrap_or(target);
        let (start, end) = if anchor <= target {
            (anchor, target)
        } else {
            (target, anchor)
        };
        self.asset_selection = ordered[start..=end].to_vec();
    }

    pub fn clear_asset_selection(&mut self) {
        self.asset_selection.clear();
        self.asset_anchor = None;
    }

    pub fn forget_asset(&mut self, path: &str) {
        self.asset_selection.retain(|item| item != path);
        if self.asset_anchor.as_deref() == Some(path) {
            self.asset_anchor = None;
        }
    }

    pub fn new() -> Self {
        Self {
            log_autoscroll: true,

            log_collapse: false,
            log_clear_on_play: false,
            log_error_pause: false,
            viewport_scale: 1.0,
            asset_folder: "assets".to_string(),
            asset_history: vec!["assets".to_string()],
            asset_history_pos: 0,
            tilemap_palette: 8,
            asset_view: AssetView::List,
            asset_columns: AssetColumns::One,
            ..Default::default()
        }
    }
}
