# Prisma Path Tracing Repair and Optimization Implementation Spec

Date: 2026-11-13. Status: Proposed, not yet implemented or performance accepted.
Author: JasonGu's AI agent (GPT-6.1 Sol / OpenCode).

## 1. Goals, Scope, and Definition of Done

Using `projects/sponza` as the main regression scene, fix path integration, material hits, and SHaRC contract, establish a credible performance baseline, then optimize ray, texture, and scheduling costs. Results must be reusable across the engine, standalone Player, and editor Game View.

Deliverables in three tiers:

1. **Must complete**: reproducible baseline, material classification, visibility query correctness, SHaRC radiance and weight fixes, independent training, history invalidation rules.
2. **Must evaluate and document trade-offs**: single-light fast path, texture LOD, AS lifecycle, thread groups, cache and output bandwidth.
3. **Future extensions**: temporal reconstruction, denoising, sparse sampling, SER, OMM, RT pipeline, async queues. These must not be dependencies of the base fix.

Base completion is not defined by any FPS number. Correctness evidence, same-condition GPU/CPU time, VRAM and cache statistics must be provided together; no claim of major bottleneck or N× speedup without measurement. 60 FPS / 16.67 ms is only a valid target when bound to specific GPU, resolution, and quality tier.

Not in base scope: rewriting ECS, migrating to D3D12, introducing editor dependencies, NRC SDK integration, enabling full path tracing on mobile by default.

## 2. Reference Versions and Evidence Boundaries

| Local repo | HEAD at investigation | Use |
|---|---|---|
| Prisma | `d1423c2d0072a1618f946c71c6df7427d2625183` | Current implementation |
| RTXGI | `04372d7f91bb3a0444f9fd95d3f34381ea4e3b39` | Concise SHaRC update / resolve / query reference |
| RTXPT | `303b408ab81790a5051abd7573830b7a4112eb8e` | v1.9.0 pipeline, ray cones, material visibility, training & resolve |

Investigation reads workspace files; HEAD does not represent all local changes. RTXGI has local modifications and submodule state changes; when producing formal comparison reports, record dirty state of all three repos and relevant patch summaries. This document did not run third-party same-condition benchmarks, nor captured Nsight.

Locations below are relative paths in corresponding repos; line numbers are for snapshot positioning only:

| Observation | Local evidence | Conclusion strength |
|---|---|---|
| Prisma BLAS not set OPAQUE, TLAS not forced opaque | `crates/prism-render/src/acceleration_structure.rs:444`, `bake_common.rs:290` | Confirmed; time share TBD |
| All non-opaque candidates run UV / alpha test, threshold hardcoded 0.5 | `assets/shaders/slang/pt_render.slang:640` | Confirmed |
| Current GPU material and upload input have no alpha mode / cutoff field | `crates/prism-render/src/managers/material_manager.rs:58,111` | Confirmed; material domain cannot be inferred from texture alpha |
| Indirect hit accumulates emission / NEE first, then queries cache | `pt_render.slang:619–721` | Confirmed; double-counting needs verification against local cache definition |
| UpdateHit return value ignored, PT has no SetThroughput call | `pt_render.slang:721`, corresponding interface in `sharc_common.slang` | Confirmed contract gap |
| Query and sparse update in same shader, update path can query then exit | `pt_render.slang:279–292,707–721` | Confirmed; register cost TBD |
| ReSTIR fixed 8 candidates, attempts time with 4-neighbor reuse | `pt_render.slang:34–35,309–349` | Confirmed; candidate evaluation is not equal number of shadow rays |
| Material texture fixed SampleLevel 0 | `assets/shaders/slang/path_integrator.slang:315–388` | Confirmed; sampling cache and bandwidth impact TBD |
| Static scene upload bakes vertices into world space, triggered by missing geometry | `crates/prism-render/src/graph_renderer.rs:2700–2759` | Should not be misreported as per-frame rebuild; dynamic updates need audit |
| Sponza default dev command may run debug | `scripts/dev.sh:25,72–74`, `projects/sponza/Cargo.toml` | Configuration fact; does not prove build user ran |

Local glTF statistics: 262,267 triangles, 25 materials (22 OPAQUE, 3 MASK); this is source model statistics only; formal report must also record runtime loaded package, submesh, instance, and material counts.

## 3. What to Adopt from RTXPT

| Reference | Position | Engine decision |
|---|---|---|
| Unified path integration and reference mode share transport implementation | `Docs/Design.md:30–40` | Keep shared BSDF / material / sampling logic, establish no-cache reference mode |
| Independent low-resolution training, then resolve | `Rtxpt/Sharc/SharcIntegration.cpp:397–510` | Independent compute training and consumption variants; no need to connect RT pipeline first |
| Cache query returns radiance from that vertex, caller avoids duplicate emission | `Rtxpt/Shaders/PathTracer/PathTracerSHaRC.hlsli:255–298` | Define local contract first, then adjust integration order, do not mechanically copy fields |
| Update handling return value and per-segment throughput | Same file `359–395`; RTXGI `Pathtracer.hlsl:757,903–906` | Unify units, weights, termination rules |
| Visibility any-hit first-hit; alpha reads only necessary data | `PathTracerBridgeDonut.hlsli:117–142`, `DonutBridgeCommon.hlsli:85–122` | Simplify alpha hit path, set flags per real material domain |
| Ray-cone texture sampling | `PathTracer/Utils/TextureSampler.hlsli:90–142`, `TexLODHelpers.hlsli` | Add shading texture LOD in phases; alpha independently verified |
| Static BLAS reuse, dynamic geometry refit, rebuild inputs | `Docs/Design.md:48–50,130–136` | Adopt lifecycle principle; do not copy "TLAS rebuilt every frame" as default |
| Main PT unifiable, training / resolve still phased | `Docs/Design.md:104–136` | Splitting training is not changing wavefront; whether to further split PT decided by profiler |

RTXPT's live main config includes DLSS-RR, optional low internal resolution, and sparse pixel tracing; README performance numbers are not engine benefit predictions. Its Vulkan SER / OMM are still listed as in-progress (`README.md:68–75`), D3D12 async DLSS overlap cannot be directly applied to this Vulkan backend.

Reference algorithms and copying source code / SDK integration are reviewed separately. RTXPT root license is NVIDIA RTX SDKs LICENSE; if porting source code or SDK, first record specific files and submodule licenses, attribution, and distribution obligations; do not assume the whole repo is MIT.

## 4. Architectural Contracts That Must Be Maintained

- `prism-render` holds Vulkan resources and passes; `prism-engine` maps author settings and derived state; projects only provide scenes and game logic.
- Keep Player UI-framework-free and editor-free; do not pull cooker / SDK toolchain into player dependency tree.
- Keep right-handed, camera −Z, column-major, Vulkan Y flip, depth [0,1]. Motion vectors must separately state units, direction, jitter convention.
- New passes implement `RenderPassNode`, declare resource read/write and lifecycle in graph. No additional "invisible" training or resolve outside the graph.
- Push constants generated via Slang reflection + `shader-bindgen`; GPU POD size, offset, alignment must be verifiable.
- Devices not supporting ray query or SHaRC atomic capability keep existing raster / baked GI fallback. When capability insufficient, record valid mode; must not show "enabled" while silently not executing.
- `docs/DESIGN.md:88` "real-time SHARC removed" contradicts §12.2 current feature table; before implementation, sync authoritative docs, clarify boundary between mobile baked GI and optional desktop PT+SHaRC, revise old blueprint's "update/query same pipeline" suggestion.

## 5. Baseline and Observation Spec (Phase 1)

### 5.1 Two Comparisons Must Be Separate

Algorithm cost comparison: Release, fixed camera / lights, same actual geometry and textures, same internal size (prefer 1920×1080), 1 spp, aligned bounce semantics, disable upscaling / frame generation / sparse sampling / denoising; test cache off/on separately. Sun angular radius, environment, alpha, backface, transmission, RR, and exposure must also be recorded. Items that cannot be aligned must list differences explicitly; cannot call strictly equal.

Product effect comparison: allow reconstruction and upscaling, record actual rendered pixels, spp, GPU ms, output size, visual quality, and reconstruction latency. Display FPS after frame generation cannot be used to prove path tracing cost reduction.

Sponza Release launch baseline:

```bash
cargo run --release --manifest-path projects/sponza/Cargo.toml
```

Before running, build scene package per project assets manifest, confirm actual loaded package paths and summary. `settings.ini` build profile cannot substitute `--release`. Run only one baseline app at a time.

### 5.2 Sampling and Reporting

- Fixed seed and frame sequence; record GPU, driver, API, clock / power state, adapter, binary and shader summary, compile optimization, validation, output / internal size, effective settings.
- Warmup default 120 frames, sample 600 frames, at least 3 independent runs; also test cold cache and cache convergence curve, do not test only static screen after infinite accumulation. If 120 frames not yet stable, extend per pre-defined conditions and record.
- Reuse `PRISMA_BENCH_REPORT` distribution and `GpuTimer`; new data versioned or written as sidecar report; CPU frame time cannot be labeled GPU time, mark missing when timestamps unsupported.
- GPU intervals must cover at least AS, training, resolve, main PT, history / reconstruction, post-processing, and full frame; segments are not full-frame critical path. All out-of-graph work must also be covered; insufficient timing capacity must not silently omit items.
- Statistics primary / scatter / shadow rays, non-opaque candidates, alpha fetch, average path length, per-bounce cache eligible / query / hit, training path and write success rate, hash occupancy / collisions / insert failures, training / resolve cost.
- Statistics use sampling or diagnostic variants; formal performance sampling turns off high-frequency global atomics; record instrumentation overhead, empty statistics vs unsupported distinguished.
- Reports retain p50 / p95 / p99, frame distribution, VRAM peak / resident, cold start AS / upload time, screenshots and raw samples. When repeated run fluctuation exceeds 3%, explain noise first.

### 5.3 Scene Matrix

Sponza must include sunlit atrium, shadow corridor, near-field perforated, far-field high-frequency texture, quick turn, camera switch, sun movement, light switch, asset reload. Plus micro-scene: opaque with transparent texture, different cutoff MASK, double-sided leaves, backface and mirror transform, BLEND and transmission, pure mirror, emissive surface, no light / single light / multiple lights, repeated instances and dynamic objects.

## 6. Material Domain, Alpha, and AS Fixes

### 6.1 Data Chain First

Audit glTF import → intermediate data → cooker / RMAT → runtime material → engine upload → GPU material → PT geometry. Explicitly save OPAQUE / MASK / BLEND, alpha cutoff, double-sided; do not infer domain from "texture has alpha" or whether alpha value is 1. Transmission does not equal alpha blend.

Existing `GpuMaterial` / `MaterialUploadInput` do not have complete fields; must first write data format and API design. If RMAT needs upgrade, define version, old package defaults, reject / compatibility strategy, and rebake path; prohibit silently changing format using undocumented reserved fields. Old materials must not all be blindly set opaque; first investigate existing semantics, retain explicit compatibility path if necessary.

### 6.2 Traversal Rules

- OPAQUE geometry sets `vk::GeometryFlagsKHR::OPAQUE`, does not execute alpha texture test; MASK stays non-opaque, uses corresponding cutoff and base color alpha factor.
- Classify by geometry / submesh; mixed-material instances cannot use full-instance FORCE_OPAQUE; BLAS shared key must include material classification affecting build.
- BLEND select and document valid behaviors for transmission / stochastic transparency / not yet supported; cannot treat as MASK or simple opaque.
- Clearly define single-sided / double-sided and inverse-scale flags; cannot enable backface culling uniformly for performance.
- Shadows only need binary occlusion when using ACCEPT_FIRST_HIT_AND_END_SEARCH; retain full Proceed / commit flow, MASK holes continue traversal. Shadow distance correctly excludes light source itself; transmitted shadows handled per separately defined semantics.
- Alpha helper only reads indices, UV, alpha material parameters, and necessary textures; does not require normal / tangent / full surface. Whether eliminated by compiler must be checked on actual shader.

Acceptance: pure opaque micro-scene alpha candidate / fetch is zero; mixed scene MASK holes, cutoff, double-sided results correct; opaque texture alpha does not change occlusion; CPU classification test, GPU screen comparison, and validation all pass. Cannot pass by forcing all materials opaque.

## 7. Path Integration and SHaRC Correctness

### 7.1 Unified Radiance Contract

First define for shared integrator: local radiance `L(x)`, camera accumulated throughput `beta`, single-scatter weight `w=f*cos/pdf`, emission, NEE, environment miss and cache value units. Verify per function whether existing BSDF already includes cosine, avoid double multiplication.

This spec chooses cache to represent "remaining outgoing radiance from this vertex"; which emission / direct items included must be uniquely determined in implementation and tests. Initial implementation does not do material demodulation; if enabled later, must simultaneously define encoding / query and cache clear rules.

Consumption path: construct hit info and validity → query cache → hit then only accumulate `beta * L_cache` and terminate → miss then accumulate agreed emission, NEE, sampled BSDF and continue. If local SHaRC can only provide cache missing some item, first revise contract and corresponding position; do not mix RTXGI / RTXPT version assumptions.

Training path: compute local lighting → `SharcUpdateHit` and handle false → advance SHaRC throughput by single-segment scatter weight → continue. Prohibit multiplying camera accumulated beta again into local sample or repeatedly pushing historical weights. miss must propagate environment; after returning false, do not send invalid scatter rays.

Shared checks include RR survival probability and weight compensation, bounce limit meaning, emissive hit and NEE MIS, delta light sampling discrete probability, transmission branch probability / refraction weight, geometric / shading normal, self-intersection offset. The above items are audit targets; not proven to have bugs item by item.

### 7.2 Tests and Reference

Establish no-cache reference variant of shared transport, close ReSTIR, culling, and denoising biased items, output linear HDR. First verify analytic micro-scenes, then use it to check cache; do not let two co-source errors certify each other.

Must test: constant environment / white furnace energy, two-segment throughput (e.g. 0.5 per segment) propagation, pure emission, single directional light, cache hit no longer accumulates local NEE / emission, UpdateHit false termination, miss propagation, delta path not replaced by coarse cache, NaN / Inf and empty hash.

SHaRC is biased; cannot require pixel-exact match with no-cache. Initial quality threshold: agreed diffuse ROI relative average brightness error ≤ 5%, no NaN / Inf, no stable double counting; complex materials set separate thresholds. Reference sample count must converge to statistical uncertainty below acceptance tolerance.

## 8. Independent SHaRC Training, Resolve, and Consumption

Suggested graph order:

```text
Scene/AS ready → optional cache clear → SharcTrain → barrier
              → SharcResolve → barrier → PathTraceConsume → history/post
```

Reuse Vulkan RayQuery compute; share hit, BSDF, light code; compile into no-cache / train / consume three controlled variants. Consumption variant does not carry training propagation array; training does not use screen ReSTIR history. Training can stop per SHaRC's own resampling rules; cannot directly inherit consumption path's cache termination.

Training initial downscale uses 5, also evaluate 4 / 8; use `ceil(width/d)`, boundary check, and per-block sample positions varying by frame, avoid fixed-pattern sampling long-term missing geometry. Set training bounce and capacity budget; record actual path count per sampling scheme.

Explicitly declare transfer clear → compute, train atomic write → resolve read/write, resolve write → consume read, and cross-frame cache access dependencies. Multi-frame parallel must not, when training next frame, overwrite cache still being consumed; first use same-queue explicit sequence; double-buffer / timeline scheme must write memory and latency cost.

Lifecycle: resize updates training size and screen history; camera movement clears screen accumulation / reprojection history but usually retains world cache; scene unload, geometry / material semantic change, cache format change clear or partially invalidate. Light change defines hard clear or bounded refresh, report tracking latency; changing exposure must not directly contaminate linear radiance cache. Freeze training, resolve, and query switch combination behavior must be explicit.

Acceptance: three stages all have GPU intervals; consumer has no training writes; non-divisible resolution, hot reload, camera switch, scene switch no out-of-bounds / stale handles; same-budget compare cold / warm cache quality, hit rate, and total cost. Cannot claim success only by raising hit rate at picture quality expense.

## 9. Light Sampling and ReSTIR

Zero light / single light take simple NEE; only skip reservoir when valid sampling domain truly has only one analytical light and no emissive / environment needs sampling. Multi-light retain switchable baseline, evaluate cost and equal-time error for candidate counts 1 / 2 / 4 / 8.

Current time reuse uses last frame reservoir, but cannot assume reliable reprojection based on that; must audit camera / object motion, depth / normal / material rejection, light ID stability, invalidation, weight normalization. Before effective history rejection completed, mobile scenes default to reliable non-time-reuse path.

Emissive and environment sampling included in unified domain / PDF design, handle NEE and BSDF hit MIS; do not hardcode bypass just for Sponza's sun. RTXPT NEE-AT / RTXDI are only candidates for later multi-light schemes; base stage does not introduce new SDK.

Acceptance: single-light path reservoir read/write is zero and result unbiased; multi-light static and moving both match reference within statistical tolerance; fewer candidates must be compared by equal-time image error, cannot just report faster.

## 10. Texture LOD, Data Access, and AS Lifecycle

### 10.1 Shading textures

Initialize ray cone from camera footprint, propagate with distance and scattering, combine triangle UV / world-space area and texture size to compute bounded, mip-range-limited LOD. Cover non-uniform scaling, degenerate UV, no mip, normal map; verify upload and `.pak` indeed contain mip chain. RTXPT shading uses ray cones, but its alpha test still samples mip 0; cannot claim it solves all alpha LOD.

First step keep existing alpha visibility mip strategy, only optimize shading textures; later separately evaluate coverage-preserving alpha mip or conservative approach. Do not directly use coarse mip for MASK and accept leaf disappearance.

Measure texture cache / bandwidth, LOD distribution, far-field flicker and near-field detail; "change all to fixed coarser mip" does not count as complete.

### 10.2 Geometry and build

First use counting to verify current static scene has no duplicate upload / BLAS / TLAS builds. Gradually change from per-instance world-space copy to shared object-space BLAS + TLAS transform; shared key includes mesh / submesh, topology, and opacity classification. Transform only updates TLAS; topology change rebuilds BLAS; deformation only then evaluates refit and post-degeneration rebuild.

Whether splitting position / UV-only traversal data is worth it decided by VRAM, load, and AS build measurement; do not blindly rewrite generic Vertex. Static compaction completes asynchronously; old BLAS / TLAS must wait for GPU use to end before destruction; record scratch, peak, and resident bytes; prohibit steady-state `device_wait_idle`.

Acceptance: static 600-frame count has no unexplained rebuild; repeated instances increasing does not linearly repeat geometry storage; dynamic / mirror scaling / material domain switch / hot reload correct; initialization and steady-state costs reported separately.

## 11. Shader, Bandwidth, and Scheduling Optimization

After correctness stabilizes, compare 8×8, 16×8, 16×16 etc. thread groups, record actual registers, spill, occupancy, warp/wave active ratio and trace / memory stalls. Prohibit judging need for wavefront solely by shader length. First let consumption path remove training state, then consider splitting visibility / shading or material variants, and limit permutation count.

Accumulation keeps FP32 reference; displaying intermediate output can evaluate FP16, reservoir can evaluate compact layout; each time record actual resource size and error. Taking 1080p as example, a single RGBA32F is ~31.6 MiB; capacity alone does not equal actual bandwidth. Prohibit using compressed precision to mask integration errors.

Only retain candidates whose performance evidence exceeds noise and quality does not degrade; no-benefit thread groups, compression, or pass splits recorded and abandoned after. Audit CPU prepare / extract / submit and duplicate allocation, use existing packet reuse; GPU optimization does not automatically solve debug CPU bottleneck.

## 12. Subsequent Reconstruction and Hardware Extension

Reconstruction first defines general input: linear HDR, depth, geometric / shading normal, roughness, albedo, reliable motion vectors, previous frame transform, jitter, exposure, and reset. Background, invalid motion, disocclusion reveal, mirror / transmission must have clear strategies; using only camera motion is insufficient to cover dynamic objects.

First form a verifiable Vulkan generic reconstruction interface, then evaluate NRD / DLSS-RR. Dependencies, licenses, features, device support, DLL / SDK distribution, and Android fallback designed separately; feature name existing does not mean SDK integrated. Reconstruction and path integration quality measured separately.

Sparse sampling and dynamic resolution only enabled after history reconstruction, disocclusion, and valid sample count are correct. OMM evaluated only when remaining MASK alpha cost is obvious and extension / toolchain available; SER / RT pipeline only initiated after profiler shows reordering benefit and Vulkan / Slang support clear; async compute requires measured overlap work, queue sync, and memory budget. All keep basic RayQuery path.

## 13. Implementation Splitting and Dependencies

The following are suggested review units; each defaults under 500 lines, exceeding split further by data contract, implementation, and tests; prohibit combining the whole spec into one big PR. Entering master each task independent branch / PR; this spec does not authorize submission, push, or release.

| Order | Delivery unit | Main ownership / entry | Precondition | Acceptance |
|---|---|---|---|---|
| 1 | Baseline manifest, interval timing, diagnostic counts | `gpu_timer.rs`, render graph report, `prism-app` bench, Sponza docs/scripts | None | Reproducible baseline, timing no omissions |
| 2 | alpha / RMAT / API contract and version scheme | asset / cooker format docs, material schema | 1 | Compatibility and migration rules reviewable |
| 3 | Import / bake / runtime alpha parameter threading | `prism-cooker`, `prism-asset`, engine asset, material manager | 2 | Roundtrip and old package test |
| 4 | Opaque geometry, simplified alpha, visibility flags | `acceleration_structure.rs`, `bake_common.rs`, PT helpers | 3 | Material matrix screen and counts |
| 5 | Shared transport reference variant and energy test | `path_integrator.slang`, PT host, micro-scene | 1 | Reference convergence and parse test |
| 6 | SHaRC units, weights, return value, double-count fix | `pt_render.slang`, SHaRC common | 5 | Propagation and cache ROI acceptance |
| 7 | Independent training shader and no-training consumption variant | SHaRC shaders, compile / reflection / bindings | 6 | Three variants compile correctly, resource layout verified |
| 8 | In-graph train / resolve / consume and history lifecycle | `sharc.rs`, `graph_renderer.rs`, graph settings | 7 | Sync validation, multi-frame / reset test |
| 9 | Single-light fast path and reservoir history rule | PT light helpers, history state | 4, 5 | Single light zero reservoir, multi-light no bias |
| 10 | Ray cones and mip chain verification | material helpers, PT state, texture pipeline | 5 | LOD / quality / bandwidth report |
| 11 | AS sharing and dynamic invalidation | mesh manager, AS, geometry sync | 4 | Static reuse, dynamic / reload verification |
| 12 | Thread group, format, and CPU hot spot experiment | PT, output resources, frame preparation | 8–11 | Only retain effective benefits |
| 13 | Contrast report and authoritative doc sync | `docs/DESIGN.md`, mobile GI doc, Sponza README | 12 | Complete baseline and limitation handover |
| Subsequent | Generic reconstruction / SDK / hardware optimization each spec | render traits, platform capability, host | Base closed loop | Independent review and acceptance |

New public API, resource format, or dependency must be recorded in corresponding design doc before coding. Do not implement all functions first then add format specs. Rendering settings use existing INI / engine mapping; diagnostic parameters must not infinitely expand into public API.

## 14. Quality Thresholds, Verification, and Rollback

Suggested initial performance threshold: same quality, final Sponza warm cache p50 GPU time relative to recorded baseline reduced by at least 20%, p95 regression no more than 3%; this is an engineering goal, not a measured conclusion. Each optimization benefit must exceed repeated-run noise, and hardware and raw / equal-quality mode reported separately. Correctness fixes that increase cost must be itemized separately; cannot revoke correct results to meet targets.

SHaRC training + resolve + consumption total time compared with cache-off, simultaneously report image error and deeper bounce benefit; cannot only compare consumption time. Until performance goal met, retain correctness fixes, record residual hotspots and next phase; do not fake benefit by reducing lighting complexity.

Implementation proceeds per change:

```bash
python scripts/pr_ready.py
cargo test -p prism-render --features rtxgi
cargo test -p prism-asset -p prism-cooker
cargo check -p prism-render --no-default-features
cargo check --manifest-path projects/sponza/Cargo.toml --release
```

Independent editor / template workspace verified per involved scope; player dependency budget checked with existing repo scripts. Shader modifications must use repo-pinned Slang 2026.21, update `.spv`, if reflection changes generate per `compile.sh --write-reflection` + shader-bindgen formal process, run binding layout / resource budget validation; prohibit hand-editing JSON or push struct.

GPU verification covers validation, sync, resize, exit, scene switch, raster↔PT, GI switch, no RT fallback; without actual Android real-device evidence, must not claim mobile verified. Screenshot tool screenshots cannot replace linear HDR quality metrics.

Each unit handover must include: implemented / not implemented, entry files, commands and results, run manifest, before/after GPU and image evidence, known risks. Base fixes retain diagnosable switches; format upgrades retain clear reader / migration strategy; performance candidates independently revertible. GPU resource release depends on completion signal; rollback must not leave invalid descriptors, stale cache ABI, or expired `.spv`.

## 15. Current Delivery and Next Steps

Landed following units per §13 split (local worktree, not committed):

| Unit | Status | Landed content and evidence |
|---|---|---|
| 1 | Implemented, missing baseline data | `gpu_timer.rs` adds full-frame interval (span query, `frame_or_total_ms`, `unmeasured_ms`), HUD and bench changed to read it, new HUD / interval tests |
| 2, 3 | Implemented | RMAT v2 carries `alpha_mode` + `alpha_cutoff`; import, cooker, asset, engine upload, `GpuMaterial` (112 bytes) full chain connected; old version decode branch deleted |
| 4 | Implemented | `BlasBuildParams.opaque` drives `GeometryFlagsKHR::OPAQUE`; PT uses `mat_alpha_pass` real threshold; shadow rays add `ACCEPT_FIRST_HIT_AND_END_SEARCH` |
| 6 | Implemented | Cache query advanced before NEE, hit terminates segment; `SharcUpdateHit` returns false terminates, per-segment calls `SharcSetThroughput` |
| 9 | Implemented | Sun single light `restir_on = false`: main hit directly samples sun, `prevReservoir` / `currReservoir` (including sky pixel zeroing write) fully not accessed; multi-light still uses original ReSTIR |
| 5 | Implemented | `pt_reference.slang` (`PT_MODE=1`): no cache, no ReSTIR, no firefly clamp, uniform light sampling, linear HDR output; `PathTracePass::set_variant` / `GraphRenderer::set_pt_variant` selectable; offscreen scene actually dispatched, reference average brightness vs consumption path < 20% assertion passed. Linear HDR readback: `RenderGraph::request_hdr_readback` / `take_hdr_readback` grabs `PT_COLOR_H` before tonemap (`hdr_readback.rs`, `HdrReadback` provides `pixel` / `mean_rgb` / `max_rgb` / `has_non_finite`), `GraphRenderer` same-name method forwards. Analytic micro-scene suite `crates/prism-render/tests/pt_analytic_gpu.rs` 6 cases all pass |
| 7 | shader three variants implemented, in-graph connection not done | `pt_render.slang` derives consumption / reference / training via `PT_MODE`; `pt_sharc_train.slang` (`PT_MODE=2`) only writes cache, no query, no screen output; three `.spv` and reflection JSON all generated, push constants all 176 bytes, binding tables consistent (unit test assertions). Training variant hit record: `SharcHitData`'s `positionWorld` / `normalWorld` originally only filled inside `if (sharc_on)`, but training variant `sharc_on=false` (`PT_MODE_TRAIN` only opens `sharc_update`), so `SharcUpdateHit` took uninitialized coordinates as normal key — deposited everything into random voxels, consumer end never hits. Changed to fill under `if (sharc_on || sharc_update)`, training and query use same key |
| 8 | Implemented | Order: each frame inside `GraphRenderer` by `SharcTrain` (`PathTracePass::dispatch_training`, 1/downscale, ceil round up) → cache buffer handoff → barrier → `SharcResolve` → barrier → consumption path execution; consumption path no longer trains (`sharc_flags` update bit constantly 0). Two stages each have independent GPU interval names `SharcTrain` / `SharcResolve`. Lifecycle: world cache and screen history separated — `resize_offscreen` / `recreate_swapchain` only clears screen history, world cache retained via reprojection; `SharcInvalidation { geometry, lights, sky }` records reasons needing whole cache invalidation (`invalidate_pt_geometry`, `set_environment`, `release_assets` released resources, frame dirty plan's `lights_dirty` / `sky_dirty` / `mesh_releases` / `material_releases`), voxel scale change separately flagged via `sharc_cache_scene_scale` epoch; reasons retained across SHaRC idle frames, consumed and cleared on first PathTrace frame back. Exposure and camera movement do not clear world cache |
| 10, 11, 12, 13 | Not started | ray cones, AS sharing, thread group and format experiments, contrast report |

Verification records (2026-11-13):

- `bash assets/shaders/compile.sh`: reflection consistent with submitted JSON, spirv-val all pass; added `pt_reference.comp.spv`, `pt_sharc_train.comp.spv` and two reflection JSONs, `shader-bindgen` generated corresponding Rust modules.
- `cargo test -p prism-render --lib`: 288 passed; `cargo test -p prism-asset -p prism-cooker`: 139 / 204 passed.
- `cargo test -p prism-render --features rtxgi --test rtxgi_sponza_golden`: passed. rtxgi build asserts GPU interval contains `SharcTrain`, proving training is independent dispatch and separated from consumption. Original assertion stopped here, but "interval opened" does not equal "dispatch really happened" — `GraphRenderer::execute` unconditionally `begin_pass`, and `PathTracePass::dispatch_training` inside interval silently returns upon any precondition. Changed to assert renderer's own recorded `sharc_train_dispatches()` / `sharc_resolve_dispatches()` greater than 0 (`dispatch_training` changed to return `anyhow::Result<bool>`, only returns `true` if actually recorded `record_dispatch`).
- `cargo test -p prism-render --test rtxgi_sponza_golden` (default feature, rtxgi off): passed; SHaRC-related assertions skipped per feature, reference variant and training variant rendering still executed.
- Unit 9 equivalence A/B: temporarily fixed `restir_on = true`, reran same offscreen scene, compared with fast-path image average brightness 2.621 vs 2.631 (difference 0.38%), per-pixel difference comes from two paths consuming random number order differently (1 spp noise), no whole-screen darkening or doubling additive bias.
- SHaRC training key fix empirical evidence (`assets/shaders/slang/pt_render.slang` `sharc_hit` initialization): same offscreen scene, same camera, ran 16 frames consecutively, `gi_mode = Off` vs `gi_mode = Sharc` per-pixel comparison — before fix 4096 pixels all bit-identical (cache zero hits), after fix `sharc_scene_scale = 5` had 67 pixels rewritten by cache, `scene_scale = 50` (factory value) had 6. This also explains why relying only on GPU timing `SharcTrain` / `SharcResolve` interval names cannot prove training really happened: when interval opened, `dispatch_training` may still be silently early-returned by guard.
- SHaRC dispatch count (eliminating empty-turn assertion): `PathTracePass::dispatch_training` now returns `anyhow::Result<bool>`, `GraphRenderer` maintains `sharc_train_dispatches` / `sharc_resolve_dispatches` two `u64` counters (with `#[cfg(feature = "rtxgi")]` `GraphRenderer::sharc_train_dispatches()` / `sharc_resolve_dispatches()`). `pt_analytic_gpu.rs` cache cases added two-way comparison: `gi_mode = Off` 16 frames must keep both counters at 0 (renderer just built, not yet entered SHaRC branch, so this 0 is exact; it simultaneously rules out "counter constantly 1" bad practice), `gi_mode = Sharc` 16 frames must let both increment. `rtxgi_sponza_golden.rs` four-frame accumulation likewise asserts both greater than 0.
- Unit 5 analytic micro-scene `cargo test -p prism-render --features rtxgi --test pt_analytic_gpu`: 6 cases all passed (white furnace linear: 0.25 and 0.5 tiers each whole-image uniform and average ratio 2.000; two-segment throughput: `(albedo + 0.0256) * 0.375` parsed value vs center pixel relative error < 5%, bounces 2 and 4 results consistent; pure emission: center equals material `emissive * emissive_strength` and three channels same, outside box 0; single directional light: `(albedo/π + 0.1024·0.25/π)` relative error < 5%, intensity 1 vs 2 pixel ratio 2.00; degenerate empty hash: `sharc_capacity = 0` no NaN/Inf and no energy amplification; cache energy: `gi_mode` off/on two runs whole-image average brightness difference < 5%, and at least one pixel rewritten by cache, proving query truly hit). They use `take_hdr_readback` linear HDR, not tonemap 8-bit screenshot — latter would compress 0.25/0.5 into 0.19/0.31.
- Unit 8 lifecycle test `cargo test -p prism-render --features rtxgi --test sharc_lifecycle_gpu`: 5 cases all passed, covering resize not invalidating (`resize_offscreen(192,160)` before/after clear count unchanged), light change clears once and not repeated, env change clears once, voxel scale change clears once, and "reasons registered in raster frame retained across frames, consumed on first PathTrace frame back". `cargo test -p prism-render --lib`: 292 passed; `cargo test -p prism-engine --lib dirty_router`: 7 passed (`light_view_proj` no longer participates in light dirty determination, since derived from camera eye, otherwise player walking would clear world cache every frame).
- `python scripts/pr_ready.py --quick` (unit 8 completion time): pre-existing failures unrelated to this work line in `editor/crates/prism-editor-core/src/build.rs` etc. three formatting, `crates/prism-cooker/src/aseprite.rs` `useless_vec` clippy; at the time shader bindings and budget (17 modules) passed. Afterward parallel TAA-on-PT work line added `guides` UBO and `guideDepth` / `guideVelocity` sampling images to `pt_render` / `pt_reference` / `pt_sharc_train`, shader budget now reports 9 violations (`sampled_images 6 > 4`, `uniform_buffers 1 > 0`, `descriptors 21 > 18`), needs that work line to decide whether to raise `assets/shaders/budgets.json`.

Uncompleted manual acceptance: Sponza HUD GPU time, MASK perforated appearance, SHaRC screen vs single-light screen visual comparison. Units 6, 8, 9 changed visible results, must be confirmed by human under `bash scripts/dev.sh sponza --release`; this time did not collect before/after GPU milliseconds, performance benefit not yet quantified.

Next step: bring `SharcTrain` / `SharcResolve` into `RenderPassNode` (currently still manually orchestrated inside `GraphRenderer::execute`, out-of-graph execution). Unit 5 required analytic micro-scene covered by `crates/prism-render/tests/pt_analytic_gpu.rs`. In-graph connection hard block explored: `GraphResources` only holds Vulkan handles and `u32` scalars (`params`), `RenderPassNode::execute` also cannot get other passes, so "SharcTrain pass directly calls `PathTracePass::dispatch_training`" cannot be realized via existing in-graph handoff; either add arbitrary-type slots to `GraphResources`, or make training / resolve each become real passes on independent forward / backward dependency chains.
