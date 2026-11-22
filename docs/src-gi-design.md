# SRC GI (Surfel Radiance Cascades) mobile dynamic global illumination optional design

> **Tier**: L2 subsystem design · Rendering advanced
> **Status**: Optional design blueprint (not yet implemented; for future UGC / strong dynamic geometry without hardware ray tracing mobile scenarios)
> **Related**: [DESIGN.md](DESIGN.md) §6, [prt-gi-64tree-design.md](prt-gi-64tree-design.md), [mobile-raytracing-gi-design.md](mobile-raytracing-gi-design.md)

This document records the complete technical design of **SRC GI (Surfel Radiance Cascades, GIBS@SIGGRAPH2021 mobile variant)** as an optional GI pipeline blueprint for Prisma to optionally support **UGC player free building / strong dynamic geometry destruction** in the future.

---

## 1. Design goals and applicability boundary

- **Goal**: on mobile devices **without hardware ray tracing** (no `VK_KHR_ray_query` / BVH traversal needed), provide **free manually-placed probes, free offline baking** real-time diffuse global illumination for **fully dynamic / UGC scenes**.
- **Boundary with PRT GI**:
  - **PRT GI (`RasterGiMode::Prt`)**: static geometry primary + dynamic lights; zero runtime intersection, single-frame response, full RGB color fidelity, fixed 4 MB budget.
  - **SRC GI (this blueprint)**: UGC free building / strong dynamic geometry; runtime software intersection + cross-frame convergence (5–28 frames) + camera-following local 64 m coverage.

---

## 2. Core pipeline: software ray tracing based on multi-level hash grid

No longer build and traverse triangle BVH; instead take **Surfel (unified-radius disk surface element)** itself as ray intersection target:

1. **Ray emission and multi-frame amortized stepping**:
   - Rays emitted from world-space probes, max trace distance capped at **64 m**.
   - Default each bounce amortized over **28 frames** to converge; **fast mode** (triggered on first scene entry or light switch) completes convergence in **5 frames**.
2. **Empty cell skipping (32-bit spatial hash)**:
   - When ray steps across grid cells, compute unique **32-bit integer hash key** from discrete 3D coordinate `loc` and current `level`, query prime buffer.
   - Empty buffer means empty cell, directly skip; hit data stops and enters intersection phase.
3. **Simplified disk intersection**:
   - Only perform **ray-disk intersection test** with Surfels carrying `(position, radius, normal)` inside grid cells (cost far lower than ray-triangle intersection); after hit, cache surface irradiance back to probe.
   - Since Surfel radius is uniform, naturally supports distance-based dynamic resolution and LOD adjustment.

---

## 3. World-space probe layout and irradiance compression

- **Automatic follow layout**: world-space probes automatically follow camera movement, no manual probe placement or offline baking needed by artists.
- **Three-level Clipmap**:
  - Total 3 levels, each level **`32 × 32 × 16`** probes.
  - **LOD 0** spacing approx **1 m**, outer layers double with LOD level, overall coverage radius approx **64 m**.
- **Irradiance compressed storage** (referencing *Assassin's Creed: Mirage* scheme):
  - **Luminance component** stored as spherical harmonics (SH), retaining directionality.
  - **Chrominance component** degenerated to directionless scalar.
  - **16-bit per channel**, packed into `uint3` texture, strictly controlling mobile VRAM and bandwidth.

---

## 4. 3D radiance cascades (3D Radiance Cascades) and memory layout

Introduce radiance cascade idea from Alexander Sannikov, dividing into multiple levels by distance:

1. **Near-far resolution trade-off**:
   - Near-field cascade (Cascade 0) keeps high spatial resolution to preserve near-field geometry edges and shadow details.
   - Each lower level: spatial resolution down to **`1/8`**, angular resolution up by **`4×`** (e.g. 64 → 256 directions), total resolution capped at Cascade 0's **`2×`**, significantly reducing ray stepping count.
2. **Direction-first layout**:
   - Place "direction" dimension outermost (all probes within same direction slice stored contiguously), allowing GPU **hardware texture sampler** to directly perform 3D trilinear interpolation, greatly reducing manual interpolation ALU cost.
3. **Far-field cascade pre-averaging**:
   - For level 0 and above far-field cascades, average every 4 directions' lighting results before writing to memory, compressing level 0+ cascade memory and bandwidth to approx **`1/4`**.

---

## 5. Mobile performance optimization and light-leak tolerance

1. **Hit-hash cache**: cache hit distance in first trace; subsequent frames skip repeated stepping when geometry unchanged (local re-computation needed on affected meshes when geometry changes, making trace overhead near zero).
2. **70% base resolution + super-resolution parsing**: GI base calculation resolution lowered to **70%** (saving approx **50%** bandwidth and execution time), then restored to full resolution via **FSR1** or **NSS** (Arm Neural Super Sampling).
3. **Light-leak control**:
   - For mobile single-sided thin-sheet geometry without physical thickness, introduce Surfel **"transparent outer ring"** attenuation mechanism to suppress inter-disk gaps and penetration light leakage.
   - Combined with probe classification based on **indoor volume**, shield erroneous cross-sampling between indoor and outdoor.

---

## 6. Pre-access assessment points for future integration into Prisma

1. **Pipeline position**: integrate as `RasterGiMode::Src` (tentative enum) into `RenderGraph`, mutually exclusive with `PrtPass` / `RestirGiPass`.
2. **Reuse with existing facilities**: spatial hash table and atomic accumulation can reference [sharc.rs](file:///root/prisma/crates/prism-render/src/sharc.rs); hardware trilinear `Direction-First` layout experience can directly feedback to `PrtPass`'s `gi_volume` optimization.
3. **Known trade-offs**: 28-frame steady-state convergence latency, chrominance directionality loss, and boundary requirements of indoor volume classification for pure open UGC scenarios need to be evaluated in combination with target project type before project initiation.
