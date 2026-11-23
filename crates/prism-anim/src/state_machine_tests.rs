use super::*;

fn idle_run_machine() -> StateMachine {
    StateMachine::new(
        vec![
            AnimState::new("idle", "idle.ranim"),
            AnimState::new("walk", "walk.ranim"),
            AnimState::new("run", "run.ranim"),
        ],
        vec![
            AnimTransition::new(0, 1).with_conditions(vec![TransitionCondition::FloatGreater {
                name: "speed".into(),
                threshold: 0.1,
            }]),
            AnimTransition::new(0, 2).with_conditions(vec![TransitionCondition::FloatGreater {
                name: "speed".into(),
                threshold: 4.0,
            }]),
            AnimTransition::new(1, 2).with_conditions(vec![TransitionCondition::FloatGreater {
                name: "speed".into(),
                threshold: 4.0,
            }]),
            AnimTransition::new(2, 1).with_conditions(vec![TransitionCondition::FloatLess {
                name: "speed".into(),
                threshold: 4.0,
            }]),
        ],
    )
}

#[test]
fn the_first_matching_transition_in_author_order_wins() {
    let machine = idle_run_machine();
    let mut params = AnimParameters::new();

    // Speed 5 satisfies both "> 0.1" and "> 4.0", but idle→walk is written first, so walk is taken first.
    params.set_float("speed", 5.0);
    assert_eq!(
        machine
            .next_transition(0, &mut params, 0.0, 1.0)
            .map(|t| t.state),
        Some(1)
    );
    // From walk only walk→run satisfied.
    assert_eq!(
        machine
            .next_transition(1, &mut params, 0.0, 1.0)
            .map(|t| t.state),
        Some(2)
    );
    // Run state speed still 5: run→walk condition (< 4) not met.
    assert_eq!(machine.next_transition(2, &mut params, 0.0, 1.0), None);
}

#[test]
fn a_slow_character_does_not_leave_idle() {
    let machine = idle_run_machine();
    let mut params = AnimParameters::new();
    params.set_float("speed", 0.05);
    assert_eq!(machine.next_transition(0, &mut params, 0.0, 1.0), None);
    // Missing parameters also don't transition (data not filled in shouldn't let character move randomly).
    assert_eq!(
        machine.next_transition(0, &mut AnimParameters::new(), 0.0, 1.0),
        None
    );
    // NaN also doesn't hold.
    params.set_float("speed", f32::NAN);
    assert_eq!(machine.next_transition(0, &mut params, 0.0, 1.0), None);
}

#[test]
fn a_trigger_fires_once_and_is_then_consumed() {
    let machine = StateMachine::new(
        vec![
            AnimState::new("idle", "idle.ranim"),
            AnimState::new("jump", "jump.ranim"),
        ],
        vec![AnimTransition::new(0, 1)
            .with_conditions(vec![TransitionCondition::Trigger {
                name: "jump".into(),
            }])
            .with_duration(0.15)],
    );
    let mut params = AnimParameters::new();
    assert_eq!(machine.next_transition(0, &mut params, 0.0, 1.0), None);

    params.set_trigger("jump");
    let target = machine
        .next_transition(0, &mut params, 0.0, 1.0)
        .expect("trigger fires");
    assert_eq!(target.state, 1);
    assert!((target.duration - 0.15).abs() < 1e-6);
    assert!(!params.has_trigger("jump"), "trigger should be consumed");

    // Re-evaluation won't retrigger.
    assert_eq!(machine.next_transition(0, &mut params, 0.0, 1.0), None);
}

#[test]
fn the_minimum_state_fraction_holds_the_transition_until_the_state_has_played() {
    let machine = StateMachine::new(
        vec![
            AnimState::new("attack", "attack.ranim"),
            AnimState::new("idle", "idle.ranim"),
        ],
        vec![AnimTransition::new(0, 1)
            .with_min_state_fraction(1.0)
            .with_conditions(vec![TransitionCondition::BoolIs {
                name: "done".into(),
                expected: true,
            }])],
    );
    let mut params = AnimParameters::new();
    params.set_bool("done", true);

    // Played to 40%: condition met but not yet earliest interruption point.
    assert_eq!(machine.next_transition(0, &mut params, 0.4, 1.0), None);
    // Played to end: transition allowed.
    assert_eq!(
        machine
            .next_transition(0, &mut params, 1.0, 1.0)
            .map(|t| t.state),
        Some(1)
    );
    // Unknown duration (0 / NaN) treated as "played to end", otherwise single clip gets stuck at end.
    assert_eq!(
        machine
            .next_transition(0, &mut params, 0.0, 0.0)
            .map(|t| t.state),
        Some(1)
    );
    assert_eq!(
        machine
            .next_transition(0, &mut params, 0.0, f32::NAN)
            .map(|t| t.state),
        Some(1)
    );
}

#[test]
fn transitions_from_other_states_and_self_loops_are_ignored() {
    let machine = StateMachine::new(
        vec![
            AnimState::new("a", "a.ranim"),
            AnimState::new("b", "b.ranim"),
        ],
        vec![
            // Self-loop: never chosen.
            AnimTransition::new(0, 0),
            // Only from b: not effective on a.
            AnimTransition::new(1, 0),
            // Out-of-bounds target: skip.
            AnimTransition::new(0, 99),
        ],
    );
    assert_eq!(
        machine.next_transition(0, &mut AnimParameters::new(), 0.0, 1.0),
        None
    );
    // Out-of-bounds source state doesn't panic.
    assert_eq!(
        machine.next_transition(99, &mut AnimParameters::new(), 0.0, 1.0),
        None
    );
}

#[test]
fn an_any_state_transition_applies_from_every_state() {
    let machine = StateMachine::new(
        vec![
            AnimState::new("idle", "idle.ranim"),
            AnimState::new("walk", "walk.ranim"),
            AnimState::new("dead", "dead.ranim"),
        ],
        vec![AnimTransition::from_any(2)
            .with_conditions(vec![TransitionCondition::Trigger { name: "die".into() }])],
    );
    for from in 0..2 {
        let mut params = AnimParameters::new();
        params.set_trigger("die");
        assert_eq!(
            machine
                .next_transition(from, &mut params, 0.0, 1.0)
                .map(|t| t.state),
            Some(2),
            "from {from}"
        );
    }
}

#[test]
fn states_are_looked_up_by_name_and_bad_durations_are_sanitized() {
    let machine = idle_run_machine();
    assert_eq!(machine.state_index("walk"), Some(1));
    assert_eq!(machine.state_index("nope"), None);
    assert_eq!(
        machine.state(1).map(|state| state.clip_path.as_str()),
        Some("walk.ranim")
    );

    let machine = StateMachine::new(
        vec![
            AnimState::new("a", "a.ranim"),
            AnimState::new("b", "b.ranim"),
        ],
        vec![
            AnimTransition::new(0, 1).with_duration(f32::NAN),
            AnimTransition::new(0, 1).with_duration(-3.0),
        ],
    );
    let target = machine
        .next_transition(0, &mut AnimParameters::new(), 0.0, 1.0)
        .expect("unconditional transition");
    assert_eq!(target.duration, 0.0, "bad duration handled as hard cut");
}

#[test]
fn parameters_round_trip_and_clear() {
    let mut params = AnimParameters::new();
    assert!(params.is_empty());
    params.set_float("speed", 2.5);
    params.set_bool("grounded", true);
    params.set_trigger("attack");
    assert_eq!(params.float("speed"), Some(2.5));
    assert_eq!(params.bool("grounded"), Some(true));
    assert!(params.has_trigger("attack"));
    assert_eq!(params.float("missing"), None);
    assert_eq!(params.bool("missing"), None);

    assert!(params.take_trigger("attack"));
    assert!(!params.take_trigger("attack"));
    params.set_trigger("a");
    params.set_trigger("b");
    params.clear_triggers();
    assert!(!params.has_trigger("a") && !params.has_trigger("b"));
    assert!(!params.is_empty());
}

#[test]
fn a_cooked_machine_becomes_evaluable() {
    use prism_asset::formats_state_machine::{
        AnimStateInfo, AnimTransitionInfo, StateMachineInfo, TransitionConditionInfo,
    };

    let info = StateMachineInfo {
        states: vec![
            AnimStateInfo {
                name: "Idle".into(),
                clip_path: "char/idle.ranim".into(),
                speed: 1.0,
                looped: true,
            },
            AnimStateInfo {
                name: "Run".into(),
                clip_path: "char/run.ranim".into(),
                speed: 1.2,
                looped: true,
            },
        ],
        transitions: vec![
            // Points to non-existent state: cooker wouldn't write such data, but bad assets shouldn't fail the whole machine.
            AnimTransitionInfo {
                from: None,
                to: 9,
                min_state_fraction: 0.0,
                duration: 0.0,
                conditions: Vec::new(),
            },
            AnimTransitionInfo {
                from: Some(0),
                to: 1,
                min_state_fraction: 0.25,
                duration: 0.2,
                conditions: vec![TransitionConditionInfo::FloatGreater {
                    name: "speed".into(),
                    threshold: 1.5,
                }],
            },
            AnimTransitionInfo {
                from: Some(1),
                to: 0,
                min_state_fraction: 0.0,
                duration: 0.0,
                conditions: vec![TransitionConditionInfo::Trigger {
                    name: "Stop".into(),
                }],
            },
        ],
    };

    let machine = StateMachine::from_info(&info);
    assert_eq!(machine.states.len(), 2);
    assert_eq!(
        machine.transitions.len(),
        2,
        "out-of-bounds transitions dropped"
    );
    assert!((machine.states[1].speed - 1.2).abs() < 1e-6);

    // Idle only played 10%: min_state_fraction blocks.
    let mut params = AnimParameters::new();
    params.set_float("speed", 2.0);
    assert!(machine.next_transition(0, &mut params, 0.1, 1.0).is_none());
    // Played to 30%: transition to Run, 0.2s.
    let target = machine
        .next_transition(0, &mut params, 0.3, 1.0)
        .expect("transitions");
    assert_eq!(target.state, 1);
    assert!((target.duration - 0.2).abs() < 1e-6);

    // In Run press Stop: trigger consumed, direction back to Idle.
    params.set_trigger("Stop");
    let target = machine
        .next_transition(1, &mut params, 0.5, 1.0)
        .expect("transitions");
    assert_eq!(target.state, 0);
    assert!(!params.has_trigger("Stop"));
    assert!(
        machine.next_transition(1, &mut params, 0.5, 1.0).is_none(),
        "trigger only fires once"
    );
}
