//! Gizmo and Inspector transform mutations with parent-matrix conversion.

use super::*;

pub(super) fn world_delta_to_parent_local(
    doc: &SceneDoc,
    index: usize,
    delta_world: [f32; 3],
) -> [f32; 3] {
    let Some(parent) = doc.entities[index]
        .parent
        .and_then(|parent_id| doc.index_of_id(parent_id))
    else {
        return delta_world;
    };
    let parent_inv = world_matrix_invertible(doc, parent).inverse();
    parent_inv
        .transform_vector3(glam::Vec3::from(delta_world))
        .to_array()
}

pub(super) fn world_position_to_local(
    doc: &SceneDoc,
    index: usize,
    target_world: [f32; 3],
) -> [f32; 3] {
    let Some(parent) = doc.entities[index]
        .parent
        .and_then(|parent_id| doc.index_of_id(parent_id))
    else {
        return target_world;
    };
    let parent_inv = world_matrix_invertible(doc, parent).inverse();
    parent_inv
        .transform_point3(glam::Vec3::from(target_world))
        .to_array()
}

pub(super) fn normalize_quat(quat: [f32; 4]) -> Option<[f32; 4]> {
    let length =
        (quat[0] * quat[0] + quat[1] * quat[1] + quat[2] * quat[2] + quat[3] * quat[3]).sqrt();
    if !length.is_finite() || length < 1e-6 {
        return None;
    }
    Some([
        quat[0] / length,
        quat[1] / length,
        quat[2] / length,
        quat[3] / length,
    ])
}

impl Workspace {
    pub fn rotate_entity(&mut self, entity_id: u32, axis: usize, delta_radians: f32) {
        if !delta_radians.is_finite() || delta_radians.abs() < 1e-6 {
            return;
        }
        let Some(index) = self.document().doc.index_of_id(entity_id) else {
            return;
        };
        let before = self.document().doc.entities[index].transform;

        let after_rotation = match self.gizmo_space {
            GizmoSpace::World => {
                let delta_world = quat_axis_angle(axis, delta_radians);
                let parent_rotation = parent_world_rotation(&self.document().doc, index);
                let parent_delta = quat_mul(
                    quat_mul(quat_conjugate(parent_rotation), delta_world),
                    parent_rotation,
                );
                quat_mul(parent_delta, before.rotation)
            }
            GizmoSpace::Local => quat_mul(before.rotation, quat_axis_angle(axis, delta_radians)),
        };
        let after = TransformJson {
            rotation: quat_normalize(after_rotation),
            ..before
        };
        let now = now_ms();
        let document = self.document_mut();
        document.apply_transform_step(entity_id, before, after, now);
    }

    pub fn rotate_entities(&mut self, entity_ids: &[u32], axis: usize, delta_radians: f32) {
        let space = self.gizmo_space;
        let axis_world = match space {
            GizmoSpace::World => {
                let mut v = [0.0; 3];
                v[axis.min(2)] = 1.0;
                v
            }
            GizmoSpace::Local => self.gizmo_basis()[axis.min(2)],
        };
        let local_axis = (space == GizmoSpace::Local).then_some(axis.min(2));
        self.rotate_entities_impl(entity_ids, axis_world, delta_radians, local_axis);
    }

    /// Rotate selected entities around an arbitrary world-space axis (used by screen-space ring).
    pub fn rotate_entities_axis_dir(
        &mut self,
        entity_ids: &[u32],
        axis_world: [f32; 3],
        delta_radians: f32,
    ) {
        self.rotate_entities_impl(entity_ids, axis_world, delta_radians, None);
    }

    fn rotate_entities_impl(
        &mut self,
        entity_ids: &[u32],
        axis_world: [f32; 3],
        delta_radians: f32,
        local_axis: Option<usize>,
    ) {
        if entity_ids.is_empty() || !delta_radians.is_finite() || delta_radians.abs() < 1e-6 {
            return;
        }
        let axis_vec = glam::Vec3::from(axis_world).normalize_or_zero();
        if axis_vec.length_squared() < 1e-6 {
            return;
        }
        let mut ids: Vec<u32> = entity_ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        let ids = Self::prune_selected_descendants(&self.document().doc, &ids);
        let doc = &self.document().doc;
        let center = if self.pivot_mode == PivotMode::Center && ids.len() > 1 {
            let mut center = [0.0; 3];
            let mut count = 0.0;
            for entity_id in &ids {
                if let Some(index) = doc.index_of_id(*entity_id) {
                    let position = world_position(doc, index);
                    for a in 0..3 {
                        center[a] += position[a];
                    }
                    count += 1.0;
                }
            }
            (count > 0.0).then(|| center.map(|value| value / count))
        } else {
            None
        };
        let delta_q = glam::Quat::from_axis_angle(axis_vec, delta_radians);
        let delta_world = [delta_q.x, delta_q.y, delta_q.z, delta_q.w];
        let mut changes = Vec::new();
        for entity_id in &ids {
            let Some(index) = doc.index_of_id(*entity_id) else {
                continue;
            };
            let before = doc.entities[index].transform;
            let after_rotation = match local_axis {
                Some(ax) if center.is_none() => {
                    quat_mul(before.rotation, quat_axis_angle(ax, delta_radians))
                }
                _ => {
                    let parent_rotation = parent_world_rotation(doc, index);
                    let parent_delta = quat_mul(
                        quat_mul(quat_conjugate(parent_rotation), delta_world),
                        parent_rotation,
                    );
                    quat_mul(parent_delta, before.rotation)
                }
            };
            let after_position = center.map_or(before.position, |center| {
                let world = world_position(doc, index);
                let offset = [
                    world[0] - center[0],
                    world[1] - center[1],
                    world[2] - center[2],
                ];
                let rotated = quat_rotate(delta_world, offset);
                let target_world = [
                    center[0] + rotated[0],
                    center[1] + rotated[1],
                    center[2] + rotated[2],
                ];
                world_position_to_local(doc, index, target_world)
            });
            changes.push((
                *entity_id,
                before,
                TransformJson {
                    position: after_position,
                    rotation: quat_normalize(after_rotation),
                    ..before
                },
            ));
        }
        if changes.is_empty() {
            return;
        }
        let now = now_ms();
        let document = self.document_mut();
        document.apply_transform_batch(changes, format!("rotate-group/{ids:?}"), now);
    }

    pub fn gizmo_basis(&self) -> [[f32; 3]; 3] {
        match self.gizmo_space {
            GizmoSpace::World => crate::viewport_math::IDENTITY_BASIS,
            GizmoSpace::Local => self
                .primary_selection()
                .map(|index| basis_from_quat(world_rotation(&self.document().doc, index)))
                .unwrap_or(crate::viewport_math::IDENTITY_BASIS),
        }
    }

    pub fn gizmo_scale_axis(&self, entity_id: u32, gizmo_axis: usize) -> usize {
        let Some(index) = self.document().doc.index_of_id(entity_id) else {
            return gizmo_axis.min(2);
        };
        if self.gizmo_space == GizmoSpace::Local {
            return gizmo_axis.min(2);
        }
        let basis = basis_from_quat(world_rotation(&self.document().doc, index));
        (0..3)
            .max_by(|&left, &right| {
                basis[left][gizmo_axis.min(2)]
                    .abs()
                    .total_cmp(&basis[right][gizmo_axis.min(2)].abs())
            })
            .unwrap_or(gizmo_axis.min(2))
    }

    pub fn toggle_gizmo_space(&mut self) {
        self.gizmo_space = self.gizmo_space.toggled();
        self.log(
            LogSource::Editor,
            LogLevel::Debug,
            format!("gizmo coordinate space: {}", self.gizmo_space.label()),
        );
    }

    pub fn toggle_grid(&mut self) {
        self.show_grid = !self.show_grid;
    }

    pub fn toggle_skybox(&mut self) {
        self.show_skybox = !self.show_skybox;
    }

    pub fn translate_entity_world(&mut self, entity_id: u32, delta_world: [f32; 3]) {
        if delta_world.iter().all(|value| value.abs() < 1e-6) {
            return;
        }
        let Some(index) = self.document().doc.index_of_id(entity_id) else {
            return;
        };
        let delta_local = world_delta_to_parent_local(&self.document().doc, index, delta_world);
        let before = self.document().doc.entities[index].transform;
        let mut after = before;
        for (position, delta) in after.position.iter_mut().zip(delta_local) {
            *position += delta;
        }
        let now = now_ms();
        let document = self.document_mut();
        document.apply_transform_step(entity_id, before, after, now);
    }

    pub fn translate_entities_world(&mut self, entity_ids: &[u32], delta_world: [f32; 3]) {
        if entity_ids.is_empty() || delta_world.iter().all(|value| value.abs() < 1e-6) {
            return;
        }
        let mut ids: Vec<u32> = entity_ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        let ids = Self::prune_selected_descendants(&self.document().doc, &ids);
        let doc = &self.document().doc;
        let mut changes = Vec::new();
        for entity_id in &ids {
            let Some(index) = doc.index_of_id(*entity_id) else {
                continue;
            };
            let delta_local = world_delta_to_parent_local(doc, index, delta_world);
            let before = doc.entities[index].transform;
            let mut after = before;
            for (position, delta) in after.position.iter_mut().zip(delta_local) {
                *position += delta;
            }
            changes.push((*entity_id, before, after));
        }
        if changes.is_empty() {
            return;
        }
        let now = now_ms();
        let document = self.document_mut();
        document.apply_transform_batch(changes, format!("transform-group/{ids:?}"), now);
    }

    pub fn set_scale_axis(&mut self, entity_id: u32, axis: usize, value: f32) {
        let Some(index) = self.document().doc.index_of_id(entity_id) else {
            return;
        };
        let before = self.document().doc.entities[index].transform;
        let mut after = before;
        after.scale[axis] = value.clamp(0.01, 1000.0);
        if before == after {
            return;
        }
        let now = now_ms();
        let document = self.document_mut();
        document.apply_transform_step(entity_id, before, after, now);
    }

    pub fn scale_entities_axis(&mut self, targets: &[(u32, f32)], axis: usize, ratio: f32) {
        if targets.is_empty() || !ratio.is_finite() || ratio <= 0.0 {
            return;
        }
        let mut sorted: Vec<(u32, f32)> = targets.to_vec();
        sorted.sort_by_key(|(entity_id, _)| *entity_id);
        sorted.dedup_by_key(|(entity_id, _)| *entity_id);
        let keep: Vec<u32> = sorted.iter().map(|(entity_id, _)| *entity_id).collect();
        let keep = Self::prune_selected_descendants(&self.document().doc, &keep);
        sorted.retain(|(entity_id, _)| keep.contains(entity_id));
        let doc = &self.document().doc;
        let mut changes = Vec::new();
        for (entity_id, start_value) in &sorted {
            let Some(index) = doc.index_of_id(*entity_id) else {
                continue;
            };
            let before = doc.entities[index].transform;
            let mut after = before;
            after.scale[axis] = (*start_value * ratio).clamp(0.01, 1000.0);
            if before != after {
                changes.push((*entity_id, before, after));
            }
        }
        if changes.is_empty() {
            return;
        }
        let ids: Vec<u32> = sorted.iter().map(|(entity_id, _)| *entity_id).collect();
        let now = now_ms();
        let document = self.document_mut();
        document.apply_transform_batch(changes, format!("scale-group/{ids:?}"), now);
    }

    pub fn scale_entities_gizmo(
        &mut self,
        targets: &[(u32, usize, f32)],
        gizmo_axis: usize,
        ratio: f32,
    ) {
        if targets.is_empty() || !ratio.is_finite() || ratio <= 0.0 {
            return;
        }
        let mut sorted: Vec<(u32, usize, f32)> = targets.to_vec();
        sorted.sort_by_key(|(entity_id, _, _)| *entity_id);
        sorted.dedup_by_key(|(entity_id, _, _)| *entity_id);
        let keep: Vec<u32> = sorted.iter().map(|(entity_id, _, _)| *entity_id).collect();
        let keep = Self::prune_selected_descendants(&self.document().doc, &keep);
        sorted.retain(|(entity_id, _, _)| keep.contains(entity_id));
        let doc = &self.document().doc;
        let center = if self.pivot_mode == PivotMode::Center
            && self.gizmo_space == GizmoSpace::World
            && sorted.len() > 1
        {
            let mut center = [0.0; 3];
            let mut count = 0.0;
            for (entity_id, _, _) in &sorted {
                if let Some(index) = doc.index_of_id(*entity_id) {
                    let position = world_position(doc, index);
                    for axis in 0..3 {
                        center[axis] += position[axis];
                    }
                    count += 1.0;
                }
            }
            (count > 0.0).then(|| center.map(|value| value / count))
        } else {
            None
        };
        let mut axis_unit = [0.0f32; 3];
        if gizmo_axis < 3 {
            axis_unit[gizmo_axis] = 1.0;
        }
        let mut changes = Vec::new();
        for (entity_id, local_axis, start) in &sorted {
            let Some(index) = doc.index_of_id(*entity_id) else {
                continue;
            };
            let before = doc.entities[index].transform;
            let mut after = before;
            after.scale[*local_axis] = (*start * ratio).clamp(0.01, 1000.0);
            if let Some(center) = center {
                let world = world_position(doc, index);
                let offset = [
                    world[0] - center[0],
                    world[1] - center[1],
                    world[2] - center[2],
                ];
                let along = (offset[0] * axis_unit[0]
                    + offset[1] * axis_unit[1]
                    + offset[2] * axis_unit[2])
                    * (ratio - 1.0);
                let target_world = [
                    world[0] + axis_unit[0] * along,
                    world[1] + axis_unit[1] * along,
                    world[2] + axis_unit[2] * along,
                ];
                after.position = world_position_to_local(doc, index, target_world);
            }
            if before != after {
                changes.push((*entity_id, before, after));
            }
        }
        if changes.is_empty() {
            return;
        }
        let ids: Vec<u32> = sorted.iter().map(|(entity_id, _, _)| *entity_id).collect();
        let now = now_ms();
        let document = self.document_mut();
        document.apply_transform_batch(changes, format!("scale-group/{ids:?}"), now);
    }

    pub fn set_transform_axis(&mut self, entity_id: u32, field: &str, axis: usize, value: f32) {
        let Some(index) = self.document().doc.index_of_id(entity_id) else {
            return;
        };
        let before = self.document().doc.entities[index].transform;
        let mut after = before;
        match field {
            "position" => after.position[axis] = value,
            "rotation" => after.rotation[axis] = value,
            "scale" => after.scale[axis] = value,
            _ => return,
        }
        let now_ms = now_ms();
        let document = self.document_mut();
        document.apply_transform_step(entity_id, before, after, now_ms);
    }

    pub fn set_transform_vector(&mut self, entity_id: u32, field: &str, values: [f32; 3]) {
        let Some(index) = self.document().doc.index_of_id(entity_id) else {
            return;
        };
        let before = self.document().doc.entities[index].transform;
        let mut after = before;
        match field {
            "position" => after.position = values,
            "scale" => after.scale = values,
            _ => return,
        }
        if before == after {
            return;
        }
        let now_ms = now_ms();
        let document = self.document_mut();
        document.apply_transform_step(entity_id, before, after, now_ms);
    }

    pub fn set_transform_vector_many(&mut self, entity_ids: &[u32], field: &str, values: [f32; 3]) {
        if !matches!(field, "position" | "scale") {
            return;
        }
        let mut ids: Vec<u32> = entity_ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        let doc = &self.document().doc;
        let mut changes = Vec::new();
        for entity_id in &ids {
            let Some(index) = doc.index_of_id(*entity_id) else {
                continue;
            };
            let before = doc.entities[index].transform;
            let mut after = before;
            if field == "position" {
                after.position = values;
            } else {
                after.scale = values;
            }
            if before != after {
                changes.push((*entity_id, before, after));
            }
        }
        if changes.is_empty() {
            return;
        }
        let now = now_ms();
        let document = self.document_mut();
        document.apply_transform_batch(changes, format!("transform-vector/{field}/{ids:?}"), now);
    }

    pub fn set_transform_axis_many(
        &mut self,
        entity_ids: &[u32],
        field: &str,
        axis: usize,
        value: f32,
    ) {
        let mut ids: Vec<u32> = entity_ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        let doc = &self.document().doc;
        let mut changes = Vec::new();
        for entity_id in &ids {
            let Some(index) = doc.index_of_id(*entity_id) else {
                continue;
            };
            let before = doc.entities[index].transform;
            let mut after = before;
            match field {
                "position" => after.position[axis] = value,
                "rotation" => after.rotation[axis] = value,
                "scale" => after.scale[axis] = value,
                _ => return,
            }
            if before != after {
                changes.push((*entity_id, before, after));
            }
        }
        if changes.is_empty() {
            return;
        }
        let now = now_ms();
        let document = self.document_mut();
        document.apply_transform_batch(
            changes,
            format!("transform-axis/{field}/{axis}/{ids:?}"),
            now,
        );
    }

    pub fn set_transform_rotation(&mut self, entity_id: u32, rotation: [f32; 4]) {
        let Some(rotation) = normalize_quat(rotation) else {
            return;
        };
        let Some(index) = self.document().doc.index_of_id(entity_id) else {
            return;
        };
        let before = self.document().doc.entities[index].transform;
        let mut after = before;
        after.rotation = rotation;
        if before == after {
            return;
        }
        let now_ms = now_ms();
        let document = self.document_mut();
        document.apply_transform_step(entity_id, before, after, now_ms);
    }

    pub fn set_transform_rotation_many(&mut self, entity_ids: &[u32], rotation: [f32; 4]) {
        let Some(rotation) = normalize_quat(rotation) else {
            return;
        };
        let mut ids: Vec<u32> = entity_ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        let doc = &self.document().doc;
        let mut changes = Vec::new();
        for entity_id in &ids {
            let Some(index) = doc.index_of_id(*entity_id) else {
                continue;
            };
            let before = doc.entities[index].transform;
            let mut after = before;
            after.rotation = rotation;
            if before != after {
                changes.push((*entity_id, before, after));
            }
        }
        if changes.is_empty() {
            return;
        }
        let now = now_ms();
        let document = self.document_mut();
        document.apply_transform_batch(changes, format!("transform-rotation/{ids:?}"), now);
    }

    pub fn snap_selection_to_ground(&mut self) -> usize {
        let doc = self.document().doc.clone();
        let mut changes = Vec::new();
        for index in &self.selection {
            let Some(entity) = doc.entities.get(*index) else {
                continue;
            };
            let Some(entity_id) = entity.id else {
                continue;
            };
            let world = world_position(&doc, *index);
            let half_height = (world_scale(&doc, *index)[1] * 0.5).abs();
            let delta_y = half_height - world[1];
            if delta_y.abs() < 1e-4 {
                continue;
            }
            let delta_local = quat_rotate(
                quat_conjugate(parent_world_rotation(&doc, *index)),
                [0.0, delta_y, 0.0],
            );
            let before = entity.transform;
            let mut after = before;
            for (position, delta) in after.position.iter_mut().zip(delta_local) {
                *position += delta;
            }
            changes.push((entity_id, before, after));
        }
        if changes.is_empty() {
            return 0;
        }
        let ids: Vec<u32> = changes.iter().map(|(id, _, _)| *id).collect();
        let count = changes.len();
        let now = now_ms();
        let document = self.document_mut();
        document.undo.push_transform_batch(
            &mut document.doc,
            changes,
            format!("snap-ground/{ids:?}"),
            now,
        );
        document.dirty = true;
        document.mark_transform_dirty();
        self.log(
            LogSource::Editor,
            LogLevel::Info,
            format!("snapped {count} entities to ground"),
        );
        count
    }
}
