//! Traversal detection, Chooser, MotionWarping targets, and chained traversal
//! state machine (GASP P4).

/// Geometry probe result for a candidate obstacle in front of the character.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TraversalProbe {
    pub obstacle_height: f32,
    pub obstacle_depth: f32,
    pub front_edge: [f32; 3],
    pub top_edge: [f32; 3],
    pub exit_edge: [f32; 3],
    pub has_clear_landing: bool,
}

/// Selected traversal action from the Chooser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraversalAction {
    None,
    Vault,
    MantleLow,
    MantleHigh,
    Climb,
}

/// Motion-warping sync target for a traversal montage window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionWarpTarget {
    pub position: [f32; 3],
    pub start_t: f32,
    pub end_t: f32,
}

/// Choose a traversal action from obstacle geometry and character speed.
pub fn choose_traversal(probe: &TraversalProbe, speed: f32) -> TraversalAction {
    if probe.obstacle_height < 0.35 || probe.obstacle_height > 2.6 {
        return TraversalAction::None;
    }
    if probe.obstacle_height <= 1.25 && probe.obstacle_depth <= 0.9 && probe.has_clear_landing && speed >= 2.0 {
        return TraversalAction::Vault;
    }
    if probe.obstacle_height <= 1.25 {
        return TraversalAction::MantleLow;
    }
    if probe.obstacle_height <= 2.0 {
        return TraversalAction::MantleHigh;
    }
    TraversalAction::Climb
}

/// Compute MotionWarping targets for the chosen traversal action.
pub fn compute_warp_targets(probe: &TraversalProbe, action: TraversalAction) -> Vec<MotionWarpTarget> {
    match action {
        TraversalAction::None => Vec::new(),
        TraversalAction::Vault => vec![
            MotionWarpTarget {
                position: probe.front_edge,
                start_t: 0.0,
                end_t: 0.35,
            },
            MotionWarpTarget {
                position: probe.exit_edge,
                start_t: 0.35,
                end_t: 0.85,
            },
        ],
        TraversalAction::MantleLow | TraversalAction::MantleHigh | TraversalAction::Climb => vec![
            MotionWarpTarget {
                position: probe.front_edge,
                start_t: 0.0,
                end_t: 0.4,
            },
            MotionWarpTarget {
                position: probe.top_edge,
                start_t: 0.4,
                end_t: 0.9,
            },
        ],
    }
}

/// Chained traversal state machine.
#[derive(Clone, Debug, PartialEq)]
pub enum TraversalState {
    Idle,
    Active {
        action: TraversalAction,
        elapsed: f32,
        duration: f32,
        targets: Vec<MotionWarpTarget>,
    },
    Cooldown(f32),
}

impl TraversalState {
    pub fn try_start(&mut self, probe: &TraversalProbe, speed: f32) -> TraversalAction {
        if !matches!(self, TraversalState::Idle) {
            return TraversalAction::None;
        }
        let action = choose_traversal(probe, speed);
        if action == TraversalAction::None {
            return TraversalAction::None;
        }
        let duration = match action {
            TraversalAction::Vault => 0.45,
            TraversalAction::MantleLow => 0.55,
            TraversalAction::MantleHigh => 0.75,
            TraversalAction::Climb => 0.95,
            TraversalAction::None => 0.0,
        };
        let targets = compute_warp_targets(probe, action);
        *self = TraversalState::Active {
            action,
            elapsed: 0.0,
            duration,
            targets,
        };
        action
    }

    pub fn step(&mut self, dt: f32) {
        match self {
            TraversalState::Idle => {}
            TraversalState::Active {
                elapsed,
                duration,
                ..
            } => {
                *elapsed += dt;
                if *elapsed >= *duration {
                    *self = TraversalState::Cooldown(0.1);
                }
            }
            TraversalState::Cooldown(rem) => {
                *rem -= dt;
                if *rem <= 0.0 {
                    *self = TraversalState::Idle;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_probe(height: f32, depth: f32, clear: bool) -> TraversalProbe {
        TraversalProbe {
            obstacle_height: height,
            obstacle_depth: depth,
            front_edge: [0.0, height, 1.0],
            top_edge: [0.0, height, 1.5],
            exit_edge: [0.0, 0.0, 2.2],
            has_clear_landing: clear,
        }
    }

    #[test]
    fn chooser_selects_expected_traversal_actions() {
        assert_eq!(choose_traversal(&sample_probe(0.2, 0.5, true), 3.0), TraversalAction::None);
        assert_eq!(choose_traversal(&sample_probe(1.0, 0.6, true), 3.0), TraversalAction::Vault);
        assert_eq!(choose_traversal(&sample_probe(1.0, 1.5, true), 3.0), TraversalAction::MantleLow);
        assert_eq!(choose_traversal(&sample_probe(1.8, 1.0, true), 1.0), TraversalAction::MantleHigh);
        assert_eq!(choose_traversal(&sample_probe(2.4, 1.0, true), 1.0), TraversalAction::Climb);
    }

    #[test]
    fn chained_traversal_state_machine_completes_and_resets() {
        let mut state = TraversalState::Idle;
        let probe = sample_probe(1.0, 0.6, true);
        assert_eq!(state.try_start(&probe, 3.0), TraversalAction::Vault);
        assert!(matches!(state, TraversalState::Active { .. }));
        state.step(0.5);
        assert!(matches!(state, TraversalState::Cooldown(_)));
        state.step(0.15);
        assert_eq!(state, TraversalState::Idle);
    }
}
