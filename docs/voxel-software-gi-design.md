# Voxel-specific Software GI Design (64-ary tree bitmask DDA + face cache + 3D radiance cascades)

> **Tier**: L2 subsystem design · Rendering advanced
> **Status**: Optional design blueprint (for mobile / general GPU without hardware ray tracing, supports UGC real-time build/destroy)
> **Related**: [DESIGN.md](DESIGN.md) §6, [prt-gi-64tree-design.md](prt-gi-64tree-design.md), [src-gi-design.md](src-gi-design.md)

This document defines a **fully hardware-ray-tracing-independent, triangle-BVH-independent, and surfel-disk-approximation-independent** voxel-specific software GI solution. Leveraging the voxel's natural structural benefits of "axis-aligned + integer grid points + 6 discrete normals", it achieves **zero disk light leakage, UGC second-level response, low random memory access** real-time diffuse global illumination on mobile.

---

## 1. Core advantages (vs. general mesh SRC GI)

| Pain point (general mesh SRC GI) | Voxel-specific software GI natural solution |
|---|---|
| Surfel disk approximation causes thin-wall gaps, self-occlusion, and "transparent outer ring" patches | **Hierarchical 64-ary tree bitmask exact DDA**, 1-voxel-thick wall 100% tight, zero geometric light leakage |
| Spatial hash conflicts + Surfel dynamic allocation/recycle pool + LOD merging | **Chunk-level static slots + voxel 6-direction face cache**, build/destroy only marks local Chunk dirty |
| Depends on manual/rule-based "indoor volume" classification to shield inner/outer sampling | DDA steps directly in exact voxel occupancy bitmap, physical occlusion is ground truth |

---

## 2. Geometry representation: three-layer 64-ary tree bitmask (`uint64` / `uvec2`)

Following Prisma `uvec2` dual-word bit arithmetic convention, organize the voxel world by `Chunk` as a three-layer `4×4×4` bitmask tree:

- **L0 (Leaf Brick, `4×4×4` voxels)**: a single `uvec2` (64 bits) represents solid occupancy of 64 voxels (1 bit/voxel).
- **L1 (Macro Brick, `16×16×16` voxels)**: a single `uvec2` represents non-empty state of `4×4×4` L0 bricks.
- **L2 (Chunk Root, `64×64×64` voxels)**: a single `uvec2` represents non-empty state of `4×4×4` L1s, combined with Clipmap / Chunk page table covering the world around the camera.

**Hierarchical DDA stepping**:
- When ray enters empty L2/L1, directly skip entire `16^3` or `4^3` block via **Amanatides–Woo integer DDA**.
- Step voxel by voxel only inside L0 bricks where mask bit is 1.
- **Zero float triangle intersection, zero disk intersection**, on hit directly get `(voxel_coord, face_id ∈ 0..5, hit_t)`.

---

## 3. Lighting cache: voxel 6-direction face cache (Voxel Face Cache)

No longer maintain dynamic Surfel list; directly cache direct light and previous frame's indirect light on **exposed voxel faces**:

- **Index key**: an exposed face exists only when the neighboring voxel in direction `face_id` (`±X, ±Y, ±Z`) is air; mapped via Chunk-local compact index or lightweight 3D page table.
- **Storage format**: each face stores `f16x4` (`rgb = E/π * albedo` radiance leaving the face, `a = valid timestamp / dirty flag`), **naturally supports omnidirectional RGB color bleeding** (no need for brightness/chrominance separation compromise).
- **UGC real-time update**: when player adds/removes blocks, only mark the 6-neighborhood exposed faces of affected blocks as dirty, refresh next frame (single frame can reset local hit cache, no full re-bake).

---

## 4. Radiance propagation and compositing: 3D radiance cascades (Direction-First)

After obtaining exact voxel DDA hit and Face Cache, use 3D radiance cascades for noise-free final collection (Final Gather):

1. **Cascade tiering**:
   - Cascade 0 (near-field high spatial resolution, soft contact shadows and corner color bleeding within 1~2 m);
   - Cascade 1..N (spatial resolution down by `1/8` per level, angular resolution up by `4×` per level), amortized over multiple frames (typically 8–16 frames, fast local reset on light/block dramatic change) emitting a small number of layered DDA rays.
2. **Direction-First hardware trilinear**:
   - Same-direction slices stored contiguously in `R16G16B16A16_SFLOAT` 3D texture; fragment shader directly uses hardware `SampleLevel` trilinear interpolation.
3. **Voxel Corner AO overlay**:
   - Ultra-high-frequency 1-voxel-edge contact shadow handled by zero-cost neighbor bitmask Corner AO, complementing cascade indirect light, further reducing probe density needed for Cascade 0.

---

## 5. Mobile adaptation points

- **All-integer/bitwise inner loop**: DDA and 64-bit mask tests avoid branch divergence as much as possible; L0 hit terminates immediately.
- **Bandwidth control**: Face Cache and cascade textures uniformly use `RGBA16F`; distant cascade writes pre-average 4 directions before write.
- **Optional resolution downgrade**: low-end devices support 70% GI resolution parsing + `FSR1` / `TAA` spatial recovery.
