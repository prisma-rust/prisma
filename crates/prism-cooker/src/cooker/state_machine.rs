//! State machine cooked format **encoder** (#560 P1).
//!
//! Decode side in `prism_asset::formats_state_machine` (runtime only decodes, byte layout written there).
//! Cooker side does three things:
//!
//! - Parse author's `*.statemachine.json` ([`parse_state_machine_json`]);
//! - Parse **state name** in transitions into index (`u32::MAX` = any state), while rejecting transitions pointing to non-existent
//!   state transitions — at runtime names not looked up, bad index will only jam state machine;
//! - Drop non-limited time/threshold (author writes wrong, silently degrades, not entire asset failure).

use prism_asset::formats_state_machine::{
    AnimStateInfo, AnimTransitionInfo, StateMachineInfo, TransitionConditionInfo,
    CONDITION_BOOL_IS, CONDITION_FLOAT_GREATER, CONDITION_FLOAT_LESS, CONDITION_TRIGGER,
    RASM_MAGIC, RASM_STATE_LOOPED, RASM_VERSION,
};
use serde_json::Value;

/// Encode `.rasm`. Any illegal state/transition returns `None` (entire asset failure).
pub fn encode_rasm(machine: &StateMachineInfo) -> Option<Vec<u8>> {
    if machine.states.is_empty() || machine.states.len() > u16::MAX as usize {
        return None;
    }
    let state_count = machine.states.len() as u32;

    let mut out = Vec::new();
    out.extend_from_slice(RASM_MAGIC);
    out.push(RASM_VERSION);
    out.extend_from_slice(&state_count.to_le_bytes());
    out.extend_from_slice(&(machine.transitions.len() as u32).to_le_bytes());

    for state in &machine.states {
        if state.name.is_empty() || state.clip_path.is_empty() {
            return None;
        }
        write_string(&mut out, &state.name)?;
        write_string(&mut out, &state.clip_path)?;
        let speed = if state.speed.is_finite() {
            state.speed
        } else {
            1.0
        };
        out.extend_from_slice(&speed.to_le_bytes());
        let flags = if state.looped { RASM_STATE_LOOPED } else { 0 };
        out.push(flags);
    }

    for transition in &machine.transitions {
        if transition.to >= state_count {
            return None;
        }
        let from = match transition.from {
            Some(from) if from < state_count => from,
            Some(_) => return None,
            None => u32::MAX,
        };
        out.extend_from_slice(&from.to_le_bytes());
        out.extend_from_slice(&transition.to.to_le_bytes());
        let min_fraction = if transition.min_state_fraction.is_finite() {
            transition.min_state_fraction.clamp(0.0, 1.0)
        } else {
            0.0
        };
        out.extend_from_slice(&min_fraction.to_le_bytes());
        let duration = if transition.duration.is_finite() && transition.duration > 0.0 {
            transition.duration
        } else {
            0.0
        };
        out.extend_from_slice(&duration.to_le_bytes());
        out.extend_from_slice(&(transition.conditions.len() as u32).to_le_bytes());
        for condition in &transition.conditions {
            write_condition(&mut out, condition)?;
        }
    }

    Some(out)
}

/// Parse author's `*.statemachine.json`.
///
/// ```json
/// {
///   "states": [
///     { "name": "Idle", "clip": "char/idle.ranim" },
///     { "name": "Run",  "clip": "char/run.ranim", "speed": 1.2, "looped": true }
///   ],
///   "transitions": [
///     { "from": "Idle", "to": "Run", "duration": 0.2,
///       "conditions": [ { "type": "float_greater", "name": "speed", "threshold": 1.5 } ] }
///   ]
/// }
/// ```
///
/// Default value: `speed = 1.0`, `looped = true`, `from = "*"` (any state),
/// `duration = 0` (hard cut), `min_state_fraction = 0`. `from` / `to` use **state name**,
/// Also accept `"*"` / `"any"` / empty string meaning any state; wrong name is hard error (return `Err`).
///
/// Condition class: `float_greater` / `float_less` (`threshold`), `bool_is` (`value`),
/// `trigger` (no payload).
pub fn parse_state_machine_json(document: &Value) -> Result<StateMachineInfo, String> {
    let states_json = document
        .get("states")
        .and_then(Value::as_array)
        .ok_or_else(|| "state machine needs a `states` array".to_string())?;
    if states_json.is_empty() {
        return Err("state machine needs at least one state".to_string());
    }

    let mut states = Vec::with_capacity(states_json.len());
    for (index, state) in states_json.iter().enumerate() {
        let name = state
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| format!("state #{index} needs a non-empty `name`"))?;
        let clip_path = state
            .get("clip")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| format!("state '{name}' needs a non-empty `clip`"))?;
        if states
            .iter()
            .any(|known: &AnimStateInfo| known.name == name)
        {
            return Err(format!("duplicate state name '{name}'"));
        }
        states.push(AnimStateInfo {
            name: name.to_string(),
            clip_path: clip_path.to_string(),
            speed: state
                .get("speed")
                .and_then(Value::as_f64)
                .map(|speed| speed as f32)
                .unwrap_or(1.0),
            looped: state.get("looped").and_then(Value::as_bool).unwrap_or(true),
        });
    }

    let mut transitions = Vec::new();
    for (index, transition) in document
        .get("transitions")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        let to_name = transition
            .get("to")
            .and_then(Value::as_str)
            .map(str::trim)
            .ok_or_else(|| format!("transition #{index} needs a `to` state name"))?;
        let to = states
            .iter()
            .position(|state| state.name == to_name)
            .ok_or_else(|| format!("transition #{index} targets unknown state '{to_name}'"))?
            as u32;
        let from = match transition.get("from").and_then(Value::as_str) {
            None => None,
            Some(reference) => {
                let trimmed = reference.trim();
                if trimmed.is_empty() || trimmed == "*" || trimmed.eq_ignore_ascii_case("any") {
                    None
                } else {
                    let index = states
                        .iter()
                        .position(|state| state.name == trimmed)
                        .ok_or_else(|| {
                            format!("transition #{index} starts from unknown state '{trimmed}'")
                        })?;
                    Some(index as u32)
                }
            }
        };

        let mut conditions = Vec::new();
        for (condition_index, condition) in transition
            .get("conditions")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            let name = condition
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| {
                    format!("transition #{index} condition #{condition_index} needs a `name`")
                })?;
            let kind = condition
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let parsed = match kind {
                "float_greater" => TransitionConditionInfo::FloatGreater {
                    name: name.to_string(),
                    threshold: condition
                        .get("threshold")
                        .and_then(Value::as_f64)
                        .ok_or_else(|| format!("condition '{name}' needs a numeric `threshold`"))?
                        as f32,
                },
                "float_less" => TransitionConditionInfo::FloatLess {
                    name: name.to_string(),
                    threshold: condition
                        .get("threshold")
                        .and_then(Value::as_f64)
                        .ok_or_else(|| format!("condition '{name}' needs a numeric `threshold`"))?
                        as f32,
                },
                "bool_is" => TransitionConditionInfo::BoolIs {
                    name: name.to_string(),
                    expected: condition
                        .get("value")
                        .and_then(Value::as_bool)
                        .ok_or_else(|| format!("condition '{name}' needs a boolean `value`"))?,
                },
                "trigger" => TransitionConditionInfo::Trigger {
                    name: name.to_string(),
                },
                other => {
                    return Err(format!(
                        "condition '{name}' has unknown type '{other}' \
                         (expected float_greater / float_less / bool_is / trigger)"
                    ))
                }
            };
            conditions.push(parsed);
        }

        transitions.push(AnimTransitionInfo {
            from,
            to,
            min_state_fraction: transition
                .get("min_state_fraction")
                .and_then(Value::as_f64)
                .unwrap_or(0.0) as f32,
            duration: transition
                .get("duration")
                .and_then(Value::as_f64)
                .unwrap_or(0.0) as f32,
            conditions,
        });
    }

    Ok(StateMachineInfo {
        states,
        transitions,
    })
}

/// Write UTF-8 string prefixed with `u16` length.
fn write_string(out: &mut Vec<u8>, value: &str) -> Option<()> {
    let bytes = value.as_bytes();
    if bytes.len() > u16::MAX as usize {
        return None;
    }
    out.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
    out.extend_from_slice(bytes);
    Some(())
}

/// Write one transition condition (class + name + negative payload).
///
/// Return `None` when name empty or floating payload not limited: before one never matches on, after one in comparison always
/// false, both only confuse author.
fn write_condition(out: &mut Vec<u8>, condition: &TransitionConditionInfo) -> Option<()> {
    let name = condition.parameter_name();
    if name.is_empty() {
        return None;
    }
    match condition {
        TransitionConditionInfo::FloatGreater { threshold, .. }
        | TransitionConditionInfo::FloatLess { threshold, .. }
            if !threshold.is_finite() =>
        {
            return None;
        }
        _ => {}
    }
    match condition {
        TransitionConditionInfo::FloatGreater { name, threshold } => {
            out.push(CONDITION_FLOAT_GREATER);
            write_string(out, name)?;
            out.extend_from_slice(&threshold.to_le_bytes());
        }
        TransitionConditionInfo::FloatLess { name, threshold } => {
            out.push(CONDITION_FLOAT_LESS);
            write_string(out, name)?;
            out.extend_from_slice(&threshold.to_le_bytes());
        }
        TransitionConditionInfo::BoolIs { name, expected } => {
            out.push(CONDITION_BOOL_IS);
            write_string(out, name)?;
            out.push(u8::from(*expected));
        }
        TransitionConditionInfo::Trigger { name } => {
            out.push(CONDITION_TRIGGER);
            write_string(out, name)?;
        }
    }
    Some(())
}

#[cfg(test)]
#[path = "state_machine_tests.rs"]
mod tests;
