//! Viewport 3D/2D transform gizmos, orientation cube, and component gizmos.

use super::*;

pub(super) fn orientation_gizmo_center(rect: Rect) -> (f32, f32) {
    (rect.max.x - 46.0, rect.min.y + 46.0)
}

pub(super) const ORIENTATION_GIZMO_RADIUS: f32 = 30.0;

pub(super) fn orientation_axis_color(axis: usize) -> (u8, u8, u8) {
    match axis {
        0 => (214, 104, 104),
        1 => (120, 196, 120),
        _ => (104, 150, 232),
    }
}

pub(super) fn draw_orientation_gizmo(painter: &egui::Painter, rect: Rect, frame: &vm::CameraFrame) {
    use egui::Color32;
    let center = orientation_gizmo_center(rect);
    painter.circle_filled(
        egui::pos2(center.0, center.1),
        24.0,
        Color32::from_rgba_unmultiplied(16, 18, 22, 130),
    );
    let mut axes: Vec<(usize, f32, f32)> = Vec::new();
    for axis in 0..3 {
        for sign in [1.0_f32, -1.0] {
            let end = vm::orientation_axis_end(frame, axis, sign, center, ORIENTATION_GIZMO_RADIUS);

            if (end.0 - center.0).abs() + (end.1 - center.1).abs() < 1e-3 {
                continue;
            }
            let world = match axis {
                0 => [sign, 0.0, 0.0],
                1 => [0.0, sign, 0.0],
                _ => [0.0, 0.0, sign],
            };
            axes.push((axis, sign, vm::dot(world, frame.forward)));
        }
    }
    axes.sort_by(|a, b| a.2.total_cmp(&b.2));
    for (axis, sign, depth) in axes {
        let end = vm::orientation_axis_end(frame, axis, sign, center, ORIENTATION_GIZMO_RADIUS);
        let (r, g, b) = orientation_axis_color(axis);
        let alpha = if depth > 0.15 {
            110
        } else if depth < -0.15 {
            235
        } else {
            170
        };
        let position = egui::pos2(end.0, end.1);
        painter.circle_filled(
            position,
            7.0,
            Color32::from_rgba_unmultiplied(r, g, b, alpha),
        );
        painter.text(
            position + egui::vec2(9.0, 0.0),
            egui::Align2::LEFT_CENTER,
            format!(
                "{}{}",
                if sign < 0.0 { "-" } else { "+" },
                ["X", "Y", "Z"][axis]
            ),
            egui::FontId::proportional(9.5),
            Color32::from_rgba_unmultiplied(r, g, b, alpha),
        );
    }
}

pub(crate) struct GizmoDrag {
    pub(crate) handle: vm::GizmoHandle,
    pub(crate) targets: Vec<u32>,
    pub(crate) axis: usize,
    pub(crate) scale_targets: Vec<(u32, usize, f32)>,
    pub(crate) multi_scale_targets: Vec<(usize, Vec<(u32, usize, f32)>, f32)>,
    pub(crate) screen_dir: [f32; 2],
    pub(crate) px_per_unit: f32,
    pub(crate) last_angle: f32,
    pub(crate) start_scale: f32,
    pub(crate) start_axis_value: f32,
    pub(crate) accum: f32,
    pub(crate) applied: f32,
    pub(crate) world_length: f32,
    pub(crate) axis_origin: [f32; 3],
    pub(crate) axis_dir: [f32; 3],
    pub(crate) plane_normal: [f32; 3],
    pub(crate) plane_u: [f32; 3],
    pub(crate) plane_v: [f32; 3],
    pub(crate) rot_axis_world: [f32; 3],
    pub(crate) last_ray_t: Option<f32>,
    pub(crate) last_plane_hit: Option<[f32; 3]>,
    pub(crate) plane_accum: [f32; 2],
    pub(crate) plane_applied: [f32; 2],
    pub(crate) last_ring_vec: Option<[f32; 3]>,
}

pub(crate) struct UiAnchorDrag {
    pub(crate) index: usize,
    pub(crate) handle: UiAnchorHandle,
    pub(crate) anchors: [f32; 4],
    pub(crate) start_anchors: [f32; 4],
    pub(crate) parent: [f32; 4],
    pub(crate) rect: [f32; 4],
}

impl ViewportState {
    pub(crate) fn gizmo_hit_at(
        &self,
        workspace: &Workspace,
        frame: &vm::CameraFrame,
        rect: Rect,
        local: (f32, f32),
    ) -> Option<(vm::GizmoHandle, [f32; 2], f32)> {
        let center = workspace.gizmo_center_world()?;
        let basis = workspace.gizmo_basis();
        let world_length = vm::gizmo_world_length_at(&workspace.camera, center, rect.height());
        let ortho_2d = workspace.camera.orthographic;
        match self.tool {
            GizmoTool::Rotate => vm::pick_rotation_handle_oriented(
                local,
                frame,
                center,
                basis,
                world_length,
                ortho_2d,
                rect.width(),
                rect.height(),
            )
            .map(|handle| (handle, [1.0f32, 0.0], 60.0)),
            GizmoTool::Translate | GizmoTool::Scale => vm::pick_gizmo_handle_oriented(
                local,
                frame,
                center,
                basis,
                world_length,
                ortho_2d,
                rect.width(),
                rect.height(),
            ),
        }
    }

    pub(crate) fn finish_gizmo_drag(&mut self, workspace: &mut Workspace) {
        if self.gizmo.take().is_some() {
            workspace.end_interaction();
        }
    }

    pub(crate) fn begin_gizmo_drag(
        &mut self,
        workspace: &mut Workspace,
        frame: &vm::CameraFrame,
        rect: Rect,
        local: (f32, f32),
    ) -> bool {
        let Some(center) = workspace.gizmo_center_world() else {
            return false;
        };
        let Some(entity) = workspace
            .primary_selection()
            .and_then(|index| workspace.document().doc.id_of_index(index))
        else {
            return false;
        };
        let world_length = vm::gizmo_world_length_at(&workspace.camera, center, rect.height());
        let basis = workspace.gizmo_basis();
        let Some((handle, dir, px_per_unit)) = self.gizmo_hit_at(workspace, frame, rect, local)
        else {
            return false;
        };

        let axis = match handle {
            vm::GizmoHandle::Axis(ax) => ax,
            vm::GizmoHandle::Plane(u, _) => u,
            vm::GizmoHandle::Center | vm::GizmoHandle::ScreenRing => 0,
        };
        let targets: Vec<u32> = workspace.selection_entity_ids();
        let collect_scale_targets = |ax: usize| -> (Vec<(u32, usize, f32)>, f32) {
            let list: Vec<(u32, usize, f32)> = targets
                .iter()
                .filter_map(|entity_id| {
                    let index = workspace.document().doc.index_of_id(*entity_id)?;
                    let scale_axis = workspace.gizmo_scale_axis(*entity_id, ax);
                    let start =
                        workspace.document().doc.entities[index].transform.scale[scale_axis];
                    Some((*entity_id, scale_axis, start))
                })
                .collect();
            let first = workspace.document().doc.entities
                [workspace.document().doc.index_of_id(entity).unwrap_or(0)]
            .transform
            .scale[workspace.gizmo_scale_axis(entity, ax)];
            (list, first)
        };
        let (scale_targets, start_scale) = collect_scale_targets(axis);
        let multi_scale_targets = match handle {
            vm::GizmoHandle::Plane(u, v) => {
                let (su, fu) = collect_scale_targets(u);
                let (sv, fv) = collect_scale_targets(v);
                vec![(u, su, fu), (v, sv, fv)]
            }
            vm::GizmoHandle::Center => (0..3)
                .map(|ax| {
                    let (st, fs) = collect_scale_targets(ax);
                    (ax, st, fs)
                })
                .collect(),
            _ => Vec::new(),
        };

        let angle_center = vm::project(center, frame, rect.width(), rect.height())
            .map(|(cx, cy, _)| (cx, cy))
            .unwrap_or((rect.width() * 0.5, rect.height() * 0.5));
        let start_axis_value = {
            let index = workspace.document().doc.index_of_id(entity).unwrap_or(0);
            let position = vm::world_position(&workspace.document().doc, index);
            position[0] * basis[axis][0]
                + position[1] * basis[axis][1]
                + position[2] * basis[axis][2]
        };
        let applied = match self.tool {
            GizmoTool::Scale => start_scale,
            GizmoTool::Translate | GizmoTool::Rotate => 0.0,
        };

        let ray = vm::ray_from_screen(local.0, local.1, frame, rect.width(), rect.height());
        let axis_dir = basis[axis];
        let (plane_normal, plane_u, plane_v) = match handle {
            vm::GizmoHandle::Plane(u, v) => (
                vm::normalize(vm::cross(basis[u], basis[v])),
                basis[u],
                basis[v],
            ),
            vm::GizmoHandle::Center => (frame.forward, frame.right, frame.up),
            vm::GizmoHandle::Axis(_) | vm::GizmoHandle::ScreenRing => {
                (frame.forward, frame.right, frame.up)
            }
        };
        let rot_axis_world = match handle {
            vm::GizmoHandle::ScreenRing => frame.forward,
            _ => basis[axis],
        };
        let last_ray_t = matches!(handle, vm::GizmoHandle::Axis(_))
            .then(|| vm::ray_closest_point_on_axis(ray.0, ray.1, center, axis_dir))
            .flatten();
        let last_plane_hit = matches!(
            handle,
            vm::GizmoHandle::Plane(_, _) | vm::GizmoHandle::Center
        )
        .then(|| {
            vm::ray_intersect_plane(
                ray.0,
                ray.1,
                center,
                plane_normal,
                frame.ortho_half_height.is_some(),
            )
        })
        .flatten();
        let last_ring_vec = if self.tool == GizmoTool::Rotate
            && vm::dot(rot_axis_world, frame.forward).abs() > 0.15
        {
            vm::ray_intersect_plane(
                ray.0,
                ray.1,
                center,
                rot_axis_world,
                frame.ortho_half_height.is_some(),
            )
            .map(|hit| vm::sub(hit, center))
        } else {
            None
        };

        self.gizmo = Some(GizmoDrag {
            handle,
            targets,
            axis,
            scale_targets,
            multi_scale_targets,
            screen_dir: dir,
            px_per_unit,
            last_angle: vm::screen_angle(local, angle_center),
            start_scale,
            start_axis_value,
            accum: 0.0,
            applied,
            world_length,
            axis_origin: center,
            axis_dir,
            plane_normal,
            plane_u,
            plane_v,
            rot_axis_world,
            last_ray_t,
            last_plane_hit,
            plane_accum: [0.0, 0.0],
            plane_applied: [0.0, 0.0],
            last_ring_vec,
        });

        workspace.begin_interaction();
        self.box_start = None;
        true
    }

    pub(crate) fn gizmo_drag_step(
        &mut self,
        workspace: &mut Workspace,
        frame: &vm::CameraFrame,
        rect: Rect,
        local: (f32, f32),
        delta: (f32, f32),
        ctrl: bool,
    ) -> bool {
        let tool = self.tool;
        let Some(drag) = self.gizmo.as_mut() else {
            return false;
        };
        let axis = drag.axis;
        let handle = drag.handle;
        let snap = workspace.snap;
        let snapping = snap_active(snap.enabled, ctrl);
        let ray = vm::ray_from_screen(local.0, local.1, frame, rect.width(), rect.height());
        let mut changed = false;
        match tool {
            GizmoTool::Translate => match handle {
                vm::GizmoHandle::Axis(_) => {
                    let parallel_ray = vm::dot(ray.1, drag.axis_dir).abs() > 0.96;
                    let ray_step = if !parallel_ray {
                        if let Some(t) = vm::ray_closest_point_on_axis(
                            ray.0,
                            ray.1,
                            drag.axis_origin,
                            drag.axis_dir,
                        ) {
                            let s = drag.last_ray_t.map(|prev| t - prev).unwrap_or(0.0);
                            drag.last_ray_t = Some(t);
                            Some(s)
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    let step = ray_step.unwrap_or_else(|| {
                        (delta.0 * drag.screen_dir[0] + delta.1 * drag.screen_dir[1])
                            / drag.px_per_unit.max(4.0)
                    });
                    if step.is_finite() && step.abs() > f32::EPSILON {
                        drag.accum += step;
                        let apply = drag_translate_step(
                            snap,
                            snapping,
                            drag.start_axis_value,
                            drag.accum,
                            drag.applied,
                        );
                        if apply.abs() > 1e-6 {
                            let world = drag.axis_dir.map(|c| c * apply);
                            workspace.translate_entities_world(&drag.targets, world);
                            drag.applied += apply;
                            changed = true;
                        }
                    }
                }
                vm::GizmoHandle::Plane(_, _) | vm::GizmoHandle::Center => {
                    if let Some(hit) = vm::ray_intersect_plane(
                        ray.0,
                        ray.1,
                        drag.axis_origin,
                        drag.plane_normal,
                        frame.ortho_half_height.is_some(),
                    ) {
                        if let Some(prev) = drag.last_plane_hit {
                            let d = vm::sub(hit, prev);
                            let du = vm::dot(d, drag.plane_u);
                            let dv = vm::dot(d, drag.plane_v);
                            drag.plane_accum[0] += du;
                            drag.plane_accum[1] += dv;
                            let target_u = if snapping {
                                snap.snapped_translate(drag.plane_accum[0])
                            } else {
                                drag.plane_accum[0]
                            };
                            let target_v = if snapping {
                                snap.snapped_translate(drag.plane_accum[1])
                            } else {
                                drag.plane_accum[1]
                            };
                            let apply_u = target_u - drag.plane_applied[0];
                            let apply_v = target_v - drag.plane_applied[1];
                            if apply_u.abs() > 1e-6 || apply_v.abs() > 1e-6 {
                                let world = vm::add(
                                    vm::scale(drag.plane_u, apply_u),
                                    vm::scale(drag.plane_v, apply_v),
                                );
                                workspace.translate_entities_world(&drag.targets, world);
                                drag.plane_applied[0] += apply_u;
                                drag.plane_applied[1] += apply_v;
                                changed = true;
                            }
                        }
                        drag.last_plane_hit = Some(hit);
                    }
                }
                vm::GizmoHandle::ScreenRing => {}
            },
            GizmoTool::Rotate => {
                let delta_angle = if vm::dot(drag.rot_axis_world, frame.forward).abs() > 0.15 {
                    if let Some(hit) = vm::ray_intersect_plane(
                        ray.0,
                        ray.1,
                        drag.axis_origin,
                        drag.rot_axis_world,
                        frame.ortho_half_height.is_some(),
                    ) {
                        let next_vec = vm::sub(hit, drag.axis_origin);
                        let da = drag
                            .last_ring_vec
                            .map(|prev| {
                                vm::signed_angle_around_axis(prev, next_vec, drag.rot_axis_world)
                            })
                            .unwrap_or(0.0);
                        drag.last_ring_vec = Some(next_vec);
                        da
                    } else {
                        0.0
                    }
                } else {
                    let center = workspace.gizmo_center_world().unwrap_or(drag.axis_origin);
                    let angle_center = vm::project(center, frame, rect.width(), rect.height())
                        .map(|(cx, cy, _)| (cx, cy))
                        .unwrap_or((rect.width() * 0.5, rect.height() * 0.5));
                    let angle = vm::screen_angle(local, angle_center);
                    let da = vm::wrap_angle(angle - drag.last_angle);
                    drag.last_angle = angle;
                    let sign = if vm::dot(drag.rot_axis_world, frame.forward) >= 0.0 {
                        1.0
                    } else {
                        -1.0
                    };
                    da * sign
                };
                if delta_angle.is_finite() && delta_angle.abs() > 1e-4 {
                    drag.accum += delta_angle;
                    let desired = if snapping {
                        snap.snapped_radians(drag.accum)
                    } else {
                        drag.accum
                    };
                    let apply = desired - drag.applied;
                    if apply.abs() > 1e-4 {
                        match handle {
                            vm::GizmoHandle::ScreenRing => {
                                workspace.rotate_entities_axis_dir(
                                    &drag.targets,
                                    drag.rot_axis_world,
                                    apply,
                                );
                            }
                            _ => {
                                workspace.rotate_entities(&drag.targets, axis, apply);
                            }
                        }
                        drag.applied = desired;
                        changed = true;
                    }
                }
            }
            GizmoTool::Scale => match handle {
                vm::GizmoHandle::Axis(_) => {
                    let step = if let Some(t) =
                        vm::ray_closest_point_on_axis(ray.0, ray.1, drag.axis_origin, drag.axis_dir)
                    {
                        let s = drag
                            .last_ray_t
                            .map(|prev| (t - prev) / drag.world_length.max(0.1))
                            .unwrap_or(0.0);
                        drag.last_ray_t = Some(t);
                        s
                    } else {
                        (delta.0 * drag.screen_dir[0] + delta.1 * drag.screen_dir[1])
                            / drag.px_per_unit.max(4.0)
                    };
                    drag.accum += step;
                    let raw = (drag.start_scale + drag.accum).max(0.01);
                    let next = if snapping {
                        snap.snapped_scale(raw)
                    } else {
                        raw
                    };
                    if (next - drag.applied).abs() > 1e-4 {
                        let ratio = next / drag.start_scale.max(1e-4);
                        workspace.scale_entities_gizmo(&drag.scale_targets, axis, ratio);
                        drag.applied = next;
                        changed = true;
                    }
                }
                vm::GizmoHandle::Plane(_, _) | vm::GizmoHandle::Center => {
                    let step = (delta.0 - delta.1) * 0.01;
                    drag.accum += step;
                    let ratio_raw = (1.0 + drag.accum).max(0.05);
                    let ratio = if snapping {
                        snap.snapped_scale(ratio_raw)
                    } else {
                        ratio_raw
                    };
                    if (ratio - (1.0 + drag.applied)).abs() > 1e-4 {
                        for (ax, targets_for_ax, _) in &drag.multi_scale_targets {
                            workspace.scale_entities_gizmo(targets_for_ax, *ax, ratio);
                        }
                        drag.applied = ratio - 1.0;
                        changed = true;
                    }
                }
                vm::GizmoHandle::ScreenRing => {}
            },
        }
        self.dirty = true;
        changed
    }

    pub(super) fn render_gizmo_commands(
        painter: &egui::Painter,
        rect: Rect,
        frame: &vm::CameraFrame,
        commands: &[GizmoCommand],
    ) -> Vec<(usize, egui::Pos2)> {
        use egui::Color32;

        let (width, height) = (rect.width(), rect.height());
        let project_pt = |p: [f32; 3]| -> Option<egui::Pos2> {
            vm::project(p, frame, width, height).map(|(x, y, _)| rect.min + egui::vec2(x, y))
        };
        let clip_seg = |a: [f32; 3], b: [f32; 3]| -> Option<(egui::Pos2, egui::Pos2)> {
            vm::project_clipped_segment(a, b, frame, width, height).map(|((ax, ay), (bx, by))| {
                (rect.min + egui::vec2(ax, ay), rect.min + egui::vec2(bx, by))
            })
        };
        let to_c32 = |c: [u8; 4]| Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
        let unit_ring = unit_ring_points(24);
        let mut hits = Vec::new();
        let mut sh_mesh = egui::Mesh::default();

        for cmd in commands {
            match cmd {
                GizmoCommand::Line {
                    start,
                    end,
                    color,
                    width: w,
                } => {
                    if let Some((a, b)) = clip_seg(*start, *end) {
                        painter.line_segment([a, b], egui::Stroke::new(*w, to_c32(*color)));
                    }
                }
                GizmoCommand::WireBox {
                    center,
                    half_extents,
                    rotation,
                    color,
                    width: w,
                } => {
                    Self::draw_wire_box_cmd(
                        painter,
                        &clip_seg,
                        *center,
                        *half_extents,
                        *rotation,
                        egui::Stroke::new(*w, to_c32(*color)),
                    );
                }
                GizmoCommand::WireSphere {
                    center,
                    radius,
                    rotation,
                    color,
                    width: w,
                } => {
                    let q =
                        glam::Quat::from_xyzw(rotation[0], rotation[1], rotation[2], rotation[3])
                            .normalize();
                    let ax = (q * glam::Vec3::X).to_array();
                    let ay = (q * glam::Vec3::Y).to_array();
                    let az = (q * glam::Vec3::Z).to_array();
                    let stroke = egui::Stroke::new(*w, to_c32(*color));
                    Self::draw_wire_ring_cmd(
                        painter, &clip_seg, &unit_ring, *center, ax, ay, *radius, stroke,
                    );
                    Self::draw_wire_ring_cmd(
                        painter, &clip_seg, &unit_ring, *center, ax, az, *radius, stroke,
                    );
                    Self::draw_wire_ring_cmd(
                        painter, &clip_seg, &unit_ring, *center, ay, az, *radius, stroke,
                    );
                }
                GizmoCommand::WireCapsule {
                    center,
                    radius,
                    half_height,
                    axis: _,
                    rotation,
                    color,
                    width: w,
                } => {
                    let q =
                        glam::Quat::from_xyzw(rotation[0], rotation[1], rotation[2], rotation[3])
                            .normalize();
                    let ax = (q * glam::Vec3::X).to_array();
                    let ay = (q * glam::Vec3::Y).to_array();
                    let az = (q * glam::Vec3::Z).to_array();
                    let stroke = egui::Stroke::new(*w, to_c32(*color));
                    let top = [
                        center[0] + ay[0] * *half_height,
                        center[1] + ay[1] * *half_height,
                        center[2] + ay[2] * *half_height,
                    ];
                    let bot = [
                        center[0] - ay[0] * *half_height,
                        center[1] - ay[1] * *half_height,
                        center[2] - ay[2] * *half_height,
                    ];
                    Self::draw_wire_ring_cmd(
                        painter, &clip_seg, &unit_ring, top, ax, az, *radius, stroke,
                    );
                    Self::draw_wire_ring_cmd(
                        painter, &clip_seg, &unit_ring, bot, ax, az, *radius, stroke,
                    );
                    Self::draw_wire_ring_cmd(
                        painter, &clip_seg, &unit_ring, top, ax, ay, *radius, stroke,
                    );
                    Self::draw_wire_ring_cmd(
                        painter, &clip_seg, &unit_ring, bot, ax, ay, *radius, stroke,
                    );
                    for dir in [ax, [-ax[0], -ax[1], -ax[2]], az, [-az[0], -az[1], -az[2]]] {
                        let a = [
                            top[0] + dir[0] * *radius,
                            top[1] + dir[1] * *radius,
                            top[2] + dir[2] * *radius,
                        ];
                        let b = [
                            bot[0] + dir[0] * *radius,
                            bot[1] + dir[1] * *radius,
                            bot[2] + dir[2] * *radius,
                        ];
                        if let Some((pa, pb)) = clip_seg(a, b) {
                            painter.line_segment([pa, pb], stroke);
                        }
                    }
                }
                GizmoCommand::WireCone {
                    apex,
                    direction,
                    length,
                    half_angle_rad,
                    basis_x,
                    basis_y,
                    color,
                    width: w,
                } => {
                    let stroke = egui::Stroke::new(*w, to_c32(*color));
                    let dist = *length * half_angle_rad.cos();
                    let ring_r = *length * half_angle_rad.sin();
                    let ring_center = [
                        apex[0] + direction[0] * dist,
                        apex[1] + direction[1] * dist,
                        apex[2] + direction[2] * dist,
                    ];
                    Self::draw_wire_ring_cmd(
                        painter,
                        &clip_seg,
                        &unit_ring,
                        ring_center,
                        *basis_x,
                        *basis_y,
                        ring_r,
                        stroke,
                    );
                    if let Some(p_apex) = project_pt(*apex) {
                        for (du, dv) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                            let edge = [
                                ring_center[0] + (basis_x[0] * du + basis_y[0] * dv) * ring_r,
                                ring_center[1] + (basis_x[1] * du + basis_y[1] * dv) * ring_r,
                                ring_center[2] + (basis_x[2] * du + basis_y[2] * dv) * ring_r,
                            ];
                            if let Some(pe) = project_pt(edge) {
                                painter.line_segment([p_apex, pe], stroke);
                            }
                        }
                    }
                }
                GizmoCommand::WireFrustum {
                    origin,
                    basis,
                    near,
                    far,
                    near_half,
                    far_half,
                    color,
                    width: w,
                } => {
                    let forward = [-basis[2][0], -basis[2][1], -basis[2][2]];
                    let corner = |dist: f32, hw: f32, hh: f32, du: f32, dv: f32| {
                        project_pt([
                            origin[0]
                                + forward[0] * dist
                                + basis[0][0] * hw * du
                                + basis[1][0] * hh * dv,
                            origin[1]
                                + forward[1] * dist
                                + basis[0][1] * hw * du
                                + basis[1][1] * hh * dv,
                            origin[2]
                                + forward[2] * dist
                                + basis[0][2] * hw * du
                                + basis[1][2] * hh * dv,
                        ])
                    };
                    let near_c = [
                        corner(*near, near_half[0], near_half[1], -1.0, -1.0),
                        corner(*near, near_half[0], near_half[1], 1.0, -1.0),
                        corner(*near, near_half[0], near_half[1], 1.0, 1.0),
                        corner(*near, near_half[0], near_half[1], -1.0, 1.0),
                    ];
                    let far_c = [
                        corner(*far, far_half[0], far_half[1], -1.0, -1.0),
                        corner(*far, far_half[0], far_half[1], 1.0, -1.0),
                        corner(*far, far_half[0], far_half[1], 1.0, 1.0),
                        corner(*far, far_half[0], far_half[1], -1.0, 1.0),
                    ];
                    let stroke = egui::Stroke::new(*w, to_c32(*color));
                    for ring in [&near_c, &far_c] {
                        for pair in 0..4 {
                            if let (Some(a), Some(b)) = (ring[pair], ring[(pair + 1) % 4]) {
                                painter.line_segment([a, b], stroke);
                            }
                        }
                    }
                    for pair in 0..4 {
                        if let (Some(a), Some(b)) = (near_c[pair], far_c[pair]) {
                            painter.line_segment([a, b], stroke);
                        }
                    }
                }
                GizmoCommand::ScreenBadge {
                    world_pos,
                    icon,
                    bg,
                    border,
                    fg,
                    pickable,
                    entity_index,
                } => {
                    let Some(center) = project_pt(*world_pos) else {
                        continue;
                    };
                    if !rect.expand(48.0).contains(center) {
                        continue;
                    }
                    if *pickable {
                        if let Some(idx) = entity_index {
                            hits.push((*idx, center));
                        }
                    }
                    let chip = egui::Rect::from_center_size(center, egui::vec2(24.0, 16.0));
                    painter.rect_filled(chip, 3.0_f32, to_c32(*bg));
                    painter.rect_stroke(
                        chip,
                        3.0_f32,
                        egui::Stroke::new(1.2_f32, to_c32(*border)),
                        egui::StrokeKind::Inside,
                    );
                    painter.text(
                        chip.center(),
                        egui::Align2::CENTER_CENTER,
                        icon.glyph(),
                        egui::FontId::monospace(8.5),
                        to_c32(*fg),
                    );
                }
                GizmoCommand::SolidSphere {
                    center,
                    radius,
                    fill,
                    outline,
                } => {
                    let Some(sc) = project_pt(*center) else {
                        continue;
                    };
                    let px_r = Self::project_sphere_radius(
                        frame, *center, *radius, width, height, sc, rect.min,
                    );
                    if !rect.expand(px_r + 8.0).contains(sc) {
                        continue;
                    }
                    painter.circle_filled(sc, px_r, to_c32(*fill));
                    if let Some(out) = outline {
                        painter.circle_stroke(sc, px_r, egui::Stroke::new(1.4_f32, to_c32(*out)));
                    }
                }
                GizmoCommand::ShProbeSphere {
                    center,
                    radius,
                    sh,
                    exposure,
                    outline,
                } => {
                    let Some(sc) = project_pt(*center) else {
                        continue;
                    };
                    let px_r = Self::project_sphere_radius(
                        frame, *center, *radius, width, height, sc, rect.min,
                    );
                    if !rect.expand(px_r + 8.0).contains(sc) {
                        continue;
                    }
                    Self::append_sh_sphere_mesh(&mut sh_mesh, sc, px_r, frame, sh, *exposure);
                    if let Some(out) = outline {
                        painter.circle_stroke(
                            sc,
                            px_r + 1.0,
                            egui::Stroke::new(1.5_f32, to_c32(*out)),
                        );
                    }
                }
                GizmoCommand::WorldLabel {
                    world_pos,
                    text,
                    color,
                } => {
                    if let Some(pos) = project_pt(*world_pos) {
                        if rect.expand(32.0).contains(pos) {
                            painter.text(
                                pos,
                                egui::Align2::LEFT_BOTTOM,
                                text,
                                egui::FontId::proportional(10.0),
                                to_c32(*color),
                            );
                        }
                    }
                }
            }
        }

        if !sh_mesh.is_empty() {
            painter.add(egui::Shape::mesh(sh_mesh));
        }

        hits
    }

    pub(super) fn project_sphere_radius(
        frame: &vm::CameraFrame,
        center: [f32; 3],
        world_radius: f32,
        width: f32,
        height: f32,
        screen_center: egui::Pos2,
        origin: egui::Pos2,
    ) -> f32 {
        let edge_world = [
            center[0] + frame.right[0] * world_radius,
            center[1] + frame.right[1] * world_radius,
            center[2] + frame.right[2] * world_radius,
        ];
        if let Some((ex, ey, _)) = vm::project(edge_world, frame, width, height) {
            let edge = origin + egui::vec2(ex, ey);
            return (edge - screen_center).length().clamp(4.0, 28.0);
        }
        8.0
    }

    pub(super) fn append_sh_sphere_mesh(
        mesh: &mut egui::Mesh,
        center: egui::Pos2,
        px_radius: f32,
        frame: &vm::CameraFrame,
        sh: &[[f32; 3]; 9],
        exposure: f32,
    ) {
        let eval_color = |nx: f32, ny: f32, nz: f32| -> egui::Color32 {
            let world_n = [
                frame.right[0] * nx - frame.up[0] * ny - frame.forward[0] * nz,
                frame.right[1] * nx - frame.up[1] * ny - frame.forward[1] * nz,
                frame.right[2] * nx - frame.up[2] * ny - frame.forward[2] * nz,
            ];
            let irr = PrtProbeVolume::eval_sh9_irradiance(world_n, sh);
            let rgb = PrtProbeVolume::irradiance_to_srgb8(irr, exposure);
            egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2])
        };

        const SEGS: usize = 12;
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(center, eval_color(0.0, 0.0, 1.0));

        // Inner mid-latitude ring (r = 0.58 R) + outer rim ring (r = 1.0 R).
        let r_mid = 0.58_f32;
        let z_mid = (1.0 - r_mid * r_mid).sqrt();
        for (ring_r, nz) in [(r_mid, z_mid), (1.0_f32, 0.08_f32)] {
            for i in 0..SEGS {
                let a = i as f32 * std::f32::consts::TAU / SEGS as f32;
                let (sa, ca) = a.sin_cos();
                let pos = center + egui::vec2(ca * px_radius * ring_r, sa * px_radius * ring_r);
                mesh.colored_vertex(pos, eval_color(ca * ring_r, sa * ring_r, nz));
            }
        }

        let inner_start = base + 1;
        let outer_start = inner_start + SEGS as u32;
        for i in 0..SEGS {
            let next = (i + 1) % SEGS;
            let i0 = inner_start + i as u32;
            let i1 = inner_start + next as u32;
            let o0 = outer_start + i as u32;
            let o1 = outer_start + next as u32;
            mesh.add_triangle(base, i0, i1);
            mesh.add_triangle(i0, o0, o1);
            mesh.add_triangle(i0, o1, i1);
        }
    }

    pub(super) fn draw_wire_ring_cmd(
        painter: &egui::Painter,
        clip_seg: &impl Fn([f32; 3], [f32; 3]) -> Option<(egui::Pos2, egui::Pos2)>,
        unit_ring: &[[f32; 2]],
        center: [f32; 3],
        axis_u: [f32; 3],
        axis_v: [f32; 3],
        radius: f32,
        stroke: egui::Stroke,
    ) {
        let count = unit_ring.len();
        if count < 2 {
            return;
        }
        let world_pt = |[c, s]: [f32; 2]| -> [f32; 3] {
            [
                center[0] + (axis_u[0] * c + axis_v[0] * s) * radius,
                center[1] + (axis_u[1] * c + axis_v[1] * s) * radius,
                center[2] + (axis_u[2] * c + axis_v[2] * s) * radius,
            ]
        };
        for i in 0..count {
            let p0 = world_pt(unit_ring[i]);
            let p1 = world_pt(unit_ring[(i + 1) % count]);
            if let Some((a, b)) = clip_seg(p0, p1) {
                painter.line_segment([a, b], stroke);
            }
        }
    }

    pub(super) fn draw_wire_box_cmd(
        painter: &egui::Painter,
        clip_seg: &impl Fn([f32; 3], [f32; 3]) -> Option<(egui::Pos2, egui::Pos2)>,
        center: [f32; 3],
        half: [f32; 3],
        rotation: [f32; 4],
        stroke: egui::Stroke,
    ) {
        let q =
            glam::Quat::from_xyzw(rotation[0], rotation[1], rotation[2], rotation[3]).normalize();
        let c = glam::Vec3::from_array(center);
        let mut corners = [[0.0f32; 3]; 8];
        for (i, corner) in corners.iter_mut().enumerate() {
            let sx = if i & 1 != 0 { half[0] } else { -half[0] };
            let sy = if i & 2 != 0 { half[1] } else { -half[1] };
            let sz = if i & 4 != 0 { half[2] } else { -half[2] };
            *corner = (c + q * glam::Vec3::new(sx, sy, sz)).to_array();
        }
        const EDGES: [(usize, usize); 12] = [
            (0, 1),
            (2, 3),
            (4, 5),
            (6, 7),
            (0, 2),
            (1, 3),
            (4, 6),
            (5, 7),
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7),
        ];
        for (a, b) in EDGES {
            if let Some((pa, pb)) = clip_seg(corners[a], corners[b]) {
                painter.line_segment([pa, pb], stroke);
            }
        }
    }

    pub(super) fn draw_gizmo(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        workspace: &Workspace,
        frame: &vm::CameraFrame,
    ) {
        let Some(center) = workspace.gizmo_center_world() else {
            return;
        };
        let world_length = vm::gizmo_world_length_at(&workspace.camera, center, rect.height());
        let basis = workspace.gizmo_basis();
        let origin = rect.min;
        let (width, height) = (rect.width(), rect.height());
        let ortho_2d = workspace.camera.orthographic;
        let colors = [
            egui::Color32::from_rgb(235, 82, 82),
            egui::Color32::from_rgb(108, 206, 86),
            egui::Color32::from_rgb(84, 148, 240),
        ];
        let highlight = egui::Color32::from_rgb(255, 214, 72);
        let active_handle = self.gizmo.as_ref().map(|drag| drag.handle);
        let hovered_handle = self.hovered_gizmo;

        if self.tool == GizmoTool::Rotate {
            let axes: &[usize] = if ortho_2d { &[2] } else { &[0, 1, 2] };
            for &axis in axes {
                let handle = vm::GizmoHandle::Axis(axis);
                let active = active_handle == Some(handle);
                let hovered = hovered_handle == Some(handle);
                let axis_color = if active || hovered {
                    highlight
                } else {
                    colors[axis]
                };
                let width_px = if active {
                    3.4_f32
                } else if hovered {
                    2.8_f32
                } else {
                    2.0_f32
                };
                let segs = vm::rotation_ring_segments_oriented(
                    frame,
                    center,
                    basis,
                    axis,
                    world_length,
                    width,
                    height,
                    vm::GIZMO_RING_SEGMENTS,
                );
                for (a, b) in segs {
                    painter.line_segment(
                        [origin + egui::vec2(a.0, a.1), origin + egui::vec2(b.0, b.1)],
                        egui::Stroke::new(width_px, axis_color),
                    );
                }
            }
            let outer_active = active_handle == Some(vm::GizmoHandle::ScreenRing);
            let outer_hovered = hovered_handle == Some(vm::GizmoHandle::ScreenRing);
            let outer_color = if outer_active || outer_hovered {
                highlight
            } else {
                egui::Color32::from_rgba_unmultiplied(220, 226, 236, 175)
            };
            let outer_w = if outer_active || outer_hovered {
                2.5_f32
            } else {
                1.4_f32
            };
            for (a, b) in vm::screen_ring_segments(
                frame,
                center,
                world_length,
                width,
                height,
                vm::GIZMO_RING_SEGMENTS,
            ) {
                painter.line_segment(
                    [origin + egui::vec2(a.0, a.1), origin + egui::vec2(b.0, b.1)],
                    egui::Stroke::new(outer_w, outer_color),
                );
            }
            return;
        }

        let planes: &[(usize, usize, usize)] = if ortho_2d {
            &[(0, 1, 2)]
        } else {
            &[(1, 2, 0), (0, 2, 1), (0, 1, 2)]
        };
        for &(u, v, color_axis) in planes {
            let handle = vm::GizmoHandle::Plane(u, v);
            let active = active_handle == Some(handle);
            let hovered = hovered_handle == Some(handle);
            let base = if active || hovered {
                highlight
            } else {
                colors[color_axis]
            };
            let corners = vm::plane_quad_corners(center, basis, u, v, world_length);
            let mut pts = Vec::with_capacity(4);
            for corner in corners {
                if let Some((x, y, _)) = vm::project(corner, frame, width, height) {
                    pts.push(origin + egui::vec2(x, y));
                }
            }
            if pts.len() == 4 {
                let fill_alpha = if active || hovered { 120 } else { 52 };
                let fill =
                    egui::Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), fill_alpha);
                painter.add(egui::Shape::convex_polygon(
                    pts,
                    fill,
                    egui::Stroke::new(if active || hovered { 1.6_f32 } else { 1.1_f32 }, base),
                ));
            }
        }

        let axes: &[usize] = if ortho_2d { &[0, 1] } else { &[0, 1, 2] };
        for &axis in axes {
            let handle = vm::GizmoHandle::Axis(axis);
            let active = active_handle == Some(handle);
            let hovered = hovered_handle == Some(handle);
            let axis_color = if active || hovered {
                highlight
            } else {
                colors[axis]
            };
            let width_px = if active {
                3.6_f32
            } else if hovered {
                2.8_f32
            } else {
                2.1_f32
            };
            let Some((start, end, _)) = vm::axis_screen_segment_oriented(
                frame,
                center,
                basis[axis],
                world_length,
                width,
                height,
            ) else {
                continue;
            };
            let a = origin + egui::vec2(start.0, start.1);
            let b = origin + egui::vec2(end.0, end.1);
            painter.line_segment([a, b], egui::Stroke::new(width_px, axis_color));
            if self.tool == GizmoTool::Scale {
                let half = if active || hovered { 5.2 } else { 4.2 };
                let cap = egui::Rect::from_center_size(b, egui::vec2(half * 2.0, half * 2.0));
                painter.rect_filled(cap, 1.5, axis_color);
                painter.rect_stroke(
                    cap,
                    1.5_f32,
                    egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(20, 22, 26)),
                    egui::StrokeKind::Inside,
                );
            } else {
                let dir = (b - a).normalized();
                let perp = egui::vec2(-dir.y, dir.x);
                let len = if active || hovered { 10.5 } else { 8.5 };
                let side = if active || hovered { 4.5 } else { 3.6 };
                let tip = b + dir * 2.5;
                let base_center = tip - dir * len;
                painter.add(egui::Shape::convex_polygon(
                    vec![tip, base_center + perp * side, base_center - perp * side],
                    axis_color,
                    egui::Stroke::new(0.8_f32, egui::Color32::from_rgb(20, 22, 26)),
                ));
            }
        }

        if let Some((cx, cy, _)) = vm::project(center, frame, width, height) {
            let c_active = active_handle == Some(vm::GizmoHandle::Center);
            let c_hovered = hovered_handle == Some(vm::GizmoHandle::Center);
            let center_color = if c_active || c_hovered {
                highlight
            } else {
                egui::Color32::from_rgb(235, 240, 248)
            };
            let half = if c_active || c_hovered { 5.5 } else { 4.5 };
            let c_rect = egui::Rect::from_center_size(
                origin + egui::vec2(cx, cy),
                egui::vec2(half * 2.0, half * 2.0),
            );
            painter.rect_filled(c_rect, 1.5, center_color);
            painter.rect_stroke(
                c_rect,
                1.5_f32,
                egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(20, 22, 26)),
                egui::StrokeKind::Inside,
            );
        }
    }
}
