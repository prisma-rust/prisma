# Prisma RTX neural rendering integration design (DLSS / ray reconstruction / frame generation)

> Goal: under the premise of **not breaking cross-platform unity and capability-driven degradation**, integrate NVIDIA RTX neural rendering trio into RenderGraph:
> DLSS-SR (upscaling), DLSS-RR (ray reconstruction), DLSS-G (frame generation).
> Reference implementation: [NVIDIA-RTX/RTXPT](https://github.com/NVIDIA-RTX/RTXPT) (nvrhi + Donut, DX12 / Vulkan dual backend).
> **Only borrow its data contract, not code structure**: RTXPT is C++ / nvrhi, this engine is ash direct Vulkan, the two have no reusable code at pass orchestration layer.

This document is an **L2 subsystem contract**: defines guide buffer graph resource handles, formats, production/consumption relationships, and milestone splitting.
Implementation details take code as authority; this document is updated synchronously with code in the same PR.

---

## 0. Design constraints and decisions

| Item | Decision | Rationale |
|------|----------|-----------|
| Access layer | **Streamline 2.x dynamic loading** (modeled on `crates/prism-render/src/anf.rs` `AnfBinding` function pointer table) | Coexists with `DlssMode` enum and `dlss` feature flag; machines without SDK have zero impact |
| Platform branches | **Do not write `#[cfg(target_os)]`**, probing concentrated in `crates/prism-render/src/capabilities.rs` | DESIGN.md §2.1 / §2.5 hard constraint |
| Failure behavior | Missing SDK / non-NVIDIA / driver too old → degrade step by step to TAA, **no silent black screen** | Same pattern as ANF's `AnfCaps::probe` |
| Order | **Complete PT's guide buffer first, then connect SDK** | Guide is DLSS-RR's input contract; without guide there is no RR, nor NRD fallback |
| Reject | Do not weld NGX native API into `pt_pass.rs` | Streamline already encapsulates SR / RR / G + Reflex, NGX is lower, non-portable layer |
| Reject | Do not make "DLSS-dedicated render path" | One pipeline for all platforms: DLSS is an **optional replacement before `PostPass`**, not a branch |

---

## 1. Status inventory (dev@18707482)

**Already present**

- `crates/prism-render/src/render_graph.rs:37-55` fixed graph resource handle table: `FORWARD_DEPTH_H`=1000, `FORWARD_NORMAL_H`=1001, `FORWARD_COLOR_H`=1002, `PT_COLOR_H`=1003, `SHADOW_MAP_H`=1004, `TAA_COLOR_H`=1005, `FORWARD_VELOCITY_H`=1006, `ANF_COLOR_H`=1007.
- `crates/prism-render/src/render_graph.rs:251-261` already defined `DlssMode` (Off / Quality / Balanced / Performance / UltraPerformance / Dlaa), `RenderSettings::dlss_mode` default `Off` (`render_graph.rs:307-308`). **Only enum, no implementation.**
- `crates/prism-render/Cargo.toml` already declares empty feature `dlss = []` (default off).
- `crates/prism-render/src/capabilities.rs:31` `RayTracingCaps` already does 4-layer ray tracing probing, `capabilities.rs:165` `rt_extension_names()` provides pattern to enable extension names on demand.
- `crates/prism-render/src/taa.rs` complete TAA (3×3 neighborhood clamp, reproject, velocity priority, reactive mask), `assets/shaders/slang/taa.slang:99-137` is its shader.
- `crates/prism-render/src/forward_pass.rs` raster path already outputs velocity (`FORWARD_VELOCITY_H`), normal.w used as transparent / reactive marker.
- `crates/prism-render/src/anf.rs:42-57` `AnfCaps::probe` + `AnfBinding` dynamic loading, ready template for Qualcomm branch (SR + FG dual capability bits).
- `crates/prism-render/src/dummy.rs` `SolidImage` provides 1×1 constant image, used to keep descriptor layout unchanged when feature off.

**Missing (what this design supplements)**

1. `PathTracePass` only publishes `PT_COLOR_H` (`pt_pass.rs:1490-1492`), **does not produce depth and motion vectors** → PT mode has no reprojection basis, TAA blocked in PT mode by `post.rs:962-967` `!is_pt` hard gate.
2. No roughness path (only normal, and `.w` already occupied by reactive).
3. PT shading has no path-space layering (diffuse / specular mixed into same accumulator), cannot output layered radiance and hit distance needed by RR.
4. No Streamline / NGX access.

---

## 2. Degradation ladder

Capability-driven degradation, high to low, each level must **automatically** hold when missing upper-level capability:

```
DLSS-RR (NVIDIA + Streamline + driver supports RR)
   ↓ no RR
NRD (open source, license-free, AMD / Intel / NVIDIA universal) + DLSS-SR
   ↓ no DLSS
TAA (already present, taa.rs)
   ↓ TAA off
Direct output (PostPass reads PT_COLOR_H / FORWARD_COLOR_H)
```

ANF is a branch on the same ladder on Qualcomm Adreno (SR + FG), not participating in above order, decided separately by `AnfCaps`.

**Key decision**: NRD and DLSS-RR share the same guide buffer (§3). Therefore graph resources produced by PR1–PR3 are not bound to any NVIDIA SDK; on AMD / Intel consumed by NRD, on NVIDIA consumed by DLSS-RR. This is the landing method of "one pipeline, multi-platform" principle in this feature.

---

## 3. Guide buffer contract

New handles from 1008 onward (after `render_graph.rs:55`). **Format once defined is ABI**: changing format equals changing shader and all consumers.

| Handle | Value | Format | Producer | Consumer | Milestone |
|--------|-------|--------|----------|----------|-----------|
| `PT_DEPTH_H` | 1008 | `R32_SFLOAT` (NDC depth, same semantics as `FORWARD_DEPTH_H`) | `PathTracePass` | TAA / NRD / DLSS-RR | PR1 |
| `PT_VELOCITY_H` | 1009 | `R32G32_SFLOAT` (screen-space UV displacement, same unit as `FORWARD_VELOCITY_H`). Chose 32-bit because `R16G16_SFLOAT` is not in Vulkan mandatory `STORAGE_IMAGE` format table, while this pass uses compute write | `PathTracePass` | TAA / NRD / DLSS-RR / DLSS-G | PR1 |
| `PT_NORMAL_ROUGHNESS_H` | 1010 | `R16G16B16A16_SFLOAT` (view-space normal.xyz + roughness.w) | `PathTracePass` | NRD / DLSS-RR | PR2 |
| `PT_DIFFUSE_H` | 1011 | `R16G16B16A16_SFLOAT` (diffuse path radiance) | `PathTracePass` | NRD / DLSS-RR | PR3 |
| `PT_SPECULAR_H` | 1012 | `R16G16B16A16_SFLOAT` (specular path radiance) | `PathTracePass` | NRD / DLSS-RR | PR3 |
| `PT_HIT_DISTANCE_H` | 1013 | `R16_SFLOAT` (main hit distance, RR's `kBufferTypeDepth` pairing) | `PathTracePass` | DLSS-RR | PR3 |

Conventions:

- **No format version number**. Before 1.0 zero compatibility (`docs/versioning.md`): if format changes, change handle format and all consumers, no dual-format branches.
- Handles declared via `create_resource_at` in `PathTracePass::setup` (`pt_pass.rs:1290-1314`); `execute` manually barriers + `set_image_view` publishes, consistent with existing `PT_COLOR_H` writing method.
- **Resize must cover new images**: `pt_pass.rs:718-762` `resize_images` currently only rebuilds accum / sampleCount / output three groups, new images must be added together, otherwise resolution change holds old-size image.
- When feature off, use `dummy.rs` 1×1 constant image as placeholder, ensuring descriptor layout does not change with switch (also holds when `dlss = []` off).
- **Normal space = view space, roughness = perceptual value**. Basis: `sl_dlss_d.h:83-87` `worldToCameraView` / `cameraViewToWorld` explanation says DLSS-RR `kBufferTypeNormals` expects camera space; `ePacked` mode (`sl_dlss_d.h:53-59`) puts roughness into normal's w channel, exactly this table's layout. Roughness writes `SurfaceMaterial.roughness` (already clamped to `0.04..1.0` art-space value), **do not** square into GGX `alpha`: `path_integrator.slang` only squares inside `ndf_ggx` etc. functions. RTXPT follows equivalent route of "world normal + real view matrix" (`PrepareGuideBuffers.hlsl:54`), both valid, this repo continues existing `scene_frag.slang:509-514` view-space G-buffer ABI.
- Main ray miss pixels write `float4(normalize(-view_ray_dir), 0.5)` (a fake plane facing camera), consistent with RTXPT `PrepareGuideBuffers.hlsl:56` `float4(-normalize(rayDir), 0.5)`: background has no surface, but all-zero normal would make denoiser treat it as degenerate plane.

---

## 4. Pipeline topology

```
                    ┌─ raster ─ ForwardPass ──────────────┐
Camera ──▶ TLAS ───┤                                        ├─▶ [AA / upscale / recon] ─▶ PostPass ─▶ Present
                    └─ ray tracing ─ PathTracePass ─────────┘        │
                                          │                            ├─ DLSS-RR  (NVIDIA + SL)
                                          ├─ PT_COLOR_H ──────────┐    ├─ NRD      (universal)
                                          ├─ PT_DEPTH_H           ├────┼──────────▶└─ TAA      (already present)
                                          ├─ PT_VELOCITY_H        │
                                          ├─ PT_NORMAL_ROUGHNESS_H│
                                          ├─ PT_DIFFUSE / SPECULAR_H│
                                          └─ PT_HIT_DISTANCE_H ───┘
```

Registration position follows existing order (`graph_renderer.rs:722-743`): `ForwardPass → GtaoPass → AnfPass → RtScheduler → PathTracePass → TaaPass → AutoExposure → PostPass`.
`TaaPass` scheduled after `PathTracePass` (PR1 adjustment): resolve needs to consume depth / velocity guide written by path tracing, registration order is execution order.
New neural rendering pass inserted **after `PathTracePass`, before `AutoExposure`**, i.e. "color already finalized, before exposure and tonemap" position — DLSS requires linear HDR input, after `AutoExposure` is not legal.

---

## 5. Milestones

Each PR independently mergeable and rollbackable, diff controlled within ~500 lines (`AGENTS.md` hard rule).

### PR1 — PT outputs depth and motion vectors (no NVIDIA dependency)

- Add `PT_DEPTH_H` / `PT_VELOCITY_H`; `pt_render.slang` adds two `RWTexture2D` writes at tail of `ptMain` (before accumulation writeout) (set 0 binding 15 / 16), and uses new set 0 binding 17 (`PtGuides` UBO: `viewProj` + `prevViewProj`) to provide two-frame projection matrices on CPU side. Not using push constant because `PtPush` already occupies 176 bytes, `pt_pass_tests.rs` and `push_constant_sizes.rs` pin it as contract.
- Depth: `float4 clip = mul(view_proj, float4(hit_pos, 1.0)); depth = clip.z / clip.w;`, same semantics as raster `D32_SFLOAT`; main ray miss writes 1.0.
- Motion vectors: use **jitter-removed** current `view_proj` and previous frame `prev_view_proj` to project `hit_pos` twice, `velocity = uv - prevUv`; miss writes 0.
- Previous frame matrix uses **set 0 new UBO (binding 17, 128 bytes: cur_view_proj + prev_view_proj)**, overwritten each frame by `PathTracePass` via mapped pointer (single host-visible buffer + CPU-side caches previous frame matrix, no double buffering like `taa.rs`, because this UBO is only read inside dispatch). **Do not expand push constant**: `PT_PUSH_RANGE_SIZE = 176` (`pt_pass.rs`) already close to `max_push_constants_size` common ceiling, increasing further would hit `context.rs` 176 check.
- Unlock `post.rs:962-967` `!is_pt` gate, allowing PT mode to also read `TAA_COLOR_H`.
- Acceptance: Sponza PT mode with TAA enabled, static image no ghosting, camera movement no dragging; `cargo test -p prism-render --test taa_gpu` passes.

### PR2 — Normal + roughness packing

- Add `PT_NORMAL_ROUGHNESS_H`; PT shader writes view-space normal and perceptual roughness.
- **Do not expand push constant**: `PtGuides` UBO expands from 128 bytes to 192 bytes, adding `worldToView` matrix. CPU side converts from `GraphFrame::inv_view_rot` (transpose of view rotation), so shader writing is fully consistent with `scene_frag.slang`'s `mul((float3x3)frame.view, worldNormal)`.
- Note `FORWARD_NORMAL_H.w` already used by TAA as reactive (`taa.slang:113`), **cannot** stuff roughness into it — this is why this PR adds a new image rather than reusing G-buffer.
- **Raster side MRT deferred to PR5**: DLSS-RR only consumes path tracing output, now adding an MRT to `ForwardPass` would immediately pay full-resolution RGBA16F bandwidth with no reader. Wait until PR5 truly connects RR then supplement based on raster G-buffer shape at that time (may merge with ReSTIR GI's `FORWARD_ALBEDO_H` into one surface image).

### PR3 — Path-space layering

- `path_integrator.slang` accumulates `throughput * radiance` into diffuse or specular two accumulators by first-bounce lobe (judgment uses existing BRDF branch since `SurfaceMaterial` already has metal-roughness).
- Add `PT_DIFFUSE_H` / `PT_SPECULAR_H` / `PT_HIT_DISTANCE_H`; `PT_COLOR_H` still writes total (Post direct output path unchanged).
- Layering doubles accumulator bandwidth, mobile needs `RenderSettings` switch outside `capabilities.rs` (default follows `dlss_mode != Off || nrd`).

### PR4 — Streamline dynamic loading layer (first connect DLSS-SR only)

- New `crates/prism-render/src/dlss.rs`, shape following `anf.rs`: `DlssCaps { available, sr, rr, fg }` + `probe(properties, driver_version)` + `DlssBinding` function pointer table, load failure returns `None` instead of panic.
- **Vulkan-specific difficulty**: Streamline wraps `vkCreateDevice` / `vkCreateSwapchainKHR` / `vkQueuePresentKHR` via interposer. ash uses direct `vkGetInstanceProcAddr`, so after `context.rs` creates instance **replace GIPA with SL's**, and swapchain + present (`swapchain.rs`'s `Swapchain::present`) must walk interposer-returned function pointers. This is PR4's entire risk; first verify DLSS-SR path, then expand RR / G.
- Each frame `sl::setTagForFrame`: color / depth / velocity / jitter / `inv_view_proj` / `prev_view_proj` / `sl::kBufferTypeScalingOutputColor`.
- `DlssMode` maps to SL quality enum; `Dlaa` = output size same as input.

### PR5 — DLSS-RR (ray reconstruction)

- Five signals connected per RTXPT contract: `kBufferTypeScalingInputColor`, `kBufferTypeAlbedo` (diffuse), `kBufferTypeSpecularAlbedo`, `kBufferTypeNormals` + `DLSSDNormalRoughnessMode::ePacked`, `kBufferTypeSpecularMotionVectors`, plus `kBufferTypeTransparencyLayer` and `kBufferTypeDepth`.
- **Exact channel order of packing mode must be verified against `sl_dlss_d.h` `ePacked` comments before starting**, do not write from memory; verification results backfilled to this document §3 table.
- RR input depth / motion vectors need **copies**: RR modifies their scale (RTXPT's `EnsureRRDepthAndMotionVectors()` is exactly for this copy), cannot directly feed `PT_DEPTH_H`.

### PR6 — DLSS-G (frame generation)

- Needs present support "submit once output N frames": `swapchain.rs`'s `Swapchain::present` currently single `queue_present`, needs to be changed to SL-driven and read back `numFramesActuallyPresented`.
- Needs `inputsProcessingCompletionFence` and UI / tone mapping separation layer (HUD must be correctly composited on interpolated frames).
- Depends on PR4 / PR5 stability; FG failure mode (screen tearing / stuttering) is much more severe than RR.

---

## 6. Verification

- Unit: `pt_pass_tests.rs` push / push-constant contract tests must continue passing (PR1 doesn't touch push for this reason).
- GPU: `cargo test -p prism-render --test taa_gpu` (PR1), `golden_frame` (PT color unchanged).
- Manual: Sponza `SPONZA_RENDER_MODE` three states (raster / pt_off / default PathTrace + SHaRC) each run one pass, DLSS each level and Off switched one by one, confirm degradation does not black screen.
- Panel: `projects/demos/sponza/src/tuning_ui.rs` already has a `DLSS Mode` cycle (Off / Quality / Balanced / Performance / UltraPerformance / Dlaa), this line only takes effect after PR4.

---

## 7. Explicitly not doing

- No DLSS DX12 backend (this engine only has Vulkan).
- No RTXPT `nvrhi` / Donut port (framework layer incompatible, zero benefit).
- No implicit behavior of "force on when NVIDIA detected": `dlss_mode` default `Off`, must be explicitly turned on.
- No compatibility branch retaining old guide format (zero compatibility before 1.0).
