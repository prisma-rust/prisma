//! Three-tier Emergence v3.0 physics world and multi-rate tag scheduler.
//!
//! Coordinates sparse L2 `PhysicalCell` simulation with `prism_scene::VoxelChunk` geometry,
//! enforcing Iron Law 1 (Anti-Ghost-Wall Writeback when L3 masks become empty) and
//! Iron Law 2 (Transient Visual Inheritance from parent L2 cells to L3 micro-voxels).

use std::collections::{HashMap, HashSet, VecDeque};

use prism_game_api::{EntityHandle, GameWorld};
use prism_scene::{
    raycast_chunks, VoxelChunk, VoxelHit, VoxelTier, VOXEL_L1_SIZE_M, VOXEL_L2_SIZE_M,
    VOXEL_L3_SIZE_M,
};

use crate::materials::{mat_id, MaterialTable};
use crate::tags::{
    BioActivity, MaterialFlags, MatterState, PhysicalCell, StimulusFlags, Temperature,
    TopologyFlags, TransientFlags,
};

/// Minecraft-style streaming chunk dimensions (`16m x 64m x 16m` per chunk).
pub const CHUNK_DIMS_L1: [u32; 3] = [16, 64, 16];
/// Active chunk streaming radius around the player (`2` -> `5 x 5 = 25` chunks = `80m x 64m x 80m`).
pub const STREAM_RADIUS_CHUNKS: i32 = 2;

const ORTHO_DIRS: [[i32; 3]; 6] = [
    [1, 0, 0],
    [-1, 0, 0],
    [0, 1, 0],
    [0, -1, 0],
    [0, 0, 1],
    [0, 0, -1],
];

const HORIZ_DIRS: [[i32; 3]; 4] = [[1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1]];

/// Active Emergence sandbox world combining ECS `VoxelChunk` mirrors and v3.0 L2 simulation.
pub struct SimWorld {
    pub seed: u64,
    pub materials: MaterialTable,
    pub chunks: Vec<VoxelChunk>,
    pub chunk_coords: Vec<[i32; 2]>,
    chunk_lookup: HashMap<[i32; 2], usize>,
    pub chunk_entities: Vec<EntityHandle>,
    synced_versions: Vec<u64>,
    chunk_cells: Vec<HashMap<[i32; 3], (u8, PhysicalCell)>>,
    active_queue: VecDeque<[i32; 3]>,
    active_set: HashSet<[i32; 3]>,
    fast_accum: f32,
    bio_accum: f32,
    clock_accum: f32,
    rng_state: u64,
    /// Pending structural shockwaves triggered by volatile explosions.
    pending_blasts: Vec<[i32; 3]>,
}

impl SimWorld {
    /// Construct a `SimWorld` from generated chunks, sparse L2 cells, and active seed list.
    pub fn new(
        materials: MaterialTable,
        chunks: Vec<VoxelChunk>,
        cells: HashMap<[i32; 3], (u8, PhysicalCell)>,
        initial_active: Vec<[i32; 3]>,
    ) -> Self {
        let count = chunks.len();
        let mut chunk_coords = Vec::with_capacity(count);
        let mut chunk_lookup = HashMap::with_capacity(count);
        for (idx, chunk) in chunks.iter().enumerate() {
            let coord = [
                chunk.origin_l1[0].div_euclid(CHUNK_DIMS_L1[0] as i32),
                chunk.origin_l1[2].div_euclid(CHUNK_DIMS_L1[2] as i32),
            ];
            chunk_coords.push(coord);
            chunk_lookup.insert(coord, idx);
        }
        let mut world = Self {
            seed: 2026,
            materials,
            chunks,
            chunk_coords,
            chunk_lookup,
            chunk_entities: Vec::new(),
            synced_versions: vec![0; count],
            chunk_cells: (0..count).map(|_| HashMap::new()).collect(),
            active_queue: VecDeque::new(),
            active_set: HashSet::new(),
            fast_accum: 0.0,
            bio_accum: 0.0,
            clock_accum: 0.0,
            rng_state: 0x9E37_79B9_7F4A_7C15,
            pending_blasts: Vec::new(),
        };
        for (pos, entry) in cells {
            if let Some((c_idx, _)) = world.chunk_coords_for_l2(pos) {
                world.chunk_cells[c_idx].insert(pos, entry);
            }
        }
        for pos in initial_active {
            world.wake_cell_and_neighbors(pos, false);
        }
        world
    }

    /// Spawn ECS chunk entities inside `GameWorld` and push initial chunk geometry.
    pub fn attach_ecs(&mut self, world: &mut GameWorld<'_>) {
        if !self.chunk_entities.is_empty() {
            return;
        }
        for (idx, chunk) in self.chunks.iter().enumerate() {
            let [cx, cz] = self.chunk_coords[idx];
            let entity = world.spawn();
            world.set(entity, prism_scene::Name(format!("voxel_chunk_{cx}_{cz}")));
            world.set(entity, prism_scene::LocalTransform::default());
            world.set(entity, prism_scene::Active(true));
            world.set(entity, chunk.clone());
            self.chunk_entities.push(entity);
            self.synced_versions[idx] = chunk.version;
        }
    }

    /// Stream chunks in/out around `center_world` in real time (Minecraft-style).
    pub fn stream_around(
        &mut self,
        center_world: glam::Vec3,
        mut ecs_world: Option<&mut GameWorld<'_>>,
        max_new_per_tick: usize,
    ) {
        let chunk_span_x = CHUNK_DIMS_L1[0] as f32;
        let chunk_span_z = CHUNK_DIMS_L1[2] as f32;
        let pcx = center_world.x.div_euclid(chunk_span_x) as i32;
        let pcz = center_world.z.div_euclid(chunk_span_z) as i32;

        // 1. Unload distant chunks beyond `STREAM_RADIUS_CHUNKS + 1`.
        let unload_radius = STREAM_RADIUS_CHUNKS + 1;
        let mut idx = 0;
        while idx < self.chunks.len() {
            let [cx, cz] = self.chunk_coords[idx];
            if (cx - pcx).abs() > unload_radius || (cz - pcz).abs() > unload_radius {
                self.unload_chunk_at(idx, ecs_world.as_deref_mut());
            } else {
                idx += 1;
            }
        }

        // 2. Collect missing chunks within `STREAM_RADIUS_CHUNKS`, sorted closest-first.
        let mut missing = Vec::new();
        for dz in -STREAM_RADIUS_CHUNKS..=STREAM_RADIUS_CHUNKS {
            for dx in -STREAM_RADIUS_CHUNKS..=STREAM_RADIUS_CHUNKS {
                let coord = [pcx + dx, pcz + dz];
                if !self.chunk_lookup.contains_key(&coord) {
                    missing.push((dx * dx + dz * dz, coord));
                }
            }
        }
        if missing.is_empty() {
            return;
        }
        missing.sort_unstable_by_key(|(dist2, _)| *dist2);

        let budget = max_new_per_tick.min(missing.len());
        for (_, [cx, cz]) in missing.into_iter().take(budget) {
            let gen = crate::world_gen::generate_chunk(&self.materials, self.seed, cx, cz);
            let new_idx = self.chunks.len();
            self.chunk_lookup.insert(gen.coord, new_idx);
            self.chunk_coords.push(gen.coord);
            self.synced_versions.push(gen.chunk.version);
            if let Some(world) = ecs_world.as_deref_mut() {
                let entity = world.spawn();
                world.set(entity, prism_scene::Name(format!("voxel_chunk_{cx}_{cz}")));
                world.set(entity, prism_scene::LocalTransform::default());
                world.set(entity, prism_scene::Active(true));
                world.set(entity, gen.chunk.clone());
                self.chunk_entities.push(entity);
            }
            self.chunks.push(gen.chunk);
            self.chunk_cells.push(gen.cells);
            for pos in gen.active {
                self.wake_cell_and_neighbors(pos, false);
            }
        }
    }

    fn unload_chunk_at(&mut self, idx: usize, ecs_world: Option<&mut GameWorld<'_>>) {
        let [cx, cz] = self.chunk_coords[idx];
        self.chunk_lookup.remove(&[cx, cz]);
        if !self.active_set.is_empty() {
            let min_l2x = cx * (CHUNK_DIMS_L1[0] as i32) * 4;
            let max_l2x = min_l2x + (CHUNK_DIMS_L1[0] as i32) * 4;
            let min_l2z = cz * (CHUNK_DIMS_L1[2] as i32) * 4;
            let max_l2z = min_l2z + (CHUNK_DIMS_L1[2] as i32) * 4;
            self.active_set.retain(|pos| {
                !(pos[0] >= min_l2x && pos[0] < max_l2x && pos[2] >= min_l2z && pos[2] < max_l2z)
            });
        }

        // O(1) unload drops the chunk's HashMap and frees its bucket allocation immediately.
        self.chunk_cells.swap_remove(idx);
        self.chunks.swap_remove(idx);
        self.chunk_coords.swap_remove(idx);
        self.synced_versions.swap_remove(idx);
        if idx < self.chunk_entities.len() {
            let removed_ent = self.chunk_entities.swap_remove(idx);
            if let Some(world) = ecs_world {
                world.despawn(removed_ent);
            }
        }
        if idx < self.chunk_coords.len() {
            self.chunk_lookup.insert(self.chunk_coords[idx], idx);
        }
    }

    /// Upload only dirty chunks whose version advanced since the last sync.
    /// Push modified `VoxelChunk` components into ECS only when dirty, returning the count synced.
    pub fn sync_dirty_chunks(&mut self, world: &mut GameWorld<'_>) -> usize {
        if self.chunk_entities.len() != self.chunks.len() {
            return 0;
        }
        let mut synced = 0usize;
        for (idx, chunk) in self.chunks.iter().enumerate() {
            if self.synced_versions[idx] != chunk.version {
                world.set(self.chunk_entities[idx], chunk.clone());
                self.synced_versions[idx] = chunk.version;
                synced += 1;
            }
        }
        synced
    }

    /// Number of loaded chunks whose mesh/voxel data changed since the last ECS sync.
    pub fn dirty_chunk_count(&self) -> usize {
        self.chunks
            .iter()
            .zip(self.synced_versions.iter())
            .filter(|(c, &v)| c.version != v)
            .count()
    }

    /// Number of currently awake cells in the active dirty set.
    pub fn active_count(&self) -> usize {
        self.active_set.len()
    }

    /// Total number of sparse tracked L2 simulation cells across loaded chunks.
    pub fn tracked_cell_count(&self) -> usize {
        self.chunk_cells.iter().map(HashMap::len).sum()
    }

    #[inline]
    fn cell_get(&self, l2: [i32; 3]) -> Option<(u8, PhysicalCell)> {
        let (c_idx, _) = self.chunk_coords_for_l2(l2)?;
        self.chunk_cells[c_idx].get(&l2).copied()
    }

    /// Map a world-space L2 integer coordinate to `(chunk_index, local_l2)`.
    #[inline]
    pub fn chunk_coords_for_l2(&self, l2: [i32; 3]) -> Option<(usize, [u32; 3])> {
        let max_y = (CHUNK_DIMS_L1[1] as i32) * 4;
        if l2[1] < 0 || l2[1] >= max_y {
            return None;
        }
        let span_x = (CHUNK_DIMS_L1[0] as i32) * 4;
        let span_z = (CHUNK_DIMS_L1[2] as i32) * 4;
        let cx = l2[0].div_euclid(span_x);
        let cz = l2[2].div_euclid(span_z);
        let &c_idx = self.chunk_lookup.get(&[cx, cz])?;
        let lx = l2[0].rem_euclid(span_x) as u32;
        let ly = l2[1] as u32;
        let lz = l2[2].rem_euclid(span_z) as u32;
        Some((c_idx, [lx, ly, lz]))
    }

    /// Map world L3 integer coordinate to `(chunk_index, local_l3, world_l2)`.
    #[inline]
    pub fn chunk_coords_for_l3(&self, l3: [i32; 3]) -> Option<(usize, [u32; 3], [i32; 3])> {
        let l2 = [
            l3[0].div_euclid(4),
            l3[1].div_euclid(4),
            l3[2].div_euclid(4),
        ];
        let (c_idx, local_l2) = self.chunk_coords_for_l2(l2)?;
        let local_l3 = [
            local_l2[0] * 4 + l3[0].rem_euclid(4) as u32,
            local_l2[1] * 4 + l3[1].rem_euclid(4) as u32,
            local_l2[2] * 4 + l3[2].rem_euclid(4) as u32,
        ];
        Some((c_idx, local_l3, l2))
    }

    fn fallback_material_id(&self, l2: [i32; 3], rgba: u32) -> u8 {
        self.materials
            .find_by_packed_rgba(rgba)
            .unwrap_or(if l2[1] < 8 {
                mat_id::BEDROCK
            } else {
                mat_id::SOIL
            })
    }

    /// Read `(mat_id, PhysicalCell, l3_mask)` at a world L2 coordinate (`None` if empty air).
    pub fn inspect_l2(&self, l2: [i32; 3]) -> Option<(u8, PhysicalCell, u64)> {
        let (c_idx, local_l2) = self.chunk_coords_for_l2(l2)?;
        let (rgba, _, mask) =
            self.chunks[c_idx].get_l2_cell(local_l2[0], local_l2[1], local_l2[2])?;
        if let Some(&(id, cell)) = self.chunk_cells[c_idx].get(&l2) {
            return Some((id, cell, mask));
        }
        let fallback_id = self.fallback_material_id(l2, rgba);
        Some((fallback_id, self.materials.spawn_cell(fallback_id), mask))
    }

    /// Check whether a world-space AABB overlaps any solid L3 voxel (liquids do not block walking).
    pub fn aabb_intersects_solid(&self, min: [f32; 3], max: [f32; 3]) -> bool {
        for chunk in &self.chunks {
            if chunk.aabb_intersects_solid(min, max, false) {
                return true;
            }
        }
        false
    }

    /// Raycast across all voxel chunks from `origin` along `dir`.
    pub fn raycast(
        &self,
        origin: [f32; 3],
        dir: [f32; 3],
        max_dist: f32,
        hit_liquids: bool,
    ) -> Option<VoxelHit> {
        let refs: Vec<&VoxelChunk> = self.chunks.iter().collect();
        raycast_chunks(&refs, origin, dir, max_dist, hit_liquids).map(|(_, hit)| hit)
    }

    /// Schedule `l2` (and optionally its 6 orthogonal neighbors) into the active dirty set.
    pub fn wake_cell_and_neighbors(&mut self, l2: [i32; 3], include_neighbors: bool) {
        if self.chunk_coords_for_l2(l2).is_some() && self.active_set.insert(l2) {
            self.active_queue.push_back(l2);
        }
        if include_neighbors {
            for d in ORTHO_DIRS {
                let n = [l2[0] + d[0], l2[1] + d[1], l2[2] + d[2]];
                if self.chunk_coords_for_l2(n).is_some() && self.active_set.insert(n) {
                    self.active_queue.push_back(n);
                }
            }
        }
    }

    fn ensure_cell_entry(&mut self, l2: [i32; 3]) -> Option<(u8, PhysicalCell, u64)> {
        let (c_idx, local_l2) = self.chunk_coords_for_l2(l2)?;
        let (rgba, _, mask) =
            self.chunks[c_idx].get_l2_cell(local_l2[0], local_l2[1], local_l2[2])?;
        if let Some(&entry) = self.chunk_cells[c_idx].get(&l2) {
            return Some((entry.0, entry.1, mask));
        }
        let fallback_id = self.fallback_material_id(l2, rgba);
        let cell = self.materials.spawn_cell(fallback_id);
        self.chunk_cells[c_idx].insert(l2, (fallback_id, cell));
        Some((fallback_id, cell, mask))
    }

    /// Commit updated `(id, cell)` state to both sparse map and `VoxelChunk` (Iron Law 2).
    fn write_cell_visual(&mut self, l2: [i32; 3], id: u8, mut cell: PhysicalCell) {
        let Some((c_idx, local_l2)) = self.chunk_coords_for_l2(l2) else {
            return;
        };
        // Iron Law 1: shattered glass/depleted cells with zero durability turn into empty void.
        let shattered = cell.durability == 0 && cell.transient.contains(TransientFlags::BROKEN);
        let (rgba, visual) = if shattered || id == mat_id::AIR {
            (0, 0)
        } else {
            self.materials.cell_visuals(id, &cell)
        };
        if (rgba >> 24) == 0 {
            self.clear_l2_cell(l2);
            return;
        }
        if !cell.transient.contains(TransientFlags::DORMANT) {
            self.wake_cell_and_neighbors(l2, false);
        }
        self.chunk_cells[c_idx].insert(l2, (id, cell));
        let _ = self.chunks[c_idx].update_l2_appearance(
            local_l2[0],
            local_l2[1],
            local_l2[2],
            rgba,
            visual,
        );
        cell.pending_stimuli.clear();
    }

    /// Remove an L2 cell cleanly, enforcing Iron Law 1 (`AIRTIGHT` & occupancy removed).
    pub fn clear_l2_cell(&mut self, l2: [i32; 3]) -> bool {
        let Some((c_idx, local_l2)) = self.chunk_coords_for_l2(l2) else {
            return false;
        };
        let had = self.chunk_cells[c_idx].remove(&l2).is_some()
            || self.chunks[c_idx]
                .get_l2_cell(local_l2[0], local_l2[1], local_l2[2])
                .is_some();
        if had {
            self.chunks[c_idx].set_l2_cell(local_l2[0], local_l2[1], local_l2[2], 0, 0, 0);
            self.wake_cell_and_neighbors(l2, true);
        }
        had
    }

    /// Carve/break voxels at the player's selected granularity (`L1`, `L2`, or `L3`).
    pub fn carve_at_hit(&mut self, tier: VoxelTier, hit: &VoxelHit) -> bool {
        match tier {
            VoxelTier::L1 => {
                let base = [
                    hit.world_l1[0] * 4,
                    hit.world_l1[1] * 4,
                    hit.world_l1[2] * 4,
                ];
                let mut any = false;
                for dz in 0..4 {
                    for dy in 0..4 {
                        for dx in 0..4 {
                            let l2 = [base[0] + dx, base[1] + dy, base[2] + dz];
                            any |= self.clear_l2_cell(l2);
                        }
                    }
                }
                any
            }
            VoxelTier::L2 => {
                // Brittle / Glass cells shatter neighbors when kinetically struck.
                if let Some((id, cell, _)) = self.inspect_l2(hit.world_l2) {
                    if cell.material.contains(MaterialFlags::GLASS) || id == mat_id::GLASS {
                        for d in ORTHO_DIRS {
                            let n = [
                                hit.world_l2[0] + d[0],
                                hit.world_l2[1] + d[1],
                                hit.world_l2[2] + d[2],
                            ];
                            if let Some((nid, mut ncell, _)) = self.ensure_cell_entry(n) {
                                if nid == mat_id::GLASS
                                    || ncell.transient.contains(TransientFlags::BRITTLE)
                                {
                                    ncell.apply_stimulus(StimulusFlags::KINETIC);
                                    ncell.durability = 0;
                                    self.write_cell_visual(n, nid, ncell);
                                }
                            }
                        }
                    }
                }
                self.clear_l2_cell(hit.world_l2)
            }
            VoxelTier::L3 => {
                let Some((c_idx, local_l3, l2)) = self.chunk_coords_for_l3(hit.world_l3) else {
                    return false;
                };
                let _ = self.ensure_cell_entry(l2);
                let Some(parent_empty) = self.chunks[c_idx].set_l3_voxel(
                    local_l3[0],
                    local_l3[1],
                    local_l3[2],
                    false,
                    0,
                    0,
                ) else {
                    return false;
                };
                // Iron Law 1: clearing the final L3 bit inside an L2 cell clears its L2 physics cell.
                if parent_empty {
                    self.chunk_cells[c_idx].remove(&l2);
                }
                self.wake_cell_and_neighbors(l2, true);
                true
            }
        }
    }

    /// Place `mat` cleanly onto the outward face of `hit` at `tier` granularity.
    pub fn place_adjacent(&mut self, tier: VoxelTier, hit: &VoxelHit, mat: u8) -> bool {
        if mat == mat_id::AIR {
            return false;
        }
        let cell = self.materials.spawn_cell(mat);
        self.place_adjacent_cell(tier, hit, mat, cell)
    }

    /// Place an initialized `PhysicalCell` (e.g. a Catalog entity/variant) onto the outward face of `hit`.
    pub fn place_adjacent_cell(
        &mut self,
        tier: VoxelTier,
        hit: &VoxelHit,
        mat: u8,
        cell: PhysicalCell,
    ) -> bool {
        if mat == mat_id::AIR {
            return false;
        }
        let (rgba, visual) = self.materials.cell_visuals(mat, &cell);
        if (rgba >> 24) == 0 {
            return false;
        }
        let wake_all = !cell.transient.contains(TransientFlags::DORMANT);
        match tier {
            VoxelTier::L1 => {
                let target_l1 = [
                    hit.world_l1[0] + hit.normal[0],
                    hit.world_l1[1] + hit.normal[1],
                    hit.world_l1[2] + hit.normal[2],
                ];
                let base_l2 = [target_l1[0] * 4, target_l1[1] * 4, target_l1[2] * 4];
                if self.chunk_coords_for_l2(base_l2).is_none() {
                    return false;
                }
                for dz in 0..4 {
                    for dy in 0..4 {
                        for dx in 0..4 {
                            let l2 = [base_l2[0] + dx, base_l2[1] + dy, base_l2[2] + dz];
                            if let Some((c_idx, local_l2)) = self.chunk_coords_for_l2(l2) {
                                self.chunk_cells[c_idx].insert(l2, (mat, cell));
                                self.chunks[c_idx].set_l2_cell(
                                    local_l2[0],
                                    local_l2[1],
                                    local_l2[2],
                                    rgba,
                                    visual,
                                    u64::MAX,
                                );
                                if wake_all {
                                    self.wake_cell_and_neighbors(l2, false);
                                }
                            }
                        }
                    }
                }
                self.wake_cell_and_neighbors(base_l2, true);
                true
            }
            VoxelTier::L2 => {
                let target_l2 = [
                    hit.world_l2[0] + hit.normal[0],
                    hit.world_l2[1] + hit.normal[1],
                    hit.world_l2[2] + hit.normal[2],
                ];
                let Some((c_idx, local_l2)) = self.chunk_coords_for_l2(target_l2) else {
                    return false;
                };
                self.chunk_cells[c_idx].insert(target_l2, (mat, cell));
                self.chunks[c_idx].set_l2_cell(
                    local_l2[0],
                    local_l2[1],
                    local_l2[2],
                    rgba,
                    visual,
                    u64::MAX,
                );
                self.wake_cell_and_neighbors(target_l2, true);
                true
            }
            VoxelTier::L3 => {
                let target_l3 = [
                    hit.world_l3[0] + hit.normal[0],
                    hit.world_l3[1] + hit.normal[1],
                    hit.world_l3[2] + hit.normal[2],
                ];
                let Some((c_idx, local_l3, l2)) = self.chunk_coords_for_l3(target_l3) else {
                    return false;
                };
                let (use_mat, use_cell) = *self.chunk_cells[c_idx].entry(l2).or_insert((mat, cell));
                let (u_rgba, u_vis) = self.materials.cell_visuals(use_mat, &use_cell);
                self.chunks[c_idx].set_l3_voxel(
                    local_l3[0],
                    local_l3[1],
                    local_l3[2],
                    true,
                    u_rgba,
                    u_vis,
                );
                self.wake_cell_and_neighbors(l2, true);
                true
            }
        }
    }

    /// Set a single L2 voxel at `l2` to `mat` (or clear it when `mat == mat_id::AIR`).
    pub fn set_l2_material(&mut self, l2: [i32; 3], mat: u8) -> bool {
        let Some((c_idx, local_l2)) = self.chunk_coords_for_l2(l2) else {
            return false;
        };
        if mat == mat_id::AIR {
            self.chunk_cells[c_idx].remove(&l2);
            self.chunks[c_idx].set_l2_cell(local_l2[0], local_l2[1], local_l2[2], 0, 0, u64::MAX);
            self.wake_cell_and_neighbors(l2, true);
            return true;
        }
        let cell = self.materials.spawn_cell(mat);
        let (rgba, visual) = self.materials.cell_visuals(mat, &cell);
        self.chunk_cells[c_idx].insert(l2, (mat, cell));
        self.chunks[c_idx].set_l2_cell(
            local_l2[0],
            local_l2[1],
            local_l2[2],
            rgba,
            visual,
            u64::MAX,
        );
        self.wake_cell_and_neighbors(l2, true);
        true
    }

    /// Inject a physical stimulus (or wetting splash) around `center_l2`.
    pub fn inject_stimulus(
        &mut self,
        center_l2: [i32; 3],
        stimulus: StimulusFlags,
        splash_wet: bool,
        radius_l2: i32,
    ) {
        let r_sq = radius_l2 * radius_l2;
        for dz in -radius_l2..=radius_l2 {
            for dy in -radius_l2..=radius_l2 {
                for dx in -radius_l2..=radius_l2 {
                    if dx * dx + dy * dy + dz * dz > r_sq {
                        continue;
                    }
                    let pos = [center_l2[0] + dx, center_l2[1] + dy, center_l2[2] + dz];
                    let Some((id, mut cell, _)) = self.ensure_cell_entry(pos) else {
                        continue;
                    };
                    if splash_wet {
                        cell.transient.insert(TransientFlags::WET);
                        cell.transient.remove(TransientFlags::BURNING);
                        cell.env.set_temperature(Temperature::Ambient);
                    }
                    cell.apply_stimulus(stimulus);
                    let _ = cell.step_reaction(false);
                    cell.transient.remove(TransientFlags::DORMANT);
                    self.write_cell_visual(pos, id, cell);
                    self.wake_cell_and_neighbors(pos, true);
                }
            }
        }
    }

    /// Compute the 3D wireframe box bounds (`min`, `max`) for cursor visualization at `tier`.
    pub fn cursor_bounds_for_hit(tier: VoxelTier, hit: &VoxelHit) -> ([f32; 3], [f32; 3]) {
        let (coords, size) = match tier {
            VoxelTier::L1 => (hit.world_l1, VOXEL_L1_SIZE_M),
            VoxelTier::L2 => (hit.world_l2, VOXEL_L2_SIZE_M),
            VoxelTier::L3 => (hit.world_l3, VOXEL_L3_SIZE_M),
        };
        let min = [
            coords[0] as f32 * size,
            coords[1] as f32 * size,
            coords[2] as f32 * size,
        ];
        let max = [min[0] + size, min[1] + size, min[2] + size];
        (min, max)
    }

    /// Advance multi-rate emergent physics (`10 Hz` fast reactions/flow, `5 Hz` bio, `1 Hz` clocks).
    pub fn step(&mut self, dt: f32) {
        let clamped_dt = dt.clamp(0.0, 0.05);
        self.fast_accum += clamped_dt;
        self.bio_accum += clamped_dt;
        self.clock_accum += clamped_dt;

        while self.fast_accum >= 0.1 {
            self.fast_accum -= 0.1;
            self.step_fast_10hz();
        }
        while self.bio_accum >= 0.2 {
            self.bio_accum -= 0.2;
            self.step_bio_5hz();
        }
        while self.clock_accum >= 1.0 {
            self.clock_accum -= 1.0;
            self.step_clocks_1hz();
        }
    }

    fn next_rand(&mut self) -> u64 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng_state = x;
        x
    }

    fn step_fast_10hz(&mut self) {
        let batch_cap = self.active_queue.len().min(512);
        let mut batch = Vec::with_capacity(batch_cap);
        for _ in 0..batch_cap {
            if let Some(pos) = self.active_queue.pop_front() {
                self.active_set.remove(&pos);
                batch.push(pos);
            }
        }

        let mut wake_list: Vec<[i32; 3]> = Vec::new();
        for pos in batch {
            let Some((id, mut cell)) = self.cell_get(pos) else {
                continue;
            };
            if cell.transient.contains(TransientFlags::STASIS) {
                continue;
            }
            let _ = cell.step_reaction(false);
            let mut keep_awake = cell.transient.intersects(
                TransientFlags::BURNING
                    .union(TransientFlags::CHARGED)
                    .union(TransientFlags::WARNING),
            ) || cell.clocks.half_life() > 0;

            // Power generators continually recharge adjacent conductive paths.
            if id == mat_id::ISOTOPE {
                cell.transient.insert(TransientFlags::CHARGED);
                cell.env.set_temperature(Temperature::Hot);
                keep_awake = true;
            }

            // Thermal combustion / ignition / structural burnout.
            if cell.transient.contains(TransientFlags::BURNING) {
                keep_awake = true;
                if cell.transient.contains(TransientFlags::WET) {
                    cell.transient.remove(TransientFlags::BURNING);
                } else {
                    if cell.material.contains(MaterialFlags::OXIDIZER) && cell.durability <= 60 {
                        self.pending_blasts.push(pos);
                    }
                    cell.durability = cell.durability.saturating_sub(14);
                    if cell.durability == 0 {
                        self.clear_l2_cell(pos);
                        continue;
                    }
                    if (self.next_rand() & 3) == 0 {
                        let d = ORTHO_DIRS[(self.next_rand() as usize) % ORTHO_DIRS.len()];
                        let n = [pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]];
                        if let Some((nid, mut nc, _)) = self.ensure_cell_entry(n) {
                            if !nc.material.contains(MaterialFlags::INSULATED) {
                                nc.apply_stimulus(StimulusFlags::HEAT);
                                let _ = nc.step_reaction(false);
                                nc.transient.remove(TransientFlags::DORMANT);
                                wake_list.push(n);
                                self.write_cell_visual(n, nid, nc);
                            }
                        }
                    }
                }
            }

            // Conductive electrical pulse propagation through wires, alloy, and WET paths.
            if cell.transient.contains(TransientFlags::CHARGED)
                && !cell.transient.contains(TransientFlags::BROKEN)
            {
                keep_awake = true;
                for d in ORTHO_DIRS {
                    let n = [pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]];
                    if let Some((nid, mut nc)) = self.cell_get(n) {
                        let conductive = nc.material.contains(MaterialFlags::CONDUCTIVE)
                            || nc.transient.contains(TransientFlags::WET);
                        if conductive
                            && !nc.transient.contains(TransientFlags::CHARGED)
                            && !nc.transient.contains(TransientFlags::BROKEN)
                        {
                            nc.apply_stimulus(StimulusFlags::SHOCK);
                            let _ = nc.step_reaction(false);
                            self.write_cell_visual(n, nid, nc);
                            wake_list.push(n);
                        }
                    }
                }
                if id != mat_id::ISOTOPE {
                    // Transient discharge decays slowly unless driven by an active power source.
                    if (self.next_rand() & 7) == 0 {
                        cell.transient.remove(TransientFlags::CHARGED);
                    }
                }
            }

            // Acid corrosion onto adjacent organic / metallic materials.
            if cell.transient.contains(TransientFlags::CORROSIVE)
                || cell.transient.contains(TransientFlags::CORRODED)
            {
                let d = ORTHO_DIRS[(self.next_rand() as usize) % ORTHO_DIRS.len()];
                let n = [pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]];
                if let Some((nid, mut nc, _)) = self.ensure_cell_entry(n) {
                    if !nc.material.contains(MaterialFlags::GLASS)
                        && !nc.material.contains(MaterialFlags::RAD_SHIELDED)
                        && nid != mat_id::ACID
                    {
                        nc.transient.insert(TransientFlags::CORRODED);
                        nc.durability = nc.durability.saturating_sub(18);
                        if nc.durability == 0 {
                            self.clear_l2_cell(n);
                        } else {
                            self.write_cell_visual(n, nid, nc);
                        }
                        wake_list.push(n);
                    }
                }
            }

            // Cryo chilling spreads cold and freezes adjacent liquids.
            if cell.env.temperature() <= Temperature::Cryo {
                for d in HORIZ_DIRS {
                    let n = [pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]];
                    if let Some((nid, mut nc)) = self.cell_get(n) {
                        if nc.env.matter() == MatterState::Liquid
                            && nc.env.temperature() > Temperature::Cryo
                        {
                            nc.apply_stimulus(StimulusFlags::COLD);
                            let _ = nc.step_reaction(false);
                            self.write_cell_visual(n, nid, nc);
                            wake_list.push(n);
                        }
                    }
                }
            }

            // Gravity & horizontal fluid spread for unfrozen liquids.
            if cell.env.matter() == MatterState::Liquid
                && !cell.transient.contains(TransientFlags::FROZEN)
            {
                let below = [pos[0], pos[1] - 1, pos[2]];
                if let Some((b_idx, b_local)) = self.chunk_coords_for_l2(below) {
                    if self.chunks[b_idx]
                        .get_l2_cell(b_local[0], b_local[1], b_local[2])
                        .is_none()
                    {
                        self.clear_l2_cell(pos);
                        let (rgba, vis) = self.materials.cell_visuals(id, &cell);
                        self.chunk_cells[b_idx].insert(below, (id, cell));
                        self.chunks[b_idx].set_l2_cell(
                            b_local[0],
                            b_local[1],
                            b_local[2],
                            rgba,
                            vis,
                            u64::MAX,
                        );
                        wake_list.push(below);
                        continue;
                    }
                    // Lateral trickle when resting on a solid or liquid floor (avoid infinite flood).
                    if (self.next_rand() & 3) == 0 && self.active_set.len() < 1200 {
                        let start = (self.next_rand() as usize) % HORIZ_DIRS.len();
                        for off in 0..HORIZ_DIRS.len() {
                            let d = HORIZ_DIRS[(start + off) % HORIZ_DIRS.len()];
                            let side = [pos[0] + d[0], pos[1], pos[2] + d[2]];
                            let side_below = [side[0], side[1] - 1, side[2]];
                            let Some((s_idx, s_local)) = self.chunk_coords_for_l2(side) else {
                                continue;
                            };
                            let Some((sb_idx, sb_local)) = self.chunk_coords_for_l2(side_below)
                            else {
                                continue;
                            };
                            let side_free = self.chunks[s_idx]
                                .get_l2_cell(s_local[0], s_local[1], s_local[2])
                                .is_none();
                            let drop_below = self.chunks[sb_idx]
                                .get_l2_cell(sb_local[0], sb_local[1], sb_local[2])
                                .is_none();
                            if side_free && drop_below {
                                self.clear_l2_cell(pos);
                                let (rgba, vis) = self.materials.cell_visuals(id, &cell);
                                self.chunk_cells[s_idx].insert(side, (id, cell));
                                self.chunks[s_idx].set_l2_cell(
                                    s_local[0],
                                    s_local[1],
                                    s_local[2],
                                    rgba,
                                    vis,
                                    u64::MAX,
                                );
                                wake_list.push(side);
                                break;
                            }
                        }
                    }
                }
            }

            if !keep_awake {
                cell.transient.insert(TransientFlags::DORMANT);
            }
            self.write_cell_visual(pos, id, cell);
            if keep_awake {
                wake_list.push(pos);
            }
        }

        for w in wake_list {
            self.wake_cell_and_neighbors(w, false);
        }

        // Resolve any volatile tank / isotope explosions.
        let blasts = std::mem::take(&mut self.pending_blasts);
        for center in blasts {
            self.detonate(center, 5);
        }
    }

    fn step_bio_5hz(&mut self) {
        let snapshot: Vec<[i32; 3]> = self.active_set.iter().copied().collect();
        for pos in snapshot {
            let Some((id, mut cell)) = self.cell_get(pos) else {
                continue;
            };
            if !cell.material.contains(MaterialFlags::ORGANIC)
                && !cell.topology.contains(TopologyFlags::BIO_NEST)
            {
                continue;
            }
            if cell.env.temperature() >= Temperature::Ambient
                && cell.env.bio_activity() == BioActivity::Cryptosis
                && cell.topology.contains(TopologyFlags::BIO_NEST)
            {
                cell.env.set_bio_activity(BioActivity::Vital);
                cell.transient.insert(TransientFlags::INFECTED);
                self.write_cell_visual(pos, id, cell);
            } else if cell.env.bio_activity() >= BioActivity::Vital
                && !cell.transient.contains(TransientFlags::BURNING)
                && (self.next_rand() & 7) == 0
            {
                let d = ORTHO_DIRS[(self.next_rand() as usize) % ORTHO_DIRS.len()];
                let n = [pos[0] + d[0], pos[1] + d[1], pos[2] + d[2]];
                if let Some((nid, mut nc)) = self.cell_get(n) {
                    if nc.topology.contains(TopologyFlags::EDIBLE)
                        && !nc.transient.contains(TransientFlags::INFECTED)
                    {
                        nc.env.set_bio_activity(BioActivity::Vital);
                        nc.transient.insert(TransientFlags::INFECTED);
                        self.write_cell_visual(n, nid, nc);
                        self.wake_cell_and_neighbors(n, true);
                    }
                }
            }
        }
    }

    fn step_clocks_1hz(&mut self) {
        let snapshot: Vec<[i32; 3]> = self.active_set.iter().copied().collect();
        for pos in snapshot {
            let Some((id, mut cell)) = self.cell_get(pos) else {
                continue;
            };
            if cell.clocks.half_life() > 0 {
                let _ = cell.step_reaction(true);
                self.write_cell_visual(pos, id, cell);
            }
        }
    }

    fn detonate(&mut self, center: [i32; 3], radius_l2: i32) {
        let r_sq = radius_l2 * radius_l2;
        for dz in -radius_l2..=radius_l2 {
            for dy in -radius_l2..=radius_l2 {
                for dx in -radius_l2..=radius_l2 {
                    let dist_sq = dx * dx + dy * dy + dz * dz;
                    if dist_sq > r_sq {
                        continue;
                    }
                    let pos = [center[0] + dx, center[1] + dy, center[2] + dz];
                    if dist_sq <= (radius_l2 - 2).max(1).pow(2) {
                        self.clear_l2_cell(pos);
                    } else if let Some((nid, mut nc, _)) = self.ensure_cell_entry(pos) {
                        nc.apply_stimulus(StimulusFlags::HEAT);
                        nc.apply_stimulus(StimulusFlags::KINETIC);
                        let _ = nc.step_reaction(false);
                        self.write_cell_visual(pos, nid, nc);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_gen::generate_world;

    #[test]
    fn l3_carve_empty_enforces_anti_ghost_wall_and_wakes_neighbors() {
        let mats = MaterialTable::compiled_default();
        let gen = generate_world(&mats, 42);
        let mut sim = SimWorld::new(mats, gen.chunks, gen.cells, gen.active);

        // Place a dedicated alloy L2 cell and carve all 64 L3 micro-voxels.
        let l2 = [20, 22, 20];
        let (c_idx, local_l2) = sim.chunk_coords_for_l2(l2).unwrap();
        let alloy_cell = sim.materials.spawn_cell(mat_id::ALLOY);
        let (rgba, vis) = sim.materials.cell_visuals(mat_id::ALLOY, &alloy_cell);
        sim.chunks[c_idx].set_l2_cell(local_l2[0], local_l2[1], local_l2[2], rgba, vis, u64::MAX);
        sim.chunk_cells[c_idx].insert(l2, (mat_id::ALLOY, alloy_cell));

        for mz in 0..4 {
            for my in 0..4 {
                for mx in 0..4 {
                    let hit = VoxelHit {
                        distance: 1.0,
                        world_pos: [5.0, 5.5, 5.0],
                        normal: [0, 0, -1],
                        world_l1: [l2[0] >> 2, l2[1] >> 2, l2[2] >> 2],
                        world_l2: l2,
                        world_l3: [l2[0] * 4 + mx, l2[1] * 4 + my, l2[2] * 4 + mz],
                        rgba,
                        visual: vis,
                        hit_tier: VoxelTier::L2,
                    };
                    assert!(sim.carve_at_hit(VoxelTier::L3, &hit));
                }
            }
        }
        assert!(
            sim.inspect_l2(l2).is_none(),
            "empty L3 mask must remove parent L2 cell"
        );
    }

    #[test]
    fn stimulus_wakes_dormant_cells_and_propagates_wet_charge() {
        let mats = MaterialTable::compiled_default();
        let gen = generate_world(&mats, 7);
        let mut sim = SimWorld::new(mats, gen.chunks, gen.cells, gen.active);

        let l2a = [40, 24, 40];
        let l2b = [41, 24, 40];
        for p in [l2a, l2b] {
            let (c_idx, local) = sim.chunk_coords_for_l2(p).unwrap();
            let mut c = sim.materials.spawn_cell(mat_id::WIRE);
            c.transient.insert(TransientFlags::WET);
            let (rgba, vis) = sim.materials.cell_visuals(mat_id::WIRE, &c);
            sim.chunks[c_idx].set_l2_cell(local[0], local[1], local[2], rgba, vis, u64::MAX);
            sim.chunk_cells[c_idx].insert(p, (mat_id::WIRE, c));
        }

        sim.inject_stimulus(l2a, StimulusFlags::SHOCK, false, 0);
        for _ in 0..4 {
            sim.step(0.05);
        }

        let (_, cb, _) = sim.inspect_l2(l2b).expect("neighbor wire exists");
        assert!(cb.transient.contains(TransientFlags::CHARGED));
    }

    #[test]
    fn burning_timber_depletes_and_triggers_anti_ghost_wall() {
        let mats = MaterialTable::compiled_default();
        let gen = generate_world(&mats, 9);
        let mut sim = SimWorld::new(mats, gen.chunks, gen.cells, gen.active);

        let target = [22, 22, 22];
        let (c_idx, local) = sim.chunk_coords_for_l2(target).unwrap();
        let mut timber = sim.materials.spawn_cell(mat_id::TIMBER);
        timber.durability = 20;
        let (rgba, vis) = sim.materials.cell_visuals(mat_id::TIMBER, &timber);
        sim.chunks[c_idx].set_l2_cell(local[0], local[1], local[2], rgba, vis, u64::MAX);
        sim.chunk_cells[c_idx].insert(target, (mat_id::TIMBER, timber));

        sim.inject_stimulus(target, StimulusFlags::HEAT, false, 0);
        for _ in 0..10 {
            sim.step(0.05);
        }
        assert!(
            sim.inspect_l2(target).is_none(),
            "burned-out timber must become empty air"
        );
    }

    #[test]
    fn runtime_chunk_streaming_loads_new_chunks_and_unloads_distant_ones() {
        let mats = MaterialTable::compiled_default();
        let gen = generate_world(&mats, 2026);
        let mut sim = SimWorld::new(mats, gen.chunks, gen.cells, gen.active);
        let mut ecs = prism_ecs::World::new();
        let mut world = GameWorld::host(&mut ecs);
        sim.attach_ecs(&mut world);

        // Move far away to `(160m, 4m, 160m)` -> chunk `(10, 10)` and stream in real time.
        sim.stream_around(glam::Vec3::new(160.0, 4.0, 160.0), Some(&mut world), 32);
        assert!(
            sim.chunk_lookup.contains_key(&[10, 10]),
            "chunk (10, 10) must stream in around player"
        );
        assert!(
            !sim.chunk_lookup.contains_key(&[0, 0]),
            "distant chunk (0, 0) must unload"
        );
        assert_eq!(sim.chunks.len(), sim.chunk_entities.len());
        assert_eq!(sim.chunks.len(), sim.chunk_cells.len());
        let expected_tracked: usize = sim.chunk_cells.iter().map(HashMap::len).sum();
        assert_eq!(sim.tracked_cell_count(), expected_tracked);
    }
}
