//! Deterministic procedural 3D map and Emergence Research Outpost generator.

use std::collections::HashMap;

use prism_scene::{VoxelBrick, VoxelChunk};

use crate::materials::{mat_id, MaterialTable};
use crate::sim::{SimWorld, CHUNK_DIMS_L1, WORLD_CHUNKS_X, WORLD_CHUNKS_Z};
use crate::tags::{PhysicalCell, TransientFlags};

/// Sparse map from world L2 coordinate to `(material_id, PhysicalCell)`.
pub type SparseCellMap = HashMap<[i32; 3], (u8, PhysicalCell)>;

/// Result of deterministic map generation: chunks, sparse L2 cells, and awake seeds.
pub struct GeneratedWorld {
    pub chunks: Vec<VoxelChunk>,
    pub cells: SparseCellMap,
    pub active: Vec<[i32; 3]>,
}

struct MapBuilder<'a> {
    materials: &'a MaterialTable,
    chunks: Vec<VoxelChunk>,
    cells: SparseCellMap,
    active: Vec<[i32; 3]>,
}

impl<'a> MapBuilder<'a> {
    fn new(materials: &'a MaterialTable) -> Self {
        let mut chunks = Vec::with_capacity(WORLD_CHUNKS_X * WORLD_CHUNKS_Z);
        for cz in 0..WORLD_CHUNKS_Z {
            for cx in 0..WORLD_CHUNKS_X {
                chunks.push(VoxelChunk::new(
                    [(cx as u32 * CHUNK_DIMS_L1[0]) as i32, 0, (cz as u32 * CHUNK_DIMS_L1[2]) as i32],
                    CHUNK_DIMS_L1,
                ));
            }
        }
        Self {
            materials,
            chunks,
            cells: HashMap::new(),
            active: Vec::new(),
        }
    }

    fn set_l1_cow(&mut self, l1: [i32; 3], mat: u8) {
        if l1[0] < 0
            || l1[1] < 0
            || l1[2] < 0
            || l1[0] >= (WORLD_CHUNKS_X as u32 * CHUNK_DIMS_L1[0]) as i32
            || l1[1] >= CHUNK_DIMS_L1[1] as i32
            || l1[2] >= (WORLD_CHUNKS_Z as u32 * CHUNK_DIMS_L1[2]) as i32
        {
            return;
        }
        let cx = l1[0] as usize / CHUNK_DIMS_L1[0] as usize;
        let cz = l1[2] as usize / CHUNK_DIMS_L1[2] as usize;
        let lx = l1[0] as u32 % CHUNK_DIMS_L1[0];
        let ly = l1[1] as u32;
        let lz = l1[2] as u32 % CHUNK_DIMS_L1[2];
        for dz in 0..4 {
            for dy in 0..4 {
                for dx in 0..4 {
                    self.cells.remove(&[l1[0] * 4 + dx, l1[1] * 4 + dy, l1[2] * 4 + dz]);
                }
            }
        }
        let cell = self.materials.spawn_cell(mat);
        let (rgba, vis) = self.materials.cell_visuals(mat, &cell);
        self.chunks[cx + WORLD_CHUNKS_X * cz].set_l1_uniform(lx, ly, lz, rgba, vis);
    }

    fn set_l2(&mut self, l2: [i32; 3], mat: u8, active: bool) {
        let Some((c_idx, local)) = SimWorld::chunk_coords_for_l2(l2) else {
            return;
        };
        if mat == mat_id::AIR {
            self.cells.remove(&l2);
            self.chunks[c_idx].set_l2_cell(local[0], local[1], local[2], 0, 0, 0);
            return;
        }
        let mut cell = self.materials.spawn_cell(mat);
        if active {
            cell.transient.remove(TransientFlags::DORMANT);
            self.active.push(l2);
        }
        let (rgba, vis) = self.materials.cell_visuals(mat, &cell);
        self.cells.insert(l2, (mat, cell));
        self.chunks[c_idx].set_l2_cell(local[0], local[1], local[2], rgba, vis, u64::MAX);
    }

    /// Carve or populate a patterned 64-bit L3 micro-voxel mask inside an L2 cell.
    fn set_l2_l3_mask(&mut self, l2: [i32; 3], mat: u8, l3_mask: u64) {
        let Some((c_idx, local)) = SimWorld::chunk_coords_for_l2(l2) else {
            return;
        };
        if l3_mask == 0 || mat == mat_id::AIR {
            self.cells.remove(&l2);
            self.chunks[c_idx].set_l2_cell(local[0], local[1], local[2], 0, 0, 0);
            return;
        }
        let cell = self.materials.spawn_cell(mat);
        let (rgba, vis) = self.materials.cell_visuals(mat, &cell);
        self.cells.insert(l2, (mat, cell));
        self.chunks[c_idx].set_l2_cell(local[0], local[1], local[2], rgba, vis, l3_mask);
    }

    fn fill_l2_box(&mut self, min_l2: [i32; 3], max_l2_excl: [i32; 3], mat: u8, active: bool) {
        for z in min_l2[2]..max_l2_excl[2] {
            for y in min_l2[1]..max_l2_excl[1] {
                for x in min_l2[0]..max_l2_excl[0] {
                    self.set_l2([x, y, z], mat, active);
                }
            }
        }
    }
}

fn hash2d(x: i32, z: i32, seed: u64) -> f32 {
    let mut h = seed ^ ((x as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    h ^= (z as i64 as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 30)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h >> 31;
    ((h >> 40) as f32) / ((1u32 << 24) as f32)
}

fn smooth_noise(x: f32, z: f32, seed: u64) -> f32 {
    let xi = x.floor() as i32;
    let zi = z.floor() as i32;
    let fx = x - xi as f32;
    let fz = z - zi as f32;
    let sx = fx * fx * (3.0 - 2.0 * fx);
    let sz = fz * fz * (3.0 - 2.0 * fz);
    let n00 = hash2d(xi, zi, seed);
    let n10 = hash2d(xi + 1, zi, seed);
    let n01 = hash2d(xi, zi + 1, seed);
    let n11 = hash2d(xi + 1, zi + 1, seed);
    let nx0 = n00 + (n10 - n00) * sx;
    let nx1 = n01 + (n11 - n01) * sx;
    nx0 + (nx1 - nx0) * sz
}

/// Deterministic heightmap elevation in meters (`[3.0 .. 11.0]`).
pub fn terrain_height_m(wx: f32, wz: f32, seed: u64) -> f32 {
    let broad = smooth_noise(wx * 0.11, wz * 0.11, seed) * 2.2
        + smooth_noise(wx * 0.27, wz * 0.27, seed ^ 0x5555) * 0.85;
    let mut h = 3.5 + broad;

    // Gentle highland cliff rising along the rear boundary (`z > 24`) to frame the outpost.
    if wz > 23.0 {
        h += ((wz - 23.0) * 0.72).min(5.2);
    }
    // Flat graded courtyard and Research Outpost pad around `x in [5..26], z in [6..24]`.
    if (4.0..=27.0).contains(&wx) && (4.5..=24.0).contains(&wz) {
        let dx = ((wx - 15.5).abs() - 8.5).max(0.0) / 3.0;
        let dz = ((wz - 14.5).abs() - 7.5).max(0.0) / 2.5;
        let blend = (dx.max(dz)).clamp(0.0, 1.0);
        h = 4.0 + (h - 4.0) * (blend * blend);
    }
    // Water basin depression (`x in [6..10.5], z in [11..14.5]`).
    if (6.0..=10.5).contains(&wx) && (11.0..=14.5).contains(&wz) {
        h = h.min(3.25);
    }
    // Acid containment trough (`x in [23..26.5], z in [13..18]`).
    if (23.0..=26.5).contains(&wx) && (13.0..=18.0).contains(&wz) {
        h = h.min(3.25);
    }
    h.clamp(2.5, 13.0)
}

fn grate_l3_mask() -> u64 {
    // Open L3 vertical & horizontal steel lattice mullions (`0.0625m` thin bars with see-through gaps).
    let mut mask = 0u64;
    for mz in 0..4u32 {
        for my in 0..4u32 {
            for mx in 0..4u32 {
                let on_bar = (mx == 0 || my == 0) && (mz == 1 || mz == 2);
                if on_bar {
                    mask |= 1u64 << VoxelChunk::l3_sub_index(mx, my, mz);
                }
            }
        }
    }
    mask
}

fn thin_pane_l3_mask_z() -> u64 {
    // 0.125m thin central Z plate inside an L2 cell (`mz == 1` and `mz == 2`).
    let mut mask = 0u64;
    for mz in 1..=2u32 {
        for my in 0..4u32 {
            for mx in 0..4u32 {
                mask |= 1u64 << VoxelChunk::l3_sub_index(mx, my, mz);
            }
        }
    }
    mask
}

fn thin_pipe_l3_mask_x() -> u64 {
    // Centered `0.125m` diameter conduit core running along X axis (`my in 1..3, mz in 1..3`).
    let mut mask = 0u64;
    for mz in 1..=2u32 {
        for my in 1..=2u32 {
            for mx in 0..4u32 {
                mask |= 1u64 << VoxelChunk::l3_sub_index(mx, my, mz);
            }
        }
    }
    mask
}

fn micro_steps_l3_mask() -> u64 {
    // L3 quarter-stair profile along +Z: bottom 4 rows solid, top back raised.
    let mut mask = 0u64;
    for mz in 0..4u32 {
        for my in 0..4u32 {
            for mx in 0..4u32 {
                let height_limit = (mz / 2) * 2 + 2;
                if my < height_limit {
                    mask |= 1u64 << VoxelChunk::l3_sub_index(mx, my, mz);
                }
            }
        }
    }
    mask
}

/// Generate the procedural terrain, three fluid/cryo pools, Research Outpost, and L3 details.
pub fn generate_world(materials: &MaterialTable, seed: u64) -> GeneratedWorld {
    let mut builder = MapBuilder::new(materials);
    let l1_x_max = WORLD_CHUNKS_X as i32 * CHUNK_DIMS_L1[0] as i32;
    let l1_z_max = WORLD_CHUNKS_Z as i32 * CHUNK_DIMS_L1[2] as i32;

    // 1. Copy-On-Write L1 Bedrock basement (`y in 0..2m` -> zero brick allocations).
    for l1z in 0..l1_z_max {
        for l1x in 0..l1_x_max {
            builder.set_l1_cow([l1x, 0, l1z], mat_id::BEDROCK);
            builder.set_l1_cow([l1x, 1, l1z], mat_id::BEDROCK);
            builder.set_l1_cow([l1x, 2, l1z], mat_id::SOIL);
        }
    }

    // 2. Sculpt L2 (`0.25m`) surface terrain heightmap & natural strata above y = 3.0m (`l2y >= 12`).
    for l2z in 0..(l1_z_max * 4) {
        for l2x in 0..(l1_x_max * 4) {
            let wx = l2x as f32 * 0.25 + 0.125;
            let wz = l2z as f32 * 0.25 + 0.125;
            let h_m = terrain_height_m(wx, wz, seed);
            let top_l2y = (h_m * 4.0).round() as i32;
            for l2y in 12..top_l2y {
                let mat = if l2y + 1 == top_l2y && h_m < 8.0 {
                    mat_id::GRASS
                } else if (h_m >= 8.0 && l2y >= top_l2y - 3) || l2y <= 13 {
                    mat_id::LIMESTONE
                } else {
                    mat_id::SOIL
                };
                builder.set_l2([l2x, l2y, l2z], mat, false);
            }
        }
    }

    // 3. Natural Water pond (`x in 6..10.5m, z in 11..14.5m`) & Acid trench (`x in 23..26.5m, z in 13..18m`).
    // Pools start settled inside their stone basins and wake immediately when breached.
    for l2z in 44..58 {
        for l2x in 24..42 {
            for l2y in 13..16 {
                builder.set_l2([l2x, l2y, l2z], mat_id::WATER, false);
            }
        }
    }
    for l2z in 52..72 {
        for l2x in 92..106 {
            for l2y in 13..16 {
                builder.set_l2([l2x, l2y, l2z], mat_id::ACID, false);
            }
        }
    }
    // Timber walkway spanning across the acid trough so player can burn or cross it!
    builder.fill_l2_box([90, 16, 60], [108, 17, 64], mat_id::TIMBER, false);

    // 4. Emergence Research Outpost (`x in 13..22m` -> `l2x in 52..88`, `z in 15..23m` -> `l2z in 60..92`, floor at `y=4.0m` -> `l2y=16`).
    builder.fill_l2_box([50, 15, 58], [90, 16, 94], mat_id::ALLOY, false);
    // Clear interior volume (`y = 4.0..7.0m`).
    builder.fill_l2_box([53, 16, 61], [87, 28, 91], mat_id::AIR, false);

    // Outer Limestone/Alloy walls (`l2y in 16..28`).
    builder.fill_l2_box([52, 16, 60], [88, 28, 61], mat_id::LIMESTONE, false); // Front Z wall (`z=15`)
    builder.fill_l2_box([52, 16, 91], [88, 28, 92], mat_id::LIMESTONE, false); // Back Z wall (`z=22.75`)
    builder.fill_l2_box([52, 16, 60], [53, 28, 92], mat_id::LIMESTONE, false); // West X wall (`x=13`)
    builder.fill_l2_box([87, 16, 60], [88, 28, 92], mat_id::LIMESTONE, false); // East X wall (`x=21.75`)

    // Alloy roof perimeter + skylight opening (`l2y = 28`).
    builder.fill_l2_box([51, 28, 59], [89, 29, 93], mat_id::ALLOY, false);
    builder.fill_l2_box([60, 28, 68], [80, 29, 84], mat_id::GLASS, false);

    // Front Doorway (`x in 17..19m` -> `l2x in 68..76`, `l2y in 16..25`).
    builder.fill_l2_box([68, 16, 60], [76, 25, 61], mat_id::AIR, false);
    // L3 micro-voxel beveled entrance sill & steps outside doorway (`l2z = 59`).
    for l2x in 68..76 {
        builder.set_l2_l3_mask([l2x, 16, 59], mat_id::ALLOY, micro_steps_l3_mask());
    }

    // Large Front Glass Observation Window (`l2x in 55..66`, `l2y in 19..26`, `l2z = 60`).
    builder.fill_l2_box([55, 19, 60], [66, 26, 61], mat_id::GLASS, false);
    // Thin L3 metallic mullions dividing the glass window every meter (`l2x = 59, 62`).
    for l2y in 19..26 {
        builder.set_l2_l3_mask([59, l2y, 60], mat_id::ALLOY, grate_l3_mask());
        builder.set_l2_l3_mask([62, l2y, 60], mat_id::ALLOY, thin_pane_l3_mask_z());
    }

    // Side L3 Ventilation Grate (`l2x = 52`, `l2z in 70..78`, `l2y in 19..24`).
    for l2z in 70..78 {
        for l2y in 19..24 {
            builder.set_l2_l3_mask([52, l2y, l2z], mat_id::ALLOY, grate_l3_mask());
        }
    }

    // Cryo-Biomass Containment Chamber inside right bay (`x in 19..21m, z in 17..20m`).
    builder.fill_l2_box([76, 16, 68], [84, 18, 80], mat_id::CRYO, false);
    builder.fill_l2_box([78, 16, 72], [82, 20, 76], mat_id::BIOMASS, true);
    builder.fill_l2_box([76, 18, 67], [84, 24, 68], mat_id::GLASS, false);

    // Reactor Pedestal + Glowing Isotope Core inside center bay (`x=17.5m, z=19.5m`).
    builder.fill_l2_box([68, 16, 76], [72, 18, 80], mat_id::ALLOY, false);
    builder.fill_l2_box([69, 18, 77], [71, 20, 79], mat_id::ISOTOPE, true);

    // Overhead L3 thin pressurized pipe run + Valve feeding Isotope bay -> Courtyard rig!
    for l2x in 46..70 {
        builder.set_l2_l3_mask([l2x, 22, 66], mat_id::CONDUIT, thin_pipe_l3_mask_x());
    }
    builder.set_l2([46, 22, 66], mat_id::VALVE, false);

    // Wet-Wire Short-Circuit Bridge linking Valve/Wire -> Water Pond -> Courtyard Explosive Tank rig!
    for l2x in 38..69 {
        builder.set_l2([l2x, 16, 52], mat_id::WIRE, false);
    }
    builder.set_l2([37, 16, 52], mat_id::TANK, false);
    builder.set_l2([37, 17, 52], mat_id::TANK, false);

    // Courtyard Timber Crates + Test Pillar right in view of initial camera!
    builder.fill_l2_box([40, 16, 40], [43, 19, 43], mat_id::TIMBER, false);
    builder.fill_l2_box([44, 16, 42], [46, 18, 44], mat_id::TIMBER, false);
    builder.set_l2([43, 19, 41], mat_id::VALVE, false);
    builder.set_l2([45, 18, 43], mat_id::TANK, false);

    // Visible 1.0m Copy-On-Write L1 monoliths & bastion piers (zero brick allocation until split).
    builder.set_l1_cow([10, 4, 8], mat_id::LIMESTONE);
    builder.set_l1_cow([10, 5, 8], mat_id::ALLOY);
    builder.set_l1_cow([11, 4, 8], mat_id::LIMESTONE);
    builder.set_l1_cow([13, 4, 11], mat_id::LIMESTONE);
    builder.set_l1_cow([14, 4, 11], mat_id::ALLOY);
    builder.set_l1_cow([12, 4, 14], mat_id::ALLOY);
    builder.set_l1_cow([12, 5, 14], mat_id::LIMESTONE);
    builder.set_l1_cow([22, 4, 14], mat_id::ALLOY);
    builder.set_l1_cow([22, 5, 14], mat_id::LIMESTONE);

    // Highland cliff interior strata above y=3m converted to pure 1.0m L1 COW rock cores.
    for l1z in 24..l1_z_max {
        for l1x in 0..l1_x_max {
            let mut min_h = f32::MAX;
            for dz in 0..4 {
                for dx in 0..4 {
                    let wx = (l1x * 4 + dx) as f32 * 0.25 + 0.125;
                    let wz = (l1z * 4 + dz) as f32 * 0.25 + 0.125;
                    min_h = min_h.min(terrain_height_m(wx, wz, seed));
                }
            }
            let top_full_l1y = min_h.floor() as i32;
            for l1y in 3..top_full_l1y {
                builder.set_l1_cow([l1x, l1y, l1z], mat_id::LIMESTONE);
            }
        }
    }

    // Keep unused helper reference suppressed cleanly (`VoxelBrick` size assert).
    let _ = std::mem::size_of::<VoxelBrick>();

    GeneratedWorld {
        chunks: builder.chunks,
        cells: builder.cells,
        active: builder.active,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_scene::L1_UNIFORM_REF;

    #[test]
    fn world_gen_creates_l1_cow_l2_terrain_and_l3_details() {
        let mats = MaterialTable::compiled_default();
        let gen = generate_world(&mats, 2026);
        let (chunks, cells, active) = (gen.chunks, gen.cells, gen.active);
        assert_eq!(chunks.len(), WORLD_CHUNKS_X * WORLD_CHUNKS_Z);
        assert!(!cells.is_empty());
        assert!(!active.is_empty());

        // Verify underground bedrock and above-ground monoliths use Copy-On-Write L1 uniform storage.
        let cow_count: usize = chunks
            .iter()
            .flat_map(|c| c.l1_brick_ref.iter())
            .filter(|&&r| r == L1_UNIFORM_REF)
            .count();
        assert!(cow_count > 100, "strata and monoliths must use COW L1 uniform storage");

        let (mon_idx, _) = SimWorld::chunk_coords_for_l2([40, 16, 32]).unwrap();
        let mon_l1_idx = chunks[mon_idx].l1_index(10 % 8, 4, 8 % 8).unwrap();
        assert_eq!(
            chunks[mon_idx].l1_brick_ref[mon_l1_idx],
            L1_UNIFORM_REF,
            "courtyard display monolith [10, 4, 8] must be visible L1 COW"
        );

        // Verify L3 micro-voxel grate exists on the front window mullion (not u64::MAX).
        let (c_idx, local) = SimWorld::chunk_coords_for_l2([59, 20, 60]).unwrap();
        let (_, _, mask) = chunks[c_idx]
            .get_l2_cell(local[0], local[1], local[2])
            .expect("mullion cell exists");
        assert_ne!(mask, 0);
        assert_ne!(mask, u64::MAX, "window mullion must use sub-L2 L3 bitmask");
    }
}
