//! Viewport orbit camera, snapping settings, and framing helpers.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbitCamera {
    pub target: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub orthographic: bool,
    pub fov_y_degrees: f32,
    pub near: f32,
    pub far: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapSettings {
    pub enabled: bool,
    pub translate: f32,
    pub rotate_degrees: f32,
    pub scale: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: [0.0, 0.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
            distance: 12.0,
            orthographic: false,
            fov_y_degrees: 60.0,
            near: 0.1,
            far: 1000.0,
        }
    }
}

impl OrbitCamera {
    pub fn set_2d(&mut self, on: bool) {
        self.orthographic = on;
        if on {
            self.yaw = 0.0;
            self.pitch = 0.0;
        }
    }

    pub fn toggle_2d(&mut self) {
        self.set_2d(!self.orthographic);
    }

    pub fn set_fov_degrees(&mut self, degrees: f32) {
        if degrees.is_finite() {
            self.fov_y_degrees = degrees.clamp(10.0, 170.0);
        }
    }

    pub fn set_clip_planes(&mut self, near: f32, far: f32) {
        if !near.is_finite() || !far.is_finite() || near <= 0.0 {
            return;
        }
        let far = far.max(near + 0.01);
        self.near = near;
        self.far = far;
    }

    pub fn ortho_half_height(&self) -> Option<f32> {
        self.orthographic.then_some(self.distance.max(0.05))
    }

    pub fn align_to_axis(&mut self, axis: usize, sign: f32) {
        if axis > 2 {
            return;
        }
        let sign = if sign < 0.0 { -1.0 } else { 1.0 };
        let mut side = [0.0f32; 3];
        side[axis] = sign;
        self.pitch = side[1].clamp(-1.0, 1.0).asin();

        self.yaw = if side[0].abs() > 1e-6 || side[2].abs() > 1e-6 {
            side[0].atan2(side[2])
        } else {
            0.0
        };
    }

    pub fn eye(&self) -> [f32; 3] {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        [
            self.target[0] + self.distance * cos_pitch * sin_yaw,
            self.target[1] + self.distance * sin_pitch,
            self.target[2] + self.distance * cos_pitch * cos_yaw,
        ]
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        if self.orthographic {
            return;
        }
        self.yaw -= dx * 0.01;
        self.pitch = (self.pitch + dy * 0.01).clamp(-1.5, 1.5);
    }

    pub fn look_in_place(&mut self, dx: f32, dy: f32) {
        let eye = self.eye();
        self.orbit(dx, dy);
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        self.target = [
            eye[0] - self.distance * cos_pitch * sin_yaw,
            eye[1] - self.distance * sin_pitch,
            eye[2] - self.distance * cos_pitch * cos_yaw,
        ];
    }

    /// Move the eye along its horizontal view basis and world vertical axis.
    pub fn fly_move(&mut self, forward: f32, right: f32, vertical: f32, dt: f32) {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        let speed = (self.distance * 0.5).clamp(2.0, 12.0) * dt.max(0.0);
        let forward_axis = [-cos_pitch * sin_yaw, -sin_pitch, -cos_pitch * cos_yaw];
        let right_axis = [cos_yaw, 0.0, -sin_yaw];
        let delta = [
            (forward_axis[0] * forward + right_axis[0] * right) * speed,
            (forward_axis[1] * forward + vertical) * speed,
            (forward_axis[2] * forward + right_axis[2] * right) * speed,
        ];
        self.target = [
            self.target[0] + delta[0],
            self.target[1] + delta[1],
            self.target[2] + delta[2],
        ];
    }

    pub fn zoom(&mut self, delta: f32) {
        self.distance = (self.distance * (1.0 - delta * 0.001)).clamp(0.2, 500.0);
    }

    pub fn zoom_towards(&mut self, delta: f32, anchor: Option<[f32; 3]>) {
        let before = self.distance;
        self.zoom(delta);
        let Some(anchor) = anchor.filter(|anchor| anchor.iter().all(|value| value.is_finite()))
        else {
            return;
        };
        if !before.is_finite() || before <= 0.0 {
            return;
        }
        let percentage = 1.0 - self.distance / before;
        if !percentage.is_finite() {
            return;
        }
        for (target, anchor) in self.target.iter_mut().zip(anchor) {
            *target += (anchor - *target) * percentage;
        }
    }

    pub fn focus(&mut self, point: [f32; 3], radius: f32) {
        self.target = point;
        if self.orthographic {
            self.distance = (radius * 1.2).max(0.5);
            return;
        }

        let half_fov = (self.fov_y_degrees.clamp(10.0, 170.0) * 0.5).to_radians();
        self.distance = (radius / half_fov.sin() * 1.2).max(0.5);
    }
}

impl Default for SnapSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            translate: 0.25,
            rotate_degrees: 15.0,
            scale: 1.0,
        }
    }
}

impl SnapSettings {
    fn snapped(value: f32, step: f32) -> f32 {
        if step <= 0.0 {
            value
        } else {
            (value / step).round() * step
        }
    }

    pub fn snapped_translate(&self, value: f32) -> f32 {
        Self::snapped(value, self.translate)
    }

    pub fn snapped_radians(&self, radians: f32) -> f32 {
        let degrees = radians.to_degrees();
        Self::snapped(degrees, self.rotate_degrees).to_radians()
    }

    pub fn snapped_scale(&self, value: f32) -> f32 {
        Self::snapped(value, self.scale).max(0.01)
    }
}

impl Workspace {
    pub fn frame_all(&mut self) {
        let indices: Vec<usize> = (0..self.document().doc.entities.len()).collect();
        let bounds = world_bounds_with(&self.document().doc, &indices, |index| {
            self.local_bounds(index)
        });
        if let Some((center, radius)) = bounds {
            self.camera.focus(center, radius);
        }
    }

    pub fn frame_all_with(&mut self, bounds_for: impl Fn(usize) -> Option<PickBounds>) {
        let indices: Vec<usize> = (0..self.document().doc.entities.len()).collect();
        let bounds = world_bounds_with(&self.document().doc, &indices, bounds_for);
        if let Some((center, radius)) = bounds {
            self.camera.focus(center, radius);
        }
    }

    pub fn frame_selection(&mut self) {
        let Some((center, radius)) = self.selection_world_bounds() else {
            return;
        };
        self.camera.focus(center, radius);
    }

    pub fn frame_selection_with(&mut self, bounds_for: impl Fn(usize) -> Option<PickBounds>) {
        let selection = self.selection.clone();
        let Some((center, radius)) =
            world_bounds_with(&self.document().doc, &selection, bounds_for)
        else {
            return;
        };
        self.camera.focus(center, radius);
    }

    pub fn selection_world_bounds(&self) -> Option<([f32; 3], f32)> {
        world_bounds_with(&self.document().doc, &self.selection, |index| {
            self.local_bounds(index)
        })
    }

    pub(super) fn local_bounds(&self, index: usize) -> Option<PickBounds> {
        self.object_bounds
            .get(index)
            .copied()
            .flatten()
            .or_else(|| authored_local_bounds(&self.document().doc, index))
    }

    pub fn set_object_bounds(&mut self, bounds: Vec<Option<PickBounds>>) {
        self.object_bounds = bounds;
    }

    pub fn selection_world_bounds_with(
        &self,
        bounds_for: impl Fn(usize) -> Option<PickBounds>,
    ) -> Option<([f32; 3], f32)> {
        world_bounds_with(&self.document().doc, &self.selection, bounds_for)
    }

    pub fn align_view_to_selection(&mut self) -> bool {
        let Some(center) = self.selection_center_world() else {
            return false;
        };
        self.camera.target = center;
        true
    }

    pub fn align_selection_to_view(&mut self) -> bool {
        let Some(center) = self.selection_center_world() else {
            return false;
        };
        let delta = [
            self.camera.target[0] - center[0],
            self.camera.target[1] - center[1],
            self.camera.target[2] - center[2],
        ];
        let ids: Vec<u32> = self
            .selection
            .iter()
            .filter_map(|index| self.document().doc.entities.get(*index)?.id)
            .collect();
        self.translate_entities_world(&ids, delta);
        true
    }

    pub fn look_along_axis(&mut self, axis: usize, sign: f32) {
        let direction = match axis {
            0 => [sign, 0.0, 0.0],
            1 => [0.0, sign, 0.0],
            _ => [0.0, 0.0, sign],
        };
        self.camera.yaw = direction[0].atan2(direction[2]);
        self.camera.pitch = direction[1].asin().clamp(-1.55, 1.55);
    }
}
