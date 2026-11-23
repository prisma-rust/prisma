//! State machine cooked format decoder (#560 P1).
//!
//! Author writes `*.statemachine.json`, cooker compiles to `.rasm` placed in `.pak`; runtime only decodes.
//! Same layering as `.rskel` / `.rskin` / `.ranim` (encode side in `prism-cooker`).
//!
//! ## RASM v1
//!
//! ```text
//! [magic:4 "RASM"][version:1][state_count:4][transition_count:4]
//! states: state_count × {
//!     name_len: u16, name: utf8
//!     clip_len: u16, clip_path: utf8
//!     speed: f32
//!     flags: u8              // bit0 = looped
//! }
//! transitions: transition_count × {
//!     from: u32              // u32::MAX = any state
//!     to:   u32
//!     min_state_fraction: f32
//!     duration: f32
//!     condition_count: u32
//!     conditions: condition_count × {
//!         kind: u8
//!         name_len: u16, name: utf8
//!         payload: depends on kind
//!             FLOAT_GREATER / FLOAT_LESS → f32 threshold
//!             BOOL_IS                    → u8 (0/1)
//!             TRIGGER                    → none
//!     }
//! }
//! ```
//!
//! State and transition reference clip by **name**, state by **index**: index resolved at cook time,
//! runtime does not need to look up name again. All counts have upper limits (`MAX_STATES` / `MAX_TRANSITIONS` /
//! `MAX_CONDITIONS`), bad file returns `None` instead of allocating a big block.

use crate::formats_skinned::Reader;

/// RASM magic.
pub const RASM_MAGIC: &[u8; 4] = b"RASM";
/// RASM version.
pub const RASM_VERSION: u8 = 1;
/// State upper limit.
pub const MAX_STATES: usize = 4096;
/// Transition upper limit.
pub const MAX_TRANSITIONS: usize = 65536;
/// Per-transition condition upper limit.
pub const MAX_CONDITIONS: usize = 256;

/// State flag: loop playback.
pub const RASM_STATE_LOOPED: u8 = 1 << 0;

/// Condition kind.
pub const CONDITION_FLOAT_GREATER: u8 = 0;
/// Condition kind: float less-than.
pub const CONDITION_FLOAT_LESS: u8 = 1;
/// Condition kind: bool equals.
pub const CONDITION_BOOL_IS: u8 = 2;
/// Condition kind: trigger (consumed on hit).
pub const CONDITION_TRIGGER: u8 = 3;

/// One decoded state.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimStateInfo {
    pub name: String,
    pub clip_path: String,
    pub speed: f32,
    pub looped: bool,
}

/// One decoded transition condition.
#[derive(Debug, Clone, PartialEq)]
pub enum TransitionConditionInfo {
    FloatGreater { name: String, threshold: f32 },
    FloatLess { name: String, threshold: f32 },
    BoolIs { name: String, expected: bool },
    Trigger { name: String },
}

impl TransitionConditionInfo {
    pub fn parameter_name(&self) -> &str {
        match self {
            Self::FloatGreater { name, .. }
            | Self::FloatLess { name, .. }
            | Self::BoolIs { name, .. }
            | Self::Trigger { name } => name,
        }
    }
}

/// One decoded transition.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimTransitionInfo {
    /// Source state index; `None` = any state.
    pub from: Option<u32>,
    pub to: u32,
    /// Source state must play to at least this ratio before transition allowed (`0` = anytime).
    pub min_state_fraction: f32,
    /// Crossfade duration (seconds); `0` = hard cut.
    pub duration: f32,
    pub conditions: Vec<TransitionConditionInfo>,
}

/// Decoded `.rasm`.
#[derive(Debug, Clone, PartialEq)]
pub struct StateMachineInfo {
    pub states: Vec<AnimStateInfo>,
    pub transitions: Vec<AnimTransitionInfo>,
}

/// Decode `.rasm`.
pub fn decode_rasm(data: &[u8]) -> Option<StateMachineInfo> {
    if data.len() < 13 || &data[..4] != RASM_MAGIC {
        return None;
    }
    let mut reader = Reader::new(data, 4);
    if reader.u8()? != RASM_VERSION {
        return None;
    }
    let state_count = reader.u32()? as usize;
    let transition_count = reader.u32()? as usize;
    if state_count > MAX_STATES || transition_count > MAX_TRANSITIONS {
        return None;
    }

    let mut states = Vec::with_capacity(state_count);
    for _ in 0..state_count {
        let name = read_string(&mut reader)?;
        let clip_path = read_string(&mut reader)?;
        let speed = reader.f32()?;
        let flags = reader.u8()?;
        if flags & !RASM_STATE_LOOPED != 0 {
            return None;
        }
        states.push(AnimStateInfo {
            name,
            clip_path,
            speed,
            looped: flags & RASM_STATE_LOOPED != 0,
        });
    }

    let mut transitions = Vec::with_capacity(transition_count);
    for _ in 0..transition_count {
        let from = reader.u32()?;
        let to = reader.u32()?;
        let min_state_fraction = reader.f32()?;
        let duration = reader.f32()?;
        let condition_count = reader.u32()? as usize;
        if condition_count > MAX_CONDITIONS {
            return None;
        }
        let mut conditions = Vec::with_capacity(condition_count);
        for _ in 0..condition_count {
            let kind = reader.u8()?;
            let name = read_string(&mut reader)?;
            let condition = match kind {
                CONDITION_FLOAT_GREATER => TransitionConditionInfo::FloatGreater {
                    name,
                    threshold: reader.f32()?,
                },
                CONDITION_FLOAT_LESS => TransitionConditionInfo::FloatLess {
                    name,
                    threshold: reader.f32()?,
                },
                CONDITION_BOOL_IS => TransitionConditionInfo::BoolIs {
                    name,
                    expected: reader.u8()? != 0,
                },
                CONDITION_TRIGGER => TransitionConditionInfo::Trigger { name },
                _ => return None,
            };
            conditions.push(condition);
        }
        transitions.push(AnimTransitionInfo {
            from: if from == u32::MAX { None } else { Some(from) },
            to,
            min_state_fraction,
            duration,
            conditions,
        });
    }

    if reader.remaining() != 0 {
        return None;
    }

    Some(StateMachineInfo {
        states,
        transitions,
    })
}

/// Read a `u16` length-prefixed UTF-8 string.
fn read_string(reader: &mut Reader<'_>) -> Option<String> {
    let len = reader.u16()? as usize;
    String::from_utf8(reader.take(len)?.to_vec()).ok()
}

#[cfg(test)]
#[path = "formats_state_machine_tests.rs"]
mod tests;
