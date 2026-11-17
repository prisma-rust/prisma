//! Deterministic per-chunk procedural 3D terrain, caves, water bodies,
//! mountains, and Minecraft-style template-pool structure generator.
//!
//! Supports both single-chunk streaming (`generate_chunk(materials, seed, cx, cz)`)
//! across infinite signed chunk coordinates and initial multi-chunk bootstrap
//! (`generate_world(materials, seed)`).

use std::collections::HashMap;

use prism_scene::VoxelChunk;

use crate::materials::{mat_id, MaterialTable};
use crate::sim::CHUNK_DIMS_L1;
use crate::structures::{
    build_default_template_pool, StructureOp, StructureRot, StructureTemplate,
};
use crate::tags::{PhysicalCell, TransientFlags};

/// Sea level in meters (`l2y = 14`).
pub const SEA_LEVEL_M: f32 = 3.5;

/// Initial bootstrap chunk window around `(0, 0)` (`4 x 4 = 16` chunks = `64m x 64m x 64m`).
pub const BOOTSTRAP_MIN_CHUNK: i32 = -1;
pub const BOOTSTRAP_MAX_CHUNK: i32 = 2;

/// Sparse map from world L2 coordinate to `(material_id, PhysicalCell)`.
pub type SparseCellMap = HashMap<[i32; 3], (u8, PhysicalCell)>;

/// Result of generating a single `16m x 64m x 16m` chunk at `(cx, cz)`.
pub struct GeneratedChunk {
    pub coord: [i32; 2],
    pub chunk: VoxelChunk,
    pub cells: SparseCellMap,
    pub active: Vec<[i32; 3]>,
}

/// Result of deterministic bootstrap map generation: chunks, coords, sparse L2 cells, and awake seeds.
pub struct GeneratedWorld {
    pub chunks: Vec<VoxelChunk>,
    pub chunk_coords: Vec<[i32; 2]>,
    pub cells: SparseCellMap,
    pub active: Vec<[i32; 3]>,
}

struct ChunkBuilder<'a> {
    materials: &'a MaterialTable,
    cx: i32,
    cz: i32,
    l1_origin_x: i32,
    l1_origin_z: i32,
    chunk: VoxelChunk,
    cells: SparseCellMap,
    active: Vec<[i32; 3]>,
}

impl<'a> ChunkBuilder<'a> {
    fn new(materials: &'a MaterialTable, cx: i32, cz: i32) -> Self {
        let l1_origin_x = cx * CHUNK_DIMS_L1[0] as i32;
        let l1_origin_z = cz * CHUNK_DIMS_L1[2] as i32;
        Self {
            materials,
            cx,
            cz,
            l1_origin_x,
            l1_origin_z,
            chunk: VoxelChunk::new([l1_origin_x, 0, l1_origin_z], CHUNK_DIMS_L1),
            cells: HashMap::new(),
            active: Vec::new(),
        }
    }

    fn set_l1_cow(&mut self, local_l1: [u32; 3], mat: u8) {
        if local_l1[0] >= CHUNK_DIMS_L1[0]
            || local_l1[1] >= CHUNK_DIMS_L1[1]
            || local_l1[2] >= CHUNK_DIMS_L1[2]
        {
            return;
        }
        let wl1_x = self.l1_origin_x + local_l1[0] as i32;
        let wl1_y = local_l1[1] as i32;
        let wl1_z = self.l1_origin_z + local_l1[2] as i32;
        for dz in 0..4 {
            for dy in 0..4 {
                for dx in 0..4 {
                    self.cells
                        .remove(&[wl1_x * 4 + dx, wl1_y * 4 + dy, wl1_z * 4 + dz]);
                }
            }
        }
        let cell = self.materials.spawn_cell(mat);
        let (rgba, vis) = self.materials.cell_visuals(mat, &cell);
        self.chunk
            .set_l1_uniform(local_l1[0], local_l1[1], local_l1[2], rgba, vis);
    }

    fn set_l2_world(&mut self, l2: [i32; 3], mat: u8, active: bool) {
        let l2_w = (CHUNK_DIMS_L1[0] * 4) as i32;
        let l2_h = (CHUNK_DIMS_L1[1] * 4) as i32;
        let l2_d = (CHUNK_DIMS_L1[2] * 4) as i32;
        let lx = l2[0] - self.l1_origin_x * 4;
        let ly = l2[1];
        let lz = l2[2] - self.l1_origin_z * 4;
        if lx < 0 || ly < 0 || lz < 0 || lx >= l2_w || ly >= l2_h || lz >= l2_d {
            return;
        }
        if mat == mat_id::AIR {
            self.cells.remove(&l2);
            self.chunk
                .set_l2_cell(lx as u32, ly as u32, lz as u32, 0, 0, 0);
            return;
        }
        let mut cell = self.materials.spawn_cell(mat);
        if active {
            cell.transient.remove(TransientFlags::DORMANT);
            self.active.push(l2);
        }
        let (rgba, vis) = self.materials.cell_visuals(mat, &cell);
        self.cells.insert(l2, (mat, cell));
        self.chunk
            .set_l2_cell(lx as u32, ly as u32, lz as u32, rgba, vis, u64::MAX);
    }

    fn set_l2_l3_mask_world(&mut self, l2: [i32; 3], mat: u8, l3_mask: u64) {
        let l2_w = (CHUNK_DIMS_L1[0] * 4) as i32;
        let l2_h = (CHUNK_DIMS_L1[1] * 4) as i32;
        let l2_d = (CHUNK_DIMS_L1[2] * 4) as i32;
        let lx = l2[0] - self.l1_origin_x * 4;
        let ly = l2[1];
        let lz = l2[2] - self.l1_origin_z * 4;
        if lx < 0 || ly < 0 || lz < 0 || lx >= l2_w || ly >= l2_h || lz >= l2_d {
            return;
        }
        if l3_mask == 0 || mat == mat_id::AIR {
            self.cells.remove(&l2);
            self.chunk
                .set_l2_cell(lx as u32, ly as u32, lz as u32, 0, 0, 0);
            return;
        }
        let cell = self.materials.spawn_cell(mat);
        let (rgba, vis) = self.materials.cell_visuals(mat, &cell);
        self.cells.insert(l2, (mat, cell));
        self.chunk
            .set_l2_cell(lx as u32, ly as u32, lz as u32, rgba, vis, l3_mask);
    }

    fn place_structure(
        &mut self,
        template: &StructureTemplate,
        origin_l2: [i32; 3],
        rot: StructureRot,
    ) {
        if let Some(fmat) = template.foundation_mat {
            let fp = rot.rotated_footprint(template.size);
            for dz in 0..fp[1] {
                for dx in 0..fp[0] {
                    let wx = origin_l2[0] + dx;
                    let wz = origin_l2[2] + dz;
                    let min_fy = (origin_l2[1] - 12).max(4);
                    for fy in min_fy..origin_l2[1] {
                        self.set_l2_world([wx, fy, wz], fmat, false);
                    }
                }
            }
        }

        for &op in &template.ops {
            match op {
                StructureOp::Solid { pos, mat, active } => {
                    let r = rot.rotate_xz(pos, template.size);
                    self.set_l2_world(
                        [
                            origin_l2[0] + r[0],
                            origin_l2[1] + r[1],
                            origin_l2[2] + r[2],
                        ],
                        mat,
                        active,
                    );
                }
                StructureOp::Carve { pos } => {
                    let r = rot.rotate_xz(pos, template.size);
                    self.set_l2_world(
                        [
                            origin_l2[0] + r[0],
                            origin_l2[1] + r[1],
                            origin_l2[2] + r[2],
                        ],
                        mat_id::AIR,
                        false,
                    );
                }
                StructureOp::Micro { pos, mat, l3_mask } => {
                    let r = rot.rotate_xz(pos, template.size);
                    self.set_l2_l3_mask_world(
                        [
                            origin_l2[0] + r[0],
                            origin_l2[1] + r[1],
                            origin_l2[2] + r[2],
                        ],
                        mat,
                        l3_mask,
                    );
                }
            }
        }
    }

    fn finish(self) -> GeneratedChunk {
        GeneratedChunk {
            coord: [self.cx, self.cz],
            chunk: self.chunk,
            cells: self.cells,
            active: self.active,
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

fn hash3d(x: i32, y: i32, z: i32, seed: u64) -> f32 {
    let mut h = seed ^ ((x as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    h ^= (y as i64 as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93);
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

fn smooth_noise_3d(x: f32, y: f32, z: f32, seed: u64) -> f32 {
    let xi = x.floor() as i32;
    let yi = y.floor() as i32;
    let zi = z.floor() as i32;
    let fx = x - xi as f32;
    let fy = y - yi as f32;
    let fz = z - zi as f32;
    let sx = fx * fx * (3.0 - 2.0 * fx);
    let sy = fy * fy * (3.0 - 2.0 * fy);
    let sz = fz * fz * (3.0 - 2.0 * fz);

    let c000 = hash3d(xi, yi, zi, seed);
    let c100 = hash3d(xi + 1, yi, zi, seed);
    let c010 = hash3d(xi, yi + 1, zi, seed);
    let c110 = hash3d(xi + 1, yi + 1, zi, seed);
    let c001 = hash3d(xi, yi, zi + 1, seed);
    let c101 = hash3d(xi + 1, yi, zi + 1, seed);
    let c011 = hash3d(xi, yi + 1, zi + 1, seed);
    let c111 = hash3d(xi + 1, yi + 1, zi + 1, seed);

    let x00 = c000 + (c100 - c000) * sx;
    let x10 = c010 + (c110 - c010) * sx;
    let x01 = c001 + (c101 - c001) * sx;
    let x11 = c011 + (c111 - c011) * sx;
    let y0 = x00 + (x10 - x00) * sy;
    let y1 = x01 + (x11 - x01) * sy;
    y0 + (y1 - y0) * sz
}

/// Deterministic heightmap elevation in meters (`[1.6 .. 52.0]`) across infinite `(wx, wz)`.
pub fn terrain_height_m(wx: f32, wz: f32, seed: u64) -> f32 {
    let continental = smooth_noise(wx * 0.016, wz * 0.016, seed ^ 0x1010);
    let ridge_raw = 1.0 - (smooth_noise(wx * 0.028, wz * 0.028, seed ^ 0x2020) * 2.0 - 1.0).abs();
    let ridge = ridge_raw * ridge_raw;
    let detail = smooth_noise(wx * 0.08, wz * 0.08, seed ^ 0x3030) * 2.6
        + smooth_noise(wx * 0.22, wz * 0.22, seed ^ 0x4040) * 0.8;

    let mountain_boost = ((continental - 0.54).max(0.0) / 0.46).powf(1.35) * 42.0 * ridge;
    let basin_dip = ((0.38 - continental).max(0.0) / 0.38) * 3.6;
    let mut h = 4.6 + (continental - 0.45) * 6.0 + detail + mountain_boost - basin_dip;

    // Gentle starter courtyard plateau at `y = 4.0m` near `(15m, 14.5m)` so player spawn and camera stay clear.
    let dx = ((wx - 15.0).abs() - 8.5).max(0.0) / 4.5;
    let dz = ((wz - 14.5).abs() - 7.5).max(0.0) / 4.5;
    let blend = dx.max(dz).clamp(0.0, 1.0);
    h = 4.0 + (h - 4.0) * (blend * blend);

    h.clamp(1.6, 52.0)
}

fn is_cave_air(wx: f32, wy: f32, wz: f32, surface_m: f32, seed: u64) -> bool {
    if wy < 2.2 || wy > surface_m - 1.5 {
        return false;
    }
    let n1 = smooth_noise_3d(wx * 0.11, wy * 0.16, wz * 0.11, seed ^ 0x5151);
    let n2 = smooth_noise_3d(wx * 0.22, wy * 0.26, wz * 0.22, seed ^ 0x6262);
    n1 * 0.72 + n2 * 0.28 > 0.64
}

/// Deterministically generates one `16m x 64m x 16m` chunk at chunk coordinates `(cx, cz)`.
pub fn generate_chunk(materials: &MaterialTable, seed: u64, cx: i32, cz: i32) -> GeneratedChunk {
    let mut builder = ChunkBuilder::new(materials, cx, cz);
    let l1_x_dim = CHUNK_DIMS_L1[0] as i32;
    let l1_y_dim = CHUNK_DIMS_L1[1] as i32;
    let l1_z_dim = CHUNK_DIMS_L1[2] as i32;
    let sea_l2y = (SEA_LEVEL_M * 4.0).round() as i32;

    // 1. Copy-On-Write L1 Bedrock & deep limestone basement (`y in 0..2m`).
    for lz in 0..CHUNK_DIMS_L1[2] {
        for lx in 0..CHUNK_DIMS_L1[0] {
            builder.set_l1_cow([lx, 0, lz], mat_id::BEDROCK);
            builder.set_l1_cow([lx, 1, lz], mat_id::LIMESTONE);
        }
    }

    // 2. Sculpt terrain, water bodies, snow peaks, and underground caves (`y >= 2.0m`).
    for local_l1z in 0..l1_z_dim {
        for local_l1x in 0..l1_x_dim {
            let l1x = builder.l1_origin_x + local_l1x;
            let l1z = builder.l1_origin_z + local_l1z;
            let mut col_h = [[0.0f32; 4]; 4];
            let mut col_top = [[0i32; 4]; 4];
            let mut max_top_l2y = sea_l2y;
            let mut min_top_l2y = i32::MAX;
            let mut min_h = f32::MAX;
            for dz in 0..4usize {
                for dx in 0..4usize {
                    let wx = (l1x * 4 + dx as i32) as f32 * 0.25 + 0.125;
                    let wz = (l1z * 4 + dz as i32) as f32 * 0.25 + 0.125;
                    let h_m = terrain_height_m(wx, wz, seed);
                    let top_l2y = (h_m * 4.0).round() as i32;
                    col_h[dz][dx] = h_m;
                    col_top[dz][dx] = top_l2y;
                    max_top_l2y = max_top_l2y.max(top_l2y);
                    min_top_l2y = min_top_l2y.min(top_l2y);
                    min_h = min_h.min(h_m);
                }
            }

            let max_l1y = ((max_top_l2y + 3) / 4).min(l1_y_dim);
            for l1y in 2..max_l1y {
                let block_top_l2y = (l1y + 1) * 4;
                if block_top_l2y <= min_top_l2y - 6 && min_h < 10.5 && l1y >= 4 {
                    let c_cave = smooth_noise_3d(
                        l1x as f32 + 0.5,
                        l1y as f32 + 0.5,
                        l1z as f32 + 0.5,
                        seed ^ 0x5151,
                    );
                    if c_cave < 0.45 {
                        builder.set_l1_cow(
                            [local_l1x as u32, l1y as u32, local_l1z as u32],
                            mat_id::SOIL,
                        );
                        continue;
                    }
                }

                let mut block_mats = [mat_id::AIR; 64];
                let mut first_mat = None;
                let mut uniform = true;

                for dz in 0..4usize {
                    for dy in 0..4usize {
                        let l2y = l1y * 4 + dy as i32;
                        let wy = l2y as f32 * 0.25 + 0.125;
                        for dx in 0..4usize {
                            let wx = (l1x * 4 + dx as i32) as f32 * 0.25 + 0.125;
                            let wz = (l1z * 4 + dz as i32) as f32 * 0.25 + 0.125;
                            let top_l2y = col_top[dz][dx];
                            let h_m = col_h[dz][dx];

                            let mat = if l2y < top_l2y {
                                if is_cave_air(wx, wy, wz, h_m, seed) {
                                    if l2y <= 10
                                        && hash3d(
                                            l1x * 4 + dx as i32,
                                            l2y,
                                            l1z * 4 + dz as i32,
                                            seed ^ 0x7777,
                                        ) > 0.92
                                    {
                                        mat_id::ACID
                                    } else {
                                        mat_id::AIR
                                    }
                                } else if l2y + 1 == top_l2y {
                                    if h_m >= 14.5 {
                                        mat_id::CRYO
                                    } else if h_m < SEA_LEVEL_M + 0.35 {
                                        mat_id::SOIL
                                    } else if h_m < 10.5 {
                                        mat_id::GRASS
                                    } else {
                                        mat_id::LIMESTONE
                                    }
                                } else if (h_m >= 10.5 && l2y >= top_l2y - 4) || l2y <= 13 {
                                    mat_id::LIMESTONE
                                } else {
                                    mat_id::SOIL
                                }
                            } else if l2y < sea_l2y && h_m < SEA_LEVEL_M {
                                mat_id::WATER
                            } else {
                                mat_id::AIR
                            };

                            block_mats[dz * 16 + dy * 4 + dx] = mat;
                            match first_mat {
                                None => first_mat = Some(mat),
                                Some(m0) if m0 != mat => uniform = false,
                                _ => {}
                            }
                        }
                    }
                }

                if uniform {
                    if let Some(m0) = first_mat {
                        if m0 != mat_id::AIR {
                            builder
                                .set_l1_cow([local_l1x as u32, l1y as u32, local_l1z as u32], m0);
                        }
                    }
                } else {
                    for dz in 0..4usize {
                        for dy in 0..4usize {
                            for dx in 0..4usize {
                                let mat = block_mats[dz * 16 + dy * 4 + dx];
                                if mat != mat_id::AIR {
                                    builder.set_l2_world(
                                        [
                                            l1x * 4 + dx as i32,
                                            l1y * 4 + dy as i32,
                                            l1z * 4 + dz as i32,
                                        ],
                                        mat,
                                        false,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 3. Place Minecraft-style structures from the weighted TemplatePool per 16m x 16m chunk.
    let pool = build_default_template_pool();
    if (0..=1).contains(&cx) && (0..=1).contains(&cz) {
        if let Some(outpost) = pool.by_id("research_outpost") {
            builder.place_structure(outpost, [52, 16, 60], StructureRot::Rot0);
        }
        if cx == 0 && cz == 0 {
            builder.set_l1_cow([10, 4, 8], mat_id::LIMESTONE);
            builder.set_l1_cow([10, 5, 8], mat_id::ALLOY);
        }
    } else if hash2d(cx, cz, seed ^ 0x8888) >= 0.22 {
        let ox = cx * 16 + 2 + (hash2d(cx, cz, seed ^ 0x9991) * 6.0) as i32;
        let oz = cz * 16 + 2 + (hash2d(cx, cz, seed ^ 0x9992) * 6.0) as i32;
        let surface_m = terrain_height_m(ox as f32 + 0.5, oz as f32 + 0.5, seed);
        let is_water = surface_m < SEA_LEVEL_M;
        let roll = (hash2d(cx, cz, seed ^ 0xAAAA) * 10_000.0) as u32;
        let rot = StructureRot::ALL[(roll as usize) % 4];
        if let Some(tmpl) = pool.pick(surface_m, is_water, roll) {
            let place_y = if is_water {
                sea_l2y - 3
            } else {
                (surface_m * 4.0).round() as i32
            };
            builder.place_structure(tmpl, [ox * 4, place_y, oz * 4], rot);
        }
    }

    builder.finish()
}

/// Generate the initial bootstrap chunk window around `(0, 0)` for `EmergenceRuntime`.
pub fn generate_world(materials: &MaterialTable, seed: u64) -> GeneratedWorld {
    let mut chunks = Vec::new();
    let mut chunk_coords = Vec::new();
    let mut cells = HashMap::new();
    let mut active = Vec::new();

    for cz in BOOTSTRAP_MIN_CHUNK..=BOOTSTRAP_MAX_CHUNK {
        for cx in BOOTSTRAP_MIN_CHUNK..=BOOTSTRAP_MAX_CHUNK {
            let gen = generate_chunk(materials, seed, cx, cz);
            chunks.push(gen.chunk);
            chunk_coords.push(gen.coord);
            cells.extend(gen.cells);
            active.extend(gen.active);
        }
    }

    GeneratedWorld {
        chunks,
        chunk_coords,
        cells,
        active,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_scene::L1_UNIFORM_REF;

    #[test]
    fn world_gen_creates_streaming_chunks_caves_water_and_template_structures() {
        let mats = MaterialTable::compiled_default();
        let gen = generate_world(&mats, 2026);
        assert_eq!(gen.chunks.len(), 16);
        assert!(!gen.cells.is_empty());
        assert!(!gen.active.is_empty());

        let cow_count: usize = gen
            .chunks
            .iter()
            .flat_map(|c| c.l1_brick_ref.iter())
            .filter(|&&r| r == L1_UNIFORM_REF)
            .count();
        assert!(
            cow_count > 1000,
            "chunks must keep interior strata as L1 COW (got {cow_count})"
        );

        // Verify negative chunk coordinates work seamlessly for infinite streaming.
        let neg = generate_chunk(&mats, 2026, -3, -4);
        assert_eq!(neg.chunk.origin_l1, [-48, 0, -64]);

        // Verify mountains and water basins exist across a 256m x 256m sample.
        let mut min_h = f32::MAX;
        let mut max_h = f32::MIN;
        for z in 0..256 {
            for x in 0..256 {
                let h = terrain_height_m(x as f32 + 0.5, z as f32 + 0.5, 2026);
                min_h = min_h.min(h);
                max_h = max_h.max(h);
            }
        }
        assert!(
            min_h < SEA_LEVEL_M,
            "expected water basins below sea level, min_h={min_h}"
        );
        assert!(
            max_h > 20.0,
            "expected mountain ridges above 20.0m, max_h={max_h}"
        );
    }
}
