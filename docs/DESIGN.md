# Prisma Design Goals and Design Principles

> This file is Prisma's **authoritative design blueprint (source of truth for intent)**.
> Code changes, but the constraints here cannot be violated. Before adding rendering
> features, passes, or platform support, first check alignment with this document.
> Specific feature designs are in the split documents under `docs/`
> (e.g. `mobile-raytracing-gi-design.md`).

## 1. One-sentence positioning

Prisma is an **experimental game engine unified across platforms**. Its rendering pipeline is
high-performance and high-quality: desktop and mobile share the same RenderGraph; path differences
come from build-time bricks and runtime capability probing, not platform branches; platform-specific
capabilities are compiled independently and loaded on demand, degrading wholesale when missing.
The pipeline always tracks the latest graphics APIs / SDKs and dependency versions on each platform;
components with an LTS channel use the latest LTS.

## 2. Seven core design goals

### 2.1 Unified across platforms: one pipeline, all platforms

- **One pipeline, all platforms**: desktop (x86_64 Vulkan) and mobile (aarch64 Vulkan) share the
  **exact same RenderGraph definition**; differences come only from runtime capability probing (2.6)
  and build-time brick selection (2.5), **no platform branches**.
- **Modularization = pass is node**: each render stage (GBuffer / Shadow / RayQuery / SHARC GI /
  Lighting / Post) is an independent `RenderPassNode`, combined via `RenderGraphBuilder`.
  New feature = new pass node, no changes to existing nodes.
- **Resource and rendering decoupling**: source formats (glTF / PNG / HDR) are imported offline by
  `prism-cooker` and cooked into RTEX / RMES / RSCN; runtime reads via `prism-asset`'s `.pak` +
  `ResourceManager`; GPU-side `RenderMaterialManager` / `BindlessTextureTable` are in `prism-render`.
  The engine crate does not depend on specific resource formats.
- **New platform integration** only implements the Vulkan backend and window/input adaptation,
  without changing pass and shader main paths.

### 2.2 High performance: bandwidth and budget are hard constraints

- **TBDR-friendly is the default constraint**: pass structure is designed for tile memory; mobile
  is not an afterthought port target.
  - Passes use **transient attachment / lazy allocation** (mobile tile memory) between them,
    avoiding full-resolution intermediate textures shuttling back and forth in system memory.
  - Strictly minimize **load/store ops**; intermediate RT uses `DONT_CARE` when possible.
  - Avoid cross-tile random global memory access (bindless large array access patterns must be controllable).
  - Half-resolution shadows / GI and other heavy passes default to downsampling (see `RayQueryPass`'s `scale = 0.5`).
  - Shader resource usage has a build-time limit (`assets/shaders/budgets.json` + `xtask`'s
    `shader-budget`): if a pass quietly adds a few more sampled textures or push constants grow to
    hundreds of bytes, self-check fails rather than waiting for mobile to choke.
- **No backporting old architecture**: there is no compromise of "making do for old renderer compatibility".
  The old monolithic `renderer.rs` (995 lines) has been torn down; new code all goes through
  **RenderGraph + pass nodes**. Any "temporarily stuffed into legacy_renderer" approach is a bug
  (legacy is only transitional, the goal is complete removal).

### 2.3 High quality: quality tier determined by capability and budget

- The same scene covers the quality range from mobile baseline to desktop ceiling **within one
  pipeline**, switched via `RenderSettings` and capability probing, without producing a parallel
  second implementation:
  - GI: baked probe volume → raster ReSTIR GI → SHARC → path tracing.
  - Shadows: raster shadow maps → RayQuery soft shadows → `None`.
  - Upscaling and denoising: built-in TAA as baseline; DLSS / DLFG / NRD (NVIDIA) and ANF (Adreno)
    as optional bricks.
- **Quality is accepted on reproducible evidence**: `PRISMA_BENCH_REPORT` P50 / P99, per-pass GPU
  timing, headless screenshots or `PRISMA_SCREENSHOT`, not subjective "looks better".
- Heavy passes may sacrifice resolution for frame rate, but may not sacrifice pass structure unity.

### 2.4 Experimental: leading-edge technology first, replacement allowed

- The pipeline's primary purpose is to validate leading-edge rendering techniques (path tracing,
  SHARC, ReSTIR GI, PRT, neural upscaling / frame generation, virtual geometry), not to stay in
  conservative implementations for "stability".
- Each technology enters as an independent brick (2.5): failure can disable the whole brick without
  affecting the default path; experimental technology must not become a hard dependency of the default path.
- Zero compatibility burden before 1.0: if technology hasn't converged, replace the design; no shims,
  old-format fallbacks, or dual implementations; docs describe only the latest form.

### 2.5 Platform features independently compilable

- **Capability = brick, `Cargo.toml` = blueprint** (§12): a platform feature is a feature; turning it
  off generates no code and links no dependencies; mutually exclusive combinations fail explicitly
  via `compile_error!`, not silently retained.
- **Every feature must be independently disabled and still compile**: `scripts/check_features.py`'s
  matrix enforces this rule; new features must add a minimal combination.
- Platform-specific implementations (DirectWrite, Streamline / DLSS, `libanf.so`) can only appear
  inside bricks, not in common paths; common paths must not have `#[cfg(target_os)]` branches
  deciding rendering behavior.

### 2.6 Hot-pluggable: capability present means enabled, missing means degraded

- At engine startup, **probe** `VkPhysicalDevice` Vulkan version, available extensions, and features
  such as descriptor-indexing / ray-query / dynamic-rendering / transient attachment, then decide
  which path to enable; probe logic is centralized and testable, not scattered across passes.
- **Vendor runtime loaded on demand**: Streamline (DLSS / DLFG / NRD) and `libanf.so` (Adreno SR / FG)
  are enabled if present, degrading to built-in TAA or full-resolution rendering if missing; the
  cross-platform unified binary does not crash.
- **Runtime switching**: GI mode, shadow mode, upscale tier controlled by `RenderSettings`; mid-tier
  GPU turns off RT / GI, the architecture does not deform because of it.
- **Pass registration boundary**: the goal is to add or replace passes by registration only, without
  changing core. Currently `GraphRenderer::new` still hardcodes the order of four passes;
  `PluginRegistry` has only type definitions and has never been constructed (§8.4) — this is a known
  gap, not design intent.
- What is swappable at runtime is **capability and implementation**, not cross-ABI dynamic library
  passes: the latter needs resource and lifecycle contracts, and is not implemented before contracts
  are defined.

> **Shadow implementation status (2026-08-21)**: currently implemented **single rasterized shadow map**
> (`ShadowMapPass` depth pre-pass + `ForwardPass` comparison sampler sampling,
> see `shaders/slang/shadow_depth.slang` / `scene_frag.slang`). `RenderSettings::shadow_mode`
> supports `Auto`/`Raster`/`RayQuery`/`None`, automatically selected by `resolve_shadow`
> based on `VK_KHR_ray_query` capability. `ShadowMapPass::SHADOW_CASCADE_COUNT = 1` is reserved.
> Texture edge length is determined by `RenderSettings::shadow_map_size`
> (`settings.ini [render] shadow_map_size`, default 2048, clamped to 256..=8192, #526); it takes
> effect when constructing the renderer / rebuilding the render graph; changing the field at runtime
> does not reallocate images.
>
> **Plan (CSM)**: Cascaded Shadow Maps not yet implemented, only a single fixed-range orthographic shadow.
> Next step: split into multiple cascades inside `ShadowMapPass` by camera frustum slices, and select
> cascade by distance in `scene.frag.slang::sample_shadow` — constants and interfaces reserved, not in this MVP scope.

### 2.7 ECS boundary

Prisma ECS is a **deterministic simulation core**, not a general-purpose ECS framework:

- **Storage**: type-indexed sparse set (`prism-ecs`), components are pure data, entities carry generation.
- **Scheduling**: single-threaded execution in registration order; `update` and `fixed_update` schedules.
- **Parallelism**: only through explicit batched jobs (`jobs.rs` scoped chunks), no system-level automatic parallelism.
- **Change tracking**: handled by each system's own dirty flags (e.g. render side's `DirtyDispatchPlan`),
  no built-in change tick, no observer / hook.

Trade-offs are clear: gain readability, debuggability, low structural change cost; pay with cache
locality for large queries and less system-level parallelism than archetype + parallel scheduling.
When the latter two are needed, first gather scale data proving the current implementation can't
cope, then design separately, no rewrite before 1.0.

### 2.8 Version strategy: follow latest platform API / SDK / dependency

- **Target latest**: rendering paths target each platform's current latest graphics API, vendor
  extensions, and SDK; components with an LTS channel take **latest LTS** (e.g. Android NDK LTS,
  Linux graphics stack LTS), not stuck on expired versions for "stability".
- **Toolchain**: Rust via `stable` (`rust-toolchain.toml`); Slang takes latest release, CI pins
  a concrete version via `SLANG_VERSION`; upgrades recompile all shaders in one batch, update
  `assets/shaders/compile-stamp.json`, and re-run binding drift; SPIR-V validation tool also takes latest.
- **Dependencies**: third-party crates take latest available; `Cargo.lock` is committed, upgrades
  run the full dependency budget (`check_deps.py`), feature matrix (`check_features.py`), and tests.
- **No old-version compatibility**: no second implementation or version branch for old drivers / old SDKs;
  capability gaps degrade via probing (2.6), not `if version < X`.
- **Upgrade is routine**: dependency / toolchain / platform SDK upgrades enter as standalone commits;
  per AGENTS §1, dependency / toolchain / workspace-level changes must run the full gate.

## 3. Derived constraints (hard rules derived from goals)

| Rule | Rationale |
|------|-----------|
| No `target_os` / `target_arch` platform branches deciding render path | 2.1 / 2.6: platform differences absorbed by capability probing |
| New render features must be `RenderPassNode`, not stuffed into legacy renderer | 2.1 / 2.4: drop historical baggage |
| Intermediate RT default `DONT_CARE` store + transient/lazy allocation | 2.2: TBDR bandwidth |
| Heavy passes (shadow / GI / reflection) default half-resolution | 2.2: mobile bandwidth / compute |
| All cross-platform layouts (push constant, UBO, SSBO) explicitly padded and verified | Consistent ABI across platforms |
| Shadow / GI / RT / debug view unified under `RenderSettings` switch | 2.3 / 2.6: degradable without deformation |
| Source formats (glTF / textures) imported/cooked at build time by `prism-cooker`, runtime reads `.pak` via `prism-asset`, engine does not read source files directly | 2.1: decoupling |
| Platform-specific capabilities must live in independent feature bricks and compile standalone | 2.5: independently compilable |
| Vendor runtime (Streamline / `libanf.so`) must not be a hard link dependency; missing must degrade | 2.6: hot-pluggable |
| Experimental tech must be fully disableable in `RenderSettings` without dragging the default path | 2.4: experimental |
| ECS must not rewrite archetype / automatic parallel scheduling / change tick without scale data | 2.7: determinism first |
| Platform API / SDK / dependency follow latest; LTS channel takes latest LTS, no locked EOL versions | 2.8: version strategy |
| No version branches for old drivers or old SDKs; capability gaps always degrade via probing | 2.8 / 2.6: no version compatibility |
| Toolchain versions centralized in `rust-toolchain.toml` and CI `SLANG_VERSION`; upgrades submitted in same batch as shader recompile | 2.8: reproducible upgrades |

## 4. Current architecture mapping (design goal vs. current state)

| Design goal | Current state |
|-------------|---------------|
| Modular pipeline | `prism-render/src/render_graph.rs` (`RenderPassNode` graph) + `forward_pass.rs` / `shadow_map_pass.rs` / `skybox_pass.rs`. **Status (2026-08-21)**: `RenderGraph::execute()` uniformly drives four passes (`ShadowMapPass` -> `ForwardPass` -> `GtaoPass` -> `PostPass`, linear in registration order). Passes declare graph-edge deps via `read_usage` / `write_usage`, and the graph auto-inserts cross-pass `vkCmdPipelineBarrier`s (layout cache persistent across frames keyed by `(handle, image_index)`, `reset_layouts` on `recreate_swapchain`). Cross-frame latency edges (GTAO double-buffered AO feedback) and swapchain->`PRESENT_SRC_KHR` are kept manual, marked as graph-boundary exceptions. Cycle detection implemented (`validate_edges`); execution order not reordered (wiring order in `GraphRenderer::new`). `GraphRenderer::prepare` wired into `render_system`/`render_runner` five-stage chain (no longer a no-op); resource lifecycle intervals declared; TBDR memory aliasing left for next step. |
| Bindless / unified across platforms | `prism-render/src/bindless.rs` (separate SRV + global sampler table) |
| Resource management decoupling | Responsibilities split into three crates: `prism-cooker` (Import→Cook, build-time only), `prism-scene` (component schema + RSCN v4), `prism-asset` (Package→Runtime, `.pak` + `ResourceManager`). Engine-side `GpuAssetResolver` holds `ResourceManager`, lazily loads from `.pak` via `load_with_deps`, then uploads on the render thread via `GraphRenderer::apply_asset_requests`; runtime chain does not depend on cooker. |
| Mobile GI | **Baked probe-volume GI** (2nd-order SH, 9 coeffs × RGB, `R32G32B32A32_SFLOAT` 3D texture; float16 is later optional compression); see §6. Mobile still does not run real-time SHARC (can't fill cache with per-frame rays). Desktop has two additional baked-independent real-time paths: PT + SHARC (`gi_mode = Sharc`) and raster ReSTIR GI (`gi_mode = Restir`, see §6.8). |
| Shadow / RT | Rasterized shadow map: `ShadowMapPass` (depth pre-pass, see `shadow_depth.slang`) + `ForwardPass` (comparison sampler sampling, see `scene_frag.slang`) |
| Capability probing | `prism-render/src/capabilities.rs` (centralized probing, in progress) |
| Frame lifecycle | `GraphRenderer` provides `begin_frame` → `prepare` → `execute` → `present` → `end_frame` stages (since 2026-08-21 `prepare` actually called in `render_system` and `render_runner::render_one_frame`, no longer a no-op); old `render()` remains only as a compat facade. `prepare` currently implements dirty-mark logging and input validation; next step extends it to schedule batched uploads. Fine-grained Runtime/Plugin split still pending PR-L2. |
| Scene sync (CPU→GPU) | **Done (2026-11-10 / #1157)**. Unified `DirtyDispatchPlan` consumed in `prepare` stage, converging batched scheduling and staging uploads of textures / meshes / materials / lights / instances. `SceneReadView` extended to a complete read-only snapshot (camera, env, lights, draw items); see §9. |
| Read-only scene view | `prism-engine::scene::SceneReadView` covers camera, lights, env resource paths, and flat `DrawItem` extraction; passes depend only on the read-only snapshot. |

## 5. Anti-goals (explicitly not doing)

- **Not** maintaining a compat layer for the old monolithic renderer.
- **Not** writing two pipelines or two shader main paths for desktop / mobile.
- **Not** using platform macros instead of capability probing to decide render features.
- **Not** introducing unverified, non-degradable "all-on" hard dependencies (e.g. forcing a particular Vulkan extension).
- **Not** making ECS a general framework (archetype, system-level automatic parallelism, change tick) — revisit with scale data (2.7).
- **Not** making experimental tech a hard dependency of the default path; failure must be able to disable the whole brick (2.4).
- **Not** doing cross-ABI dynamic library pass hot-loading before contracts are defined (2.6).

---

## 6. Baked GI and RenderGraph refactor (planning)

> This section's spec drives `RenderGraph` interface design, to avoid "refactor architecture first,
> then get slapped by GI a second time".
> GI is not an independent pass; it is a diffuse indirect-light sampling branch inside `ForwardPass`;
> but it conversely requires the graph to model **three resource lifecycle classes**, which is the
> core constraint of this refactor.

### 6.1 Resource classification and lifecycle (graph must explicitly model three classes)

| Class | Lifecycle trigger | Example | Destroy responsibility |
|-------|-------------------|---------|------------------------|
| **Scene-level (scene)** | Scene / level load / unload | probe volume 3D texture, IBL env cube, material table | Scene manager (not swapchain callback) |
| **Swapchain-level (swapchain)** | swapchain recreate (resize / rotation / device-lost recovery) | ForwardPass HDR color / depth / normal MRT, allocated **per swapchain image count** | Graph recreate: drop these resources' framebuffers first, then recreate swapchain (see lessons §21, §29 device-lost warnings) |
| **Frame-level (frame)** | Per frame in-flight | AO double buffering (GTAO reads last frame, writes current, 1-frame latency), per-frame-in-flight descriptor set | Graph frame loop (by `frame_index`, not `image_index`) |

**Key trap (flagged early)**: probe volume 3D texture is **scene-level**, must never be hooked to the swapchain recreate callback. Change only on level change, not on resize. The graph needs a `SceneScope` resource table, independent of `SwapchainScope` / `FrameScope`.

### 6.2 RenderGraph interface revision

Goal: resource handle declaration + graph ownership + automatic barriers + lifecycle scope.

- `RenderPassNode::setup(&mut self, graph: &mut RenderGraphBuilder, settings)` — declare
  which `ResourceHandle`s (graph edges) to read/write **and resource scope** (scene / swapchain / frame);
  physical resources are built uniformly by the graph at `allocate_resources` / `import`.
- `RenderPassNode::execute(&mut self, ctx: &RenderContext, resources: &GraphResources)`
  — takes only command buffer + bound/queryable resource handles, **not** managing framebuffer lifecycle itself.
- Resource handle is graph-internal ID (`ResourceHandle(u32)`); pass holds no raw `vk::Image` / `vk::Framebuffer`.
- Graph topologically sorts at compile time; at runtime it per-frame deduces and inserts **automatic barriers** —
  `RenderGraphExecutor` maintains a per-resource `ResourceStateTracker`, tracking each image's
  `layout` / `access` / `stage`, and auto-inserts `vkCmdPipelineBarrier` on pass switches,
  no longer relying on explicit manual barriers inside passes. State tracking is per `(handle, image_index, layer_count)` per frame;
  all tracked state is reset on swapchain recreate.
  Phase 1 can transition first: manual dep table for ordering + explicit barriers inside passes, behavior unchanged before enabling automatic barriers.
- **Multi-camera compositing (#461)**: `render_system` enumerates all enabled cameras (`Camera.depth` ascending), each camera gets one `CameraView` (view/proj, clear color, `clear_flags`). `ForwardPass` in one render pass clears per camera via `vkCmdClearAttachments` (`Skybox`/`SolidColor` clear color+depth, `Depth` clears depth only, `Nothing` clears nothing) + redraws the scene; UBO switches view/proj via `UNORM_BUFFER_DYNAMIC` camera slot. Version 1 supports fullscreen opaque layering only, no per-camera viewport rect / post-process stack.
- **Local post-process volume (#502 Phase 2)**: `PostProcessVolume` component uses an axis-aligned box (`size` half-extents × entity scale, rotation ignored) to bound an area, with `priority` / `blend_distance` / `global` and color grading, bloom, aberration, grain, white balance params. Main thread, after writing `FramePacket.settings`, calls `scene::systems::post_process::resolve_post_grade`: weight 1 inside box, linear fade-out outside within `blend_distance`, mixing params into current frame `PostGrade` from low to high priority. No volume = bit-identical with `RenderSettings::post_grade()`; multi-camera compositing shares one post-process, so resolved only by main camera position.
- `ShadowMapPass` correctly implements `RenderPassNode`, as reference. **#526 CSM**: `shadow_cascade_count = 1` allocates old `shadow_size²` single shadow map; `= 2` allocates `2 × shadow_size` 2×1 atlas, left/right tiles are near/far cascades, pixels select cascade by camera forward depth. Atlas size determined at render graph construction; changing settings at runtime requires rebuild.

### 6.3 Pass topology (after refactor)

```
ShadowMapPass → ForwardPass → GtaoPass → RestirGiPass → PostPass
   (graph edge)   (graph edge)  (graph edge)  (graph edge)
```

- **Baked GI is not a separate pass**: it is an `if (flag(PBR_FLAG_GI))` branch inside `ForwardPass` that samples the probe volume.
- **Real-time ReSTIR GI is a separate pass** (`RestirGiPass`, see §6.8): it must read the G-buffer and trace secondary rays; folding it into `ForwardPass` would mean doing ray queries per fragment, so it becomes its own node. `ForwardPass` only samples the screen-space signal it writes on the next frame, according to `gi_mode`.
- Graph-edge contract:
  - `ShadowMapPass` writes `shadow_map` (depth) → `ForwardPass` reads.
  - `ForwardPass` writes `hdr_color` / `normal_mrt` / `depth` (swapchain-level, per image_index) →
    `GtaoPass` reads depth+normal; `PostPass` reads hdr_color.
  - `GtaoPass` writes `ao[frame]` (frame-level double buffer) → `ForwardPass` (reads on the next frame, `ao[(frame+1)%2]`, 1-frame latency).
    **The cross-frame dependency is declared by `GtaoPass::setup` as "read the previous frame's AO / write the current frame's AO", so the graph does not order GTAO before the slot it itself reads**; on the first frame the upstream view is null and the shader does not sample (PBR_FLAG_AO is off by default).
  - `ForwardPass` writes `albedo_roughness` (`SV_Target3`: rgb = base color, a = perceptual roughness) →
    `RestirGiPass` reads it to demodulate indirect diffuse.
  - `RestirGiPass` writes `restir_gi_color` (frame-level double buffer; `restir_gi_scale` sets the resolution) → `ForwardPass` (reads on the next frame, 1-frame latency, same shape as AO).
    The cross-frame latency edge is likewise "graph-edge declaration + manual barrier inside the pass": when `gi_view` is null, push constant `gi_mode = 0` and the shader falls back to the probe volume.
  - `RestirGiPass` and `PathTracePass` are the **two peer consumers** of the SHARC world-space cache: with `gi_mode = Sharc` the cache is trained and queried by PT; with `gi_mode = Restir` it is deposited and queried by `RestirGiPass` itself, and PT does not participate.

### 6.4 Baked GI data specification (PR-5 update)

- **SH order**: order 2, 9 coefficients × RGB. Each coefficient is `float32` (currently `R32G32B32A32_SFLOAT`; a later extension can switch to `float16` to save bandwidth, which is tight on mobile).
- **SH representation**: the probe volume stores **radiance SH** (not irradiance SH). The baker does not pre-convolve with a cosine; it projects incident radiance onto the SH basis by Monte Carlo integration. At runtime two evaluation paths use it differently:
  - **Diffuse** → `EvalSH9Irradiance()` applies the Ramamoorthi & Hanrahan A_l factors (A₀=π, A₁=2π/3, A₂=π/4) and returns E(n) = ∫ L(ω) max(0, n·ω) dω. The caller divides by π to obtain the Lambertian BRDF.
  - **Specular** → `EvalSH9Radiance()` reconstructs L(ω) directly from SH; the split-sum approximation replaces the prefiltered env map.
  - The same 9 coefficients serve both paths, with no extra storage.
- **/π consistency**: the IBL irradiance cubemap stores E/π (cosine-weighted average), while GI SH stores radiance (no cosine). At runtime the diffuse path normalizes both to kd · albedo · E/π, removing the old bug where GI was π times brighter than IBL.
- **Probe grid**: `origin: vec3`, `spacing: vec3`, `dims: ivec3` (grid resolution), passed to the shader through a cbuffer/UBO.
- **3D texture packing**: one 2D slice per layer (`R32G32B32A32_SFLOAT`), depth = dims.z × 9
  (one RGB layer per coefficient). Sampling uses integer `Load` plus manual trilinear interpolation, **not a hardware sampler**, to prevent cross-talk between coefficient layers.
- **Baking tool** (`prism-build-pipeline bake-gi` subcommand, not part of the runtime): multi-bounce path tracing (3 bounces + Russian roulette) runs the full bounce chain for each Fibonacci sphere direction and writes `assets/gi/probe_volume.bin` (`prism-render::probe_loader::save_probe_volume`); reading it back also lives in `prism-render` (`probe_loader::load_probe_volume` + `GraphRenderer::load_probe_volume_file`, uploading into `SceneScope`), **bypassing `prism-asset`** (which only handles `.pak`). The engine side has no call site yet, so the baked output only takes effect after manual wiring.
- **Memory budget**: order-2 SH with float32 gives 9×3×4 = 108 bytes per probe; a 16³ grid is ≈ 432KB, 32³ ≈ 3.5MB.
  Switching to float16 in a later extension gives 54 bytes per probe, 32³ ≈ 1.8MB.

### 6.5 `scene_frag.slang` changes (PR-5 update)

- **Diffuse GI**: `SampleProbeVolumeIrradiance()` → `EvalSH9Irradiance(n)` computes E(n):
  `gi_diffuse = kd_ibl · ibl_intensity · (E(n) / π) · albedo`
  replacing IBL irradiance cubemap sampling.
- **Specular GI**: `SampleProbeVolumeRadiance()` → `EvalSH9Radiance(r)` computes L(r):
  `specular_ibl = ibl_intensity · L(r) · (f_ibl · brdf_LUT.x + brdf_LUT.y)`
  replacing prefiltered IBL env map sampling. Order-2 SH is very blurry for specular reflections, but bounced light in indoor scenes is inherently low-frequency and this is still more correct than leaking the env sky.
- `PBR_FLAG_GI` (bit 14) controls the switch; `RenderSettings::gi_mode` is reused (0=Off, non-zero=On; baked has no Update state, hence only 0/non-zero).

### 6.7 SH representation change log (PR-5)

| Version | SH storage | Diffuse evaluation | Specular evaluation | /π consistency |
|------|---------|-------------|--------------|----------|
| Before PR-3 | **Irradiance SH** (cosine pre-convolved) | `EvalSH9(n)` = E(n) | no GI specular (uses IBL env) | ❌ GI is π times brighter than IBL |
| PR-3 | **Irradiance SH** | `EvalSH9(n)` = E(n) | reuses `EvalSH9(r)` as specular (irradiance misused) | ❌ missing /π + physically wrong specular |
| **PR-5** | **Radiance SH** (no cosine pre-convolution) | `EvalSH9Irradiance(n)` = E(n) with A_l factors | `EvalSH9Radiance(r)` = L(r) reconstructed directly | ✅ both paths normalized to kd·albedo·E/π |

### 6.6 Migration steps (separable PRs, each independently verifiable, CI stays green)

- **PR-1: graph resource model + ForwardPass joins the graph (no shader changes)**. Convert `ForwardPass` into a
  `RenderPassNode`; make HDR/depth/normal graph-declared swapchain-level resources; remove the manual
  set_target / set_ao / execute orchestration from `GraphRenderer::render()` and instead build one `RenderContext`
  and call `graph.execute(ctx)`. Barriers stay manual (explicit inside passes) and the graph only orders work.
  Behavior is unchanged → CI green.
- **PR-2: GtaoPass / PostPass join the graph (no shader changes)**. Same pattern: declare graph-edge dependencies,
  delete manual orchestration. The focus is verifying that the graph expresses the 1-frame-latency AO cross-frame dependency correctly.
- **PR-3: probe volume scene-level resource + `scene_frag` GI branch**. Add the `SceneScope` resource table, loader
  interface (`prism-render::probe_loader`) and the `PBR_FLAG_GI` sampling branch. GI is now connected and the graph's "three lifecycle classes" are actually consumed.

> **Ordering principle**: PR-1/PR-2 first pin down the graph interface against the needs of its consumer (GI) (§6.1's three lifecycle classes); PR-3 then connects GI for real.
> Do not guess GI's requirements by changing the interface in PR-1 (that invites a second rework).

### 6.8 Real-time ReSTIR GI (raster path, independent of PT)

> Goal: obtain real-time indirect diffuse without enabling path tracing; the SHARC world-space cache is reused, but **the trainer becomes raster GI itself**.

- **Trigger conditions**: `RenderSettings::gi_mode = GiMode::Restir` + `RenderMode::Raster` + the `prism-render/restir_gi` feature + device support for `VK_KHR_ray_query`. If any condition fails, it degrades to the baked probe volume (`ForwardPass`'s `gi_mode` push constant is 0) without error and without changing the graph structure.
- **Data flow**: `ForwardPass` G-buffer (depth / view-normal / albedo+roughness / velocity) → `restir_gi.slang` reconstructs world positions at `restir_gi_scale` (default 0.5) resolution, traces `restir_gi_rays_per_pixel` secondary rays with a cosine distribution → NEE + ambient light at hit points, sky on miss → temporal reuse (velocity reprojection + depth validation, capped by `restir_gi_max_history`) + spatial reuse (`restir_gi_spatial_samples` neighbors) → outputs **E(n)/π** (not outgoing radiance), and `ForwardPass` composites with `kd · albedo · signal` on the next frame.
- **/π convention**: the output has the same dimension as the probe volume's `EvalSH9Irradiance / π`, so both GI paths in `scene_frag.slang` share one composite formula; the ReSTIR branch only replaces the signal source, and specular still uses the prefiltered env cubemap.
- **SHARC reuse**: `RestirGiPass` receives the same three persistent buffers as PT through `set_sharc_buffers()` (hash entries / accumulation / resolved) and performs `SharcInit` / `SharcUpdateHit` / `SharcUpdateMiss` / `SharcGetCachedRadiance` inside `restir_gi.slang`. When a ray hits an already-converged voxel it takes the cached radiance and terminates early. Cache clear / resolve is still driven by `GraphRenderer`'s shared orchestration (`SharcTargets` + `SharcResolvePass`); the two consumers are mutually exclusive only over "who trains".
- **Geometry source**: `RestirGiPass::set_geometry()` receives `PtGeometryInstance` in the same shape as PT (`bake_common`) and builds BLAS/TLAS internally with `build_pt_scene()`; `GraphRenderer::prepare` calls `sync_restir_gi_geometry()` when `gi_mode = Restir` and the pass has no geometry yet, sharing `collect_rt_instances()`'s world-space vertex transforms with `sync_pt_geometry()`.
- **Switches**: `restir_gi_scale` (resolution multiplier), `restir_gi_rays_per_pixel`, `restir_gi_spatial_samples`, `restir_gi_max_history`, `restir_gi_temporal_reuse`, `restir_gi_spatial_reuse`, `restir_gi_max_secondary_dist`, passed through field by field from `prism-engine::render_settings` → `prism-app::render_runner`.
- **Invariants**: `RestirGiPass` only declares graph edges (reads 4 G-buffers, writes `restir_gi_color`) and never touches `ForwardPass`'s attachments; when the feature is off, `ForwardPass`'s set 4 binding 1 stays null and the `gi_mode` push constant is always 0.

### 6.9 PRT64 and the 64-ary tree (later phase)

> Through §6.8, this section describes the **shipped** SH probe volume and ReSTIR path. Higher-order precomputed radiance transfer
> (PRT64), 64-ary-tree chunked streaming and Cluster-Major dynamic light propagation belong to the **next phase**; their data contract
> (`.prtbin` header and cluster payload), Pass 1–4 topology, light-leak prevention, and M1–M4 acceptance criteria are all recorded in
> [`prt-gi-64tree-design.md`](prt-gi-64tree-design.md). This section does not repeat them; that document is authoritative for the spec.
> §6.1's three lifecycle classes and §6.4's `E/π` dimension constraint **continue to apply**.

*Related documents: `docs/lessons-learned.md` (§21/§29 framebuffer destruction order, §30 CI drift).*

## 7. Texture pipeline

> This section's spec drives the whole chain from "source PNG/HDR" to "GPU sampling". The current implementation is
> **full RGBA8 upload at runtime** (§31 records: Intel Sponza 2022, 4.5 GB of decompressed
> pixels, ~1.8s load, 4.5 GB of GPU memory), and this section defines the target architecture: **offline preprocessing +
> block compression + on-demand streaming**. This is another hard constraint beyond TBDR bandwidth —
> mobile GPUs have tighter memory and bandwidth than desktop, and cannot run uncompressed at all.

### 7.1 Design principles

| Principle | Rationale |
|------|------|
| **Offline preprocessing, zero decoding at runtime** | PNG decoding (701ms) + block-compression encoding (tens of seconds per image) should not be redone on every launch. Re-import only when the source fingerprint changes. |
| **Mobile formats first, desktop capability-probed fallback** | Matches §2.2's bandwidth constraint and §2.6's capability probing. ASTC is native on new mobile devices and also supported by modern desktop GPUs; BC is the fallback for old desktop devices. |
| **Artifacts do not enter git; generated locally / in CI** | 72 4K BC7 images ≈ 2.25 GB would blow up the git repository. Source SHA256 guarantees consistency. |
| **glTF is not read directly; import offline through `prism-cooker`** | Matches §3 "source formats are imported at build time; the engine does not read files directly". Import / cook happens in `prism-cooker`; the runtime only reads RTEX inside `.pak`. |
| **The container carries the mip chain, enabling later streaming** | KTX2 natively stores the full mip chain; phase 3 streaming builds on this. |

### 7.2 Format selection (new mobile devices first)

**ASTC is the primary format**, covering new mobile devices + modern desktop GPUs; BC is the fallback for old desktop devices;

| Texture type | Primary format (mobile + modern desktop) | Desktop fallback | Low-end mobile fallback |
|----------|--------------------------|----------|--------------|
| Albedo / Color (LDR) | **ASTC 6×6 sRGB** (3.56 bpp) | BC7 sRGB | ETC2 |
| Normal map (tangent-space) | **ASTC 6×6** or a dedicated two-channel variant | BC5 | ETC2 |
| Metallic / Roughness (packed) | **ASTC 6×6** | BC7 | ETC2 |
| Single-channel mask / AO / height | **ASTC 6×6** | BC4 | ETC2 R |
| Emissive (LDR) | **ASTC 6×6 sRGB** | BC7 sRGB | ETC2 |
| Emissive (HDR) / IBL env / lightmap | **ASTC HDR 6×6** (`VK_EXT_texture_compression_astc_hdr`) | BC6H | RGBA16f (uncompressed fallback) |
| UI / font atlas | **ASTC 4×4** (8 bpp, quality first) | BC7 | ETC2 |

**Why ASTC rather than BC**:
- ASTC 6×6 (3.56 bpp) is **2.2× smaller** than BC7 (8 bpp) at similar quality -- mobile bandwidth and memory are tighter.
- ASTC is native on mobile hardware (Adreno 6xx+ / Mali Midgard+ / Apple A7+ all support it), and desktop RTX 20+ / Intel Ice Lake+ / AMD RDNA2+ also support it.
- Vulkan probes it with `vkGetPhysicalDeviceFeatures::textureCompressionASTC_LDR`, matching §2.6 "capability-driven degradation".
- ASTC HDR is probed through the `VK_EXT_texture_compression_astc_hdr` extension, covering HDR floating-point scenes separately (the ASTC counterpart of BC6H).

**BC as the desktop fallback**: old desktop GPUs (pre-RTX20 / pre-RDNA2 / pre-Ice Lake) lack ASTC and fall back to BC7/BC5/BC4/BC6H. The fallback path is probed by `capabilities.rs`, with no `#[cfg(target_os)]`.

**Neural texture compression (NTC) is not adopted yet**: NVIDIA NTC (SIGGRAPH 2023) and Qualcomm Adreno neural texture compression are still research / early-stage, with no standard Vulkan extension, no cross-vendor toolchain, and tensor-core decoding. **Revisit after Khronos standardizes** (see `docs/lessons-learned.md` §31.6).

### 7.3 Resource lifecycle classification (aligned with §6.1)

Textures fall into the **scene-level** class among §6.1's three lifecycles:

| Class | Trigger | Example | Destroy responsibility |
|------|------|------|----------|
| **Scene-level (texture)** | Scene load / unload | BC/ASTC textures decoded from KTX2, IBL env cube, BRDF LUT | Scene manager (not the swapchain callback) |

**Key**: textures must never be attached to the swapchain recreate callback. Textures change only when the level changes; resize does not touch them.
`RenderTextureManager`'s resource table is `SceneScope`, independent of `SwapchainScope` / `FrameScope`.

### 7.4 Offline import pipeline (xtask subcommand)

A new `xtask texture-import` pre-converts source images referenced by glTF (PNG/HDR) into a KTX2 cache.

**Input**: glTF file (e.g. `NewSponza_Main_glTF_003.gltf`)
**Output**: `scene_cache/<scene>/` directory + `manifest.json`

```
assets/
  scenes.json
  scene_cache/                          ← .gitignore
    sponza/
      desktop/                          ← desktop artifacts (BC7/BC5/BC4/BC6H)
        arch_stone_wall_01_BaseColor.bc7.ktx2
        arch_stone_wall_01_Normal.bc5.ktx2
        ...
      android/                          ← mobile artifacts (ASTC 6×6 / ASTC HDR)
        arch_stone_wall_01_BaseColor.astc6.ktx2
        arch_stone_wall_01_Normal.astc6.ktx2
        ...
      manifest.json                     ← centralized metadata
```

**manifest.json structure** (one record per image):
```json
{
  "scene": "sponza",
  "source_gltf_sha256": "a3f2...",
  "textures": [
    {
      "name": "arch_stone_wall_01_BaseColor",
      "source_uri": "textures/arch_stone_wall_01_BaseColor.png",
      "source_sha256": "b7c4...",
      "width": 4096, "height": 4096,
      "kind": "albedo",                  // determines sRGB + format selection
      "mip_levels": 12,
      "desktop": "desktop/arch_stone_wall_01_BaseColor.bc7.ktx2",
      "desktop_format": "BC7_SRGB",
      "android": "android/arch_stone_wall_01_BaseColor.astc6.ktx2",
      "android_format": "ASTC_6x6_SRGB"
    }
  ]
}
```

**Import logic**:
1. Parse the glTF and enumerate the image URI list.
2. For each image, infer `kind` heuristically from the filename (`*_BaseColor` -> albedo/sRGB, `*_Normal` -> normal/linear, `*_Roughness*Metalness` -> MR/linear, with the `*Normal` suffix taking priority over BaseColor).
3. Compute the source file SHA256 and compare with the manifest; **hit with matching sha -> skip** (incremental import).
4. Miss -> decode PNG with the `image` crate -> encode with `bc7enc` / `astc-encoder` -> write KTX2.
5. Update the manifest.

**kind -> format mapping** (aligned with the §7.2 table):
```rust
match kind {
    TextureKind::Albedo | TextureKind::EmissiveLdr => {
        (DesktopFmt::BC7Srgb, MobileFmt::Astc6x6Srgb)
    }
    TextureKind::Normal => {
        (DesktopFmt::BC5, MobileFmt::Astc6x6)  // linear
    }
    TextureKind::MetallicRoughness => {
        (DesktopFmt::BC7, MobileFmt::Astc6x6)  // linear
    }
    TextureKind::HdrEnv | TextureKind::EmissiveHdr => {
        (DesktopFmt::BC6H, MobileFmt::AstcHdr6x6)
    }
    TextureKind::Mask => {
        (DesktopFmt::BC4, MobileFmt::Astc6x6)  // linear
    }
}
```

### 7.5 Runtime load path (KTX2 direct read, superseded by §10)

> **Obsolete after the 2026-10 layered refactor**: `gltf_loader` / `SceneStore` were removed along with the old `prism-asset` split;
> import / cook moved to `prism-cooker`, and the runtime only reads RTEX inside `.pak` (see §10.12). Sections 7.5–7.7
> are kept as historical design and are no longer implemented this way.

`gltf_loader::load` was once planned to prefer the KTX2 cache:

```
Parse glTF to get the image URI list
  -> look up scene_cache/<scene>/manifest.json
     ├─ hit (sha256 matches):
     │    mmap KTX2 bytes (choose desktop/ or android/ by platform)
     │    -> pass directly to BatchUploader (already BC/ASTC-compressed blocks, no decoding)
     │    -> record (asset_h, vk::Format, width, height, mip_levels)
     │    saves: PNG decode (701ms) + to_rgba8 conversion + runtime compressed-format encoding
     └─ miss:
          fall back to the existing PNG -> RGBA8 path (kept so the app still runs without a cache)
          log::warn!("texture cache miss for {uri}, run `cargo run -p xtask -- texture-import <scene>`")
```

**Key constraints**:
- A cache miss only warns, not errors -- a first run or cleared cache can still run (through the old RGBA8 path), preserving the developer experience.
- The runtime **does no encoding** (encoding a 4K image with BC7 takes tens of seconds, a disastrous experience). Encoding happens only offline in xtask.
- KTX2 is GPU-ready bytes; after mmap, `vkCmdCopyBufferToImage` uploads directly, at **near-zero CPU cost**.

### 7.6 BatchUploader / TextureUploadInput changes

**Implemented (2026-08-21)**: `TextureUploadInput` has been extended to support compressed formats:

```rust
pub struct TextureUploadInput {
    pub width: u32,
    pub height: u32,
    pub format: TextureFormat, // Rgba8 | Bc7* | Bc5 | Bc4 | Bc6H | Astc* | Etc2
    pub mip_levels: u32,       // 0=auto (Rgba8) / 1=single level (compressed) / >1=full KTX2 chain
    pub pixels: Vec<u8>,       // compressed block bytes (BC/ASTC) or RGBA8
}

pub enum TextureFormat {
    Rgba8, Rgba8Srgb,
    Bc7Unorm, Bc7Srgb, Bc5Unorm, Bc4Unorm, Bc6HUfloat,
    Astc4x4Unorm/Srgb, Astc6x6Unorm/Srgb, Astc8x8Unorm/Srgb,
    Etc2R8G8B8A8Srgb, // covers the full CookProfile set
}
impl TextureFormat { fn is_compressed()->bool; fn compressed_byte_len()->usize; fn vk_format()->vk::Format }
```

`BatchUploader::upload_image` branches by format:
- **Rgba8**: create image + staging + copy + generate the mip blit chain.
- **BC/ASTC**: create image (with `vk::Format`) + staging + copy all mip levels (**no blit**; compressed formats cannot be blitted, and the mip chain is pre-generated by KTX2). Transition directly to `SHADER_READ_ONLY_OPTIMAL`.
- `RenderTextureManager::reserve/reserve_into` already validates `compressed_byte_len` and branches on `mip_levels` according to `is_compressed`.

**The mip chain is carried by KTX2**: the offline tool downsamples and encodes level by level from mip 0 with encoders such as `bc7enc`, writing them into the KTX2 mip level array. The runtime copies all mips at once, with no runtime downsampling.

### 7.7 Phase breakdown (independent PRs, each step keeps CI green)

- **PR-T1: BC/ASTC upload support (without changing the load path)** ✅ **completed 2026-08-21**. The full `TextureFormat` extension + `mip_levels` + `is_compressed/vk_format` + `RenderTextureManager` validation/upload branches have landed. A later extension adds a sampling-approximation test for BC7 encoding through the new path. **This PR does not touch glTF loading; the runtime still uses RGBA8 (the compressed path can be exercised manually)**.
- **PR-T1.1: BC / ASTC / ETC2 cooker encoding** ✅ **completed 2026-11-02**. `TextureCooker` really encodes formats according to `CookProfile.texture.compression`: BC1/BC3/BC5/BC7 go through pure-Rust `texture-compressor`, ASTC 4×4/6×6/8×8/12×12 through vendored `ctt-astcenc`, ETC2 RGBA through vendored `ctt-etcpak`; the RTEX format bytes and runtime sRGB / linear mapping were filled in as well. Only BC6H (HDR) still explicitly falls back to RGBA8 (#519).
- **PR-T2: xtask texture-import offline tool**. Add `xtask/src/bin/texture-import.rs` with `ktx2` + `bc7enc` + `astc-encoder` dependencies. Scan glTF -> encode -> write KTX2 + manifest.json. Command line: `cargo run -p xtask -- texture-import --scene sponza --platform desktop,android`. **This PR only produces the tool; it does not change the engine**.
- **PR-T3: `prism-asset` runtime prefers KTX2**. `gltf_loader::load` gains a cache lookup branch: a hit takes the KTX2 path, a miss falls back to RGBA8 (with a warning). `SceneStore` gains KTX2 parsing (`ktx2` crate decode). **After this PR lands, a scene that has run `xtask texture-import` once loads in ~0.5s instead of ~1.8s**.
- **PR-T4 (optional): mip chain streaming**. KTX2 mip levels are loaded/unloaded dynamically by view distance, loading only low mips on the first frame. This requires `SceneStore` to support partial loads and the render pipeline to tolerate "texture not ready". It is a large effort, left for a later milestone.

> **Ordering principle**: PR-T1 first builds the "can upload compressed formats" capability (independent of the import tool), PR-T2 then builds the import tool (independent of engine changes), and PR-T3 finally connects the two. Each step is independently verifiable, avoiding "change the engine first, then discover the import tool cannot keep up".
> **Status**: T1 (compressed-format upload) is done; T2 / T3 were not implemented as described here, and texture import / cook moved to `prism-cooker` in §10, with the runtime reading RTEX inside `.pak`.

### 7.8 Non-goals / anti-goals

- **Do not** encode BC/ASTC at runtime. Encoding is slow (seconds per image) and must happen in the offline tool.
- **Do not** commit KTX2 artifacts to git. They are too large; SHA256 guarantees consistency.
- **Do not** use Basis Universal UASTC as a single cross-platform encoding. It loses a little quality, and Prisma supports ASTC on both desktop and mobile (all modern desktop GPUs do), so "encode once, transcode everywhere" is unnecessary. Separate per-platform artifacts are clearer.
- **Do not** use neural texture compression (NTC / Adreno neural textures). There is no standard extension and no cross-vendor toolchain; wait for Khronos standardization.
- **Do not** support BC1/BC2/BC3 separately. BC2/BC3 are DX9-era formats superseded by BC7; BC1 is only for extreme size cases, and mobile ASTC 12×12 already covers that niche.

### 7.9 VRAM suballocation and batch staging arena (#1030)

**Implemented (2026-11-10)**:
- **StagingArena chunk reuse**: `BatchUploader` embeds a `StagingArena` that merges hundreds of per-resource `vkAllocateMemory` staging allocations into large staging buffer allocations (16 MiB chunks by default), growing on demand and suballocating with 16-byte (buffer) and 256-byte (image) alignment.
- **llvmpipe driver synchronization guarantee**: before submitting commands, `unmap_all` explicitly unmaps every staging chunk, fixing dirty caches and copy races caused by persistent mapping on the pure-software rasterization backend, and guaranteeing cross-driver consistency.
- **MemorySuballocator core**: a pure-Rust VRAM suballocator indexed by multiple memory types with free-list coalescing, exposing reserved / active allocation VRAM statistics on `GraphRenderer` and `ReportStats`.

## 8. Frame lifecycle and architecture layering (planning)

> **2026-08-21 update**: `GraphRenderer::render()` has been split into `begin_frame` → `prepare` → `execute` → `present` → `end_frame`, and `prepare` is now actually called from `render_system`/`render_runner` (dirty detection reserved). As scene assets grow, the next extension needs to split further into the three layers `RenderRuntime`/`RenderAppShell`/`Plugin` (see §8.2) to guarantee thread safety and resource contracts.

### 8.1 Phases of a frame

```
Before Render = begin_frame + update + prepare + after_prepare
Render        = app.render (RenderGraph recording + submit)
After Render  = present + end_frame
```

Access permissions per phase:

| Phase | Allowed | Not allowed |
|------|--------|----------|
| `update` | Modify `World` (CPU scene semantics: add/remove instances, swap materials, change transforms) | GPU resources, bindless tables |
| `prepare` | Read the current `World` snapshot, write GPU scene buffers, descriptors, bindless | Modify `World` |
| `after_prepare` | Read-only queries (batch raycast, scene version checks) | Modify `World` or the GPU scene |
| `render` | Read-only GPU scene (`SceneReadView`), record commands through the RenderGraph | Modify scene state |

### 8.2 Three layers of responsibility

| Layer | Responsibility | Code |
|----|------|---------|
| **RenderRuntime** | GPU resource owner: `VulkanContext`, bindless manager, descriptor system, swapchain, command pool, render world (scene GPU snapshot). Provides phased access entry points. | Split out of the current `GraphRenderer` into the new crate `prism-render-runtime` |
| **RenderAppShell** | Per-frame ordering: calls the runtime's phase hooks in a fixed order and passes a narrowed context to the App. | New module, depends on the runtime |
| **App / Plugin** | Concrete business logic: decides pass order, owns window-sized resources (RT targets, main view), camera, GUI. | Currently spread across `app.rs` + `render_system.rs`, progressively split into Plugins |

### 8.3 Startup and resize

- Single startup entry point: the platform layer creates the window, and the render thread drives the App through `Box<dyn RenderApp>`.
- resize is handled only at a safe point in the render loop. `RenderRuntime::handle_resize` rebuilds the swapchain + present state and returns `ResizeCtx`; the App uses it to notify Plugins that need window-sized resources rebuilt (RT working target, main view, GBuffer).
- Shutdown: call `shutdown()` on the App and Plugins first, then destroy `RenderRuntime` (GPU idle → release subresources → destroy the `Gfx` owner).

### 8.4 Plugin model (extension route)

Rendering capabilities are split into pluggable Plugins; each Plugin:

- registers its own pass nodes with the RenderGraph during `setup`.
- is called by the App in order during `update` / `prepare` / `render`.
- owns its resource lifetimes (e.g. `RtPipeline` owns the RT working target and main view target).
- **Application-layer `AppPlugin` (#1165, reserved and unused)**: `prism-app::plugin::AppPlugin` defines the top-level plugin contract, and `DefaultPlugins` wraps rendering/physics/audio into three thin plugins that only call `enable_subsystem`. **It is not part of a user project's execution chain**: project gameplay goes through `add_game_system` / `export_project!`, and capability pruning goes through the `Subsystem` enum (see "Three paths" below). There is currently no `add_plugin` caller anywhere in the repository, and no third-party `AppPlugin` implementation.

#### Three paths (do not mix)

| Path | What it solves | Entry point | Status |
|---|---|---|---|
| `add_game_system` / `export_project!` | User projects register gameplay | The project crate's `build_app()` / `prism_project_get_api` | Used by every project |
| `Subsystem` enum + cargo feature | Select engine capabilities at compile time (render/audio/physics) | The feature list in `Cargo.toml` | Used by every project |
| `AppPlugin` | Replace engine **built-in modules** (swapping in Rapier for custom physics, etc.) | `App::add_plugin` | Reserved, no users |

The `ShadowMapPass` / `ForwardPass` / `GtaoPass` / `PostPass` order is currently hardcoded in `GraphRenderer::new`; a later extension changes this so the App adds passes on demand in the `render` hook.

### 8.5 Migration steps

- **PR-L1: phase split** ✅ **completed 2026-08-21**. `GraphRenderer::render()` has been split into `begin_frame` → `prepare` → `execute` → `present` → `end_frame` and wired into the call chain, with input validation / dirty-log reservations in `prepare`. It still runs sequentially in a single thread; CI is green.
- **PR-L2: Runtime / App separation** ✅ **completed 2026-08-21 (implementation)**. Added `prism-render/src/render_runtime.rs:RenderRuntime` (a thin wrapper over `VulkanContext`+`GraphRenderer`) and `prism-render/src/plugin.rs:RenderPlugin + PluginRegistry`, satisfying §8.2's three-layer responsibility contract; `GraphRenderer::context_arc()` is exposed, so a later extension can split it into its own crate.
- **PR-L3: Plugin integration** ⏳ **not landed (trait definitions only)**. `prism-render/src/plugin.rs` contains only the 40-line `RenderPlugin` + `PluginRegistry` definitions: `PluginRegistry` is never constructed, `setup_all` has zero callers beyond its definition, and `RenderPlugin` has zero implementations. `ShadowMapPass`/`GtaoPass` still follow the hardcoded order in `GraphRenderer::new`—this is the actual current shape, not a TODO leftover.

## 9. Scene data synchronization (CPU→GPU design)

> **2026-11-10 completed (#1157)**: the unified `DirtyDispatchPlan` has landed and is consumed in the `prepare` phase, converging batch uploads and releases; `SceneReadView` has advanced to a complete read-only scene snapshot.

### 9.1 Synchronization pipeline overview

```
World (authoritative CPU semantics)
  │
  ├─ AssetHub loads glTF/textures in the background → emits ModelLoaded events
  │
  └─ sync_for_render (every frame at the update → prepare boundary)
       │
       ├─ SceneAssetIngestor: turns loader-produced CPU bytes into typed handles
       │   (TextureHandle / MeshHandle / MaterialHandle)
       │
       ├─ SceneChanges: the set of CPU semantic changes this frame
       │   (added/removed/changed instance / material / light / texture / sky)
       │
       └─ DirtyRouter: SceneChanges + static rules → DirtyDispatchPlan
            │
            ├─ RenderTextureManager: upload textures → bindless registration
            ├─ RenderMeshManager: submesh geometry upload + mesh BLAS
            ├─ RenderMaterialManager: material buffer upload
            ├─ RenderInstanceManager: instance slot allocation + active render list
            ├─ RenderSkyManager: HDRI importance alias table + sky dispatch
            ├─ RenderAnalyticLightManager: analytic light buffer
            └─ RenderEmissiveLightTable: emissive triangle records + alias table
```

### 9.2 Dirty routing rules

Dirty dispatch does not use a general event bus; it uses a **static rule set**, where each SceneChange class maps to one or more render-manager dispatches:

| CPU change | Triggered render dispatch |
|----------|----------------------|
| texture added / changed / removed | `RenderTextureManager` → upload / update / release the bindless slot |
| texture ready | → mark materials that depend on that texture dirty |
| mesh added / changed / removed | `RenderMeshManager` → geometry upload + BLAS rebuild |
| material changed | `RenderMaterialManager` → material buffer upload |
| instance changed (transform / active / binding) | `RenderInstanceManager` → instance slot update + `RenderEmissiveLightTable` dirty |
| sky changed | `RenderSkyManager` → sky dispatch |
| analytic light changed | `RenderAnalyticLightManager` → light buffer rebuild |
| emissive table rebuild (cascaded from mesh / material / instance dirty) | `RenderEmissiveLightTable` → emissive triangle records + alias table |

**Key rules**:
- The prepare phase consumes `SceneChanges` **once** and produces a `DirtyDispatchPlan`; dirty state must not accumulate across frames.
- `RenderEmissiveLightTable` depends on `RenderInstanceManager`'s prepare result (the active instance list), so its rebuild is ordered after the instance dispatch.
- The texture → material → instance cascade is currently only a frame-to-frame comparison of `SceneChanges` / draw-item signatures in `prism-engine::dirty_router`; `RenderTextureManager` / `RenderMaterialManager` update by slot, with no cross-layer reference rejection. The unified `DirtyDispatchPlan` above is the target shape, not yet implemented.

### 9.3 Read-only scene view

Passes do not access manager internals directly; they go through the read-only `SceneReadView`:

```rust
pub struct SceneReadView<'a> {
    /// scene root buffers (instance / material / light SSBOs)
    scene_buffers: &'a SceneGpuBuffers,
    /// TLAS handle (ray query)
    tlas: vk::AccelerationStructureKHR,
    /// bindless texture table
    textures: &'a BindlessTextureTable,
    /// sky distribution alias table (HDRI importance sampling)
    sky: &'a SkyGpuState,
}
```

- `SceneReadView` is produced after prepare and is read-only during render.
- The version snapshot (`accum_signature`) exposes whether scene semantics changed, so the App can decide whether to reset offline accumulation state.
- It does not expose the `RenderWorld` owner or concrete GPU buffer layouts.

### 9.4 Resource lifecycle and synchronization phases

Scoped by lifetime; the prepare phase runs in this order:

1. **Frame-level synchronization**: acquire present target, FIF wait, command pool reset.
2. **Dirty dispatch**: consume `SceneChanges`, write into render manager pending queues.
3. **GPU upload**: texture → submesh geometry → material buffer → instance buffer.
4. **Emissive table**: built after instance dispatch and before scene buffer upload.
5. **Descriptor update**: per-frame descriptor writes (scene UBO, bindless table version).
6. **SceneReadView generation**: construct the immutable snapshot after all uploads complete.

### 9.5 Migration steps

- **PR-S1: SceneChanges extraction** ✅ **completed**. `prism-engine::render_system::SceneChanges` + `extract_frame_packet` now uniformly extract camera/lights/`draw_items` and compare them centrally in `DirtyRouter::update`.
- **PR-S2: DirtyRouter integration with RenderManager and DirtyDispatchPlan** ✅ **completed (#1157)**. `DirtyRouter` uniformly produces `DirtyDispatchPlan` and consumes it in the `prepare` phase, converging batch uploads into `prepare`.
- **PR-S3: SceneReadView complete read-only snapshot** ✅ **completed (#1157)**. `SceneReadView` now supports complete read-only extraction of cameras, lights, environment and draw items; passes depend only on the view and `GraphFrame`.
- **PR-S4 (optional): background AssetHub async loading** ✅ **implemented and wired**. The `prism-app::io_runner` + `asset_bridge::AssetResolveRequest/Result` channel + `render_runner`'s async `GraphRenderer` construction are connected, background-decoded `ModelLoaded` events are delivered via `flume`, and the synchronous `GpuAssetResolver` remains the fallback.

## 10. Asset pipeline v2 — offline preprocessing pipeline

> Added 2026-07-25, layered refactor 2026-10. Asset responsibilities are split into three crates:
> **`prism-asset`** (runtime `.pak` / `ResourceManager`), **`prism-scene`**
> (component schema + `.scene.json` + RSCN v4 encoding/decoding), **`prism-cooker`**
> (importer / cooker / db / `prism-cook` CLI). The old 7-crate split is not coming back.
> Build orchestration lives in `prism-build-pipeline` and the editor, and does not enter the game runtime.

### 10.1 Architecture overview

```
Source files (Assets/)
    │
    ▼  [Import]  ← importer module + db module
  ┌─────────────┐
  │ Intermediate │  RTXI (texture), RMXI (mesh)
  └──────┬──────┘
         │
         ▼  [Cook]  ← prism-cooker (+ CookProfile)
  ┌─────────────┐
  │ Runtime fmt  │  RTEX (texture mip chain), RMES (planar vertices), RSCN v4 (scene)
  └──────┬──────┘
         │
         ▼  [Package]  ← prism-asset::package
  ┌─────────────┐
  │  .pak archive│  RPAK format + xxh3 checksum
  └──────┬──────┘
         │
         ▼  [Runtime]  ← prism-asset::runtime
  ┌─────────────┐
  │  Handle<T>  │  lazy loading + memory budget + dependency resolution + hot reload
  └─────────────┘
```

**Core design principles**:
- **The editor does the heavy computation offline**; the runtime only reads `.pak`, with zero decode and zero encode.
- **No build-tool dependency at runtime**: `prism-asset` does not depend on `prism-cooker` / `prism-scene`;
  `prism-cooker` is used only on build machines / in the editor.
- **One scene schema only**: both cooker and runtime interpret components through `prism-scene`'s
  `ComponentRegistry` + `TypeDesc`; RSCN v4 does not duplicate the schema per scene.
- **`Handle<T>` is separate from ECS `Entity`**: a Handle is an asset-lifetime concept (generation-counted slot),
  an Entity is a frame-level ECS concept; the two are never mixed.

### 10.2 Crate layering and responsibilities

| crate | Role | Responsibility | Main dependencies |
|-------|------|------|----------|
| `prism-asset` | Runtime | `AssetId` (64-bit generation+serial), `AssetType`, `Handle<T>`, `.pak` (RPAK + zstd + xxh3), `ResourceManager` (lazy load/budget/hot reload), streaming | serde, zstd, tokio (feature-gated) |
| `prism-gfx` | Shared data | Render-state enums for shapes (`CullMode`/`CompareFunction`/`BlendMode`/`RenderQueue`/`ShadowCastingMode`/lighting and camera enums) + reflection encoding; Vulkan-free, ECS-free | `prism-reflect`, serde, glam |
| `prism-scene` | Schema | Scene/UI components, `ComponentRegistry`, `.scene.json` authoring format, scene manifest, RSCN v4 encoding/decoding, heightfield pure data | `prism-ecs`, `prism-reflect`, `prism-gfx`, `prism-ini`, serde |
| `prism-cooker` | Build time | Importer framework, cooker framework, CookProfile, asset database/import cache, `SceneCooker`, `prism-cook` CLI | `prism-asset`, `prism-scene`, image, gltf |
| `prism-build-pipeline` | Orchestration | Batch entry points such as `cook`, `build-scenes`, `pack`, `heightmap`; `bake-gi` requires the `gi-bake` feature | `prism-cooker`, `prism-scene`; `gi-bake` additionally needs `prism-engine`, `prism-render` |

`prism-asset` features serve only runtime pruning (`runtime` / `package` / `types` /
`streaming` / `hot-reload`) and no longer include `cooker` / `importer` / `db` / `cli`.
Build tools consume `prism-asset`'s `core` / `package` but do not enter the game dependency chain in reverse.

```sh
cargo build -p prism-asset -p prism-scene -p prism-cooker
cargo test  -p prism-asset -p prism-scene -p prism-cooker
cargo run   -p prism-cooker --bin prism-cook -- init
```

### 10.3 Asset type system (core module)

**`AssetId`** (`id.rs`):
```
high 32 bits = generation (monotonic epoch), low 32 bits = serial
AssetId::generate()       → process-wide atomic increment
AssetId::tombstone(n)     → deletion marker (gen=u32::MAX, sorts after all live IDs)
AssetIdGenerator          → persistent ID generator for the editor
```

**`AssetType`** (`type.rs`) — a `#[repr(u32)]` enum, discriminated by u32 at runtime:
```
Binary(0) / Texture(1) / Mesh(2) / Material(3) / Shader(4) /
Prefab(5) / Scene(6) / Audio(7) / Font(8) / Unknown(0xFF)
```
- `from_extension()` infers the type from the extension (fonts: `ttf` / `otf` / `ttc`)
- `Font` goes into the package as-is, without cooking: the runtime reads the bytes and hands them to the UI text rasterizer (`Text.font`, #583)
- `AssetRef { id, asset_type }` — a serializable lightweight cross-asset reference

**`Handle<T>`** (`handle.rs`) — a generation-counted handle, `u64`-sized:
```
low 32 bits = slot index, high 32 bits = generation
null = (0, 0), static region = index < 1024
AnyHandle = type-erased version, supporting heterogeneous storage
```

### 10.4 Intermediate formats (Importer→Cooker contract)

| Format | Magic | Payload |
|------|------|------|
| **RTXI** (intermediate texture) | `b"RTXI"` | `[magic:4][w:4][h:4][ch:1][fmt:1][RGBA8 pixels:N]` |
| **RMXI** (intermediate mesh) | `b"RMXI"` | v1: `[magic:4][ver:1][verts:4][idxs:4][uv_ch:4][pos:N][nrm:N][uv:N][idx:N]`; v2 appends `[submesh_count:4][index_start:4][index_count:4][material_index:4]…` at the end |

Intermediate formats are pure CPU data with no GPU dependency. Importers decode source files;
the Cooker consumes intermediate formats to produce runtime formats.

### 10.5 Runtime formats (Cooker→Package contract)

| Format | Magic | Structure |
|------|------|------|
| **RTEX** (cooked texture) | `b"RTEX"` | header + mip offset table + per-level mip data (box-filter downsampling) |
| **RMES** (cooked mesh) | `b"RMES"` | header + attribute offset table + **planar** vertex blocks (positions → normals → uv; the offset fields are the authoritative addressing) + index data; v2 appends the same submesh table at the end |
| **RATL** (cooked sprite atlas metadata) | `b"RATL"` | header + `texture_path` + dimensions + slice list (`name_hash`, `pixel_rect`, `uv_rect`, `original_size`, `trimmed_offset`, `pivot`, `nine_slice`, #1158) |
| **RA2D** (cooked 2D animation metadata) | `b"RA2D"` | header + `atlas_path` + frame list (`sprite_name`, `duration_ms`) + Tags (`from`, `to`, `direction`) + Slices (#1159) |
| **RTMG** (cooked tilemap level) | `b"RTMG"` | header + dimensions + `tileset_path` + multi-layer GID arrays + object-layer entities + greedy-merged collision boxes (#1159) |

> RMES v2's submesh table lets one entity split into multiple `DrawItem`s according to
> `MeshRenderer.material_paths` (`index_start` / `index_count`), so different ranges of the same mesh can use different materials.

> The `stride` field in the RMES header is not used to locate attributes: `MeshCooker::write_rmes` writes planar blocks,
> and `stride` only records the interleaved size. `decode_rmes` in `prism-render/src/asset_decoders.rs`
> parses by offset fields, not by interleaved `stride` reads.

RTEX's mip chain is generated by the Cooker with a 2×2 box filter (`TextureCooker::generate_mips`);
no downsampling is needed at runtime. It can later be replaced with higher-quality Kaiser/Lanczos filtering.

### 10.6 .pak archive format (package module)

```
┌─ PackageHeader ────────────────────────┬── 52 bytes ─┐
│ magic(4)=b"RPAK"  version(4)=1          │
│ asset_count(4)  registry_offset(8)      │
│ registry_size(8)  data_offset(8)        │
│ data_size(8)  checksum(8)               │
├─ RuntimeAssetRecord[n] ────────────────┬── 48 bytes × n ─┐
│ id(8)  type_id(4)  flags(4)             │
│ offset(8)  size(8)  compressed_size(8)  │
│ dep_start(4)  dep_count(4)              │
├─ Dependency Array[m] ──────────────────┬── 8 bytes × m ─┐
│ [AssetId(u64)]                          │
├─ Data Chunks ──────────────────────────┤
│ [asset 0 data][asset 1 data]...         │
└─────────────────────────────────────────┘
```
- **Compression**: zstd (per asset, marked `FLAG_COMPRESSED`)
- **Streaming**: `FLAG_STREAMED` assets stay uncompressed, and the reader can fetch blocks by offset/length only;
  when `CookSettings.streaming` is on, cooked assets at or above `streaming_threshold_bytes`
  (default 4 MiB, 1 MiB on mobile) get this flag automatically. `prism-build-pipeline pack/build-scenes
  --stream-threshold <bytes>` can set the same threshold by file size for an already-cooked directory.
- **Multi-package / split packages (#509)**: `prism-build-pipeline pack <cooked-dir> --optional-dir <dir>`
  (repeatable, longest prefix wins) splits the given subdirectory into `optional/optional_<group>.pak`, with the rest going into the base package
  (default `assets/packed/game.pak`). Asset IDs are derived only from the in-package relative path, so splitting does not renumber anything;
  the base package's `.pak.meta.json` lists each optional package's `group` / `pak` /
  `asset_count` under `optional_paks`. At runtime, **automatic mounting only scans the top level of `assets/`**, so packages inside
  `assets/optional/` are not resident at startup; call `GpuAssetResolver::load_optional_packages()` when needed (already-mounted packages are skipped).
- **Checksum**: xxh3-64 covers `header[4..44]` (version / asset_count / offsets and lengths; `magic` and
  `checksum` themselves excluded) + registry + deps + data. The reader validates **before parsing the registry**, so
  tampering with `version` / `asset_count` yields `ChecksumMismatch` immediately (#89).
- **Reading**: `PackageReader` `pread`s by offset from the file (uncompressed assets are still copied),
  and opening reads only the header + registry + deps.

### 10.7 Cook Profile system (`prism-cooker`, src/cooker/profile.rs)

**Priority chain** (high→low):
```
1. CLI overrides (command-line arguments)
2. Active project config (`--profile`; no persisted active profile, platform default otherwise)
3. Platform default (`--platform` → desktop/android/ios/embedded)
4. base.json (lowest)
```

**Five built-in profiles**:

| Profile | Texture compression | Max size | Generate tangents | Vertex compression | Streaming |
|------|---------|---------|---------|---------|------|
| base | RGBA8 | unlimited | no | no | no |
| desktop | **BC7** | 4096 | **yes** | no | no |
| android | **ASTC 8×8** | 2048 | no | **yes** | **yes** |
| ios | ASTC 8×8 | 2048 | no | yes | yes |
| embedded | ETC2 RGBA | 1024 | no | yes | yes |

**Important note (2026-11-02 update)**: `TextureCooker` now produces compressed RTEX for BC1/BC3/BC5/BC7, ASTC 4×4/6×6/8×8/12×12 and ETC2 RGBA according to the profile; the runtime's `TextureFormat/vk_format` and `RenderTextureManager` upload side support the same set of formats and select the sRGB / linear variant per texture slot. BC6H (HDR) still falls back to RGBA8.

**`CookSettings`** provides `settings_hash()`: deterministic JSON → xxh3-64,
used as the incremental build cache key.

**Override semantics**: fields in a profile file are per-field `Option`s (`TextureOverrides` / `MeshOverrides` /
`ShaderOverrides` / `CompressionOverrides`); only **written** fields override the parent or built-in value,
and absent fields keep the inherited result. So `{"texture":{"quality":95}}` does not reset the parent's `max_size` to 0,
and explicitly writing a default value (`"compression":"rgba8"`, `"generate_mips":false`) also takes effect.
`streaming_threshold_bytes` is likewise overridable: it only applies when `streaming: true`, and a
cooked asset at or above the threshold gets `FLAG_STREAMED` and skips zstd; `prism-cook build
--stream-threshold` can temporarily override the profile value.

**`ProfileManager`:**
- `resolve(name)` → recursive inheritance merge → `CookSettings`
- cycle detection (text report of the cycle path)
- `apply_cli_overrides()` → stack command-line overrides
- built-in profiles are held by `BUILTIN_DEFAULTS` as a static `LazyLock<HashMap>`;
  user profiles load from disk at `profiles_dir/{name}.json`; for `prism-cook build`, `profiles_dir`
  is `<project>/Library/Profiles/`, and resolution order is `--profile` → `--platform` → `desktop`.

### 10.8 Runtime ResourceManager (`prism-asset::runtime`)

**Core flow**:
1. `load_package("game.pak")` → register all assets (fill the slot array + `AssetId→index` map)
2. `load::<T: Asset>(id)` → on first use, read data from `.pak` on demand, cache it in the slot, return `Handle<T>`
3. `get::<T>(handle)` → generation check → deserialize → return `T`
4. `unload(handle)` / `unload_all()` → release data, update memory tracking

**Memory budget**:
- `set_memory_budget(bytes)` + `set_eviction_policy(Lru|Fifo|None)`
- when `load()` exceeds the budget → automatically evict the least recently used resources
- `evict(target_bytes)` triggers eviction manually

**Dependency resolution**:
- `load_with_deps<T>(id)` → DFS topological load of all dependencies
- cycle detection (warn + skip)

**Hot reload** (feature-gated `hot-reload`):
- `HotReloadWatcher`: polls `.pak` file modification times
- `on_pak_changed()`: re-read data → update the slot **in place** (generation is not incremented: handle identity stays the same,
  so `Handle`s issued before the hot reload keep working)

**`Asset` trait**:
```rust
pub trait Asset: Sized + Send + 'static {
    fn asset_type() -> AssetType;
    fn from_bytes(data: &[u8]) -> Result<Self, RuntimeError>;
    fn into_bytes(self) -> Vec<u8>;
}
```
Built-in `impl Asset for Vec<u8>` (binary blob).

### 10.9 CLI tools (`prism-cooker`, bin: `prism-cook`)

| Command | Function |
|------|------|
| `init` | Create the `Assets/` + `Library/` directory structure |
| `scan` | Scan `Assets/`, infer types by extension, write to the database |
| `import` | Run the matching Importer for each file, with an incremental cache |
| `build --output game.pak` | Cook all assets → pack `.pak`; `--profile <name>` / `--platform <p>` resolve the profile first (`--compression` overrides the profile's level) |
| `validate game.pak` | Validate magic + version + checksum |
| `verify` | Validate `Library/Intermediate/`: every record must have a content-addressed intermediate file, and list orphan files |
| `list` | List all assets in the database |
| `inspect <id>` | Show details for a single asset (dependency tree visible) |

`Library/Intermediate/` filenames are content-addressed:
`<source-hash>-<settings-hash>-v<importer-version>.bin` (the same three keys `ImportCache` uses for its decision). The same
source file imported with the same settings lands on the same filename in any project / checkout, so intermediate artifacts can be
reused across projects; changing settings or the importer version produces a new name, and old files are reported as orphans by `verify`
and cleaned up by `gc` (not yet implemented). `AssetId` remains only a runtime identifier inside `.pak`.

### 10.10 Scene cook and the runtime seam

Scene assets follow a separate typed path and no longer stuff JSON into RSCN:

```text
.scene.json
   │  prism-cooker::SceneCooker (parse + validate)
   ▼
prism-scene::rscn::encode_scene
   │  type_id + schema_hash + position-encoded payload
   ▼
.rscn  ──►  prism-engine::SceneLoader (decode_scene + ComponentRegistry)
```

- `prism-build-pipeline build-scenes` reads `assets/scenes.json`, cooks the scenes at the listed paths,
  rewrites the `.scene.json` paths in the manifest to `.rscn`, packs them, and produces the runtime
  `assets/scenes.pak` + `.pak.meta.json`; this is the release entry point.
  It also **recursively collects resources referenced by scenes**: environment HDR, `SpriteRenderer.sprite_path`,
  `MeshRenderer.mesh_path` (glTF/GLB → RMES) and
  `material_path` / `material_paths` (`.mat.json` → RMAT, along with the 5 textures),
  `Style.image` / `Tilemap.tileset_path` / `Text.font`. Materials/images embedded in glTF/GLB are also cooked into
  `{mesh}.materials/*.mat.json` (RMAT) and `{mesh}.textures/*.png` (RTEX) when the mesh is packed;
  when an entity has no authored material, scene cooking automatically fills in these generated paths per submesh (#520).
  Any unresolvable reference
  (missing file, absolute path, escaping `assets/`) makes `build-scenes` **exit non-zero** and list
  the paths—otherwise the packaged build would silently lack resources at runtime (#530);
  content unrelated to scene references that the runtime reads by **path string** is packed wholesale as "directory = content":
  `sprites/` (runtime-generated sprites), `audio/` (`play_sound("audio/…")`),
  `data/` (level configs and other project data, #1049). `asset_paths::read_asset_text` /
  `asset_exists` look for loose files first, then fall back to `.pak` through the in-package path manifest, so desktop and
  Android packaged builds read the same data;
- `prism-build-pipeline cook <input.scene.json>` cooks a single scene, calling `SceneCooker` directly;
- generic texture/mesh/binary assets still go through `prism-cook import/build`;
- `prism-build-pipeline pack <cooked-dir>` packs an already-cooked directory into `.pak` and writes
  the `.pak.meta.json` path manifest; asset IDs are derived from the **in-package relative path** (FNV-1a,
  [`AssetId::from_path`]), so adding files to the directory does not renumber existing resources—the texture `AssetId`
  baked into `RMAT` material data only lines up with in-package IDs when derived from the path (#530).
  `--optional-dir <dir>` (repeatable) splits that subdirectory into `optional/optional_<group>.pak`,
  mounted on demand at runtime (see §10.6); `--stream-threshold <bytes>` still marks
  `FLAG_STREAMED` by in-package file size (#509);
- the runtime reads only `.rscn` / `.pak` and does not depend on `prism-cooker`;
- during development you can also load from loose `.rscn` files, with the path decided by `SceneSource`.

**Development mode vs release mode**: both share the same runtime decode path; the only difference is the asset source
(loose files vs `.pak`), not two formats.

### 10.11 Integration Gate

To close the loop from "offline preprocessing → .pak → engine runtime", the following PRs are needed:

- **[G1] Runtime format decoders** 🟡 **partially complete**:
  - `prism-asset/src/formats.rs` (`decode_rtex` / `decode_rmes`) parses into pure-data `RtexInfo` / `RmesInfo`,
    used by `TextureAsset` / `MeshAsset` in `runtime/typed.rs`;
  - `prism-render/src/asset_decoders.rs` (`decode_rtex` / `decode_rmes`) parses the Cooker's actual layout
    (RTEX version field + mip offset table, RMES attribute offsets + planar vertex blocks) into
    `TextureUploadInput` / `MeshUploadInput`, including format-code mapping (RGBA8 / BC5 / BC6H) and bounds checks;
  - **Progress**: `prism-engine`'s `load_texture_data` now supports concatenating the full pre-baked RTEX mip chain
    (`TextureUploadInput.mip_levels > 1` + tightly packed pixels for all mip levels), and `BatchUploader` copies it
    straight to the GPU, avoiding redundant blits and preserving anisotropic filtering quality (#1156). The two GPU decoders
    (`asset_decoders.rs`) can remain as independent parsing fallbacks.

- **[G2] Cooker output format and engine integration** ✅ **scene path connected**: `SceneCooker` produces RSCN v4, and
  `prism-engine`'s `SceneLoader` decodes it with the same `prism-scene` schema; `prism-cooker` has cook/decode tests with real
  `.scene.json` files. End-to-end comparison of compressed texture/mesh data is still pending.

- **Scene switching (#482)**: `Engine::{load_scene_json_file_with_mode, unload_scene}` +
  the `Persistent` marker are engine primitives; the host's `App::load_scene_json_file` reuses the same `Engine`,
  so `Persistent` subtrees and user systems survive scene switches, and GPU handles of old entities are reclaimed by the resolver.
- **GPU resource reclamation** (#495): `GpuAssetResolver` records the slot / handle currently used by each
  `(entity, slot)`; on hot reload, component removal, entity destruction, or path rewrite, it hands the record back to
  `GraphRenderer::release_assets`, and the renderer looks up and destroys it by bindless slot / `MeshHandle` / material slot
  (for textures it first rewrites that slot's descriptor to the magenta fallback, avoiding sampling a released view).
  The old slot is released in the same frame that the new slot is uploaded to, so there is never an intermediate frame with
  "a released slot still being sampled".
- **[G3] ResourceManager → Engine bridge** ✅ **partially complete**: `GpuAssetResolver` now holds a `ResourceManager`
  and supports `load_package/load_package_bytes` and `load_with_deps`; `load_resource_package()` in `App::new`
  mounts **all** `.pak` files under `assets/` (loading the path manifest alongside each package when a same-named
  `.pak.meta.json` exists), no longer hardcoding `scenes.pak`; the `Handle<T>`→`Entity` mapping is maintained by
  `GpuAssetResolver`'s reference cache (`MeshRef` / `MaterialRef` / `SpriteRef` hold `Handle`s). The full chain
  `CLI build → .pak → ResourceManager → GPU` can be verified manually.

- **[G4] Build script integration** ✅ **landed**: `prism-build-pipeline build-scenes` uniformly performs
  `scenes.json → cooked RSCN → scenes.pak + path manifest`; generic assets still go through `prism-cook build`,
  and `scripts/run.ps1` calls it automatically after `cargo build`. `scripts/build-android.ps1` /
  `build-android-game.sh` copy **all** `.pak` and `.pak.meta.json` under `assets/` by extension
  into the APK's `assets/game/` (erroring if there are none); at Android startup `run_on_android` copies
  `game/` to internal storage and switches cwd. The CI asset-build step is still pending.

- **[G5] CookProfile integration into engine settings** ⏳ **config ready**: the 5 `CookProfile` tiers + `settings_hash`
  are implemented; passing `--profile` through to `AppConfig` startup parameters is still pending.

- **[G6] Hot reload pipeline** ✅ **wired in both editor and player**: `ResourceManager::on_pak_changed` swaps out the stale
  reader (`Handle`s stay valid), and `HotReloadWatcher` is always compiled with `runtime`. The editor reopens the rebuilt
  `.pak` in its 500ms poll and calls `invalidate_assets`; the player (`prism-app`) does the same when `PRISMA_HOT_RELOAD=1`
  (off by default in release builds; no thread, no stat). Remaining: reclaim old GPU handles by handle, hot reload loose assets
  (files under `assets/` that are not packed), and end-to-end visual verification on the player side.

> **Milestone suggestion**: G1+G2+G3 form the "closed-loop MVP" ✅ implemented and connected (manually verifiable end to end);
> G4+G5 are "developer experience polish" ⏳; G6 is "editor experience" ✅ implemented.

### 10.12 Relationship between §7's texture pipeline and §10

The KTX2/BC/ASTC offline compression scheme designed in §7 is conceptually an enhancement of the
`TextureImporter` → `TextureCooker` chain in §10. Specifically:

- **PR-T1 (compressed-format upload support)** from §7 is part of §10's G1—extending
  `BatchUploader::upload_image` to support BC/ASTC `vk::Format`.
- **PR-T2 (xtask texture-import)** from §7 is an offline encoding enhancement of §10's Cooker—
  adding a `compress: Some(Bc7|Astc|...)` path to `TextureCooker`, replacing the current RGBA8 passthrough.
- **PR-T3 (runtime prefers KTX2)** from §7 is superseded by §10's `.pak` scheme—
  the runtime no longer reads KTX2 files, but reads `.pak` files containing already-compressed RTEX data.

So §7 is not deleted; it is absorbed by the §10 framework as an internal implementation detail. New texture compression features
should be implemented at §10's Cooker and Package layers, not by bypassing the pipeline and reading the filesystem directly.

### 10.13 Authoring format v3: entity identity (design decision)

**Status**: **implemented** (#31). `prism-scene` (`id` field / validation / migration / cook) and `prism-editor-api` (subtree operations use ids, index remapping removed) landed together; prerequisites are #23 (single transform source) and #29 (cook linearization).

#### Current state and problem

`.scene.json` v2 uses **array indices** as entity identity:

```json
{ "entities": [ { "name": "Root" }, { "name": "Child", "parent": 0 } ] }
```

Order carries three things at once: traversal order, entity identity, and parent-reference target. The cost becomes explicit in `prism-editor-api`:
`move_entity` / `delete_entities` / `sync_prefab_instances` / `instantiate_prefab` each maintain a `remap[]` and **rewrite every entity's `parent`**; any missed update silently mis-parents an entity, and the format cannot express
"cross-scene references" or round-trip editing by external tools.

#### Decision

v3 introduces stable ids, with `parent` pointing at an id:

```json
{
  "version": 3,
  "entities": [
    { "id": 0, "name": "Root",  "parent": null, "transform": { }, "components": { } },
    { "id": 7, "name": "Child", "parent": 0,    "transform": { }, "components": { } }
  ]
}
```

- **Ids increase monotonically** (`max(id) + 1`), assigned by the editor on create / duplicate / instantiate; deleted ids are **never reused**,
  preventing old references from "resurrecting" onto a new entity.
- **Array order is no longer identity**; it only expresses file layout and editing order.
- **The width is `u32`** (not `u64` / UUID), for three reasons:
  1. UUIDs fill diffs with noise, are hard to review manually, and pollute test/fixture determinism (requiring an injected generator);
     this project uses a single-developer + agent git flow, where concurrent entity-creation conflicts are extremely unlikely, and duplicate ids are caught by `validate`.
  2. The only selling point of `u64` is "reserve high bits for a namespace", but that path does not work in this stack: `parent` is a TS `number` in the editor, with a safe-integer limit of **2^53**; putting a namespace in the high 32 bits pushes ids past that limit,
     and `JSON.parse` silently loses precision—which is more dangerous than a collision (collisions are caught by validation, precision loss is not).
     And the width is not visible in JSON anyway (small numbers serialize as-is), so `u64`'s readability benefit is zero.
  3. If a namespace is truly needed, string ids are the only option, which is back to UUID cost and has already been rejected.
- The code treats ids as an **opaque invariant** (only "unique within the document and never reused"), not assuming they equal indices or are contiguous,
  so any future requirement change is confined to the type width plus one migration.

#### Invariants (`SceneJson::validate`)

1. At least one entity; `id`s are unique.
2. `parent` must point at an **existing** id and must not point at itself.
3. Cycles: **tolerant on load** (`topological_order` appends unvisited entities in array order, giving a deterministic result),
   **rejected by the editor** (`reparent`'s existing descendant check is kept).
   A hard failure would make a hand-written bad cycle render the scene unopenable, costing more than it gains; cooking logs a `warn` for cycles.

#### Migration v2 → v3

- `id = array index` (identity mapping), `parent` values **unchanged** → migration is purely additive; old documents' diff contains only the `id` field.
- Idempotent: documents that already have `id`s are not rewritten; missing `id`s are filled in.
- An old engine reading v3 hits `migrate_scene_doc`'s "version above supported range" error (explicit failure, no silent downgrade).
- `.prefab.json` shares the same `SceneJson` shape and migration as scenes, so it is upgraded too.

#### Editor responsibilities (where code gets deleted)

| Operation | v2 (current) | v3 |
|---|---|---|
| Move entity | move the array + **rewrite all parents** | move the array element only |
| Change parent | go through `move_entity`'s index remapping | write only the target entity's `parent` field |
| Delete | compute `removed[]` + `remap[]` and rewrite everything | remove the row; set children pointing at the deleted id to `parent = null` |
| Duplicate / instantiate | append at the end + compute a `base` offset | allocate a contiguous id block for the subtree, attach the root to the target parent |
| Save as prefab | extract the subtree and shift internal parent indices | renumber subtree ids to **`0..k`** (prefabs own their id space) |
| Sync prefab instance | match by array position + shift everything | replace the subtree, reallocating internal ids; `PrefabInstance` metadata unchanged |

#### Relationship with RSCN

**RSCN v4 is unchanged**. The runtime builds the hierarchy in topological order only and does not need ids:

- cooking still writes `parent: i32` (topological index), which is smaller and faster to decode than storing ids;
- `encode_scene` uses an `id → topological position` `Vec` mapping (O(n), see §10.10 and #29);
- only when "runtime cross-entity references" appear (UI event bindings, script fields pointing at entities) should RSCN gain an optional id table;
  until then, do not introduce one.

#### Rejected alternatives

| Alternative | Reason for rejection |
|---|---|
| Nested `children` tree | Conflicts with flat ECS; moving a subtree moves a whole block of data; diff noise; random access degrades |
| UUID / GUID | No benefit within a single scene, worse diffs and readability; collision rate is negligible in this project |
| Keep array indices | Every new hierarchy feature in the editor adds another remap; cost grows linearly and cross-scene references remain impossible |
| Store `Entity` in `components` | Runtime handles (index+generation) cannot be persisted; they become invalid on load |

#### Acceptance criteria

1. All existing `.scene.json` / `.prefab.json` files migrate to a **fully equivalent** hierarchy (`topological_order`
   and the parent-child snapshot match before and after migration).
2. After random sequences of move / delete / duplicate / instantiate, `validate()` passes and no id dangles.
3. The line count of `remap`-related code in the editor's `scene.rs` drops by ≥ 60% (the main benefit of this decision is **deleting code**).
4. Cooking + decoding a real scene yields the same hierarchy as before migration (extend `prism-cooker`'s `cooks_real_default_scene`).

#### Phasing

The semantics of ids and the editor's index remapping **cannot land separately**: as soon as `parent` changes from an index to an id,
the `remap[]` machinery in `move_entity` / `delete_entities` would treat ids as indices and silently rewrite them. Therefore:

- **PR-1 (data layer + editor, must be one PR)**:
  - `prism-scene`: the `id` field, `validate` invariants, v2→v3 `migrate`, `topological_order` / `encode_scene`
    using the `id → index` mapping, migration of existing `.scene.json` / `.prefab.json`, tests;
  - `prism-editor-api`: assign ids on create / duplicate / instantiate, delete every `remap[]` path, renumber in `extract_subtree`;
  - the frontend only needs to confirm that `parent` is still a `number` (the semantic change is transparent to TS).
- **PR-2 (optional)**: an optional RSCN id table, only when runtime entity references are needed.

#### Open questions

- After `extract_subtree` renumbering, should prefab-instance "diffing" match by structural path instead of id—decide when the prefab workflow actually lands.

### 10.14 Project configuration files (`settings.ini` / `scenes.json`)

There are only two project-level configuration files, both under `assets/`:

| File | Purpose | Format |
|------|------|------|
| `assets/settings.ini` | `[app]` / `[build]` / `[render]` / `[window]` sections | custom INI dialect (see below) |
| `assets/scenes.json` | scene manifest (order = build = launch order) | `{"scenes":[{"name","path","enabled"}]}` |

**Why not TOML**: the configuration has only a few dozen scalar fields, while TOML pulls in the whole
`toml` + `toml_parser` + `winnow` + `toml_datetime` chain (60–80 KB in the player tree). The scene manifest is an array anyway,
and `serde_json` is already in the tree (`.scene.json` / `.pak.meta.json`), so switching to JSON adds no dependency. `serde_json` stays, and
**`toml` no longer appears in any crate** (`cargo tree -p prism-app -e normal | grep toml` must be empty).

#### Internal resolution / integer scaling (`[render]`, #613)

2D projects draw at a design resolution (`projects/demos/fishing` is 640×360), but windows are often much larger:
rendering directly at window resolution on 2000×1125 and upscaling with nearest-neighbor gives a non-integer 3.125× ratio,
turning one art pixel into rectangles of varying size. Two keys separate "render resolution" from "output resolution":

```ini
[render]
; empty / off / native = render at window resolution (default, behavior identical to before)
internal_resolution = 640x360
; stretch = fill the window (non-square pixels); integer = largest integer multiple + centered letterbox
internal_scale_mode = integer
```

- It takes effect only when both axes are written; if only one is written, the other is derived from the window aspect ratio;
- the scene (ForwardPass's HDR / depth / normal intermediate targets, path-tracing accumulation) renders at the internal size,
  while the UI overlay still uses window size (text stays sharp);
- `PostPass` upscales the internal image to the window: `stretch` fills, `integer` takes an integer multiple of
  `min(window width/internal width, window height/internal height)` and centers it with black bars;
  with `integer` the sampler switches to nearest-neighbor, so pixel art is not blurred by interpolation;
- camera projection uses the **internal** aspect ratio (`RenderSettings::render_aspect`), otherwise 2D composition is stretched;
- the editor's Game view goes through the same chain (`EngineHost::render` + `GraphRenderer::set_internal_resolution`),
  so Play shows the same pixel ratio as the packaged build.

#### INI dialect (single implementation standard)

1. `[section]` lines, with `key = value` inside; section order does not matter;
2. full-line comments start with `#` or `;` (leading whitespace ignored); **inline comments are not parsed**,
   so `#` / `;` in paths are not truncated;
3. values may be double-quoted, with `\"` `\\` `\n` inside quotes; unquoted values run to the end of the line and are trimmed;
   unknown escapes (e.g. `C:\temp`) keep the backslash;
4. the parser **produces only `String`**; `u32` / `bool` / `f32` conversion is the caller's job, and failure uses the default + `log::warn!`
   (a broken config file must not prevent startup);
5. unknown sections / keys are ignored with a `log::debug!` entry (the editor writes
   `[build]` into the same file, so the engine must not error);
6. the same section appearing twice is **merged**, with the later value of a duplicated key winning.

On write-back, numbers/booleans are bare (`width = 1600`, `vsync = false`) and other strings are quoted
(`company = "Acme"`); comments are not in the model and are lost on write-back (same as the previous TOML round-trip).

There is exactly one implementation; the dialect changes only here:

- `crates/prism-ini` — parsing / write-back / type conversion (depends only on `log`, no business dependencies);
- the three consumers are all re-exports: `crates/prism-engine/src/config/ini.rs` — engine reads
  (`AppConfig::from_ini`); `crates/prism-editor-api/src/ini.rs` — the MCP / tool document model
  (preserving unknown sections); `editor/crates/prism-editor-core/src/ini.rs` — the editor data layer
  (separate workspace).

#### Load order and migration

Both files follow "new file first, old file read-only fallback":

1. `assets/settings.ini` → fallback `assets/settings.toml` (`log::warn!` prompting migration);
2. `assets/scenes.json` → fallback `assets/scenes.toml` (same; the old manifest
   `scenes.toml` inside `.pak` is also parseable, so old scene packs keep working).

**The write side produces only the new format**: neither the editor nor the build pipeline writes `.toml` anymore.

The one-time migration is done by `prism-build-pipeline build-scenes`:
`migrate_legacy_config(assets/)` rewrites `settings.toml` into `settings.ini` and `scenes.toml` into `scenes.json` when **the new file does not exist**,
printing one `migrated …` line; old files are kept (the engine prefers the new ones) and can be deleted manually once confirmed. Migration is idempotent
and will not rewrite on repeated runs.

## 11. Dependency budget (player tree)

"Lightweight" must be enforced by CI, otherwise it will inevitably creep back. The player binary's only top-level dependency is
`prism-app`; every extra crate it carries is one more crate per user project and per Android build
(historical peak: 255 crates, including rapier3d/parry3d, tokio, exr, jpeg decoders).

Rules:

| Rule | Enforcement |
|------|------|
| Build-time dependencies (`gltf` / `clap` / `rayon` / image codecs) must not enter the player tree | `scripts/check_deps.py` blacklist |
| Optional capabilities must be features, off by default | `prism-app/physics`, `prism-app/audio`, `prism-asset/async-io` |
| The workspace `image` dependency has zero default codecs; each crate declares only the formats it uses | Root `Cargo.toml` |
| Total crates in the player tree ≤ `MAX_PLAYER_CRATES` (currently 155) | Same |

The check runs separately from `cargo check` and only does dependency resolution (no `.spv`, no compilation):
`python3 scripts/check_deps.py`, wired into `scripts/pr_ready.py` and the CI `deps` job.
When over budget, **do not** raise the constant: either make the dependency a feature, or explain in the PR why it must ship with the player.

### Measured baseline

Desktop player binary (`projects/template`'s `template`, Termux aarch64, `cargo build --release`,
`lto = "thin"` + `opt-level = 3`). Reproduce:

```bash
cd projects/template && cargo build --release
ls -l target/release/template        # unstripped
cp target/release/template /tmp/p && strip /tmp/p && ls -l /tmp/p   # stripped (release shape)
```

| Version | Player-tree crates | Unstripped | Stripped |
|------|-------------|---------|---------|
| Initial (rapier3d + tokio + exr/jpeg + the whole audio stack on by default) | 255 | 15.49 MB | 10.37 MB |
| #532 dependency diet (rapier/tokio/exr removed) | 196 | 14.72 MB | 9.78 MB |
| After #532 + audio as a feature (off by default) | **144** | **10.38 MB** | **6.70 MB** |

Total: crates −44%, stripped size −35% (10.37 MB → 6.70 MB).

The two reductions have completely different benefit structures, which determines what to do next:

- **Build-time dependencies (rapier / tokio / exr / jpeg) barely change size**: −59 crates bought only
  −0.60 MB. Stop cutting scattered crates for size.
- **The audio stack is the size hog**: one feature switch is −3.09 MB (stripped −32%), because
  cpal + firewheel + symphonia (the full mp3/flac/vorbis/ogg/isomp4 codec set + FFT resampling)
  are the real runtime heavyweights. Size-sensitive projects (small games, demos) can turn it off.
- The remaining 6.7 MB is mainly the winit / x11 / wayland window stack, ash + the Vulkan loader,
  fonts and 2D (fontdue / tiny-skia)—all genuinely needed for rendering and windows, unless building headless.

**Conclusion**: dependency count and compile time are the "clean by default" metrics; **binary size comes from turning off big capabilities via features**,
not from deleting scattered dependencies.

---

## 12. Bricks: the project template decides which subsystems compile

The engine does not make trade-offs for projects. **Capability = bricks, `Cargo.toml` = blueprint**: whichever features a project template picks,
only those subsystems compile and link. The engine's own `default` stays "everything", so libraries and examples are not
held hostage by one project's taste; pruning comes from the **template**, not from deleting defaults.

### 12.1 Two mechanisms (do not mix)

| Layer | Mechanism | Example |
|---|---|---|
| **Subsystem** | cargo feature (compile time; genuinely no code generated / dependency linked) | `pathtrace`, `audio`, `physics`, `async-io` |
| **Component** | `ComponentRegistry` grouping (registered at runtime) | Component groups unused by a scene are not registered and are treated as "unknown components" on load |

Components must **not** be cargo features: N components means 2^N combinations, CI cannot compile them and templates cannot express them.
Component-level trade-offs go through registry grouping + scene cooking (unregistered groups are not extracted, and their code is not linked).

### 12.2 Current bricks

| Brick | feature | Default | What turning it off saves |
|---|---|---|---|
| Path tracing | `prism-render/pathtrace` | on | `pt_pass.rs` (~1.6k lines) + accumulation/BLAS-TLAS resources |
| SHaRC (NVIDIA RTXGI v1.6) | `prism-render/sharc` | on | NVIDIA RTXGI SHARC v1.6 world-space radiance cache (`sharc.rs` + resolve compute pipeline + 3 persistent GPU buffers). **On by default**: adds no new dependency. **No longer depends on `pathtrace`**: the cache has two peer consumers—PT training/query (`gi_mode = Sharc`) and raster ReSTIR GI depositing/querying on its own (`gi_mode = Restir`, see §6.8). Off = both real-time GI paths degrade to the baked probe volume (with one warning) |
| ReSTIR GI (raster) | `prism-render/restir_gi` | on | `restir_gi.rs` + `restir_gi.slang` (~1.2k lines Rust + ~590 lines slang) + double-buffered GI image + reservoir buffer. **Does not depend on `pathtrace`**: only requires `VK_KHR_ray_query`, so devices without an RT pipeline can still run it; without ray query, `gi_mode = Restir` falls back to the baked probe volume and warns once. `sharc` is an **optional** accelerator: when compiled, the cache serves as bounce endpoints; otherwise only a single bounce + sky |
| NRC | `prism-render/nrc` (depends on `pathtrace`) | off | NVIDIA Neural Radiance Cache integration |
| DLSS | `prism-render/dlss` | off | NVIDIA Streamline / DLSS super-resolution and ray reconstruction integration; also the loader for `sl.interposer` (without it, Vulkan calls are not intercepted) |
| DLFG | `prism-render/dlfg` | off | DLSS frame generation (depends on `dlss`) |
| NRD | `prism-render/nrd` | off | NVIDIA Real-Time Denoisers; reuses Streamline's dynamic-library loader and capability probing, but does not need the DLSS runtime |
| Audio | `prism-app/audio` + `prism-engine/audio` | off | cpal + firewheel + symphonia (~60 crates, −3.1 MB) |
| Physics | `prism-app/physics` | off | rapier3d + parry3d (~40 crates) |
| Async asset IO | `prism-asset/async-io` | off | tokio |
| Window backends | `prism-app/x11`, `prism-app/wayland` (re-exported through `prism-platform`) | both on | Keep only one on Linux: −0.46 MB (wayland only) to −1.05 MB (x11 only) |
| UI overlay | `prism-render/ui` (re-exported through `prism-app/ui`) | on | Glyph atlas + ui_overlay pipeline; when off, UI commands are still filled in and the render side ignores them. Text rasterization is decided by the backend below |
| Text raster A | `prism-render/ui-fontdue` (re-exported) | on | fontdue + ttf-parser (all platforms, ~90 KB); switching to `ui-directwrite` removes it |
| Text raster B | `prism-render/ui-directwrite` (re-exported, **Windows only**) | off | Rasterize with the system `dwrite.dll`; fontdue/ttf-parser stay out of the binary |
| Component group 3D | `prism-scene/group-3d` (re-exported through `prism-app`/`prism-engine`) | on | Lights/skybox/mesh/camera control/GI config are not registered, and their serde and reflection code is not linked |
| Component group 2D | `prism-scene/group-2d` | on | Sprites/line segments/tilemaps/2D cameras/2D physics (RigidBody2D/Collider2D) are not registered |
| Component group UI | `prism-scene/group-ui` | on | Node/Style/Text/layout types are not registered |
| IBL | `prism-render/ibl` (re-exported) | on | Environment cubemap convolution/irradiance/prefilter/BRDF LUT/HDR decoding are not compiled; degrades to 1x1 neutral gray with an unchanged descriptor layout. −64 KB for 3D |
| Skybox | `prism-render/skybox` (depends on `ibl`) | on | `skybox.vert/frag.spv` + pipeline; `SkyboxPass` degrades to an empty implementation with unchanged call sites |
| Shadow map | `prism-render/shadows` | on | `shadow_map_pass` is not compiled; forward's set 3 degrades to a 1x1 far-depth dummy |
| GTAO | `prism-render/gtao` | on | The half-resolution AO pass is not compiled; forward's set 4 degrades to a 1x1 white dummy |

**Text raster backends** (`ui` only decides "draw UI or not"; the backend decides "who produces glyph bitmaps"):

| | `ui-fontdue` (default) | `ui-directwrite` |
|---|---|---|
| Platform | All platforms | Windows only (enabling it on another target = `compile_error!`) |
| Glyph source | `fontdue` reads system font files (msyh/Noto/DejaVu…); `PRISMA_UI_FONT` can point at a project-bundled font | System `IDWriteFactory` → `IDWriteBitmapRenderTarget` (GDI interop) |
| Font family | Path-based candidate list | `Microsoft YaHei` → `Microsoft YaHei UI` → `SimHei` → `Arial` → `Segoe UI` fallback |
| Metrics | `horizontal_line_metrics` | `DWRITE_FONT_METRICS` |
| Form | `fontdue::Font::rasterize` gives coverage directly | White bitmap + black grayscale AA glyphs; coverage = `255 − R` |
| Extra dependencies | fontdue + ttf-parser (~90 KB) | `windows` bindings (`Win32_Graphics_DirectWrite` / `Gdi`, Windows targets only) |

Two hard rules:

1. **Mutually exclusive**: enabling both `ui-fontdue` and `ui-directwrite` is a `compile_error!`—otherwise
   fontdue/ttf-parser would silently stay in the binary and the switch would be a no-op.
2. **Enabling `ui` without any backend** is a legal combination: draw only screen-space quads, no text (saves 90 KB),
   and warn once at startup about which feature is missing.

Both backends only do **single-glyph cmap lookup + coverage rasterization**, and explicitly do not support: complex text shaping
(Arabic / Indic ligatures and contextual substitution), variable font axes, color emoji glyphs,
kerning / ligatures. Those require a HarfBuzz / `IDWriteTextLayout`-level shaping pipeline,
outside either backend's scope. The DirectWrite backend has so far **only been cross-compile verified**;
real hardware still needs a smoke test: text clarity (grayscale AA), DPI scaling, font fallback (using `Segoe UI` when Microsoft YaHei is absent).

**To be split** (ordered by benefit; each split must be added to the §12.2 table and the `check_features.py` matrix):

- `2d` / `3d`: `SpritePass` currently lives inside `ForwardPass`'s render pass; after splitting it into its own pass,
  2D projects can avoid compiling 3D entirely (forward/shadow/skybox/mesh/primitives).
- `gi` (probe baking), `screenshot`.
- `ui` (fontdue/tiny-skia + `ui_overlay`).

### 12.3 Hard rules

1. **Every feature must be independently disableable and still compile**—`scripts/check_features.py`'s matrix +
   the CI `features` job enforce this; a new feature must add a minimal combination.
2. **Defaults only grow**: new capabilities go into `default`; templates that want trimming set `default-features = false` explicitly;
   do not change engine defaults for one project's size (it would silently remove functionality from other projects).
3. **Disabling must be diagnosable**: when a disabled feature makes a capability unavailable, `log::warn!` at startup/runtime explains
   "which feature to enable", rather than silently degrading (`Subsystem::Audio` already does this).
4. **Template = the `Cargo.toml` feature list + `build_app()`'s subsystem list**; the two must agree
   (enabling a feature without installing the subsystem wastes compilation; installing a subsystem without the feature warns).

### 12.4 Size ledger (how many bytes each brick is worth)

Method: `scripts/size_report.py` temporarily rewrites `projects/template`'s feature list, builds with the same
`release` profile, and records the unstripped binary size. The difference comes only from features.

| Brick | Δ binary (unstripped) | Notes |
|---|---|---|
| `audio` (cpal+firewheel+symphonia) | **+4.14 MB** | The size hog, a runtime stack of 50 crates |
| `pathtrace` | +135 KB | 1.6k lines + SPIR-V + BLAS/TLAS |
| `physics` | +3 KB (measured before wiring) | After #522 `physics_runner` really steps rigid bodies; rapier is still compiled in only with `prism-app/physics` |
| base (all off) | 10.24 MB | See the breakdown below |

`ui-fontdue` → `ui-directwrite` (Windows / `x86_64-pc-windows-gnu` / `dist` profile +
`build-dist.sh`'s build-std, **stripped**):

| Configuration | Binary | Difference |
|---|---|---|
| `ui-fontdue` (default) | 3 145 728 B | baseline |
| `ui-directwrite` | 3 084 288 B | **−61 440 B (−60 KiB, −2.0 %)** |

What is saved is fontdue + ttf-parser; the `windows` binding (the dozens of DirectWrite/GDI imports) offsets part of it,
so the difference is smaller than the "fontdue ~90 KB" estimate in §12.2. Both numbers come from the same
profile, target, and build command; the difference comes only from the text backend.

Attribution by crate (`nm -S`, base build): our own code totals **~731 KB** (`prism-render` 205 KB,
`prism-scene` 192 KB, `prism-ecs` 107 KB, `prism-engine` 98 KB, the rest ~130 KB);
the remaining ~4.7 MB is the Rust baseline + window stack + third parties.

Two conclusions to remember:

1. **Lines of code ≈ size is wrong**. 1.6k lines of path tracing are worth only 135 KB; a physics module that is "not wired"
   is almost 0 (the linker removes it), but it still costs 40 crates of compile time and dependency surface.
2. **3D/2D splitting yields almost no size benefit** (3D passes total ~200 KB); its benefit is
   **dependency surface, compile time, API clarity**. What actually changes the binary's order of magnitude:
   big third-party pieces (audio 4.1 MB, UI/font stack, window stack), release profile (`panic = "abort"`,
   `lto = "fat"`, `codegen-units = 1`, strip).
3. **Components are not size units**. Components are POD data + reflection metadata; the whole `prism-scene` crate
   is only 192 KB; disabling one component does not change the binary (code is shared, and the linker only keeps called paths anyway),
   it only affects scene data and the registry. Component-level trade-offs are a **runtime complexity** question, not a size question.

### 12.5 Release profile layering (debug / release / dist)

Build configuration is also part of the "blueprint": different consumers use different profiles; do not force one release to carry everything.

| profile | Use | Key settings |
|---|---|---|
| `dev` | Development, editor | `opt-level = 1`, `line-tables-only` debug info (not for dependencies), `panic = unwind` |
| `release` | Editor / tools / CI | `opt-level = 3`, `lto = "thin"`, `panic = unwind` |
| `dist` | Players / Android packaging | `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`, `strip = "symbols"` |

`dev` keeps only line tables: workspace crates can still locate a panic's file:line, and dependency crates carry no DWARF.
Full debug info bloats `target/` and `incremental/` to several GB with little daily debugging benefit.

`panic = "abort"` **must not** go into `release`: the editor runs the engine in-process, and abort would crash the whole editor
instead of failing one viewport. Usage: `cargo build --profile dist` (the Android packaging scripts already use it).

Measured (`projects/template`, default features, Termux aarch64):

| Configuration | Unstripped | Stripped |
|---|---|---|
| `release` (thin LTO, cu=16, unwind) | 10.38 MB | 6.70 MB |
| `release` + fat LTO + cu=1 (still unwind) | 7.41 MB (−28.6%) | 5.64 MB (−15.7%) |
| `dist` (plus panic=abort, strip) | **6.61 MB (−36.3%)** | **5.12 MB (−23.5%)** |

Conclusion: **the gain comes mainly from fat LTO + a single codegen unit (−28.6%), not from panic=abort** (the latter saves another
0.81 MB). So the editor can stay on `release` (keeping unwind and backtraces) and only players/Android use `dist`.
On this machine, fat LTO barely changed build time (1m31s vs 1m35s), so no need to worry about CI.

#### What the 5 MB is made of (`dist`, 5.12 MB stripped)

By section (`size -A`): `.text` 3.86 MB (75%), `.rodata` 0.49 MB (10%),
`.eh_frame` 0.35 MB (7%), `.rela.dyn` 0.20 MB (4%), `.data` 0.15 MB.

By code origin (`nm -S` attribution; after fat LTO symbols are attributed to callers, so approximate):

| Category | Size | Share | Reducible |
|---|---|---|---|
| Rust baseline (core/std/alloc/hashbrown/gimli) | 0.88 MB | 22% | Only via `build-std` + panic_abort (nightly) |
| Engine code (prism_*) | 0.85 MB | 22% | Split into features |
| Window stack (winit/wayland/x11/smithay) | 0.84 MB | 21% | Linux tax; on Windows it becomes system DLLs, on Android android-activity |
| Other third parties | 0.49 MB | 12% | Case by case |
| **Logging regex (env_logger → regex)** | **0.47 MB** | **12%** | **Easiest cut: drop env_filter/regex** |
| Fonts/2D (tiny_skia/ttf_parser/fontdue) | 0.23 MB | 6% | `ui` feature; on Windows switch to `ui-directwrite` (−61 KiB) |
| Serialization (serde_json; `toml` removed, −128 KB) | ~0.02 MB | <1% | Shared by scene documents/manifest, essentially at the floor |
| Compression (zstd/flate2) | 0.04 MB | 1% | Almost not worth it |

**Window stack pruned by platform** (measured, `dist` stripped):

| Combination | Binary | Notes |
|---|---|---|
| X11 + Wayland (default) | 5.12 MB | Both session types can start |
| Wayland only | 4.66 MB (−0.46 MB) | Cannot start in a pure X11 environment |
| X11 only | **4.07 MB (−1.05 MB)** | A pure Wayland environment needs XWayland as a fallback |

- **Windows / Android never pay this cost**: winit's platform dependencies are pruned by target;
  `cargo tree --target aarch64-linux-android` has zero wayland/x11, and Windows uses system DLLs.
- Only Linux has to choose, and **must choose one** (winit fails to compile without a backend), so the template's minimal combination is
  `--no-default-features --features x11`, not "all off".

**Windows: winit can be dropped entirely** (`win32-native`, 2026-10). A window backend using only Win32 APIs was added to
`prism-platform` (`RegisterClassExW` / `CreateWindowExW` / `PeekMessageW` message pump), `prism-app`'s `winit` became an optional dependency,
both backends share the same engine/render/shutdown logic, and events land in a neutral `PlatformEvent` before entering `InputManager`.
With `win32-native` enabled, winit's count in the dependency tree is 0 (`cargo tree --target x86_64-pc-windows-gnu | grep -c winit`),
not "compiled in and then removed by LTO".

Measured (`bash scripts/build-dist.sh projects/template x86_64-pc-windows-gnu`, dist + strip):

| Windows window backend | `template.exe` | Notes |
|---|---|---|
| `winit-window` (default) | 3,146,240 B (3.00 MB) | winit 0.30 + system DLLs |
| `win32-window` (`win32-native`) | **2,978,816 B (2.84 MB)** | −167,424 B (−163.5 KiB / −5.3%) |

- Much smaller than Linux's `winit ≈ 375 KB`: on Windows, winit's window/message/DPI handling relies entirely on
  user32/dwmapi, unlike Linux which must bundle the whole x11/wayland/smithay protocol stack.
- Cost: the Win32 backend lacks IME, drag-and-drop, multi-monitor DPI smoothing, taskbar progress, and touch;
  pointer lock is "hide + ClipCursor + recenter" rather than Raw Input. See
  the module docs in `crates/prism-platform/src/win32.rs` and `AGENTS.md`.

Conclusion: **only 0.85 MB of the 5 MB actually belongs to "the engine"**; the rest is the language baseline, the window stack, and two
replaceable third-party pieces (regex logging, font stack). Next priorities: remove env_logger's regex
(−0.47 MB) > the `ui` brick (−0.23 MB) > serialization (−0.14 MB).

#### Cross-platform binary size (`dist` profile, stripped)

| Target | Artifact | Size | text |
|---|---|---|---|
| Linux x86_64 / aarch64 (x11 + wayland) | `template` | 5.12 MB | 4.97 MB |
| Linux (x11 only) | `template` | 4.07 MB | — |
| Windows `x86_64-pc-windows-gnu` (winit) | `template.exe` | 4.11 MB | 4.09 MB |
| Windows `x86_64-pc-windows-gnu` (`win32-native`, no winit) | `template.exe` | See "Window stack pruned by platform" above | — |
| Android `aarch64-linux-android` | `libtemplate_core.so` | **3.26 MB** | 3.15 MB |

- Windows and "Linux x11-only" are nearly identical (4.11 vs 4.07 MB); the difference is the wayland stack—**confirming that
  the 0.84 MB window stack exists only with both Linux backends**.
- Android is smallest (no winit platform stack, only NDK glue); APK size is separate (`.pak` assets + Java shell).
- Cross-compilation is reproducible on this machine: Windows uses the mingw linker; Android uses
  an NDK processed by `scripts/make-ndk-host-native.sh` (aarch64 host + x86_64 prebuilt shim),
  and requires setting `CC_/CXX_/AR_aarch64_linux_android` together (`android-activity`'s build.rs looks for `clang++`).

#### Reclaiming the regex in logging (env_logger → env_filter → regex)

`env_logger`'s default `regex` feature exists only for `RUST_LOG` regex filtering, yet it pulls in the whole regex engine
(regex-automata + regex-syntax + aho-corasick). We only use directives like `module=level`;
with `regex` off, `env_filter` uses **substring matching**, which is sufficient.

- The root workspace's `env_logger` switches to `default-features = false`; build tools explicitly enable
  `["auto-color", "humantime", "regex"]`, **the player runtime enables only `["auto-color", "humantime"]`**.
- Android has a second path: `android_logger`'s defaults also bring in `env_filter/regex`,
  turned off as well.
- Cost: `RUST_LOG` can no longer contain regexes (e.g. `prism_(render|engine)=debug`);
  enable the `regex` feature on the relevant crate when needed.

Measured (`dist`, stripped):

| Platform | Before | After | Δ |
|---|---|---|---|
| Linux (x11+wayland) | 5.12 MB | **4.47 MB** | −0.66 MB (−12.8%) |
| Windows | 4.11 MB | **3.26 MB** | −0.85 MB (−20.7%) |
| Android | 3.26 MB | 3.26 MB | 0 (the regex code was already removed by fat LTO, but it still costs compile time and dependency surface) |

#### Remaining "water" map (`dist`, stripped, Linux)

| crate / source | Size | Notes / reducibility |
|---|---|---|
| `winit` | 375 KB | The platform backend itself; already prunable by x11/wayland |
| `core` + `std` + `alloc` + `hashbrown` | 678 KB | Rust baseline, only nightly `build-std` |
| `prism_scene` | 244 KB | Component registry + serde for all components: **the next big item** |
| `wayland_client` / `x11_dl` / `x11rb` / `smithay` / `tiny_skia` | ~450 KB | Only with both Linux backends; disappears with x11-only or Windows |
| `prism_render` / `prism_engine` / `prism_app` / `prism_ecs` / `prism_reflect` | ~640 KB | The engine body |
| `serde_json` | not listed separately | Shared by `.scene.json` / `scenes.json` / `.pak.meta.json`, required (`toml` removed, measured −128 KB) |
| `fontdue` family (`ttf_parser`, etc.) | ~90 KB→64 KB | `ui` feature; on Windows `ui-directwrite` uses system DirectWrite (measured −61 KiB) |
| `gimli` / `addr2line` | 33 KB | std panic backtraces; needs build-std |
| `libloading` | 33 KB | ash dynamically loads `vulkan-1`; a required cost |
| `image` + png | ~21 KB | Screenshots; could become a `screenshot` feature |
| `zstd` / flate2 / crc32 | ~44 KB | `.pak` decompression; required for asset compression |

Based on this map, **what can still be reliably reclaimed** is: `screenshot` (~20 KB), `prism_scene` component grouping (hundreds of KB,
but registry grouping must be designed first). The rest is either required by the platform or needs nightly.

#### Preset tiers (`dist`, stripped, x11+wayland; crate counts include workspace crates)

| Tier | Feature combination | Crates | Size |
|------|-------------|-------|------|
| 3D full | `pathtrace,ui,x11,wayland,group-3d,group-2d,group-ui` | 148 | 4.47 MB |
| 2D + UI | `pathtrace,ui,x11,wayland,group-2d,group-ui` | 148 | 4.27 MB (−197 KB) |
| 2D minimal | `pathtrace,x11,wayland,group-2d` | 141 | 4.14 MB (−331 KB) |
| 2D minimal + X11 only | `pathtrace,x11,group-2d` | 130 | 3.02 MB (−1.45 MB) |
| **2D minimal + X11 only + no ray tracing** | `x11,group-2d` | 128 | **2.89 MB (−1.58 MB, −35%)** |

These tiers are the options in the hub's "New Project" wizard (2D/3D × UI × path tracing × window backend).

**Wayland is the single largest item on Linux**: winit's `sctk-adwaita` (client-side decorations) brings in
`tiny-skia` + `ttf-parser` + `wayland-client`, about 1.1 MB total. An X11-only build does not even need font rasterization—so the
"Linux minimal" tier pushes 2D projects to just over 3 MB. **Presets are semantic promises**:
adding a feature to a tier adds weight to every project in that tier, and the rationale must be written down.

#### Component grouping measurements

| Combination | Size | Notes |
|---|---|---|
| All groups (default) | 4.47 MB | 3D + 2D + UI components all registered |
| `group-3d` off | **4.27 MB (−197 KB)** | serde and reflection code for lights/skybox/mesh/GI config is no longer linked |
| 3D off, then UI off | another −50 KB | Node/Style/Text |

**Key pitfall**: `prism-scene`'s default features re-enable component groups anywhere that depends on it directly (`prism-engine`, the three user projects),
so `prism-app`'s `--no-default-features` stops working. Therefore every place that directly depends on
`prism-scene` must write `default-features = false`—the same class of mistake as the `prism-game-api` incident, now fixed everywhere in the project.

The reason component grouping's benefit is far smaller than the "20 KB per component" intuition: serde's deserialization code is shared across types
(the same `serde_json::from_value` generic instantiation is merged by ICF), so disabling one group only reclaims the parts unique to that group.

#### 3D pass-level pruning: −24 KB of code, but the stripped file is unchanged

After `ibl` (#543, −64 KB), `shadow_map` / `gtao` / `skybox` also became features
(`skybox` depends on `ibl`). Method: temporarily change `projects/template`'s `prism-app` to
`default-features = false, features = ["x11", "group-3d", ...]`, same worktree,
same target, `--profile dist` (fat LTO + cu=1 + panic=abort + strip).

| Combination | File bytes | Δ file | `.text`+`.rodata`+… total | Δ code |
|---|---|---|---|---|
| All on (ibl+skybox+shadows+gtao) | 3,022,664 | — | 2,993,578 | — |
| `shadows` off | 3,022,664 | **0** | 2,988,274 | −5,304 |
| `gtao` off | 3,022,664 | **0** | 2,979,746 | −13,832 |
| `skybox` off | 3,022,664 | **0** | 2,988,426 | −5,152 |
| All off (no ibl/skybox/shadows/gtao) | **2,957,128** | −65,536 | 2,931,602 | −61,976 |

Two points to remember:

1. **The code really shrank, but the file did not lose a single byte**: aarch64 LOAD segments are 64 KB aligned
   (`readelf -l`'s `Align 0x10000`), and the RW segment offset follows the rounded-up end of the RO segment, so any shrink ≤ 33 KB
   lands entirely in inter-segment padding. Only `ibl` (−64 KB) crossed the 64 KB step.
   So these three **must not be sold as size presets** (writing them into a preset promises a benefit that does not exist);
   treat them as capability switches only: projects that do not need shadows, AO, or a skybox compile 24 KB less code + 3 fewer SPIR-V modules,
   and the difference is visible only on 4 KB-aligned platforms (x86_64 / Windows) and in debug builds.
2. This is also why **3D/2D splitting's value is dependency surface, compile time and API clarity, not binary size**—
   §12.4's conclusion holds again here.

Numbers use stable `--profile dist` (same basis as §12.5's left column); `scripts/build-dist.sh`'s
build-std tier has smaller absolute values, but the 64 KB segment-alignment conclusion is unchanged.

Only two things remain movable:

- Rust baseline ~678 KB (core/std/alloc/hashbrown)—needs nightly `build-std`, which raises the contribution barrier (§12.5);
- `serde_json` (shared by config, scene manifest, `.pak` path manifest; `toml` was removed entirely, measured −128 KB)—
  further reduction would mean a binary config format and a noticeably worse authoring experience.

#### Release tier: nightly `-Z build-std` (`scripts/build-dist.sh`)

Recover another 9–10% in release packages: `-Z build-std=std,panic_abort` makes std go through fat LTO with the project,
and `-Cpanic=immediate-abort` deletes the large panic formatting (`core::fmt`) chunk.

| Configuration | stable `dist` | build-std + immediate-abort | Change |
|------|--------------|------------------------------|------|
| 3D full | 4,465,304 | 4,006,440 | −459 KB (−10.3%) |
| 2D minimal + X11 only | 2,891,032 | 2,628,768 | −262 KB (−9.1%) |

Key design: **flags go only on the command line, never into any `Cargo.toml`**.
Writing `panic = "immediate-abort"` into a profile makes stable cargo refuse to parse the whole manifest,
whereas `RUSTFLAGS="-Zunstable-options -Cpanic=immediate-abort"` affects only that one build—
so development stays on stable (`cargo run` unchanged), and only `scripts/build-dist.sh` needs nightly + `rust-src`.

Dist numbers in the size ledger (§12.4) refer to the stable tier by default; release packages multiply by ~0.9 according to this section.

#### Extreme size tier: `--size` (for competitions / demos)

```bash
bash scripts/build-dist.sh --size projects/template
powershell -File scripts/build-dist.ps1 -Size projects/template   # Windows host, equivalent
```

= `tiny` profile (`opt-level = "z"` + fat LTO + `cu=1` + `panic=abort` + strip)
+ `-C llvm-args=-inline-threshold=0 -C llvm-args=-inlinehint-threshold=0`
+ `build-std` + `immediate-abort`.

| Tier (aarch64 Linux, 3D full) | Size |
|------|------|
| stable `dist` (opt-level 3) | 4,465,304 |
| `dist` + build-std (`build-dist.sh`) | 3,940,904 |
| **`tiny` + build-std (`--size`)** | **2,695,720 (−40%)** |

Cost: disabling inlining slows CPU hot paths by 10–30%; GPU-bound scenes barely notice, CPU-bound scenes notice clearly.
To get both, set key crates back to `opt-level = 3` individually:

```toml
[profile.tiny.package.prism-engine]
opt-level = 3
```

**State the boundary clearly**: `--size`'s realistic range is **2.5–3.2 MB**. True 64k intros (30–60 KB)
rely on `no_std` + hand-written system APIs + a Crinkler-style PE compressor, which is not the same thing as these flags;
do not advertise the "size tier" as competition-grade in the wizard.

#### Compression shell (`--pack`)

```bash
bash scripts/build-dist.sh --size --pack projects/template   # Linux / macOS host
powershell -File scripts/build-dist.ps1 -Pack projects/template   # Windows host
```

After building, run `upx --best --lzma <bin>` once on the artifact, then print the before/after sizes and compression ratio.
It is orthogonal to `--size` / `--profile` and is an optional **last** step; positional arguments are unchanged.

**Use it only for dist / release builds, and the artifact must be code-signed.** This is a precondition, not a suggestion:

- UPX is essentially a **self-extracting shell**: the original binary is compressed into a stub, which decompresses it in memory at runtime
  and jumps in. So "it runs" depends on the stub `mprotect`ing an executable region—exactly the classic heuristic that mainstream EDR / antivirus
  products use to detect "packed malware" (UPX has long been a favorite packer for malicious samples).
- **Unsigned packed artifacts will very likely be flagged by Windows Defender**, and signing (Authenticode /
  a timestamped code-signing certificate) usually downgrades "unknown packer" to "known publisher", which is the only reliable mitigation.
  Without a certificate, do not add `--pack` to Windows artifacts.
- Some AV products **may still flag it even after signing** (especially strict enterprise environments): that is an external factor
  the engine cannot fix; test in the target environment before release.
- When `upx` is missing the script **only warns and does not fail**, keeping the uncompressed artifact: `--pack` is optional and
  should not turn CI or someone else's machine red because a tool is missing. Android artifacts (`lib*.so`) are skipped automatically—
  the system loader / Play scanning dominates, and a packer only buys false positives and slower first page-in.

**Costs** (skip it if you cannot accept them):

| Cost | Notes |
|---|---|
| Startup time +10~30 ms | First decompression + verification; more noticeable on cold starts and SSDs |
| Memory peak | The moment of decompression holds both the compressed data and the decompressed image (roughly 2× the code section) |
| Debugging difficulty | Backtraces are all stub addresses; symbols/strip info do not match the runtime image; forget about crash dumps too |
| Anti-cheat / some store channels | Packers trigger signature-consistency checks or manual review; do not assume they are fine in a managed release chain |
| Size benefit varies by configuration | Artifacts already fat-LTO'd and stripped usually compress to 40%–55%, not a fixed number |

**Do not** add `--pack` to the default flow, the CI `dist-size` job, or any `[profile.*]`:
it trades size against startup time and is not the same class of technique as "smaller code"; mixing it into the default path only turns
"why was the artifact blocked by antivirus" into a problem nobody can reproduce.

One implementation detail: UPX **overwrites in place** the artifact under `target/<target>/<profile>/<pkg>`,
so running another build without `--pack` in the same target does not "decompress" it back
(cargo considers the artifact up to date); for a clean artifact, change a line of code to trigger relinking, or delete the file.

## 13. 2D physics geometry kernel

`prism-physics2d` provides deterministic 2D collision geometry that does not depend on ECS, rendering, or a rigid-body backend;
`prism-engine::physics2d` and `prism-game-api::physics2d` re-export the same implementation.
This lets the static Player and editor dynamic project modules share game movement code, without sending room geometry across the ABI,
and without making plugins link a second copy of `prism-engine`.

`prism-scene::Collider2D` is the AABB collider at the scene-component layer, supporting local offsets, triggers, and layer masks;
its world bounds share the XY transform rules with `SpriteRenderer::world_bounds`, including local scale, rotation, and pivot.
`GameWorld::colliders2d`, `overlap2d`, and `move_and_slide2d` use the same interface in the static Player and project modules.
Collision queries compose local transforms on the fly along the current `Parent` chain, ensuring parent/child positions modified earlier in the same gameplay step
also participate in collision; runtime entities without a local transform fall back to `LocalToWorld`. Thus sprites, particles, doors, and triggers remain consistent with collision
under parent translation, rotation, and scale.

The engine splits gameplay updates into a per-frame `Schedule` and a fixed-step `fixed_schedule`. `App::add_fixed_game_system`
registers fixed-step gameplay; the engine caches this frame's input at `begin_frame`, and the fixed step consumes press/release edges only once,
while each substep can read the current held state. Project modules join the same fixed step through the ABI's optional `fixed_update` callback,
while the normal `tick` still runs once per render frame; `InputSnapshot::interpolation_alpha` provides the fixed-step remainder for render interpolation.

Fixed-step-driven 2D entities can carry the runtime component `TransformInterpolation2D`. The engine saves their local
XY position before the fixed step, and sprite extraction and `VirtualCamera2D` target reads interpolate by `interpolation_alpha`; simulation and collision always
read the current `LocalTransform`, and the render position never writes back into physics state. Projects enable it through
`GameWorld::enable_transform_interpolation2d` and call `snap_transform_interpolation2d` after respawn, teleport, or room-entry landing points
to avoid interpolating across positions. The component is derived runtime state and is not written to `.scene.json` / RSCN.

Coordinates use the XY plane, +X right and +Y up; Z does not participate in 2D geometry. Units are decided by the caller,
so EchoWell can use pixels directly while scene physics can still use meters. `Aabb2`'s `min` / `max`
are world-space bounds; boundary contact does not count as overlap. 2D ray distances and movement deltas use the same world units.

The first slice provides AABB overlap, AABB ray casts, and continuous `move_and_slide` on static AABB obstacles:
the movement sweep solves for contact time directly, not via per-pixel substeps; the result includes the corrected position, actual displacement, contact normal,
and wall/ground/ceiling flags. Normal movement treats obstacles as immovable geometry input, and triggers are excluded from the solid list by the caller.
Circle/polygon colliders, dynamic rigid bodies, collision events, and the Rapier2D runtime
are still unimplemented. The 2D virtual camera proposed in GitHub issue #1160 (damping, dead zone, bounds clamping, pixel snapping)
is already provided by the `VirtualCamera2D` scene component and engine system; render-pass pruning for pure 2D mode is still unimplemented,
and is not covered by the current AABB geometry kernel.

## Embedded SPIR-V Crate Split (`prism-render-shaders`, #1225)

Compiled SPIR-V byte slices (`assets/shaders/*.spv`) are embedded in the zero-dependency leaf crate `crates/prism-render-shaders` (`pub const <NAME>_SPV: &[u8]`) rather than directly inside `prism-render` or `prism-build-pipeline`. Its `build.rs` keeps the missing/stale `.spv` gate, while `shader_bindings/` stays inside `prism-render` so shader layout changes still type-check together with the passes.
