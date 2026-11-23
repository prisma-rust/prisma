//! Read-only view of animation graph / state machine assets (#560 editor panel).
//!
//! This does not copy the cooker's strict validation; it only flattens the fields suitable for
//! display from the authoring JSON into a UI-friendly structure: parameters, node types, clips,
//! connections, outputs, and state machine states / transitions / conditions.
//! Error info is retained up to the first bad field, so panels can display it directly.

use serde_json::{json, Value};
use std::collections::BTreeMap;

/// A parameter in an animation graph.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimParameterView {
    pub name: String,
    pub kind: String,
    pub default: f32,
}

/// A connection in an animation graph.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimInputView {
    pub node: String,
    pub weight: f32,
    /// Only affects these joints (empty = whole pose).
    pub bone_mask: Vec<String>,
}

/// A node in an animation graph.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimNodeView {
    pub name: String,
    pub kind: String,
    pub clips: Vec<String>,
    /// Blend space sample coordinates; y is always 0 for 1D nodes.
    pub positions: Vec<[f32; 2]>,
    /// Playback speed per sample in the blend space; empty = all 1.0.
    pub speeds: Vec<f32>,
    pub inputs: Vec<AnimInputView>,
    pub state_machine: String,
    pub parameter: String,
    /// Second parameter name of a 2D blend space; empty for other nodes.
    pub parameter_y: String,
    pub speed: f32,
    pub weight: f32,
    pub looped: bool,
}

impl AnimNodeView {
    pub fn is_output(&self) -> bool {
        self.kind.eq_ignore_ascii_case("output")
    }
}

/// Read-only summary of an entire animation graph.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AnimGraphView {
    pub parameters: Vec<AnimParameterView>,
    pub nodes: Vec<AnimNodeView>,
    pub output: Option<String>,
}

/// A state in a state machine.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimStateView {
    pub name: String,
    pub clip: String,
    pub speed: f32,
    pub looped: bool,
}

/// A transition condition; `summary` is already in display-ready form.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimConditionView {
    /// `float_greater` / `float_less` / `bool_is` / `trigger`.
    pub kind: String,
    pub name: String,
    /// Only meaningful for float conditions.
    pub threshold: f32,
    /// Only meaningful for `bool_is`.
    pub value: bool,
    pub summary: String,
}

/// A transition in a state machine.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimTransitionView {
    /// `None` means any state (`*` / `any` / empty string).
    pub from: Option<String>,
    pub to: String,
    pub duration: f32,
    pub min_state_fraction: f32,
    pub conditions: Vec<AnimConditionView>,
}

/// Read-only summary of an entire state machine.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AnimStateMachineView {
    pub states: Vec<AnimStateView>,
    pub transitions: Vec<AnimTransitionView>,
}

/// Parse `*.animgraph.json`.
pub fn parse_anim_graph_view(document: &Value) -> Result<AnimGraphView, String> {
    let parameters = document
        .get("parameters")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(index, parameter)| {
            let name = parameter
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| format!("parameter #{index} needs a name"))?;
            Ok(AnimParameterView {
                name: name.to_string(),
                kind: parameter
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("float")
                    .to_string(),
                default: parameter
                    .get("default")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0) as f32,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let nodes_json = document
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| "animation graph needs a `nodes` array".to_string())?;
    if nodes_json.is_empty() {
        return Err("animation graph needs at least one node".to_string());
    }

    let mut nodes = Vec::with_capacity(nodes_json.len());
    for (index, node) in nodes_json.iter().enumerate() {
        let name = node
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| format!("node #{index} needs a non-empty name"))?
            .to_string();
        let mut clips = Vec::new();
        if let Some(clip) = node
            .get("clip")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|clip| !clip.is_empty())
        {
            clips.push(clip.to_string());
        }
        if let Some(list) = node.get("clips").and_then(Value::as_array) {
            for clip in list.iter().filter_map(Value::as_str) {
                if !clip.trim().is_empty() && !clips.iter().any(|known| known == clip) {
                    clips.push(clip.trim().to_string());
                }
            }
        }

        let mut inputs = Vec::new();
        if let Some(input) = node.get("input") {
            if let Some(input) = parse_input(input) {
                inputs.push(input);
            }
        }
        if let Some(list) = node.get("inputs").and_then(Value::as_array) {
            for input in list.iter().filter_map(parse_input) {
                inputs.push(input);
            }
        }

        let parameter = node
            .get("parameter")
            .or_else(|| node.get("parameter_x"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let parameter_y = node
            .get("parameter_y")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        nodes.push(AnimNodeView {
            name,
            kind: node
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            clips,
            positions: parse_positions(node),
            speeds: parse_speeds(node),
            inputs,
            state_machine: node
                .get("state_machine")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            parameter,
            parameter_y,
            speed: node.get("speed").and_then(Value::as_f64).unwrap_or(1.0) as f32,
            weight: node.get("weight").and_then(Value::as_f64).unwrap_or(1.0) as f32,
            looped: node.get("looped").and_then(Value::as_bool).unwrap_or(true),
        });
    }

    let output = nodes
        .iter()
        .find(|node| node.is_output())
        .map(|node| node.name.clone());
    Ok(AnimGraphView {
        parameters,
        nodes,
        output,
    })
}

fn parse_positions(node: &Value) -> Vec<[f32; 2]> {
    node.get("positions")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|position| match position {
            Value::Number(number) => [number.as_f64().unwrap_or(0.0) as f32, 0.0],
            Value::Array(pair) => [
                pair.first().and_then(Value::as_f64).unwrap_or(0.0) as f32,
                pair.get(1).and_then(Value::as_f64).unwrap_or(0.0) as f32,
            ],
            _ => [0.0, 0.0],
        })
        .collect()
}

fn parse_speeds(node: &Value) -> Vec<f32> {
    node.get("speeds")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|speed| speed.as_f64().map(|speed| speed as f32))
        .collect()
}

fn parse_input(value: &Value) -> Option<AnimInputView> {
    match value {
        Value::String(node) if !node.trim().is_empty() => Some(AnimInputView {
            node: node.trim().to_string(),
            weight: 1.0,
            bone_mask: Vec::new(),
        }),
        Value::Object(map) => {
            let node = map
                .get("node")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|node| !node.is_empty())?;
            Some(AnimInputView {
                node: node.to_string(),
                weight: map
                    .get("weight")
                    .and_then(Value::as_f64)
                    .map(|weight| weight as f32)
                    .unwrap_or(1.0),
                bone_mask: map
                    .get("bone_mask")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|joint| !joint.is_empty())
                    .map(str::to_string)
                    .collect(),
            })
        }
        _ => None,
    }
}

/// Parse `*.statemachine.json`.
pub fn parse_state_machine_view(document: &Value) -> Result<AnimStateMachineView, String> {
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
            .ok_or_else(|| format!("state #{index} needs a non-empty name"))?;
        states.push(AnimStateView {
            name: name.to_string(),
            clip: state
                .get("clip")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            speed: state.get("speed").and_then(Value::as_f64).unwrap_or(1.0) as f32,
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
        let to = transition
            .get("to")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|to| !to.is_empty())
            .ok_or_else(|| format!("transition #{index} needs a `to` state"))?
            .to_string();
        let from = transition
            .get("from")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|from| !from.is_empty() && *from != "*" && !from.eq_ignore_ascii_case("any"))
            .map(str::to_string);
        let conditions = transition
            .get("conditions")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(parse_condition)
            .collect();
        transitions.push(AnimTransitionView {
            from,
            to,
            duration: transition
                .get("duration")
                .and_then(Value::as_f64)
                .unwrap_or(0.0) as f32,
            min_state_fraction: transition
                .get("min_state_fraction")
                .and_then(Value::as_f64)
                .unwrap_or(0.0) as f32,
            conditions,
        });
    }

    Ok(AnimStateMachineView {
        states,
        transitions,
    })
}

fn parse_condition(condition: &Value) -> Option<AnimConditionView> {
    let kind = condition.get("type").and_then(Value::as_str)?;
    let name = condition.get("name").and_then(Value::as_str)?;
    let threshold = condition
        .get("threshold")
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as f32;
    let value = condition
        .get("value")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let summary = match kind {
        "float_greater" => format!("{name} > {threshold}"),
        "float_less" => format!("{name} < {threshold}"),
        "bool_is" => format!("{name} == {value}"),
        "trigger" => format!("{name} (trigger)"),
        other => format!("{name} ({other})"),
    };
    Some(AnimConditionView {
        kind: kind.to_string(),
        name: name.to_string(),
        threshold,
        value,
        summary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nodes_can_be_duplicated_with_unique_names() {
        let mut doc = json!({
            "parameters": [],
            "nodes": [
                { "name": "walk", "type": "clip_player", "clip": "walk.ranim", "speed": 1.5 },
                { "name": "out", "type": "output", "input": "walk" }
            ]
        });
        let copy = duplicate_graph_node(&mut doc, "walk").expect("copies");
        assert_eq!(copy, "walk_copy");
        assert_eq!(doc["nodes"][2]["name"], "walk_copy");
        assert_eq!(doc["nodes"][2]["clip"], "walk.ranim");
        assert_eq!(doc["nodes"][2]["speed"].as_f64(), Some(1.5));
        assert_eq!(
            doc["nodes"][1]["input"], "walk",
            "original reference not affected"
        );

        let second = duplicate_graph_node(&mut doc, "walk").expect("copies again");
        assert_eq!(second, "walk_copy2");
        let third = duplicate_graph_node(&mut doc, "walk_copy").expect("copies a copy");
        assert_eq!(third, "walk_copy_copy");
    }

    #[test]
    fn the_output_node_cannot_be_duplicated() {
        let mut doc = json!({
            "nodes": [
                { "name": "walk", "type": "clip_player", "clip": "walk.ranim" },
                { "name": "out", "type": "output", "input": "walk" }
            ]
        });
        assert!(duplicate_graph_node(&mut doc, "out").is_err());
        assert!(duplicate_graph_node(&mut doc, "missing").is_err());
        assert_eq!(
            doc["nodes"].as_array().unwrap().len(),
            2,
            "failure does not modify document"
        );
    }

    #[test]
    fn a_graph_view_lists_parameters_links_and_the_output() {
        let view = parse_anim_graph_view(&json!({
            "parameters": [{ "name": "speed", "type": "float", "default": 1.5 }],
            "nodes": [
                { "name": "locomotion", "type": "blend_space_1d",
                  "clips": ["idle.ranim", "run.ranim"], "parameter": "speed" },
                { "name": "aim", "type": "clip_player", "clip": "aim.ranim" },
                { "name": "stack", "type": "blend_stack",
                  "inputs": ["locomotion", { "node": "aim", "weight": 0.25 }] },
                { "name": "out", "type": "output", "input": "stack" }
            ]
        }))
        .expect("valid graph");
        assert_eq!(view.parameters[0].name, "speed");
        assert_eq!(view.parameters[0].default, 1.5);
        assert_eq!(view.nodes.len(), 4);
        assert_eq!(view.nodes[0].clips, ["idle.ranim", "run.ranim"]);
        assert_eq!(view.nodes[2].inputs[1].node, "aim");
        assert_eq!(view.nodes[2].inputs[1].weight, 0.25);
        assert_eq!(view.output.as_deref(), Some("out"));
    }

    #[test]
    fn a_graph_view_surfaces_missing_nodes_and_bad_names() {
        assert!(parse_anim_graph_view(&json!({})).is_err());
        assert!(parse_anim_graph_view(&json!({ "nodes": [] })).is_err());
        assert!(parse_anim_graph_view(&json!({ "nodes": [{ "type": "output" }] })).is_err());
    }

    #[test]
    fn states_can_be_duplicated_with_unique_names() {
        let mut doc = json!({
            "states": [
                { "name": "Idle", "clip": "idle.ranim" },
                { "name": "Run", "clip": "run.ranim", "speed": 1.5, "looped": false }
            ],
            "transitions": [{ "from": "Idle", "to": "Run", "duration": 0.2 }]
        });
        let copy = duplicate_state(&mut doc, "Run").expect("copies");
        assert_eq!(copy, "Run_copy");
        assert_eq!(doc["states"][2]["name"], "Run_copy");
        assert_eq!(doc["states"][2]["clip"], "run.ranim");
        assert_eq!(doc["states"][2]["speed"].as_f64(), Some(1.5));
        assert_eq!(doc["states"][2]["looped"], false);
        assert_eq!(
            doc["transitions"][0]["to"], "Run",
            "transition table unchanged"
        );
        assert_eq!(
            duplicate_state(&mut doc, "Run").expect("again"),
            "Run_copy2"
        );
        assert!(duplicate_state(&mut doc, "Missing").is_err());
    }

    #[test]
    fn a_state_machine_view_lists_states_and_readable_conditions() {
        let view = parse_state_machine_view(&json!({
            "states": [
                { "name": "Idle", "clip": "idle.ranim" },
                { "name": "Run", "clip": "run.ranim", "speed": 1.2, "looped": false }
            ],
            "transitions": [
                { "from": "Idle", "to": "Run", "duration": 0.2,
                  "conditions": [
                    { "type": "float_greater", "name": "speed", "threshold": 1.5 },
                    { "type": "trigger", "name": "jump" }
                  ] },
                { "from": "*", "to": "Idle", "duration": 0.0 }
            ]
        }))
        .expect("valid state machine");
        assert_eq!(view.states[1].speed, 1.2);
        assert!(!view.states[1].looped);
        assert_eq!(view.transitions[0].from.as_deref(), Some("Idle"));
        assert_eq!(view.transitions[0].conditions[0].summary, "speed > 1.5");
        assert_eq!(view.transitions[0].conditions[0].kind, "float_greater");
        assert_eq!(view.transitions[0].conditions[0].name, "speed");
        assert_eq!(view.transitions[0].conditions[0].threshold, 1.5);
        assert_eq!(view.transitions[0].conditions[1].summary, "jump (trigger)");
        assert_eq!(view.transitions[0].conditions[1].kind, "trigger");
        assert!(view.transitions[1].from.is_none());
    }
}

/// Canvas position of an animation graph node (`<graph>.layout.json`).
///
/// Stored separately from the graph: changing the layout does not change animation data nor
/// invalidate the cook cache. Coordinates are logical pixels within the panel, origin at the
/// top-left of the canvas.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AnimGraphLayout {
    pub positions: BTreeMap<String, [f32; 2]>,
}

impl AnimGraphLayout {
    /// Layout file path: right next to the graph asset, e.g. `graphs/hero.animgraph.json.layout.json`.
    pub fn path_for(graph_path: &str) -> String {
        format!("{graph_path}.layout.json")
    }

    pub fn position(&self, node: &str) -> Option<[f32; 2]> {
        self.positions.get(node).copied()
    }

    pub fn set_position(&mut self, node: impl Into<String>, position: [f32; 2]) {
        if position[0].is_finite() && position[1].is_finite() {
            self.positions.insert(node.into(), position);
        }
    }

    pub fn to_json(&self) -> Value {
        let nodes: serde_json::Map<String, Value> = self
            .positions
            .iter()
            .map(|(name, position)| (name.clone(), json!(position)))
            .collect();
        json!({ "version": 1, "nodes": nodes })
    }

    /// Parse the layout; missing file / empty object yields an empty layout, bad JSON returns an error.
    pub fn from_json(document: &Value) -> Result<Self, String> {
        let Some(nodes) = document.get("nodes") else {
            return Ok(Self::default());
        };
        let Some(nodes) = nodes.as_object() else {
            return Err("animation graph layout needs a `nodes` object".to_string());
        };
        let mut positions = BTreeMap::new();
        for (name, position) in nodes {
            let Some(values) = position.as_array() else {
                continue;
            };
            let (Some(x), Some(y)) = (
                values.first().and_then(Value::as_f64),
                values.get(1).and_then(Value::as_f64),
            ) else {
                continue;
            };
            if x.is_finite() && y.is_finite() {
                positions.insert(name.clone(), [x as f32, y as f32]);
            }
        }
        Ok(Self { positions })
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn a_layout_round_trips_node_positions() {
        let mut layout = AnimGraphLayout::default();
        layout.set_position("locomotion", [40.0, 80.0]);
        layout.set_position("out", [320.5, 12.0]);
        let json = layout.to_json();
        let parsed = AnimGraphLayout::from_json(&json).expect("parses");
        assert_eq!(parsed.position("locomotion"), Some([40.0, 80.0]));
        assert_eq!(parsed.position("out"), Some([320.5, 12.0]));
    }

    #[test]
    fn a_missing_or_malformed_layout_falls_back_to_empty() {
        assert!(AnimGraphLayout::from_json(&json!({}))
            .unwrap()
            .positions
            .is_empty());
        assert!(AnimGraphLayout::from_json(&json!({ "nodes": [] })).is_err());
        let layout = AnimGraphLayout::from_json(&json!({
            "nodes": { "a": [1.0, 2.0], "b": "bad", "c": [3.0] }
        }))
        .expect("tolerates bad entries");
        assert_eq!(layout.positions.len(), 1);
        assert_eq!(layout.position("a"), Some([1.0, 2.0]));
    }

    #[test]
    fn the_layout_path_sits_next_to_the_graph() {
        assert_eq!(
            AnimGraphLayout::path_for("graphs/hero.animgraph.json"),
            "graphs/hero.animgraph.json.layout.json"
        );
    }
}

fn find_node_mut<'a>(document: &'a mut Value, name: &str) -> Option<&'a mut Value> {
    document
        .get_mut("nodes")?
        .as_array_mut()?
        .iter_mut()
        .find(|node| node.get("name").and_then(Value::as_str) == Some(name))
}

fn input_name(value: &Value) -> Option<&str> {
    match value {
        Value::String(name) => Some(name),
        Value::Object(map) => map.get("node").and_then(Value::as_str),
        _ => None,
    }
}

/// Rename a graph node, and rewrite all references to it accordingly.
///
/// Changes: the node's own `name`, other nodes' `inputs` / `input` connections, and the top-level
/// `output` (if it points to this node). The graph layout in `<graph>.layout.json` is stored
/// separately and does not follow the rename — after renaming the node returns to the canvas's
/// default grid position, just drag it again.
pub fn rename_graph_node(document: &mut Value, old: &str, new: &str) -> Result<(), String> {
    let new = new.trim();
    if new.is_empty() {
        return Err("a node needs a non-empty name".to_string());
    }
    if old == new {
        return Ok(());
    }
    let Some(old_name) = find_node_mut(document, old)
        .and_then(|node| node.get("name").and_then(Value::as_str))
        .map(str::to_string)
    else {
        return Err(format!("unknown node '{old}'"));
    };
    let names: Vec<String> = document
        .get("nodes")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|node| node.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    if names.iter().any(|name| name == new) {
        return Err(format!("a node named '{new}' already exists"));
    }
    let Some(node) = find_node_mut(document, &old_name) else {
        return Err(format!("unknown node '{old_name}'"));
    };
    node["name"] = json!(new);
    if let Some(nodes) = document.get_mut("nodes").and_then(Value::as_array_mut) {
        for node in nodes.iter_mut() {
            if let Some(input) = node.get_mut("input") {
                if input.as_str() == Some(old_name.as_str()) {
                    *input = json!(new);
                }
            }
            let Some(inputs) = node.get_mut("inputs").and_then(Value::as_array_mut) else {
                continue;
            };
            for input in inputs.iter_mut() {
                match input {
                    Value::String(name) if name == &old_name => *name = new.to_string(),
                    Value::Object(map)
                        if map.get("node").and_then(Value::as_str) == Some(old_name.as_str()) =>
                    {
                        map.insert("node".to_string(), json!(new));
                    }
                    _ => {}
                }
            }
        }
    }
    if document.get("output").and_then(Value::as_str) == Some(old_name.as_str()) {
        document["output"] = json!(new);
    }
    Ok(())
}

/// Append a connection to a graph node (writing back to the author's `inputs` array).
///
/// Chained nodes (using singular `input` of `slot` / `cached_pose` / `output`) do not mix arrays:
/// return an error for them to avoid generating a shape the cooker doesn't recognize.
pub fn add_graph_input(document: &mut Value, target: &str, source: &str) -> Result<(), String> {
    if target == source {
        return Err("a node cannot connect to itself".to_string());
    }
    let names: Vec<String> = document
        .get("nodes")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|node| node.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    if !names.iter().any(|name| name == source) {
        return Err(format!("unknown source node '{source}'"));
    }
    let Some(node) = find_node_mut(document, target) else {
        return Err(format!("unknown target node '{target}'"));
    };
    if node.get("input").is_some() {
        return Err(format!(
            "node '{target}' uses a single input; edit it as JSON"
        ));
    }
    if node.get("inputs").is_none() {
        node["inputs"] = json!([]);
    }
    let Some(inputs) = node.get_mut("inputs").and_then(Value::as_array_mut) else {
        return Err(format!("node '{target}' has an invalid `inputs` field"));
    };
    if inputs
        .iter()
        .filter_map(input_name)
        .any(|name| name == source)
    {
        return Err(format!("node '{target}' already connects to '{source}'"));
    }
    inputs.push(Value::String(source.to_string()));
    Ok(())
}

/// Set a connection's weight (0..=1).
///
/// When the author writes a string `"aim"`, it is automatically upgraded to
/// `{ "node": "aim", "weight": w }`; an existing connection only changes `weight`,
/// keeping other fields such as `bone_mask`.
pub fn set_graph_input_weight(
    document: &mut Value,
    target: &str,
    index: usize,
    weight: f32,
) -> Result<(), String> {
    if !weight.is_finite() || !(0.0..=1.0).contains(&weight) {
        return Err("link weight must be finite and between 0 and 1".to_string());
    }
    let Some(node) = find_node_mut(document, target) else {
        return Err(format!("unknown target node '{target}'"));
    };
    let Some(inputs) = node.get_mut("inputs").and_then(Value::as_array_mut) else {
        return Err(format!("node '{target}' has no editable `inputs` array"));
    };
    let Some(input) = inputs.get_mut(index) else {
        return Err(format!("node '{target}' has no input #{index}"));
    };
    match input {
        Value::String(name) => {
            let name = name.clone();
            *input = json!({ "node": name, "weight": weight });
            Ok(())
        }
        Value::Object(map) => {
            map.insert("weight".to_string(), json!(weight));
            Ok(())
        }
        _ => Err(format!("node '{target}' input #{index} is malformed")),
    }
}

/// Remove the `index`-th connection from a graph node's `inputs` array.
pub fn remove_graph_input(document: &mut Value, target: &str, index: usize) -> Result<(), String> {
    let Some(node) = find_node_mut(document, target) else {
        return Err(format!("unknown target node '{target}'"));
    };
    let Some(inputs) = node.get_mut("inputs").and_then(Value::as_array_mut) else {
        return Err(format!("node '{target}' has no editable `inputs` array"));
    };
    if index >= inputs.len() {
        return Err(format!("node '{target}' has no input #{index}"));
    }
    inputs.remove(index);
    Ok(())
}

/// Set a connection's bone mask; empty list restores "whole pose".
///
/// String connections are upgraded to objects when a mask is needed (weight stays 1);
/// existing objects only replace `bone_mask`, keeping `weight` etc. Joint names are trimmed,
/// deduplicated, order-preserved; limit matches the cooker (1024).
pub fn set_graph_input_bone_mask(
    document: &mut Value,
    target: &str,
    index: usize,
    joints: &[String],
) -> Result<(), String> {
    const MAX_MASK: usize = 1024;
    let mut mask: Vec<&str> = Vec::new();
    for joint in joints {
        let joint = joint.trim();
        if !joint.is_empty() && !mask.contains(&joint) {
            mask.push(joint);
        }
    }
    if mask.len() > MAX_MASK {
        return Err(format!("a bone mask can hold at most {MAX_MASK} joints"));
    }
    let Some(node) = find_node_mut(document, target) else {
        return Err(format!("unknown target node '{target}'"));
    };
    let Some(inputs) = node.get_mut("inputs").and_then(Value::as_array_mut) else {
        return Err(format!("node '{target}' has no editable `inputs` array"));
    };
    let Some(input) = inputs.get_mut(index) else {
        return Err(format!("node '{target}' has no input #{index}"));
    };
    match input {
        Value::String(name) => {
            if mask.is_empty() {
                return Ok(());
            }
            let name = name.clone();
            *input = json!({ "node": name, "weight": 1.0, "bone_mask": mask });
            Ok(())
        }
        Value::Object(map) => {
            if mask.is_empty() {
                map.remove("bone_mask");
            } else {
                map.insert("bone_mask".to_string(), json!(mask));
            }
            Ok(())
        }
        _ => Err(format!("node '{target}' input #{index} is malformed")),
    }
}

/// Change a clip node (`clip_player`) clip path: write singular `clip` and remove the `clips` array.
///
/// The cooker concatenates `clip` and `clips`; having both means "more than one clip",
/// and the clip node is rejected, so when setting a single clip the array must be cleared.
pub fn set_graph_node_clip(document: &mut Value, name: &str, clip: &str) -> Result<(), String> {
    let clip = clip.trim();
    if clip.is_empty() {
        return Err("a clip node needs a clip path".to_string());
    }
    let Some(node) = find_node_mut(document, name) else {
        return Err(format!("unknown node '{name}'"));
    };
    if !node.is_object() {
        return Err(format!("node '{name}' is malformed"));
    }
    node["clip"] = json!(clip);
    if node.get("clips").is_some() {
        node.as_object_mut().expect("checked above").remove("clips");
    }
    Ok(())
}

/// How many dimensions a blend space has: `Some(1)` / `Some(2)`, other nodes return `None`.
fn blend_space_dims(node: &Value) -> Option<usize> {
    match node
        .get("type")
        .and_then(Value::as_str)?
        .to_ascii_lowercase()
        .as_str()
    {
        "blend_space_1d" | "blendspace1d" | "blend_space" => Some(1),
        "blend_space_2d" | "blendspace2d" => Some(2),
        _ => None,
    }
}

fn normalize_parameter_kind(kind: &str) -> Result<&'static str, String> {
    match kind.trim().to_ascii_lowercase().as_str() {
        "float" => Ok("float"),
        "bool" => Ok("bool"),
        "trigger" => Ok("trigger"),
        other => Err(format!(
            "unknown parameter type '{other}' (expected float / bool / trigger)"
        )),
    }
}

/// bool / trigger defaults are only 0 / 1; other numbers are normalized by non-zero.
fn parameter_default_for(kind: &str, default: f32) -> f32 {
    if kind == "float" {
        default
    } else if default >= 0.5 {
        1.0
    } else {
        0.0
    }
}

/// Add a parameter.
pub fn add_parameter(
    document: &mut Value,
    name: &str,
    kind: &str,
    default: f32,
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a parameter needs a name".to_string());
    }
    if !default.is_finite() {
        return Err("a parameter default must be finite".to_string());
    }
    let kind = normalize_parameter_kind(kind)?;
    if !document.get("parameters").is_some_and(Value::is_array) {
        document["parameters"] = json!([]);
    }
    let parameters = document
        .get_mut("parameters")
        .and_then(Value::as_array_mut)
        .expect("created above");
    if parameters
        .iter()
        .any(|parameter| parameter.get("name").and_then(Value::as_str) == Some(name))
    {
        return Err(format!("a parameter named '{name}' already exists"));
    }
    parameters.push(json!({
        "name": name,
        "type": kind,
        "default": parameter_default_for(kind, default),
    }));
    Ok(())
}

/// Remove a parameter; a parameter referenced by nodes cannot be deleted (it would silently
/// change the blend space's behavior).
pub fn remove_parameter(document: &mut Value, name: &str) -> Result<(), String> {
    if let Some(nodes) = document.get("nodes").and_then(Value::as_array) {
        for (index, node) in nodes.iter().enumerate() {
            for field in ["parameter", "parameter_x", "parameter_y"] {
                if node.get(field).and_then(Value::as_str) == Some(name) {
                    let node_name = node
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("<unnamed>");
                    return Err(format!(
                        "parameter '{name}' is used by node '{node_name}' (#{index}) as `{field}`"
                    ));
                }
            }
        }
    }
    let parameters = document
        .get_mut("parameters")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| format!("parameter '{name}' does not exist"))?;
    let Some(index) = parameters
        .iter()
        .position(|parameter| parameter.get("name").and_then(Value::as_str) == Some(name))
    else {
        return Err(format!("parameter '{name}' does not exist"));
    };
    parameters.remove(index);
    Ok(())
}

/// Change a parameter's default value; bool / trigger are normalized to 0 / 1.
pub fn set_parameter_default(document: &mut Value, name: &str, default: f32) -> Result<(), String> {
    if !default.is_finite() {
        return Err("a parameter default must be finite".to_string());
    }
    let Some(parameters) = document.get_mut("parameters").and_then(Value::as_array_mut) else {
        return Err(format!("parameter '{name}' does not exist"));
    };
    let Some(parameter) = parameters
        .iter_mut()
        .find(|parameter| parameter.get("name").and_then(Value::as_str) == Some(name))
    else {
        return Err(format!("parameter '{name}' does not exist"));
    };
    let kind = parameter
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| "float".to_string());
    parameter["default"] = json!(parameter_default_for(kind.trim(), default));
    Ok(())
}

/// Polygon area enclosed by sample points (shoelace formula, absolute value); returns 0 with
/// fewer than 3 points or non-finite coordinates.
///
/// The panel uses this to judge whether a 2D blend space is **degenerate**: when all samples
/// are collinear, runtime triangulation fails and silently falls back to 1D / single clip,
/// which is hard for the author to tell from the numbers.
pub fn blend_space_2d_area(positions: &[[f32; 2]]) -> f32 {
    if positions.len() < 3 {
        return 0.0;
    }
    if positions
        .iter()
        .any(|position| !position[0].is_finite() || !position[1].is_finite())
    {
        return 0.0;
    }
    let mut twice = 0.0f64;
    for (index, position) in positions.iter().enumerate() {
        let next = positions[(index + 1) % positions.len()];
        twice += position[0] as f64 * next[1] as f64 - next[0] as f64 * position[1] as f64;
    }
    (twice * 0.5).abs() as f32
}

/// Whether 2D samples are degenerate (area close to 0 relative to bounding box).
///
/// Threshold scales with bounding box area: `area <= 1e-5 * max(1, width * height)`, so the
/// 0..10 and 0..1 velocity axis coordinate sets can use the same judgment.
pub fn blend_space_2d_degenerate(positions: &[[f32; 2]]) -> bool {
    if positions.len() < 3 {
        return false;
    }
    let area = blend_space_2d_area(positions);
    let (mut min_x, mut max_x) = (f32::INFINITY, f32::NEG_INFINITY);
    let (mut min_y, mut max_y) = (f32::INFINITY, f32::NEG_INFINITY);
    for position in positions {
        if !position[0].is_finite() || !position[1].is_finite() {
            return true;
        }
        min_x = min_x.min(position[0]);
        max_x = max_x.max(position[0]);
        min_y = min_y.min(position[1]);
        max_y = max_y.max(position[1]);
    }
    let extent = ((max_x - min_x) * (max_y - min_y)).max(1.0);
    area <= 1e-5 * extent
}

/// Change a blend space's parameter name: 1D writes only `parameter`, 2D writes `parameter_y` too.
pub fn set_blend_space_parameter(
    document: &mut Value,
    name: &str,
    x: &str,
    y: &str,
) -> Result<(), String> {
    let x = x.trim();
    if x.is_empty() {
        return Err("a blend space needs a parameter name".to_string());
    }
    let Some(node) = find_node_mut(document, name) else {
        return Err(format!("unknown node '{name}'"));
    };
    if !node.is_object() {
        return Err(format!("node '{name}' is malformed"));
    }
    let Some(dims) = blend_space_dims(node) else {
        return Err(format!("node '{name}' is not a blend space"));
    };
    node["parameter"] = json!(x);
    if dims == 2 {
        let y = y.trim();
        if y.is_empty() {
            return Err("a 2D blend space needs `parameter_y`".to_string());
        }
        node["parameter_y"] = json!(y);
    } else if node.get("parameter_y").is_some() {
        node.as_object_mut()
            .expect("checked above")
            .remove("parameter_y");
    }
    Ok(())
}

/// Rename a parameter, and rewrite its references within the graph
/// (blend space nodes' `parameter` / `parameter_x` / `parameter_y`).
///
/// Only the graph itself; condition names in the `.statemachine.json` referenced by the graph
/// need [`rename_condition_parameter`]; cross-file version see
/// `Workspace::rename_anim_graph_parameter`.
pub fn rename_parameter(document: &mut Value, old: &str, new: &str) -> Result<(), String> {
    let old = old.trim();
    let new = new.trim();
    if new.is_empty() {
        return Err("a parameter needs a name".to_string());
    }
    if old == new {
        return Ok(());
    }
    let Some(parameters) = document.get_mut("parameters").and_then(Value::as_array_mut) else {
        return Err(format!("parameter '{old}' does not exist"));
    };
    if parameters
        .iter()
        .any(|parameter| parameter.get("name").and_then(Value::as_str) == Some(new))
    {
        return Err(format!("a parameter named '{new}' already exists"));
    }
    let Some(parameter) = parameters
        .iter_mut()
        .find(|parameter| parameter.get("name").and_then(Value::as_str) == Some(old))
    else {
        return Err(format!("parameter '{old}' does not exist"));
    };
    parameter["name"] = json!(new);
    if let Some(nodes) = document.get_mut("nodes").and_then(Value::as_array_mut) {
        for node in nodes {
            for field in ["parameter", "parameter_x", "parameter_y"] {
                if node.get(field).and_then(Value::as_str) == Some(old) {
                    node[field] = json!(new);
                }
            }
        }
    }
    Ok(())
}

/// Rename all conditions referencing `old` to `new` in the state machine; returns whether anything changed.
///
/// Only the condition name is changed, not condition type / threshold; nonexistent parameter
/// names in the state machine are not created.
pub fn rename_condition_parameter(
    document: &mut Value,
    old: &str,
    new: &str,
) -> Result<bool, String> {
    let old = old.trim();
    let new = new.trim();
    if new.is_empty() {
        return Err("a parameter needs a name".to_string());
    }
    let mut changed = false;
    let Some(transitions) = document
        .get_mut("transitions")
        .and_then(Value::as_array_mut)
    else {
        return Ok(false);
    };
    for transition in transitions {
        let Some(conditions) = transition
            .get_mut("conditions")
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        for condition in conditions {
            if condition.get("name").and_then(Value::as_str) == Some(old) {
                condition["name"] = json!(new);
                changed = true;
            }
        }
    }
    Ok(changed)
}

/// Change a blend space sample's coordinates; 1D nodes only write x (`positions` holds numbers).
pub fn set_blend_space_position(
    document: &mut Value,
    name: &str,
    index: usize,
    position: [f32; 2],
) -> Result<(), String> {
    if !position[0].is_finite() || !position[1].is_finite() {
        return Err("a blend space position must be finite".to_string());
    }
    let Some(node) = find_node_mut(document, name) else {
        return Err(format!("unknown node '{name}'"));
    };
    if !node.is_object() {
        return Err(format!("node '{name}' is malformed"));
    };
    let Some(dims) = blend_space_dims(node) else {
        return Err(format!("node '{name}' is not a blend space"));
    };
    let Some(positions) = node.get_mut("positions").and_then(Value::as_array_mut) else {
        return Err(format!("node '{name}' has no editable `positions` array"));
    };
    let Some(entry) = positions.get_mut(index) else {
        return Err(format!("node '{name}' has no sample #{index}"));
    };
    *entry = if dims == 1 {
        json!(position[0])
    } else {
        json!([position[0], position[1]])
    };
    Ok(())
}

/// Append a sample to a blend space (`clips` / `positions` grow together).
///
/// If the node has per-sample `speeds` (v2), a `1.0` is also added, otherwise the cooker
/// rejects the whole graph for "clip count and speed count mismatch". A node with both `clip`
/// and `clips` errors first, because sample order would become ambiguous.
pub fn add_blend_space_sample(
    document: &mut Value,
    name: &str,
    clip: &str,
    position: [f32; 2],
) -> Result<(), String> {
    let clip = clip.trim();
    if clip.is_empty() {
        return Err("a blend space sample needs a clip path".to_string());
    }
    if !position[0].is_finite() || !position[1].is_finite() {
        return Err("a blend space position must be finite".to_string());
    }
    let Some(node) = find_node_mut(document, name) else {
        return Err(format!("unknown node '{name}'"));
    };
    if !node.is_object() {
        return Err(format!("node '{name}' is malformed"));
    }
    let Some(dims) = blend_space_dims(node) else {
        return Err(format!("node '{name}' is not a blend space"));
    };
    if node.get("clip").is_some() {
        return Err(format!(
            "node '{name}' uses a single `clip`; move it into `clips` first"
        ));
    }
    if node.get("clips").is_none() {
        node["clips"] = json!([]);
    }
    if node.get("positions").is_none() {
        node["positions"] = json!([]);
    }
    let clip_count = node
        .get("clips")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    let position_count = node
        .get("positions")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    if clip_count != position_count {
        return Err(format!(
            "node '{name}' has {clip_count} clips but {position_count} positions; fix the mismatch first"
        ));
    }
    if let Some(speeds) = node.get("speeds").and_then(Value::as_array) {
        if speeds.len() != clip_count {
            return Err(format!(
                "node '{name}' has {clip_count} clips but {} sample speeds; fix the mismatch first",
                speeds.len()
            ));
        }
    }
    let Some(clips) = node.get_mut("clips").and_then(Value::as_array_mut) else {
        return Err(format!("node '{name}' has an invalid `clips` field"));
    };
    clips.push(json!(clip));
    let Some(positions) = node.get_mut("positions").and_then(Value::as_array_mut) else {
        return Err(format!("node '{name}' has an invalid `positions` field"));
    };
    positions.push(if dims == 1 {
        json!(position[0])
    } else {
        json!([position[0], position[1]])
    });
    if let Some(speeds) = node.get_mut("speeds").and_then(Value::as_array_mut) {
        speeds.push(json!(1.0));
    }
    Ok(())
}

/// Remove one sample from a blend space; 1D needs at least 2 left, 2D at least 3, otherwise
/// the cooker rejects.
pub fn remove_blend_space_sample(
    document: &mut Value,
    name: &str,
    index: usize,
) -> Result<(), String> {
    let Some(node) = find_node_mut(document, name) else {
        return Err(format!("unknown node '{name}'"));
    };
    if !node.is_object() {
        return Err(format!("node '{name}' is malformed"));
    }
    let Some(dims) = blend_space_dims(node) else {
        return Err(format!("node '{name}' is not a blend space"));
    };
    let Some(clips) = node.get_mut("clips").and_then(Value::as_array_mut) else {
        return Err(format!("node '{name}' has no editable `clips` array"));
    };
    if index >= clips.len() {
        return Err(format!("node '{name}' has no sample #{index}"));
    }
    let minimum = if dims == 1 { 2 } else { 3 };
    if clips.len() <= minimum {
        return Err(format!(
            "a {dims}D blend space needs at least {minimum} samples"
        ));
    }
    clips.remove(index);
    if let Some(positions) = node.get_mut("positions").and_then(Value::as_array_mut) {
        if index < positions.len() {
            positions.remove(index);
        }
    }
    if let Some(speeds) = node.get_mut("speeds").and_then(Value::as_array_mut) {
        if index < speeds.len() {
            speeds.remove(index);
        }
    }
    Ok(())
}

/// Change a blend space sample's clip path (`clips[index]`).
pub fn set_blend_space_sample_clip(
    document: &mut Value,
    name: &str,
    index: usize,
    clip: &str,
) -> Result<(), String> {
    let clip = clip.trim();
    if clip.is_empty() {
        return Err("a blend space sample needs a clip path".to_string());
    }
    let Some(node) = find_node_mut(document, name) else {
        return Err(format!("unknown node '{name}'"));
    };
    if !node.is_object() {
        return Err(format!("node '{name}' is malformed"));
    };
    if blend_space_dims(node).is_none() {
        return Err(format!("node '{name}' is not a blend space"));
    }
    let Some(clips) = node.get_mut("clips").and_then(Value::as_array_mut) else {
        return Err(format!("node '{name}' has no editable `clips` array"));
    };
    let Some(entry) = clips.get_mut(index) else {
        return Err(format!("node '{name}' has no sample #{index}"));
    };
    *entry = json!(clip);
    Ok(())
}

/// Change a blend space sample's playback speed (`speeds[index]`).
///
/// If the node has no `speeds` yet, a `1.0` column is filled by clip count first; an existing
/// array length must match `clips` (the cooker rejects misalignment), otherwise the author is
/// asked to fix the count first.
pub fn set_blend_space_sample_speed(
    document: &mut Value,
    name: &str,
    index: usize,
    speed: f32,
) -> Result<(), String> {
    if !speed.is_finite() || speed < 0.0 {
        return Err("sample speed must be finite and >= 0".to_string());
    }
    let Some(node) = find_node_mut(document, name) else {
        return Err(format!("unknown node '{name}'"));
    };
    if !node.is_object() {
        return Err(format!("node '{name}' is malformed"));
    }
    if blend_space_dims(node).is_none() {
        return Err(format!("node '{name}' is not a blend space"));
    }
    let clip_count = node
        .get("clips")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    if index >= clip_count {
        return Err(format!("node '{name}' has no sample #{index}"));
    }
    if node.get("speeds").is_none() {
        node["speeds"] = Value::Array(vec![json!(1.0); clip_count]);
    }
    let Some(speeds) = node.get_mut("speeds").and_then(Value::as_array_mut) else {
        return Err(format!("node '{name}' has an invalid `speeds` array"));
    };
    if speeds.len() != clip_count {
        return Err(format!(
            "node '{name}' has {clip_count} clips but {} sample speeds; fix the mismatch first",
            speeds.len()
        ));
    }
    speeds[index] = json!(speed);
    Ok(())
}

/// Change a node's playback speed (`0` = frozen, must be finite and non-negative).
pub fn set_graph_node_speed(document: &mut Value, name: &str, speed: f32) -> Result<(), String> {
    if !speed.is_finite() || speed < 0.0 {
        return Err("node speed must be finite and >= 0".to_string());
    }
    set_node_field(document, name, "speed", json!(speed))
}

/// Change a node's own weight (0..=1; the cooker also clamps to this range).
pub fn set_graph_node_weight(document: &mut Value, name: &str, weight: f32) -> Result<(), String> {
    if !weight.is_finite() || !(0.0..=1.0).contains(&weight) {
        return Err("node weight must be finite and between 0 and 1".to_string());
    }
    set_node_field(document, name, "weight", json!(weight))
}

/// Change whether a node loops playback.
pub fn set_graph_node_looped(document: &mut Value, name: &str, looped: bool) -> Result<(), String> {
    set_node_field(document, name, "looped", json!(looped))
}

fn set_node_field(
    document: &mut Value,
    name: &str,
    field: &str,
    value: Value,
) -> Result<(), String> {
    let Some(node) = find_node_mut(document, name) else {
        return Err(format!("unknown node '{name}'"));
    };
    if !node.is_object() {
        return Err(format!("node '{name}' is malformed"));
    }
    node[field] = value;
    Ok(())
}

/// Append a clip node (`clip_player`, single clip).
///
/// Only creates a shape that **passes cooker validation in the current frame**: a clip node's
/// required field is only the clip path, connections can be added later. `blend_space_*`
/// / `state_machine` / `blend_stack` need clips, coordinates, machine path, or at least one
/// input to be valid; creating them out of thin air would write a document the cook can't
/// handle, so these types are still hand-written JSON.
pub fn add_clip_node(document: &mut Value, name: &str, clip: &str) -> Result<(), String> {
    let name = name.trim();
    let clip = clip.trim();
    if name.is_empty() {
        return Err("a node needs a non-empty name".to_string());
    }
    if clip.is_empty() {
        return Err("a clip node needs a clip path".to_string());
    }
    let Some(nodes) = document.get_mut("nodes").and_then(Value::as_array_mut) else {
        return Err("animation graph needs a `nodes` array".to_string());
    };
    if nodes
        .iter()
        .any(|node| node.get("name").and_then(Value::as_str) == Some(name))
    {
        return Err(format!("a node named '{name}' already exists"));
    }
    nodes.push(json!({ "name": name, "type": "clip_player", "clip": clip }));
    Ok(())
}

/// Append an animation graph node.
///
/// Only accepts node types that pass cooker validation on the spot: `clip_player` /
/// `blend_stack` / `slot` / `layered_blend` / `cached_pose` / `state_machine`. Blend spaces
/// need >=2/3 clips and coordinates, still hand-written or copied from an existing node by the
/// author. `source`:
/// - `clip_player`: clip path;
/// - `state_machine`: state machine path;
/// - others: a single upstream node name (the cooker requires slot / cached pose to have exactly
///   one input, blend stack / layered blend at least one).
pub fn add_graph_node(
    document: &mut Value,
    name: &str,
    kind: &str,
    source: &str,
) -> Result<(), String> {
    let name = name.trim();
    let source = source.trim();
    if name.is_empty() {
        return Err("a node needs a non-empty name".to_string());
    }
    let kind = match kind.trim().to_ascii_lowercase().as_str() {
        "clip" | "clip_player" | "clipplayer" => "clip_player",
        "blend_stack" | "blendstack" => "blend_stack",
        "slot" => "slot",
        "layered_blend" | "layeredblend" => "layered_blend",
        "cached_pose" | "cachedpose" => "cached_pose",
        "state_machine" | "statemachine" => "state_machine",
        other => return Err(format!("unsupported node type '{other}'")),
    };
    let Some(nodes) = document.get_mut("nodes").and_then(Value::as_array_mut) else {
        return Err("animation graph needs a `nodes` array".to_string());
    };
    if nodes
        .iter()
        .any(|node| node.get("name").and_then(Value::as_str) == Some(name))
    {
        return Err(format!("a node named '{name}' already exists"));
    }
    match kind {
        "clip_player" => {
            if source.is_empty() {
                return Err("a clip node needs a clip path".to_string());
            }
            nodes.push(json!({ "name": name, "type": kind, "clip": source }));
        }
        "state_machine" => {
            if source.is_empty() {
                return Err("a state machine node needs a machine path".to_string());
            }
            nodes.push(json!({ "name": name, "type": kind, "state_machine": source }));
        }
        _ => {
            if source.is_empty() {
                return Err(format!("a {kind} node needs an input node"));
            }
            if !nodes
                .iter()
                .any(|node| node.get("name").and_then(Value::as_str) == Some(source))
            {
                return Err(format!("unknown input node '{source}'"));
            }
            if kind == "blend_stack" {
                nodes.push(json!({ "name": name, "type": kind, "inputs": [source] }));
            } else {
                nodes.push(json!({ "name": name, "type": kind, "input": source }));
            }
        }
    }
    Ok(())
}

/// Duplicate a node, return the new name.
///
/// The new name appends `_copy` / `_copy2`… after the original; other fields (clip, coordinates,
/// inputs, bone mask) are kept as-is, so the duplicated node is itself valid, with wiring still
/// pointing to the original upstreams. Output nodes cannot be duplicated (a graph has exactly
/// one output).
pub fn duplicate_graph_node(document: &mut Value, name: &str) -> Result<String, String> {
    let nodes = document
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| "animation graph needs a `nodes` array".to_string())?;
    let Some(source) = nodes
        .iter()
        .find(|node| node.get("name").and_then(Value::as_str) == Some(name))
    else {
        return Err(format!("unknown node '{name}'"));
    };
    if source
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.eq_ignore_ascii_case("output"))
    {
        return Err(format!(
            "node '{name}' is the graph output; a graph has exactly one"
        ));
    }
    let existing: std::collections::HashSet<&str> = nodes
        .iter()
        .filter_map(|node| node.get("name").and_then(Value::as_str))
        .collect();
    let base = format!("{name}_copy");
    let mut candidate = base.clone();
    let mut suffix = 2;
    while existing.contains(candidate.as_str()) {
        candidate = format!("{base}{suffix}");
        suffix += 1;
    }
    let mut copy = source.clone();
    if !copy.is_object() {
        return Err(format!("node '{name}' is malformed"));
    }
    copy["name"] = json!(candidate);
    let Some(nodes) = document.get_mut("nodes").and_then(Value::as_array_mut) else {
        return Err("animation graph needs a `nodes` array".to_string());
    };
    nodes.push(copy);
    Ok(candidate)
}

/// Remove a node; the output node, the last node, and referenced nodes cannot be removed directly.
///
/// When referenced, return an error naming the referrer instead of silently disconnecting: the
/// author decides where the line goes, the panel writes it back, and half a graph is never
/// persisted.
pub fn remove_graph_node(document: &mut Value, name: &str) -> Result<(), String> {
    let nodes = document
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| "animation graph needs a `nodes` array".to_string())?;
    let index = nodes
        .iter()
        .position(|node| node.get("name").and_then(Value::as_str) == Some(name))
        .ok_or_else(|| format!("unknown node '{name}'"))?;
    if nodes.len() <= 1 {
        return Err("an animation graph needs at least one node".to_string());
    }
    if nodes[index]
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.eq_ignore_ascii_case("output"))
    {
        return Err(format!(
            "node '{name}' is the graph output; rewire it instead"
        ));
    }
    for node in nodes {
        let target = node.get("name").and_then(Value::as_str).unwrap_or("?");
        let referenced = node
            .get("inputs")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(input_name)
            .any(|source| source == name)
            || node
                .get("input")
                .and_then(Value::as_str)
                .is_some_and(|source| source == name);
        if referenced {
            return Err(format!(
                "node '{target}' still connects to '{name}'; remove that link first"
            ));
        }
    }
    let Some(nodes) = document.get_mut("nodes").and_then(Value::as_array_mut) else {
        return Err("animation graph needs a `nodes` array".to_string());
    };
    nodes.remove(index);
    Ok(())
}

#[cfg(test)]
mod parameter_tests {
    use super::*;
    use serde_json::json;

    fn document() -> Value {
        json!({
            "parameters": [{ "name": "speed", "type": "float", "default": 1.5 }],
            "nodes": [
                { "name": "locomotion", "type": "blend_space_1d",
                  "clips": ["idle.ranim", "run.ranim"], "parameter": "speed" },
                { "name": "out", "type": "output", "input": "locomotion" }
            ]
        })
    }

    #[test]
    fn parameters_can_be_added_and_defaults_edited() {
        let mut doc = document();
        add_parameter(&mut doc, " grounded ", "Bool", 3.0).expect("adds");
        assert_eq!(doc["parameters"][1]["name"], "grounded");
        assert_eq!(doc["parameters"][1]["type"], "bool");
        assert_eq!(doc["parameters"][1]["default"].as_f64(), Some(1.0));

        set_parameter_default(&mut doc, "speed", 2.5).expect("float keeps value");
        assert_eq!(doc["parameters"][0]["default"].as_f64(), Some(2.5));
        set_parameter_default(&mut doc, "grounded", 0.2).expect("bool normalizes");
        assert_eq!(doc["parameters"][1]["default"].as_f64(), Some(0.0));

        let mut empty = json!({ "nodes": [] });
        add_parameter(&mut empty, "trigger", "trigger", 0.0).expect("creates the list");
        assert_eq!(empty["parameters"][0]["name"], "trigger");
    }

    #[test]
    fn parameters_can_be_renamed_across_graph_references() {
        let mut doc = document();
        rename_parameter(&mut doc, "speed", " velocity ").expect("renames");
        assert_eq!(doc["parameters"][0]["name"], "velocity");
        assert_eq!(doc["nodes"][0]["parameter"], "velocity");

        rename_parameter(&mut doc, "velocity", "velocity").expect("same name is a no-op");
        assert!(rename_parameter(&mut doc, "missing", "other").is_err());
        assert!(rename_parameter(&mut doc, "velocity", " ").is_err());
    }

    #[test]
    fn renaming_rejects_names_that_already_exist() {
        let mut doc = document();
        add_parameter(&mut doc, "grounded", "bool", 0.0).expect("adds");
        let error = rename_parameter(&mut doc, "speed", "grounded").expect_err("duplicate");
        assert!(error.contains("grounded"), "{error}");
        assert_eq!(
            doc["parameters"][0]["name"], "speed",
            "can't rename on failure"
        );
    }

    #[test]
    fn state_machine_conditions_follow_a_parameter_rename() {
        let mut machine = json!({
            "states": [
                { "name": "Idle", "clip": "idle.ranim" },
                { "name": "Run", "clip": "run.ranim" }
            ],
            "transitions": [
                { "from": "Idle", "to": "Run", "conditions": [
                    { "type": "float_greater", "name": "speed", "threshold": 1.0 },
                    { "type": "trigger", "name": "jump" }
                ] }
            ]
        });
        assert!(rename_condition_parameter(&mut machine, "speed", "velocity").expect("renames"));
        assert_eq!(
            machine["transitions"][0]["conditions"][0]["name"],
            "velocity"
        );
        assert_eq!(machine["transitions"][0]["conditions"][1]["name"], "jump");
        assert!(!rename_condition_parameter(&mut machine, "missing", "other").expect("no-op"));
        assert!(rename_condition_parameter(&mut machine, "jump", "").is_err());
    }

    #[test]
    fn referenced_parameters_cannot_be_removed() {
        let mut doc = document();
        let error = remove_parameter(&mut doc, "speed").expect_err("referenced");
        assert!(error.contains("locomotion"), "{error}");
        add_parameter(&mut doc, "extra", "float", 0.0).expect("adds");
        remove_parameter(&mut doc, "extra").expect("unreferenced removal");
        assert!(remove_parameter(&mut doc, "extra").is_err(), "already gone");
    }

    #[test]
    fn invalid_parameters_are_rejected() {
        let mut doc = document();
        assert!(add_parameter(&mut doc, "  ", "float", 0.0).is_err());
        assert!(
            add_parameter(&mut doc, "speed", "float", 0.0).is_err(),
            "duplicate"
        );
        assert!(
            add_parameter(&mut doc, "ok", "int", 0.0).is_err(),
            "unknown type"
        );
        assert!(add_parameter(&mut doc, "ok", "float", f32::NAN).is_err());
        assert!(set_parameter_default(&mut doc, "missing", 0.0).is_err());
        assert!(set_parameter_default(&mut doc, "speed", f32::INFINITY).is_err());
        assert!(remove_parameter(&mut doc, "missing").is_err());
    }
}

#[cfg(test)]
mod blend_space_area_tests {
    use super::*;

    #[test]
    fn the_area_uses_the_shoelace_formula() {
        let square = [[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0]];
        assert!((blend_space_2d_area(&square) - 4.0).abs() < 1e-6);
        assert!((blend_space_2d_area(&[[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]) - 0.5).abs() < 1e-6);
        assert_eq!(blend_space_2d_area(&[[0.0, 0.0], [1.0, 1.0]]), 0.0);
        assert_eq!(
            blend_space_2d_area(&[[0.0, 0.0], [f32::NAN, 1.0], [1.0, 0.0]]),
            0.0
        );
    }

    #[test]
    fn degenerate_sample_sets_are_flagged() {
        assert!(blend_space_2d_degenerate(&[
            [0.0, 0.0],
            [1.0, 1.0],
            [2.0, 2.0]
        ]));
        assert!(blend_space_2d_degenerate(&[
            [0.0, 0.0],
            [1.0, 0.0],
            [f32::INFINITY, 1.0]
        ]));
        assert!(!blend_space_2d_degenerate(&[
            [0.0, 0.0],
            [1.0, 0.0],
            [0.0, 1.0]
        ]));
        assert!(
            !blend_space_2d_degenerate(&[[0.0, 0.0], [1.0, 1.0]]),
            "two points are not enough to judge degeneracy"
        );
    }
}

#[cfg(test)]
mod blend_space_tests {
    use super::*;
    use serde_json::json;

    fn document() -> Value {
        json!({
            "nodes": [
                { "name": "locomotion", "type": "blend_space_1d",
                  "clips": ["idle.ranim", "run.ranim"], "positions": [0.0, 5.0],
                  "parameter": "speed" },
                { "name": "strafe", "type": "blend_space_2d",
                  "clips": ["f.ranim", "b.ranim", "l.ranim"],
                  "positions": [[0.0, 1.0], [0.0, -1.0], [-1.0, 0.0]],
                  "parameter_x": "forward", "parameter_y": "right" },
                { "name": "out", "type": "output", "input": "locomotion" }
            ]
        })
    }

    #[test]
    fn a_view_reads_sample_positions_for_both_dimensions() {
        let view = parse_anim_graph_view(&document()).expect("parses");
        assert_eq!(view.nodes[0].positions, [[0.0, 0.0], [5.0, 0.0]]);
        assert_eq!(view.nodes[0].parameter, "speed");
        assert!(view.nodes[0].parameter_y.is_empty());
        assert_eq!(
            view.nodes[1].positions,
            [[0.0, 1.0], [0.0, -1.0], [-1.0, 0.0]]
        );
        assert_eq!(view.nodes[1].parameter, "forward");
        assert_eq!(view.nodes[1].parameter_y, "right");
    }

    #[test]
    fn blend_space_parameters_can_be_edited() {
        let mut doc = document();
        set_blend_space_parameter(&mut doc, "locomotion", " locomotion_speed ", "").expect("1d");
        assert_eq!(doc["nodes"][0]["parameter"], "locomotion_speed");

        set_blend_space_parameter(&mut doc, "strafe", " fwd ", " side ").expect("2d");
        assert_eq!(doc["nodes"][1]["parameter"], "fwd");
        assert_eq!(doc["nodes"][1]["parameter_y"], "side");
    }

    #[test]
    fn bad_blend_space_parameters_are_rejected() {
        let mut doc = document();
        assert!(set_blend_space_parameter(&mut doc, "locomotion", "  ", "").is_err());
        assert!(set_blend_space_parameter(&mut doc, "strafe", "fwd", "  ").is_err());
        assert!(set_blend_space_parameter(&mut doc, "out", "fwd", "side").is_err());
        assert!(set_blend_space_parameter(&mut doc, "missing", "fwd", "side").is_err());
    }

    #[test]
    fn sample_positions_keep_their_dimension() {
        let mut doc = document();
        set_blend_space_position(&mut doc, "locomotion", 1, [7.5, 99.0]).expect("1d");
        assert_eq!(doc["nodes"][0]["positions"][1], json!(7.5));
        assert_eq!(doc["nodes"][0]["positions"][0], json!(0.0));

        set_blend_space_position(&mut doc, "strafe", 2, [-2.0, 0.5]).expect("2d");
        assert_eq!(doc["nodes"][1]["positions"][2], json!([-2.0, 0.5]));
    }

    #[test]
    fn bad_sample_positions_are_rejected() {
        let mut doc = document();
        assert!(set_blend_space_position(&mut doc, "locomotion", 0, [f32::NAN, 0.0]).is_err());
        assert!(set_blend_space_position(&mut doc, "locomotion", 9, [0.0, 0.0]).is_err());
        assert!(set_blend_space_position(&mut doc, "out", 0, [0.0, 0.0]).is_err());
        assert!(set_blend_space_position(&mut doc, "missing", 0, [0.0, 0.0]).is_err());
        let mut without = json!({
            "nodes": [{ "name": "space", "type": "blend_space_1d", "parameter": "speed" }]
        });
        assert!(set_blend_space_position(&mut without, "space", 0, [0.0, 0.0]).is_err());
    }

    #[test]
    fn samples_can_be_added_to_1d_and_2d_spaces() {
        let mut doc = document();
        add_blend_space_sample(&mut doc, "locomotion", " sprint.ranim ", [9.0, 1.0]).expect("1d");
        assert_eq!(
            doc["nodes"][0]["clips"],
            json!(["idle.ranim", "run.ranim", "sprint.ranim"])
        );
        assert_eq!(doc["nodes"][0]["positions"], json!([0.0, 5.0, 9.0]));

        add_blend_space_sample(&mut doc, "strafe", "r.ranim", [1.0, 1.0]).expect("2d");
        assert_eq!(doc["nodes"][1]["clips"].as_array().unwrap().len(), 4);
        assert_eq!(doc["nodes"][1]["positions"][3], json!([1.0, 1.0]));
    }

    #[test]
    fn adding_a_sample_keeps_the_speed_table_in_sync() {
        let mut doc = json!({
            "nodes": [{ "name": "space", "type": "blend_space_1d",
                        "clips": ["a.ranim", "b.ranim"], "positions": [0.0, 1.0],
                        "parameter": "speed", "speeds": [1.0, 2.0] }]
        });
        add_blend_space_sample(&mut doc, "space", "c.ranim", [2.0, 0.0]).expect("adds");
        assert_eq!(doc["nodes"][0]["speeds"], json!([1.0, 2.0, 1.0]));
        remove_blend_space_sample(&mut doc, "space", 1).expect("removes");
        assert_eq!(doc["nodes"][0]["speeds"], json!([1.0, 1.0]));
        assert_eq!(doc["nodes"][0]["positions"], json!([0.0, 2.0]));
    }

    #[test]
    fn removing_keeps_the_minimum_sample_count() {
        let mut doc = document();
        assert!(remove_blend_space_sample(&mut doc, "locomotion", 0).is_err());
        assert!(remove_blend_space_sample(&mut doc, "strafe", 0).is_err());
        assert!(remove_blend_space_sample(&mut doc, "locomotion", 5).is_err());
        assert!(remove_blend_space_sample(&mut doc, "missing", 0).is_err());
        assert!(remove_blend_space_sample(&mut doc, "out", 0).is_err());
    }

    #[test]
    fn adding_samples_validates_clips_positions_and_speeds() {
        let mut doc = document();
        assert!(add_blend_space_sample(&mut doc, "locomotion", "  ", [0.0, 0.0]).is_err());
        assert!(
            add_blend_space_sample(&mut doc, "locomotion", "x.ranim", [f32::NAN, 0.0]).is_err()
        );
        assert!(add_blend_space_sample(&mut doc, "out", "x.ranim", [0.0, 0.0]).is_err());
        assert!(add_blend_space_sample(&mut doc, "missing", "x.ranim", [0.0, 0.0]).is_err());

        let mut single = json!({
            "nodes": [{ "name": "s", "type": "blend_space_1d", "clip": "a.ranim",
                        "clips": ["a.ranim"], "positions": [0.0], "parameter": "speed" }]
        });
        assert!(add_blend_space_sample(&mut single, "s", "b.ranim", [1.0, 0.0]).is_err());

        let mut mismatched = json!({
            "nodes": [{ "name": "m", "type": "blend_space_1d",
                        "clips": ["a.ranim", "b.ranim"], "positions": [0.0],
                        "parameter": "speed" }]
        });
        assert!(add_blend_space_sample(&mut mismatched, "m", "c.ranim", [2.0, 0.0]).is_err());

        let mut bad_speeds = json!({
            "nodes": [{ "name": "v", "type": "blend_space_1d",
                        "clips": ["a.ranim", "b.ranim"], "positions": [0.0, 1.0],
                        "parameter": "speed", "speeds": [1.0] }]
        });
        assert!(add_blend_space_sample(&mut bad_speeds, "v", "c.ranim", [2.0, 0.0]).is_err());
    }

    #[test]
    fn sample_clips_can_be_replaced() {
        let mut doc = document();
        set_blend_space_sample_clip(&mut doc, "locomotion", 1, " jog.ranim ").expect("sets");
        assert_eq!(doc["nodes"][0]["clips"][1], "jog.ranim");
        assert!(set_blend_space_sample_clip(&mut doc, "locomotion", 0, "  ").is_err());
        assert!(set_blend_space_sample_clip(&mut doc, "locomotion", 9, "a.ranim").is_err());
        assert!(set_blend_space_sample_clip(&mut doc, "out", 0, "a.ranim").is_err());
    }

    #[test]
    fn a_view_reads_per_sample_speeds() {
        let mut doc = document();
        assert!(parse_anim_graph_view(&doc).expect("parses").nodes[0]
            .speeds
            .is_empty());
        doc["nodes"][0]["speeds"] = json!([1.0, 2.5]);
        let view = parse_anim_graph_view(&doc).expect("parses");
        assert_eq!(view.nodes[0].speeds, [1.0, 2.5]);
    }

    #[test]
    fn sample_speeds_can_be_created_and_edited() {
        let mut doc = document();
        set_blend_space_sample_speed(&mut doc, "locomotion", 1, 2.0).expect("creates table");
        assert_eq!(doc["nodes"][0]["speeds"], json!([1.0, 2.0]));

        set_blend_space_sample_speed(&mut doc, "locomotion", 0, 0.5).expect("edits");
        assert_eq!(doc["nodes"][0]["speeds"], json!([0.5, 2.0]));
    }

    #[test]
    fn bad_sample_speeds_are_rejected() {
        let mut doc = document();
        assert!(set_blend_space_sample_speed(&mut doc, "locomotion", 0, -1.0).is_err());
        assert!(set_blend_space_sample_speed(&mut doc, "locomotion", 0, f32::NAN).is_err());
        assert!(set_blend_space_sample_speed(&mut doc, "locomotion", 9, 1.0).is_err());
        assert!(set_blend_space_sample_speed(&mut doc, "out", 0, 1.0).is_err());
        assert!(set_blend_space_sample_speed(&mut doc, "missing", 0, 1.0).is_err());

        let mut mismatched = json!({
            "nodes": [{ "name": "m", "type": "blend_space_1d",
                        "clips": ["a.ranim", "b.ranim"], "positions": [0.0, 1.0],
                        "parameter": "speed", "speeds": [1.0] }]
        });
        assert!(set_blend_space_sample_speed(&mut mismatched, "m", 0, 2.0).is_err());
    }
}

#[cfg(test)]
mod rename_tests {
    use super::*;
    use serde_json::json;

    fn document() -> Value {
        json!({
            "output": "out",
            "nodes": [
                { "name": "a", "type": "clip_player", "clip": "a.ranim" },
                { "name": "b", "type": "clip_player", "clip": "b.ranim" },
                { "name": "stack", "type": "blend_stack",
                  "inputs": ["a", { "node": "b", "weight": 0.5 }] },
                { "name": "out", "type": "output", "input": "stack" }
            ]
        })
    }

    #[test]
    fn renaming_rewrites_every_reference() {
        let mut doc = document();
        rename_graph_node(&mut doc, "a", " run ").expect("renames");
        assert_eq!(doc["nodes"][0]["name"], "run");
        assert_eq!(doc["nodes"][2]["inputs"][0], "run");
        assert_eq!(doc["nodes"][2]["inputs"][1]["node"], "b");

        rename_graph_node(&mut doc, "b", "aim").expect("renames");
        assert_eq!(doc["nodes"][2]["inputs"][1]["node"], "aim");
        assert_eq!(doc["nodes"][2]["inputs"][1]["weight"], 0.5);

        rename_graph_node(&mut doc, "out", "root").expect("renames output");
        assert_eq!(doc["nodes"][3]["name"], "root");
        assert_eq!(doc["nodes"][3]["input"], "stack");
        assert_eq!(doc["output"], "root");
        assert_eq!(
            parse_anim_graph_view(&doc)
                .expect("parses")
                .output
                .as_deref(),
            Some("root")
        );
    }

    #[test]
    fn rename_rejects_duplicates_blanks_and_unknown_names() {
        let mut doc = document();
        assert!(rename_graph_node(&mut doc, "a", "b").is_err());
        assert!(rename_graph_node(&mut doc, "a", "  ").is_err());
        assert!(rename_graph_node(&mut doc, "missing", "aim").is_err());
        rename_graph_node(&mut doc, "a", "a").expect("same name is a no-op");
        assert_eq!(doc["nodes"][0]["name"], "a");
    }
}

#[cfg(test)]
mod node_prop_tests {
    use super::*;
    use serde_json::json;

    fn document() -> Value {
        json!({
            "nodes": [
                { "name": "a", "type": "clip_player", "clip": "a.ranim", "clips": ["a.ranim"] },
                { "name": "out", "type": "output", "input": "a" }
            ]
        })
    }

    #[test]
    fn a_clip_node_can_point_at_another_clip() {
        let mut doc = document();
        set_graph_node_clip(&mut doc, "a", " b.ranim ").expect("sets");
        assert_eq!(doc["nodes"][0]["clip"], "b.ranim");
        assert!(
            doc["nodes"][0].get("clips").is_none(),
            "a single clip can't also keep a clips array: {:?}",
            doc["nodes"][0]
        );
    }

    #[test]
    fn node_speed_weight_and_loop_can_be_edited() {
        let mut doc = document();
        set_graph_node_speed(&mut doc, "a", 1.5).expect("speed");
        set_graph_node_weight(&mut doc, "a", 0.25).expect("weight");
        set_graph_node_looped(&mut doc, "a", false).expect("looped");
        assert_eq!(doc["nodes"][0]["speed"].as_f64(), Some(1.5));
        assert_eq!(doc["nodes"][0]["weight"].as_f64(), Some(0.25));
        assert_eq!(doc["nodes"][0]["looped"], false);
    }

    #[test]
    fn bad_node_properties_are_rejected() {
        let mut doc = document();
        assert!(set_graph_node_clip(&mut doc, "a", "  ").is_err());
        assert!(set_graph_node_clip(&mut doc, "missing", "b.ranim").is_err());
        assert!(set_graph_node_speed(&mut doc, "a", -1.0).is_err());
        assert!(set_graph_node_speed(&mut doc, "a", f32::NAN).is_err());
        assert!(set_graph_node_weight(&mut doc, "a", 1.5).is_err());
        assert!(set_graph_node_weight(&mut doc, "a", f32::NAN).is_err());
        assert!(set_graph_node_looped(&mut doc, "missing", true).is_err());
        assert!(set_graph_node_clip(&mut json!({ "nodes": ["bad"] }), "a", "b.ranim").is_err());
    }
}

#[cfg(test)]
mod node_tests {
    use super::*;
    use serde_json::json;

    fn document() -> Value {
        json!({
            "nodes": [
                { "name": "a", "type": "clip_player", "clip": "a.ranim" },
                { "name": "b", "type": "clip_player", "clip": "b.ranim" },
                { "name": "stack", "type": "blend_stack", "inputs": ["a"] },
                { "name": "out", "type": "output", "input": "stack" }
            ]
        })
    }

    #[test]
    fn a_clip_node_can_be_added_and_shows_up_in_the_view() {
        let mut doc = document();
        add_clip_node(&mut doc, "  aim  ", " aim.ranim ").expect("adds");
        assert_eq!(doc["nodes"][4]["name"], "aim");
        assert_eq!(doc["nodes"][4]["type"], "clip_player");
        assert_eq!(doc["nodes"][4]["clip"], "aim.ranim");
        let view = parse_anim_graph_view(&doc).expect("still parses");
        assert_eq!(view.nodes.len(), 5);
    }

    #[test]
    fn duplicate_or_blank_clip_nodes_are_rejected() {
        let mut doc = document();
        assert!(add_clip_node(&mut doc, "a", "a.ranim").is_err());
        assert!(add_clip_node(&mut doc, "  ", "a.ranim").is_err());
        assert!(add_clip_node(&mut doc, "aim", "  ").is_err());
        assert!(add_clip_node(&mut json!({}), "aim", "a.ranim").is_err());
    }

    #[test]
    fn simple_nodes_can_be_added_with_required_wiring() {
        let mut doc = document();
        add_graph_node(&mut doc, " aim ", "clip_player", " aim.ranim ").expect("adds clip");
        assert_eq!(doc["nodes"][4]["clip"], "aim.ranim");
        add_graph_node(&mut doc, "sm", "state_machine", "anim/hero.rasm").expect("adds machine");
        assert_eq!(doc["nodes"][5]["state_machine"], "anim/hero.rasm");
        add_graph_node(&mut doc, "stack2", "blend_stack", "a").expect("adds stack");
        assert_eq!(doc["nodes"][6]["inputs"], json!(["a"]));
        add_graph_node(&mut doc, "slot", "slot", "a").expect("adds slot");
        assert_eq!(doc["nodes"][7]["input"], "a");
        add_graph_node(&mut doc, "layer", "layered_blend", "b").expect("adds layer");
        assert_eq!(doc["nodes"][8]["input"], "b");
        add_graph_node(&mut doc, "cached", "cached_pose", "stack").expect("adds cached pose");
        assert_eq!(doc["nodes"][9]["input"], "stack");
        let view = parse_anim_graph_view(&doc).expect("still parses");
        assert_eq!(view.nodes.len(), 10);
    }

    #[test]
    fn adding_nodes_validates_names_kinds_and_sources() {
        let mut doc = document();
        assert!(add_graph_node(&mut doc, "  ", "clip_player", "a.ranim").is_err());
        assert!(add_graph_node(&mut doc, "a", "clip_player", "a.ranim").is_err());
        assert!(add_graph_node(&mut doc, "x", "clip_player", "").is_err());
        assert!(add_graph_node(&mut doc, "x", "state_machine", "").is_err());
        assert!(add_graph_node(&mut doc, "x", "blend_space_1d", "a").is_err());
        assert!(add_graph_node(&mut doc, "x", "blend_stack", "missing").is_err());
        assert!(add_graph_node(&mut doc, "x", "blend_stack", "").is_err());
        assert!(add_graph_node(&mut doc, "x", "slot", "").is_err());
        assert!(add_graph_node(&mut doc, "x", "layered_blend", "").is_err());
        assert!(add_graph_node(&mut doc, "x", "cached_pose", "").is_err());
        assert!(add_graph_node(&mut json!({}), "x", "slot", "a").is_err());
        assert_eq!(
            doc["nodes"].as_array().unwrap().len(),
            4,
            "failure does not modify document"
        );
    }

    #[test]
    fn an_unreferenced_node_can_be_removed() {
        let mut doc = document();
        remove_graph_node(&mut doc, "b").expect("removes");
        assert_eq!(doc["nodes"].as_array().unwrap().len(), 3);
        assert!(doc["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|node| node["name"] != "b"));
    }

    #[test]
    fn the_output_referenced_and_missing_nodes_are_protected() {
        let mut doc = document();
        assert!(remove_graph_node(&mut doc, "out").is_err());
        assert!(remove_graph_node(&mut doc, "a").is_err());
        assert!(remove_graph_node(&mut doc, "stack").is_err());
        assert!(remove_graph_node(&mut doc, "missing").is_err());
        let mut single = json!({ "nodes": [{ "name": "a", "type": "clip_player" }] });
        assert!(remove_graph_node(&mut single, "a").is_err());
    }
}

#[cfg(test)]
mod link_tests {
    use super::*;

    fn document() -> Value {
        json!({
            "nodes": [
                { "name": "a", "type": "clip_player", "clip": "a.ranim" },
                { "name": "b", "type": "clip_player", "clip": "b.ranim" },
                { "name": "stack", "type": "blend_stack", "inputs": ["a"] },
                { "name": "out", "type": "output", "input": "stack" }
            ]
        })
    }

    #[test]
    fn links_can_be_added_and_removed() {
        let mut doc = document();
        add_graph_input(&mut doc, "stack", "b").expect("adds");
        assert_eq!(doc["nodes"][2]["inputs"], json!(["a", "b"]));
        remove_graph_input(&mut doc, "stack", 0).expect("removes");
        assert_eq!(doc["nodes"][2]["inputs"], json!(["b"]));
    }

    #[test]
    fn self_unknown_duplicate_and_chain_nodes_are_rejected() {
        let mut doc = document();
        assert!(add_graph_input(&mut doc, "stack", "stack").is_err());
        assert!(add_graph_input(&mut doc, "stack", "missing").is_err());
        assert!(add_graph_input(&mut doc, "stack", "a").is_err());
        assert!(add_graph_input(&mut doc, "out", "a").is_err());
        assert!(remove_graph_input(&mut doc, "stack", 9).is_err());
    }
}

/// Append a transition to a state machine (writing back to the author's `transitions` array).
///
/// `from = None` means any state. State names must exist, no self-loops, and the same
/// `(from, to)` cannot repeat; new transitions default to `duration = 0`, an unconditional hard cut.
pub fn add_state_transition(
    document: &mut Value,
    from: Option<&str>,
    to: &str,
) -> Result<(), String> {
    let states: Vec<String> = document
        .get("states")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|state| state.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    if !states.iter().any(|name| name == to) {
        return Err(format!("unknown target state '{to}'"));
    }
    if let Some(from) = from {
        if !states.iter().any(|name| name == from) {
            return Err(format!("unknown source state '{from}'"));
        }
        if from == to {
            return Err("a transition cannot start and end on the same state".to_string());
        }
    }
    if document.get("transitions").is_none() {
        document["transitions"] = json!([]);
    }
    let Some(transitions) = document
        .get_mut("transitions")
        .and_then(Value::as_array_mut)
    else {
        return Err("state machine has an invalid `transitions` field".to_string());
    };
    if transitions.iter().any(|transition| {
        let existing_from = transition
            .get("from")
            .and_then(Value::as_str)
            .filter(|from| *from != "*" && !from.eq_ignore_ascii_case("any"));
        let existing_to = transition.get("to").and_then(Value::as_str);
        existing_to == Some(to) && existing_from == from
    }) {
        return Err(format!(
            "a transition from '{}' to '{to}' already exists",
            from.unwrap_or("any")
        ));
    }
    transitions.push(json!({ "from": from.unwrap_or("*"), "to": to, "duration": 0.0 }));
    Ok(())
}

/// Change a transition's source / target state; `from = None` means any state.
///
/// State names must exist, no self-loops, and you can't produce a `(from, to)` identical to an
/// existing transition (duplicate transitions at runtime always hit the first one, meaningless).
/// Other fields are kept as-is.
pub fn set_transition_states(
    document: &mut Value,
    index: usize,
    from: Option<&str>,
    to: &str,
) -> Result<(), String> {
    let states: Vec<String> = document
        .get("states")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|state| state.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    if !states.iter().any(|name| name == to) {
        return Err(format!("unknown target state '{to}'"));
    }
    if let Some(from) = from {
        if !states.iter().any(|name| name == from) {
            return Err(format!("unknown source state '{from}'"));
        }
        if from == to {
            return Err("a transition cannot start and end on the same state".to_string());
        }
    }
    let transitions = document
        .get("transitions")
        .and_then(Value::as_array)
        .ok_or_else(|| "state machine has no editable `transitions` array".to_string())?;
    if index >= transitions.len() {
        return Err(format!("state machine has no transition #{index}"));
    }
    for (other_index, transition) in transitions.iter().enumerate() {
        if other_index == index {
            continue;
        }
        let other_from = transition
            .get("from")
            .and_then(Value::as_str)
            .filter(|from| *from != "*" && !from.eq_ignore_ascii_case("any"));
        let other_to = transition.get("to").and_then(Value::as_str);
        if other_to == Some(to) && other_from == from {
            return Err(format!(
                "a transition from '{}' to '{to}' already exists",
                from.unwrap_or("any")
            ));
        }
    }
    let transition = find_transition_mut(document, index)?;
    transition["from"] = json!(from.unwrap_or("*"));
    transition["to"] = json!(to);
    Ok(())
}

/// Move a transition back and forth in the `transitions` array, return the index after the move.
///
/// At runtime the **first** transition whose conditions match and whose source state matches is
/// taken (`next_transition` in `state_machine.rs`), so order is priority. Out-of-bounds moves
/// return an error instead of silently clamping to an endpoint; the panel uses this to disable
/// arrows at the ends.
pub fn move_state_transition(
    document: &mut Value,
    index: usize,
    offset: isize,
) -> Result<usize, String> {
    let transitions = document
        .get_mut("transitions")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "state machine has no editable `transitions` array".to_string())?;
    if index >= transitions.len() {
        return Err(format!("state machine has no transition #{index}"));
    }
    let len = transitions.len() as isize;
    let target = index as isize + offset;
    if target < 0 || target >= len {
        return Err(format!(
            "transition #{index} cannot move by {offset} (only {len} transitions)"
        ));
    }
    let transition = transitions.remove(index);
    transitions.insert(target as usize, transition);
    Ok(target as usize)
}

/// Duplicate a transition and insert it after the original, return the new index.
///
/// Producing an identical `(from, to)` is allowed: the author usually changes the source /
/// target or condition right after. At runtime the first matching transition is taken, so
/// duplicates before editing don't change behavior.
pub fn duplicate_state_transition(document: &mut Value, index: usize) -> Result<usize, String> {
    let transitions = document
        .get_mut("transitions")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "state machine has no editable `transitions` array".to_string())?;
    if index >= transitions.len() {
        return Err(format!("state machine has no transition #{index}"));
    }
    if !transitions[index].is_object() {
        return Err(format!("transition #{index} is malformed"));
    }
    let copy = transitions[index].clone();
    let target = index + 1;
    transitions.insert(target, copy);
    Ok(target)
}

/// Remove the `index`-th transition from a state machine's `transitions` array.
pub fn remove_state_transition(document: &mut Value, index: usize) -> Result<(), String> {
    let Some(transitions) = document
        .get_mut("transitions")
        .and_then(Value::as_array_mut)
    else {
        return Err("state machine has no editable `transitions` array".to_string());
    };
    if index >= transitions.len() {
        return Err(format!("state machine has no transition #{index}"));
    }
    transitions.remove(index);
    Ok(())
}

fn find_state_mut(document: &mut Value, index: usize) -> Result<&mut Value, String> {
    let state = document
        .get_mut("states")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "state machine needs a `states` array".to_string())?
        .get_mut(index)
        .ok_or_else(|| format!("state machine has no state #{index}"))?;
    if !state.is_object() {
        return Err(format!("state #{index} is malformed"));
    }
    Ok(state)
}

/// Change a state's clip path (must be non-empty after trimming).
pub fn set_state_clip(document: &mut Value, index: usize, clip: &str) -> Result<(), String> {
    let clip = clip.trim();
    if clip.is_empty() {
        return Err("a state needs a non-empty clip path".to_string());
    }
    find_state_mut(document, index)?["clip"] = json!(clip);
    Ok(())
}

/// Change a state's playback speed multiplier (`0` = frozen, must be finite and non-negative).
pub fn set_state_speed(document: &mut Value, index: usize, speed: f32) -> Result<(), String> {
    if !speed.is_finite() || speed < 0.0 {
        return Err("state speed must be finite and >= 0".to_string());
    }
    find_state_mut(document, index)?["speed"] = json!(speed);
    Ok(())
}

/// Change whether a state loops.
pub fn set_state_looped(document: &mut Value, index: usize, looped: bool) -> Result<(), String> {
    find_state_mut(document, index)?["looped"] = json!(looped);
    Ok(())
}

/// Duplicate a state, return the new name.
///
/// The new name automatically gets `_copy` / `_copy2`; the transition table is kept as-is (the
/// duplicated state temporarily has no transitions, the author connects them). Name uniqueness
/// is consistent with other states.
pub fn duplicate_state(document: &mut Value, name: &str) -> Result<String, String> {
    let states = document
        .get("states")
        .and_then(Value::as_array)
        .ok_or_else(|| "state machine needs a `states` array".to_string())?;
    let Some(source) = states
        .iter()
        .find(|state| state.get("name").and_then(Value::as_str) == Some(name))
    else {
        return Err(format!("unknown state '{name}'"));
    };
    if !source.is_object() {
        return Err(format!("state '{name}' is malformed"));
    }
    let existing: std::collections::HashSet<&str> = states
        .iter()
        .filter_map(|state| state.get("name").and_then(Value::as_str))
        .collect();
    let base = format!("{name}_copy");
    let mut candidate = base.clone();
    let mut suffix = 2;
    while existing.contains(candidate.as_str()) {
        candidate = format!("{base}{suffix}");
        suffix += 1;
    }
    let mut copy = source.clone();
    copy["name"] = json!(candidate);
    let Some(states) = document.get_mut("states").and_then(Value::as_array_mut) else {
        return Err("state machine needs a `states` array".to_string());
    };
    states.push(copy);
    Ok(candidate)
}

/// Move a state back and forth in the `states` array, return the index after the move.
///
/// The runtime treats the **first state** as the initial state (falls to index 0 when
/// `Animator.state` is empty), so this order also decides the initial state. Transitions
/// reference states by name, so reordering doesn't change transitions. Out-of-bounds moves
/// return an error instead of silently clamping to an endpoint; the panel uses this to disable
/// arrows at the ends.
pub fn move_state(document: &mut Value, index: usize, offset: isize) -> Result<usize, String> {
    let states = document
        .get_mut("states")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "state machine needs a `states` array".to_string())?;
    if index >= states.len() {
        return Err(format!("state machine has no state #{index}"));
    }
    let len = states.len() as isize;
    let target = index as isize + offset;
    if target < 0 || target >= len {
        return Err(format!(
            "state #{index} cannot move by {offset} (only {len} states)"
        ));
    }
    let state = states.remove(index);
    states.insert(target as usize, state);
    Ok(target as usize)
}

/// Append a state.
///
/// Name and clip path must both be non-empty after trimming (the cooker rejects empty clips);
/// name must not duplicate an existing state. New state defaults to `speed = 1.0`, `looped = true`,
/// filled in by the cooker's defaults.
pub fn add_state(document: &mut Value, name: &str, clip: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a state needs a non-empty name".to_string());
    }
    let clip = clip.trim();
    if clip.is_empty() {
        return Err("a state needs a non-empty clip path".to_string());
    }
    let states = document
        .get_mut("states")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "state machine needs a `states` array".to_string())?;
    if states.iter().any(|state| {
        !state.is_object()
            || state
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .is_none()
    }) {
        return Err("state machine has a malformed state entry".to_string());
    }
    if states
        .iter()
        .any(|state| state.get("name").and_then(Value::as_str) == Some(name))
    {
        return Err(format!("a state named '{name}' already exists"));
    }
    states.push(json!({ "name": name, "clip": clip }));
    Ok(())
}

/// Remove a state, and remove all transitions referencing it; return the number of transitions
/// removed along with it.
///
/// A state machine must keep at least one state (cooker and runtime both treat "the first state
/// is the initial state"); after removing the first state, the next state becomes initial.
/// Transitions to any state (`*` / `any`) are not affected.
pub fn remove_state(document: &mut Value, name: &str) -> Result<usize, String> {
    let name = name.trim();
    let state_index = {
        let states = document
            .get("states")
            .and_then(Value::as_array)
            .ok_or_else(|| "state machine needs a `states` array".to_string())?;
        let index = states
            .iter()
            .position(|state| state.get("name").and_then(Value::as_str) == Some(name))
            .ok_or_else(|| format!("unknown state '{name}'"))?;
        if states.len() <= 1 {
            return Err("a state machine needs at least one state".to_string());
        }
        index
    };
    if let Some(value) = document.get("transitions") {
        if !value.is_array() {
            return Err("state machine has an invalid `transitions` field".to_string());
        }
    }
    let states = document
        .get_mut("states")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "state machine needs a `states` array".to_string())?;
    states.remove(state_index);

    let Some(value) = document.get_mut("transitions") else {
        return Ok(0);
    };
    let transitions = value
        .as_array_mut()
        .ok_or_else(|| "state machine has an invalid `transitions` field".to_string())?;
    let before = transitions.len();
    transitions.retain(|transition| {
        let from = transition
            .get("from")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|from| !from.is_empty() && *from != "*" && !from.eq_ignore_ascii_case("any"));
        let to = transition.get("to").and_then(Value::as_str).map(str::trim);
        from != Some(name) && to != Some(name)
    });
    Ok(before - transitions.len())
}

/// Rename a state, and synchronously rewrite `from` / `to` in transitions.
///
/// Only the state machine document is changed: the runtime `Animator.state` and parameter names
/// in conditions are not here; the panel prompts the author to check whether `.scene.json`
/// hardcodes the old name. Name must be non-empty after trimming and not duplicate another state.
pub fn rename_state(document: &mut Value, index: usize, new_name: &str) -> Result<(), String> {
    let new_name = new_name.trim();
    if new_name.is_empty() {
        return Err("a state needs a non-empty name".to_string());
    }
    let old_name = find_state_mut(document, index)?
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if old_name == new_name {
        return Ok(());
    }
    let states = document
        .get("states")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if states
        .iter()
        .any(|state| state.get("name").and_then(Value::as_str) == Some(new_name))
    {
        return Err(format!("a state named '{new_name}' already exists"));
    }
    find_state_mut(document, index)?["name"] = json!(new_name);
    if let Some(transitions) = document
        .get_mut("transitions")
        .and_then(Value::as_array_mut)
    {
        for transition in transitions.iter_mut() {
            for key in ["from", "to"] {
                if transition.get(key).and_then(Value::as_str) == Some(old_name.as_str()) {
                    transition[key] = json!(new_name);
                }
            }
        }
    }
    Ok(())
}

fn find_transition_mut(document: &mut Value, index: usize) -> Result<&mut Value, String> {
    let transition = document
        .get_mut("transitions")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "state machine has no editable `transitions` array".to_string())?
        .get_mut(index)
        .ok_or_else(|| format!("state machine has no transition #{index}"))?;
    if !transition.is_object() {
        return Err(format!("transition #{index} is malformed"));
    }
    Ok(transition)
}

fn find_condition_mut<'a>(
    document: &'a mut Value,
    index: usize,
    condition: usize,
) -> Result<&'a mut Value, String> {
    let transition = find_transition_mut(document, index)?;
    let entry = transition
        .get_mut("conditions")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| format!("transition #{index} has no editable `conditions` array"))?
        .get_mut(condition)
        .ok_or_else(|| format!("transition #{index} has no condition #{condition}"))?;
    if !entry.is_object() {
        return Err(format!(
            "transition #{index} condition #{condition} is malformed"
        ));
    }
    Ok(entry)
}

/// Set a transition's blend duration and minimum state fraction.
///
/// Both match the runtime meaning: `duration` is fade-in seconds (0 = hard cut),
/// `min_state_fraction` is the minimum fraction played before leaving the source state (0..=1).
pub fn set_transition_timing(
    document: &mut Value,
    index: usize,
    duration: f32,
    min_state_fraction: f32,
) -> Result<(), String> {
    if !duration.is_finite() || duration < 0.0 {
        return Err("transition duration must be finite and >= 0".to_string());
    }
    if !min_state_fraction.is_finite() || !(0.0..=1.0).contains(&min_state_fraction) {
        return Err("min_state_fraction must be finite and between 0 and 1".to_string());
    }
    let transition = find_transition_mut(document, index)?;
    transition["duration"] = json!(duration);
    transition["min_state_fraction"] = json!(min_state_fraction);
    Ok(())
}

/// Append a condition to the `index`-th transition.
///
/// `kind` only accepts the four types the runtime knows; `threshold` is only used for float
/// conditions, `value` only for `bool_is`. The same `(type, name)` is not added twice — two
/// identical conditions just redundantly write AND.
pub fn add_state_condition(
    document: &mut Value,
    index: usize,
    kind: &str,
    name: &str,
    threshold: f32,
    value: bool,
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a condition needs a non-empty parameter name".to_string());
    }
    let kind = match kind.trim() {
        kind @ ("float_greater" | "float_less" | "bool_is" | "trigger") => kind,
        other => {
            return Err(format!(
                "unknown condition type '{other}' \
                 (expected float_greater / float_less / bool_is / trigger)"
            ))
        }
    };
    if !threshold.is_finite() {
        return Err("a condition threshold must be finite".to_string());
    }
    let transition = find_transition_mut(document, index)?;
    if transition.get("conditions").is_none() {
        transition["conditions"] = json!([]);
    }
    let Some(conditions) = transition
        .get_mut("conditions")
        .and_then(Value::as_array_mut)
    else {
        return Err(format!(
            "transition #{index} has an invalid `conditions` field"
        ));
    };
    if conditions.iter().any(|condition| {
        condition.get("type").and_then(Value::as_str) == Some(kind)
            && condition.get("name").and_then(Value::as_str) == Some(name)
    }) {
        return Err(format!(
            "transition #{index} already has the condition '{name} ({kind})'"
        ));
    }
    let condition = match kind {
        "float_greater" | "float_less" => {
            json!({ "type": kind, "name": name, "threshold": threshold })
        }
        "bool_is" => json!({ "type": kind, "name": name, "value": value }),
        _ => json!({ "type": kind, "name": name }),
    };
    conditions.push(condition);
    Ok(())
}

/// Remove the `condition`-th condition from the `index`-th transition.
pub fn remove_state_condition(
    document: &mut Value,
    index: usize,
    condition: usize,
) -> Result<(), String> {
    let transition = find_transition_mut(document, index)?;
    let Some(conditions) = transition
        .get_mut("conditions")
        .and_then(Value::as_array_mut)
    else {
        return Err(format!(
            "transition #{index} has no editable `conditions` array"
        ));
    };
    if condition >= conditions.len() {
        return Err(format!("transition #{index} has no condition #{condition}"));
    }
    conditions.remove(condition);
    Ok(())
}

/// Change a float condition's threshold; `bool_is` / `trigger` have no threshold, return an error.
pub fn set_condition_threshold(
    document: &mut Value,
    index: usize,
    condition: usize,
    threshold: f32,
) -> Result<(), String> {
    if !threshold.is_finite() {
        return Err("a condition threshold must be finite".to_string());
    }
    let entry = find_condition_mut(document, index, condition)?;
    if !matches!(
        entry.get("type").and_then(Value::as_str),
        Some("float_greater" | "float_less")
    ) {
        return Err(format!(
            "transition #{index} condition #{condition} has no threshold"
        ));
    }
    entry["threshold"] = json!(threshold);
    Ok(())
}

/// Change a `bool_is` condition's expected value; other condition types return an error.
pub fn set_condition_value(
    document: &mut Value,
    index: usize,
    condition: usize,
    value: bool,
) -> Result<(), String> {
    let entry = find_condition_mut(document, index, condition)?;
    if entry.get("type").and_then(Value::as_str) != Some("bool_is") {
        return Err(format!(
            "transition #{index} condition #{condition} is not a bool condition"
        ));
    }
    entry["value"] = json!(value);
    Ok(())
}

/// Switch a condition's type, and fill in / remove the payload that type requires.
///
/// The parameter name is kept; `float_greater` / `float_less` keep the existing threshold
/// (fill `0.0` if absent); `bool_is` keeps the existing boolean (fill `false` if absent);
/// `trigger` removes `threshold` / `value`. If switching would duplicate another `(type, name)`
/// in the same transition, reject it, to avoid writing a redundant condition the runtime can
/// never reach.
pub fn set_condition_kind(
    document: &mut Value,
    index: usize,
    condition: usize,
    kind: &str,
) -> Result<(), String> {
    let kind = match kind.trim() {
        kind @ ("float_greater" | "float_less" | "bool_is" | "trigger") => kind,
        other => {
            return Err(format!(
                "unknown condition type '{other}' \
                 (expected float_greater / float_less / bool_is / trigger)"
            ))
        }
    };
    let transition = find_transition_mut(document, index)?;
    let Some(conditions) = transition
        .get_mut("conditions")
        .and_then(Value::as_array_mut)
    else {
        return Err(format!(
            "transition #{index} has no editable `conditions` array"
        ));
    };
    if condition >= conditions.len() {
        return Err(format!("transition #{index} has no condition #{condition}"));
    }
    if !conditions[condition].is_object() {
        return Err(format!(
            "transition #{index} condition #{condition} is malformed"
        ));
    }
    let name = conditions[condition]
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            format!("transition #{index} condition #{condition} needs a non-empty name")
        })?
        .to_string();
    if conditions.iter().enumerate().any(|(other_index, other)| {
        other_index != condition
            && other.get("type").and_then(Value::as_str) == Some(kind)
            && other.get("name").and_then(Value::as_str) == Some(name.as_str())
    }) {
        return Err(format!(
            "transition #{index} already has the condition '{name} ({kind})'"
        ));
    }
    let entry = &mut conditions[condition];
    entry["type"] = json!(kind);
    match kind {
        "float_greater" | "float_less" => {
            if entry.get("threshold").and_then(Value::as_f64).is_none() {
                entry["threshold"] = json!(0.0);
            }
            if let Some(object) = entry.as_object_mut() {
                object.remove("value");
            }
        }
        "bool_is" => {
            if entry.get("value").and_then(Value::as_bool).is_none() {
                entry["value"] = json!(false);
            }
            if let Some(object) = entry.as_object_mut() {
                object.remove("threshold");
            }
        }
        _ => {
            if let Some(object) = entry.as_object_mut() {
                object.remove("threshold");
                object.remove("value");
            }
        }
    }
    Ok(())
}

/// Change a condition's parameter name, keeping its type and payload.
///
/// `(type, name)` cannot repeat within the same transition; `float_greater speed` and
/// `float_less speed` are two different conditions, allowed to coexist.
pub fn set_condition_name(
    document: &mut Value,
    index: usize,
    condition: usize,
    name: &str,
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a condition needs a non-empty parameter name".to_string());
    }
    let transition = find_transition_mut(document, index)?;
    let Some(conditions) = transition
        .get_mut("conditions")
        .and_then(Value::as_array_mut)
    else {
        return Err(format!(
            "transition #{index} has no editable `conditions` array"
        ));
    };
    if condition >= conditions.len() {
        return Err(format!("transition #{index} has no condition #{condition}"));
    }
    if !conditions[condition].is_object() {
        return Err(format!(
            "transition #{index} condition #{condition} is malformed"
        ));
    }
    let kind = conditions[condition]
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("transition #{index} condition #{condition} needs a type"))?
        .to_string();
    if conditions.iter().enumerate().any(|(other_index, other)| {
        other_index != condition
            && other.get("type").and_then(Value::as_str) == Some(kind.as_str())
            && other.get("name").and_then(Value::as_str) == Some(name)
    }) {
        return Err(format!(
            "transition #{index} already has the condition '{name} ({kind})'"
        ));
    }
    conditions[condition]["name"] = json!(name);
    Ok(())
}

#[cfg(test)]
mod state_tests {
    use super::*;
    use serde_json::json;

    fn document() -> Value {
        json!({
            "states": [
                { "name": "Idle", "clip": "idle.ranim" },
                { "name": "Run", "clip": "run.ranim", "speed": 1.2, "looped": true }
            ],
            "transitions": [
                { "from": "Idle", "to": "Run", "duration": 0.2 },
                { "from": "*", "to": "Idle", "duration": 0.0 }
            ]
        })
    }

    #[test]
    fn a_state_clip_speed_and_loop_flag_can_be_edited() {
        let mut doc = document();
        set_state_clip(&mut doc, 0, " idle_alt.ranim ").expect("sets clip");
        set_state_speed(&mut doc, 0, 0.75).expect("sets speed");
        set_state_looped(&mut doc, 1, false).expect("sets loop");
        assert_eq!(doc["states"][0]["clip"], "idle_alt.ranim");
        assert_eq!(doc["states"][0]["speed"].as_f64(), Some(0.75f32 as f64));
        assert_eq!(doc["states"][1]["looped"], false);
    }

    #[test]
    fn bad_state_values_and_indices_are_rejected() {
        let mut doc = document();
        assert!(set_state_clip(&mut doc, 0, "  ").is_err());
        assert!(set_state_clip(&mut doc, 9, "a.ranim").is_err());
        assert!(set_state_speed(&mut doc, 0, -1.0).is_err());
        assert!(set_state_speed(&mut doc, 0, f32::NAN).is_err());
        assert!(set_state_speed(&mut doc, 9, 1.0).is_err());
        assert!(set_state_looped(&mut doc, 9, true).is_err());
        assert!(set_state_clip(&mut json!({ "states": ["bad"] }), 0, "a.ranim").is_err());
    }

    #[test]
    fn states_can_be_reordered_without_touching_transitions() {
        let mut doc = document();
        add_state(&mut doc, "Jump", "jump.ranim").expect("adds");
        let moved = move_state(&mut doc, 2, -2).expect("moves to the front");
        assert_eq!(moved, 0);
        assert_eq!(doc["states"][0]["name"], "Jump");
        assert_eq!(doc["states"][1]["name"], "Idle");
        assert_eq!(doc["states"][2]["name"], "Run");
        assert_eq!(
            doc["transitions"][0]["to"], "Run",
            "transitions reference by name"
        );
        assert_eq!(doc["transitions"][1]["from"], "*");

        let moved = move_state(&mut doc, 0, 1).expect("moves back");
        assert_eq!(moved, 1);
        assert_eq!(doc["states"][0]["name"], "Idle");
        assert_eq!(doc["states"][1]["name"], "Jump");
    }

    #[test]
    fn moving_states_rejects_out_of_range_indices() {
        let mut doc = document();
        assert!(move_state(&mut doc, 0, -1).is_err(), "already at the front");
        assert!(move_state(&mut doc, 1, 1).is_err(), "already at the last");
        assert!(move_state(&mut doc, 9, -1).is_err());
        assert!(move_state(&mut json!({}), 0, 1).is_err());
        assert!(move_state(&mut json!({ "states": ["bad"] }), 0, 1).is_err());
        assert_eq!(
            doc["states"].as_array().unwrap().len(),
            2,
            "failure does not modify document"
        );
    }

    #[test]
    fn states_can_be_added_with_a_name_and_clip() {
        let mut doc = document();
        add_state(&mut doc, " Jump ", " jump.ranim ").expect("adds");
        assert_eq!(
            doc["states"][2],
            json!({ "name": "Jump", "clip": "jump.ranim" })
        );
        assert_eq!(
            doc["transitions"].as_array().unwrap().len(),
            2,
            "transition table unchanged"
        );
    }

    #[test]
    fn adding_states_rejects_blank_duplicate_and_malformed_input() {
        let mut doc = document();
        assert!(add_state(&mut doc, " ", "jump.ranim").is_err());
        assert!(add_state(&mut doc, "Jump", "  ").is_err());
        assert!(add_state(&mut doc, "Idle", "idle.ranim").is_err());
        assert!(add_state(&mut json!({}), "Jump", "jump.ranim").is_err());
        assert!(add_state(&mut json!({ "states": ["bad"] }), "Jump", "jump.ranim").is_err());
        assert_eq!(
            doc["states"].as_array().unwrap().len(),
            2,
            "failure does not modify document"
        );
    }

    #[test]
    fn removing_a_state_drops_transitions_that_reference_it() {
        let mut doc = document();
        add_state_transition(&mut doc, Some("Run"), "Idle").expect("adds");
        let removed = remove_state(&mut doc, "Run").expect("removes");
        assert_eq!(removed, 2);
        assert_eq!(doc["states"].as_array().unwrap().len(), 1);
        assert_eq!(doc["states"][0]["name"], "Idle");
        assert_eq!(
            doc["transitions"],
            json!([{ "from": "*", "to": "Idle", "duration": 0.0 }]),
            "only keep any-state transitions unrelated to Run"
        );
    }

    #[test]
    fn removing_states_rejects_unknown_names_the_last_state_and_bad_transitions() {
        let mut doc = document();
        assert!(remove_state(&mut doc, "Missing").is_err());
        remove_state(&mut doc, "Run").expect("removes");
        assert!(
            remove_state(&mut doc, "Idle").is_err(),
            "can't delete down to zero states"
        );
        assert_eq!(doc["states"].as_array().unwrap().len(), 1);

        let mut doc = json!({
            "states": [
                { "name": "Idle", "clip": "idle.ranim" },
                { "name": "Run", "clip": "run.ranim" }
            ],
            "transitions": "bad"
        });
        assert!(remove_state(&mut doc, "Run").is_err());
        assert_eq!(
            doc["states"].as_array().unwrap().len(),
            2,
            "failure does not modify state table"
        );
    }

    #[test]
    fn renaming_a_state_rewrites_transition_ends() {
        let mut doc = document();
        rename_state(&mut doc, 1, " Sprint ").expect("renames");
        assert_eq!(doc["states"][1]["name"], "Sprint");
        assert_eq!(doc["transitions"][0]["to"], "Sprint");
        assert_eq!(doc["transitions"][1]["from"], "*");
        assert_eq!(doc["transitions"][1]["to"], "Idle");
    }

    #[test]
    fn renaming_rejects_duplicates_blanks_and_unknown_indices() {
        let mut doc = document();
        assert!(rename_state(&mut doc, 0, "Run").is_err());
        assert!(rename_state(&mut doc, 0, " ").is_err());
        assert!(rename_state(&mut doc, 9, "Jump").is_err());
        rename_state(&mut doc, 0, "Idle").expect("same name is a no-op");
        assert_eq!(doc["states"][0]["name"], "Idle");
    }
}

#[cfg(test)]
mod condition_tests {
    use super::*;
    use serde_json::json;

    fn document() -> Value {
        json!({
            "states": [
                { "name": "Idle", "clip": "idle.ranim" },
                { "name": "Run", "clip": "run.ranim" }
            ],
            "transitions": [
                { "from": "Idle", "to": "Run", "duration": 0.2,
                  "conditions": [{ "type": "float_greater", "name": "speed", "threshold": 1.5 }] }
            ]
        })
    }

    #[test]
    fn transition_timing_can_be_edited_and_is_validated() {
        let mut doc = document();
        set_transition_timing(&mut doc, 0, 0.35, 0.5).expect("sets");
        assert_eq!(
            doc["transitions"][0]["duration"].as_f64(),
            Some(0.35f32 as f64)
        );
        assert_eq!(
            doc["transitions"][0]["min_state_fraction"].as_f64(),
            Some(0.5)
        );
        assert!(set_transition_timing(&mut doc, 0, -1.0, 0.0).is_err());
        assert!(set_transition_timing(&mut doc, 0, f32::NAN, 0.0).is_err());
        assert!(set_transition_timing(&mut doc, 0, 0.1, 1.5).is_err());
        assert!(set_transition_timing(&mut doc, 9, 0.1, 0.0).is_err());
        assert!(
            set_transition_timing(&mut json!({ "transitions": ["bad"] }), 0, 0.1, 0.0).is_err()
        );
    }

    #[test]
    fn conditions_can_be_added_and_removed() {
        let mut doc = document();
        add_state_condition(&mut doc, 0, "float_less", "speed", 0.25, false).expect("adds float");
        add_state_condition(&mut doc, 0, "bool_is", "grounded", 0.0, false).expect("adds bool");
        add_state_condition(&mut doc, 0, "trigger", "jump", 0.0, false).expect("adds trigger");
        assert_eq!(
            doc["transitions"][0]["conditions"][1],
            json!({ "type": "float_less", "name": "speed", "threshold": 0.25 })
        );
        assert_eq!(
            doc["transitions"][0]["conditions"][2],
            json!({ "type": "bool_is", "name": "grounded", "value": false })
        );
        assert_eq!(
            doc["transitions"][0]["conditions"][3],
            json!({ "type": "trigger", "name": "jump" })
        );
        remove_state_condition(&mut doc, 0, 0).expect("removes");
        assert_eq!(doc["transitions"][0]["conditions"][0]["name"], "speed");
        assert_eq!(doc["transitions"][0]["conditions"][0]["type"], "float_less");
    }

    #[test]
    fn adding_conditions_creates_the_array_and_rejects_bad_input() {
        let mut doc = json!({
            "states": [{ "name": "Idle", "clip": "idle.ranim" }],
            "transitions": [{ "from": "*", "to": "Idle" }]
        });
        add_state_condition(&mut doc, 0, "trigger", "jump", 0.0, false).expect("adds");
        assert_eq!(
            doc["transitions"][0]["conditions"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let mut doc = document();
        assert!(add_state_condition(&mut doc, 0, "nope", "speed", 0.0, false).is_err());
        assert!(add_state_condition(&mut doc, 0, "trigger", "  ", 0.0, false).is_err());
        assert!(add_state_condition(&mut doc, 0, "float_greater", "speed", 2.0, false).is_err());
        assert!(add_state_condition(&mut doc, 0, "float_less", "speed", f32::NAN, false).is_err());
        assert!(add_state_condition(&mut doc, 9, "trigger", "jump", 0.0, false).is_err());
        assert!(remove_state_condition(&mut doc, 0, 9).is_err());
    }

    #[test]
    fn thresholds_and_bool_values_can_be_edited() {
        let mut doc = document();
        set_condition_threshold(&mut doc, 0, 0, 2.5).expect("sets threshold");
        assert_eq!(doc["transitions"][0]["conditions"][0]["threshold"], 2.5);
        assert!(set_condition_threshold(&mut doc, 0, 0, f32::NAN).is_err());

        add_state_condition(&mut doc, 0, "bool_is", "grounded", 0.0, false).expect("adds bool");
        set_condition_value(&mut doc, 0, 1, true).expect("sets bool");
        assert_eq!(doc["transitions"][0]["conditions"][1]["value"], true);

        assert!(set_condition_value(&mut doc, 0, 0, true).is_err());
        assert!(set_condition_threshold(&mut doc, 0, 1, 0.5).is_err());
        assert!(set_condition_threshold(&mut doc, 0, 9, 0.5).is_err());
        assert!(set_condition_value(&mut doc, 9, 0, true).is_err());
    }

    #[test]
    fn condition_kinds_can_be_switched_and_payloads_are_normalized() {
        let mut doc = document();
        set_condition_kind(&mut doc, 0, 0, "bool_is").expect("switches to bool");
        assert_eq!(
            doc["transitions"][0]["conditions"][0],
            json!({ "type": "bool_is", "name": "speed", "value": false })
        );

        set_condition_value(&mut doc, 0, 0, true).expect("sets bool");
        set_condition_kind(&mut doc, 0, 0, "trigger").expect("switches to trigger");
        assert_eq!(
            doc["transitions"][0]["conditions"][0],
            json!({ "type": "trigger", "name": "speed" })
        );

        set_condition_kind(&mut doc, 0, 0, "float_less").expect("switches back to float");
        assert_eq!(
            doc["transitions"][0]["conditions"][0],
            json!({ "type": "float_less", "name": "speed", "threshold": 0.0 })
        );
        set_condition_threshold(&mut doc, 0, 0, 0.5).expect("threshold works again");
        assert_eq!(doc["transitions"][0]["conditions"][0]["threshold"], 0.5);
    }

    #[test]
    fn switching_condition_kind_rejects_duplicates_and_bad_input() {
        let mut doc = document();
        add_state_condition(&mut doc, 0, "bool_is", "speed", 0.0, false).expect("adds bool");
        assert!(
            set_condition_kind(&mut doc, 0, 0, "bool_is").is_err(),
            "switching duplicates existing condition"
        );
        assert_eq!(
            doc["transitions"][0]["conditions"][0]["type"], "float_greater",
            "failure does not modify document"
        );

        assert!(set_condition_kind(&mut doc, 0, 0, "nope").is_err());
        assert!(set_condition_kind(&mut doc, 9, 0, "trigger").is_err());
        assert!(set_condition_kind(&mut doc, 0, 9, "trigger").is_err());

        let mut blank = json!({
            "transitions": [{ "conditions": [{ "type": "trigger", "name": " " }] }]
        });
        assert!(set_condition_kind(&mut blank, 0, 0, "bool_is").is_err());
        let mut malformed = json!({ "transitions": [{ "conditions": ["bad"] }] });
        assert!(set_condition_kind(&mut malformed, 0, 0, "trigger").is_err());
    }

    #[test]
    fn condition_names_can_be_edited_and_keep_type_and_payload() {
        let mut doc = document();
        set_condition_name(&mut doc, 0, 0, " velocity ").expect("renames");
        assert_eq!(
            doc["transitions"][0]["conditions"][0],
            json!({ "type": "float_greater", "name": "velocity", "threshold": 1.5 })
        );

        add_state_condition(&mut doc, 0, "bool_is", "velocity", 0.0, false).expect("adds bool");
        set_condition_name(&mut doc, 0, 0, "velocity").expect("same name on another type is fine");
        assert_eq!(doc["transitions"][0]["conditions"][1]["type"], "bool_is");
        assert_eq!(doc["transitions"][0]["conditions"][1]["name"], "velocity");
    }

    #[test]
    fn condition_names_reject_duplicates_and_bad_input() {
        let mut doc = document();
        add_state_condition(&mut doc, 0, "float_greater", "speed2", 0.0, false).expect("adds");
        assert!(
            set_condition_name(&mut doc, 0, 0, "speed2").is_err(),
            "duplicate name within same type"
        );
        assert_eq!(doc["transitions"][0]["conditions"][0]["name"], "speed");

        assert!(set_condition_name(&mut doc, 0, 0, "  ").is_err());
        assert!(set_condition_name(&mut doc, 9, 0, "speed").is_err());
        assert!(set_condition_name(&mut doc, 0, 9, "speed").is_err());

        let mut malformed = json!({ "transitions": [{ "conditions": ["bad"] }] });
        assert!(set_condition_name(&mut malformed, 0, 0, "speed").is_err());
        let mut without_type = json!({ "transitions": [{ "conditions": [{ "name": "speed" }] }] });
        assert!(set_condition_name(&mut without_type, 0, 0, "speed").is_err());
    }
}

#[cfg(test)]
mod transition_tests {
    use super::*;

    fn document() -> Value {
        json!({
            "states": [
                { "name": "Idle", "clip": "idle.ranim" },
                { "name": "Run", "clip": "run.ranim" },
                { "name": "Jump", "clip": "jump.ranim" }
            ],
            "transitions": [
                { "from": "Idle", "to": "Run", "duration": 0.2 }
            ]
        })
    }

    #[test]
    fn transitions_can_be_added_from_a_state_or_any_state() {
        let mut doc = document();
        add_state_transition(&mut doc, Some("Run"), "Jump").expect("adds");
        add_state_transition(&mut doc, None, "Idle").expect("adds any");
        assert_eq!(doc["transitions"][1]["from"], "Run");
        assert_eq!(doc["transitions"][2]["from"], "*");
    }

    #[test]
    fn transitions_can_be_removed() {
        let mut doc = document();
        add_state_transition(&mut doc, Some("Run"), "Jump").expect("adds");
        remove_state_transition(&mut doc, 0).expect("removes");
        assert_eq!(doc["transitions"].as_array().unwrap().len(), 1);
        assert_eq!(doc["transitions"][0]["to"], "Jump");
    }

    #[test]
    fn transitions_can_be_duplicated_with_their_conditions() {
        let mut doc = document();
        add_state_transition(&mut doc, Some("Run"), "Jump").expect("adds");
        add_state_condition(&mut doc, 0, "trigger", "jump", 0.0, false).expect("adds condition");
        set_transition_timing(&mut doc, 0, 0.3, 0.5).expect("sets timing");

        let new_index = duplicate_state_transition(&mut doc, 0).expect("duplicates");
        assert_eq!(new_index, 1);
        assert_eq!(doc["transitions"].as_array().unwrap().len(), 3);
        assert_eq!(doc["transitions"][0], doc["transitions"][1]);
        assert_eq!(
            doc["transitions"][2]["to"], "Jump",
            "original second one moved back"
        );
    }

    #[test]
    fn duplicating_transitions_rejects_bad_indices_and_malformed_input() {
        let mut doc = document();
        assert!(duplicate_state_transition(&mut doc, 9).is_err());
        assert!(duplicate_state_transition(&mut json!({}), 0).is_err());
        assert!(duplicate_state_transition(&mut json!({ "transitions": ["bad"] }), 0).is_err());
        assert_eq!(
            doc["transitions"].as_array().unwrap().len(),
            1,
            "failure does not modify document"
        );
    }

    #[test]
    fn bad_transition_names_duplicates_and_self_links_are_rejected() {
        let mut doc = document();
        assert!(add_state_transition(&mut doc, Some("Idle"), "Missing").is_err());
        assert!(add_state_transition(&mut doc, Some("Missing"), "Run").is_err());
        assert!(add_state_transition(&mut doc, Some("Idle"), "Idle").is_err());
        assert!(add_state_transition(&mut doc, Some("Idle"), "Run").is_err());
        assert!(remove_state_transition(&mut doc, 9).is_err());
    }

    #[test]
    fn transitions_can_be_retargeted() {
        let mut doc = document();
        set_transition_states(&mut doc, 0, Some("Run"), "Jump").expect("retargets");
        assert_eq!(doc["transitions"][0]["from"], "Run");
        assert_eq!(doc["transitions"][0]["to"], "Jump");
        assert_eq!(doc["transitions"][0]["duration"].as_f64(), Some(0.2));

        set_transition_states(&mut doc, 0, None, "Idle").expect("any state");
        assert_eq!(doc["transitions"][0]["from"], "*");
        assert_eq!(doc["transitions"][0]["to"], "Idle");
    }

    #[test]
    fn transitions_move_and_clamp_at_the_ends() {
        let mut doc = document();
        add_state_transition(&mut doc, Some("Run"), "Jump").expect("adds");
        let moved = move_state_transition(&mut doc, 1, -1).expect("moves up");
        assert_eq!(moved, 0);
        assert_eq!(doc["transitions"][0]["to"], "Jump");
        assert_eq!(doc["transitions"][1]["to"], "Run");
        let moved = move_state_transition(&mut doc, 0, 1).expect("moves down");
        assert_eq!(moved, 1);
        assert_eq!(doc["transitions"][1]["to"], "Jump");
        assert!(
            move_state_transition(&mut doc, 0, -1).is_err(),
            "already at the front"
        );
        assert!(
            move_state_transition(&mut doc, 1, 1).is_err(),
            "already at the last"
        );
        assert!(move_state_transition(&mut doc, 9, -1).is_err());
    }

    #[test]
    fn retargeting_rejects_bad_names_self_links_and_duplicates() {
        let mut doc = document();
        add_state_transition(&mut doc, Some("Run"), "Jump").expect("adds");
        assert!(set_transition_states(&mut doc, 0, Some("Idle"), "Missing").is_err());
        assert!(set_transition_states(&mut doc, 0, Some("Missing"), "Run").is_err());
        assert!(set_transition_states(&mut doc, 0, Some("Run"), "Run").is_err());
        assert!(set_transition_states(&mut doc, 1, Some("Idle"), "Run").is_err());
        assert!(set_transition_states(&mut doc, 9, None, "Run").is_err());
        set_transition_states(&mut doc, 0, Some("Idle"), "Run").expect("same pair is fine");
        assert_eq!(doc["transitions"][0]["from"], "Idle");
    }
}

#[cfg(test)]
mod weight_tests {
    use super::*;

    fn document() -> Value {
        json!({
            "nodes": [
                { "name": "a", "type": "clip_player", "clip": "a.ranim" },
                { "name": "b", "type": "clip_player", "clip": "b.ranim" },
                { "name": "stack", "type": "blend_stack",
                  "inputs": ["a", { "node": "b", "weight": 0.5, "bone_mask": ["spine"] }] }
            ]
        })
    }

    #[test]
    fn a_string_link_is_upgraded_to_an_object_with_the_new_weight() {
        let mut doc = document();
        set_graph_input_weight(&mut doc, "stack", 0, 0.25).expect("sets");
        assert_eq!(
            doc["nodes"][2]["inputs"][0],
            json!({ "node": "a", "weight": 0.25 })
        );
    }

    #[test]
    fn an_object_link_keeps_its_other_fields() {
        let mut doc = document();
        set_graph_input_weight(&mut doc, "stack", 1, 0.75).expect("sets");
        assert_eq!(doc["nodes"][2]["inputs"][1]["weight"], 0.75);
        assert_eq!(doc["nodes"][2]["inputs"][1]["bone_mask"], json!(["spine"]));
    }

    #[test]
    fn invalid_weights_and_indices_are_rejected() {
        let mut doc = document();
        assert!(set_graph_input_weight(&mut doc, "stack", 0, 2.0).is_err());
        assert!(set_graph_input_weight(&mut doc, "stack", 0, f32::NAN).is_err());
        assert!(set_graph_input_weight(&mut doc, "stack", 9, 0.5).is_err());
        assert!(set_graph_input_weight(&mut doc, "missing", 0, 0.5).is_err());
    }

    #[test]
    fn a_string_link_gains_a_mask_and_upgrades_to_an_object() {
        let mut doc = document();
        set_graph_input_bone_mask(&mut doc, "stack", 0, &[" spine ".to_string()]).expect("sets");
        assert_eq!(
            doc["nodes"][2]["inputs"][0],
            json!({ "node": "a", "weight": 1.0, "bone_mask": ["spine"] })
        );
    }

    #[test]
    fn an_object_link_keeps_its_weight_and_the_mask_is_deduplicated() {
        let mut doc = document();
        let mask = vec![
            "spine".to_string(),
            " arm ".to_string(),
            "spine".to_string(),
            String::new(),
        ];
        set_graph_input_bone_mask(&mut doc, "stack", 1, &mask).expect("sets");
        assert_eq!(doc["nodes"][2]["inputs"][1]["weight"], 0.5);
        assert_eq!(
            doc["nodes"][2]["inputs"][1]["bone_mask"],
            json!(["spine", "arm"])
        );
    }

    #[test]
    fn an_empty_mask_clears_the_field() {
        let mut doc = document();
        set_graph_input_bone_mask(&mut doc, "stack", 1, &[]).expect("clears");
        assert!(doc["nodes"][2]["inputs"][1].get("bone_mask").is_none());
        assert_eq!(doc["nodes"][2]["inputs"][1]["weight"], 0.5);
        set_graph_input_bone_mask(&mut doc, "stack", 0, &[]).expect("no-op");
        assert_eq!(doc["nodes"][2]["inputs"][0], json!("a"));
    }

    #[test]
    fn masks_are_read_back_into_the_view() {
        let doc = json!({
            "nodes": [
                { "name": "a", "type": "clip_player", "clip": "a.ranim" },
                { "name": "stack", "type": "layered_blend",
                  "inputs": [{ "node": "a", "weight": 0.5, "bone_mask": ["spine", "arm"] }] }
            ]
        });
        let view = parse_anim_graph_view(&doc).expect("parses");
        assert_eq!(view.nodes[1].inputs[0].bone_mask, ["spine", "arm"]);
    }

    #[test]
    fn bad_mask_targets_are_rejected() {
        let mut doc = document();
        assert!(set_graph_input_bone_mask(&mut doc, "missing", 0, &["spine".to_string()]).is_err());
        assert!(set_graph_input_bone_mask(&mut doc, "stack", 9, &["spine".to_string()]).is_err());
        assert!(set_graph_input_bone_mask(&mut doc, "out", 0, &["spine".to_string()]).is_err());
        assert!(set_graph_input_bone_mask(
            &mut json!({ "nodes": [{ "name": "a", "inputs": [42] }] }),
            "a",
            0,
            &["spine".to_string()]
        )
        .is_err());
    }
}
