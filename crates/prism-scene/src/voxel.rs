//! Generic three-tier sparse voxel grid component (`L1 -> L2 -> L3`).
//!
//! Geometry hierarchy:
//! - Level 1 (`1.0 m` macro cell): Empty (`0`), Copy-On-Write uniform solid (`L1_UNIFORM_REF`),
//!   or pointer `brick_idx + 1` into `bricks`.
//! - Level 2 (`0.25 m` brick, `4x4x4 = 64` cells per L1): Stores per-cell occupancy bitmask,
//!   packed RGBA color, packed visual metadata (`VoxelVisualFlags`), and 64-bit L3 micro mask.
//! - Level 3 (`0.0625 m` micro-voxel, `4x4x4 = 64` bits per L2): Stored as 64-bit occupancy
//!   masks (`l3_mask_lo`, `l3_mask_hi`) inside each L2 brick, inheriting transient visual state
//!   from the parent L2 cell and clearing parent L2 occupancy when all L3 bits become zero.

use prism_reflect::Reflect;
use serde::{Deserialize, Serialize};

/// Spatial side length of one Level-1 cell in meters.
pub const VOXEL_L1_SIZE_M: f32 = 1.0;
/// Spatial side length of one Level-2 cell in meters.
pub const VOXEL_L2_SIZE_M: f32 = 0.25;
/// Spatial side length of one Level-3 micro-voxel in meters.
pub const VOXEL_L3_SIZE_M: f32 = 0.0625;
/// Sub-cells per axis at each subdivision tier (`4x4x4 = 64`).
pub const VOXEL_TIER_SUBDIV: u32 = 4;
/// Total sub-cells inside one brick (`64`).
pub const VOXEL_BRICK_CELLS: usize = 64;
/// Sentinel value in `VoxelChunk::l1_brick_ref` indicating a Copy-On-Write uniform L1 solid.
pub const L1_UNIFORM_REF: u32 = u32::MAX;
/// Default L1 chunk dimensions (`8 x 24 x 8 = 1536` L1 cells, safe under RTTIBin list limits).
pub const DEFAULT_CHUNK_DIMS_L1: [u32; 3] = [8, 24, 8];

/// Visual flags packed into the lower 16 bits of `visual` words for voxel rendering.
pub struct VoxelVisualFlags;

impl VoxelVisualFlags {
    pub const NONE: u32 = 0;
    pub const WET: u32 = 1 << 0;
    pub const BURNING: u32 = 1 << 1;
    pub const FROZEN: u32 = 1 << 2;
    pub const CHARGED: u32 = 1 << 3;
    pub const CORRODED: u32 = 1 << 4;
    pub const INFECTED: u32 = 1 << 5;
    pub const WARNING: u32 = 1 << 6;
    pub const STASIS: u32 = 1 << 7;
    pub const BROKEN: u32 = 1 << 8;
    pub const LIQUID: u32 = 1 << 9;
    pub const TRANSLUCENT: u32 = 1 << 10;
    pub const EMISSIVE: u32 = 1 << 11;
    pub const METALLIC: u32 = 1 << 12;
    pub const HEAT_GLOW: u32 = 1 << 13;
}

/// Pack RGBA channels `[0..255]` into little-endian `u32` (`R` in lowest byte).
#[inline]
pub const fn pack_rgba(r: u8, g: u8, b: u8, a: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16) | ((a as u32) << 24)
}

/// Unpack RGBA channels `[r, g, b, a]` from a packed `u32`.
#[inline]
pub const fn unpack_rgba(rgba: u32) -> [u8; 4] {
    [
        (rgba & 0xFF) as u8,
        ((rgba >> 8) & 0xFF) as u8,
        ((rgba >> 16) & 0xFF) as u8,
        ((rgba >> 24) & 0xFF) as u8,
    ]
}

/// Pack 16-bit visual flags, roughness (`0..255`), and emissive strength (`0..255`) into `u32`.
#[inline]
pub const fn pack_visual(flags16: u32, roughness_u8: u8, emissive_u8: u8) -> u32 {
    (flags16 & 0xFFFF) | ((roughness_u8 as u32) << 16) | ((emissive_u8 as u32) << 24)
}

/// Extract lower 16-bit `VoxelVisualFlags` from a packed visual word.
#[inline]
pub const fn visual_flags(visual: u32) -> u32 {
    visual & 0xFFFF
}

/// Extract roughness byte `[0..255]` from a packed visual word.
#[inline]
pub const fn visual_roughness(visual: u32) -> u8 {
    ((visual >> 16) & 0xFF) as u8
}

/// Extract emissive byte `[0..255]` from a packed visual word.
#[inline]
pub const fn visual_emissive(visual: u32) -> u8 {
    ((visual >> 24) & 0xFF) as u8
}

/// Voxel subdivision granularity selector for queries and editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum VoxelTier {
    /// 1.0 m macro cell.
    L1,
    /// 0.25 m simulation cell (`4x4x4` inside an L1 cell).
    #[default]
    L2,
    /// 0.0625 m micro-voxel (`4x4x4` inside an L2 cell).
    L3,
}

/// Result of a CPU raycast against one or more `VoxelChunk` components.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoxelHit {
    /// Ray distance `t` in world units (meters).
    pub distance: f32,
    /// World-space hit position.
    pub world_pos: [f32; 3],
    /// Outward surface normal (`+/-X`, `+/-Y`, `+/-Z`).
    pub normal: [i32; 3],
    /// World-space integer L1 cell coordinates.
    pub world_l1: [i32; 3],
    /// World-space integer L2 cell coordinates (`1 unit = 0.25 m`).
    pub world_l2: [i32; 3],
    /// World-space integer L3 micro-voxel coordinates (`1 unit = 0.0625 m`).
    pub world_l3: [i32; 3],
    /// Packed RGBA color of the hit cell.
    pub rgba: u32,
    /// Packed visual metadata word of the hit cell.
    pub visual: u32,
    /// Native voxel tier at which the ray terminated (`L1`, `L2`, or `L3`).
    pub hit_tier: VoxelTier,
}

/// `4x4x4` Level-2 brick containing 64 sub-cells (`0.25 m` each) and their `u64` Level-3 masks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
#[reflect(name = "prism_engine::scene::VoxelBrick")]
pub struct VoxelBrick {
    /// Lower 32 occupancy bits (`l2_idx 0..31`) inside this brick.
    pub occ_lo: u32,
    /// Upper 32 occupancy bits (`l2_idx 32..63`) inside this brick.
    pub occ_hi: u32,
    /// Packed RGBA colors (`len == 64`).
    pub l2_rgba: Vec<u32>,
    /// Packed visual words (`len == 64`).
    pub l2_visual: Vec<u32>,
    /// Lower 32 bits of each L2 cell's `4x4x4` L3 micro-voxel mask (`len == 64`).
    pub l3_mask_lo: Vec<u32>,
    /// Upper 32 bits of each L2 cell's `4x4x4` L3 micro-voxel mask (`len == 64`).
    pub l3_mask_hi: Vec<u32>,
}

impl Default for VoxelBrick {
    fn default() -> Self {
        Self::new_empty()
    }
}

impl VoxelBrick {
    /// Allocate an empty L2 brick with zeroed `64`-element storage arrays.
    pub fn new_empty() -> Self {
        Self {
            occ_lo: 0,
            occ_hi: 0,
            l2_rgba: vec![0; VOXEL_BRICK_CELLS],
            l2_visual: vec![0; VOXEL_BRICK_CELLS],
            l3_mask_lo: vec![0; VOXEL_BRICK_CELLS],
            l3_mask_hi: vec![0; VOXEL_BRICK_CELLS],
        }
    }

    /// Allocate a brick filled uniformly with `(rgba, visual)` and full `u64::MAX` L3 masks.
    pub fn new_uniform(rgba: u32, visual: u32) -> Self {
        if (rgba >> 24) == 0 {
            return Self::new_empty();
        }
        Self {
            occ_lo: u32::MAX,
            occ_hi: u32::MAX,
            l2_rgba: vec![rgba; VOXEL_BRICK_CELLS],
            l2_visual: vec![visual; VOXEL_BRICK_CELLS],
            l3_mask_lo: vec![u32::MAX; VOXEL_BRICK_CELLS],
            l3_mask_hi: vec![u32::MAX; VOXEL_BRICK_CELLS],
        }
    }

    /// Ensure backing vectors are exactly 64 elements long after deserialization.
    pub fn normalize_lengths(&mut self) {
        self.l2_rgba.resize(VOXEL_BRICK_CELLS, 0);
        self.l2_visual.resize(VOXEL_BRICK_CELLS, 0);
        self.l3_mask_lo.resize(VOXEL_BRICK_CELLS, 0);
        self.l3_mask_hi.resize(VOXEL_BRICK_CELLS, 0);
    }

    /// Full 64-bit L2 occupancy mask across all `4x4x4` sub-cells.
    #[inline]
    pub fn occupancy_mask(&self) -> u64 {
        (self.occ_lo as u64) | ((self.occ_hi as u64) << 32)
    }

    /// Set full 64-bit L2 occupancy mask.
    #[inline]
    pub fn set_occupancy_mask(&mut self, mask: u64) {
        self.occ_lo = mask as u32;
        self.occ_hi = (mask >> 32) as u32;
    }

    /// True if all 64 L2 cells are empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.occ_lo == 0 && self.occ_hi == 0
    }

    /// Full 64-bit L3 micro-voxel mask for local L2 index `idx in 0..64`.
    #[inline]
    pub fn l3_mask(&self, idx: usize) -> u64 {
        let lo = self.l3_mask_lo.get(idx).copied().unwrap_or(0) as u64;
        let hi = self.l3_mask_hi.get(idx).copied().unwrap_or(0) as u64;
        lo | (hi << 32)
    }

    /// Write full 64-bit L3 micro-voxel mask for `idx in 0..64`, enforcing anti-ghost-wall invariant.
    #[inline]
    pub fn set_l3_mask(&mut self, idx: usize, mask: u64) {
        self.normalize_lengths();
        self.l3_mask_lo[idx] = mask as u32;
        self.l3_mask_hi[idx] = (mask >> 32) as u32;
        let bit = 1u64 << idx;
        if mask == 0 {
            let next_occ = self.occupancy_mask() & !bit;
            self.set_occupancy_mask(next_occ);
            self.l2_rgba[idx] = 0;
            self.l2_visual[idx] = 0;
        } else {
            let next_occ = self.occupancy_mask() | bit;
            self.set_occupancy_mask(next_occ);
        }
    }
}

/// Sparse three-tier voxel chunk ECS component (`L1 = 1m`, `L2 = 0.25m`, `L3 = 0.0625m`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
#[reflect(name = "prism_engine::scene::VoxelChunk")]
pub struct VoxelChunk {
    /// World-space integer L1 coordinates `[x, y, z]` of this chunk's minimum corner.
    pub origin_l1: [i32; 3],
    /// Dimensions `[dx, dy, dz]` measured in L1 (`1 m`) cells.
    pub dims_l1: [u32; 3],
    /// Monotonically incremented whenever voxel occupancy or appearance changes.
    pub version: u64,
    /// Per-L1 cell storage reference: `0` = empty, `u32::MAX` = uniform COW, `idx+1` = `bricks[idx]`.
    pub l1_brick_ref: Vec<u32>,
    /// Packed RGBA color for uniform COW L1 cells.
    pub l1_uniform_rgba: Vec<u32>,
    /// Packed visual metadata for uniform COW L1 cells.
    pub l1_uniform_visual: Vec<u32>,
    /// Allocated L2 bricks referenced by `l1_brick_ref`.
    pub bricks: Vec<VoxelBrick>,
    /// Free indices inside `bricks` available for reuse when L1 cells collapse to empty.
    pub free_bricks: Vec<u32>,
}

impl Default for VoxelChunk {
    fn default() -> Self {
        Self::new([0, 0, 0], DEFAULT_CHUNK_DIMS_L1)
    }
}

impl VoxelChunk {
    /// Create an empty chunk anchored at `origin_l1` with dimensions `dims_l1`.
    pub fn new(origin_l1: [i32; 3], dims_l1: [u32; 3]) -> Self {
        let count = (dims_l1[0] as usize)
            .saturating_mul(dims_l1[1] as usize)
            .saturating_mul(dims_l1[2] as usize);
        Self {
            origin_l1,
            dims_l1,
            version: 1,
            l1_brick_ref: vec![0; count],
            l1_uniform_rgba: vec![0; count],
            l1_uniform_visual: vec![0; count],
            bricks: Vec::new(),
            free_bricks: Vec::new(),
        }
    }

    /// World-space minimum corner of this chunk in meters.
    #[inline]
    pub fn world_min(&self) -> [f32; 3] {
        [
            self.origin_l1[0] as f32 * VOXEL_L1_SIZE_M,
            self.origin_l1[1] as f32 * VOXEL_L1_SIZE_M,
            self.origin_l1[2] as f32 * VOXEL_L1_SIZE_M,
        ]
    }

    /// World-space maximum corner of this chunk in meters.
    #[inline]
    pub fn world_max(&self) -> [f32; 3] {
        [
            (self.origin_l1[0] + self.dims_l1[0] as i32) as f32 * VOXEL_L1_SIZE_M,
            (self.origin_l1[1] + self.dims_l1[1] as i32) as f32 * VOXEL_L1_SIZE_M,
            (self.origin_l1[2] + self.dims_l1[2] as i32) as f32 * VOXEL_L1_SIZE_M,
        ]
    }

    /// Total number of L1 cells in this chunk.
    #[inline]
    pub fn l1_cell_count(&self) -> usize {
        self.dims_l1[0] as usize * self.dims_l1[1] as usize * self.dims_l1[2] as usize
    }

    /// Compute linear L1 cell index from local L1 coordinates `[0..dx, 0..dy, 0..dz]`.
    #[inline]
    pub fn l1_index(&self, lx: u32, ly: u32, lz: u32) -> Option<usize> {
        if lx < self.dims_l1[0] && ly < self.dims_l1[1] && lz < self.dims_l1[2] {
            Some((lx + self.dims_l1[0] * (ly + self.dims_l1[1] * lz)) as usize)
        } else {
            None
        }
    }

    /// Local L2 index `0..64` from chunk-local L2 coordinates.
    #[inline]
    pub const fn l2_sub_index(l2x: u32, l2y: u32, l2z: u32) -> usize {
        ((l2x & 3) + 4 * ((l2y & 3) + 4 * (l2z & 3))) as usize
    }

    /// Local L3 bit index `0..64` from chunk-local L3 coordinates.
    #[inline]
    pub const fn l3_sub_index(l3x: u32, l3y: u32, l3z: u32) -> usize {
        ((l3x & 3) + 4 * ((l3y & 3) + 4 * (l3z & 3))) as usize
    }

    /// Mark chunk data modified so CPU/GPU caches re-upload on the next frame.
    #[inline]
    pub fn mark_dirty(&mut self) {
        self.version = self.version.wrapping_add(1).max(1);
    }

    fn alloc_brick(&mut self, brick: VoxelBrick) -> u32 {
        if let Some(slot) = self.free_bricks.pop() {
            let idx = slot as usize;
            if idx < self.bricks.len() {
                self.bricks[idx] = brick;
                return slot + 1;
            }
        }
        self.bricks.push(brick);
        self.bricks.len() as u32
    }

    fn free_l1_brick_if_allocated(&mut self, l1_idx: usize) {
        let cur = self.l1_brick_ref[l1_idx];
        if cur != 0 && cur != L1_UNIFORM_REF {
            let brick_idx = (cur - 1) as usize;
            if brick_idx < self.bricks.len() {
                self.bricks[brick_idx] = VoxelBrick::new_empty();
                self.free_bricks.push(brick_idx as u32);
            }
        }
        self.l1_brick_ref[l1_idx] = 0;
        self.l1_uniform_rgba[l1_idx] = 0;
        self.l1_uniform_visual[l1_idx] = 0;
    }

    /// Fill an L1 (`1 m`) cell uniformly via Copy-On-Write without allocating an L2 brick.
    pub fn set_l1_uniform(&mut self, l1x: u32, l1y: u32, l1z: u32, rgba: u32, visual: u32) -> bool {
        let Some(idx) = self.l1_index(l1x, l1y, l1z) else {
            return false;
        };
        self.free_l1_brick_if_allocated(idx);
        if (rgba >> 24) == 0 {
            self.mark_dirty();
            return true;
        }
        self.l1_brick_ref[idx] = L1_UNIFORM_REF;
        self.l1_uniform_rgba[idx] = rgba;
        self.l1_uniform_visual[idx] = visual;
        self.mark_dirty();
        true
    }

    /// Perform Copy-On-Write L1->L2 split if needed, returning the mutable index in `self.bricks`.
    pub fn split_l1_to_l2(&mut self, l1x: u32, l1y: u32, l1z: u32) -> Option<usize> {
        let l1_idx = self.l1_index(l1x, l1y, l1z)?;
        let cur = self.l1_brick_ref[l1_idx];
        if cur == 0 {
            let new_ref = self.alloc_brick(VoxelBrick::new_empty());
            self.l1_brick_ref[l1_idx] = new_ref;
            return Some((new_ref - 1) as usize);
        }
        if cur == L1_UNIFORM_REF {
            let rgba = self.l1_uniform_rgba[l1_idx];
            let visual = self.l1_uniform_visual[l1_idx];
            let new_ref = self.alloc_brick(VoxelBrick::new_uniform(rgba, visual));
            self.l1_brick_ref[l1_idx] = new_ref;
            self.l1_uniform_rgba[l1_idx] = 0;
            self.l1_uniform_visual[l1_idx] = 0;
            return Some((new_ref - 1) as usize);
        }
        Some((cur - 1) as usize)
    }

    /// Collapse a brick back to uniform COW (when all 64 L2 cells are identical and full) or empty.
    pub fn try_collapse_brick(&mut self, l1x: u32, l1y: u32, l1z: u32) {
        let Some(l1_idx) = self.l1_index(l1x, l1y, l1z) else {
            return;
        };
        let cur = self.l1_brick_ref[l1_idx];
        if cur == 0 || cur == L1_UNIFORM_REF {
            return;
        }
        let brick_idx = (cur - 1) as usize;
        if brick_idx >= self.bricks.len() {
            self.l1_brick_ref[l1_idx] = 0;
            return;
        }
        let brick = &self.bricks[brick_idx];
        if brick.is_empty() {
            self.free_l1_brick_if_allocated(l1_idx);
            return;
        }
        if brick.occ_lo == u32::MAX && brick.occ_hi == u32::MAX {
            let rgba0 = brick.l2_rgba[0];
            let vis0 = brick.l2_visual[0];
            let all_same = brick.l2_rgba.iter().all(|&c| c == rgba0)
                && brick.l2_visual.iter().all(|&v| v == vis0)
                && brick.l3_mask_lo.iter().all(|&m| m == u32::MAX)
                && brick.l3_mask_hi.iter().all(|&m| m == u32::MAX);
            if all_same {
                self.free_l1_brick_if_allocated(l1_idx);
                self.l1_brick_ref[l1_idx] = L1_UNIFORM_REF;
                self.l1_uniform_rgba[l1_idx] = rgba0;
                self.l1_uniform_visual[l1_idx] = vis0;
            }
        }
    }

    /// Set one L2 (`0.25 m`) cell's color, visual metadata, and L3 micro-voxel mask.
    ///
    /// Clearing `l3_mask == 0` or `alpha == 0` clears L2 occupancy and frees empty L1 bricks.
    pub fn set_l2_cell(
        &mut self,
        l2x: u32,
        l2y: u32,
        l2z: u32,
        rgba: u32,
        visual: u32,
        l3_mask: u64,
    ) -> bool {
        let (l1x, l1y, l1z) = (l2x >> 2, l2y >> 2, l2z >> 2);
        let Some(l1_idx) = self.l1_index(l1x, l1y, l1z) else {
            return false;
        };
        let clearing = l3_mask == 0 || (rgba >> 24) == 0;
        let cur_ref = self.l1_brick_ref[l1_idx];
        if clearing && cur_ref == 0 {
            return true;
        }
        if !clearing
            && cur_ref == L1_UNIFORM_REF
            && l3_mask == u64::MAX
            && self.l1_uniform_rgba[l1_idx] == rgba
            && self.l1_uniform_visual[l1_idx] == visual
        {
            return true;
        }

        let Some(brick_idx) = self.split_l1_to_l2(l1x, l1y, l1z) else {
            return false;
        };
        let sub = Self::l2_sub_index(l2x, l2y, l2z);
        let brick = &mut self.bricks[brick_idx];
        brick.normalize_lengths();
        if clearing {
            brick.set_l3_mask(sub, 0);
        } else {
            brick.l2_rgba[sub] = rgba;
            brick.l2_visual[sub] = visual;
            brick.set_l3_mask(sub, l3_mask);
        }
        self.try_collapse_brick(l1x, l1y, l1z);
        self.mark_dirty();
        true
    }

    /// Update only `(rgba, visual)` of an already-occupied L2 cell (fast transient visual update).
    pub fn update_l2_appearance(
        &mut self,
        l2x: u32,
        l2y: u32,
        l2z: u32,
        rgba: u32,
        visual: u32,
    ) -> bool {
        let (l1x, l1y, l1z) = (l2x >> 2, l2y >> 2, l2z >> 2);
        let Some(l1_idx) = self.l1_index(l1x, l1y, l1z) else {
            return false;
        };
        let cur_ref = self.l1_brick_ref[l1_idx];
        if cur_ref == 0 {
            return false;
        }
        if (rgba >> 24) == 0 {
            return self.set_l2_cell(l2x, l2y, l2z, 0, 0, 0);
        }
        if cur_ref == L1_UNIFORM_REF
            && self.l1_uniform_rgba[l1_idx] == rgba
            && self.l1_uniform_visual[l1_idx] == visual
        {
            return true;
        }
        let Some(brick_idx) = self.split_l1_to_l2(l1x, l1y, l1z) else {
            return false;
        };
        let sub = Self::l2_sub_index(l2x, l2y, l2z);
        let brick = &mut self.bricks[brick_idx];
        if (brick.occupancy_mask() >> sub) & 1 == 0 {
            return false;
        }
        brick.l2_rgba[sub] = rgba;
        brick.l2_visual[sub] = visual;
        self.try_collapse_brick(l1x, l1y, l1z);
        self.mark_dirty();
        true
    }

    /// Set or clear a single L3 (`0.0625 m`) micro-voxel in chunk-local L3 coordinates.
    ///
    /// Returns `Some(parent_l2_became_empty)` when coordinates are inside bounds, enforcing
    /// Anti-Ghost-Wall Writeback whenever the parent L2 cell's `l3_mask` becomes zero.
    pub fn set_l3_voxel(
        &mut self,
        l3x: u32,
        l3y: u32,
        l3z: u32,
        solid: bool,
        fallback_rgba: u32,
        fallback_visual: u32,
    ) -> Option<bool> {
        let (l2x, l2y, l2z) = (l3x >> 2, l3y >> 2, l3z >> 2);
        let (l1x, l1y, l1z) = (l2x >> 2, l2y >> 2, l2z >> 2);
        let l1_idx = self.l1_index(l1x, l1y, l1z)?;
        let cur_ref = self.l1_brick_ref[l1_idx];
        if !solid && cur_ref == 0 {
            return Some(true);
        }
        if solid && cur_ref == L1_UNIFORM_REF {
            return Some(false);
        }
        let brick_idx = self.split_l1_to_l2(l1x, l1y, l1z)?;
        let l2_sub = Self::l2_sub_index(l2x, l2y, l2z);
        let l3_bit = 1u64 << Self::l3_sub_index(l3x, l3y, l3z);

        let brick = &mut self.bricks[brick_idx];
        brick.normalize_lengths();
        let old_mask = brick.l3_mask(l2_sub);
        let new_mask = if solid {
            old_mask | l3_bit
        } else {
            old_mask & !l3_bit
        };
        if solid && (brick.occupancy_mask() >> l2_sub) & 1 == 0 {
            if (fallback_rgba >> 24) == 0 {
                return Some(true);
            }
            brick.l2_rgba[l2_sub] = fallback_rgba;
            brick.l2_visual[l2_sub] = fallback_visual;
        }
        brick.set_l3_mask(l2_sub, new_mask);
        let parent_empty = new_mask == 0;
        self.try_collapse_brick(l1x, l1y, l1z);
        self.mark_dirty();
        Some(parent_empty)
    }

    /// Read `(rgba, visual, l3_mask)` of a chunk-local L2 cell (`None` if out of bounds or empty).
    pub fn get_l2_cell(&self, l2x: u32, l2y: u32, l2z: u32) -> Option<(u32, u32, u64)> {
        let (l1x, l1y, l1z) = (l2x >> 2, l2y >> 2, l2z >> 2);
        let l1_idx = self.l1_index(l1x, l1y, l1z)?;
        let r = self.l1_brick_ref[l1_idx];
        if r == 0 {
            return None;
        }
        if r == L1_UNIFORM_REF {
            return Some((
                self.l1_uniform_rgba[l1_idx],
                self.l1_uniform_visual[l1_idx],
                u64::MAX,
            ));
        }
        let brick = self.bricks.get((r - 1) as usize)?;
        let sub = Self::l2_sub_index(l2x, l2y, l2z);
        if (brick.occupancy_mask() >> sub) & 1 == 0 {
            return None;
        }
        let mask = brick.l3_mask(sub);
        if mask == 0 {
            return None;
        }
        Some((brick.l2_rgba[sub], brick.l2_visual[sub], mask))
    }

    /// Test whether a chunk-local L3 micro-voxel is solid (`include_liquid == false` ignores liquids).
    pub fn is_l3_solid(&self, l3x: u32, l3y: u32, l3z: u32, include_liquid: bool) -> bool {
        let Some((rgba, visual, mask)) = self.get_l2_cell(l3x >> 2, l3y >> 2, l3z >> 2) else {
            return false;
        };
        if (rgba >> 24) == 0 {
            return false;
        }
        if !include_liquid && (visual & VoxelVisualFlags::LIQUID) != 0 {
            return false;
        }
        let bit = 1u64 << Self::l3_sub_index(l3x, l3y, l3z);
        (mask & bit) != 0
    }

    /// Test whether a world-space AABB `[min, max]` overlaps any solid L3 voxel in this chunk.
    pub fn aabb_intersects_solid(
        &self,
        min: [f32; 3],
        max: [f32; 3],
        include_liquid: bool,
    ) -> bool {
        let eps = 1e-4;
        let wmin = self.world_min();
        let wmax = self.world_max();
        if max[0] - eps <= wmin[0]
            || min[0] + eps >= wmax[0]
            || max[1] - eps <= wmin[1]
            || min[1] + eps >= wmax[1]
            || max[2] - eps <= wmin[2]
            || min[2] + eps >= wmax[2]
        {
            return false;
        }

        let max_l3 = [
            (self.dims_l1[0] * 16) as i32 - 1,
            (self.dims_l1[1] * 16) as i32 - 1,
            (self.dims_l1[2] * 16) as i32 - 1,
        ];
        let mut lo = [0i32; 3];
        let mut hi = [0i32; 3];
        for axis in 0..3 {
            let l0 = ((min[axis] + eps - wmin[axis]) / VOXEL_L3_SIZE_M).floor() as i32;
            let l1 = ((max[axis] - eps - wmin[axis]) / VOXEL_L3_SIZE_M).floor() as i32;
            lo[axis] = l0.clamp(0, max_l3[axis]);
            hi[axis] = l1.clamp(0, max_l3[axis]);
            if hi[axis] < lo[axis] {
                return false;
            }
        }

        for l1z in (lo[2] >> 4)..=(hi[2] >> 4) {
            for l1y in (lo[1] >> 4)..=(hi[1] >> 4) {
                for l1x in (lo[0] >> 4)..=(hi[0] >> 4) {
                    let Some(l1_idx) = self.l1_index(l1x as u32, l1y as u32, l1z as u32) else {
                        continue;
                    };
                    let r = self.l1_brick_ref[l1_idx];
                    if r == 0 {
                        continue;
                    }
                    if r == L1_UNIFORM_REF {
                        let vis = self.l1_uniform_visual[l1_idx];
                        if include_liquid || (vis & VoxelVisualFlags::LIQUID) == 0 {
                            return true;
                        }
                        continue;
                    }
                    let Some(brick) = self.bricks.get((r - 1) as usize) else {
                        continue;
                    };
                    if brick.is_empty() {
                        continue;
                    }
                    let l2x0 = (lo[0] >> 2).max(l1x * 4);
                    let l2x1 = (hi[0] >> 2).min(l1x * 4 + 3);
                    let l2y0 = (lo[1] >> 2).max(l1y * 4);
                    let l2y1 = (hi[1] >> 2).min(l1y * 4 + 3);
                    let l2z0 = (lo[2] >> 2).max(l1z * 4);
                    let l2z1 = (hi[2] >> 2).min(l1z * 4 + 3);
                    for l2z in l2z0..=l2z1 {
                        for l2y in l2y0..=l2y1 {
                            for l2x in l2x0..=l2x1 {
                                let sub = Self::l2_sub_index(l2x as u32, l2y as u32, l2z as u32);
                                if (brick.occupancy_mask() >> sub) & 1 == 0 {
                                    continue;
                                }
                                let vis = brick.l2_visual[sub];
                                if !include_liquid && (vis & VoxelVisualFlags::LIQUID) != 0 {
                                    continue;
                                }
                                let mask = brick.l3_mask(sub);
                                if mask == u64::MAX {
                                    return true;
                                }
                                if mask == 0 {
                                    continue;
                                }
                                let l3x0 = lo[0].max(l2x * 4);
                                let l3x1 = hi[0].min(l2x * 4 + 3);
                                let l3y0 = lo[1].max(l2y * 4);
                                let l3y1 = hi[1].min(l2y * 4 + 3);
                                let l3z0 = lo[2].max(l2z * 4);
                                let l3z1 = hi[2].min(l2z * 4 + 3);
                                for l3z in l3z0..=l3z1 {
                                    for l3y in l3y0..=l3y1 {
                                        for l3x in l3x0..=l3x1 {
                                            let bit = 1u64
                                                << Self::l3_sub_index(
                                                    l3x as u32, l3y as u32, l3z as u32,
                                                );
                                            if (mask & bit) != 0 {
                                                return true;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        false
    }

    /// Trace a ray `[origin + t * dir]` against this chunk using 3-tier L1 -> L2 -> L3 DDA.
    pub fn raycast(
        &self,
        origin: [f32; 3],
        dir: [f32; 3],
        max_dist: f32,
        hit_liquids: bool,
    ) -> Option<VoxelHit> {
        let len_sq = dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2];
        if len_sq < 1e-12 || max_dist <= 0.0 {
            return None;
        }
        let inv_len = len_sq.sqrt().recip();
        let d = [dir[0] * inv_len, dir[1] * inv_len, dir[2] * inv_len];

        let bmin = self.world_min();
        let bmax = self.world_max();
        let (mut t_min, mut t_max, mut enter_normal) =
            intersect_aabb(origin, d, bmin, bmax, 0.0, max_dist)?;
        if t_min < 0.0 {
            t_min = 0.0;
        }
        t_max = t_max.min(max_dist);
        if t_min >= t_max {
            return None;
        }

        // Convert to chunk-local L1 space (`1 unit = 1 m`).
        let rel_origin = [
            origin[0] - bmin[0],
            origin[1] - bmin[1],
            origin[2] - bmin[2],
        ];
        let eps = 1e-4;
        let start_p = [
            rel_origin[0] + d[0] * (t_min + eps),
            rel_origin[1] + d[1] * (t_min + eps),
            rel_origin[2] + d[2] * (t_min + eps),
        ];

        let mut cell = [
            (start_p[0].floor() as i32).clamp(0, self.dims_l1[0] as i32 - 1),
            (start_p[1].floor() as i32).clamp(0, self.dims_l1[1] as i32 - 1),
            (start_p[2].floor() as i32).clamp(0, self.dims_l1[2] as i32 - 1),
        ];
        let step = [
            if d[0] >= 0.0 { 1 } else { -1 },
            if d[1] >= 0.0 { 1 } else { -1 },
            if d[2] >= 0.0 { 1 } else { -1 },
        ];
        let t_delta = [
            if d[0].abs() > 1e-8 {
                1.0 / d[0].abs()
            } else {
                1e30
            },
            if d[1].abs() > 1e-8 {
                1.0 / d[1].abs()
            } else {
                1e30
            },
            if d[2].abs() > 1e-8 {
                1.0 / d[2].abs()
            } else {
                1e30
            },
        ];
        let mut t_next = [
            next_boundary_t(rel_origin[0], d[0], cell[0]),
            next_boundary_t(rel_origin[1], d[1], cell[1]),
            next_boundary_t(rel_origin[2], d[2], cell[2]),
        ];
        let mut t_cur = t_min;
        let mut normal = enter_normal;

        for _ in 0..(self.dims_l1[0] + self.dims_l1[1] + self.dims_l1[2] + 8) {
            if cell[0] < 0
                || cell[1] < 0
                || cell[2] < 0
                || cell[0] >= self.dims_l1[0] as i32
                || cell[1] >= self.dims_l1[1] as i32
                || cell[2] >= self.dims_l1[2] as i32
            {
                break;
            }
            let t_cell_exit = t_next[0].min(t_next[1]).min(t_next[2]).min(t_max);
            let l1_idx = self
                .l1_index(cell[0] as u32, cell[1] as u32, cell[2] as u32)
                .unwrap();
            let l1_ref = self.l1_brick_ref[l1_idx];
            if l1_ref == L1_UNIFORM_REF {
                let rgba = self.l1_uniform_rgba[l1_idx];
                let vis = self.l1_uniform_visual[l1_idx];
                if (rgba >> 24) != 0 && (hit_liquids || (vis & VoxelVisualFlags::LIQUID) == 0) {
                    return Some(
                        self.build_hit(origin, d, t_cur, normal, cell, None, None, rgba, vis),
                    );
                }
            } else if l1_ref != 0 {
                if let Some(brick) = self.bricks.get((l1_ref - 1) as usize) {
                    if !brick.is_empty() {
                        if let Some(hit) = self.raycast_brick(
                            origin,
                            rel_origin,
                            d,
                            cell,
                            brick,
                            t_cur,
                            t_cell_exit,
                            normal,
                            hit_liquids,
                        ) {
                            return Some(hit);
                        }
                    }
                }
            }

            if t_cell_exit >= t_max {
                break;
            }
            if t_next[0] <= t_next[1] && t_next[0] <= t_next[2] {
                t_cur = t_next[0];
                t_next[0] += t_delta[0];
                cell[0] += step[0];
                normal = [-step[0], 0, 0];
            } else if t_next[1] <= t_next[2] {
                t_cur = t_next[1];
                t_next[1] += t_delta[1];
                cell[1] += step[1];
                normal = [0, -step[1], 0];
            } else {
                t_cur = t_next[2];
                t_next[2] += t_delta[2];
                cell[2] += step[2];
                normal = [0, 0, -step[2]];
            }
            if t_cur >= t_max {
                break;
            }
            enter_normal = normal;
            let _ = enter_normal;
        }
        None
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "arity mirrors the float-parameter packing that the callers already flatten"
    )]
    fn raycast_brick(
        &self,
        origin: [f32; 3],
        rel_origin: [f32; 3],
        d: [f32; 3],
        l1_cell: [i32; 3],
        brick: &VoxelBrick,
        t_enter: f32,
        t_exit: f32,
        mut normal: [i32; 3],
        hit_liquids: bool,
    ) -> Option<VoxelHit> {
        let scale = 4.0;
        let eps = 1e-4;
        let base = [
            (rel_origin[0] - l1_cell[0] as f32) * scale,
            (rel_origin[1] - l1_cell[1] as f32) * scale,
            (rel_origin[2] - l1_cell[2] as f32) * scale,
        ];
        let start_p = [
            base[0] + d[0] * scale * (t_enter + eps),
            base[1] + d[1] * scale * (t_enter + eps),
            base[2] + d[2] * scale * (t_enter + eps),
        ];
        let mut c = [
            (start_p[0].floor() as i32).clamp(0, 3),
            (start_p[1].floor() as i32).clamp(0, 3),
            (start_p[2].floor() as i32).clamp(0, 3),
        ];
        let step = [
            if d[0] >= 0.0 { 1 } else { -1 },
            if d[1] >= 0.0 { 1 } else { -1 },
            if d[2] >= 0.0 { 1 } else { -1 },
        ];
        let t_delta = [
            if d[0].abs() > 1e-8 {
                0.25 / d[0].abs()
            } else {
                1e30
            },
            if d[1].abs() > 1e-8 {
                0.25 / d[1].abs()
            } else {
                1e30
            },
            if d[2].abs() > 1e-8 {
                0.25 / d[2].abs()
            } else {
                1e30
            },
        ];
        let mut t_next = [
            next_boundary_t(base[0], d[0] * scale, c[0]),
            next_boundary_t(base[1], d[1] * scale, c[1]),
            next_boundary_t(base[2], d[2] * scale, c[2]),
        ];
        let mut t_cur = t_enter;
        let occ = brick.occupancy_mask();

        for _ in 0..16 {
            if c[0] < 0 || c[1] < 0 || c[2] < 0 || c[0] >= 4 || c[1] >= 4 || c[2] >= 4 {
                break;
            }
            let t_cell_exit = t_next[0].min(t_next[1]).min(t_next[2]).min(t_exit);
            let sub = Self::l2_sub_index(c[0] as u32, c[1] as u32, c[2] as u32);
            if (occ >> sub) & 1 != 0 {
                let rgba = brick.l2_rgba[sub];
                let vis = brick.l2_visual[sub];
                if (rgba >> 24) != 0 && (hit_liquids || (vis & VoxelVisualFlags::LIQUID) == 0) {
                    let mask = brick.l3_mask(sub);
                    let l2_local = [
                        l1_cell[0] * 4 + c[0],
                        l1_cell[1] * 4 + c[1],
                        l1_cell[2] * 4 + c[2],
                    ];
                    if mask == u64::MAX {
                        return Some(self.build_hit(
                            origin,
                            d,
                            t_cur,
                            normal,
                            l1_cell,
                            Some(l2_local),
                            None,
                            rgba,
                            vis,
                        ));
                    } else if mask != 0 {
                        if let Some(hit) = self.raycast_l3_mask(
                            origin,
                            rel_origin,
                            d,
                            l1_cell,
                            l2_local,
                            mask,
                            rgba,
                            vis,
                            t_cur,
                            t_cell_exit,
                            normal,
                        ) {
                            return Some(hit);
                        }
                    }
                }
            }
            if t_cell_exit >= t_exit {
                break;
            }
            if t_next[0] <= t_next[1] && t_next[0] <= t_next[2] {
                t_cur = t_next[0];
                t_next[0] += t_delta[0];
                c[0] += step[0];
                normal = [-step[0], 0, 0];
            } else if t_next[1] <= t_next[2] {
                t_cur = t_next[1];
                t_next[1] += t_delta[1];
                c[1] += step[1];
                normal = [0, -step[1], 0];
            } else {
                t_cur = t_next[2];
                t_next[2] += t_delta[2];
                c[2] += step[2];
                normal = [0, 0, -step[2]];
            }
            if t_cur >= t_exit {
                break;
            }
        }
        None
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "arity mirrors the float-parameter packing that the callers already flatten"
    )]
    fn raycast_l3_mask(
        &self,
        origin: [f32; 3],
        rel_origin: [f32; 3],
        d: [f32; 3],
        l1_cell: [i32; 3],
        l2_local: [i32; 3],
        mask: u64,
        rgba: u32,
        vis: u32,
        t_enter: f32,
        t_exit: f32,
        mut normal: [i32; 3],
    ) -> Option<VoxelHit> {
        let scale = 16.0;
        let eps = 1e-4;
        let base = [
            rel_origin[0] * scale - (l2_local[0] * 4) as f32,
            rel_origin[1] * scale - (l2_local[1] * 4) as f32,
            rel_origin[2] * scale - (l2_local[2] * 4) as f32,
        ];
        let start_p = [
            base[0] + d[0] * scale * (t_enter + eps),
            base[1] + d[1] * scale * (t_enter + eps),
            base[2] + d[2] * scale * (t_enter + eps),
        ];
        let mut c = [
            (start_p[0].floor() as i32).clamp(0, 3),
            (start_p[1].floor() as i32).clamp(0, 3),
            (start_p[2].floor() as i32).clamp(0, 3),
        ];
        let step = [
            if d[0] >= 0.0 { 1 } else { -1 },
            if d[1] >= 0.0 { 1 } else { -1 },
            if d[2] >= 0.0 { 1 } else { -1 },
        ];
        let t_delta = [
            if d[0].abs() > 1e-8 {
                0.0625 / d[0].abs()
            } else {
                1e30
            },
            if d[1].abs() > 1e-8 {
                0.0625 / d[1].abs()
            } else {
                1e30
            },
            if d[2].abs() > 1e-8 {
                0.0625 / d[2].abs()
            } else {
                1e30
            },
        ];
        let mut t_next = [
            next_boundary_t(base[0], d[0] * scale, c[0]),
            next_boundary_t(base[1], d[1] * scale, c[1]),
            next_boundary_t(base[2], d[2] * scale, c[2]),
        ];
        let mut t_cur = t_enter;

        for _ in 0..16 {
            if c[0] < 0 || c[1] < 0 || c[2] < 0 || c[0] >= 4 || c[1] >= 4 || c[2] >= 4 {
                break;
            }
            let bit = 1u64 << Self::l3_sub_index(c[0] as u32, c[1] as u32, c[2] as u32);
            if (mask & bit) != 0 {
                let l3_local = [
                    l2_local[0] * 4 + c[0],
                    l2_local[1] * 4 + c[1],
                    l2_local[2] * 4 + c[2],
                ];
                return Some(self.build_hit(
                    origin,
                    d,
                    t_cur,
                    normal,
                    l1_cell,
                    Some(l2_local),
                    Some(l3_local),
                    rgba,
                    vis,
                ));
            }
            let t_cell_exit = t_next[0].min(t_next[1]).min(t_next[2]).min(t_exit);
            if t_cell_exit >= t_exit {
                break;
            }
            if t_next[0] <= t_next[1] && t_next[0] <= t_next[2] {
                t_cur = t_next[0];
                t_next[0] += t_delta[0];
                c[0] += step[0];
                normal = [-step[0], 0, 0];
            } else if t_next[1] <= t_next[2] {
                t_cur = t_next[1];
                t_next[1] += t_delta[1];
                c[1] += step[1];
                normal = [0, -step[1], 0];
            } else {
                t_cur = t_next[2];
                t_next[2] += t_delta[2];
                c[2] += step[2];
                normal = [0, 0, -step[2]];
            }
            if t_cur >= t_exit {
                break;
            }
        }
        None
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "arity mirrors the float-parameter packing that the callers already flatten"
    )]
    fn build_hit(
        &self,
        origin: [f32; 3],
        d: [f32; 3],
        t: f32,
        normal: [i32; 3],
        l1_local: [i32; 3],
        l2_local: Option<[i32; 3]>,
        l3_local: Option<[i32; 3]>,
        rgba: u32,
        visual: u32,
    ) -> VoxelHit {
        let world_pos = [
            origin[0] + d[0] * t,
            origin[1] + d[1] * t,
            origin[2] + d[2] * t,
        ];
        // Compute exact inward sub-cell coordinates near surface entry when hitting uniform tier.
        let bmin = self.world_min();
        let nudge = [
            world_pos[0] - normal[0] as f32 * (VOXEL_L3_SIZE_M * 0.25),
            world_pos[1] - normal[1] as f32 * (VOXEL_L3_SIZE_M * 0.25),
            world_pos[2] - normal[2] as f32 * (VOXEL_L3_SIZE_M * 0.25),
        ];
        let world_l1 = [
            self.origin_l1[0] + l1_local[0],
            self.origin_l1[1] + l1_local[1],
            self.origin_l1[2] + l1_local[2],
        ];
        let world_l2 = if let Some(l2) = l2_local {
            [
                self.origin_l1[0] * 4 + l2[0],
                self.origin_l1[1] * 4 + l2[1],
                self.origin_l1[2] * 4 + l2[2],
            ]
        } else {
            [
                self.origin_l1[0] * 4
                    + ((nudge[0] - bmin[0]) / VOXEL_L2_SIZE_M)
                        .floor()
                        .clamp((l1_local[0] * 4) as f32, (l1_local[0] * 4 + 3) as f32)
                        as i32,
                self.origin_l1[1] * 4
                    + ((nudge[1] - bmin[1]) / VOXEL_L2_SIZE_M)
                        .floor()
                        .clamp((l1_local[1] * 4) as f32, (l1_local[1] * 4 + 3) as f32)
                        as i32,
                self.origin_l1[2] * 4
                    + ((nudge[2] - bmin[2]) / VOXEL_L2_SIZE_M)
                        .floor()
                        .clamp((l1_local[2] * 4) as f32, (l1_local[2] * 4 + 3) as f32)
                        as i32,
            ]
        };
        let world_l3 = if let Some(l3) = l3_local {
            [
                self.origin_l1[0] * 16 + l3[0],
                self.origin_l1[1] * 16 + l3[1],
                self.origin_l1[2] * 16 + l3[2],
            ]
        } else {
            [
                self.origin_l1[0] * 16
                    + ((nudge[0] - bmin[0]) / VOXEL_L3_SIZE_M).floor().clamp(
                        (world_l2[0] - self.origin_l1[0] * 4) as f32 * 4.0,
                        (world_l2[0] - self.origin_l1[0] * 4) as f32 * 4.0 + 3.0,
                    ) as i32,
                self.origin_l1[1] * 16
                    + ((nudge[1] - bmin[1]) / VOXEL_L3_SIZE_M).floor().clamp(
                        (world_l2[1] - self.origin_l1[1] * 4) as f32 * 4.0,
                        (world_l2[1] - self.origin_l1[1] * 4) as f32 * 4.0 + 3.0,
                    ) as i32,
                self.origin_l1[2] * 16
                    + ((nudge[2] - bmin[2]) / VOXEL_L3_SIZE_M).floor().clamp(
                        (world_l2[2] - self.origin_l1[2] * 4) as f32 * 4.0,
                        (world_l2[2] - self.origin_l1[2] * 4) as f32 * 4.0 + 3.0,
                    ) as i32,
            ]
        };
        let hit_tier = if l3_local.is_some() {
            VoxelTier::L3
        } else if l2_local.is_some() {
            VoxelTier::L2
        } else {
            VoxelTier::L1
        };
        VoxelHit {
            distance: t,
            world_pos,
            normal,
            world_l1,
            world_l2,
            world_l3,
            rgba,
            visual,
            hit_tier,
        }
    }
}

/// Raycast across a slice of `VoxelChunk` references and return the nearest hit `(chunk_idx, hit)`.
pub fn raycast_chunks(
    chunks: &[&VoxelChunk],
    origin: [f32; 3],
    dir: [f32; 3],
    max_dist: f32,
    hit_liquids: bool,
) -> Option<(usize, VoxelHit)> {
    let mut best: Option<(usize, VoxelHit)> = None;
    let mut best_t = max_dist;
    for (idx, chunk) in chunks.iter().enumerate() {
        if let Some(hit) = chunk.raycast(origin, dir, best_t, hit_liquids) {
            if best.is_none() || hit.distance < best_t {
                best_t = hit.distance;
                best = Some((idx, hit));
            }
        }
    }
    best
}

/// Broad-phase AABB test against the chunks registered in `index`.
///
/// `resolve` maps an index key back to the caller's chunk. Chunks the resolver
/// does not know (already streamed out) are skipped, so the index may briefly
/// outlive the storage it describes.
///
/// `scratch` is reused across calls; keeping one per caller removes the
/// per-query allocation that a fresh candidate list would cost.
pub fn aabb_intersects_solid_indexed<'a, F>(
    index: &VoxelChunkIndex,
    min: [f32; 3],
    max: [f32; 3],
    include_liquid: bool,
    scratch: &mut Vec<u64>,
    mut resolve: F,
) -> bool
where
    F: FnMut(u64) -> Option<&'a VoxelChunk>,
{
    index.aabb_candidates(min, max, scratch);
    for &key in scratch.iter() {
        if let Some(chunk) = resolve(key) {
            if chunk.aabb_intersects_solid(min, max, include_liquid) {
                return true;
            }
        }
    }
    false
}

/// Broad-phase raycast against the chunks registered in `index`.
///
/// Candidates are tested nearest-first, so the first chunk that reports a hit
/// already bounds the search: the loop stops as soon as the next candidate
/// starts beyond the best distance found so far.
pub fn raycast_chunks_indexed<'a, F>(
    index: &VoxelChunkIndex,
    origin: [f32; 3],
    dir: [f32; 3],
    max_dist: f32,
    hit_liquids: bool,
    scratch: &mut Vec<(f32, u64)>,
    mut resolve: F,
) -> Option<VoxelHit>
where
    F: FnMut(u64) -> Option<&'a VoxelChunk>,
{
    index.ray_candidates(origin, dir, max_dist, scratch);
    let mut best_t = max_dist;
    let mut best: Option<VoxelHit> = None;
    for &(enter, key) in scratch.iter() {
        if enter > best_t {
            break;
        }
        let Some(chunk) = resolve(key) else {
            continue;
        };
        if let Some(hit) = chunk.raycast(origin, dir, best_t, hit_liquids) {
            if best.is_none() || hit.distance < best_t {
                best_t = hit.distance;
                best = Some(hit);
            }
        }
    }
    best
}

#[inline]
fn next_boundary_t(rel: f32, d: f32, cell: i32) -> f32 {
    if d > 1e-8 {
        ((cell + 1) as f32 - rel) / d
    } else if d < -1e-8 {
        (cell as f32 - rel) / d
    } else {
        1e30
    }
}

fn intersect_aabb(
    origin: [f32; 3],
    d: [f32; 3],
    bmin: [f32; 3],
    bmax: [f32; 3],
    t_lo: f32,
    t_hi: f32,
) -> Option<(f32, f32, [i32; 3])> {
    let mut tmin = t_lo;
    let mut tmax = t_hi;
    let mut enter_normal = [0i32; 3];
    for axis in 0..3 {
        if d[axis].abs() < 1e-8 {
            if origin[axis] < bmin[axis] || origin[axis] > bmax[axis] {
                return None;
            }
        } else {
            let inv = 1.0 / d[axis];
            let mut t0 = (bmin[axis] - origin[axis]) * inv;
            let mut t1 = (bmax[axis] - origin[axis]) * inv;
            let mut n = [0i32; 3];
            n[axis] = if d[axis] > 0.0 { -1 } else { 1 };
            if t0 > t1 {
                std::mem::swap(&mut t0, &mut t1);
            }
            if t0 > tmin {
                tmin = t0;
                enter_normal = n;
            }
            if t1 < tmax {
                tmax = t1;
            }
            if tmin > tmax {
                return None;
            }
        }
    }
    Some((tmin, tmax, enter_normal))
}

/// Optional 3D wireframe box rendered in-world by the voxel pass to show aimed target bounds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(default)]
#[reflect(name = "prism_engine::scene::VoxelCursorHighlight")]
pub struct VoxelCursorHighlight {
    pub active: bool,
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub color: [f32; 4],
}

impl Default for VoxelCursorHighlight {
    fn default() -> Self {
        Self {
            active: false,
            min: [0.0; 3],
            max: [0.0; 3],
            color: [1.0, 0.92, 0.35, 1.0],
        }
    }
}

/// Broad-phase index over the world-space bounds of a set of [`VoxelChunk`]s.
///
/// A streaming world holds one chunk per `16 m x 64 m x 16 m` block. Answering a
/// collision or aim query by walking every chunk makes the cost scale with the
/// streaming radius, and each rejected chunk still pays for its own `L1 -> L2 ->
/// L3` walk inside [`VoxelChunk::aabb_intersects_solid`] or [`VoxelChunk::raycast`].
/// The index stores only the world-space boxes, sorted along `X`, so an AABB
/// sweep stops at the first box starting past the query and a ray query hands
/// back candidates ordered by entry distance.
///
/// The index owns no voxel data: callers keep their own chunk storage and pass
/// the same `key` back to resolve a candidate. Keys are opaque to the index.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct VoxelChunkIndex {
    /// `(key, world_min, world_max)` sorted by `world_min[0]`.
    entries: Vec<(u64, [f32; 3], [f32; 3])>,
}

impl VoxelChunkIndex {
    /// Creates an empty index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Drops every entry while keeping the allocation.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Number of indexed chunks.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the index currently holds no chunks.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Indexes `chunk` under `key`, replacing any box already stored for it.
    ///
    /// Streaming touches this once per chunk that enters or leaves the radius,
    /// so the linear key scan is not on the per-frame path.
    pub fn insert(&mut self, key: u64, chunk: &VoxelChunk) {
        let entry = (key, chunk.world_min(), chunk.world_max());
        if let Some(slot) = self.entries.iter_mut().find(|probe| probe.0 == key) {
            *slot = entry;
        } else {
            let pos = self
                .entries
                .partition_point(|probe| probe.1[0] <= entry.1[0]);
            self.entries.insert(pos, entry);
        }
    }

    /// Removes `key` and reports whether it was present.
    pub fn remove(&mut self, key: u64) -> bool {
        match self.entries.iter().position(|probe| probe.0 == key) {
            Some(idx) => {
                self.entries.remove(idx);
                true
            }
            None => false,
        }
    }

    /// Appends the keys of every chunk whose world box overlaps `[min, max]`.
    ///
    /// `out` is cleared first, so callers can reuse one scratch vector across
    /// frames instead of allocating per query.
    pub fn aabb_candidates(&self, min: [f32; 3], max: [f32; 3], out: &mut Vec<u64>) {
        out.clear();
        for (key, bmin, bmax) in &self.entries {
            if bmin[0] > max[0] {
                break;
            }
            if bmax[0] < min[0]
                || bmin[1] > max[1]
                || bmax[1] < min[1]
                || bmin[2] > max[2]
                || bmax[2] < min[2]
            {
                continue;
            }
            out.push(*key);
        }
    }

    /// Appends `(entry_distance, key)` for every chunk box the ray enters,
    /// sorted by entry distance.
    ///
    /// `dir` is normalized internally; a zero-length direction yields no
    /// candidates. `out` is cleared first, so callers can reuse one scratch
    /// vector across frames.
    pub fn ray_candidates(
        &self,
        origin: [f32; 3],
        dir: [f32; 3],
        max_dist: f32,
        out: &mut Vec<(f32, u64)>,
    ) {
        out.clear();
        let len_sq = dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2];
        if len_sq < 1e-12 || max_dist <= 0.0 {
            return;
        }
        let inv_len = len_sq.sqrt().recip();
        let d = [dir[0] * inv_len, dir[1] * inv_len, dir[2] * inv_len];
        for (key, bmin, bmax) in &self.entries {
            if let Some((t_enter, _, _)) = intersect_aabb(origin, d, *bmin, *bmax, 0.0, max_dist) {
                out.push((t_enter.max(0.0), *key));
            }
        }
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cow_l1_split_and_anti_ghost_wall_writeback() {
        let mut chunk = VoxelChunk::new([0, 0, 0], [4, 4, 4]);
        let stone = pack_rgba(128, 128, 128, 255);
        let vis = pack_visual(VoxelVisualFlags::NONE, 200, 0);
        assert!(chunk.set_l1_uniform(1, 2, 1, stone, vis));
        let l1_idx = chunk.l1_index(1, 2, 1).unwrap();
        assert_eq!(chunk.l1_brick_ref[l1_idx], L1_UNIFORM_REF);
        assert!(chunk.bricks.is_empty());

        // Carve every single L3 micro-voxel inside L2 (lx=4, ly=8, lz=4).
        let (l2x, l2y, l2z) = (4u32, 8u32, 4u32);
        for mz in 0..4 {
            for my in 0..4 {
                for mx in 0..4 {
                    let parent_empty = chunk
                        .set_l3_voxel(l2x * 4 + mx, l2y * 4 + my, l2z * 4 + mz, false, 0, 0)
                        .expect("in bounds");
                    let is_last = mx == 3 && my == 3 && mz == 3;
                    assert_eq!(parent_empty, is_last);
                }
            }
        }
        // Iron Law 1: clearing all L3 bits clears parent L2 occupancy automatically.
        assert!(chunk.get_l2_cell(l2x, l2y, l2z).is_none());
        // Adjacent L2 cell inside the split L1 brick remains intact.
        assert!(chunk.get_l2_cell(l2x + 1, l2y, l2z).is_some());
        assert_ne!(chunk.l1_brick_ref[l1_idx], L1_UNIFORM_REF);
        assert_ne!(chunk.l1_brick_ref[l1_idx], 0);
    }

    #[test]
    fn raycast_finds_l3_carved_cavity_and_back_wall() {
        let mut chunk = VoxelChunk::new([0, 0, 0], [4, 4, 4]);
        let alloy = pack_rgba(90, 120, 140, 255);
        let vis = pack_visual(VoxelVisualFlags::METALLIC | VoxelVisualFlags::WET, 80, 0);
        chunk.set_l2_cell(4, 4, 4, alloy, vis, u64::MAX);

        // Carve a 2x2 L3 pinhole through the center of this 0.25m L2 cell along Z.
        for mz in 0..4 {
            for my in 1..3 {
                for mx in 1..3 {
                    chunk.set_l3_voxel(16 + mx, 16 + my, 16 + mz, false, 0, 0);
                }
            }
        }

        // Ray straight through the carved L3 hole misses the front face!
        let miss = chunk.raycast([1.125, 1.125, 0.0], [0.0, 0.0, 1.0], 5.0, true);
        assert!(miss.is_none(), "ray should pass cleanly through L3 hole");

        // Offset ray hits solid L3 rim and inherits L2 transient WET|METALLIC flags (Iron Law 2).
        let hit = chunk
            .raycast([1.03, 1.03, 0.0], [0.0, 0.0, 1.0], 5.0, true)
            .expect("should hit solid L3 border");
        assert_eq!(hit.normal, [0, 0, -1]);
        assert_eq!(
            visual_flags(hit.visual) & (VoxelVisualFlags::WET | VoxelVisualFlags::METALLIC),
            VoxelVisualFlags::WET | VoxelVisualFlags::METALLIC
        );
    }

    #[test]
    fn aabb_collision_respects_l3_micro_geometry_and_liquids() {
        let mut chunk = VoxelChunk::new([0, 0, 0], [4, 4, 4]);
        let water = pack_rgba(40, 110, 190, 140);
        let water_vis = pack_visual(
            VoxelVisualFlags::LIQUID | VoxelVisualFlags::TRANSLUCENT,
            20,
            0,
        );
        chunk.set_l2_cell(0, 0, 0, water, water_vis, u64::MAX);
        // Water should not block solid-only player collision.
        assert!(!chunk.aabb_intersects_solid([0.05, 0.05, 0.05], [0.20, 0.20, 0.20], false));
        assert!(chunk.aabb_intersects_solid([0.05, 0.05, 0.05], [0.20, 0.20, 0.20], true));

        // Add one L3 bar at mx=0..4, my=0, mz=0 -> [0.0..0.25, 0.0..0.0625, 0.0..0.0625]
        let wall = pack_rgba(180, 180, 180, 255);
        let wall_vis = pack_visual(VoxelVisualFlags::NONE, 180, 0);
        for mx in 0..4 {
            chunk.set_l3_voxel(4 + mx, 0, 0, true, wall, wall_vis);
        }
        assert!(chunk.aabb_intersects_solid([0.26, 0.01, 0.01], [0.49, 0.05, 0.05], false));
        assert!(!chunk.aabb_intersects_solid([0.26, 0.08, 0.08], [0.49, 0.24, 0.24], false));
    }

    #[test]
    fn chunk_index_returns_only_overlapping_boxes() {
        let mut index = VoxelChunkIndex::new();
        assert!(index.is_empty());
        for i in 0..4u32 {
            let chunk = VoxelChunk::new([i as i32 * 8, 0, 0], [8, 24, 8]);
            index.insert(i as u64, &chunk);
        }
        assert_eq!(index.len(), 4);

        let mut out = Vec::new();
        // Inside the box of key 2 only ([16..24) on X).
        index.aabb_candidates([17.0, 1.0, 1.0], [18.0, 2.0, 2.0], &mut out);
        assert_eq!(out, vec![2]);

        // Straddling the 0/1 boundary returns both neighbours, never the far ones.
        index.aabb_candidates([7.5, 1.0, 1.0], [8.5, 2.0, 2.0], &mut out);
        out.sort_unstable();
        assert_eq!(out, vec![0, 1]);

        // Above the chunks: nothing overlaps on Y.
        index.aabb_candidates([1.0, 40.0, 1.0], [2.0, 41.0, 2.0], &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn chunk_index_ray_candidates_are_ordered_by_entry_distance() {
        let mut index = VoxelChunkIndex::new();
        for i in 0..3u32 {
            let chunk = VoxelChunk::new([i as i32 * 8, 0, 0], [8, 24, 8]);
            index.insert(100 + i as u64, &chunk);
        }

        let mut out = Vec::new();
        // Travelling +X from x = -1 enters key 100 first, then 101, then 102.
        index.ray_candidates([-1.0, 4.0, 4.0], [1.0, 0.0, 0.0], 40.0, &mut out);
        assert_eq!(
            out.iter().map(|&(_, key)| key).collect::<Vec<_>>(),
            vec![100, 101, 102]
        );
        assert!(out.windows(2).all(|w| w[0].0 <= w[1].0));

        // A short ray stops before the second chunk: it would only reach x = 7.5.
        index.ray_candidates([-1.0, 4.0, 4.0], [1.0, 0.0, 0.0], 8.5, &mut out);
        assert_eq!(
            out.iter().map(|&(_, key)| key).collect::<Vec<_>>(),
            vec![100]
        );

        // A degenerate direction yields no candidates.
        index.ray_candidates([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], 40.0, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn indexed_queries_match_the_brute_force_walk() {
        let mut chunks = Vec::new();
        for i in 0..3i32 {
            let mut chunk = VoxelChunk::new([i * 4, 0, 0], [4, 4, 4]);
            let stone = pack_rgba(128, 128, 128, 255);
            let vis = pack_visual(VoxelVisualFlags::NONE, 200, 0);
            // Only the middle chunk holds solid voxels.
            if i == 1 {
                chunk.set_l2_cell(0, 0, 0, stone, vis, u64::MAX);
            }
            chunks.push(chunk);
        }
        let mut index = VoxelChunkIndex::new();
        for (i, chunk) in chunks.iter().enumerate() {
            index.insert(i as u64, chunk);
        }
        let resolve = |key: u64| chunks.get(key as usize);

        let mut aabb_scratch = Vec::new();
        // Inside chunk 1's solid L2 cell at [4..5) on every axis.
        let min = [4.1, 0.1, 0.1];
        let max = [4.2, 0.2, 0.2];
        assert!(aabb_intersects_solid_indexed(
            &index,
            min,
            max,
            false,
            &mut aabb_scratch,
            resolve
        ));
        let brute = chunks
            .iter()
            .any(|chunk| chunk.aabb_intersects_solid(min, max, false));
        assert!(brute);

        // Empty space above the chunks agrees with the brute-force walk too.
        let min = [4.1, 40.0, 0.1];
        let max = [4.2, 41.0, 0.2];
        assert_eq!(
            aabb_intersects_solid_indexed(&index, min, max, false, &mut aabb_scratch, resolve),
            chunks
                .iter()
                .any(|chunk| chunk.aabb_intersects_solid(min, max, false))
        );

        // A ray down -X from beyond the far chunk hits the middle one first.
        let mut ray_scratch = Vec::new();
        let indexed = raycast_chunks_indexed(
            &index,
            [20.0, 0.125, 0.125],
            [-1.0, 0.0, 0.0],
            40.0,
            false,
            &mut ray_scratch,
            resolve,
        );
        let refs: Vec<&VoxelChunk> = chunks.iter().collect();
        let brute = raycast_chunks(&refs, [20.0, 0.125, 0.125], [-1.0, 0.0, 0.0], 40.0, false)
            .map(|(_, hit)| hit);
        assert_eq!(
            indexed.map(|hit| hit.world_l2),
            brute.map(|hit| hit.world_l2)
        );
        assert!(indexed.is_some());
    }

    #[test]
    fn chunk_index_insert_replaces_and_remove_drops() {
        let mut index = VoxelChunkIndex::new();
        index.insert(7, &VoxelChunk::new([0, 0, 0], [8, 24, 8]));
        // Same key, moved box: the entry is replaced, not duplicated.
        index.insert(7, &VoxelChunk::new([64, 0, 0], [8, 24, 8]));
        assert_eq!(index.len(), 1);

        let mut out = Vec::new();
        index.aabb_candidates([1.0, 1.0, 1.0], [2.0, 2.0, 2.0], &mut out);
        assert!(out.is_empty());
        index.aabb_candidates([65.0, 1.0, 1.0], [66.0, 2.0, 2.0], &mut out);
        assert_eq!(out, vec![7]);

        assert!(index.remove(7));
        assert!(!index.remove(7));
        assert!(index.is_empty());
    }
}
