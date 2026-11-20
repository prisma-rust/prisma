//! GASP-Godot 9 GameplayCamera rig presets and damped rig blender (issue #560 P2).
//!
//! Right-handed Y-up convention (`-Z` forward, `+X` right, `+Y` up).

use glam::Vec3;

/// The 9 GameplayCamera rig modes from GASP-Godot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum CameraRigId {
    #[default]
    Walk,
    Run,
    Sprint,
    Crouch,
    Aim,
    Strafe,
    Fall,
    Traversal,
    DebugFree,
}

impl CameraRigId {
    /// All 9 canonical rig identifiers.
    pub const ALL: [Self; 9] = [
        Self::Walk,
        Self::Run,
        Self::Sprint,
        Self::Crouch,
        Self::Aim,
        Self::Strafe,
        Self::Fall,
        Self::Traversal,
        Self::DebugFree,
    ];
}

/// Parametric third-person camera rig descriptor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraRigParams {
    pub fov_deg: f32,
    pub arm_length: f32,
    /// Offset in camera space (`+X` right, `+Y` up, `+Z` back).
    pub socket_offset: Vec3,
    /// Offset in character space around the pivot.
    pub target_offset: Vec3,
    pub position_lag_speed: f32,
    pub collision_probe_radius: f32,
}

impl CameraRigParams {
    /// Blend smoothly toward `other` by factor `alpha` in `[0, 1]`.
    #[must_use]
    pub fn lerp(self, other: Self, alpha: f32) -> Self {
        let t = alpha.clamp(0.0, 1.0);
        Self {
            fov_deg: self.fov_deg + (other.fov_deg - self.fov_deg) * t,
            arm_length: self.arm_length + (other.arm_length - self.arm_length) * t,
            socket_offset: self.socket_offset.lerp(other.socket_offset, t),
            target_offset: self.target_offset.lerp(other.target_offset, t),
            position_lag_speed: self.position_lag_speed
                + (other.position_lag_speed - self.position_lag_speed) * t,
            collision_probe_radius: self.collision_probe_radius
                + (other.collision_probe_radius - self.collision_probe_radius) * t,
        }
    }

    /// Preset parameters for each of the 9 GASP GameplayCamera rigs.
    #[must_use]
    pub const fn preset(rig: CameraRigId) -> Self {
        match rig {
            CameraRigId::Walk => Self {
                fov_deg: 65.0,
                arm_length: 3.2,
                socket_offset: Vec3::new(0.45, 0.35, 0.0),
                target_offset: Vec3::new(0.0, 1.45, 0.0),
                position_lag_speed: 12.0,
                collision_probe_radius: 0.2,
            },
            CameraRigId::Run => Self {
                fov_deg: 70.0,
                arm_length: 3.6,
                socket_offset: Vec3::new(0.45, 0.40, 0.0),
                target_offset: Vec3::new(0.0, 1.45, 0.0),
                position_lag_speed: 10.0,
                collision_probe_radius: 0.2,
            },
            CameraRigId::Sprint => Self {
                fov_deg: 78.0,
                arm_length: 4.1,
                socket_offset: Vec3::new(0.40, 0.45, 0.0),
                target_offset: Vec3::new(0.0, 1.45, 0.0),
                position_lag_speed: 8.5,
                collision_probe_radius: 0.2,
            },
            CameraRigId::Crouch => Self {
                fov_deg: 62.0,
                arm_length: 2.7,
                socket_offset: Vec3::new(0.48, 0.25, 0.0),
                target_offset: Vec3::new(0.0, 0.95, 0.0),
                position_lag_speed: 14.0,
                collision_probe_radius: 0.18,
            },
            CameraRigId::Aim => Self {
                fov_deg: 50.0,
                arm_length: 1.65,
                socket_offset: Vec3::new(0.60, 0.25, 0.0),
                target_offset: Vec3::new(0.0, 1.50, 0.0),
                position_lag_speed: 20.0,
                collision_probe_radius: 0.15,
            },
            CameraRigId::Strafe => Self {
                fov_deg: 68.0,
                arm_length: 3.3,
                socket_offset: Vec3::new(0.55, 0.38, 0.0),
                target_offset: Vec3::new(0.0, 1.45, 0.0),
                position_lag_speed: 14.0,
                collision_probe_radius: 0.2,
            },
            CameraRigId::Fall => Self {
                fov_deg: 76.0,
                arm_length: 4.3,
                socket_offset: Vec3::new(0.35, 0.50, 0.0),
                target_offset: Vec3::new(0.0, 1.40, 0.0),
                position_lag_speed: 6.5,
                collision_probe_radius: 0.25,
            },
            CameraRigId::Traversal => Self {
                fov_deg: 72.0,
                arm_length: 3.5,
                socket_offset: Vec3::new(0.50, 0.42, 0.0),
                target_offset: Vec3::new(0.0, 1.35, 0.0),
                position_lag_speed: 9.0,
                collision_probe_radius: 0.2,
            },
            CameraRigId::DebugFree => Self {
                fov_deg: 75.0,
                arm_length: 0.0,
                socket_offset: Vec3::ZERO,
                target_offset: Vec3::ZERO,
                position_lag_speed: 0.0,
                collision_probe_radius: 0.0,
            },
        }
    }
}

/// Stateful camera rig blender with obstacle arm-length clamping.
#[derive(Clone, Copy, Debug)]
pub struct CameraRigBlender {
    pub active_rig: CameraRigId,
    pub current: CameraRigParams,
    pub blend_speed: f32,
}

impl Default for CameraRigBlender {
    fn default() -> Self {
        Self {
            active_rig: CameraRigId::Walk,
            current: CameraRigParams::preset(CameraRigId::Walk),
            blend_speed: 8.0,
        }
    }
}

impl CameraRigBlender {
    /// Advance rig blending toward `target_rig` over `dt` seconds.
    pub fn step(&mut self, target_rig: CameraRigId, dt: f32) -> CameraRigParams {
        self.active_rig = target_rig;
        let target = CameraRigParams::preset(target_rig);
        let alpha = if dt <= 0.0 {
            0.0
        } else {
            1.0 - (-self.blend_speed * dt).exp()
        };
        self.current = self.current.lerp(target, alpha);
        self.current
    }

    /// Clamp arm length against an optional camera collision hit distance.
    #[must_use]
    pub fn clamped_arm_length(&self, hit_distance: Option<f32>) -> f32 {
        match hit_distance {
            Some(d) => (d - self.current.collision_probe_radius)
                .clamp(0.25, self.current.arm_length.max(0.25)),
            None => self.current.arm_length,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_nine_rigs_have_valid_presets() {
        assert_eq!(CameraRigId::ALL.len(), 9);
        for rig in CameraRigId::ALL {
            let p = CameraRigParams::preset(rig);
            assert!(p.fov_deg > 40.0 && p.fov_deg < 110.0);
            assert!(p.arm_length >= 0.0);
        }
    }

    #[test]
    fn blender_converges_and_clamps_collision() {
        let mut blender = CameraRigBlender::default();
        for _ in 0..60 {
            blender.step(CameraRigId::Aim, 1.0 / 60.0);
        }
        let aim = CameraRigParams::preset(CameraRigId::Aim);
        assert!((blender.current.fov_deg - aim.fov_deg).abs() < 0.25);
        assert!(blender.clamped_arm_length(Some(0.8)) < 0.8);
    }
}
