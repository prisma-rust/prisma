//! Viewport UI anchor, skeleton, sprite, collider, and component icon overlays.

use super::*;

pub(super) fn light_budget_label(
    workspace: &Workspace,
    kept: usize,
    total: usize,
) -> Option<String> {
    if total == 0 {
        return None;
    }
    if kept < total {
        Some(workspace.tf(
            "desktop.viewport.lightsDropped",
            &[
                ("kept", &kept.to_string()),
                ("total", &total.to_string()),
                ("dropped", &(total - kept).to_string()),
            ],
        ))
    } else {
        Some(workspace.tf("desktop.viewport.lights", &[("total", &total.to_string())]))
    }
}

#[cfg_attr(not(test), allow(dead_code, reason = "Used in viewport unit tests"))]
pub(super) fn icon_visible(workspace: &Workspace, is_camera: bool, is_light: bool) -> bool {
    (is_camera && workspace.show_camera_icons) || (is_light && workspace.show_light_icons)
}

pub(super) fn sculpt_terrain_at_ray(
    workspace: &Workspace,
    ray: ([f32; 3], [f32; 3]),
    settings: TerrainBrushSettings,
) -> TerrainSculptResult {
    let Some(index) = workspace.primary_selection() else {
        return Ok(None);
    };
    let doc = &workspace.document().doc;
    let Some(entity) = doc.entities.get(index) else {
        return Ok(None);
    };
    let Some(terrain) = entity
        .components
        .get("prism_engine::scene::Terrain")
        .and_then(Value::as_object)
    else {
        return Ok(None);
    };
    let heightmap = terrain
        .get("heightmap")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if heightmap.is_empty() {
        return Ok(None);
    }
    let size = terrain
        .get("size")
        .and_then(Value::as_array)
        .and_then(|size| {
            Some([
                size.first()?.as_f64()? as f32,
                size.get(1)?.as_f64()? as f32,
            ])
        })
        .unwrap_or([100.0, 100.0]);
    let height_scale = terrain
        .get("height_scale")
        .and_then(Value::as_f64)
        .unwrap_or(1.0) as f32;
    let resolution = terrain
        .get("resolution")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;

    let root = workspace
        .project_root
        .as_ref()
        .ok_or_else(|| "no project root; cannot edit the terrain heightmap".to_string())?;
    let relative = heightmap.strip_prefix("assets/").unwrap_or(heightmap);
    let path = root.join("assets").join(relative);
    let bytes =
        std::fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut field = if resolution == 0 {
        Heightfield::from_raw_auto(&bytes)
    } else {
        Heightfield::from_raw(&bytes, resolution)
    }
    .ok_or_else(|| format!("{} is not a valid heightmap", path.display()))?;

    let Some((u, v)) = terrain_hit_uv(
        &field,
        vm::world_position(doc, index),
        size,
        height_scale,
        ray,
    ) else {
        return Ok(None);
    };
    let affected = field.sculpt(u, v, settings.radius, settings.strength, settings.mode);
    if affected == 0 {
        return Ok(None);
    }
    std::fs::write(&path, field.to_raw_bytes())
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    Ok(Some((doc.id_of_index(index).unwrap_or(0), affected)))
}

pub(super) fn terrain_hit_uv(
    field: &Heightfield,
    terrain_origin: [f32; 3],
    size: [f32; 2],
    height_scale: f32,
    ray: ([f32; 3], [f32; 3]),
) -> Option<(f32, f32)> {
    let half = [size[0].max(0.001) * 0.5, size[1].max(0.001) * 0.5];
    let (ray_origin, direction) = ray;
    let along = |t: f32| {
        [
            ray_origin[0] + direction[0] * t,
            ray_origin[1] + direction[1] * t,
            ray_origin[2] + direction[2] * t,
        ]
    };
    let sample = |point: [f32; 3]| -> Option<(f32, f32)> {
        let x = point[0] - terrain_origin[0];
        let z = point[2] - terrain_origin[2];
        if x.abs() > half[0] || z.abs() > half[1] {
            return None;
        }
        Some((x / (half[0] * 2.0) + 0.5, z / (half[1] * 2.0) + 0.5))
    };

    let max_distance = 5000.0;
    let samples = 1024;
    let mut previous: Option<f32> = None;
    for step in 1..=samples {
        let t = max_distance * step as f32 / samples as f32;
        let point = along(t);
        let Some((u, v)) = sample(point) else {
            previous = None;
            continue;
        };
        let height = field.sample_height(u, v) * height_scale + terrain_origin[1];
        if point[1] <= height {
            let mut lo = previous.unwrap_or(0.0);
            let mut hi = t;
            for _ in 0..12 {
                let mid = (lo + hi) * 0.5;
                let point = along(mid);
                let Some((u, v)) = sample(point) else {
                    break;
                };
                let height = field.sample_height(u, v) * height_scale + terrain_origin[1];
                if point[1] > height {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            return sample(along((lo + hi) * 0.5));
        }
        previous = Some(t);
    }
    None
}

impl ViewportState {
    pub(super) fn ui_style<'a>(
        &self,
        workspace: &'a Workspace,
        index: usize,
    ) -> Option<&'a serde_json::Value> {
        workspace
            .document()
            .doc
            .entities
            .get(index)?
            .components
            .get("prism_engine::ui::Style")
    }

    pub(super) fn ui_anchors(style: &serde_json::Value) -> [f32; 4] {
        let number = |field: &str, fallback: f32| {
            style
                .get("anchors")
                .and_then(|anchors| anchors.get(field))
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(fallback as f64) as f32
        };
        [
            number("min_x", 0.0),
            number("min_y", 0.0),
            number("max_x", 1.0),
            number("max_y", 1.0),
        ]
    }

    pub(super) fn ui_rect_to_viewport(ui_rect: [f32; 4], rect: Rect, extent: (u32, u32)) -> Rect {
        let sx = rect.width() / extent.0.max(1) as f32;
        let sy = rect.height() / extent.1.max(1) as f32;
        Rect::from_min_size(
            rect.min + egui::vec2(ui_rect[0] * sx, ui_rect[1] * sy),
            egui::vec2(ui_rect[2] * sx, ui_rect[3] * sy),
        )
    }

    pub(super) fn ui_handle_at(
        &self,
        workspace: &Workspace,
        rect: Rect,
        extent: (u32, u32),
        pointer: egui::Pos2,
    ) -> Option<UiAnchorDrag> {
        if self.kind() != ViewKind::Scene {
            return None;
        }
        for index in &workspace.selection {
            if workspace.is_entity_hidden(*index) {
                continue;
            }
            let Some(style) = self.ui_style(workspace, *index) else {
                continue;
            };
            let Some((ui_rect, parent)) = self.host.ui_rect(*index) else {
                continue;
            };
            let parent_rect = Self::ui_rect_to_viewport(parent, rect, extent);
            let anchors = self
                .ui_drag
                .as_ref()
                .filter(|drag| drag.index == *index)
                .map(|drag| drag.anchors)
                .unwrap_or_else(|| Self::ui_anchors(style));
            let handles = anchor_handles(anchors, parent_rect);
            let Some(handle) = hit_handle(&handles, pointer) else {
                continue;
            };
            return Some(UiAnchorDrag {
                index: *index,
                handle,
                anchors,
                start_anchors: anchors,
                parent,
                rect: ui_rect,
            });
        }
        None
    }

    pub(super) fn draw_ui_rects(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        workspace: &Workspace,
        extent: (u32, u32),
    ) {
        use egui::Color32;

        if workspace.selection.is_empty() || self.kind() != ViewKind::Scene {
            return;
        }
        let accent = Color32::from_rgb(90, 200, 255);
        let active = Color32::from_rgb(255, 196, 90);
        let stroke = egui::Stroke::new(1.2_f32, accent);

        for index in &workspace.selection {
            if workspace.is_entity_hidden(*index) {
                continue;
            }
            let Some(style) = self.ui_style(workspace, *index) else {
                continue;
            };
            let Some((ui_rect, parent)) = self.host.ui_rect(*index) else {
                continue;
            };
            let active_drag = self.ui_drag.as_ref().filter(|drag| drag.index == *index);
            let anchors = active_drag
                .map(|drag| drag.anchors)
                .unwrap_or_else(|| Self::ui_anchors(style));
            let view_rect = Self::ui_rect_to_viewport(ui_rect, rect, extent);
            let parent_rect = Self::ui_rect_to_viewport(parent, rect, extent);

            painter.rect_stroke(
                view_rect,
                egui::CornerRadius::ZERO,
                stroke,
                egui::StrokeKind::Inside,
            );
            for (handle, position) in anchor_handles(anchors, parent_rect) {
                let selected = active_drag.is_some_and(|drag| drag.handle == handle);
                let color = if selected { active } else { accent };
                let size = HANDLE_RADIUS * 2.0;
                let handle_rect = egui::Rect::from_center_size(position, egui::vec2(size, size));
                painter.rect_filled(handle_rect, egui::CornerRadius::ZERO, color);
                painter.rect_stroke(
                    handle_rect,
                    egui::CornerRadius::ZERO,
                    egui::Stroke::new(1.0_f32, Color32::from_gray(24)),
                    egui::StrokeKind::Inside,
                );
            }
        }
    }

    pub(super) fn draw_skeleton(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        workspace: &Workspace,
        frame: &vm::CameraFrame,
    ) {
        use egui::Color32;

        if workspace.selection.is_empty() {
            return;
        }
        let bone = Color32::from_rgb(255, 206, 84);
        let joint = Color32::from_rgb(255, 236, 168);
        let (width, height) = (rect.width(), rect.height());

        for index in &workspace.selection {
            let overlay = self.host.skeleton_overlay(*index);
            if overlay.is_empty() {
                continue;
            }
            let screen: Vec<Option<egui::Pos2>> = overlay
                .joints
                .iter()
                .map(|position| {
                    vm::project(*position, frame, width, height)
                        .map(|(x, y, _)| rect.min + egui::vec2(x, y))
                })
                .collect();
            for (parent, child) in &overlay.segments {
                if let Some(((ax, ay), (bx, by))) = vm::project_clipped_segment(
                    overlay.joints[*parent],
                    overlay.joints[*child],
                    frame,
                    width,
                    height,
                ) {
                    painter.line_segment(
                        [rect.min + egui::vec2(ax, ay), rect.min + egui::vec2(bx, by)],
                        egui::Stroke::new(1.6_f32, bone),
                    );
                }
            }
            for (index, point) in screen.iter().enumerate() {
                let Some(point) = point else {
                    continue;
                };

                let radius = if index == 0 { 3.5 } else { 2.5 };
                painter.circle_filled(*point, radius, joint);
            }
        }
    }

    pub(super) fn draw_selection_outline(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        workspace: &Workspace,
        frame: &vm::CameraFrame,
    ) {
        use egui::Color32;

        if workspace.selection.is_empty() {
            return;
        }
        let doc = &workspace.document().doc;
        let selected: std::collections::HashSet<usize> =
            workspace.selection.iter().copied().collect();

        let mut related: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for index in &selected {
            let mut current = doc.entities.get(*index).and_then(|entity| entity.parent);
            while let Some(id) = current {
                let Some(parent) = doc.index_of_id(id) else {
                    break;
                };
                if !selected.contains(&parent) {
                    related.insert(parent);
                }
                current = doc.entities.get(parent).and_then(|entity| entity.parent);
            }
            for child in doc.descendants(*index) {
                if !selected.contains(&child) {
                    related.insert(child);
                }
            }
        }

        let accent = Color32::from_rgb(255, 154, 60);
        let helper = Color32::from_rgb(255, 154, 60).gamma_multiply(0.45);

        for (index, transform, _) in proxies(workspace, None) {
            let is_selected = selected.contains(&index);
            let is_related = related.contains(&index);
            if !is_selected && !is_related {
                continue;
            }

            let is_mesh = doc
                .entities
                .get(index)
                .is_some_and(|entity| entity.has_component("prism_engine::scene::MeshRenderer"));
            if is_selected && is_mesh {
                continue;
            }
            let edges =
                vm::projected_box_clipped_edges(&transform, frame, rect.width(), rect.height());
            let stroke = if is_selected {
                egui::Stroke::new(1.6_f32, accent)
            } else {
                egui::Stroke::new(1.0_f32, helper)
            };
            for (a, b) in edges {
                painter.line_segment(
                    [
                        rect.min + egui::vec2(a.0, a.1),
                        rect.min + egui::vec2(b.0, b.1),
                    ],
                    stroke,
                );
            }
        }
    }

    pub(super) fn draw_sprite_rects(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        workspace: &Workspace,
        frame: &vm::CameraFrame,
    ) {
        use egui::Color32;

        const SPRITE: &str = "prism_engine::scene::SpriteRenderer";

        if workspace.selection.is_empty() {
            return;
        }
        let width = rect.width();
        let height = rect.height();
        let stroke = egui::Stroke::new(1.4_f32, Color32::from_rgb(90, 200, 255));
        let pivot_color = Color32::from_rgb(90, 200, 255);

        for (index, transform, _) in proxies(workspace, None) {
            if !workspace.selection.contains(&index) {
                continue;
            }
            let doc = &workspace.document().doc;
            let Some(entity) = doc.entities.get(index) else {
                continue;
            };
            let Some(sprite) = entity.component(SPRITE) else {
                continue;
            };
            let number = |field: &str| sprite.get(field).and_then(|value| value.as_f64());
            let axis = |field: &str, axis: usize| {
                sprite
                    .get(field)
                    .and_then(|value| value.as_array())
                    .and_then(|values| values.get(axis))
                    .and_then(|value| value.as_f64())
            };
            let ppu = number("pixels_per_unit").unwrap_or(100.0).max(0.0001);
            let frame_w = axis("frame_size", 0).unwrap_or(0.0);
            let frame_h = axis("frame_size", 1).unwrap_or(0.0);
            if frame_w <= 0.0 || frame_h <= 0.0 {
                continue;
            }
            let size = [(frame_w / ppu) as f32, (frame_h / ppu) as f32];
            let pivot = [
                axis("pivot", 0).unwrap_or(0.5) as f32,
                axis("pivot", 1).unwrap_or(0.5) as f32,
            ];
            let Some(edges) =
                vm::projected_rect_edges(&transform, size, pivot, frame, width, height)
            else {
                continue;
            };
            for (a, b) in edges {
                painter.line_segment(
                    [
                        rect.min + egui::vec2(a.0, a.1),
                        rect.min + egui::vec2(b.0, b.1),
                    ],
                    stroke,
                );

                painter.rect_filled(
                    egui::Rect::from_center_size(
                        rect.min + egui::vec2(a.0, a.1),
                        egui::vec2(5.0, 5.0),
                    ),
                    0.0,
                    stroke.color,
                );
            }
            if let Some((x, y, _)) = vm::project(transform.position, frame, width, height) {
                painter.circle_filled(rect.min + egui::vec2(x, y), 3.0, pivot_color);
            }
        }
    }

    /// SceneView Gizmos executed through [`GizmoRegistry`] plus PRT probe visualization.
    pub(super) fn draw_component_icons(
        &mut self,
        painter: &egui::Painter,
        rect: Rect,
        workspace: &Workspace,
        frame: &vm::CameraFrame,
    ) -> Vec<(usize, egui::Pos2)> {
        let doc = &workspace.document().doc;
        let (width, height) = (rect.width(), rect.height());
        let world_len = match workspace.gizmo_center_world() {
            Some(center) => vm::gizmo_world_length_at(&workspace.camera, center, height),
            None => vm::gizmo_world_length(&workspace.camera, height),
        };
        let aspect = if width > 0.0 && height > 0.0 {
            width / height
        } else {
            16.0 / 9.0
        };
        let hidden_entities = workspace.hidden_entity_ids();

        let mut gizmos = GizmoContext::new();
        let filter = GizmoFilter {
            show_cameras: workspace.show_camera_icons,
            show_lights: workspace.show_light_icons,
            show_colliders: workspace.show_collider_gizmos,
            show_audio: workspace.show_audio_gizmos,
            prt: &workspace.prt_gizmos,
        };
        self.gizmo_registry.collect_scene(
            doc,
            &workspace.selection,
            &hidden_entities,
            None,
            aspect,
            world_len,
            &filter,
            &mut gizmos,
        );

        let should_load_prt = workspace.prt_gizmos.enabled
            || workspace.selection.iter().any(|&idx| {
                doc.entities
                    .get(idx)
                    .is_some_and(|e| e.has_component("prism_engine::scene::GiConfig"))
            });
        if should_load_prt {
            let prt_settings = workspace.prt_gizmos.clone();
            let focus_world = workspace.gizmo_center_world();
            if let Some(volume) = self.ensure_probe_volume(workspace) {
                emit_prt_probe_gizmos(volume, &prt_settings, focus_world, frame.eye, &mut gizmos);
            }
        }

        Self::render_gizmo_commands(painter, rect, frame, gizmos.commands())
    }
}
