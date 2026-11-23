use super::*;
use prism_asset::formats_state_machine::decode_rasm;

fn document() -> Value {
    serde_json::json!({
        "states": [
            { "name": "Idle", "clip": "char/idle.ranim" },
            { "name": "Walk", "clip": "char/walk.ranim", "speed": 0.8 },
            { "name": "Run", "clip": "char/run.ranim", "speed": 1.2, "looped": false }
        ],
        "transitions": [
            {
                "from": "Idle", "to": "Walk", "duration": 0.2,
                "conditions": [{ "type": "float_greater", "name": "speed", "threshold": 0.1 }]
            },
            {
                "from": "Walk", "to": "Run", "duration": 0.15, "min_state_fraction": 0.3,
                "conditions": [{ "type": "float_greater", "name": "speed", "threshold": 1.5 }]
            },
            {
                "from": "*", "to": "Idle",
                "conditions": [{ "type": "trigger", "name": "Stop" }]
            },
            {
                "from": "Run", "to": "Idle",
                "conditions": [
                    { "type": "bool_is", "name": "grounded", "value": false },
                    { "type": "float_less", "name": "speed", "threshold": 0.05 }
                ]
            }
        ]
    })
}

#[test]
fn the_author_document_becomes_a_decodable_machine() {
    let machine = parse_state_machine_json(&document()).expect("parses");
    assert_eq!(machine.states.len(), 3);
    assert_eq!(machine.states[0].clip_path, "char/idle.ranim");
    assert!((machine.states[1].speed - 0.8).abs() < 1e-6);
    assert!(machine.states[1].looped, "looped defaults to true");
    assert!(!machine.states[2].looped);
    assert_eq!(machine.transitions[1].from, Some(1));
    assert_eq!(machine.transitions[1].to, 2);
    assert!((machine.transitions[1].min_state_fraction - 0.3).abs() < 1e-6);
    // "*" → any state.
    assert_eq!(machine.transitions[2].from, None);

    // encode → decode must recover field by field (cooked out is what runtime sees).
    let bytes = encode_rasm(&machine).expect("encodes");
    let decoded = decode_rasm(&bytes).expect("decodes");
    assert_eq!(decoded, machine);
}

#[test]
fn a_machine_without_transitions_is_valid() {
    let machine = parse_state_machine_json(&serde_json::json!({
        "states": [{ "name": "Idle", "clip": "char/idle.ranim" }]
    }))
    .expect("parses");
    assert!(machine.transitions.is_empty());
    assert!(machine.states[0].looped);
    assert!((machine.states[0].speed - 1.0).abs() < 1e-6);
}

#[test]
fn broken_documents_report_what_is_wrong() {
    let cases = [
        (serde_json::json!({}), "states"),
        (serde_json::json!({ "states": [] }), "at least one state"),
        (
            serde_json::json!({ "states": [{ "name": "Idle" }] }),
            "needs a non-empty `clip`",
        ),
        (
            serde_json::json!({ "states": [{ "clip": "a.ranim" }] }),
            "needs a non-empty `name`",
        ),
        (
            serde_json::json!({
                "states": [
                    { "name": "Idle", "clip": "a.ranim" },
                    { "name": "Idle", "clip": "b.ranim" }
                ]
            }),
            "duplicate state name",
        ),
        (
            serde_json::json!({
                "states": [{ "name": "Idle", "clip": "a.ranim" }],
                "transitions": [{ "to": "Missing" }]
            }),
            "unknown state 'Missing'",
        ),
        (
            serde_json::json!({
                "states": [{ "name": "Idle", "clip": "a.ranim" }],
                "transitions": [{ "from": "Nope", "to": "Idle" }]
            }),
            "starts from unknown state 'Nope'",
        ),
        (
            serde_json::json!({
                "states": [{ "name": "Idle", "clip": "a.ranim" }],
                "transitions": [{
                    "to": "Idle",
                    "conditions": [{ "type": "teleport", "name": "x" }]
                }]
            }),
            "unknown type 'teleport'",
        ),
        (
            serde_json::json!({
                "states": [{ "name": "Idle", "clip": "a.ranim" }],
                "transitions": [{
                    "to": "Idle",
                    "conditions": [{ "type": "float_greater", "name": "speed" }]
                }]
            }),
            "numeric `threshold`",
        ),
    ];
    for (document, expected) in cases {
        let error = parse_state_machine_json(&document).expect_err("must fail");
        assert!(
            error.contains(expected),
            "expected an error mentioning {expected:?}, got {error:?}"
        );
    }
}

#[test]
fn encoding_rejects_broken_or_oversized_records() {
    // Target index out of bounds.
    let mut machine = parse_state_machine_json(&document()).expect("parses");
    machine.transitions[0].to = 99;
    assert!(encode_rasm(&machine).is_none());

    // Source state index out of bounds.
    let mut machine = parse_state_machine_json(&document()).expect("parses");
    machine.transitions[0].from = Some(99);
    assert!(encode_rasm(&machine).is_none());

    // Empty clip path.
    let mut machine = parse_state_machine_json(&document()).expect("parses");
    machine.states[0].clip_path.clear();
    assert!(encode_rasm(&machine).is_none());

    // Condition with empty name: writing it out will never match on anyway.
    let mut machine = parse_state_machine_json(&document()).expect("parses");
    machine.transitions[0].conditions = vec![TransitionConditionInfo::Trigger {
        name: String::new(),
    }];
    assert!(encode_rasm(&machine).is_none());

    // Non-limited threshold rejected as well.
    let mut machine = parse_state_machine_json(&document()).expect("parses");
    machine.transitions[0].conditions = vec![TransitionConditionInfo::FloatGreater {
        name: "speed".into(),
        threshold: f32::NAN,
    }];
    assert!(encode_rasm(&machine).is_none());

    // No state.
    let empty = StateMachineInfo {
        states: Vec::new(),
        transitions: Vec::new(),
    };
    assert!(encode_rasm(&empty).is_none());
}

#[test]
fn out_of_range_numbers_are_clamped_instead_of_written_raw() {
    let mut machine = parse_state_machine_json(&serde_json::json!({
        "states": [{ "name": "A", "clip": "a.ranim", "speed": 1.0 }],
        "transitions": [{
            "to": "A", "duration": -1.0, "min_state_fraction": 7.5
        }]
    }))
    .expect("parses");
    machine.transitions[0].min_state_fraction = f32::NAN;
    let bytes = encode_rasm(&machine).expect("encodes");
    let decoded = decode_rasm(&bytes).expect("decodes");
    // Negative duration = hard cut, ratio beyond range clamped to 1, NaN to 0.
    assert_eq!(decoded.transitions[0].duration, 0.0);
    assert_eq!(decoded.transitions[0].min_state_fraction, 0.0);

    let mut machine = parse_state_machine_json(&serde_json::json!({
        "states": [{ "name": "A", "clip": "a.ranim" }],
        "transitions": [{ "to": "A", "min_state_fraction": 7.5 }]
    }))
    .expect("parses");
    let bytes = encode_rasm(&machine).expect("encodes");
    let decoded = decode_rasm(&bytes).expect("decodes");
    assert_eq!(decoded.transitions[0].min_state_fraction, 1.0);

    // Non-limited speed falls back to 1.0, not writing NaN into asset.
    machine.states[0].speed = f32::INFINITY;
    let bytes = encode_rasm(&machine).expect("encodes");
    let decoded = decode_rasm(&bytes).expect("decodes");
    assert_eq!(decoded.states[0].speed, 1.0);
}
