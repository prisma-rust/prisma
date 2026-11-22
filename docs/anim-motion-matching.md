# Motion Matching (#560 P3)

Pick the frame best matching "current pose + desired trajectory" from the clip library to continue playback,
replacing "speed greater than X switches to run" style thresholds in hand-written state machines. Convention consistent
with animation system: model space = skeleton root space, entity transform multiplied last, +Y up.

## Data model

`crates/prism-motion-matching` is divided into four blocks, coupled only through value types:

| Type | Role |
|------|------|
| `Trajectory` / `TrajectoryPoint` | Where the character is required to go: several future sample points of position, velocity, facing |
| `PoseFeature` | Pose signature: positions and velocities of tracked joints (`from_matrices` uses previous frame differencing) |
| `MotionDatabase` / `MotionFrame` | Each clip sampled by `sample_interval` into frames, each frame carrying both pose feature and this frame's trajectory |
| `search` / `MotionMatch` | Linear scan, adds two halves of cost by weight and takes minimum |
| `SelectionHistory` / `SearchBias` | Recent choices, used to make decisions "sticky" instead of jumping between two nearly equal frames |

## Cost

```text
pose_cost       = per joint average (|Δposition|² + |Δvelocity|²)
trajectory_cost = per sample average (w_pos·|Δposition|² + w_vel·|Δvelocity|² + w_facing·(1 - dot))
cost            = w_pose·pose_cost + w_traj·trajectory_cost
```

- Frames with inconsistent joint count or trajectory sample count are **skipped directly**, no misaligned feature comparison.
- Facing compares only horizontal component: normal-up / degenerate vector samples count as 0.
- `Trajectory::predicted` is "keep current velocity moving straight" prediction, used to turn player input intent into query;
  it does not read clips.

## Persistence and re-selection

Pure nearest neighbor bounces back and forth between two frames only a few millimeters apart, looking like stutter:
adjacent samples of the same walk have nearly equal cost, winner flips every few frames. Real motion matching uses
two methods to stabilize decisions, both expressed as **cost multipliers** (`SearchBias`), not changing scan logic:

```text
cost = (w_pose·pose_cost + w_traj·trajectory_cost) · multiplier(frame)
multiplier = bias.continuing   if frame is the last selected frame in `SelectionHistory` (default 0.85)
           = bias.reselect     if frame was recently selected but is no longer current (default 1.15)
           = 1.0               otherwise
```

- `search` (without history) equals `SearchBias::NONE`, behavior exactly as before; use
  `search_biased(..., bias, Some(&history))` when stickiness is needed.
- Returned `MotionMatch` has `cost` including multiplier, `pose_cost` / `trajectory_cost` keep original distance,
  debug panel can directly see "kept because cheap or because already playing".
- History only records **decision** (`frame` / `clip` / `time`), not pose; recording same frame repeatedly only refreshes time,
  does not fill the window. `clear()` used for teleport, scene reload etc. when "previous frame's pose no longer relevant".
- Multiplier only scales: `continuing = 1.0` / `reselect = 1.0` is pure independent decision; non-finite or
  non-positive config treated as 1.0, cost never goes negative.
- Which frame is "obviously better" decided by multiplier: `continuing = 0.5` means another frame must be half as cheap to switch.

## Modeling conventions

- Trajectory comes from **root joint**'s own future motion in clip (`DatabaseSettings::root_joint` defaults to 0):
  loop clips wrap, one-shot clips hold last frame at end.
- Pose sampled from skeleton rest pose, single clip, so frame describes the clip itself, not entity Transform,
  nor blend / state machine output.
- `DatabaseSettings::tracking_all` tracks all joints, suitable for first pass; project later can pick a set of joints
  (left/right feet, hips, hands) to reduce feature dimension.

## Foot lock

When switching frames, old and new frame gait phases differ, feet teleport a few centimeters, looking like ground sliding under feet.
`FootLock` remembers "this foot has been planted at this position", then each evaluation pulls foot back to original position,
until it steps itself:

```rust
let mut lock = FootLock::new(2);
let settings = FootLockSettings { joints: vec![left_foot, right_foot], ..Default::default() };
let report = lock.update(&skeleton, &mut pose, &settings, dt);
```

- **Plant / release by speed**: single-frame speed ≤ `plant_speed` (default 0.35 m/s) counts as planted;
  > `release_speed` (default 0.9 m/s) releases immediately. Speed uses "position since last evaluation";
  lock correction itself is not counted, otherwise self-cancellation.
- **Correction capped**: `max_offset` (default 0.25 m) clamps per-frame displacement, locking too long would make legs
  unequal; better to release than ruin pose.
- `weight = 0` (or non-finite) directly disables, pose not changed by a single byte; `dt <= 0` or non-finite only
  records position, no speed judgment.
- Correction first converted to parent joint space then written back to local translation, so child joints (toes) follow.
- Call `reset()` after scene change / teleport, otherwise foot would be pulled back to old position.

## Not yet done

- Not connected to `Animator`: runtime wiring of query, frame switching, cross-clip blending is next slice.
- No acceleration structure: linear scan of a few to a few thousand frames is enough; KD-tree / VP-tree etc. only if profiling requires.
- No inertialization blending: frame switch currently hard cut + foot lock.
- Historical "recently selected" only looks at frame number, no time-decay weight; can add an age weight layer outside `SearchBias` if needed.
