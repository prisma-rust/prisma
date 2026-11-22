# Pose retargeting (Retargeting, #560 P5)

Move a pose evaluated on the **source skeleton** to the **target skeleton** for playback. Currently only does the FK half:
no IK goal / pole vector, no `IKRig` / `RetargeterDef` asset, no chain type mapping
(Interpolated / OneToOne / Reversed / None) — those need a new asset type in `prism-asset` first.

Math lives in `crates/prism-anim-retarget`, doesn't touch ECS / render / assets, just like `prism-anim-ik` and
`prism-motion-matching`, it's a pure value layer.

## Mapping

```rust
let map = RetargetMap::by_name(&source, &target);       // same-named joints
let map = RetargetMap::new(vec![JointPair::new(3, 7)]); // explicit pairing
let pose = retarget(&source, &source_pose, &target, &map, &options)?;
```

- `by_name` covers the common case of "same rig re-exported at different proportions".
- When the same target joint is mapped twice, keep the first, ignore the rest (do not silently change override order).
- **Unmapped target joints stay at rest pose**: mapping only the FK chain can still produce a playable pose.

## Transfer rules

For each joint pair:

| Quantity | Rule |
|----------|------|
| Rotation | Default relative to both rest poses: `rest_target · rest_source⁻¹ · anim_source`; when `relative_to_rest = false`, directly copy local rotation (suitable for retarget poses made specifically for this pair) |
| Translation | Default scales target rest displacement by "this frame length / source rest length" ratio, i.e. **keep target skeleton's own bone lengths**; `scale_translation = false` uses rest directly |
| Root joint | Default does **not** transfer root displacement (character movement left to gameplay / root motion); `transfer_root = true` brings it along |
| Scale channel | Directly copy source joint's scale |
| Global scale | `global_scale` multiplied onto translation (character height difference), defaults to `1.0` keeping target proportions |

Why not directly copy translation: short character playing tall character's walk would stretch legs, feet off ground; scaling by rest length ratio preserves bending angles, bone lengths remain target's own.

## Returns `None` on failure

Mapping out of bounds, or pose length not equal to source skeleton joint count → `None`. A truncated pose is treated as an animation bug, not caller parameter error.

## Not yet done

- IK goals / pole vector / PBIK, IK convergence for non-retarget skeletons.
- Chain type mapping and retarget pose asset (`RetargeterDef`).
- Editor dual-skeleton side-by-side preview, error threshold validation (feet / hands not penetrating).
