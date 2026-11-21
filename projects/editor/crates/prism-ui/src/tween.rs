//! Declarative property tweening and spring interpolation keyed by stable `NodeKey`.

use std::collections::HashMap;

use crate::style::Color;
use crate::tree::NodeKey;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TweenProperty {
    Opacity,
    OffsetX,
    OffsetY,
    Scale,
    BgColor,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ease {
    Linear,
    OutCubic,
    InOutCubic,
    Spring { stiffness: f32, damping: f32 },
}

impl Ease {
    pub fn sample(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::OutCubic => 1.0 - (1.0 - t).powi(3),
            Self::InOutCubic => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
            Self::Spring { .. } => 1.0 - (1.0 - t).powi(3),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ScalarTrack {
    start: f32,
    current: f32,
    target: f32,
    velocity: f32,
    elapsed: f32,
    duration: f32,
    ease: Ease,
}

#[derive(Clone, Copy, Debug)]
struct ColorTrack {
    start: Color,
    current: Color,
    target: Color,
    elapsed: f32,
    duration: f32,
    ease: Ease,
}

/// Retains active property animations across frame rebuilds.
#[derive(Default)]
pub struct TweenStore {
    scalars: HashMap<(NodeKey, TweenProperty), ScalarTrack>,
    colors: HashMap<(NodeKey, TweenProperty), ColorTrack>,
}

impl TweenStore {
    pub fn animate_scalar(
        &mut self,
        key: NodeKey,
        prop: TweenProperty,
        target: f32,
        duration: f32,
        ease: Ease,
    ) -> f32 {
        let id = (key, prop);
        match self.scalars.get_mut(&id) {
            Some(track) => {
                if (track.target - target).abs() > 1e-4 {
                    track.start = track.current;
                    track.target = target;
                    track.elapsed = 0.0;
                    track.duration = duration.max(1e-4);
                    track.ease = ease;
                }
                track.current
            }
            None => {
                self.scalars.insert(
                    id,
                    ScalarTrack {
                        start: target,
                        current: target,
                        target,
                        velocity: 0.0,
                        elapsed: duration,
                        duration: duration.max(1e-4),
                        ease,
                    },
                );
                target
            }
        }
    }

    pub fn animate_color(
        &mut self,
        key: NodeKey,
        prop: TweenProperty,
        target: Color,
        duration: f32,
        ease: Ease,
    ) -> Color {
        let id = (key, prop);
        match self.colors.get_mut(&id) {
            Some(track) => {
                if track.target != target {
                    track.start = track.current;
                    track.target = target;
                    track.elapsed = 0.0;
                    track.duration = duration.max(1e-4);
                    track.ease = ease;
                }
                track.current
            }
            None => {
                self.colors.insert(
                    id,
                    ColorTrack {
                        start: target,
                        current: target,
                        target,
                        elapsed: duration,
                        duration: duration.max(1e-4),
                        ease,
                    },
                );
                target
            }
        }
    }

    pub fn step(&mut self, dt: f32) -> bool {
        let mut active = false;
        for track in self.scalars.values_mut() {
            if let Ease::Spring { stiffness, damping } = track.ease {
                let force = (track.target - track.current) * stiffness;
                track.velocity = (track.velocity + force * dt) * (-damping * dt).exp();
                track.current += track.velocity * dt;
                if (track.target - track.current).abs() > 1e-3 || track.velocity.abs() > 1e-3 {
                    active = true;
                } else {
                    track.current = track.target;
                    track.velocity = 0.0;
                }
            } else if track.elapsed < track.duration {
                track.elapsed = (track.elapsed + dt).min(track.duration);
                let t = track.ease.sample(track.elapsed / track.duration);
                track.current = track.start + (track.target - track.start) * t;
                active = true;
            }
        }
        for track in self.colors.values_mut() {
            if track.elapsed < track.duration {
                track.elapsed = (track.elapsed + dt).min(track.duration);
                let t = track.ease.sample(track.elapsed / track.duration);
                track.current = track.start.lerp(track.target, t);
                active = true;
            }
        }
        active
    }
}
