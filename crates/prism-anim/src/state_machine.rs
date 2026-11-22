//! Data-driven state machine core (#560 P1).
//!
//! Only does **evaluation**: states reference clip paths, transitions carry "parameter condition +
//! earliest interruptible fraction + transition duration". Actual playback (sampling, cross-fade,
//! playhead) lives in `prism-engine`'s `animator`; this layer does not touch ECS / rendering, so
//! it can be tested standalone.
//!
//! Design conventions:
//!
//! - **Transitions are evaluated in author order; the first "all conditions true + earliest
//!   interruptible fraction reached" wins**; only one transition per frame (UE semantics), avoiding
//!   double state jumps in one frame.
//! - **Triggers are consumable**: once `matches` hits, the trigger is removed from the parameter
//!   table, so "press once, jump once" won't re-enter every frame just because the condition stays
//!   true.
//! - Non-finite parameters (NaN) never participate in comparisons; the condition is considered
//!   false — bad input shouldn't make the character jump states randomly.

use std::collections::{BTreeMap, BTreeSet};

/// Animation parameter table: input to state machine transitions.
///
/// Uses `BTreeMap` / `BTreeSet` instead of HashMap: parameter names are author-written strings,
/// with a handful of entries; ordered containers give deterministic iteration order (stable debug
/// output and serialization).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnimParameters {
    floats: BTreeMap<String, f32>,
    bools: BTreeMap<String, bool>,
    triggers: BTreeSet<String>,
}

impl AnimParameters {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_float(&mut self, name: impl Into<String>, value: f32) {
        self.floats.insert(name.into(), value);
    }

    pub fn float(&self, name: &str) -> Option<f32> {
        self.floats.get(name).copied()
    }

    pub fn set_bool(&mut self, name: impl Into<String>, value: bool) {
        self.bools.insert(name.into(), value);
    }

    pub fn bool(&self, name: &str) -> Option<bool> {
        self.bools.get(name).copied()
    }

    /// Set a trigger; the next matching transition will consume it.
    pub fn set_trigger(&mut self, name: impl Into<String>) {
        self.triggers.insert(name.into());
    }

    pub fn has_trigger(&self, name: &str) -> bool {
        self.triggers.contains(name)
    }

    /// Take a trigger; returns `false` if it was never set.
    pub fn take_trigger(&mut self, name: &str) -> bool {
        self.triggers.remove(name)
    }

    pub fn clear_triggers(&mut self) {
        self.triggers.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.floats.is_empty() && self.bools.is_empty() && self.triggers.is_empty()
    }
}

/// A transition condition.
#[derive(Debug, Clone, PartialEq)]
pub enum TransitionCondition {
    /// Float parameter "greater than" threshold.
    FloatGreater { name: String, threshold: f32 },
    /// Float parameter "less than" threshold.
    FloatLess { name: String, threshold: f32 },
    /// Bool parameter equals given value (`false` when the parameter is missing).
    BoolIs { name: String, expected: bool },
    /// Consume one trigger.
    Trigger { name: String },
}

impl TransitionCondition {
    /// Evaluate; when `Trigger` hits, it is removed from `params`.
    ///
    /// Missing / non-finite parameter → `false`: better to stay in the current state than jump
    /// around because data wasn't filled in.
    pub fn matches(&self, params: &mut AnimParameters) -> bool {
        match self {
            Self::FloatGreater { name, threshold } => match params.float(name) {
                Some(value) if value.is_finite() && threshold.is_finite() => value > *threshold,
                _ => false,
            },
            Self::FloatLess { name, threshold } => match params.float(name) {
                Some(value) if value.is_finite() && threshold.is_finite() => value < *threshold,
                _ => false,
            },
            Self::BoolIs { name, expected } => params.bool(name).unwrap_or(false) == *expected,
            Self::Trigger { name } => params.take_trigger(name),
        }
    }
}

/// A state: points to a clip, plus playback parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimState {
    pub name: String,
    /// `.ranim` path; empty = this state plays nothing (e.g. "death" expressed by other means).
    pub clip_path: String,
    pub speed: f32,
    pub looped: bool,
}

impl AnimState {
    pub fn new(name: impl Into<String>, clip_path: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            clip_path: clip_path.into(),
            speed: 1.0,
            looped: true,
        }
    }
}

/// A transition.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimTransition {
    /// Source state index; `usize::MAX` means "any state" (`from_any`).
    pub from: usize,
    pub to: usize,
    /// Conditions all true, and current state has played to this ratio (`0.0` = interruptible anytime, `1.0` = must play to end)
    /// before transition is allowed.
    pub min_state_fraction: f32,
    /// Transition duration (seconds); `0` = hard cut.
    pub duration: f32,
    pub conditions: Vec<TransitionCondition>,
}

impl AnimTransition {
    pub fn new(from: usize, to: usize) -> Self {
        Self {
            from,
            to,
            min_state_fraction: 0.0,
            duration: 0.0,
            conditions: Vec::new(),
        }
    }

    /// Transition from any state.
    pub fn from_any(to: usize) -> Self {
        Self::new(ANY_STATE, to)
    }

    pub fn with_conditions(mut self, conditions: Vec<TransitionCondition>) -> Self {
        self.conditions = conditions;
        self
    }

    pub fn with_duration(mut self, duration: f32) -> Self {
        self.duration = duration;
        self
    }

    pub fn with_min_state_fraction(mut self, fraction: f32) -> Self {
        self.min_state_fraction = fraction;
        self
    }
}

/// `AnimTransition::from` takes this value meaning "any state".
pub const ANY_STATE: usize = usize::MAX;

/// Result of a successful evaluation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransitionTarget {
    pub state: usize,
    /// Transition duration (seconds), non-finite / negative values already filtered out.
    pub duration: f32,
}

/// State machine definition.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StateMachine {
    pub states: Vec<AnimState>,
    pub transitions: Vec<AnimTransition>,
}

impl StateMachine {
    /// Converts cooked `.rasm` (`prism_asset::formats_state_machine`) into an evaluatable state machine.
    ///
    /// Cooker has already parsed state names into indices; here only three runtime-relevant things:
    /// - Drop transitions pointing to non-existent states (bad assets shouldn't fail the whole machine, just drop that one);
    /// - `speed` non-finite → 1.0, `min_state_fraction` clamped to `0..=1`;
    /// - `from` sentinel value `u32::MAX` → [`ANY_STATE`].
    pub fn from_info(info: &prism_asset::formats_state_machine::StateMachineInfo) -> Self {
        use prism_asset::formats_state_machine::TransitionConditionInfo as Info;

        let states: Vec<AnimState> = info
            .states
            .iter()
            .map(|state| AnimState {
                name: state.name.clone(),
                clip_path: state.clip_path.clone(),
                speed: if state.speed.is_finite() {
                    state.speed
                } else {
                    1.0
                },
                looped: state.looped,
            })
            .collect();
        let state_count = states.len();

        let transitions = info
            .transitions
            .iter()
            .filter(|transition| (transition.to as usize) < state_count)
            .filter(|transition| {
                transition
                    .from
                    .is_none_or(|from| (from as usize) < state_count)
            })
            .map(|transition| AnimTransition {
                from: transition
                    .from
                    .map(|from| from as usize)
                    .unwrap_or(ANY_STATE),
                to: transition.to as usize,
                min_state_fraction: if transition.min_state_fraction.is_finite() {
                    transition.min_state_fraction.clamp(0.0, 1.0)
                } else {
                    0.0
                },
                duration: if transition.duration.is_finite() && transition.duration > 0.0 {
                    transition.duration
                } else {
                    0.0
                },
                conditions: transition
                    .conditions
                    .iter()
                    .map(|condition| match condition {
                        Info::FloatGreater { name, threshold } => {
                            TransitionCondition::FloatGreater {
                                name: name.clone(),
                                threshold: *threshold,
                            }
                        }
                        Info::FloatLess { name, threshold } => TransitionCondition::FloatLess {
                            name: name.clone(),
                            threshold: *threshold,
                        },
                        Info::BoolIs { name, expected } => TransitionCondition::BoolIs {
                            name: name.clone(),
                            expected: *expected,
                        },
                        Info::Trigger { name } => {
                            TransitionCondition::Trigger { name: name.clone() }
                        }
                    })
                    .collect(),
            })
            .collect();

        Self {
            states,
            transitions,
        }
    }

    pub fn new(states: Vec<AnimState>, transitions: Vec<AnimTransition>) -> Self {
        Self {
            states,
            transitions,
        }
    }

    pub fn state_index(&self, name: &str) -> Option<usize> {
        self.states.iter().position(|state| state.name == name)
    }

    pub fn state(&self, index: usize) -> Option<&AnimState> {
        self.states.get(index)
    }

    /// Finds the next transition.
    ///
    /// `state_time` / `state_duration` are **current state** seconds played and clip duration,
    /// used to determine earliest interruption ratio (duration unknown treated as "already played", otherwise state gets stuck).
    ///
    /// Returns `None` = stay in current state. Hit trigger is consumed, un-hit ones kept as-is —
    /// so "transition evaluated first in the same frame consumed the trigger" does not affect subsequent transition judgments.
    pub fn next_transition(
        &self,
        from: usize,
        params: &mut AnimParameters,
        state_time: f32,
        state_duration: f32,
    ) -> Option<TransitionTarget> {
        if from >= self.states.len() {
            return None;
        }
        let fraction = state_fraction(state_time, state_duration);
        for transition in &self.transitions {
            if transition.from != ANY_STATE && transition.from != from {
                continue;
            }
            if transition.to >= self.states.len() || transition.to == from {
                continue;
            }
            if fraction + 1e-4 < sanitize_fraction(transition.min_state_fraction) {
                continue;
            }
            if !transition
                .conditions
                .iter()
                .all(|condition| condition.matches(params))
            {
                continue;
            }
            return Some(TransitionTarget {
                state: transition.to,
                duration: if transition.duration.is_finite() {
                    transition.duration.max(0.0)
                } else {
                    0.0
                },
            });
        }
        None
    }
}

/// Current state progress (0..=1). Duration unknown / non-finite → treated as played to end.
fn state_fraction(state_time: f32, state_duration: f32) -> f32 {
    if !state_duration.is_finite() || state_duration <= 0.0 {
        return 1.0;
    }
    if !state_time.is_finite() {
        return 0.0;
    }
    (state_time / state_duration).clamp(0.0, 1.0)
}

fn sanitize_fraction(fraction: f32) -> f32 {
    if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
#[path = "state_machine_tests.rs"]
mod tests;
