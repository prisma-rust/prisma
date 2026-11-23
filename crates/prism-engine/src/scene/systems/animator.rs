//! Skeletal animation sampling (#560 P0-2).
//!
//! Data stream: `SkeletonRoot.skeleton_path` + `Animator.clip_path` → already decoded
//! [`prism_anim::Skeleton`] / [`prism_anim::Clip`] (placed in [`SkeletonLibrary`] resource inside)
//! → sampled into `Pose` → `bone_palette` → [`BonePalette`].
//!
//! Resource loading step from `. pak` inside happens in `asset_resolver::load_animation_assets`
//! (needs `&mut World` and `ResourceManager`, so not done in degrees system inside).
//!
//! This layer still does not render: GPU skinning in P0-3, at that time `BonePalette::matrices` directly uploaded.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use prism_anim::{
    palette_to_arrays, AnimGraphEvaluator, AnimGraphSource, AnimParameters, BlendLayer,
    BlendSample, BlendSample2D, BlendSpace1D, BlendSpace2D, BlendStack, BoneMask, Clip,
    GraphEvalError, GraphNodeSnapshot, Montage, MontageSnapshot, Pose, Skeleton, StateMachine,
};
use prism_asset::formats_anim_graph::AnimGraphInfo;
use prism_ecs::{Entity, World};
use prism_scene::components::{Animator, Camera, LocalToWorld, LocalTransform, SkeletonRoot};

/// Already decoded skeletons and clips, indexed by asset path (same as `MeshBounds`, is World resource).
#[derive(Debug, Default)]
pub struct SkeletonLibrary {
    skeletons: HashMap<String, Arc<Skeleton>>,
    clips: HashMap<String, Arc<Clip>>,
}

impl SkeletonLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert skeleton; returns `true` indicating this is new path.
    pub fn insert_skeleton(&mut self, path: impl Into<String>, skeleton: Skeleton) -> bool {
        self.skeletons
            .insert(path.into(), Arc::new(skeleton))
            .is_none()
    }

    pub fn insert_clip(&mut self, path: impl Into<String>, clip: Clip) -> bool {
        self.clips.insert(path.into(), Arc::new(clip)).is_none()
    }

    pub fn skeleton(&self, path: &str) -> Option<Arc<Skeleton>> {
        self.skeletons.get(path).cloned()
    }

    pub fn clip(&self, path: &str) -> Option<Arc<Clip>> {
        self.clips.get(path).cloned()
    }

    /// Clips inside Notify (time + name, sorted by time); paths not loaded return empty.
    pub fn clip_notifies(&self, path: &str) -> Vec<(f32, String)> {
        self.clips
            .get(path)
            .map(|clip| {
                clip.notifies
                    .iter()
                    .map(|notify| (notify.time, notify.name.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Clip key frame time points appearing inside (sorted ascending, deduplicated); paths not loaded return empty.
    pub fn clip_key_times(&self, path: &str) -> Vec<f32> {
        self.clips
            .get(path)
            .map(|clip| clip.key_times())
            .unwrap_or_default()
    }

    pub fn has_skeleton(&self, path: &str) -> bool {
        self.skeletons.contains_key(path)
    }

    pub fn has_clip(&self, path: &str) -> bool {
        self.clips.contains_key(path)
    }

    pub fn skeleton_count(&self) -> usize {
        self.skeletons.len()
    }

    pub fn clip_count(&self) -> usize {
        self.clips.len()
    }

    pub fn clear(&mut self) {
        self.skeletons.clear();
        self.clips.clear();
    }
}

/// Bone matrices computed this frame (column-major, P0-3 directly writes to storage buffer).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BonePalette {
    pub matrices: Vec<[[f32; 4]; 4]>,
}

impl BonePalette {
    pub fn from_matrices(palette: &[glam::Mat4]) -> Self {
        Self {
            matrices: palette_to_arrays(palette),
        }
    }

    pub fn len(&self) -> usize {
        self.matrices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.matrices.is_empty()
    }
}

/// Close key frame event triggered this frame (#560 P1 Notify).
#[derive(Debug, Clone, PartialEq)]
pub struct AnimEvent {
    pub entity: Entity,
    /// Event name, interpreted by project (`Footstep.L` / `Traversal.Window` / …).
    pub name: String,
    /// Event time inside clip (seconds).
    pub time: f32,
}

/// Per-entity animation graph node runtime summary (World resource, #560 real-time debug).
///
/// **opt-in**: engine only heavy-builds it every frame when World has this resource; if not, does not do extra char copy outside.
/// Key is entity, value is [`GraphNodeSnapshot`] list (order consistent with graph node). Frozen / throttled
/// entity retains one fast snapshot, convenient for frontend display "stopped at where".
#[derive(Debug, Clone, Default)]
pub struct AnimGraphDebug {
    pub entities: HashMap<Entity, Vec<GraphNodeSnapshot>>,
    /// Per-graph-entity Slot Montage summary (sorted by Slot name).
    pub montages: HashMap<Entity, Vec<MontageSnapshot>>,
    /// Per-graph-entity current graph asset path (frontend filters by asset).
    pub paths: HashMap<Entity, String>,
}

impl AnimGraphDebug {
    /// Per-entity node fast snapshot; returns `None` if not a graph entity.
    pub fn states(&self, entity: Entity) -> Option<&[GraphNodeSnapshot]> {
        self.entities.get(&entity).map(Vec::as_slice)
    }

    /// Per-entity Slot Montage summary; empty list when no Montage played.
    pub fn montages(&self, entity: Entity) -> Option<&[MontageSnapshot]> {
        self.montages.get(&entity).map(Vec::as_slice)
    }

    /// Per-graph-entity current graph asset path.
    pub fn path(&self, entity: Entity) -> Option<&str> {
        self.paths.get(&entity).map(String::as_str)
    }
}

/// Events triggered by one `animator_system` call (World resource).
///
/// Each tick **entirely heavy-built** (not accumulated): consumer reads after `update` always only this frame
/// crossed time points, will not replay footsteps duplicated within one frame. To cross-frame cache, `drain` yourself.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnimEvents {
    pub events: Vec<AnimEvent>,
}

impl AnimEvents {
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Take this frame's events (resource itself remains empty).
    pub fn drain(&mut self) -> Vec<AnimEvent> {
        std::mem::take(&mut self.events)
    }

    /// Event names triggered by some entity this frame.
    pub fn names_for(&self, entity: Entity) -> impl Iterator<Item = &str> {
        self.events
            .iter()
            .filter(move |event| event.entity == entity)
            .map(|event| event.name.as_str())
    }
}

/// Already decoded state machines, indexed by `.rasm` path (#560 P1).
///
/// Separate from [`SkeletonLibrary`]: state machine is optional (many entities only use single clip / empty blend between),
/// Scenes without state machine should not be forced to build an empty table.
#[derive(Debug, Default)]
pub struct StateMachineLibrary {
    machines: HashMap<String, Arc<StateMachine>>,
}

impl StateMachineLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert state machine; returns `true` indicating this is new path.
    pub fn insert(&mut self, path: impl Into<String>, machine: StateMachine) -> bool {
        self.machines
            .insert(path.into(), Arc::new(machine))
            .is_none()
    }

    pub fn state_machine(&self, path: &str) -> Option<Arc<StateMachine>> {
        self.machines.get(path).cloned()
    }

    pub fn has_state_machine(&self, path: &str) -> bool {
        self.machines.contains_key(path)
    }

    /// All already loaded state machines (asset parsing after machine load completes relies on it to supplement clip requests).
    pub fn all(&self) -> impl Iterator<Item = &Arc<StateMachine>> {
        self.machines.values()
    }

    pub fn len(&self) -> usize {
        self.machines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.machines.is_empty()
    }
}

/// Already decoded animation graphs (`.rag` / `*.animgraph.json`), indexed by asset path (#560 P1).
#[derive(Debug, Default)]
pub struct AnimGraphLibrary {
    graphs: HashMap<String, Arc<AnimGraphInfo>>,
}

impl AnimGraphLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, path: impl Into<String>, graph: AnimGraphInfo) -> bool {
        self.graphs.insert(path.into(), Arc::new(graph)).is_none()
    }

    pub fn graph(&self, path: &str) -> Option<Arc<AnimGraphInfo>> {
        self.graphs.get(path).cloned()
    }

    pub fn has_graph(&self, path: &str) -> bool {
        self.graphs.contains_key(path)
    }

    pub fn all(&self) -> impl Iterator<Item = (&String, &Arc<AnimGraphInfo>)> {
        self.graphs.iter()
    }

    pub fn len(&self) -> usize {
        self.graphs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.graphs.is_empty()
    }
}

/// Per-entity graph evaluator (runtime resource, not in scene).
#[derive(Debug, Default)]
struct AnimGraphRuntimes {
    runtimes: HashMap<Entity, GraphRuntime>,
}

#[derive(Debug)]
struct GraphRuntime {
    path: String,
    evaluator: AnimGraphEvaluator,
}

impl AnimGraphRuntimes {
    fn get_or_create(
        &mut self,
        entity: Entity,
        path: &str,
        graph: Arc<AnimGraphInfo>,
    ) -> &mut AnimGraphEvaluator {
        let slot = self.runtimes.entry(entity).or_insert_with(|| GraphRuntime {
            path: path.to_string(),
            evaluator: AnimGraphEvaluator::new((*graph).clone()),
        });
        if slot.path != path {
            slot.path = path.to_string();
            slot.evaluator = AnimGraphEvaluator::new((*graph).clone());
        }
        &mut slot.evaluator
    }
}

/// Per-entity per-frame graph evaluation work item (#560 P6).
///
/// Evaluator taken from [`AnimGraphRuntimes`] inside during collect phase, put back after evaluation, so each
/// work item exclusively owns its evaluator, can be assigned to different threads.
struct GraphPoseJob {
    entity: Entity,
    skeleton: Arc<Skeleton>,
    runtime: GraphRuntime,
    /// Time this frame should advance (LOD throttled frame is `2 * dt`).
    dt: f32,
    result: Option<Result<(Pose, Vec<AnimEvent>), GraphEvalError>>,
}

impl GraphPoseJob {
    fn evaluate(&mut self, library: &LibraryView, dt: f32) {
        let entity = self.entity;
        let evaluated = self
            .runtime
            .evaluator
            .evaluate_with_events(library, &self.skeleton, dt);
        self.result = Some(evaluated.map(|(pose, events)| {
            let events = events
                .into_iter()
                .map(|event| AnimEvent {
                    entity,
                    name: event.name,
                    time: event.time,
                })
                .collect();
            (pose, events)
        }));
    }
}

/// First order applies Montage requests / parameters, then distributes each entity's graph evaluation to worker threads (#560 P6).
///
/// Each graph evaluator belongs to only one entity, does not share state with each other; `LibraryView` is read-only and fully is
/// `Arc`, can be safely borrowed by multiple threads at the same time. Fewer than two entities or only single-core does not build threads,
/// Directly order evaluate --- thread open/close overhead more expensive than this work in single-character scene inside.
fn evaluate_graph_poses(
    world: &World,
    library: &LibraryView,
    graph_runtimes: &mut AnimGraphRuntimes,
    params: &AnimParameters,
    montage_requests: &mut HashMap<Entity, Vec<MontageRequest>>,
    graph_entities: &mut HashSet<Entity>,
    plans: &HashMap<Entity, f32>,
) -> HashMap<Entity, Result<(Pose, Vec<AnimEvent>), GraphEvalError>> {
    let mut jobs: Vec<GraphPoseJob> = Vec::new();
    for (entity, animator) in world.query::<Animator>() {
        let path = animator.anim_graph_path.trim();
        if path.is_empty() {
            continue;
        }
        let Some(root) = world.get::<SkeletonRoot>(entity) else {
            continue;
        };
        let Some(skeleton) = library.skeleton(&root.skeleton_path) else {
            continue;
        };
        let Some(graph) = library.graph(path) else {
            continue;
        };
        graph_entities.insert(entity);
        // Frozen / frame-skipped entities keep evaluators (state machine / Montage progress comes back and continues), this frame does not advance.
        let Some(&entity_dt) = plans.get(&entity) else {
            continue;
        };
        let evaluator = graph_runtimes.get_or_create(entity, path, graph);
        if let Some(requests) = montage_requests.remove(&entity) {
            apply_montage_requests(evaluator, requests, &library.clips);
        }
        // Project-side written float / bool / trigger synced into graph evaluator.
        for parameter in evaluator.graph().parameters.clone() {
            match parameter.kind {
                prism_asset::formats_anim_graph::GraphParameterKind::Float => {
                    if let Some(value) = params.float(&parameter.name) {
                        evaluator.parameters_mut().set_float(parameter.name, value);
                    }
                }
                prism_asset::formats_anim_graph::GraphParameterKind::Bool => {
                    if let Some(value) = params.bool(&parameter.name) {
                        evaluator.parameters_mut().set_bool(parameter.name, value);
                    }
                }
                prism_asset::formats_anim_graph::GraphParameterKind::Trigger => {
                    if params.has_trigger(&parameter.name) {
                        evaluator.parameters_mut().set_trigger(parameter.name);
                    }
                }
            }
        }
        let Some(runtime) = graph_runtimes.runtimes.remove(&entity) else {
            continue;
        };
        jobs.push(GraphPoseJob {
            entity,
            skeleton,
            runtime,
            dt: entity_dt,
            result: None,
        });
    }

    let threads = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
        .min(jobs.len())
        .min(MAX_GRAPH_EVAL_THREADS);
    prism_ecs::jobs::scoped_parallel_chunks(&mut jobs, threads, |job| {
        job.evaluate(library, job.dt);
    });

    let mut results = HashMap::with_capacity(jobs.len());
    for job in jobs {
        let GraphPoseJob {
            entity,
            runtime,
            result,
            ..
        } = job;
        if let Some(result) = result {
            results.insert(entity, result);
        }
        graph_runtimes.runtimes.insert(entity, runtime);
    }
    results
}

/// Graph evaluation uses at most several worker threads; beyond entity count, more threads don't increase throughput.
const MAX_GRAPH_EVAL_THREADS: usize = 8;

/// A single Montage request action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MontageAction {
    /// Immediately play and clear this Slot's queue.
    PlayNow,
    /// Queued to this Slot's tail; immediately starts when empty idle.
    Enqueue,
    /// Stop this Slot and clear empty queue.
    Stop,
    /// Stops after current segment finishes, does not connect to next segment.
    StopAfterCurrent,
    /// Pause Slot's active segment (retains time / queue / weight).
    Pause,
    /// Resume Slot's active segment.
    Resume,
}

/// Project-side corrects some entity Slot-initiated Montage request (#560 P1).
///
/// Request lives only one frame: added to [`AnimMontageRequests`], next [`animator_system`] will consume it.
/// Clip path must already be loaded in [`SkeletonLibrary`]; entity must positively drive animation graph, and graph inside have
/// same-named Slot node, otherwise request will be ignored and logged.
#[derive(Debug, Clone, PartialEq)]
pub struct MontageRequest {
    pub entity: Entity,
    pub slot: String,
    pub action: MontageAction,
    pub clip_path: String,
    pub looped: bool,
    pub speed: f32,
    pub weight: f32,
    pub start_time: f32,
    /// Starts from which Montage segment of clip; empty = starts from `start_time`.
    pub section: String,
    /// Fade-in duration at start (seconds).
    pub blend_in: f32,
    /// Fade-out duration before single Montage ends (seconds).
    pub blend_out: f32,
}

impl MontageRequest {
    /// Immediately play. Default single time, speed 1, weight 1, starts from 0.
    pub fn play(entity: Entity, slot: impl Into<String>, clip_path: impl Into<String>) -> Self {
        Self {
            entity,
            slot: slot.into(),
            action: MontageAction::PlayNow,
            clip_path: clip_path.into(),
            looped: false,
            speed: 1.0,
            weight: 1.0,
            start_time: 0.0,
            section: String::new(),
            blend_in: 0.0,
            blend_out: 0.0,
        }
    }

    /// Queued play. Default single time, speed 1, weight 1, starts from 0.
    pub fn enqueue(entity: Entity, slot: impl Into<String>, clip_path: impl Into<String>) -> Self {
        Self {
            action: MontageAction::Enqueue,
            ..Self::play(entity, slot, clip_path)
        }
    }

    /// Pause Slot's active segment.
    pub fn pause(entity: Entity, slot: impl Into<String>) -> Self {
        Self {
            action: MontageAction::Pause,
            ..Self::play(entity, slot, "")
        }
    }

    /// Resume Slot's active segment.
    pub fn resume(entity: Entity, slot: impl Into<String>) -> Self {
        Self {
            action: MontageAction::Resume,
            ..Self::play(entity, slot, "")
        }
    }

    /// Stop Slot's active segment and queue.
    pub fn stop(entity: Entity, slot: impl Into<String>) -> Self {
        Self {
            action: MontageAction::Stop,
            ..Self::play(entity, slot, "")
        }
    }

    /// Stops after current segment finishes.
    pub fn stop_after_current(entity: Entity, slot: impl Into<String>) -> Self {
        Self {
            action: MontageAction::StopAfterCurrent,
            ..Self::play(entity, slot, "")
        }
    }

    pub fn with_looping(mut self, looped: bool) -> Self {
        self.looped = looped;
        self
    }

    pub fn with_speed(mut self, speed: f32) -> Self {
        self.speed = speed;
        self
    }

    pub fn with_weight(mut self, weight: f32) -> Self {
        self.weight = weight;
        self
    }

    pub fn with_start_time(mut self, start_time: f32) -> Self {
        self.start_time = start_time;
        self
    }

    /// Starts from named Montage segment; falls back to `start_time` when segment not present.
    pub fn with_section(mut self, section: impl Into<String>) -> Self {
        self.section = section.into();
        self
    }

    /// Fade-in duration at start (seconds).
    pub fn with_blend_in(mut self, seconds: f32) -> Self {
        self.blend_in = seconds;
        self
    }

    /// Fade-out duration before single Montage ends (seconds).
    pub fn with_blend_out(mut self, seconds: f32) -> Self {
        self.blend_out = seconds;
        self
    }
}

/// Montage request queue filled by project side each frame, consumed by [`animator_system`].
#[derive(Debug, Default)]
pub struct AnimMontageRequests {
    requests: Vec<MontageRequest>,
}

impl AnimMontageRequests {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, request: MontageRequest) {
        self.requests.push(request);
    }

    pub fn play(&mut self, entity: Entity, slot: impl Into<String>, clip_path: impl Into<String>) {
        self.push(MontageRequest::play(entity, slot, clip_path));
    }

    pub fn enqueue(
        &mut self,
        entity: Entity,
        slot: impl Into<String>,
        clip_path: impl Into<String>,
    ) {
        self.push(MontageRequest::enqueue(entity, slot, clip_path));
    }

    /// Immediately plays from named segment start.
    pub fn play_section(
        &mut self,
        entity: Entity,
        slot: impl Into<String>,
        clip_path: impl Into<String>,
        section: impl Into<String>,
    ) {
        self.push(MontageRequest::play(entity, slot, clip_path).with_section(section));
    }

    pub fn stop(&mut self, entity: Entity, slot: impl Into<String>) {
        self.push(MontageRequest::stop(entity, slot));
    }

    pub fn stop_after_current(&mut self, entity: Entity, slot: impl Into<String>) {
        self.push(MontageRequest::stop_after_current(entity, slot));
    }

    pub fn len(&self) -> usize {
        self.requests.len()
    }

    pub fn is_empty(&self) -> bool {
        self.requests.is_empty()
    }

    pub fn requests(&self) -> &[MontageRequest] {
        &self.requests
    }

    /// Takes all pending requests (system calls every frame).
    pub fn take(&mut self) -> Vec<MontageRequest> {
        std::mem::take(&mut self.requests)
    }
}

/// Apply one frame's requests to entity graph evaluator.
fn apply_montage_requests(
    evaluator: &mut AnimGraphEvaluator,
    requests: Vec<MontageRequest>,
    clips: &HashMap<String, Arc<Clip>>,
) {
    for request in requests {
        let slot = request.slot.trim();
        if slot.is_empty() {
            log::warn!("animator: montage request has an empty slot name");
            continue;
        }
        match request.action {
            MontageAction::Stop => evaluator.stop_montage(slot),
            MontageAction::StopAfterCurrent => evaluator.stop_montage_after_current(slot),
            MontageAction::Pause => {
                if !evaluator.pause_montage(slot) {
                    log::warn!("animator: montage pause on unknown slot '{slot}'");
                }
            }
            MontageAction::Resume => {
                if !evaluator.resume_montage(slot) {
                    log::warn!("animator: montage resume on unknown slot '{slot}'");
                }
            }
            MontageAction::PlayNow | MontageAction::Enqueue => {
                let path = request.clip_path.trim();
                let Some(clip) = clips.get(path).cloned() else {
                    log::warn!("animator: montage clip '{path}' is not loaded");
                    continue;
                };
                if !request.section.trim().is_empty()
                    && !clip
                        .sections
                        .iter()
                        .any(|section| section.name == request.section)
                {
                    log::warn!(
                        "animator: montage clip '{path}' has no section '{}'",
                        request.section
                    );
                }
                let montage = Montage::new(clip)
                    .with_start_time(request.start_time)
                    .with_start_section(request.section.as_str())
                    .with_looping(request.looped)
                    .with_speed(request.speed)
                    .with_weight(request.weight)
                    .with_blend_in(request.blend_in)
                    .with_blend_out(request.blend_out);
                if request.action == MontageAction::PlayNow {
                    evaluator.play_montage(slot, montage);
                } else {
                    evaluator.enqueue_montage(slot, montage);
                }
            }
        }
    }
}

/// Root joint (joint 0) relative per-frame bit movement increment (#560 P0, first only recorded, P2 handed to CharacterMovement).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RootMotionDelta {
    /// Root joint local empty-interval translation increment (meters).
    pub translation: glam::Vec3,
    /// Yaw increment around Y axis (radians, most short path).
    pub yaw: f32,
}

/// Playhead (runtime component, not in scene).
#[derive(Debug, Clone, PartialEq)]
pub struct AnimatorPlayhead {
    /// Current playback key: clip path, or [`BLEND_SPACE_KEY`]. Changes cause playback from start.
    pub clip_path: String,
    pub time: f32,
    /// Single play and already played.
    pub finished: bool,
    /// Cross-fade fade-out source pose (`None` = current has no transition), transition starts frozen, weight to 1 drops.
    ///
    /// Store pose not as "old clip + time": blend empty interval exit, source clip deleted, continuous clip change these
    /// cases don't have (or shouldn't again find) a samplable clip, fast snapshot lets them all able to always fade out.
    pub previous_pose: Option<Pose>,
    /// Per-frame **actual display** pose: below one transition's fade-out source is it.
    pub pose: Pose,
    /// Transition already performed seconds (>= blend_duration clears source).
    pub blend_elapsed: f32,
    /// This transition's own duration (state machine transition brings `duration`); `None` = use component's
    /// `blend_duration`.
    pub blend_total: Option<f32>,
    /// This frame from root joint pushes bit movement increment; first frame / switch play content / freeze frame is zero.
    pub root_motion: RootMotionDelta,
    /// Per-frame most final pose root joint local transform, used to calculate below frame delta.
    pub root_previous: Option<prism_anim::JointTransform>,
    /// Smooth after blend empty interval parameter (x, y); `None` = no smooth open or parameter non-have limit.
    pub smoothed_blend: Option<[f32; 2]>,
}

impl AnimatorPlayhead {
    /// Transition weight: `0` = full old clip, `1` = full new clip.
    pub fn blend_weight(&self, blend_duration: f32) -> Option<f32> {
        if self.previous_pose.is_none() || blend_duration <= 0.0 {
            return None;
        }
        Some((self.blend_elapsed / blend_duration).clamp(0.0, 1.0))
    }

    /// Use this frame's most final pose to update root motion delta (#560 P0).
    ///
    /// Read root joint's **local** transform: when switching play content, playhead is new built, `root_previous` is empty,
    /// so transition / clip switch's first frame delta is zero, will not repeat old clip's root bit movement once.
    fn record_root_motion(&mut self, pose: &Pose) {
        let Some(now) = pose.local(0) else {
            self.root_motion = RootMotionDelta::default();
            return;
        };
        self.root_motion = match self.root_previous {
            Some(previous) => RootMotionDelta {
                translation: now.translation - previous.translation,
                yaw: yaw_delta(previous.rotation, now.rotation),
            },
            None => RootMotionDelta::default(),
        };
        self.root_previous = Some(now);
    }
}

/// Root motion consumption (#560 P2): subtract this frame's root translation from pose inside, hand to entity's
/// [`LocalTransform`]. Return need apply model empty interval increment.
fn consume_root_translation(
    animator: &Animator,
    playhead: &AnimatorPlayhead,
    pose: &mut Pose,
    entity: Entity,
    moves: &mut Vec<(Entity, glam::Vec3)>,
) {
    if !animator.root_motion_consume {
        return;
    }
    let delta = playhead.root_motion.translation;
    if !delta.is_finite() || delta.length_squared() <= f32::EPSILON {
        return;
    }
    if let Some(mut root) = pose.local(0) {
        root.translation = glam::Vec3::ZERO;
        pose.set_local(0, root);
    }
    moves.push((entity, delta));
}

/// Blend parameter exponential smooth (#560 P1). `dt = 0` returns current value (first frame directly falls into target on).
fn smoothed_blend_parameters(
    playhead: &mut AnimatorPlayhead,
    animator: &Animator,
    dt: f32,
) -> (f32, f32) {
    let target = [animator.blend_parameter, animator.blend_parameter_y];
    let tau = animator.blend_smoothing;
    if !(tau.is_finite() && tau > 0.0) || !target[0].is_finite() || !target[1].is_finite() {
        playhead.smoothed_blend = None;
        return (target[0], target[1]);
    }
    let current = playhead.smoothed_blend.unwrap_or(target);
    let alpha = if dt.is_finite() && dt > 0.0 {
        1.0 - (-dt / tau).exp()
    } else {
        0.0
    };
    let next = [
        current[0] + (target[0] - current[0]) * alpha,
        current[1] + (target[1] - current[1]) * alpha,
    ];
    playhead.smoothed_blend = Some(next);
    (next[0], next[1])
}

/// Yaw difference between two quaternions (most short path, radians).
fn yaw_delta(from: glam::Quat, to: glam::Quat) -> f32 {
    let relative = to * from.inverse();
    let mut yaw = 2.0 * relative.y.atan2(relative.w);
    let tau = std::f32::consts::TAU;
    if yaw > std::f32::consts::PI {
        yaw -= tau;
    } else if yaw < -std::f32::consts::PI {
        yaw += tau;
    }
    if yaw.is_finite() {
        yaw
    } else {
        0.0
    }
}

/// graph evaluator in playhead inside "clip path" bit flag.
pub const ANIM_GRAPH_KEY: &str = "<anim-graph>";

/// blend empty interval in playhead inside "clip path" bit flag.
///
/// blend empty interval not single clip, but playhead needs **one key** to judge "play content changed". Use this
/// not able to conflict with true real asset path flag, entering/exiting blend empty interval naturally triggers a cross-fade.
pub const BLEND_SPACE_KEY: &str = "<blend-space>";

/// 2D blend empty interval playhead bit key. With 1D key separate: switching between 1D / 2D same needs to
/// trigger a cross-fade (their pose source completely not same).
pub const BLEND_SPACE_2D_KEY: &str = "<blend-space-2d>";

/// advance has `Animator` and heavy calculates skeletal matrix, returns updated entity count.
/// Animation LOD near start point and freeze distance; bad value falls back to 25m / 50m (#560 P6).
fn lod_thresholds(animator: &Animator) -> (f32, f32) {
    let far = if animator.lod_far.is_finite() && animator.lod_far > 0.0 {
        animator.lod_far
    } else {
        50.0
    };
    let near = if animator.lod_near.is_finite() && animator.lod_near > 0.0 {
        animator.lod_near.min(far)
    } else {
        far
    };
    (near, far)
}

/// Animation LOD throttle state: records this frame skipped, below one frame entity to update (#560 P6).
#[derive(Debug, Default, Clone)]
pub struct AnimLodThrottle {
    skipped: HashSet<Entity>,
}

/// First enabled camera world position; animation LOD not active when no camera.
fn camera_eye(world: &World) -> Option<glam::Vec3> {
    for (entity, camera) in world.query::<Camera>() {
        if !camera.enabled {
            continue;
        }
        if let Some(matrix) = world.get::<LocalToWorld>(entity) {
            return Some(matrix.0.w_axis.truncate());
        }
        if let Some(transform) = world.get::<LocalTransform>(entity) {
            return Some(transform.position);
        }
    }
    None
}

fn entity_position(world: &World, entity: Entity) -> glam::Vec3 {
    if let Some(matrix) = world.get::<LocalToWorld>(entity) {
        return matrix.0.w_axis.truncate();
    }
    if let Some(transform) = world.get::<LocalTransform>(entity) {
        return transform.position;
    }
    glam::Vec3::ZERO
}

/// Each animation entity's `dt` for this frame; entities not in the table skip entirely this frame.
///
/// - Beyond `lod_far`: freeze (not in table, graph evaluator retained);
/// - `lod_near..lod_far`: evaluate every other frame, update frame uses `2 * dt` to maintain playback speed;
/// - Near / LOD not open / no camera: `dt` every frame.
fn animation_frame_plans(
    world: &World,
    eye: Option<glam::Vec3>,
    dt: f32,
    throttle: &mut AnimLodThrottle,
) -> (HashMap<Entity, f32>, HashSet<Entity>) {
    let mut plans = HashMap::new();
    let mut root_only = HashSet::new();
    for (entity, animator) in world.query::<Animator>() {
        let Some(eye) = eye else {
            plans.insert(entity, dt);
            continue;
        };
        if !animator.lod_enabled {
            throttle.skipped.remove(&entity);
            plans.insert(entity, dt);
            continue;
        }
        let (near, far) = lod_thresholds(animator);
        let distance = entity_position(world, entity).distance(eye);
        if distance > far {
            throttle.skipped.remove(&entity);
            if (animator.lod_root_only || !animator.lod_bones.is_empty())
                && animator.anim_graph_path.trim().is_empty()
                && animator.state_machine_path.trim().is_empty()
                && !animator.uses_blend_space()
                && !animator.uses_blend_space_2d()
            {
                plans.insert(entity, dt);
                root_only.insert(entity);
            }
            continue;
        }
        if distance > near {
            if throttle.skipped.remove(&entity) {
                // Updated on a frame, this frame skips and clears flag.
                continue;
            }
            throttle.skipped.insert(entity);
            plans.insert(entity, dt * 2.0);
            continue;
        }
        throttle.skipped.remove(&entity);
        plans.insert(entity, dt);
    }
    (plans, root_only)
}

pub fn animator_system(world: &mut World, dt: f32) -> usize {
    let Some(library) = world.get_resource::<SkeletonLibrary>() else {
        return 0;
    };
    let machines: HashMap<String, Arc<StateMachine>> = world
        .get_resource::<StateMachineLibrary>()
        .map(|library| library.machines.clone())
        .unwrap_or_default();
    let graphs = world
        .get_resource::<AnimGraphLibrary>()
        .map(|library| library.graphs.clone())
        .unwrap_or_default();
    let library = LibraryView {
        skeletons: library.skeletons.clone(),
        clips: library.clips.clone(),
        machines,
        graphs,
    };
    let mut graph_runtimes = world
        .remove_resource::<AnimGraphRuntimes>()
        .unwrap_or_default();
    // debug fast path only maintained when caller actively inserts resource (see `AnimGraphDebug`).
    let mut graph_debug = world.remove_resource::<AnimGraphDebug>();
    let mut graph_entities: HashSet<Entity> = HashSet::new();
    // State machine parameters (project-side written, engine reads every frame). Clone a copy: `take_trigger` needs `&mut`,
    // cannot continue borrowing World while iterating entities.
    let mut params = world
        .get_resource::<AnimParameters>()
        .cloned()
        .unwrap_or_default();
    let mut state_updates: Vec<(Entity, String)> = Vec::new();
    // Root motion consumption (#560 P2): subtract this frame's root translation from pose inside, hand to entity's
    let mut root_motion_moves: Vec<(Entity, glam::Vec3)> = Vec::new();
    // Project-side requests live only one frame; grouped by entity, consumed when driving graph.
    let mut montage_requests: HashMap<Entity, Vec<MontageRequest>> = HashMap::new();
    if let Some(resource) = world.get_resource_mut::<AnimMontageRequests>() {
        for request in resource.take() {
            montage_requests
                .entry(request.entity)
                .or_default()
                .push(request);
        }
    }

    // Foot IK (#560 P4) smoothing state: engine-owned, loaded like the runtime map
    // above and written back after the pass.
    let mut foot_ik_states: HashMap<Entity, super::foot_ik::FootIkState> = world
        .query::<super::foot_ik::FootIkState>()
        .map(|(entity, state)| (entity, state.clone()))
        .collect();

    let mut updates: Vec<(Entity, BonePalette, AnimatorPlayhead)> = Vec::new();
    let mut events: Vec<AnimEvent> = Vec::new();
    // Animation LOD (#560 P6): far entities frozen, near entities evaluated every other frame; graph run retains state to resume later.
    let mut throttle = world
        .remove_resource::<AnimLodThrottle>()
        .unwrap_or_default();
    let (plans, root_only_entities) =
        animation_frame_plans(world, camera_eye(world), dt, &mut throttle);
    world.insert_resource(throttle);
    // Graph evaluation first runs in parallel by entity (#560 P6); the loop below only orders World reads/writes.
    let mut graph_results = evaluate_graph_poses(
        world,
        &library,
        &mut graph_runtimes,
        &params,
        &mut montage_requests,
        &mut graph_entities,
        &plans,
    );
    for (entity, animator) in world.query::<Animator>() {
        let Some(root) = world.get::<SkeletonRoot>(entity) else {
            continue;
        };
        let Some(skeleton) = library.skeleton(&root.skeleton_path) else {
            continue;
        };
        let Some(&entity_dt) = plans.get(&entity) else {
            continue;
        };
        let dt = entity_dt;

        // Animation graph (#560 P1): when path non-empty and graph already loaded, takes priority over state machine / blend space / single clip.
        // Evaluator persisted per entity (state machine nodes need cross-frame state); rebuilding heavily when graph path changes.
        if !animator.anim_graph_path.trim().is_empty() {
            match graph_results.remove(&entity) {
                Some(Ok((pose, graph_events))) => {
                    events.extend(graph_events);
                    let mut playhead = world.get::<AnimatorPlayhead>(entity).cloned().unwrap_or(
                        AnimatorPlayhead {
                            clip_path: ANIM_GRAPH_KEY.into(),
                            time: 0.0,
                            finished: false,
                            previous_pose: None,
                            pose: Pose::default(),
                            blend_elapsed: 0.0,
                            blend_total: None,
                            root_motion: RootMotionDelta::default(),
                            root_previous: None,
                            smoothed_blend: None,
                        },
                    );
                    playhead.clip_path = ANIM_GRAPH_KEY.into();
                    playhead.finished = false;
                    playhead.previous_pose = None;
                    playhead.record_root_motion(&pose);
                    let mut pose = pose;
                    consume_root_translation(
                        animator,
                        &playhead,
                        &mut pose,
                        entity,
                        &mut root_motion_moves,
                    );
                    foot_ik_pass(world, entity, &skeleton, &mut pose, &mut foot_ik_states, dt);
                    playhead.pose = pose.clone();
                    let palette = BonePalette::from_matrices(&skeleton.bone_palette(&pose));
                    updates.push((entity, palette, playhead));
                    continue;
                }
                Some(Err(error)) => {
                    log::warn!(
                        "animator: graph '{}' on entity {:?} failed: {error}",
                        animator.anim_graph_path.trim(),
                        entity
                    );
                }
                None => {}
            }
        }

        let mut single_path = if animator.clip_path.is_empty() {
            root.default_clip_path.clone()
        } else {
            animator.clip_path.clone()
        };

        // State machine (#560 P1): first check whether this frame will transition, then use (possibly already swapped-out) state to decide what to play.
        //
        // Transition criterion uses **this frame** playback time vs current state's clip duration: that frame confirms real playback reached
        // this position, closer to author's correct `min_state_fraction` expectation than using "time after this frame advances".
        let machine = if animator.state_machine_path.trim().is_empty() {
            None
        } else {
            library.machines.get(&animator.state_machine_path).cloned()
        };
        let mut state_name = animator.state.clone();
        let mut transition_duration: Option<f32> = None;
        let mut state_switched = false;
        let mut state_speed = 1.0f32;
        let mut state_looped = true;
        if let Some(machine) = &machine {
            let current = machine.state_index(&state_name).unwrap_or(0);
            let previous_time = world
                .get::<AnimatorPlayhead>(entity)
                .map(|head| head.time)
                .unwrap_or_else(|| sanitize_time(animator.time));
            let state_duration = machine
                .state(current)
                .and_then(|state| library.clips.get(&state.clip_path))
                .map(|clip| clip.duration)
                .unwrap_or(0.0);
            let target =
                machine.next_transition(current, &mut params, previous_time, state_duration);
            let index = match &target {
                Some(target) => target.state,
                None => current,
            };
            if let Some(target) = target {
                transition_duration = Some(target.duration);
                state_switched = true;
            }
            if let Some(state) = machine.state(index) {
                // Empty / unknown state names also become true real names: editor and save see "current state",
                // not an empty string.
                state_name = state.name.clone();
                state_speed = state.speed;
                state_looped = state.looped;
                if !state.clip_path.trim().is_empty() {
                    single_path = state.clip_path.clone();
                }
            }
            if state_name != animator.state {
                state_updates.push((entity, state_name.clone()));
            }
        }
        // State machine owns speed / loop semantics; when no state machine attached, fall back to reading component.
        let speed = if machine.is_some() {
            animator.speed * state_speed
        } else {
            animator.speed
        };
        let looped = if machine.is_some() {
            state_looped
        } else {
            animator.looped
        };

        // 2D blend space priority (#560 P1): at least three samples and able to triangulate to take effect; coordinates
        // not limited / collinear fall back to 1D or single clip -- a few degrees of freedom is always better than entity not moving.
        let mut space_2d: Option<BlendSpace2D> = None;
        if animator.uses_blend_space_2d() {
            let samples: Vec<BlendSample2D> = animator
                .blend_2d_clip_paths
                .iter()
                .zip(animator.blend_2d_positions.iter())
                .filter_map(|(path, position)| {
                    library.clip(path).map(|clip| BlendSample2D {
                        position: *position,
                        clip,
                    })
                })
                .collect();
            let candidate = BlendSpace2D::new(samples, skeleton.rest_pose());
            if !candidate.is_empty() {
                space_2d = Some(candidate);
            }
        }
        let blend_space_2d = space_2d.is_some();

        // 1D blend space (#560 P1): listed clips inside need at least two truly loaded,
        // otherwise fall back to single clip. When 2D active, do not rebuild 1D samples to avoid wasted work.
        let mut samples: Vec<BlendSample> = Vec::new();
        if !blend_space_2d && animator.uses_blend_space() {
            samples = animator
                .blend_clip_paths
                .iter()
                .zip(animator.blend_positions.iter())
                .filter_map(|(path, position)| {
                    library.clip(path).map(|clip| BlendSample {
                        position: *position,
                        clip,
                    })
                })
                .collect();
        }
        let blend_space = !blend_space_2d && samples.len() >= 2;
        let clip = if blend_space_2d || blend_space {
            None
        } else {
            match library.clip(&single_path) {
                Some(clip) => Some(clip),
                None => continue,
            }
        };
        let playback_key = if blend_space_2d {
            BLEND_SPACE_2D_KEY.to_string()
        } else if blend_space {
            BLEND_SPACE_KEY.to_string()
        } else {
            single_path.clone()
        };

        // Playhead: switching clips starts from author-provided start time, otherwise continues on this frame.
        let mut playhead = match world.get::<AnimatorPlayhead>(entity) {
            Some(head) if head.clip_path == playback_key => head.clone(),
            Some(head) => AnimatorPlayhead {
                // Switch clip: freeze old clip's pose at the moment it stopped as fade-out source (#560 P1).
                // Old clip already not in library (asset deleted / this frame is blend space), fall back to already-held fast snapshot --
                // switching twice in a row will not lose the positive fade-out portion.
                clip_path: playback_key.clone(),
                time: if state_switched {
                    0.0
                } else {
                    sanitize_time(animator.time)
                },
                finished: false,
                previous_pose: if !head.pose.is_empty() {
                    Some(head.pose.clone())
                } else {
                    head.previous_pose.clone()
                },
                pose: Pose::default(),
                blend_elapsed: 0.0,
                blend_total: transition_duration,
                root_motion: RootMotionDelta::default(),
                root_previous: None,
                smoothed_blend: None,
            },
            None => AnimatorPlayhead {
                clip_path: playback_key.clone(),
                time: sanitize_time(animator.time),
                finished: false,
                previous_pose: None,
                pose: Pose::default(),
                blend_elapsed: 0.0,
                blend_total: transition_duration,
                root_motion: RootMotionDelta::default(),
                root_previous: None,
                smoothed_blend: None,
            },
        };
        // Blend space has no length of its own: take the longest clip, shorter ones naturally loop to catch up.
        let duration = match &clip {
            Some(clip) => clip.duration.max(0.0),
            None => {
                let mut longest = 0.0f32;
                if let Some(space) = &space_2d {
                    for sample in space.samples() {
                        longest = longest.max(sample.clip.duration);
                    }
                } else {
                    for sample in &samples {
                        longest = longest.max(sample.clip.duration);
                    }
                }
                longest.max(0.0)
            }
        };
        // Local time already in this playback content this frame: only triggers notify if it exists --
        // just created / just switched clip's first frame should not blast out all clip events at once.
        let previous_local = match world.get::<AnimatorPlayhead>(entity) {
            Some(head) if head.clip_path == playback_key => {
                Some(local_time(duration, looped, head.time))
            }
            _ => None,
        };

        if animator.playing && dt.is_finite() {
            playhead.time += dt * speed;
            if !looped {
                playhead.time = playhead.time.clamp(0.0, duration);
            }
        }
        playhead.finished = !looped && duration > 0.0 && playhead.time >= duration;

        // Whether looped: state machine state takes priority, otherwise `Animator` component decides (clip's internal
        // `looped` is only an import default value).
        let local = local_time(duration, looped, playhead.time);

        // Close key frame events: only trigger before playback, and only when really in this clip this frame.
        // Reverse playback (`speed < 0`) needs incorrect reverse scanning interval, temporarily not triggered -- better to miss a footstep once
        // than play all events backwards during reverse playback.
        if let (Some(previous), Some(clip)) = (previous_local, clip.as_ref()) {
            if animator.playing && dt.is_finite() && speed > 0.0 {
                for notify in clip.notifies_between(previous, local) {
                    events.push(AnimEvent {
                        entity,
                        name: notify.name.clone(),
                        time: notify.time,
                    });
                }
            }
        }
        // Animation LOD subset: keep this frame's pose, only advance time + sample root joint (and `lod_bones`).
        // When no name list, it means "root bone only"; first entry fully evaluates one frame to avoid T-pose pop.
        if root_only_entities.contains(&entity)
            && playhead.pose.len() == skeleton.joints().len()
            && !playhead.pose.is_empty()
        {
            let mut pose = playhead.pose.clone();
            if let Some(clip) = clip.as_ref() {
                clip.sample_joint_into(local, 0, &mut pose);
                let mut seen = vec![false; skeleton.joints().len()];
                for name in &animator.lod_bones {
                    let Some(joint) = skeleton.joint_index(name.trim()) else {
                        continue;
                    };
                    if joint == 0 || seen[joint] {
                        continue;
                    }
                    seen[joint] = true;
                    clip.sample_joint_into(local, joint, &mut pose);
                }
            }
            playhead.previous_pose = None;
            playhead.blend_elapsed = 0.0;
            playhead.blend_total = None;
            playhead.record_root_motion(&pose);
            consume_root_translation(
                animator,
                &playhead,
                &mut pose,
                entity,
                &mut root_motion_moves,
            );
            playhead.pose = pose.clone();
            let palette = BonePalette::from_matrices(&skeleton.bone_palette(&pose));
            updates.push((entity, palette, playhead));
            continue;
        }
        let (blend_x, blend_y) = smoothed_blend_parameters(&mut playhead, animator, dt);
        let mut pose = skeleton.rest_pose();
        match &clip {
            Some(clip) => {
                clip.sample_into(local, &mut pose);
            }
            None => {
                let phase = if duration > 0.0 {
                    local / duration
                } else {
                    0.0
                };
                match &mut space_2d {
                    Some(space) => {
                        space.evaluate_into([blend_x, blend_y], phase, &mut pose);
                    }
                    None => {
                        let mut space = BlendSpace1D::new(samples.clone(), skeleton.rest_pose());
                        space.evaluate_into(blend_x, phase, &mut pose);
                    }
                }
            }
        }

        // Cross-fade: old clip frozen at transition open moment, blended into new pose by weight.
        //
        // First advance then take weight: on clip-switch frame `dt` is "distance from real time since last frame",
        // transition should start timing from that moment, otherwise most frames never reach weight 1.
        let blend_duration = playhead
            .blend_total
            .map(sanitize_blend_duration)
            .unwrap_or_else(|| sanitize_blend_duration(animator.blend_duration));
        if blend_duration <= 0.0 {
            // No blend duration = hard cut: drop fast snapshot, otherwise the first frame when duration is adjusted back
            // will suddenly fade in an already-expired pose.
            playhead.previous_pose = None;
            playhead.blend_elapsed = 0.0;
            playhead.blend_total = None;
        } else if playhead.previous_pose.is_some() && animator.playing && dt.is_finite() {
            playhead.blend_elapsed += dt.abs();
        }
        if let Some(weight) = playhead.blend_weight(blend_duration) {
            if let Some(fading) = playhead.previous_pose.as_ref() {
                // `fading` is old pose, `pose` is new pose: `t = weight` 0 -> old, 1 -> new.
                let mut blended = skeleton.rest_pose();
                prism_anim::blend_into(&mut blended, fading, &pose, weight);
                pose = blended;
            }
            // Weight already reached 1: new pose is the full new clip, fast snapshot can be dropped.
            if weight >= 1.0 {
                playhead.previous_pose = None;
                playhead.blend_elapsed = 0.0;
                playhead.blend_total = None;
            }
        }

        // Additive layer (#560 P1): incremental relative (bind) pose, weighted into final pose.
        // Placed after cross-fade -- additive layer should not participate in "transition between two base clips".
        let additive_weight = if animator.additive_weight.is_finite() {
            animator.additive_weight.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if additive_weight > 0.0 && !animator.additive_clip_path.trim().is_empty() {
            if let Some(layer) = library.clip(animator.additive_clip_path.as_str()) {
                let mut additive = skeleton.rest_pose();
                layer.sample_into(layer.wrap_time(local), &mut additive);
                let reference = skeleton.rest_pose();
                prism_anim::apply_additive(&mut pose, &additive, &reference, additive_weight);
            }
        }

        // Animation graph layer (#560 P1): after base clip + cross-fade + single additive layer,
        // stack `Animator::layers` in author order. Each layer samples on same time axis,
        // per-bone mask lets only listed bones participate; BlendStack handles order, weight and
        // additive semantics; missing clip / 0 weight layers are skipped.
        if !animator.layers.is_empty() {
            let reference = skeleton.rest_pose();
            let mut stack = BlendStack::new(pose, reference.clone());
            for layer in &animator.layers {
                if layer.clip_path.trim().is_empty() || layer.weight <= 0.0 {
                    continue;
                }
                let Some(clip) = library.clip(layer.clip_path.as_str()) else {
                    continue;
                };
                let mut layer_pose = reference.clone();
                clip.sample_into(clip.wrap_time(local), &mut layer_pose);
                let mask = if layer.bone_mask.is_empty() {
                    BoneMask::all()
                } else {
                    let names: Vec<&str> = layer.bone_mask.iter().map(String::as_str).collect();
                    BoneMask::from_names(&skeleton, &names)
                };
                stack.push(
                    BlendLayer::new(layer_pose)
                        .with_weight(layer.weight)
                        .with_mask(mask)
                        .with_additive(layer.additive),
                );
            }
            let mut stacked = Pose::default();
            stack.evaluate_into(&mut stacked);
            pose = stacked;
        }

        // Root motion (#560 P0): record root joint relative increment this frame; when consumption open, hand translation
        // to entity Transform, and subtract from pose inside (#560 P2).
        playhead.record_root_motion(&pose);
        consume_root_translation(
            animator,
            &playhead,
            &mut pose,
            entity,
            &mut root_motion_moves,
        );
        foot_ik_pass(world, entity, &skeleton, &mut pose, &mut foot_ik_states, dt);
        // This pose is the fade-out source for the next transition (no need to clone: `pose` already used up on this face).
        playhead.pose = pose;
        let palette = BonePalette::from_matrices(&skeleton.bone_palette(&playhead.pose));
        updates.push((entity, palette, playhead));
    }

    for (entity, requests) in montage_requests {
        for _request in requests {
            log::warn!(
                "animator: montage request on entity {entity:?} ignored (no active animation graph)"
            );
        }
    }

    let count = updates.len();
    for (entity, delta) in root_motion_moves {
        if let Some(transform) = world.get_mut::<LocalTransform>(entity) {
            transform.position += transform.rotation * delta;
        }
    }

    for (entity, palette, playhead) in updates {
        world.insert(entity, palette);
        world.insert(entity, playhead);
    }
    for (entity, state) in foot_ik_states {
        if world.is_alive(entity) {
            world.insert(entity, state);
        }
    }
    // State written back to component by engine: editor / save see current state, not asking playhead again.
    for (entity, state) in state_updates {
        if let Some(animator) = world.get_mut::<Animator>(entity) {
            animator.state = state;
        }
    }
    graph_runtimes
        .runtimes
        .retain(|entity, _| graph_entities.contains(entity));
    if let Some(debug) = graph_debug.as_mut() {
        debug.entities.clear();
        debug.montages.clear();
        debug.paths.clear();
        for (entity, runtime) in &graph_runtimes.runtimes {
            debug
                .entities
                .insert(*entity, runtime.evaluator.node_snapshots());
            debug
                .montages
                .insert(*entity, runtime.evaluator.montage_snapshots());
            debug.paths.insert(*entity, runtime.path.clone());
        }
    }
    world.insert_resource(graph_runtimes);
    if let Some(debug) = graph_debug {
        world.insert_resource(debug);
    }
    world.insert_resource(AnimEvents { events });
    count
}

/// Run foot placement for one entity while the animator streams over the world.
///
/// Skipped for entities that are not solved this frame (LOD throttling, missing
/// `FootIk`) — the helper itself decides and leaves the pose alone.
fn foot_ik_pass(
    world: &World,
    entity: Entity,
    skeleton: &Skeleton,
    pose: &mut Pose,
    states: &mut HashMap<Entity, super::foot_ik::FootIkState>,
    dt: f32,
) {
    super::foot_ik::apply_for_entity(world, entity, skeleton, pose, states, dt);
}

/// Cross-fade duration: negative value / NaN / infinity all treated as 0 (direct switch).
fn sanitize_blend_duration(duration: f32) -> f32 {
    if duration.is_finite() && duration > 0.0 {
        duration
    } else {
        0.0
    }
}

/// Fold playhead time into clip local time (loop takes remainder, single clamps to `0..=duration`).
fn local_time(duration: f32, looped: bool, time: f32) -> f32 {
    if duration > 0.0 {
        if looped {
            time.rem_euclid(duration)
        } else {
            time.clamp(0.0, duration)
        }
    } else {
        0.0
    }
}

fn sanitize_time(time: f32) -> f32 {
    if time.is_finite() {
        time
    } else {
        0.0
    }
}

/// Borrow below to avoid holding World resource while iterating.
struct LibraryView {
    skeletons: HashMap<String, Arc<Skeleton>>,
    clips: HashMap<String, Arc<Clip>>,
    machines: HashMap<String, Arc<StateMachine>>,
    graphs: HashMap<String, Arc<AnimGraphInfo>>,
}

impl LibraryView {
    fn skeleton(&self, path: &str) -> Option<Arc<Skeleton>> {
        self.skeletons.get(path).cloned()
    }

    fn clip(&self, path: &str) -> Option<Arc<Clip>> {
        self.clips.get(path).cloned()
    }

    fn graph(&self, path: &str) -> Option<Arc<AnimGraphInfo>> {
        self.graphs.get(path).cloned()
    }
}

impl AnimGraphSource for LibraryView {
    fn clip(&self, path: &str) -> Option<Arc<Clip>> {
        self.clips.get(path).cloned()
    }

    fn state_machine(&self, path: &str) -> Option<Arc<StateMachine>> {
        self.machines.get(path).cloned()
    }
}

#[cfg(test)]
#[path = "animator_tests.rs"]
pub(crate) mod tests;
