# PRT GI and 64-tree streaming design (M1–M4 contract)

> **Level**: L2 specialized subsystem design contract · advanced rendering
> **Status**: design draft (code not yet landed)
> **Related**: [DESIGN.md](DESIGN.md) §6 (Baked GI and Render Graph), [mobile-raytracing-gi-design.md](mobile-raytracing-gi-design.md), [lessons-learned.md](lessons-learned.md)

This document solidifies M1–M4 milestone deliverables into **verifiable engineering contracts**: data format, pipeline topology, GPU-side numeric conventions,
and **measurement methods and decision thresholds** for each quantitative metric. Any implementation deviating from this file's contract must revise this file first as authority.

> **Quality-first baseline (highest constraint)**: this phase adopts no means of trading quality for performance via downsampling, reduced precision, temporal decimation or visibility-based culling.
> All metrics judged at **full resolution, f32 arithmetic, 1:1 sampling**. When performance misses target, change algorithm or representation, not image quality —
> see §7.3 red lines.

---

## 0. Current state and gaps (recognize foundation first)

| Capability | Repo current state | Needed by this design |
|---|---|---|
| Offline baking | `prism-build-pipeline bake-gi` (`gi-bake` feature): RayQuery multi-bounce path tracing, projected to **2nd-order SH (9 coefficients)** regular-grid probes | Higher-order PRT transport vectors + **PT clustering** + **64-tree** + `.prtbin` export |
| Runtime representation | `prism-render::gi` (`ProbeVolumeInfo` + `eval_sh9`), `probe_loader` (`PRPV` magic) | PRT coefficient-space cluster-major sparse storage and evaluation |
| Runtime consumption | `scene_frag.slang` `PBR_FLAG_GI` branch, 3D texture `Load` + manual trilinear | New `EvaluatePRT64` sampling layer and Pass 4 composition |
| Dynamic lights | ReSTIR GI (raster secondary rays), SHARC cache; **no** clustered dynamic point-light propagation | Cluster-Major propagation + fixed-point atomic accumulation |
| Spatial structure | Regular grid (`dims: [u32;3]` + `spacing`) | 64-tree blocks + async streaming |

Conclusion: **this design is not tuning existing probe volumes, but a new representation and pipeline**. Existing `PRPV` 2nd-order grid path
kept as low-frequency diffuse fallback signal source, PRT path connected via new `gi_mode` value, both share `SceneScope` and
`ForwardPass` composition formula (same `E/π` dimension), **no duplicated evaluation code**.

---

## 1. Terminology rulings (eliminate ambiguity first)

| Term | Definition here | Explicitly **not** |
|---|---|---|
| **PRT64** | 7th-order real spherical harmonics, **precomputed radiance transfer** vector with `(7+1)^2 = 64` coefficients per channel | 64 sample rays; 64 probes |
| **64-tree** | Spatial index tree subdividing each node `4×4×4` into **64 children** (64-way octree) | 64-level tree; octree typo |
| **Clustering (PT clustering tool)** | Offline PCA + k-means on probe transport matrices yielding **cluster**: a set of 64-tree leaves sharing same 64-coefficient transport basis | Runtime k-means; material clustering |
| **`EvaluatePRT64`** | **Whole chain** evaluating cluster 64 coefficients into radiance / irradiance under current lighting (Pass 1–4), not a single function call | 64 dot products per pixel (see §7.2, read-amplification and frequency-domain reasons) |
| **Dual `uint32` bit operations** | Sparse keys / masks / f16 pairs **uniformly** represented as `uvec2` on GPU, sole representation from day one | "Compatibility layer" for hardware lacking 64-bit (violates zero-compat discipline in [versioning.md](versioning.md), forbidden) |

> **Zero-compat discipline**: `uvec2`, fixed-point accumulation, f16 packing are **sole implementations**, no "detect hardware take another path" branches.
> Device capability differences **don't affect** math path, resolution or precision: no capability-driven quality degradation this phase,
> capability detection only decides "can run", not "how accurately" (consistent with §7.3 quality red line).

---

## 2. Data contract: `.prtbin`

Single file, little-endian, 16-byte aligned, directly uploadable via `vkCmdCopyBufferToImage` / `Buffer`, **no runtime reordering**.

### 2.1 Header (`PRTB`, 64 bytes)

| Offset | Size | Field | Notes |
|---:|---:|---|---|
| 0 | 4 | `magic` | `b"PRTB"` |
| 4 | 2 | `version` | `u16`, currently `1` |
| 6 | 2 | `flags` | bit0: body is f16 (f32 when unset); bit1: includes `valid` byte |
| 8 | 4 | `sh_band` | `u32`, always `7` (64 coefficients) |
| 12 | 4 | `leaf_count` | `u32`, **hard cap 8600** (see §2.4) |
| 16 | 4 | `node_count` | `u32` |
| 20 | 4 | `cluster_cells` | `u32`, spatial cells per cluster (default 1) |
| 24 | 12 | `world_origin` | `[f32;3]`, root AABB min corner |
| 36 | 4 | `world_extent` | `f32`, root cube edge length (meters) |
| 40 | 4 | `leaf_grid` | `u32`, leaf edge = `world_extent / 4^leaf_grid` |
| 44 | 4 | `payload_bytes` | `u32`, cluster payload byte count |
| 48 | 8 | `reserved` | Zeroed |
| 56 | 8 | `built_from_sha` | First 8 bytes truncated SHA-256 of scene `scenes.pak`, reject mismatch |

### 2.2 Node table (`node_count × 16` bytes)

| Offset | Size | Field |
|---:|---:|---|
| 0 | 4 | `first_child` (`u32`; `0xFFFF_FFFF` means leaf) |
| 4 | 4 | `leaf_index` (`u32`; `0xFFFF_FFFF` for internal nodes) |
| 8 | 8 | `child_mask` (`u64`, bit `i` means child `i` exists; **GPU side `uvec2`**) |

Node order is BFS (breadth-first), so `first_child` monotonically increases, enabling one linear scan for streaming insertion.
GPU node table and cluster payload in **two buffers**: node table resident, cluster payload block-unloadable.

### 2.3 Cluster payload (cluster-major)

`leaf_count` records, each:
`64 coefficients × RGB × 2 bytes (f16) = 384 bytes`, optional trailing `1` byte `valid` flag (`flags.bit1`).
Arranged `[coeff][rgb]`, consistent with `gi.slang` `EvalSH9Radiance` coefficient order (basis index 0..63,
numbering follows real-SH order in `gi.rs` doc table extended to 7th order).

**Why cluster-major**: runtime Pass 1 schedules `workgroup = 1 cluster`, one coalesced read of
384 bytes (= 96 `uint32`, processed as `uvec2` doublewords), 64 threads in workgroup each hold 1 coefficient,
avoiding any cross-lane gather. This is storage prerequisite for M3 "Cluster-Major propagation".

### 2.4 Volume budget (direct derivation of M1 acceptance)

```
cluster payload = leaf_count × 384 B
node table      = node_count × 16 B   ≈ leaf_count × 16 B × 1.02 (64-tree internal nodes only ~1.6%)
header          = 64 B
```

From above can solve leaf count for volume cap: `leaf_count` over `8600` definitely exceeds 3.5 MB budget,
**bake tool must fail-exit at that threshold**, not silently truncate. Hard constraint depending only on format constants,
independent of scene complexity — complexity decides whether baking completes within threshold, not the threshold itself.

---

## 3. M1: Offline bake toolchain

New subcommand `prism-build-pipeline bake-prt` (reuses `gi-bake` feature and `bake_gi.rs` BLAS / TLAS,
scene loading, `collect_bake_instances` infrastructure, no reimplementation).

### 3.1 Four phases

1. **Sampling**: emit directional samples from 64-tree leaf candidate centers (first split by `leaf_grid` rule),
   each sample records full transport "direction × hit point × subsequent bounces", projected to 64-coefficient basis.
   Sample count starts at `bake_samples_per_leaf` (default `4096`), adaptive additional sampling by variance (target relative error ≤ 3% per coefficient).
2. **PT clustering**: PCA on `leaf_count × 64×3` transport matrix, take principal components (cumulative energy ≥ 99%) then k-means,
   `k = leaf_count`. Clustering is **quality optimization**: leaves in same cluster share transport basis, while leaves "on opposite sides of wall shouldn't interpolate" go to different clusters,
   first mechanism of §5.3 anti-light-leak.
3. **64-tree build**: bottom-up, `4×4×4` merge; leaf split criteria:
   (a) AABB crosses geometry surface (surfaces on both sides of occupied grid), (b) angle between transport vectors exceeds `cluster_merge_deg` (default 12°).
   Criterion (a) guarantees **no leaf crosses wall**.
4. **Export**: `.prtbin` (§2) + bake report JSON (time, hit rate, per-cluster residual, `leaf_count`).

### 3.2 Acceptance

| Item | Threshold | Measurement |
|---|---|---|
| End-to-end time | **≤ 15 min** (single RTX 4090) | `bake-prt` built-in wall clock report, excludes first `scenes.pak` build |
| Output size | **≤ 3.5 MB** | `leaf_count ≤ 8600` hard assert + actual file bytes |
| Scene scale | 500 m × 500 m city, ≥ 3000 static instances | Report records instances / triangles / leaf count |

> **Primary risk is VRAM not time**: BLAS occupancy grows linearly with triangle count for 500 m city, in large scenes VRAM may become constraint before
> 15 min limit. This file doesn't preset specific time / VRAM numbers, tool timing and measured VRAM authoritative;
> after triangle budget confirmed (§7.5) decide whether BLAS needs block build.

---

## 4. M2: Runtime base pipeline

### 4.1 Pass topology (inserted into existing Render Graph)

```
ShadowMapPass → ForwardPass → GtaoPass → RestirGiPass → PrtPass(1,2,3) → PostPass
                                                   ↘ ForwardPass samples PrtPass output next frame (1-frame latency)
```

Isomorphic to `GtaoPass` / `RestirGiPass`: **frame-level double buffering + 1 frame latency**, resources in `FrameScope`,
never reuse `SceneScope`'s `vk::Image` (DESIGN §6.1 red line). `.prtbin` node table and cluster payload in
`SceneScope`, destroyed only on scene change.

| Pass | Type | Work | Output |
|---|---|---|---|
| **1. ClusterEval** | compute | Evaluate 64 coefficients × current sun / sky direction for **all** clusters | `cluster_radiance` (f16 storage, f32 arithmetic) |
| **2. Propagate** | compute | 6-neighbor propagation of dynamic light contributions (M3) | `cluster_radiance` delta (fixed-point atomic accumulation) |
| **3. ResolveVolume** | compute | Cluster → sparse voxel grid (**64³**, 8 m) trilinear splatting + leak gating, output 2nd-order (9 coefficients) irradiance | `gi_volume` (f32, 64³ × 9×3) |
| **4. PixelSample** | pixel | `EvaluatePRT64`: **full-resolution** trilinear sampling + `E/π` composition | HDR color |

Pass 3 dropping to 2nd order is **correct layering of representation**, not quality reduction: irradiance naturally low-frequency, high-order energy already integrated in
cluster → voxel resolve; reintroducing 64 orders at pixel layer merely re-integrates same energy.
Voxel and pixel layers both **full-resolution, f32 evaluation**, output precision unaffected (reason §7.2).

### 4.2 Sun / sky real-time rotation

Sun direction is Pass 1 uniform, rotation triggers no rebake or resource rebuild: just rerun Pass 1 each frame.
Pass 1 **processes all clusters every frame**, no temporal decimation of "reuse previous frame when delta below threshold" —
that causes per-cluster jumps during rotation. Smoothness guaranteed by **constant per-frame cost**, not by skipping computation.

### 4.3 Acceptance

| Item | Threshold | Measurement |
|---|---|---|
| `EvaluatePRT64` full chain | **≤ 0.30 ms** @ 1080p, Snapdragon 7+ Gen 2 | `VK_EXT_host_query_reset` timestamps, Pass 1–4 sum; scope §7.1 |
| Static sun / sky rotation | No visible stutter (P99 frame time ≤ scene median × 1.2) | `PRISMA_BENCH_REPORT` P50 / P99 |
| 1080p full screen | Render resolution == output resolution, 1:1, no upsampling | Resolution assert + per-pixel golden image |

## 5. M3: Dynamic lights and anti-light-leak

### 5.1 Cluster-Major propagation

- **Full propagation**: `64×32×64` light cluster grid (aligned with GI `64³`), max 64 lights per cluster;
  on overflow **error and abort this frame's dynamic-light propagation**, don't silently truncate by intensity — cutting lights makes brightness jump.
- Propagation runs on **all** clusters, no sparse skip by "whether lit": skipped clusters keep previous frame value,
  leaving trails behind moving lights. Sparsity only from storage layout (cluster-major contiguous segments), not compute culling.
- Each cluster propagation is 6-neighbor weighted average × 64 coefficients (`workgroup = 1 cluster`, 64 threads one coefficient each).

### 5.2 Fixed-point `InterlockedAdd`

Mobile doesn't guarantee float32 atomics (`shaderBufferFloat32Atomics` missing or slow on some Adreno / Mali),
so **accumulation always via `uint32` fixed point**:

- Range: Q8.12 per channel (`[0, 16)`, resolution `2^-12`), packed as `R|G|B` segments;
- Accumulate: `InterlockedAdd(key, value)`, three `uint32` per cluster;
- Readback: Pass 3 decodes uniformly and saturates, overflow doesn't wrap (`min(sum, 0xFFFF)`).

**This is sole implementation**, no float atomic branch.

### 5.3 Anti-light-leak (20 cm thin wall)

Three mechanisms, none can be missing:

1. **Leaf doesn't cross wall**: 64-tree split criterion (a) ensures cluster AABB lies on only one side of wall, eliminating "sharing a cluster" at source.
2. **Resolve gating**: during Pass 3 splatting, if line from voxel center to nearest cluster falls on wall backside (using cluster pre-stored
   `single_sided` flag + normal consistency between voxel and cluster center), reject that cluster's contribution, weight zeroed not attenuated.
3. **Weight gating**: during voxel sampling apply `max(w - leak_bias, 0)` normalization to trilinear weights,
   `leak_bias` default `0.05` (adjustable via `settings.ini`). This is **correction suppressing wrong contributions**,
   not quality reduction: only removes cross-wall weights that shouldn't exist, not legitimate ones.

### 5.4 Acceptance

| Item | Threshold | Measurement |
|---|---|---|
| 16 dynamic point lights, Pass 1–3 total | **≤ 0.15 ms** | GPU timestamp segments; 16 lights all on, all in frustum |
| Thin-wall light leak | Indirect luminance on wall dark side ≤ **2%** of bright-side mean, and **no visible gradient** | golden image: 20 cm wall, equidistant cameras both sides, per-pixel compare |
| Light overflow | Explicit error abort when per-cluster light list exceeds 64, no silent truncation | Error count and log assert |
| Trailing | Residual luminance after moving point light ≤ 1% of steady state | Moving-light sequence frames, per-pixel compare |

---

## 6. M4: Streaming load and stability

- **Block granularity**: 64-tree `level = 2` nodes (`world_extent / 16`, ~31 m in 500 m scene) as streaming unit,
  each unit independent cluster payload segment, node table resident.
- **Queue**: `prism-asset`'s `async-io` feature handles IO; main thread only `enqueue`,
  copy done on transfer queue, main thread no per-block blocking wait. Streaming only decides whether block "exists",
  not "how accurate" — loaded block precision and resolution don't change with streaming budget.
- **Budget and eviction**: `MemoryBudget` = `min(VRAM budget, settings.ini cap)`, LRU evicts non-visible blocks;
  eviction returns to pool at frame end, no `vkFreeMemory` mid-frame (avoid device-lost trap in [lessons-learned.md](lessons-learned.md)).
- **Stability**: `PRISMA_BENCH_REPORT` baseline + 30-minute resource count comparison.

### Acceptance

| Item | Threshold | Measurement |
|---|---|---|
| Block-switch main-thread allocation | **< 0.2 ms** (GC / alloc total) | Frame-scope allocation counter, P99 |
| Continuous 30 min | No leak (`vk::DeviceMemory` allocation count and RSS monotonically non-increasing) | Sample every 60 s, last 10 min slope ≤ 0 |
| Frame-rate drift | P99 frame time drift ≤ 10% relative to minute 1 | Report comparison |

---

## 7. Acceptance scope and measurement methods

This section only defines **under what scope and by what means each metric is measured**. Metric numbers come from milestone tables,
targets not predictions derived by this file.

### 7.1 Scope definitions

| Metric | Scope | Measurement |
|---|---|---|
| M1 end-to-end time | `bake-prt` process wall clock, excludes first `scenes.pak` build, driver init and shader compilation | Tool-internal timing report |
| M1 output size | `.prtbin` on-disk total bytes (header + node table + payload) | File size; `leaf_count` assert fails first |
| M2 `EvaluatePRT64` | Pass 1–4 GPU timestamp sum, 1080p, **full-resolution 1:1 sampling** | `VK_EXT_host_query_reset` segmented timing |
| M2 rotation smoothness | Frame-time distribution under continuous sun / sky rotation | `PRISMA_BENCH_REPORT` P50 / P99 |
| M3 16 dynamic lights | Pass 1–3 GPU timestamp sum, 16 lights all in frustum and visible | Same, bucketed per pass |
| M3 thin-wall leak | Indirect luminance ratio of equidistant cameras both sides of 20 cm wall | golden image per-pixel compare |
| M4 block-switch overhead | P99 of main-thread frame-scope allocations | Allocation counter (non-GC language, counts allocator calls) |
| M4 long stability | 30 min continuous memory and frame-time drift | Sample every 60 s, compare first / last intervals |

**Only measurement scope**: all metrics at full resolution, 1:1 sampling, f32 arithmetic.
No "reduced-quality scope" vs "high-quality scope" dual numbers; when metric misses target change algorithm or representation, not sampling rate or precision.

### 7.2 Why evaluating 64 coefficients at cluster granularity

This is **structural** tradeoff, not quality reduction, independent of any estimates:

- **Read amplification**: per-pixel evaluation reads full transport vector per pixel, read volume grows with pixel count;
  at cluster granularity read volume grows only with cluster count, decoupled from screen resolution. This is data-layout layering —
  math solved at both granularities is identical.
- **Frequency domain**: irradiance is low-frequency, high-order energy already integrated in cluster → voxel resolve;
  reintroducing 64 orders at pixel layer merely re-integrates same energy.
- **Conclusion**: solving 64 coefficients at cluster granularity, 2nd order at voxel and pixel layers is correct representation layering;
  voxel and pixel layers both full-resolution f32, output not degraded by layering.

### 7.3 Quality red lines (explicitly not doing)

Following means **never used** in this design, even if performance gains:

| Means | Why not |
|---|---|
| Half / reduced-resolution resolve | Indirect-light edges blur, conflicts with 1:1 sampling scope |
| Depth-aware upsampling, temporal accumulation denoising | Introduces TAA-style trailing and ghosting |
| f16 for arithmetic and intermediate signals | f16 only disk format and transport-vector storage format, promoted to f32 on read |
| Skipping clusters by delta (temporal decimation) | Per-cluster jumps when rotating sun |
| Culling clusters or lights by distance / visibility | Loses bounce contributions outside frustum, perceptible quality loss |

### 7.4 Quantities requiring real measurement confirmation (no preset values)

- Actual per-Pass cost at full resolution (Pass 4 main);
- BLAS VRAM occupancy and feasible triangle ceiling for 500 m city — for M1,
  **VRAM may become constraint before time**, concrete threshold by measurement.

### 7.5 Items to confirm before implementation but not blocking format and bake phase

1. **M2 budget**: if 0.30 ms unreachable at full-resolution scope, discuss algorithm or representation adjustment first,
   **don't trade sampling rate or precision for meeting target**.
2. **M1 scene ceiling**: confirm triangle budget for 500 m city, deciding whether BLAS needs block build.

---

## 8. PR breakdown (each PR independently verifiable, diff < ~500 lines)

| # | Branch | Deliverable | Verification |
|---|---|---|---|
| **A** | `feat/prt-format` | `prism-render::prt_volume`: `.prtbin` codec, f16 quantization, `leaf_count` assert, round-trip unit test | `cargo test -p prism-render`; size assert |
| **B** | `feat/prt-bake-cluster` | `bake-prt` subcommand: sampling + PCA / k-means clustering + 64-tree build + export (reuse `bake_gi` infra) | Sponza runs through; report JSON validated |
| **C** | `feat/prt-bake-budget` | Size / time budget gates and 500 m city synthetic scene fixture | `bake-prt` budget assert in CI |
| **D** | `feat/prt-eval-pass` | Slang `prt.slang` (`EvalPRT64Cluster`) + Pass 1 / 3 / 4 into Render Graph + new `gi_mode` value | A / B compare with existing probe volume; GPU timestamps |
| **E** | `feat/prt-dynamic-lights` | Pass 2 full propagation + Q8.12 fixed-point `InterlockedAdd` + Proxy OBB culling | 16-light benchmark; trailing and overflow asserts |
| **F** | `feat/prt-leak-fix` | Leaf one-sided criterion + resolve gating + `leak_bias` boundary clamp | 20 cm thin wall golden image |
| **G** | `feat/prt-streaming` | 64-tree block async load / unload + `MemoryBudget` + frame-budget queue | 30 min stability + P99 allocation |
| **H** | `feat/prt-bench` | `PRISMA_BENCH_REPORT` adds `gi.*` segment fields and scope annotations | Report schema unit test |

Dependency order: A → B → C → D → E → F → G, H parallel. **Once A's format contract merged, later PRs may only extend version number,
not rewrite field semantics in place** (direct version bump allowed before 1.0, no compat reads).

---

## 9. Boundary with existing designs

- **No new Render Graph resource categories**: PRT all in `SceneScope` (node table / cluster payload) and
  `FrameScope` (`gi_volume` double buffering), see DESIGN §6.1.
- **No duplicate SH evaluation**: Rust and Slang implementations of 64-coefficient basis functions must be generated from same table
  (follow lock-step discipline of `gi.rs` and `gi.slang`), locked by round-trip unit test.
- **No float atomics**: see §5.2.
- **No change to `ForwardPass` composition formula**: PRT and probe volume output same `E(n)/π` dimension,
  `scene_frag.slang` composition line unchanged (DESIGN §6.5).
