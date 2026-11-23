//! Scene and Game zero-copy Vulkan viewports.

mod gizmos;
mod input;
mod overlays;

use gizmos::*;
use input::*;
pub(crate) use input::{
    bounds_table, drag_samples, game_cursor, letterbox, pick, proxy_transforms,
};
use overlays::*;

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use crate::egui_host::ImageRegistry;
use ash::vk;
use egui::{Align, Layout, Rect, Sense, Vec2};
use prism_editor_core::{
    emit_prt_probe_gizmos, unit_ring_points, viewport_math as vm, GizmoCommand, GizmoContext,
    GizmoFilter, GizmoRegistry, PrtProbeVolume, SelectMode, TransformJson, Workspace,
};
use prism_engine::input::{ElementState, KeyCode, MouseButton};
use prism_engine::scene::systems::terrain::{Heightfield, SculptMode};
use serde_json::Value;

use crate::engine_host::{EngineHost, ViewKind};
use crate::ui_gizmo::{
    anchor_handles, anchors_from_drag, hit_handle, UiAnchorHandle, HANDLE_RADIUS,
};

/// Which interaction starts on a primary-button press inside the Scene view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SceneDragIntent {
    None,
    /// UI anchor handles always win over the world gizmo (#429).
    UiAnchor,
    Gizmo,
    BoxSelect,
}

pub struct ImageBridge {
    pub registry: ImageRegistry,
    pub sampler: vk::Sampler,
}

pub(crate) struct ViewportTexture {
    pub(super) id: egui::TextureId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GameResolution {
    #[default]
    MatchWindow,

    Fixed {
        width: u32,
        height: u32,
    },

    Aspect {
        width: u32,
        height: u32,
    },
}

impl GameResolution {
    pub fn label(self) -> String {
        match self {
            GameResolution::MatchWindow => "Match Window".to_string(),
            GameResolution::Fixed { width, height } => format!("{width}x{height}"),
            GameResolution::Aspect { width, height } => {
                format!("{width}:{height} (fit)")
            }
        }
    }

    pub fn extent(self, panel_extent: (u32, u32)) -> (u32, u32) {
        match self {
            GameResolution::MatchWindow => panel_extent,

            GameResolution::Fixed { width, height } => {
                (width.clamp(1, 4096), height.clamp(1, 4096))
            }
            GameResolution::Aspect { width, height } => {
                let ratio = width.max(1) as f32 / height.max(1) as f32;
                let (panel_w, panel_h) =
                    (panel_extent.0.max(1) as f32, panel_extent.1.max(1) as f32);
                let (target_w, target_h) = if panel_w / panel_h > ratio {
                    (panel_h * ratio, panel_h)
                } else {
                    (panel_w, panel_w / ratio)
                };
                (
                    (target_w.round() as u32).clamp(1, 4096),
                    (target_h.round() as u32).clamp(1, 4096),
                )
            }
        }
    }
}

pub const GAME_RESOLUTION_PRESETS: &[GameResolution] = &[
    GameResolution::MatchWindow,
    GameResolution::Fixed {
        width: 1920,
        height: 1080,
    },
    GameResolution::Fixed {
        width: 1280,
        height: 720,
    },
    GameResolution::Fixed {
        width: 2560,
        height: 1440,
    },
    GameResolution::Fixed {
        width: 1080,
        height: 1920,
    },
    GameResolution::Fixed {
        width: 2400,
        height: 1080,
    },
    GameResolution::Fixed {
        width: 750,
        height: 1334,
    },
];

pub const GAME_ASPECT_PRESETS: &[GameResolution] = &[
    GameResolution::Aspect {
        width: 16,
        height: 9,
    },
    GameResolution::Aspect {
        width: 16,
        height: 10,
    },
    GameResolution::Aspect {
        width: 4,
        height: 3,
    },
    GameResolution::Aspect {
        width: 21,
        height: 9,
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GizmoTool {
    #[default]
    Translate,
    Rotate,
    Scale,
}

impl GizmoTool {
    pub fn label(self) -> &'static str {
        match self {
            GizmoTool::Translate => "Move",
            GizmoTool::Rotate => "Rotate",
            GizmoTool::Scale => "Scale",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TerrainBrushSettings {
    pub mode: SculptMode,
    pub radius: f32,
    pub strength: f32,
}

impl Default for TerrainBrushSettings {
    fn default() -> Self {
        Self {
            mode: SculptMode::Raise,
            radius: 0.1,
            strength: 1.0,
        }
    }
}

pub(super) const TERRAIN_BRUSH_ID: &str = "prism-terrain-brush-settings";

pub fn terrain_brush_settings(ui: &egui::Ui) -> TerrainBrushSettings {
    ui.data(|data| data.get_temp(egui::Id::new(TERRAIN_BRUSH_ID)))
        .unwrap_or_default()
}

pub fn set_terrain_brush_settings(ui: &mut egui::Ui, settings: TerrainBrushSettings) {
    ui.data_mut(|data| data.insert_temp(egui::Id::new(TERRAIN_BRUSH_ID), settings));
}

type TerrainSculptResult = Result<Option<(u32, usize)>, String>;

pub struct ViewportState {
    pub host: EngineHost,
    pub(super) texture: Option<ViewportTexture>,
    pub(super) registered_view: vk::ImageView,
    pub(super) extent: (u32, u32),

    pub dirty: bool,
    pub(super) flying_camera: bool,
    pub(super) fly_speed_scale: f32,
    pub(crate) game_keyboard_focused: bool,
    pub error: Option<String>,
    pub last_frame_ms: f32,
    pub fps: f32,
    pub frames: u64,
    pub(super) fps_window_started: Instant,
    pub(super) fps_window_frames: u32,

    pub tool: GizmoTool,

    pub sculpt: bool,
    pub(super) sculpt_last: Option<Instant>,
    pub(super) gizmo: Option<GizmoDrag>,
    pub(super) hovered_gizmo: Option<vm::GizmoHandle>,
    pub(super) gizmo_registry: GizmoRegistry,
    pub(super) probe_volume_cache: Option<(PathBuf, Option<SystemTime>, Option<PrtProbeVolume>)>,
    pub(super) ui_drag: Option<UiAnchorDrag>,

    pub raw_ui_edit: bool,
    pub(super) orbit_last_pos: Option<egui::Pos2>,

    pub(super) box_start: Option<egui::Pos2>,
    pub(super) box_current: Option<egui::Pos2>,

    pub(super) pick_cycle: Option<PickCycle>,

    pub(super) forwarded_keys: HashSet<KeyCode>,

    pub(super) forwarded_left_down: bool,
    /// Whether Game view currently holds the forwarded secondary (right) mouse button.
    pub(super) forwarded_right_down: bool,
    /// Whether Game view currently holds the forwarded middle mouse button.
    pub(super) forwarded_middle_down: bool,
    /// Whether the pointer was inside Game view on the previous frame (prevents entry jump deltas).
    pub(super) forwarded_cursor_inside: bool,

    pub report_path: Option<std::path::PathBuf>,

    pub target: GameResolution,

    pub project_resolution: Option<(u32, u32)>,
}

pub struct ViewportResponse {
    pub document_changed: bool,
}

pub(super) fn drop_hint(workspace: &Workspace, path: &str) -> String {
    if path.ends_with(".prefab.json") {
        workspace.t("desktop.viewport.dropPrefab")
    } else if path.ends_with(".scene.json") {
        workspace.t("desktop.viewport.dropScene")
    } else {
        workspace.t("desktop.viewport.dropOther")
    }
}

impl ViewportState {
    pub fn new(
        kind: ViewKind,
        context: std::sync::Arc<prism_render::VulkanContext>,
        width: u32,
        height: u32,
        shadow_map_size: u32,
        shadow_cascade_count: u32,
    ) -> anyhow::Result<Self> {
        let host = EngineHost::new(
            kind,
            context,
            width,
            height,
            shadow_map_size,
            shadow_cascade_count,
        )?;
        Ok(Self {
            host,
            texture: None,
            registered_view: vk::ImageView::null(),
            extent: (0, 0),
            dirty: true,
            flying_camera: false,
            fly_speed_scale: 1.0,
            game_keyboard_focused: false,
            error: None,
            last_frame_ms: 0.0,
            fps: 0.0,
            frames: 0,
            fps_window_started: Instant::now(),
            fps_window_frames: 0,
            tool: GizmoTool::Translate,
            sculpt: false,
            sculpt_last: None,
            gizmo: None,
            hovered_gizmo: None,
            gizmo_registry: GizmoRegistry::with_builtins(),
            probe_volume_cache: None,
            ui_drag: None,
            raw_ui_edit: false,
            orbit_last_pos: None,
            box_start: None,
            box_current: None,
            pick_cycle: None,
            forwarded_keys: HashSet::new(),
            forwarded_left_down: false,
            forwarded_right_down: false,
            forwarded_middle_down: false,
            forwarded_cursor_inside: false,
            report_path: None,
            target: GameResolution::default(),
            project_resolution: None,
        })
    }

    pub fn kind(&self) -> ViewKind {
        self.host.kind()
    }

    pub fn flying_camera(&self) -> bool {
        self.flying_camera
    }

    /// Mutable access to the Unity-style component gizmo drawer registry.
    #[expect(dead_code, reason = "Exposed for external gizmo drawers")]
    pub fn gizmo_registry_mut(&mut self) -> &mut GizmoRegistry {
        &mut self.gizmo_registry
    }

    /// Clear any cached CPU PRT probe volume so the next frame reloads from disk.
    pub fn invalidate_probe_cache(&mut self) {
        self.probe_volume_cache = None;
    }

    /// Currently cached PRT probe volume loaded for Scene View visualization.
    pub fn cached_probe_volume(&self) -> Option<&PrtProbeVolume> {
        self.probe_volume_cache
            .as_ref()
            .and_then(|entry| entry.2.as_ref())
    }

    pub(super) fn resolve_probe_volume_path(workspace: &Workspace) -> Option<PathBuf> {
        const GI: &str = "prism_engine::scene::GiConfig";
        let mut declared: Option<String> = None;
        for entity in &workspace.document().doc.entities {
            if !entity.active() {
                continue;
            }
            if let Some(cfg) = entity.component(GI) {
                let path = cfg
                    .get("volume")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                if cfg.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
                    declared = path.or_else(|| Some("assets/gi/probe_volume.bin".to_string()));
                    break;
                }
                if declared.is_none() {
                    declared = path;
                }
            }
        }
        let rel = declared.unwrap_or_else(|| "assets/gi/probe_volume.bin".to_string());
        let p = PathBuf::from(&rel);
        if p.is_absolute() {
            return p.is_file().then_some(p);
        }
        let mut candidates = Vec::new();
        if let Some(root) = workspace.project_root.as_ref() {
            candidates.push(root.join(&p));
            candidates.push(root.join("assets").join(&p));
        }
        candidates.push(p);
        candidates.into_iter().find(|c| c.is_file())
    }

    pub(super) fn ensure_probe_volume(&mut self, workspace: &Workspace) -> Option<&PrtProbeVolume> {
        let Some(path) = Self::resolve_probe_volume_path(workspace) else {
            self.probe_volume_cache = None;
            return None;
        };
        let mtime = std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok());
        let cache_hit =
            self.probe_volume_cache
                .as_ref()
                .is_some_and(|(cached_path, cached_mtime, _)| {
                    cached_path == &path && *cached_mtime == mtime
                });
        if !cache_hit {
            let parsed = std::fs::read(&path)
                .ok()
                .and_then(|bytes| PrtProbeVolume::from_bytes(&bytes).ok());
            self.probe_volume_cache = Some((path, mtime, parsed));
        }
        self.probe_volume_cache
            .as_ref()
            .and_then(|entry| entry.2.as_ref())
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn reset_forwarded_input(&mut self) {
        self.forwarded_keys.clear();
        self.forwarded_left_down = false;
        self.forwarded_right_down = false;
        self.forwarded_middle_down = false;
        self.forwarded_cursor_inside = false;
    }

    pub fn render_frame(&mut self, bridge: &ImageBridge) -> bool {
        if let Err(error) = self.relayout(bridge) {
            self.error = Some(format!("{error:#}"));
            return false;
        }
        let started = Instant::now();
        match self.host.render() {
            Ok(Some((view, extent))) => {
                self.extent = (extent.width, extent.height);
                if view != self.registered_view {
                    if let Some(old) = self.texture.take() {
                        bridge.registry.unregister_user_texture(old.id);
                    }
                    let id = bridge.registry.register_user_texture(view, bridge.sampler);
                    self.texture = Some(ViewportTexture { id });
                    self.registered_view = view;
                }
                self.last_frame_ms = started.elapsed().as_secs_f32() * 1000.0;
                self.frames += 1;
                self.fps_window_frames += 1;
                let elapsed = self.fps_window_started.elapsed().as_secs_f32();
                if elapsed >= 0.25 {
                    self.fps = self.fps_window_frames as f32 / elapsed;
                    self.fps_window_frames = 0;
                    self.fps_window_started = Instant::now();
                }
                self.dirty = false;
                self.error = None;
                self.maybe_write_render_graph_report();
                true
            }
            Ok(None) => false,
            Err(error) => {
                self.error = Some(format!("{error:#}"));
                false
            }
        }
    }

    pub(super) fn maybe_write_render_graph_report(&mut self) {
        let Some(path) = self.report_path.clone() else {
            return;
        };
        let interval = prism_render::render_graph_report::REFRESH_INTERVAL_FRAMES;
        if self.frames != 1 && self.frames % interval != 0 {
            return;
        }
        if let Err(error) = self.host.write_render_graph_report(&path, self.frames) {
            self.error = Some(format!("render graph report write failed: {error:#}"));
        }
    }

    pub(super) fn relayout(&mut self, bridge: &ImageBridge) -> anyhow::Result<()> {
        let target = self.extent;
        if target.0 == 0 || target.1 == 0 {
            return Ok(());
        }
        let current = self
            .host
            .renderer()
            .offscreen_extent()
            .map(|extent| (extent.width, extent.height))
            .unwrap_or_default();
        if current == target {
            return Ok(());
        }
        self.host.resize(target.0, target.1)?;

        if let Some(old) = self.texture.take() {
            bridge.registry.unregister_user_texture(old.id);
        }
        self.registered_view = vk::ImageView::null();
        Ok(())
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &mut Workspace,
        bridge: &ImageBridge,
        playing: bool,
        scale: f32,
    ) -> ViewportResponse {
        let rect = ui.available_rect_before_wrap();
        let ppp = ui.ctx().pixels_per_point();
        let panel_extent = viewport_extent(rect.width(), rect.height(), ppp, scale);

        self.extent = if self.kind() == ViewKind::Game {
            self.target.extent(panel_extent)
        } else {
            panel_extent
        };

        let image_rect = if self.kind() == ViewKind::Game {
            letterbox(rect, self.extent)
        } else {
            rect
        };

        let current = self
            .host
            .renderer()
            .offscreen_extent()
            .map(|extent| (extent.width, extent.height))
            .unwrap_or_default();
        if current != self.extent {
            self.dirty = true;
        }

        if self.dirty || playing {
            self.render_frame(bridge);
        }

        let response = ui.allocate_rect(rect, Sense::click_and_drag());

        self.forward_game_input(ui, image_rect, &response, self.game_keyboard_focused);
        let painter = ui.painter_at(rect);

        if let Some(payload) = response.dnd_hover_payload::<crate::widgets::AssetDrag>() {
            let accepts =
                payload.path.ends_with(".prefab.json") || payload.path.ends_with(".scene.json");
            let (edge, tint, hint) = if accepts {
                (
                    egui::Color32::from_rgb(90, 170, 120),
                    egui::Color32::from_rgba_unmultiplied(90, 170, 120, 36),
                    drop_hint(workspace, &payload.path),
                )
            } else {
                (
                    egui::Color32::from_rgb(214, 96, 96),
                    egui::Color32::from_rgba_unmultiplied(214, 96, 96, 28),
                    workspace.t("desktop.viewport.dropRejected"),
                )
            };
            painter.rect_stroke(
                rect.shrink(2.0),
                egui::CornerRadius::ZERO,
                egui::Stroke::new(2.0_f32, edge),
                egui::StrokeKind::Inside,
            );
            painter.rect_filled(rect.shrink(2.0), egui::CornerRadius::ZERO, tint);
            if let Some(pos) = response
                .hover_pos()
                .or_else(|| response.interact_pointer_pos())
            {
                let local = (pos.x - rect.min.x, pos.y - rect.min.y);
                let frame = vm::camera_frame(&workspace.camera);
                let ray =
                    vm::ray_from_screen(local.0, local.1, &frame, rect.width(), rect.height());
                if accepts {
                    if let Some(point) = vm::drop_point(&workspace.camera, ray) {
                        if let Some((x, y, _)) =
                            vm::project(point, &frame, rect.width(), rect.height())
                        {
                            let center = rect.min + egui::vec2(x, y);
                            let stroke = egui::Stroke::new(1.5_f32, edge);
                            painter.line_segment(
                                [
                                    center + egui::vec2(-7.0, 0.0),
                                    center + egui::vec2(7.0, 0.0),
                                ],
                                stroke,
                            );
                            painter.line_segment(
                                [
                                    center + egui::vec2(0.0, -7.0),
                                    center + egui::vec2(0.0, 7.0),
                                ],
                                stroke,
                            );
                        }
                        let ghost = TransformJson {
                            position: point,
                            rotation: [0.0, 0.0, 0.0, 1.0],
                            scale: [1.0, 1.0, 1.0],
                        };
                        for (a, b) in vm::projected_box_clipped_edges(
                            &ghost,
                            &frame,
                            rect.width(),
                            rect.height(),
                        ) {
                            painter.line_segment(
                                [
                                    rect.min + egui::vec2(a.0, a.1),
                                    rect.min + egui::vec2(b.0, b.1),
                                ],
                                egui::Stroke::new(1.0_f32, edge),
                            );
                        }
                    }
                }

                let name = payload.path.rsplit('/').next().unwrap_or(&payload.path);
                let text = format!("{name}  →  {hint}");
                let anchor = pos + egui::vec2(16.0, 18.0);
                let galley = painter.layout_no_wrap(
                    text,
                    egui::FontId::monospace(11.0),
                    egui::Color32::WHITE,
                );
                let chip = egui::Rect::from_min_size(anchor, galley.size() + egui::vec2(8.0, 4.0));
                painter.rect_filled(
                    chip,
                    egui::CornerRadius::ZERO,
                    egui::Color32::from_rgba_unmultiplied(18, 20, 24, 220),
                );
                painter.rect_stroke(
                    chip,
                    egui::CornerRadius::ZERO,
                    egui::Stroke::new(1.0_f32, edge),
                    egui::StrokeKind::Inside,
                );
                painter.galley(anchor + egui::vec2(4.0, 2.0), galley, egui::Color32::WHITE);
            }
        }
        if let Some(payload) = response.dnd_release_payload::<crate::widgets::AssetDrag>() {
            let path = payload.path.clone();
            let drop_local = response
                .interact_pointer_pos()
                .map(|pos| (pos.x - rect.min.x, pos.y - rect.min.y));
            if path.ends_with(".prefab.json") {
                match workspace.instantiate_prefab_asset(&path, None) {
                    Ok(_) => {
                        if let Some(local) = drop_local {
                            let frame = vm::camera_frame(&workspace.camera);
                            let ray = vm::ray_from_screen(
                                local.0,
                                local.1,
                                &frame,
                                rect.width(),
                                rect.height(),
                            );
                            if let Some(point) = vm::drop_point(&workspace.camera, ray) {
                                if let Some(index) = workspace.primary_selection() {
                                    let entity =
                                        workspace.document().doc.id_of_index(index).unwrap_or(0);
                                    let current =
                                        vm::world_position(&workspace.document().doc, index);
                                    let delta = [
                                        point[0] - current[0],
                                        point[1] - current[1],
                                        point[2] - current[2],
                                    ];
                                    workspace.translate_entity_world(entity, delta);
                                }
                            }
                        }
                        self.dirty = true;
                    }
                    Err(error) => {
                        self.error = Some(workspace.tf(
                            "desktop.status.instantiateFailed",
                            &[("error", &error.to_string())],
                        ))
                    }
                }
            } else if path.ends_with(".scene.json") {
                if let Err(error) = workspace.open_project_file(&path) {
                    self.error = Some(workspace.tf(
                        "desktop.status.openFailed",
                        &[("path", &path), ("error", &error.to_string())],
                    ));
                }
            }
        }

        if let Some(texture) = &self.texture {
            painter.image(
                texture.id,
                image_rect,
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else {
            painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(24, 26, 30));
            let text = self
                .error
                .clone()
                .unwrap_or_else(|| "renderer not ready".to_string());
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(13.0),
                egui::Color32::from_gray(150),
            );
        }

        if self.kind() == ViewKind::Game {
            let bar = Rect::from_min_size(
                image_rect.min + egui::vec2(6.0, 6.0),
                Vec2::new(190.0, 22.0),
            );
            let mut bar_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(bar)
                    .layout(Layout::left_to_right(Align::Center)),
            );
            let mut chosen = self.target;
            egui::ComboBox::from_id_salt(("game-resolution", rect.min.x as i32))
                .selected_text(self.target.label())
                .width(120.0)
                .show_ui(&mut bar_ui, |ui| {
                    for preset in game_resolution_presets(self.project_resolution) {
                        if ui
                            .selectable_label(preset == self.target, preset.label())
                            .on_hover_text(match preset {
                                GameResolution::MatchWindow => {
                                    "Render at the panel size (old behaviour)"
                                }
                                GameResolution::Fixed { .. } => {
                                    "Render off-screen at this resolution and letterbox it"
                                }
                                GameResolution::Aspect { .. } => {
                                    "Keep this aspect ratio and fit it to the panel"
                                }
                            })
                            .clicked()
                        {
                            chosen = preset;
                        }
                    }
                });
            if chosen != self.target {
                self.target = chosen;

                self.dirty = true;
            }
        }

        let mut document_changed = false;
        let frame = vm::camera_frame(&workspace.camera);
        let alt = ui.input(|input| input.modifiers.alt);
        let local_of = |pos: egui::Pos2| (pos.x - rect.min.x, pos.y - rect.min.y);
        let sculpting = self.kind() == ViewKind::Scene && self.sculpt && !alt;
        if self.kind() == ViewKind::Scene {
            if let Some(drag) = self.gizmo.as_ref() {
                self.hovered_gizmo = Some(drag.handle);
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            } else if !sculpting && !alt {
                self.hovered_gizmo = response.hover_pos().and_then(|pos| {
                    self.gizmo_hit_at(workspace, &frame, rect, local_of(pos))
                        .map(|(handle, _, _)| handle)
                });
                if self.hovered_gizmo.is_some() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                }
            } else {
                self.hovered_gizmo = None;
            }
        }
        let mut icon_hits: Vec<(usize, egui::Pos2)> = Vec::new();
        if self.kind() == ViewKind::Scene {
            if workspace.show_selection_outline {
                self.draw_selection_outline(&painter, rect, workspace, &frame);
            }

            self.draw_sprite_rects(&painter, rect, workspace, &frame);

            if workspace.show_skeleton {
                self.draw_skeleton(&painter, rect, workspace, &frame);
            }

            self.draw_ui_rects(&painter, rect, workspace, self.extent);

            icon_hits = self.draw_component_icons(&painter, rect, workspace, &frame);
            self.draw_gizmo(&painter, rect, workspace, &frame);

            draw_orientation_gizmo(&painter, rect, &frame);
        }

        if sculpting
            && (response.drag_started_by(egui::PointerButton::Primary)
                || response.dragged_by(egui::PointerButton::Primary)
                || response.clicked())
        {
            let due = self
                .sculpt_last
                .is_none_or(|last| last.elapsed() >= std::time::Duration::from_millis(50));
            if due {
                if let Some(pos) = response.interact_pointer_pos() {
                    let local = local_of(pos);
                    let frame = vm::camera_frame(&workspace.camera);
                    let ray =
                        vm::ray_from_screen(local.0, local.1, &frame, rect.width(), rect.height());
                    let settings = terrain_brush_settings(ui);
                    match sculpt_terrain_at_ray(workspace, ray, settings) {
                        Ok(Some((scene_id, _affected))) => {
                            self.host.reload_terrain_heightmap(scene_id);
                            self.error = None;
                            self.dirty = true;
                        }
                        Ok(None) => {}
                        Err(error) => self.error = Some(error),
                    }
                    self.sculpt_last = Some(Instant::now());
                }
            }
        }

        if response.dragged() {
            self.pick_cycle = None;
        }

        if response.drag_started_by(egui::PointerButton::Primary)
            && self.kind() == ViewKind::Scene
            && !alt
            && !sculpting
        {
            if let Some(pos) = response.interact_pointer_pos() {
                let local = local_of(pos);

                let ui_handle = self.ui_handle_at(workspace, rect, self.extent, pos);
                let has_ui_handle = ui_handle.is_some();
                let has_gizmo =
                    !has_ui_handle && self.gizmo_hit_at(workspace, &frame, rect, local).is_some();
                let intent = scene_drag_intent(
                    has_ui_handle,
                    has_gizmo,
                    response.dragged_by(egui::PointerButton::Primary),
                );
                self.ui_drag = ui_handle;
                match intent {
                    SceneDragIntent::UiAnchor | SceneDragIntent::None => {}
                    SceneDragIntent::Gizmo => {
                        if !self.begin_gizmo_drag(workspace, &frame, rect, local)
                            && response.dragged_by(egui::PointerButton::Primary)
                        {
                            self.box_start = Some(pos);
                            self.box_current = Some(pos);
                        }
                    }
                    SceneDragIntent::BoxSelect => {
                        self.box_start = Some(pos);
                        self.box_current = Some(pos);
                    }
                }
            }
        }

        let pointer_delta = ui.input(|i| i.pointer.delta());
        let ctrl = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);
        if response.dragged_by(egui::PointerButton::Primary)
            && !(self.kind() == ViewKind::Scene && alt)
            && !sculpting
        {
            if let Some(pos) = response.interact_pointer_pos() {
                let local = local_of(pos);
                let extent = self.extent;
                if let Some(drag) = self.ui_drag.as_mut() {
                    let parent_rect = Self::ui_rect_to_viewport(drag.parent, rect, extent);
                    drag.anchors =
                        anchors_from_drag(drag.handle, pos, parent_rect, drag.anchors, ctrl);
                } else if self.gizmo.is_some() {
                    if self.gizmo_drag_step(
                        workspace,
                        &frame,
                        rect,
                        local,
                        (pointer_delta.x, pointer_delta.y),
                        ctrl,
                    ) {
                        document_changed = true;
                    }
                } else if let Some(start) = self.box_start {
                    self.box_current = Some(pos);
                    painter.rect_stroke(
                        egui::Rect::from_two_pos(start, pos),
                        egui::CornerRadius::ZERO,
                        egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(90, 160, 240)),
                        egui::StrokeKind::Inside,
                    );
                }
            }
        }
        if self.kind() == ViewKind::Scene {
            let (right_held, right_pressed) = ui.input(|input| {
                (
                    input.pointer.button_down(egui::PointerButton::Secondary),
                    input.pointer.button_pressed(egui::PointerButton::Secondary),
                )
            });
            if !right_held || alt || workspace.camera.orthographic {
                self.flying_camera = false;
            } else if right_pressed && response.hovered() {
                self.flying_camera = true;
            }
            if self.flying_camera {
                let (forward, right, vertical, scroll_y) = ui.input(|input| {
                    let key = |key| if input.key_down(key) { 1.0 } else { 0.0 };
                    (
                        key(egui::Key::W) - key(egui::Key::S),
                        key(egui::Key::D) - key(egui::Key::A),
                        key(egui::Key::E) - key(egui::Key::Q),
                        input.raw_scroll_delta.y,
                    )
                });
                if scroll_y.abs() > 0.0 {
                    self.fly_speed_scale =
                        (self.fly_speed_scale * (1.0 + scroll_y * 0.002)).clamp(0.1, 20.0);
                }
                if forward != 0.0 || right != 0.0 || vertical != 0.0 {
                    let dt = ui.input(|input| input.stable_dt) * self.fly_speed_scale;
                    workspace.camera.fly_move(forward, right, vertical, dt);
                    self.dirty = true;
                }
            }
            let rmb_pan_2d = workspace.camera.orthographic
                && !alt
                && response.dragged_by(egui::PointerButton::Secondary);
            if response.dragged_by(egui::PointerButton::Middle) || rmb_pan_2d {
                if let Some(pos) = response.interact_pointer_pos() {
                    let delta = self
                        .orbit_last_pos
                        .replace(pos)
                        .map(|last| pos - last)
                        .unwrap_or_default();
                    pan_camera(&mut workspace.camera, delta.x, delta.y, rect.height());
                }
                self.dirty = true;
            } else if response.dragged_by(egui::PointerButton::Secondary) {
                if let Some(pos) = response.interact_pointer_pos() {
                    let delta = self
                        .orbit_last_pos
                        .replace(pos)
                        .map(|last| pos - last)
                        .unwrap_or_default();
                    if alt {
                        workspace.camera.zoom((delta.x - delta.y) * 2.5);
                    } else {
                        workspace.camera.look_in_place(delta.x, delta.y);
                    }
                }
                self.dirty = true;
            } else {
                self.orbit_last_pos = None;
            }
        } else {
            self.flying_camera = false;
        }
        if self.kind() == ViewKind::Scene
            && alt
            && response.dragged_by(egui::PointerButton::Primary)
        {
            workspace.camera.orbit(pointer_delta.x, pointer_delta.y);
            self.dirty = true;
            self.gizmo = None;
            self.box_start = None;
            self.box_current = None;
        }

        if response.drag_stopped_by(egui::PointerButton::Primary) {
            if let Some(drag) = self.ui_drag.take() {
                let changed = if drag.anchors != drag.start_anchors {
                    if self.raw_ui_edit {
                        workspace.set_ui_anchors_raw(drag.index, drag.anchors)
                    } else {
                        workspace.set_ui_anchors_preserving_rect(
                            drag.index,
                            drag.anchors,
                            drag.parent,
                            drag.rect,
                        )
                    }
                } else {
                    false
                };
                if changed {
                    document_changed = true;
                    self.dirty = true;
                }
            }

            if let (Some(start), Some(end)) = (self.box_start.take(), self.box_current.take()) {
                let selection_rect = egui::Rect::from_two_pos(start, end);
                if selection_rect.width() > 3.0 || selection_rect.height() > 3.0 {
                    let local =
                        vm::local_box((start.x, start.y), (end.x, end.y), (rect.min.x, rect.min.y));
                    let hits = vm::box_select(
                        local,
                        &frame,
                        rect.width(),
                        rect.height(),
                        &proxy_transforms(workspace),
                    );
                    let modifiers = ui.input(|input| input.modifiers);
                    let mode = SelectMode::from_modifiers(
                        modifiers.shift,
                        modifiers.ctrl || modifiers.command,
                    );
                    workspace.select_many_with(&hits, mode);
                    self.dirty = true;
                }
            }
            self.gizmo = None;

            workspace.end_interaction();
        }
        if response.drag_stopped_by(egui::PointerButton::Middle)
            || response.drag_stopped_by(egui::PointerButton::Secondary)
        {
            self.orbit_last_pos = None;
        }
        if response.hovered() && !self.flying_camera {
            let scroll = ui.input(|i| i.raw_scroll_delta.y);
            if scroll.abs() > 0.0 {
                let frame = vm::camera_frame(&workspace.camera);
                let anchor = response.hover_pos().map(|pos| {
                    let (x, y) = local_of(pos);
                    let (origin, direction) =
                        vm::ray_from_screen(x, y, &frame, rect.width(), rect.height());
                    let distance = workspace.camera.distance;
                    let t = if frame.ortho_half_height.is_some() {
                        distance
                    } else {
                        let fwd_dot = (direction[0] * frame.forward[0]
                            + direction[1] * frame.forward[1]
                            + direction[2] * frame.forward[2])
                            .max(0.1);
                        distance / fwd_dot
                    };
                    [
                        origin[0] + direction[0] * t,
                        origin[1] + direction[1] * t,
                        origin[2] + direction[2] * t,
                    ]
                });
                workspace.camera.zoom_towards(scroll, anchor);
                self.dirty = true;
            }
        }
        if response.double_clicked() && self.kind() == ViewKind::Scene {
            workspace.frame_selection();
            self.dirty = true;
        } else if !sculpting && response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let local = local_of(pos);
                if self.kind() == ViewKind::Scene {
                    let aligned = vm::pick_orientation_axis(
                        (pos.x, pos.y),
                        orientation_gizmo_center(rect),
                        ORIENTATION_GIZMO_RADIUS,
                        &frame,
                    );
                    if let Some((axis, sign)) = aligned {
                        workspace.camera.align_to_axis(axis, sign);
                        self.pick_cycle = None;
                        self.dirty = true;
                    } else {
                        let icon = icon_hits
                            .iter()
                            .map(|(index, center)| {
                                let dx = local.0 - (center.x - rect.min.x);
                                let dy = local.1 - (center.y - rect.min.y);
                                (*index, (dx * dx + dy * dy).sqrt())
                            })
                            .filter(|(_, distance)| *distance <= 11.0)
                            .min_by(|a, b| a.1.total_cmp(&b.1))
                            .map(|(index, _)| index);

                        let modifiers = ui.input(|input| input.modifiers);
                        let mode = SelectMode::from_modifiers(
                            modifiers.shift,
                            modifiers.ctrl || modifiers.command,
                        );
                        let candidates = pick_candidates(
                            workspace,
                            Some(&self.host),
                            local,
                            rect.size(),
                            &frame,
                        );
                        if let Some(index) = icon {
                            workspace.select_with(index, mode);
                            self.pick_cycle = None;
                        } else if matches!(mode, SelectMode::Replace) {
                            match next_pick(self.pick_cycle.as_ref(), local, candidates) {
                                Some((index, cycle)) => {
                                    self.pick_cycle = Some(cycle);
                                    workspace.select_with(index, mode);
                                }
                                None => {
                                    workspace.clear_selection_in(mode);
                                    self.pick_cycle = None;
                                }
                            }
                        } else {
                            match candidates.first() {
                                Some(index) => workspace.select_with(*index, mode),
                                None => workspace.clear_selection_in(mode),
                            }
                            self.pick_cycle = None;
                        }
                        self.dirty = true;
                    }
                }
            }
        }

        let (lights_kept, lights_total) = self.host.local_light_stats();
        let mut stats = workspace.tf(
            "desktop.viewport.stats",
            &[
                (
                    "view",
                    &workspace.t(match self.kind() {
                        ViewKind::Scene => "menu.sceneView",
                        ViewKind::Game => "menu.gameView",
                    }),
                ),
                (
                    "tool",
                    &workspace.t(match self.tool {
                        GizmoTool::Translate => "toolbar.move",
                        GizmoTool::Rotate => "toolbar.rotate",
                        GizmoTool::Scale => "toolbar.scale",
                    }),
                ),
                ("ms", &format!("{:.1}", self.last_frame_ms)),
                ("fps", &format!("{:.1}", self.fps)),
                ("frames", &self.frames.to_string()),
            ],
        );
        if let Some(label) = light_budget_label(workspace, lights_kept, lights_total) {
            stats.push_str(" · ");
            stats.push_str(&label);
        }
        let pos = rect.left_bottom() + egui::vec2(6.0, -6.0);
        painter.text(
            pos,
            egui::Align2::LEFT_BOTTOM,
            stats,
            egui::FontId::monospace(11.0),
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 170),
        );

        ViewportResponse { document_changed }
    }
}

#[cfg(test)]
mod drop_hint_tests {
    use prism_editor_core::Workspace;

    #[test]
    fn the_drop_label_names_the_action() {
        assert_eq!(
            super::drop_hint(&Workspace::sample(), "assets/prefabs/crate.prefab.json"),
            "instantiate at marker"
        );
        assert_eq!(
            super::drop_hint(&Workspace::sample(), "assets/scenes/level.scene.json"),
            "open scene"
        );
        assert_eq!(
            super::drop_hint(&Workspace::sample(), "assets/materials/red.mat.json"),
            "placeable"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Pos2, Vec2};

    #[test]
    fn hidden_entities_are_not_pick_proxies() {
        let mut workspace = Workspace::sample();
        let before = proxies(&workspace, None).len();

        workspace.set_entity_hidden(1, true);

        let after = proxies(&workspace, None).len();
        assert_eq!(
            after,
            before - 2,
            "hidden parent and subtree do not participate in picking"
        );
    }

    #[test]
    fn collider_bounds_follow_the_authored_shape() {
        let mut workspace = Workspace::sample();
        let collider = serde_json::json!({
            "shape": "Sphere",
            "center": [1.0, 2.0, 3.0],
            "radius": 0.25
        });
        workspace.document_mut().doc.entities[0]
            .components
            .insert("prism_engine::scene::Collider".to_string(), collider);

        let sphere = collider_bounds(&workspace.document().doc, 0).expect("sphere bounds");
        assert_eq!(sphere.min, [0.75, 1.75, 2.75]);
        assert_eq!(sphere.max, [1.25, 2.25, 3.25]);

        let collider = serde_json::json!({
            "shape": "Capsule",
            "radius": 0.5,
            "half_height": 2.0
        });
        workspace.document_mut().doc.entities[0].components["prism_engine::scene::Collider"] =
            collider;
        let capsule = collider_bounds(&workspace.document().doc, 0).expect("capsule bounds");
        assert_eq!(capsule.min, [-0.5, -2.5, -0.5]);
        assert_eq!(capsule.max, [0.5, 2.5, 0.5]);

        let collider = serde_json::json!({
            "shape": "Box",
            "half_extents": [1.0, 2.0, 3.0]
        });
        workspace.document_mut().doc.entities[0].components["prism_engine::scene::Collider"] =
            collider;
        let cuboid = collider_bounds(&workspace.document().doc, 0).expect("box bounds");
        assert_eq!(cuboid.min, [-1.0, -2.0, -3.0]);
        assert_eq!(cuboid.max, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn component_icon_filters_are_independent() {
        let mut workspace = Workspace::sample();
        assert!(icon_visible(&workspace, true, false));
        assert!(icon_visible(&workspace, false, true));
        workspace.show_camera_icons = false;
        assert!(!icon_visible(&workspace, true, false));
        assert!(icon_visible(&workspace, false, true));
        workspace.show_light_icons = false;
        assert!(!icon_visible(&workspace, false, true));
        assert!(!icon_visible(&workspace, true, true));
    }

    #[test]
    fn repeated_clicks_cycle_through_overlapping_entities() {
        let candidates = vec![3, 7, 9];
        let (first, cycle) = next_pick(None, (100.0, 50.0), candidates.clone()).unwrap();
        assert_eq!(first, 3);
        let (second, cycle) = next_pick(Some(&cycle), (101.0, 51.0), candidates.clone()).unwrap();
        assert_eq!(second, 7, "within 4px treated as same pick");
        let (third, cycle) = next_pick(Some(&cycle), (100.0, 50.0), candidates.clone()).unwrap();
        assert_eq!(third, 9);

        let (again, _) = next_pick(Some(&cycle), (100.0, 50.0), candidates.clone()).unwrap();
        assert_eq!(again, 3);

        let (moved, _) = next_pick(Some(&cycle), (140.0, 50.0), candidates).unwrap();
        assert_eq!(moved, 3);
        let (changed, _) = next_pick(Some(&cycle), (100.0, 50.0), vec![7, 9]).unwrap();
        assert_eq!(changed, 7);

        assert_eq!(next_pick(Some(&cycle), (0.0, 0.0), Vec::new()), None);
    }

    #[test]
    fn the_stats_label_reports_dropped_lights() {
        let workspace = Workspace::sample();
        assert_eq!(
            light_budget_label(&workspace, 0, 0),
            None,
            "no local light does not occupy position"
        );
        assert_eq!(
            light_budget_label(&workspace, 3, 3),
            Some("light 3".to_string())
        );
        assert_eq!(
            light_budget_label(&workspace, 8, 12),
            Some("light 8/12 (drop 4)".to_string())
        );
    }

    #[test]
    fn ctrl_inverts_the_snap_toggle() {
        assert!(!snap_active(false, false), "free drag when off");
        assert!(snap_active(true, false), "snap when on");
        assert!(snap_active(false, true), "hold Ctrl temporarily open");
        assert!(!snap_active(true, true), "hold Ctrl temporarily close");
    }

    #[test]
    fn slow_snapped_drag_still_moves_once_a_step_is_reached() {
        let snap = prism_editor_core::workspace::SnapSettings::default();
        let mut applied = 0.0;
        let mut accum = 0.0;

        for _ in 0..6 {
            accum += 0.02;
            applied += drag_translate_step(snap, true, 0.0, accum, applied);
        }
        assert_eq!(applied, 0.0, "cannot move before half step");

        accum += 0.02;
        applied += drag_translate_step(snap, true, 0.0, accum, applied);
        assert_eq!(applied, 0.25);

        accum += 0.2;
        let extra = drag_translate_step(snap, true, 0.0, accum, applied);
        assert_eq!(extra, 0.0);

        accum += 0.06;
        let extra = drag_translate_step(snap, true, 0.0, accum, applied);
        assert_eq!(extra, 0.25);

        let free = drag_translate_step(snap, false, 0.0, 0.3, 0.25);
        assert!((free - 0.05).abs() < 1e-6);
    }

    #[test]
    fn game_keys_use_the_engine_abi_mapping() {
        assert_eq!(engine_key(egui::Key::W), Some(KeyCode::KeyW));
        assert_eq!(engine_key(egui::Key::T), Some(KeyCode::KeyT));
        assert_eq!(engine_key(egui::Key::F3), Some(KeyCode::F3));
        assert_eq!(engine_key(egui::Key::Space), Some(KeyCode::Space));
        assert_eq!(engine_key(egui::Key::Num5), Some(KeyCode::Digit5));
        assert_eq!(engine_key(egui::Key::Slash), Some(KeyCode::Other(0xBF)));
        assert_eq!(engine_key(egui::Key::Backspace), Some(KeyCode::Other(0x08)));
        assert_eq!(engine_key(egui::Key::Z), Some(KeyCode::Other(b'Z' as u32)));
        assert_eq!(engine_key(egui::Key::F1), None);
    }

    #[test]
    fn extent_follows_scale_and_dpi() {
        assert_eq!(viewport_extent(800.0, 600.0, 1.0, 1.0), (800, 600));
        assert_eq!(viewport_extent(800.0, 600.0, 2.0, 1.0), (1600, 1200));
        assert_eq!(viewport_extent(800.0, 600.0, 1.0, 0.5), (400, 300));
    }

    #[test]
    fn extent_clamps_degenerate_input() {
        assert_eq!(viewport_extent(0.0, 0.0, 1.0, 1.0), (1, 1));
        assert_eq!(viewport_extent(9000.0, 9000.0, 1.0, 2.0), (4096, 4096));

        assert_eq!(viewport_extent(100.0, 100.0, 1.0, 0.0), (25, 25));
    }

    #[test]
    fn a_fixed_resolution_ignores_the_panel_size() {
        let target = GameResolution::Fixed {
            width: 1920,
            height: 1080,
        };
        assert_eq!(target.extent((800, 600)), (1920, 1080));
        assert_eq!(target.extent((3840, 2160)), (1920, 1080));

        assert_eq!(GameResolution::MatchWindow.extent((800, 600)), (800, 600));

        assert_eq!(
            GameResolution::Fixed {
                width: 8000,
                height: 9000
            }
            .extent((1, 1)),
            (4096, 4096)
        );
    }

    #[test]
    fn an_aspect_target_fits_the_panel_without_changing_its_ratio() {
        let target = GameResolution::Aspect {
            width: 16,
            height: 9,
        };
        assert_eq!(target.extent((1600, 800)), (1422, 800));

        assert_eq!(target.extent((800, 800)), (800, 450));

        assert_eq!(target.extent((1920, 1080)), (1920, 1080));
    }

    #[test]
    fn presets_put_the_project_resolution_first() {
        let presets = game_resolution_presets(Some((1600, 900)));
        assert_eq!(presets[0], GameResolution::MatchWindow);
        assert_eq!(
            presets[1],
            GameResolution::Fixed {
                width: 1600,
                height: 900
            }
        );
        assert_eq!(
            presets[2],
            GameResolution::Aspect {
                width: 16,
                height: 9
            }
        );
        assert!(game_resolution_presets(None)
            .iter()
            .all(|preset| !matches!(preset, GameResolution::Fixed { width: 1600, .. })));
    }

    #[test]
    fn letterbox_keeps_the_aspect_ratio_and_centres_the_image() {
        let outer = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(1000.0, 1000.0));

        let fitted = letterbox(outer, (1920, 1080));
        assert!((fitted.width() - 1000.0).abs() < 1e-3);
        assert!((fitted.height() - 562.5).abs() < 1e-3);
        assert!((fitted.center() - outer.center()).length() < 1e-3);

        let wide = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(2000.0, 500.0));
        let fitted = letterbox(wide, (1920, 1080));
        assert!((fitted.height() - 500.0).abs() < 1e-3);
        assert!((fitted.width() - 888.888).abs() < 0.01);

        assert_eq!(letterbox(outer, (0, 0)), outer);
    }

    #[test]
    fn drag_samples_walk_from_the_start_to_the_end() {
        let samples = drag_samples((10.0, 20.0), (50.0, 100.0), 4);
        assert_eq!(
            samples,
            vec![(20.0, 40.0), (30.0, 60.0), (40.0, 80.0), (50.0, 100.0)]
        );

        let mut previous = (10.0f32, 20.0f32);
        let mut total = (0.0f32, 0.0f32);
        for sample in &drag_samples((10.0, 20.0), (50.0, 100.0), 8) {
            total.0 += sample.0 - previous.0;
            total.1 += sample.1 - previous.1;
            previous = *sample;
        }
        assert!((total.0 - 40.0).abs() < 1e-4);
        assert!((total.1 - 80.0).abs() < 1e-4);

        assert_eq!(drag_samples((0.0, 0.0), (5.0, 5.0), 0), vec![(5.0, 5.0)]);
    }

    #[test]
    fn game_cursor_stays_inside_the_render_target() {
        let rect = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(800.0, 600.0));
        let extent = viewport_extent(800.0, 600.0, 2.0, 1.0);
        assert_eq!(extent, (1600, 1200));

        assert_eq!(game_cursor(rect, rect.min, extent), [0.0, 0.0]);

        let corner = game_cursor(rect, rect.max, extent);
        assert_eq!(corner, [extent.0 as f32, extent.1 as f32]);

        let middle = game_cursor(rect, rect.center(), extent);
        assert_eq!(middle, [extent.0 as f32 / 2.0, extent.1 as f32 / 2.0]);
    }

    #[test]
    fn ui_rects_map_from_engine_pixels_to_panel_points() {
        let panel = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(400.0, 200.0));
        let mapped =
            ViewportState::ui_rect_to_viewport([100.0, 50.0, 200.0, 100.0], panel, (800, 400));
        assert_eq!(
            mapped,
            Rect::from_min_size(Pos2::new(60.0, 45.0), Vec2::new(100.0, 50.0))
        );
    }

    #[test]
    fn game_cursor_follows_the_clamped_extent() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(2560.0, 1440.0));
        let extent = viewport_extent(rect.width(), rect.height(), 2.0, 1.0);
        assert_eq!(extent, (4096, 2880));

        let right_edge = game_cursor(rect, Pos2::new(rect.max.x, 0.0), extent);
        assert_eq!(right_edge[0], extent.0 as f32);
        assert!(
            right_edge[0] <= extent.0 as f32,
            "cursor must not exceed InputSnapshot::screen ({} > {})",
            right_edge[0],
            extent.0
        );
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::*;
    use crate::test_harness::EguiHarness;
    use std::cell::RefCell;

    const STYLE: &str = "prism_engine::ui::Style";

    struct AnchorScene {
        workspace: Workspace,
        index: usize,
        parent: Rect,
        anchors: [f32; 4],
        dragging: Option<UiAnchorHandle>,
    }

    impl AnchorScene {
        fn draw(&mut self, ctx: &egui::Context) {
            egui::CentralPanel::default().show(ctx, |ui| {
                let response = ui.allocate_rect(self.parent, egui::Sense::click_and_drag());
                let handles = anchor_handles(self.anchors, self.parent);
                if response.drag_started_by(egui::PointerButton::Primary) {
                    if let Some(pos) = response.interact_pointer_pos() {
                        self.dragging = hit_handle(&handles, pos);
                    }
                }
                if response.dragged_by(egui::PointerButton::Primary) {
                    if let (Some(handle), Some(pos)) =
                        (self.dragging, response.interact_pointer_pos())
                    {
                        self.anchors =
                            anchors_from_drag(handle, pos, self.parent, self.anchors, false);
                    }
                }
                if response.drag_stopped_by(egui::PointerButton::Primary)
                    && self.dragging.take().is_some()
                {
                    self.workspace.set_ui_anchors_raw(self.index, self.anchors);
                }
                for (_, pos) in anchor_handles(self.anchors, self.parent) {
                    ui.painter()
                        .circle_filled(pos, HANDLE_RADIUS, egui::Color32::WHITE);
                }
            });
        }
    }

    fn style(anchors: [f32; 4]) -> serde_json::Value {
        serde_json::json!({
            "anchors": {
                "min_x": anchors[0],
                "min_y": anchors[1],
                "max_x": anchors[2],
                "max_y": anchors[3],
            }
        })
    }

    fn assert_style(workspace: &Workspace, expected: [f32; 4]) {
        let anchors = &workspace.document().doc.entities[1].components[STYLE]["anchors"];
        let actual = [
            anchors["min_x"].as_f64().expect("min_x") as f32,
            anchors["min_y"].as_f64().expect("min_y") as f32,
            anchors["max_x"].as_f64().expect("max_x") as f32,
            anchors["max_y"].as_f64().expect("max_y") as f32,
        ];
        for (actual, expected) in actual.iter().zip(expected.iter()) {
            assert!(
                (actual - expected).abs() <= 1e-5,
                "anchor {actual} != {expected}"
            );
        }
    }

    #[test]
    fn point_and_stretch_anchor_drags_use_explicit_frame_steps() {
        let mut workspace = Workspace::sample();
        workspace.document_mut().doc.entities[1]
            .components
            .insert(STYLE.to_string(), style([0.5; 4]));
        let parent = Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(200.0, 100.0));
        let scene = RefCell::new(AnchorScene {
            workspace,
            index: 1,
            parent,
            anchors: [0.5; 4],
            dragging: None,
        });
        let mut harness = EguiHarness::with_size(500.0, 300.0);
        let mut draw = |ctx: &egui::Context| scene.borrow_mut().draw(ctx);

        harness.drag(
            egui::pos2(200.0, 100.0),
            egui::pos2(250.0, 125.0),
            40,
            &mut draw,
        );
        assert_eq!(scene.borrow().anchors, [0.75, 0.75, 0.75, 0.75]);
        assert_style(&scene.borrow().workspace, [0.75, 0.75, 0.75, 0.75]);

        {
            let mut scene = scene.borrow_mut();
            scene.anchors = [0.0, 0.0, 1.0, 1.0];
            scene.workspace.document_mut().doc.entities[1]
                .components
                .insert(STYLE.to_string(), style([0.0, 0.0, 1.0, 1.0]));
        }
        harness.drag(
            egui::pos2(300.0, 100.0),
            egui::pos2(250.0, 100.0),
            40,
            &mut draw,
        );
        assert_eq!(scene.borrow().anchors, [0.0, 0.0, 0.75, 1.0]);
        assert_style(&scene.borrow().workspace, [0.0, 0.0, 0.75, 1.0]);
    }

    #[test]
    fn ui_anchor_handles_take_priority_over_world_gizmos() {
        assert_eq!(
            scene_drag_intent(true, true, true),
            SceneDragIntent::UiAnchor,
            "a UI handle must win even when a gizmo is under the same pixel"
        );
        assert_eq!(scene_drag_intent(false, true, true), SceneDragIntent::Gizmo);
        assert_eq!(
            scene_drag_intent(false, false, true),
            SceneDragIntent::BoxSelect
        );
        assert_eq!(
            scene_drag_intent(false, false, false),
            SceneDragIntent::None
        );
    }

    #[test]
    fn terrain_ray_hits_the_flat_field_and_maps_to_normalized_uv() {
        let field = Heightfield::flat(17);
        let hit = terrain_hit_uv(
            &field,
            [0.0, 0.0, 0.0],
            [10.0, 10.0],
            1.0,
            ([0.0, 5.0, 0.0], [0.0, -1.0, 0.0]),
        )
        .expect("vertical ray hits the flat terrain");
        assert!((hit.0 - 0.5).abs() < 1e-4);
        assert!((hit.1 - 0.5).abs() < 1e-4);

        let hit = terrain_hit_uv(
            &field,
            [0.0, 0.0, 0.0],
            [10.0, 10.0],
            1.0,
            ([2.5, 5.0, -2.5], [0.0, -1.0, 0.0]),
        )
        .expect("offset ray hits");
        assert!((hit.0 - 0.75).abs() < 1e-4);
        assert!((hit.1 - 0.25).abs() < 1e-4);
    }

    #[test]
    fn terrain_ray_misses_outside_the_footprint_and_upward() {
        let field = Heightfield::flat(17);
        assert!(terrain_hit_uv(
            &field,
            [0.0, 0.0, 0.0],
            [10.0, 10.0],
            1.0,
            ([20.0, 5.0, 0.0], [0.0, -1.0, 0.0]),
        )
        .is_none());
        assert!(terrain_hit_uv(
            &field,
            [0.0, 0.0, 0.0],
            [10.0, 10.0],
            1.0,
            ([0.0, 5.0, 0.0], [0.0, 1.0, 0.0]),
        )
        .is_none());
    }
}
