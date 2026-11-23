//! GASP-style foley audio event resolution from animation notifies (issue #560 P6).
//!
//! Translates `ClipNotify` names (e.g. `Footstep.L`, `Footstep.R`, `Land`, `Traversal.Start`)
//! plus ground surface material and character speed into gain/pitch-modulated [`FoleyEvent`]s
//! with per-action cooldown deduplication.

use std::collections::HashMap;

/// Physical ground surface kind used to select foley sound banks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum SurfaceMaterialKind {
    #[default]
    Default,
    Concrete,
    Wood,
    Metal,
    Grass,
    Water,
}

impl SurfaceMaterialKind {
    /// Parse a surface tag string (case-insensitive).
    #[must_use]
    pub fn parse(tag: &str) -> Self {
        match tag.trim().to_ascii_lowercase().as_str() {
            "concrete" | "stone" | "asphalt" => Self::Concrete,
            "wood" | "plank" => Self::Wood,
            "metal" | "steel" | "grate" => Self::Metal,
            "grass" | "dirt" | "soil" => Self::Grass,
            "water" | "puddle" => Self::Water,
            _ => Self::Default,
        }
    }
}

/// Semantic foley action mapped from animation notify names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FoleyAction {
    FootstepLeft,
    FootstepRight,
    Jump,
    Land,
    Traversal,
    Slide,
}

impl FoleyAction {
    /// Map a `ClipNotify` name to a foley action if recognized.
    #[must_use]
    pub fn from_notify_name(name: &str) -> Option<Self> {
        let lower = name.trim().to_ascii_lowercase();
        match lower.as_str() {
            "footstep.l" | "footstep_l" | "foley.foot.l" => Some(Self::FootstepLeft),
            "footstep.r" | "footstep_r" | "foley.foot.r" => Some(Self::FootstepRight),
            "jump" | "foley.jump" => Some(Self::Jump),
            "land" | "foley.land" => Some(Self::Land),
            "traversal.start" | "traversal.vault" | "foley.traversal" => Some(Self::Traversal),
            "slide" | "foley.slide" => Some(Self::Slide),
            _ => None,
        }
    }
}

/// Resolved foley trigger ready to hand to `prism-audio`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FoleyEvent {
    pub action: FoleyAction,
    pub surface: SurfaceMaterialKind,
    /// Linear gain multiplier in `[0.1, 1.5]`.
    pub gain: f32,
    /// Pitch multiplier in `[0.85, 1.15]`.
    pub pitch: f32,
}

/// Stateful foley dispatcher with per-action cooldown to prevent double triggers during blends.
#[derive(Clone, Debug)]
pub struct FoleyDispatcher {
    min_cooldown_sec: f32,
    last_trigger_sec: HashMap<FoleyAction, f32>,
}

impl Default for FoleyDispatcher {
    fn default() -> Self {
        Self::new(0.08)
    }
}

impl FoleyDispatcher {
    /// Create a dispatcher with `min_cooldown_sec` between identical actions.
    #[must_use]
    pub fn new(min_cooldown_sec: f32) -> Self {
        Self {
            min_cooldown_sec: min_cooldown_sec.max(0.0),
            last_trigger_sec: HashMap::new(),
        }
    }

    /// Resolve a notify name into a [`FoleyEvent`] if recognized and outside cooldown.
    pub fn try_dispatch(
        &mut self,
        notify_name: &str,
        surface: SurfaceMaterialKind,
        speed: f32,
        now_sec: f32,
    ) -> Option<FoleyEvent> {
        let action = FoleyAction::from_notify_name(notify_name)?;
        if let Some(&prev) = self.last_trigger_sec.get(&action) {
            if (now_sec - prev) < self.min_cooldown_sec {
                return None;
            }
        }
        self.last_trigger_sec.insert(action, now_sec);

        let speed_norm = (speed / 6.0).clamp(0.0, 1.25);
        let gain = match action {
            FoleyAction::Land | FoleyAction::Traversal => (0.65 + 0.5 * speed_norm).clamp(0.2, 1.5),
            _ => (0.35 + 0.65 * speed_norm).clamp(0.1, 1.25),
        };
        let pitch_base = match surface {
            SurfaceMaterialKind::Metal => 1.06,
            SurfaceMaterialKind::Water => 0.92,
            SurfaceMaterialKind::Grass => 0.96,
            _ => 1.0,
        };
        let pitch = (pitch_base + (speed_norm - 0.5) * 0.06).clamp(0.85, 1.15);

        Some(FoleyEvent {
            action,
            surface,
            gain,
            pitch,
        })
    }

    /// Clear cooldown state (e.g. on respawn or scene reload).
    pub fn reset(&mut self) {
        self.last_trigger_sec.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_standard_gasp_notifies_and_enforces_cooldown() {
        let mut dispatcher = FoleyDispatcher::new(0.1);
        let first = dispatcher.try_dispatch("Footstep.L", SurfaceMaterialKind::Metal, 4.5, 1.0);
        assert!(first.is_some());
        let ev = first.unwrap();
        assert_eq!(ev.action, FoleyAction::FootstepLeft);
        assert_eq!(ev.surface, SurfaceMaterialKind::Metal);
        assert!(ev.gain > 0.35);

        assert!(dispatcher
            .try_dispatch("Footstep.L", SurfaceMaterialKind::Metal, 4.5, 1.05)
            .is_none());
        assert!(dispatcher
            .try_dispatch("Footstep.R", SurfaceMaterialKind::Metal, 4.5, 1.05)
            .is_some());
        assert!(dispatcher
            .try_dispatch("Footstep.L", SurfaceMaterialKind::Metal, 4.5, 1.15)
            .is_some());
    }

    #[test]
    fn unknown_notify_returns_none() {
        let mut dispatcher = FoleyDispatcher::default();
        assert!(dispatcher
            .try_dispatch("CameraShake", SurfaceMaterialKind::Concrete, 2.0, 0.5)
            .is_none());
    }
}
