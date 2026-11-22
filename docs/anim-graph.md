# Animation graph format (RAGP v2, #560 P1)

The animation graph connects clips, blend spaces, BlendStack, Slot, layered blending, and state machines into a directed acyclic graph.
The author writes `*.animgraph.json`, cooker validates and compiles it into `.rag` (RAGP v2) placed into `.pak`;
runtime only decodes, without looking up names at load time.

> Current scope: format, authoring JSON validation, cooker encoding, build pipeline import, runtime decoding, asset type,
> `AnimGraphEvaluator`, and runtime driving of `Animator.anim_graph_path`.

## Authoring format

```json
{
  "parameters": [
    { "name": "speed", "type": "float", "default": 0.0 },
    { "name": "aiming", "type": "bool", "default": false }
  ],
  "nodes": [
    {
      "name": "locomotion",
      "type": "blend_space_1d",
      "clips": ["idle.ranim", "run.ranim"],
      "positions": [0.0, 5.0],
      "speeds": [1.0, 2.0],
      "parameter": "speed"
    },
    {
      "name": "aim",
      "type": "clip_player",
      "clip": "aim.ranim",
      "additive": true,
      "weight": 0.5,
      "bone_mask": ["spine_02", "hand_r"]
    },
    {
      "name": "stack",
      "type": "blend_stack",
      "inputs": ["locomotion", { "node": "aim", "weight": 0.5 }]
    },
    { "name": "out", "type": "output", "input": "stack" }
  ]
}
```

Rules:

- `type`: `clip_player`, `blend_space_1d`, `blend_space_2d`, `blend_stack`,
  `slot`, `layered_blend`, `cached_pose`, `state_machine`, `output`.
- Node names must be non-empty and unique; strings in `inputs` are equivalent to weight-1 connections, or can be written as
  `{ "node": "aim", "weight": 0.5, "bone_mask": ["spine_02"] }`.
- `blend_space_1d` requires at least 2 clips, `positions` correspond one-to-one with clips;
  `blend_space_2d` requires at least 3 clips, `positions` are `[x, y]`.
- Optional `speeds` gives each BlendSpace sample independent playback speed; length must equal clip count,
  values must be finite and `>= 0`; default / all 1 uses shared phase.
- `slot` / `cached_pose` / `output` have exactly 1 input;
  `blend_stack` / `layered_blend` have at least 1 input.
- The graph must have exactly 1 `output` node; connections must point to existing nodes and cannot form cycles.
- Weights are clamped to `0..=1`; non-finite values fall back to defaults; old tools default to `speed = 1`,
  `looped = true`, `weight = 1`, `additive = false`.

## Runtime interface

- `prism_asset::runtime::AnimGraphAsset` — `ResourceManager` typed loading of `.rag`.
- `prism_asset::formats_anim_graph::AnimGraphInfo` — parameters, nodes, input indices and output indices.
- `AssetType::AnimGraph` / extension `rag`.

## Build pipeline

- `prism-cook import` / `build` identifies `*.animgraph.json` via `AnimGraphImporter`:
  parses and validates authoring JSON, encodes into RAGP intermediate data, and resolves clip / state machine paths into asset dependencies.
- `AnimGraphCooker` validates RAGP magic / version / structure of intermediate data, then packages as-is into `.pak`
  (`AssetType::AnimGraph`).
- Import cache / dependency topological sort reuses the existing asset pipeline; invalid graph format causes that asset import to fail, instead of
  bringing bad data into the pack.

## Byte layout

```text
[magic:4 "RAGP"][version:1]
[parameter_count:4]
parameters: count × { name:u16+utf8, kind:u8, default:f32 }
[node_count:4]
nodes: count × {
    name:u16+utf8, kind:u8, flags:u8, speed:f32, weight:f32,
    clip_count:4 + clips, sample_count:4 + [x,y] f32,
    param_x:u16+utf8, param_y:u16+utf8, machine:u16+utf8,
    input_count:4 × { node:u32, weight:f32, mask_count:4 + names },
    speed_count:4 + f32 × count          // v2; v1 does not have this table
}
[output:u32]
```

`flags`: bit0 = looped, bit1 = additive. All counts have limits; truncated, illegal magic /
version, unknown kind / flags, out-of-bounds node indices, and trailing bytes will cause decoding to return `None`.
v1 clips (no speed table) can still be decoded; `sample_speeds` is empty.

## Runtime evaluation

`prism_anim::AnimGraphEvaluator` evaluates `AnimGraphInfo` into a `Pose`:

```rust
struct Assets { /* decoded Clip / StateMachine */ }
impl AnimGraphSource for Assets {
    fn clip(&self, path: &str) -> Option<Arc<Clip>> { /* ... */ }
    fn state_machine(&self, path: &str) -> Option<Arc<StateMachine>> { /* ... */ }
}

let mut evaluator = AnimGraphEvaluator::new(graph_info);
evaluator.parameters_mut().set_float("speed", 4.0);
let pose = evaluator.evaluate(&assets, &skeleton, dt)?;
```

- Each node is evaluated only once per frame; ClipPlayer / BlendSpace / StateMachine each advance time.
- `BlendStack` / `LayeredBlend`: the first input is base, the remaining inputs are stacked by weight and bone mask.
- `Slot` covers an optional montage clip (clips[0], independent time/loop/Notify) on top of the base input by `weight`; with no clip it passes through transparently; the evaluator also maintains a Montage playback queue for each Slot (see below). `CachedPose` / `Output` pass through transparently.
- StateMachine node internally maintains current state, state time, and transition cross-fade; Trigger is consumed per state machine semantics.
- `evaluate_with_events` additionally returns `GraphNotifyEvent`: triggered when ClipPlayer / StateMachine crosses a clip Notify; loop wraparound scans in two segments, time 0 is not re-emitted on the first frame, shared nodes emit only once.
  BlendSpace defaults to letting only the **sample with the largest weight** generate events (`BlendSpaceNotifyPolicy::Dominant`), avoiding duplicate footsteps from multiple sample points; switching to `BlendSpaceNotifyPolicy::None` disables entirely.
  StateMachine by default still scans from the leaving state to the transition point on transition frames; `StateTransitionNotifyPolicy::SuppressOnTransition` drops this frame's leaving-state events, avoiding "foot just lifted then switched away" footsteps.
- Missing clip / state machine, empty blend space, empty state machine, or cycles all return `GraphEvalError`, without panicking.

Montage playback is handled by `prism_anim::MontagePlayer`: `play_now` overwrites immediately, `enqueue`
FIFO queues, auto-connects to next segment after single playback completes, `stop` / `stop_after_current`
clears the corresponding state, `tick` collects cross-frame Notify. The evaluator connects it to Slot nodes: `play_montage` /
`enqueue_montage` / `stop_montage` operate by node name, active segment covers Slot input by weight;
when no active segment, falls back to author-written `clips[0]`.

Montage sections (#560 P4): `.ranim` v5's section table (author-side `*.skinning/montages.json`) is resolved at runtime into `Clip.sections`. `Montage::with_start_section("Vault")` starts from section beginning,
`MontagePlayer::active_section()` returns the section containing the current time, `jump_to_section` jumps sections during playback;
if section does not exist, falls back to `start_time`. Engine's `MontageRequest::with_section` /
`AnimMontageRequests::play_section` brings section name into request; debug snapshot's `MontageSnapshot.section` displays current section.

Montage blend-in / blend-out (#560 P4): `Montage::with_blend_in(seconds)` linearly ramps from 0 weight to author weight,
`with_blend_out(seconds)` linearly ramps down to 0 before single Montage ends; looping Montage does not use blend-out,
avoiding a flash at each loop end. `MontagePlayer::active_weight()` returns actual weight multiplied by blend-in /
blend-out factors (debug snapshot's `weight` is also this); blend-in does not advance when paused. Engine request corresponds to
`MontageRequest::with_blend_in` / `with_blend_out`.

Montage BranchIn (#560 P4): when clip Notify name is `BranchIn.<section>`, playback head crossing this event
jumps to the start of the same-named section (`BRANCH_IN_PREFIX`). If target section does not exist, event is still emitted but no jump; not triggered when paused.
BranchIn itself still enters the event queue as a normal Notify; the editor's existing Notify sidecar can write it.
For example `{"time": 0.5, "name": "BranchIn.Land"}`.

MotionWarping window (#560 P4): `.ranim` v5's warp window table (author-side `*.skinning/montages.json`'s `warp_windows`) is resolved into `Clip.warp_windows`.
`MontagePlayer::active_warp_window()` returns current window, `warp_window_progress()` returns 0..=1 progress within window;
debug snapshot's `MontageSnapshot.warp_window` / `warp_window_progress` display synchronously.
This layer only provides window data and queries; actual root motion warping / target point solving is consumed by subsequent MotionWarping.

Sync markers: `.ranim` v5's sync marker table is converted via `Clip::sync_marker_set()` into `SyncMarkerSet`;
asset side comes from `*.skinning/sync_markers.json`. `BlendSpace1D/2D` automatically builds `SyncGroup` when all samples have same-structure markers,
mapping phase to remaining samples with the **current max-weight sample** as the leading clip (leader changes when parameter switches);
when markers missing or counts inconsistent, falls back to shared normalized phase.
Evaluator can use `set_blend_space_speeds(node, speeds)` to configure independent playback heads for samples (by `node.clips` order),
then each sample advances by its own speed, skipping SyncGroup mapping; empty configuration restores shared phase.

Editor panel: `Window → General → Animation Graph`, after selecting `.animgraph.json` / `.statemachine.json`,
displays structured view (parameters, nodes, connections, states, transition conditions); panel top's `↶ Undo edit` / `↷ Redo edit`
undo / redo this asset's JSON changes (full snapshot, continuous drag merged into one step; parameter rename along with state machine's multi-file change
is one group, rolled back together, independent from scene undo stack).
Panel top lists shape issues cooker will reject (node type / clip count / connections / output / cycles / parameters);
graph side can add/remove parameters, change default values, rename parameters
(synchronously rewrites blend space references and graph-referenced state machine sidecar condition names; referenced parameters cannot be deleted), drag layout, add/remove/rename/copy clip nodes,
change node's clip / speed / weight / loop, change blend space parameter name / sample coordinates (including 2D canvas drag point, with collinear degeneration hint) / sample add/remove, add/remove connections and change weight /
bone mask; state machine side can change state name / clip / speed / looped, copy state, add/remove transitions, reconnect transition `from` / `to`,
adjust transition order with `↑` / `↓` (order is priority, runtime takes first matching transition), drag transition `blend` / `min` duration, and add/remove conditions and change float threshold / bool expected value. When selecting
`*.skinning/notifies.json`, the same panel becomes event table editor: lists Notify by clip, can change time / name,
delete, or add via draft form (including new clip key); each clip's timeline can drag gold marker to change time directly, clicking blank space opens new draft at that time.

`Animator.blend_smoothing` (seconds) when enabled, blend space parameters approach target with exponential smoothing,
avoiding velocity jumps that make pose pop; default 0 = passthrough.

Each frame at runtime, `AnimatorPlayhead.root_motion` also records root joint displacement / yaw delta relative to previous frame
(first frame and clip-switch first frame are zero). Default only records, does not modify entity Transform.

When `Animator.root_motion_consume = true` (#560 P2), engine adds this frame's **root translation** to entity's
`LocalTransform.position` (first rotates to world space by entity's current orientation), and clears root translation in pose,
avoiding double displacement of "entity moved, bone root also moves". First frame / clip-switch first frame delta is zero, so no teleport.
Root **rotation** delta is currently only recorded, not consumed: yaw must be subtracted together with the entire bone chain to avoid double rotation,
left to CharacterMovement for unified handling.

## Real-time debug data (#560)

Debug overlay / editor tools can insert an `AnimGraphDebug` resource into World (default does not exist):

```rust
world.insert_resource(AnimGraphDebug::default());
// After animator_system each frame:
if let Some(states) = world.get_resource::<AnimGraphDebug>().and_then(|d| d.states(entity)) {
    for node in states {
        // node.name / node.kind / node.time / node.state
        // node.blending / node.blend_weight / node.cached / node.weight
        // node.state_name / node.state_clip (state machine node)
    }
}
```

`AnimGraphDebug` also provides `montages(entity)`: one `MontageSnapshot` per Slot
(`slot` / `clip` / `section` / `time` / `weight` / `playing` / `paused` / `finished` / `queued`),
sorted by Slot name, only includes Slots that have been `play` / `enqueue`d, used to see where overlay animation is stuck in debug panel.

Engine only rebuilds snapshot each frame when resource exists (node order consistent with graph, strings copied); zero overhead when not present.
Frozen / throttled graph entities retain last snapshot, convenient to see "stopped where". `time` is node's own local time,
state machine node's `state` is current state index, `blending` / `blend_weight` reflects cross-fade,
`cached` indicates whether `cached_pose` node has cached pose, `weight` is this node's contribution to final pose
(multiplied down by connection weights from output node, shared nodes take max, not connected to output is 0; BlendStack /
LayeredBlend connection weight is layer weight). `weight` is for display only, does not participate in evaluation. State machine node also has
`state_name` / `state_clip` (current state name and its clip path), other nodes are empty strings.

Engine-side project code sends requests via `AnimMontageRequests` resource (`MontageRequest::play` /
`enqueue` / `stop` / `stop_after_current` / `pause` / `resume`, can carry loop / speed / weight /
start time / section name / blend-in / blend-out); `animator_system` consumes once per frame, parses clip path into `Montage` injected into corresponding entity's
Slot. Request lives only one frame; if clip not loaded, entity has no graph or Slot name doesn't exist, ignored with warning; section name doesn't exist
logs warning and falls back to `start_time`.

`pause` only freezes that Slot's active segment: time, queue, weight unchanged, no Notify produced during pause;
`resume` continues from original time. Explicit `play` / `play_now` clears pause, `stop` clears pause state.
`MontagePlayer` also provides `set_paused` / `is_paused` / `active_time` for UI display.

Engine side: `Animator.anim_graph_path` non-empty and graph loaded takes priority over state machine / blend space /
single clip; `load_animation_assets` loads `.rag` / `*.animgraph.json`, then additionally loads state machine and clip referenced by graph.
`animator_system` persists one evaluator per entity, syncs project parameter table's
float/bool/trigger into it, and merges graph Notify into existing `AnimEvents`; on graph failure,
falls back to regular clip path and logs warning.

Multi-character parallelism (#560 P6): `animator_system` first **sequentially** applies Montage requests, syncs parameters,
then takes each entity's own evaluator from table, hands to `prism_ecs::jobs::scoped_parallel_chunks`
for block-parallel evaluation (up to 8 worker threads); pose, playback head, and events still return to main thread to sequentially write into ECS.
Evaluators do not share state, so results are consistent with single-threaded — directly sequential for fewer than two graph entities or single-core device,
no threads spawned.
