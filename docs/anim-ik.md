# Animation IK (`prism-anim-ik`)

`crates/prism-anim-ik` is the math foundation for #560 P4: analytical IK, not depending on engine / render /
ECS, shared by engine, editor preview, and offline tools. Currently only two-bone IK; Foot Placement,
LegIK, PBIK, IK Rig / Retargeter are later slices, do not write another copy elsewhere before landing.

## Conventions

- Consistent with `prism-anim`: right-handed, +Y up, camera −Z; quaternion `xyzw` (`glam::Quat`), column-major matrices.
- Solving in **component space** — i.e. the space output by `Skeleton::world_matrices(pose)`, not world space.
  Target points and pole directions must first be converted to the same space; the entity's own `LocalToWorld`
  does not participate at this layer.
- Weight `weight` is a `0..=1` blending coefficient; `slerp` and position `lerp` each interpolate by it;
  `0` means no solve (return `None`, pose unchanged).

## Two-bone IK

```rust
use prism_anim_ik::{apply_two_bone_ik, solve_two_bone, TwoBoneChain};

// Pure math: given three-bone positions, target, and pole, get two-level world-space incremental rotations.
let solution = solve_two_bone(hip, knee, foot, target, pole, 1.0);

// Attach to skeleton: convert increments to local rotations of two bones and write back to Pose.
apply_two_bone_ik(&skeleton, &mut pose, TwoBoneChain::new(0, 1, 2), target, pole, 1.0);
```

- Middle joint position computed by law of cosines; bending plane determined by projecting `pole` onto the
  plane perpendicular to "toward target" direction; when `pole` is parallel to target direction, fall back to
  the coordinate axis least aligned with target direction, ensuring stable result.
- When unreachable, straighten the chain; when too close, fold by `|a − b|`; both cases set
  `TwoBoneSolution::clamped`, `reach` is the actual distance used.
- Non-finite input, zero-length bones, target coincident with root joint, duplicate or out-of-range bone indices
  all return `None`, no NaN pose written.
- The two rotations in `TwoBoneSolution` are **world-space increments** (`new = delta * old`);
  `apply_two_bone_ik` converts them to local increments using parent world rotation, returned
  `middle_position` / `end_position` are positions actually reached after applying the pose
  (position after `slerp` is not `lerp` of positions).

## Tests

`crates/prism-anim-ik/src/two_bone_tests.rs` covers: hit target, out of range straighten,
too close fold, pole determines bend direction, pole parallel fallback, weight blending,
various degenerate inputs, actual position after writing back to `Pose`, weight 0 leaves pose unchanged,
illegal bone indices.

## Foot Placement

`FootPlacement` (`src/foot.rs`) plants both feet to the ground per frame, then lets the pelvis follow down:

```rust
let report = placement.update(
    &skeleton, &mut pose,
    0,                                  // hips joint
    [left_chain, right_chain],          // thigh -> knee -> ankle
    &settings, &ground_queries, dt,
);
```

- Ground injected by caller: implement `GroundQuery::cast(origin, direction, max_distance)`.
  Engine connects physics raycast (#522); tests use plane or stairs.
- Each foot probes downward from `probe_height` above ankle: not hitting ground, ground higher than `max_step_up`
  (wall), or lower than `max_step_down` (hole) all count as no ground, weight decays to 0 by `blend_seconds`,
  foot stays at last target position.
- **Pelvis only goes down**: based on un-corrected pelvis, compute the most exceeded distance of the tightest leg,
  move down and clamp to `pelvis_min..=0`. Lifting up only makes already-planted feet even less reachable; lifting
  foot is solved by two-bone IK bending knee. Correction amount is recorded as "already baked into pose offset",
  repeated calls do not accumulate.
- Both legs' `upper` must be under hips joint (`parent == hips`); pelvis itself is not rotated, otherwise
  both legs would rotate the same bone each.
- `weight = 0` directly clears state and keeps pose unchanged; illegal chain / mismatched pose length returns
  `applied = false`, writes no bones.

## Engine-side ground query

`prism_anim_ik` itself knows neither ECS nor render; ground query is implemented by caller. Engine already has
`prism_engine::anim_ground::SceneGroundQuery`:

```rust
use prism_engine::anim_ground::SceneGroundQuery;

// Only ground layer (1), and skip character's own colliders.
let ground = SceneGroundQuery::new(world, 1).excluding(character);
```

- Underlying uses `physics::raycast_excluding` (same box / sphere / capsule intersection as `raycast`, pure ECS geometry),
  so no `prism-app/physics` feature needed, no dependency on rapier stepping order.
- Triggers don't count as ground: `is_sensor` volumes cannot bear weight.
- Probe starts from inside character's capsule, must `excluding(character)`, otherwise nearest hit is always character itself.

## Engine wiring (`FootIk` component)

Add `FootIk` (`prism_engine::scene::FootIk`) on scene entity, joint names resolved by skeleton:

```json
"foot_ik": {
  "hips_joint": "hips",
  "left_thigh": "thigh_l", "left_shin": "shin_l", "left_foot": "foot_l",
  "right_thigh": "thigh_r", "right_shin": "shin_r", "right_foot": "foot_r",
  "ground_layer": 1
}
```

- Called after `Animator` evaluates pose, before computing bone palette (both anim graph branch and regular clip /
  state machine / blend space branches; animated LOD-frozen distant entities **do not run**, avoid conflicting with throttling).
- Pose is skeleton root space, collider is world space: `ModelGroundQuery` in
  `prism_engine::scene::systems::foot_ik` converts probe to world via entity `LocalToWorld`, hit converted back.
  Model scale treated as uniform scale (probe distance scaled by directional length).
- `FootIk` defaults `ground_layer = 1`, so ground colliders must be on layer 1; `0` means probe nothing.
- Any joint name resolution failure, `enabled = false`, `weight = 0`, or pose length not equal to skeleton joint count,
  **pose is returned as-is**, no mid-way bone writing.
- Smooth state (plant position, baked pelvis offset) is engine's own `FootIkState`, not in authoring document, not in
  RSCN, written back by system like `AnimatorPlayhead`.
