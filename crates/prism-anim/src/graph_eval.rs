//! Animation graph evaluator (#560 P1).
//!
//! [`AnimGraphEvaluator`] evaluates a cooked [`AnimGraphInfo`] (RAGP v1) into a [`Pose`].
//! It does not depend on ECS / rendering / asset loading; the caller provides decoded clips
//! and state machines via [`AnimGraphSource`], then passes in the skeleton (for reference pose
//! and bone name → index).
//!
//! Evaluation is a per-frame directed acyclic graph traversal:
//!
//! - ClipPlayer / BlendSpace / StateMachine each maintain their own time;
//! - BlendStack / LayeredBlend take the first input as base, the rest are stacked in order;
//! - Slot optionally overlays a montage clip on top of the base input (passes through transparently
//!   when there is no clip);
//! - CachedPose / Output are transparent pass-through nodes;
//! - Each node is evaluated at most once per frame (fan-out does not advance time repeatedly);
//! - A cyclic graph (e.g. hand-constructed `AnimGraphInfo`) returns an error instead of stack overflow.
//!
//! State machine nodes maintain "current state + state time + cross-fade snapshot" inside the
//! evaluator. Transition conditions are evaluated via [`StateMachine::next_transition`], Triggers
//! are consumed; when the transition duration is > 0, fade in from the pre-transition pose.
//! This is consistent with the engine `animator` playhead semantics, but this layer does not
//! touch ECS.
//!
//! A Slot's montage can be immediately overridden or queued via
//! [`AnimGraphEvaluator::play_montage`] / [`AnimGraphEvaluator::enqueue_montage`],
//! `stop_montage` / `stop_montage_after_current` clear the corresponding state; Montage Notify
//! events are merged into graph events.
//!
//! Known boundaries: Montage Notify intervals (State Notify) are not yet implemented; CachedPose
//! reuses per-frame cache and does not freeze poses across frames.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use prism_asset::formats_anim_graph::{
    AnimGraphInfo, GraphNodeInfo, GraphNodeKind, GraphParameterKind,
};

use crate::blend::{blend_into, BlendSample, BlendSpace1D};
use crate::blend_space2d::{BlendSample2D, BlendSpace2D};
use crate::blend_stack::{BlendLayer, BlendStack, BoneMask};
use crate::clip::Clip;
use crate::montage::{Montage, MontagePlayer};
use crate::pose::Pose;
use crate::skeleton::Skeleton;
use crate::state_machine::{AnimParameters, StateMachine};

/// Resources needed by the evaluator: look up decoded clips / state machines by path.
pub trait AnimGraphSource {
    fn clip(&self, path: &str) -> Option<Arc<Clip>>;
    fn state_machine(&self, path: &str) -> Option<Arc<StateMachine>>;
}

/// A clip Notify crossed during graph evaluation (#560 P1).
#[derive(Debug, Clone, PartialEq)]
pub struct GraphNotifyEvent {
    /// Name of the graph node that triggered the event.
    pub node: String,
    /// Path of the clip the event belongs to.
    pub clip_path: String,
    pub name: String,
    /// Local time of the event within the clip (seconds).
    pub time: f32,
}

/// Graph evaluation errors. Bad data should not panic, nor leave the host rendering half a pose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphEvalError {
    MissingOutput,
    NodeIndex(u32),
    MissingClip { node: String, path: String },
    MissingStateMachine { node: String, path: String },
    EmptyBlendSpace { node: String },
    EmptyStateMachine { node: String },
    MissingInput { node: String },
    Cycle { node: String },
}

impl std::fmt::Display for GraphEvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingOutput => write!(f, "animation graph has no output node"),
            Self::NodeIndex(index) => {
                write!(f, "animation graph references node #{index} out of range")
            }
            Self::MissingClip { node, path } => {
                write!(f, "node '{node}' cannot find clip '{path}'")
            }
            Self::MissingStateMachine { node, path } => {
                write!(f, "node '{node}' cannot find state machine '{path}'")
            }
            Self::EmptyBlendSpace { node } => {
                write!(f, "node '{node}' has no usable blend-space samples")
            }
            Self::EmptyStateMachine { node } => {
                write!(f, "node '{node}' has an empty state machine")
            }
            Self::MissingInput { node } => write!(f, "node '{node}' has no input"),
            Self::Cycle { node } => write!(f, "animation graph has a cycle at '{node}'"),
        }
    }
}

impl std::error::Error for GraphEvalError {}

#[derive(Debug, Clone, Default)]
struct NodeRuntime {
    time: f32,
    state: usize,
    blend: Option<StateBlend>,
    stamp: u64,
    cached: Pose,
    /// Normalized phase of each sample's independent playhead (used only by BlendSpace nodes with
    /// speeds configured).
    sample_times: Vec<f32>,
    /// Current state name / clip path of state machine nodes (empty for other nodes), for debug snapshots.
    state_name: String,
    state_clip: String,
}

#[derive(Debug, Clone)]
struct StateBlend {
    from: Pose,
    elapsed: f32,
    total: f32,
}

/// Notify policy for BlendSpace nodes (#560).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlendSpaceNotifyPolicy {
    /// No events are produced (old behavior, avoids duplicate footstep sounds from multiple samples).
    None,
    /// Only the currently highest-weight sample produces events; scans a single clip's local time interval.
    #[default]
    Dominant,
}

/// How to handle events leaving a state on the frame a state machine transitions (#560 Notify interval).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StateTransitionNotifyPolicy {
    /// Scan normally up to the transition point (old behavior).
    #[default]
    Keep,
    /// If a transition occurs this frame, do not emit events leaving the state, avoiding footstep
    /// sounds right as the foot is lifted and cut away.
    SuppressOnTransition,
}

/// A runtime summary of a node for this frame ([`AnimGraphEvaluator::node_snapshots`]).
#[derive(Debug, Clone, PartialEq)]
pub struct GraphNodeSnapshot {
    pub name: String,
    pub kind: GraphNodeKind,
    /// Node local time (seconds); for state machine nodes, the current state's own time.
    pub time: f32,
    /// Current state index of the state machine node; 0 for other nodes.
    pub state: usize,
    /// Whether the state machine is currently cross-fading in.
    pub blending: bool,
    /// Cross-fade weight `0..=1` (1 when there is no transition).
    pub blend_weight: f32,
    /// Whether the `cached_pose` node already has a cached pose.
    pub cached: bool,
    /// This node's weight on the final pose `0..=1` (multiplied down from the output node along
    /// connection weights).
    pub weight: f32,
    /// Current state name of the state machine node; empty string for other nodes.
    pub state_name: String,
    /// Clip path of the current state of the state machine node; empty string for other nodes.
    pub state_clip: String,
}

/// A runtime summary of a Slot's Montage ([`AnimGraphEvaluator::montage_snapshots`]).
#[derive(Debug, Clone, PartialEq)]
pub struct MontageSnapshot {
    pub slot: String,
    /// Clip name of the active segment; `None` when there is no active segment.
    pub clip: Option<String>,
    /// Name of the Montage segment at the current time; `None` when there is no segment / not in a segment.
    pub section: Option<String>,
    /// Name of the MotionWarping window at the current time; `None` when not in a window.
    pub warp_window: Option<String>,
    /// Normalized progress within the current warp window.
    pub warp_window_progress: Option<f32>,
    /// Time already played by the active segment (seconds).
    pub time: Option<f32>,
    pub weight: f32,
    pub playing: bool,
    pub paused: bool,
    /// Active segment finished and queue empty.
    pub finished: bool,
    /// Number of queued segments (excluding the active segment).
    pub queued: usize,
}

/// An animation graph evaluator instance, corresponding to one character.
#[derive(Debug, Clone)]
pub struct AnimGraphEvaluator {
    graph: AnimGraphInfo,
    runtime: Vec<NodeRuntime>,
    parameters: AnimParameters,
    frame: u64,
    pending_events: Vec<GraphNotifyEvent>,
    /// Each Slot node's own Montage playback queue (#560 P1).
    montages: HashMap<String, MontagePlayer>,
    /// BlendSpace event policy (#560 Notify interval).
    blend_space_notify: BlendSpaceNotifyPolicy,
    /// State transition frame event policy.
    state_transition_notify: StateTransitionNotifyPolicy,
    /// Per-sample playback speed for BlendSpace by node name (empty = shared phase).
    blend_space_speeds: HashMap<String, Vec<f32>>,
}

impl AnimGraphEvaluator {
    /// Build the evaluator and initialize default values per graph parameter declaration.
    pub fn new(graph: AnimGraphInfo) -> Self {
        let mut parameters = AnimParameters::new();
        for parameter in &graph.parameters {
            match parameter.kind {
                GraphParameterKind::Float => {
                    parameters.set_float(parameter.name.clone(), parameter.default)
                }
                GraphParameterKind::Bool => {
                    parameters.set_bool(parameter.name.clone(), parameter.default != 0.0)
                }
                GraphParameterKind::Trigger => {}
            }
        }
        let runtime = vec![NodeRuntime::default(); graph.nodes.len()];
        Self {
            graph,
            runtime,
            parameters,
            frame: 0,
            pending_events: Vec::new(),
            montages: HashMap::new(),
            blend_space_notify: BlendSpaceNotifyPolicy::default(),
            state_transition_notify: StateTransitionNotifyPolicy::default(),
            blend_space_speeds: HashMap::new(),
        }
    }

    pub fn graph(&self) -> &AnimGraphInfo {
        &self.graph
    }

    pub fn parameters(&self) -> &AnimParameters {
        &self.parameters
    }

    pub fn parameters_mut(&mut self) -> &mut AnimParameters {
        &mut self.parameters
    }

    /// Play a Montage immediately on the Slot node named `slot` (replacing the slot's current segment).
    pub fn play_montage(&mut self, slot: &str, montage: Montage) -> bool {
        self.montages
            .entry(slot.to_string())
            .or_default()
            .play_now(montage)
    }

    /// Queue a Montage on `slot`; starts immediately when idle.
    pub fn enqueue_montage(&mut self, slot: &str, montage: Montage) -> bool {
        self.montages
            .entry(slot.to_string())
            .or_default()
            .enqueue(montage)
    }

    /// Stop the active segment and queue of `slot`.
    pub fn stop_montage(&mut self, slot: &str) {
        if let Some(player) = self.montages.get_mut(slot) {
            player.stop();
        }
    }

    /// Let `slot`'s current segment finish then stop, without taking the next queued segment.
    pub fn stop_montage_after_current(&mut self, slot: &str) {
        if let Some(player) = self.montages.get_mut(slot) {
            player.stop_after_current();
        }
    }

    /// Pause the Slot's active Montage (queue / weight unchanged); return `false` if the Slot does not exist.
    pub fn pause_montage(&mut self, slot: &str) -> bool {
        match self.montages.get_mut(slot) {
            Some(player) => {
                player.set_paused(true);
                true
            }
            None => false,
        }
    }

    /// Resume the Slot's active Montage; return `false` if the Slot does not exist.
    pub fn resume_montage(&mut self, slot: &str) -> bool {
        match self.montages.get_mut(slot) {
            Some(player) => {
                player.set_paused(false);
                true
            }
            None => false,
        }
    }

    pub fn montage_player(&self, slot: &str) -> Option<&MontagePlayer> {
        self.montages.get(slot)
    }

    /// Set the BlendSpace Notify policy; `None` restores the old behavior.
    pub fn set_blend_space_notify_policy(&mut self, policy: BlendSpaceNotifyPolicy) {
        self.blend_space_notify = policy;
    }

    pub fn blend_space_notify_policy(&self) -> BlendSpaceNotifyPolicy {
        self.blend_space_notify
    }

    /// Set the state transition frame event policy.
    pub fn set_state_transition_notify_policy(&mut self, policy: StateTransitionNotifyPolicy) {
        self.state_transition_notify = policy;
    }

    pub fn state_transition_notify_policy(&self) -> StateTransitionNotifyPolicy {
        self.state_transition_notify
    }

    /// Configure per-sample playback speed for a BlendSpace node (in `node.clips` order).
    ///
    /// After configuration, the node maintains an independent playhead per sample, no longer
    /// sharing the global phase nor doing SyncGroup mapping; an empty list / all-1 is equivalent
    /// to the default shared phase.
    pub fn set_blend_space_speeds(&mut self, node: &str, speeds: Vec<f32>) {
        if speeds.is_empty() || speeds.iter().all(|speed| (*speed - 1.0).abs() < 1e-6) {
            self.blend_space_speeds.remove(node);
        } else {
            self.blend_space_speeds.insert(node.to_string(), speeds);
        }
    }

    pub fn blend_space_speeds(&self, node: &str) -> Option<&[f32]> {
        self.blend_space_speeds.get(node).map(Vec::as_slice)
    }

    /// Runtime summary of each node this frame (for debug panels / overlays).
    ///
    /// The return order matches the node order in the graph; `kind` lets the caller decide how
    /// to display, `time` is the node's own local time (the current state's time for state
    /// machine nodes), `blending` / `blend_weight` reflect whether the state machine is
    /// cross-fading in. Only scalars are copied; evaluator state is not modified.
    pub fn node_snapshots(&self) -> Vec<GraphNodeSnapshot> {
        let weights = self.node_weights();
        self.graph
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                let runtime = &self.runtime[index];
                let (blending, blend_weight) = match &runtime.blend {
                    Some(blend) if blend.total > 0.0 => {
                        (true, (blend.elapsed / blend.total).clamp(0.0, 1.0))
                    }
                    Some(_) => (true, 1.0),
                    None => (false, 1.0),
                };
                GraphNodeSnapshot {
                    name: node.name.clone(),
                    kind: node.kind,
                    time: runtime.time,
                    state: runtime.state,
                    blending,
                    blend_weight,
                    cached: !runtime.cached.is_empty(),
                    weight: weights[index],
                    state_name: runtime.state_name.clone(),
                    state_clip: runtime.state_clip.clone(),
                }
            })
            .collect()
    }

    /// From the output node downward, gives each node's contribution to the final pose (`0..=1`)
    /// by connection weight.
    ///
    /// Shared nodes take the maximum along the path (when one node is referenced by two edges,
    /// the one with the larger weight counts); nodes not connected to the output node are 0.
    /// BlendStack / LayeredBlend connection weights are the layer weights; other nodes are
    /// usually 1 — for debug display only, not used in evaluation.
    fn node_weights(&self) -> Vec<f32> {
        let mut weights = vec![0.0f32; self.graph.nodes.len()];
        let mut seen = vec![false; self.graph.nodes.len()];
        self.collect_weights(self.graph.output, 1.0, &mut weights, &mut seen);
        weights
    }

    fn collect_weights(&self, index: u32, weight: f32, weights: &mut [f32], seen: &mut [bool]) {
        let index = index as usize;
        if index >= self.graph.nodes.len() {
            return;
        }
        if weight > weights[index] {
            weights[index] = weight;
        }
        if seen[index] {
            return;
        }
        seen[index] = true;
        for input in &self.graph.nodes[index].inputs {
            let weight = (weight * input.weight).clamp(0.0, 1.0);
            self.collect_weights(input.node, weight, weights, seen);
        }
    }

    /// Runtime summary of each Slot's Montage (sorted by Slot name, for debug panels).
    ///
    /// Only includes Slots that have been `play` / `enqueue`d; when the active segment finished
    /// and the queue is empty, `playing = false`, `finished = true`.
    pub fn montage_snapshots(&self) -> Vec<MontageSnapshot> {
        let mut slots: Vec<(&String, &MontagePlayer)> = self.montages.iter().collect();
        slots.sort_by(|left, right| left.0.cmp(right.0));
        slots
            .into_iter()
            .map(|(slot, player)| MontageSnapshot {
                slot: slot.clone(),
                clip: player.active_clip().map(|clip| clip.name.clone()),
                section: player.active_section().map(|section| section.name.clone()),
                warp_window: player
                    .active_warp_window()
                    .map(|window| window.name.clone()),
                warp_window_progress: player.warp_window_progress(),
                time: player.active_time(),
                weight: player.active_weight(),
                playing: player.is_playing(),
                paused: player.is_paused(),
                finished: player.is_finished(),
                queued: player.queue_len(),
            })
            .collect()
    }

    /// Current state index of the state machine node (for debug panels / tests); returns `None`
    /// when not a state machine node.
    pub fn state_machine_state(&self, name: &str) -> Option<usize> {
        let index = self.graph.nodes.iter().position(|node| node.name == name)?;
        (self.graph.nodes[index].kind == GraphNodeKind::StateMachine)
            .then_some(self.runtime[index].state)
    }

    /// Evaluate one frame, returning the output node's pose.
    ///
    /// `dt` non-finite or negative is treated as 0; missing resources return an error. On failure,
    /// the evaluator's cache and state may have partially advanced; the caller should keep the
    /// previous frame's pose instead of showing half a result.
    pub fn evaluate(
        &mut self,
        source: &impl AnimGraphSource,
        skeleton: &Skeleton,
        dt: f32,
    ) -> Result<Pose, GraphEvalError> {
        self.evaluate_with_events(source, skeleton, dt)
            .map(|(pose, _)| pose)
    }

    /// Evaluate one frame and return the Notify events crossed this frame.
    ///
    /// Only ClipPlayer / StateMachine nodes produce events; BlendSpace samples multiple clips
    /// at once, and repeatedly firing events would amplify one footstep into several, left for
    /// later "dominant sample" policies. The first frame does not re-fire time = 0 events;
    /// loop wrap-around scans in two segments.
    pub fn evaluate_with_events(
        &mut self,
        source: &impl AnimGraphSource,
        skeleton: &Skeleton,
        dt: f32,
    ) -> Result<(Pose, Vec<GraphNotifyEvent>), GraphEvalError> {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        self.frame = self.frame.wrapping_add(1);
        self.pending_events.clear();
        let reference = skeleton.rest_pose();
        let mut visiting = vec![false; self.graph.nodes.len()];
        let pose = self.evaluate_node(
            self.graph.output,
            source,
            skeleton,
            &reference,
            dt,
            &mut visiting,
            &mut HashSet::new(),
        )?;
        Ok((pose, std::mem::take(&mut self.pending_events)))
    }

    fn record_notifies(
        &mut self,
        node: &str,
        clip_path: &str,
        clip: &Clip,
        previous_local: f32,
        current_local: f32,
    ) {
        for notify in clip.notifies_between(previous_local, current_local) {
            self.pending_events.push(GraphNotifyEvent {
                node: node.to_string(),
                clip_path: clip_path.to_string(),
                name: notify.name.clone(),
                time: notify.time,
            });
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "arity mirrors the float-parameter packing that the callers already flatten"
    )]
    fn evaluate_node(
        &mut self,
        index: u32,
        source: &impl AnimGraphSource,
        skeleton: &Skeleton,
        reference: &Pose,
        dt: f32,
        visiting: &mut [bool],
        stack: &mut HashSet<u32>,
    ) -> Result<Pose, GraphEvalError> {
        let index_usize = index as usize;
        if index_usize >= self.graph.nodes.len() {
            return Err(GraphEvalError::NodeIndex(index));
        }
        if self.runtime[index_usize].stamp == self.frame {
            return Ok(self.runtime[index_usize].cached.clone());
        }
        if !stack.insert(index) || visiting[index_usize] {
            return Err(GraphEvalError::Cycle {
                node: self.graph.nodes[index_usize].name.clone(),
            });
        }
        visiting[index_usize] = true;

        let node = self.graph.nodes[index_usize].clone();
        let pose = match node.kind {
            GraphNodeKind::ClipPlayer => {
                self.evaluate_clip(index_usize, &node, source, reference, dt)?
            }
            GraphNodeKind::BlendSpace1D | GraphNodeKind::BlendSpace2D => {
                self.evaluate_blend_space(index_usize, &node, source, reference, dt)?
            }
            GraphNodeKind::BlendStack | GraphNodeKind::LayeredBlend => {
                self.evaluate_stack(&node, source, skeleton, reference, dt, visiting, stack)?
            }
            GraphNodeKind::Slot => self.evaluate_slot(
                index_usize,
                &node,
                source,
                skeleton,
                reference,
                dt,
                visiting,
                stack,
            )?,
            GraphNodeKind::CachedPose | GraphNodeKind::Output => {
                let Some(input) = node.inputs.first() else {
                    return Err(GraphEvalError::MissingInput {
                        node: node.name.clone(),
                    });
                };
                self.evaluate_node(input.node, source, skeleton, reference, dt, visiting, stack)?
            }
            GraphNodeKind::StateMachine => {
                self.evaluate_state_machine(index_usize, &node, source, reference, dt)?
            }
        };

        visiting[index_usize] = false;
        stack.remove(&index);
        self.runtime[index_usize].stamp = self.frame;
        self.runtime[index_usize].cached = pose.clone();
        Ok(pose)
    }

    fn evaluate_clip(
        &mut self,
        index: usize,
        node: &GraphNodeInfo,
        source: &impl AnimGraphSource,
        reference: &Pose,
        dt: f32,
    ) -> Result<Pose, GraphEvalError> {
        let path = node
            .clips
            .first()
            .ok_or_else(|| GraphEvalError::MissingClip {
                node: node.name.clone(),
                path: String::new(),
            })?;
        let clip = source
            .clip(path)
            .ok_or_else(|| GraphEvalError::MissingClip {
                node: node.name.clone(),
                path: path.clone(),
            })?;
        let previous_time = self.runtime[index].time;
        let time = advance_time(previous_time, dt, node.speed);
        self.runtime[index].time = time;
        let previous_local = local_time(clip.duration, node.looped, previous_time);
        let local = local_time(clip.duration, node.looped, time);
        if dt > 0.0 && node.speed >= 0.0 {
            self.record_notifies(&node.name, path, &clip, previous_local, local);
        }
        let mut pose = reference.clone();
        clip.sample_into(local, &mut pose);
        Ok(pose)
    }

    fn evaluate_blend_space(
        &mut self,
        index: usize,
        node: &GraphNodeInfo,
        source: &impl AnimGraphSource,
        reference: &Pose,
        dt: f32,
    ) -> Result<Pose, GraphEvalError> {
        let previous_time = self.runtime[index].time;
        let duration = node
            .clips
            .iter()
            .filter_map(|path| source.clip(path).map(|clip| clip.duration))
            .fold(0.0f32, f32::max);
        let previous_phase = if duration > 0.0 {
            previous_time.rem_euclid(duration) / duration
        } else {
            0.0
        };
        let time = advance_time(previous_time, dt, node.speed);
        self.runtime[index].time = time;
        let phase = if duration > 0.0 {
            time.rem_euclid(duration) / duration
        } else {
            0.0
        };

        let speeds = self
            .blend_space_speeds
            .get(&node.name)
            .cloned()
            .or_else(|| (!node.sample_speeds.is_empty()).then(|| node.sample_speeds.clone()));
        let (previous_sample_phases, sample_phases) = if let Some(speeds) = speeds {
            let runtime = &mut self.runtime[index];
            if runtime.sample_times.len() != node.clips.len() {
                runtime.sample_times = vec![0.0; node.clips.len()];
            }
            let previous = runtime.sample_times.clone();
            for (sample, time) in runtime.sample_times.iter_mut().enumerate() {
                let speed = speeds
                    .get(sample)
                    .copied()
                    .filter(|speed| speed.is_finite())
                    .unwrap_or(1.0);
                *time = (*time + dt * speed).rem_euclid(1.0);
            }
            (Some(previous), Some(runtime.sample_times.clone()))
        } else {
            (None, None)
        };

        let mut pose = reference.clone();
        if node.kind == GraphNodeKind::BlendSpace1D {
            let samples = node
                .clips
                .iter()
                .enumerate()
                .map(|(sample, path)| {
                    let clip = source
                        .clip(path)
                        .ok_or_else(|| GraphEvalError::MissingClip {
                            node: node.name.clone(),
                            path: path.clone(),
                        })?;
                    Ok(BlendSample {
                        position: node
                            .positions
                            .get(sample)
                            .map_or(0.0, |position| position[0]),
                        clip,
                    })
                })
                .collect::<Result<Vec<_>, GraphEvalError>>()?;
            let parameter = self.parameters.float(&node.parameter_x).unwrap_or(0.0);
            let mut space = BlendSpace1D::new(samples, reference.clone());
            let clips: Vec<Arc<Clip>> = space
                .samples()
                .iter()
                .map(|sample| sample.clip.clone())
                .collect();
            let ordered = sample_phases
                .as_ref()
                .map(|phases| self.phases_in_space_order(&clips, node, source, phases));
            let ordered_previous = previous_sample_phases
                .as_ref()
                .map(|phases| self.phases_in_space_order(&clips, node, source, phases));
            let evaluated = match ordered.as_ref() {
                Some(phases) => space.evaluate_into_with_phases(parameter, phases, &mut pose),
                None => space.evaluate_into(parameter, phase, &mut pose),
            };
            if !evaluated {
                return Err(GraphEvalError::EmptyBlendSpace {
                    node: node.name.clone(),
                });
            }
            let dominant_index = space.dominant_sample(parameter);
            let dominant = if self.blend_space_notify == BlendSpaceNotifyPolicy::Dominant {
                dominant_index
                    .and_then(|sample| space.samples().get(sample))
                    .map(|sample| sample.clip.clone())
            } else {
                None
            };
            let (previous_phase, phase) = match (&ordered_previous, &ordered, dominant_index) {
                (Some(previous), Some(current), Some(index)) => (
                    previous.get(index).copied().unwrap_or(previous_phase),
                    current.get(index).copied().unwrap_or(phase),
                ),
                _ => (previous_phase, phase),
            };
            self.record_blend_space_notifies(
                &node.name,
                node,
                source,
                dominant,
                previous_phase,
                phase,
                dt,
            );
        } else {
            let samples = node
                .clips
                .iter()
                .enumerate()
                .map(|(sample, path)| {
                    let clip = source
                        .clip(path)
                        .ok_or_else(|| GraphEvalError::MissingClip {
                            node: node.name.clone(),
                            path: path.clone(),
                        })?;
                    Ok(BlendSample2D {
                        position: node.positions.get(sample).copied().unwrap_or([0.0, 0.0]),
                        clip,
                    })
                })
                .collect::<Result<Vec<_>, GraphEvalError>>()?;
            let at = [
                self.parameters.float(&node.parameter_x).unwrap_or(0.0),
                self.parameters.float(&node.parameter_y).unwrap_or(0.0),
            ];
            let mut space = BlendSpace2D::new(samples, reference.clone());
            let clips: Vec<Arc<Clip>> = space
                .samples()
                .iter()
                .map(|sample| sample.clip.clone())
                .collect();
            let ordered = sample_phases
                .as_ref()
                .map(|phases| self.phases_in_space_order(&clips, node, source, phases));
            let ordered_previous = previous_sample_phases
                .as_ref()
                .map(|phases| self.phases_in_space_order(&clips, node, source, phases));
            let evaluated = match ordered.as_ref() {
                Some(phases) => space.evaluate_into_with_phases(at, phases, &mut pose),
                None => space.evaluate_into(at, phase, &mut pose),
            };
            if !evaluated {
                return Err(GraphEvalError::EmptyBlendSpace {
                    node: node.name.clone(),
                });
            }
            let dominant_index = space.dominant_sample(at);
            let dominant = if self.blend_space_notify == BlendSpaceNotifyPolicy::Dominant {
                dominant_index
                    .and_then(|sample| space.samples().get(sample))
                    .map(|sample| sample.clip.clone())
            } else {
                None
            };
            let (previous_phase, phase) = match (&ordered_previous, &ordered, dominant_index) {
                (Some(previous), Some(current), Some(index)) => (
                    previous.get(index).copied().unwrap_or(previous_phase),
                    current.get(index).copied().unwrap_or(phase),
                ),
                _ => (previous_phase, phase),
            };
            self.record_blend_space_notifies(
                &node.name,
                node,
                source,
                dominant,
                previous_phase,
                phase,
                dt,
            );
        }
        Ok(pose)
    }

    /// Let the highest-weight BlendSpace sample fire Notify over its own local time interval.
    #[expect(
        clippy::too_many_arguments,
        reason = "arity mirrors the float-parameter packing that the callers already flatten"
    )]
    fn record_blend_space_notifies(
        &mut self,
        node_name: &str,
        node: &GraphNodeInfo,
        source: &impl AnimGraphSource,
        clip: Option<Arc<Clip>>,
        previous_phase: f32,
        phase: f32,
        dt: f32,
    ) {
        if dt <= 0.0 || node.speed < 0.0 {
            return;
        }
        let Some(clip) = clip else {
            return;
        };
        let Some(path) = node.clips.iter().find(|path| {
            source
                .clip(path)
                .is_some_and(|candidate| Arc::ptr_eq(&candidate, &clip))
        }) else {
            return;
        };
        let from = previous_phase * clip.duration;
        let to = phase * clip.duration;
        self.record_notifies(node_name, path, &clip, from, to);
    }

    /// Reorder phases in `node.clips` order into BlendSpace's internal sample order.
    fn phases_in_space_order(
        &self,
        samples: &[Arc<Clip>],
        node: &GraphNodeInfo,
        source: &impl AnimGraphSource,
        phases: &[f32],
    ) -> Vec<f32> {
        samples
            .iter()
            .map(|clip| {
                node.clips
                    .iter()
                    .position(|path| {
                        source
                            .clip(path)
                            .is_some_and(|candidate| Arc::ptr_eq(&candidate, clip))
                    })
                    .and_then(|index| phases.get(index))
                    .copied()
                    .unwrap_or(0.0)
            })
            .collect()
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "arity mirrors the float-parameter packing that the callers already flatten"
    )]
    fn evaluate_slot(
        &mut self,
        index: usize,
        node: &GraphNodeInfo,
        source: &impl AnimGraphSource,
        skeleton: &Skeleton,
        reference: &Pose,
        dt: f32,
        visiting: &mut [bool],
        stack: &mut HashSet<u32>,
    ) -> Result<Pose, GraphEvalError> {
        let Some(input) = node.inputs.first() else {
            return Err(GraphEvalError::MissingInput {
                node: node.name.clone(),
            });
        };
        let base =
            self.evaluate_node(input.node, source, skeleton, reference, dt, visiting, stack)?;
        let mut montage_events = Vec::new();
        let mut montage_pose = None;
        if let Some(player) = self.montages.get_mut(&node.name) {
            if player.is_playing() {
                player.tick(dt);
                montage_events = player.take_notifies();
                let mut overlay = reference.clone();
                if player.sample_into(reference, &mut overlay) {
                    montage_pose = Some((overlay, player.active_weight()));
                }
            }
        }
        for event in montage_events {
            self.pending_events.push(GraphNotifyEvent {
                node: node.name.clone(),
                clip_path: event.clip_name,
                name: event.name,
                time: event.time,
            });
        }
        if let Some((overlay, weight)) = montage_pose {
            if weight >= 1.0 {
                return Ok(overlay);
            }
            if weight <= 0.0 {
                return Ok(base);
            }
            let mut blended = Pose::default();
            blend_into(&mut blended, &base, &overlay, weight);
            return Ok(blended);
        }

        let Some(path) = node.clips.first().filter(|path| !path.trim().is_empty()) else {
            return Ok(base);
        };
        let weight = if node.weight.is_finite() {
            node.weight.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if weight <= 0.0 {
            return Ok(base);
        }
        let clip = source
            .clip(path)
            .ok_or_else(|| GraphEvalError::MissingClip {
                node: node.name.clone(),
                path: path.clone(),
            })?;
        let previous_time = self.runtime[index].time;
        let time = advance_time(previous_time, dt, node.speed);
        self.runtime[index].time = time;
        let previous_local = local_time(clip.duration, node.looped, previous_time);
        let local = local_time(clip.duration, node.looped, time);
        if dt > 0.0 && node.speed >= 0.0 {
            self.record_notifies(&node.name, path, &clip, previous_local, local);
        }
        let mut overlay = reference.clone();
        clip.sample_into(local, &mut overlay);
        if weight >= 1.0 {
            return Ok(overlay);
        }
        let mut blended = Pose::default();
        blend_into(&mut blended, &base, &overlay, weight);
        Ok(blended)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "arity mirrors the float-parameter packing that the callers already flatten"
    )]
    fn evaluate_stack(
        &mut self,
        node: &GraphNodeInfo,
        source: &impl AnimGraphSource,
        skeleton: &Skeleton,
        reference: &Pose,
        dt: f32,
        visiting: &mut [bool],
        stack: &mut HashSet<u32>,
    ) -> Result<Pose, GraphEvalError> {
        let Some(first) = node.inputs.first() else {
            return Err(GraphEvalError::MissingInput {
                node: node.name.clone(),
            });
        };
        let base =
            self.evaluate_node(first.node, source, skeleton, reference, dt, visiting, stack)?;
        if node.inputs.len() == 1 {
            return Ok(base);
        }
        let mut blend_stack = BlendStack::new(base, reference.clone());
        for input in node.inputs.iter().skip(1) {
            let layer_pose =
                self.evaluate_node(input.node, source, skeleton, reference, dt, visiting, stack)?;
            let mask = if input.bone_mask.is_empty() {
                BoneMask::all()
            } else {
                let names: Vec<&str> = input.bone_mask.iter().map(String::as_str).collect();
                BoneMask::from_names(skeleton, &names)
            };
            blend_stack.push(
                BlendLayer::new(layer_pose)
                    .with_weight(input.weight)
                    .with_mask(mask),
            );
        }
        let mut out = Pose::default();
        blend_stack.evaluate_into(&mut out);
        Ok(out)
    }

    fn evaluate_state_machine(
        &mut self,
        index: usize,
        node: &GraphNodeInfo,
        source: &impl AnimGraphSource,
        reference: &Pose,
        dt: f32,
    ) -> Result<Pose, GraphEvalError> {
        let machine = source
            .state_machine(&node.state_machine_path)
            .ok_or_else(|| GraphEvalError::MissingStateMachine {
                node: node.name.clone(),
                path: node.state_machine_path.clone(),
            })?;
        if machine.states.is_empty() {
            return Err(GraphEvalError::EmptyStateMachine {
                node: node.name.clone(),
            });
        }

        let previous_state = self.runtime[index].state.min(machine.states.len() - 1);
        let previous_time = self.runtime[index].time;
        let mut state = previous_state;
        let mut state_time = self.runtime[index].time;
        let current = &machine.states[state];
        let current_clip =
            if current.clip_path.trim().is_empty() {
                None
            } else {
                Some(source.clip(&current.clip_path).ok_or_else(|| {
                    GraphEvalError::MissingClip {
                        node: node.name.clone(),
                        path: current.clip_path.clone(),
                    }
                })?)
            };
        let current_duration = current_clip.as_ref().map_or(0.0, |clip| clip.duration);
        let current_looped = current.looped;
        let speed = sanitize_speed(current.speed);
        let previous_local = local_time(current_duration, current_looped, previous_time);
        state_time = advance_time(state_time, dt, speed);
        let active_local = local_time(current_duration, current_looped, state_time);
        let transition =
            machine.next_transition(state, &mut self.parameters, state_time, current_duration);
        let suppress = transition.is_some()
            && self.state_transition_notify == StateTransitionNotifyPolicy::SuppressOnTransition;
        if dt > 0.0 && speed >= 0.0 && !suppress {
            if let Some(clip) = current_clip.as_ref() {
                self.record_notifies(
                    &node.name,
                    &current.clip_path,
                    clip,
                    previous_local,
                    active_local,
                );
            }
        }

        let mut previous = None;
        if let Some(target) = transition.filter(|target| target.state < machine.states.len()) {
            let from_clip = current_clip.as_ref();
            if let Some(clip) = from_clip {
                let mut pose = reference.clone();
                let local = local_time(clip.duration, current.looped, state_time);
                clip.sample_into(local, &mut pose);
                previous = Some(pose);
            }
            state = target.state;
            state_time = 0.0;
            self.runtime[index].blend = Some(StateBlend {
                from: previous.clone().unwrap_or_else(|| reference.clone()),
                elapsed: 0.0,
                total: target.duration.max(0.0),
            });
        }

        let state_info = &machine.states[state];
        let clip =
            if state_info.clip_path.trim().is_empty() {
                None
            } else {
                Some(source.clip(&state_info.clip_path).ok_or_else(|| {
                    GraphEvalError::MissingClip {
                        node: node.name.clone(),
                        path: state_info.clip_path.clone(),
                    }
                })?)
            };
        let mut pose = reference.clone();
        if let Some(clip) = clip {
            let local = local_time(clip.duration, state_info.looped, state_time);
            clip.sample_into(local, &mut pose);
        }

        if let Some(blend) = self.runtime[index].blend.as_mut() {
            blend.elapsed += dt;
            if blend.total > 0.0 {
                let weight = (blend.elapsed / blend.total).clamp(0.0, 1.0);
                let mut blended = reference.clone();
                blend_into(&mut blended, &blend.from, &pose, weight);
                pose = blended;
                if weight >= 1.0 {
                    self.runtime[index].blend = None;
                }
            } else {
                self.runtime[index].blend = None;
            }
        }

        let runtime = &mut self.runtime[index];
        runtime.state = state;
        runtime.time = state_time;
        if runtime.state_name != state_info.name {
            runtime.state_name = state_info.name.clone();
        }
        if runtime.state_clip != state_info.clip_path {
            runtime.state_clip = state_info.clip_path.clone();
        }
        Ok(pose)
    }
}

fn advance_time(time: f32, dt: f32, speed: f32) -> f32 {
    let speed = sanitize_speed(speed);
    let next = time + dt * speed;
    if next.is_finite() {
        next
    } else {
        time
    }
}

fn sanitize_speed(speed: f32) -> f32 {
    if speed.is_finite() {
        speed
    } else {
        1.0
    }
}

fn local_time(duration: f32, looped: bool, time: f32) -> f32 {
    if duration <= 0.0 {
        return 0.0;
    }
    if looped {
        time.rem_euclid(duration)
    } else {
        time.clamp(0.0, duration)
    }
}

#[cfg(test)]
#[path = "graph_eval_tests.rs"]
mod tests;
