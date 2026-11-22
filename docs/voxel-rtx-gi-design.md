# Voxel-specific Hardware RTX GI Design (RT Core pipeline + voxel face cache + ReSTIR / DLSS-RR)

> **Tier**: L2 subsystem design · Rendering advanced
> **Status**: Optional design blueprint (for desktop / console platforms supporting hardware ray tracing RT Core)
> **Related**: [DESIGN.md](DESIGN.md) §6, [rtx-dlss-integration-design.md](rtx-dlss-integration-design.md), [mobile-raytracing-gi-design.md](mobile-raytracing-gi-design.md), [voxel-software-gi-design.md](voxel-software-gi-design.md)

This document defines a voxel-specific hardware global illumination solution fully leveraging **NVIDIA RTX RT Core** (`VK_KHR_ray_tracing_pipeline` + RayGen/ClosestHit/Miss + SER) and `VK_KHR_ray_query`, targeting **real-time multi-bounce diffuse + specular reflection + emissive voxel soft shadows** movie-level voxel rendering.

---

## 1. Why run RT Core (RTX) on voxels is highly advantageous

1. **Greedy Meshing simplified geometry**:
   - After merging consecutive same-material blocks via Greedy Meshing, a `64×64×64` Chunk typically has **10×–30× fewer triangles** than raw block faces; RT Core BVH traversal is extremely lightweight, easily supporting fully dynamic Chunk TLAS incremental rebuild.
2. **Natural orthogonality without UV seams**:
   - After hitting any triangle, only `floor(hit_pos - hit_normal * eps)` is needed to uniquely determine voxel coordinates and orientation `face_id ∈ 0..5`, **completely no need for Mesh Cards, UV unwrapping, or complex geometric parameterization**.
3. **Emissive voxels directly participate in ray tracing**:
   - Lava, lanterns, neon blocks need not be converted to approximate point lights; directly hit as polygonal emissive surfaces by RT Core, producing real soft shadows and color bleeding.

---

## 2. Acceleration structure and RT Core pipeline selection

### 2.1 Two-level acceleration structure (BLAS / TLAS)
- **Chunk-level BLAS**: each non-empty Chunk (`32^3` or `64^3`) maintains an independent Greedy-Meshed BLAS, enabling `ALLOW_UPDATE` / async compression queue; when player builds/destroys blocks, only rebuild BLAS of 1–4 affected chunks.
- **Dynamic object BLAS**: independent entities (doors, vehicles, dropped items) attached to the same TLAS, uniformly participating in indirect light occlusion and reflection.
- **Opaque and Alpha-Test split**: fully solid blocks take `OPAQUE` path skipping AnyHit; leaves, glass, fences etc. take separate instance mask or `VK_EXT_opacity_micromap` (OMM) acceleration.

### 2.2 `RayTracingPipeline` vs `RayQuery` division
- **Main GI / specular multi-bounce (`VK_KHR_ray_tracing_pipeline`)**:
  - Independent **RayGen → ClosestHit / Miss** pipeline, combined with **SER (Shader Execution Reordering, when available)** to reorder rays by hit material/direction, maximizing RT Core and warp utilization.
- **Direct light soft shadow / AO (`RayQuery` inline)**:
  - Only visibility judgment (`RAY_FLAG_ACCEPT_FIRST_HIT_AND_END_SEARCH | RAY_FLAG_SKIP_CLOSEST_HIT_SHADER`), completed inline in compute shader, avoiding Shader Table scheduling overhead.

---

## 3. Multi-bounce lighting architecture: ReSTIR GI + voxel face cache (Voxel Surface Cache)

Combining **1-bounce precise path sampling** with **voxel face cache multi-bounce reuse**:

1. **Voxel Surface Cache (world-space permanent / semi-permanent multi-bounce base)**:
   - Similar to [sharc.rs](file:///root/prisma/crates/prism-render/src/sharc.rs) spatial hash idea, but key simplified to **`(voxel_xyz, face_id)`** (64-bit exact integer key, no float quantization drift).
   - Each time RT Core ray at secondary hit point (Secondary Hit) looks up the historical outgoing radiance $L_o$ of that voxel face, achieving **3–4+ bounce** diffuse energy transfer stably with **1–2 hardware ray depths**.
2. **Screen-space ReSTIR GI (1 spp main path resampling)**:
   - Emit **1 cosine-weighted / NEE (Next Event Estimation) ray** against full-resolution G-Buffer (or half-resolution + upsample);
   - Via **temporal + spatial reservoir reuse**, amplify effective sample count to equivalent 8–16 spp, precisely capturing near-field high-frequency indirect light from emissive blocks and strong sun bounce.
   - Reservoir reuse gated by **voxel face normal dot product + depth plane distance**, strictly preventing erroneous reuse across block corners and thin walls.
3. **Specular reflection independent channel (Specular RT)**:
   - For smooth metal / water / ice blocks, emit 1 specular ray from G-Buffer by GGX importance sampling; after hit, directly read direct light + Voxel Surface Cache at hit point, achieving clear and physically correct real-time reflection.

---

## 4. Denoising and DLSS-RR / NRD coordination

Following the Guide Buffer contract from [rtx-dlss-integration-design.md](rtx-dlss-integration-design.md):

- **Standard path (NRD / custom temporal denoising)**:
  - Output `DiffuseHitT + SpecularHitT`, screen-space motion vectors, world normal / roughness, execute Diffuse/Specular dual-channel temporal denoising separately (preserve block sharp edges, avoid blurring right angles).
- **NVIDIA RTX flagship path (DLSS-RR ray reconstruction)**:
  - Feed raw 1 spp Diffuse/Specular Radiance directly with Guide Buffers (Albedo, Specular Albedo, Normal+Roughness, Motion Vectors, Depth) into DLSS-RR, simultaneously completing upscaling and ghost-free denoising, **completely replacing traditional separated temporal denoiser**.

---

## 5. Unified interface with software solution (`voxel-software-gi-design.md`)

Both voxel GI solutions share upper-level Chunk data and G-Buffer compositing interface:
- **Desktop RTX device**: run this file's `RT Core Pipeline + Voxel Surface Cache + ReSTIR GI (+ DLSS-RR)`;
- **Mobile / no hardware ray tracing device**: automatically fall back to [voxel-software-gi-design.md](voxel-software-gi-design.md)'s `64-ary tree bitmask DDA + voxel face cache + 3D radiance cascades`.
