# Prisma mobile ray tracing + real-time GI modular pipeline design

> Goal: mobile-first, visual quality first, stable 60fps. Modular render features (RT toggleable, GI switchable).
> Porting reference: TruvisRenderer's SHARC world-space radiance cache (v1.6, NVIDIA RTXGI port) and RayQuery inline usage.
> This file is an **architecture blueprint**, confirm before implementation. All shader ABIs follow Truvis `.agents/rules/shader-abi-layout.md` spirit (explicit padding, cross-platform layout verification).

---

## 0. Design constraints and decisions

| Item | Decision |
|------|----------|
| Pipeline architecture | **New RenderGraph** (modular pass nodes), abandon existing monolithic `renderer.rs` (995 lines) |
| Ray tracing model | **RayQuery inline** (not RT pipeline) — mobile RT pipeline too heavy, inline query only does visibility, best cost/performance |
| Ray tracing scope (v1) | **Shadows + reflections** both (user decision) |
| GI scheme | **Directly port SHARC** (world-space hash-grid voxel radiance cache) — most modern, Truvis validated |
| Base layer | Always-running **raster GBuffer → deferred/forward+ PBR**, quality first |
| Quality vs performance | Quality first, target stable 60fps (not extreme performance) |
| TBDR-friendly | transient attachment + subpass inline (see §6) |
| Bindless | descriptor-indexing runtime array (existing `bindless.rs` upgraded to separate SRV + fixed sampler array, see §5) |

---

## 1. Modular pipeline topology

```
                          ┌─ [RT: off] ── walk raster PBR direct lighting + IBL
    Scene ──▶ GBuffer ────┤
    (raster)              ├─ [RT: on] ── RayQuery pass:
                          │                  · Soft shadows (visibility query vs TLAS)
                          │                  · Reflections (query vs TLAS, sample hit material)
                          │
                          └─ [GI mode] ── SHARC cache:
                                        · Off     → IBL environment only
                                        · Update  → sparse path writes cache (screen = Off)
                                        · On      → next step bounce cache query
```

**RenderGraph nodes (modular, pluggable):**
1. `GbufferPass` (raster, base) — always runs
2. `RayQueryPass` (compute, raster GBuffer input) — RT switch controlled, outputs shadow/reflection contribution
3. `SharcUpdatePass` (compute, RayQuery driven) — runs when GI=Update/On
4. `SharcResolvePass` (compute, 1D dispatch) — cross-frame merge / eviction
5. `LightingPass` (raster/compute) — composite direct (±RT) + IBL + SHARC GI → HDR
6. `PostPass` (tone map / SDR) — output

Each pass is a `RenderPassNode` trait implementation with `inputs/outputs/execute()`. Switch = whether node is added to graph.

---

## 2. GBuffer layout (ported from Truvis, Android format adaptation)

| Attachment | Format | Content |
|-----------|--------|---------|
| GBufferA | `R16G16B16A16_SFLOAT` | world normal.xyz + roughness |
| GBufferB | `R16G16B16A16_SFLOAT` | world pos.xyz + linear depth |
| GBufferC | `R8G8B8A8_UNORM` | albedo.rgb + metallic |

> Mobile note: `RGBA32F` GBuffer has high TBDR tile memory pressure but is readable; if bandwidth tight, GBufferA/B can downgrade to `R10G10B10A2` + normal octa-pack (quality first, keep SFLOAT first).

---

## 3. RayQuery ray tracing (mobile hybrid)

**Only inline RayQuery, no RT pipeline.**

### 3.1 Shadows
```hlsl
RayQuery<RAY_FLAG_ACCEPT_FIRST_HIT_AND_END_SEARCH | RAY_FLAG_SKIP_CLOSEST_HIT_SHADER> rq;
rq.TraceRayInline(tlas, RAY_FLAG_NONE, 0xff, origin, t_min, dir, t_max);
rq.Proceed();
bool shadowed = rq.CommittedStatus() == STATUS_CANDIDATE_NON_OPAQUE
             || rq.CommittedStatus() == STATUS_COMMITTED_TRIANGLE_HIT;
```
- One query per light (or clustered light batching)
- α-test materials: need `RAY_FLAG_FORCE_OPAQUE` or non-opaque candidate (refer to Truvis `material_access.slangi` alpha mask handling)

### 3.2 Reflections
- For rough surfaces: 1 query takes hit point, samples hit material albedo/emissive + recursive SHARC GI query
- Mirror: can do 1-bounce query, query SHARC instead of recursive ray at low roughness
- Quality first: reflections can do 1 query per pixel (60fps budget)

### 3.3 TLAS
- Mobile `VK_KHR_acceleration_structure`: build/update BLAS+TLAS each frame from PrismaECS mesh components
- Instance mask distinguishes opaque / alpha-mask (matches Truvis `FORCE_OPAQUE` convention)

---

## 4. SHARC GI (directly port Truvis, adapt to mobile)

### 4.1 Three persistent buffers (independent descriptor set `SHARC_SET_NUM`)
| Buffer | Type | Purpose |
|--------|------|---------|
| `sharc_hash_entries` | `RWStructuredBuffer<uint64>` | spatial hash key (0=empty slot), 64-bit atomic contention |
| `sharc_accumulation` | `RWStructuredBuffer<SharcAccumulationData>` | this-frame atomic accumulation (u32-quantized radiance) |
| `sharc_resolved` | `RWStructuredBuffer<SharcPackedData>` | cross-frame accumulation (fp16 radiance + frame metadata) |

Element types (strict ABI, from Truvis `realtime_rt.slangi`):
```hlsl
struct SharcAccumulationData { uint4 data; };           // xyz=quantized radiance, w=sample count
struct SharcPackedData { float16_t4 radianceData; uint sampleData; uint sampleDataExt; };
```

### 4.2 File mapping from Truvis source
| Truvis source | Prisma target |
|---------------|---------------|
| `sharc_common.slangi` | `shaders/slang/sharc/common.slang` |
| `sharc_hash_grid.slangi` | `shaders/slang/sharc/hash_grid.slang` |
| `sharc_integration.slangi` | `shaders/slang/sharc/integration.slang` |
| `realtime_rt.slangi`(buffer ABI section) | `shaders/slang/sharc/buffers.slang` |
| RayQuery call(`raygen_direct_lighting`) | `shaders/slang/rt/ray_query.slang` |

### 4.3 Mobile adaptation points
- **64-bit atomics**: need `shaderBufferInt64Atomics` (Android Vulkan 1.2+ Adreno/Mali support). Truvis already forces `SHARC_ENABLE_64_BIT_ATOMICS=1`, we do the same.
- **Voxel capacity**: mobile VRAM small, capacity set to 1/4~1/8 of desktop (e.g. 2^20 instead of 2^23), `sharc_scene_scale` adjusted coarser.
- **Resolution downsample**: SHARC Update/Query can run at half resolution (quality-first accepts slight blur).
- **Shader compilation**: SHARC uses `#define SHARC_UPDATE 1` + push-constant `sharc_phase` phased (Update/Resolve), same as Truvis, no multiple pipelines split.

### 4.4 GI mode state machine (ported from Truvis `realtime_rt` constants)
```
SHARC_MODE_OFF     = 0  // never touch SHARC buffer, walk IBL
SHARC_MODE_UPDATE  = 1  // only maintain cache, screen = Off
SHARC_MODE_ON      = 2  // next step bounce cache query
SHARC_PHASE_NONE/UPDATE/RESOLVE  // same compute dispatch phased
```

---

## 5. Bindless upgrade (separate SRV + fixed sampler)

Existing `bindless.rs` (combined `SamplerCube[]`) → upgrade to Truvis style:
- `bindless_srvs[]`: `texture_2d` / `texture_cube` separate sampled image arrays
- `global_samplers[]`: fixed small sampler array (linear-wrap / linear-clamp / nearest / shadow)
- `BindlessSrv::sample(handle, uv, samplerType)` internally `NonUniformResourceIndex`
- **`INVALID_TEX_ID` fallback**: invalid handle → `INVALID_TEX_COLOR` (magenta), avoid reading undefined descriptor crash (critical on mobile)

Rust side `TextureHandle` changed to `struct { u32 index }` + `INVALID` constant.

---

## 6. TBDR-friendly (mobile extreme optimization)

| Optimization | Approach |
|-------------|----------|
| Transient attachment | all GBuffer/depth use `LAZILY_ALLOCATED`, don't occupy system memory |
| Subpass inline | GBuffer → Lighting same renderpass, input attachment passed, no writeback to main memory |
| Compact format | HDR uses `R11G11B10` / `RGB10A2`, GBuffer prioritizes SFLOAT (quality) |
| Avoid full-screen readback | RayQuery/SHARC walk compute, don't force GBuffer resolve to readable |
| Tile parallel | raster GBuffer naturally tile-friendly; compute pass pay attention to tile locality |

> Stable 60fps first: if some mid-tier GPU can't handle full features, downgrade via switches (RT off / GI=Off), don't sacrifice architecture.

---

## 7. Implementation milestones (suggested order)

1. **RenderGraph skeleton**: `RenderPassNode` trait + graph construction + base GBuffer pass (raster PBR, quality first) [done]
2. **Bindless upgrade**: `bindless.rs` → separate SRV/sampler + INVALID fallback [done]
3. **TLAS construction**: build BLAS/TLAS from PrismaECS mesh (acceleration-structure module) [done]
4. **RayQuery shadows**: `RayQueryPass` + `rt/ray_query.slang` (shadows first, most stable) [done]
5. **RayQuery reflections**: extend `RayQueryPass` (reflections) [done]
6. **SHARC GI (RTXGI v1.6)**: modular `sharc` feature + `sharc.rs` (1M entries Hash Grid + Resolve Pass) + Vulkan 1.2+ `buffer_device_address` / 64-bit atomic support [done]
7. **Sponza validation and goldenframe comparison**: `projects/demos/sponza` user project access + tuning UI + offscreen actual rendering and RTXGI official reference image quantitative comparison [done]
8. **Lighting compositing and denoising extension**: direct(±RT) + IBL + SHARC mix [in progress]
9. **CI extension**: SHARC/GI shader compilation + drift guard (reuse existing slang workflow) [done]

---

## 8. Sponza and RTXGI SHARC validation and goldenframe specification

- **Architecture isolation**: rendering pipeline changes and RTXGI integration strictly belong to engine layer (`sharc.rs` and `render_graph` in `crates/prism-render`), user project `projects/demos/sponza` only contains assets and tuning UI (`tuning_ui.rs`), does not overstep into pipeline logic.
- **Goldenframe specification comparison**:
  - Test suite: `crates/prism-render/tests/sharc_sponza_golden.rs`
  - Differential threshold: channel tolerance 12, 4x gain heatmap generation.
  - Output artifacts: `sponza_raster.actual.png`, `sponza_pt_cache_off.actual.png`, `sponza_pt_sharc_on.actual.png`, `sponza_sharc_diff.diff.png`, `sponza_raster_vs_pt.diff.png`, `rtxgi_reference_diff.diff.png`.
  - RTXGI official reference image comparison (1920x940): changed pixels 94.32%, average difference 103.752, effectively validating SHARC's smoothing convergence capability for indirect light Monte Carlo variance.
