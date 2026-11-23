//! Deterministic per-chunk procedural natural terrain, caves, water bodies, mountains, and
//! vegetation decoration.
//!
//! Terrain is authored at L1 (1 m) granularity so flat ground collapses into a single
//! Copy-On-Write cell. L2/L3 detail belongs to man-made blueprints, which procedural generation
//! never places: the only blueprints in the world are the ones hand-placed around the spawn
//! courtyard (see `SPAWN_BLUEPRINTS`), one of each type.
//!
//! Supports both single-chunk streaming (`generate_chunk(materials, seed, cx, cz)`) across
//! infinite signed chunk coordinates and initial multi-chunk bootstrap (`generate_world`).

use std::collections::HashMap;

use prism_scene::VoxelChunk;

use crate::materials::{mat_id, MaterialTable};
use crate::sim::CHUNK_DIMS_L1;
use crate::structures::{
    build_default_template_pool, build_natural_template_pool, stamp_structure, StructureRot,
    WorldVoxelWriter,
};
use crate::tags::PhysicalCell;

/// Sea level in meters (`l2y = 14`).
pub const SEA_LEVEL_M: f32 = 3.5;

/// Initial bootstrap chunk window around `(0, 0)` (`4 x 4 = 16` chunks = `64m x 64m x 64m`).
pub const BOOTSTRAP_MIN_CHUNK: i32 = -1;
pub const BOOTSTRAP_MAX_CHUNK: i32 = 2;

/// Player spawn point in meters, on the flat spawn courtyard.
pub const SPAWN_POINT_M: [f32; 2] = [6.5, 6.5];

/// Flat spawn courtyard bounds in meters; kept clear of vegetation and natural relief.
pub const COURTYARD_MIN_M: [f32; 2] = [4.0, 6.0];
pub const COURTYARD_MAX_M: [f32; 2] = [26.0, 23.0];

/// Hand-placed blueprints around the spawn point: exactly one of every default template.
/// Entries are `(template id, world L2 origin, yaw)`, authored by hand rather than generated.
pub const SPAWN_BLUEPRINTS: [(&str, [i32; 3], StructureRot); 6] = [
    ("research_outpost", [48, 16, 52], StructureRot::Rot0),
    ("water_pier", [68, 11, 32], StructureRot::Rot0),
    ("watchtower", [88, 16, 76], StructureRot::Rot0),
    ("monolith_shrine", [88, 16, 28], StructureRot::Rot0),
    ("timber_cabin", [28, 16, 68], StructureRot::Rot0),
    ("oak_tree", [24, 16, 36], StructureRot::Rot0),
];

/// Spawn pond in meters: the courtyard basin that carries the hand-placed water pier.
const POND_MIN_M: [i32; 2] = [17, 8];
const POND_MAX_M: [i32; 2] = [22, 11];
/// Pond floor (`2.0 m`), carved out of the `4.0 m` courtyard plateau.
const POND_FLOOR_L1: i32 = 2;

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
        chunk_writer(self).write_l2(l2, mat, u64::MAX, active);
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
///
/// Every column is quantized to whole meters so natural ground always fills complete L1 cells and
/// stays Copy-On-Write; L2 relief is never needed for terrain.
pub fn terrain_height_m(wx: f32, wz: f32, seed: u64) -> f32 {
    let continental = smooth_noise(wx * 0.016, wz * 0.016, seed ^ 0x1010);
    let ridge_raw = 1.0 - (smooth_noise(wx * 0.028, wz * 0.028, seed ^ 0x2020) * 2.0 - 1.0).abs();
    let ridge = ridge_raw * ridge_raw;
    let detail = smooth_noise(wx * 0.08, wz * 0.08, seed ^ 0x3030) * 2.6
        + smooth_noise(wx * 0.22, wz * 0.22, seed ^ 0x4040) * 0.8;

    let mountain_boost = ((continental - 0.54).max(0.0) / 0.46).powf(1.35) * 42.0 * ridge;
    let basin_dip = ((0.38 - continental).max(0.0) / 0.38) * 3.6;
    let mut h = 4.6 + (continental - 0.45) * 6.0 + detail + mountain_boost - basin_dip;

    // Gentle starter courtyard plateau at 4.0 m covering the whole spawn plot, so the player, the
    // camera and every hand-placed blueprint sit on one flat L1 surface.
    let center_x = (COURTYARD_MIN_M[0] + COURTYARD_MAX_M[0]) * 0.5;
    let center_z = (COURTYARD_MIN_M[1] + COURTYARD_MAX_M[1]) * 0.5;
    let half_x = (COURTYARD_MAX_M[0] - COURTYARD_MIN_M[0]) * 0.5;
    let half_z = (COURTYARD_MAX_M[1] - COURTYARD_MIN_M[1]) * 0.5;
    let dx = ((wx - center_x).abs() - half_x).max(0.0) / 4.5;
    let dz = ((wz - center_z).abs() - half_z).max(0.0) / 4.5;
    let blend = dx.max(dz).clamp(0.0, 1.0);
    h = 4.0 + (h - 4.0) * (blend * blend);

    h.clamp(1.6, 52.0).round()
}

fn is_cave_air(wx: f32, wy: f32, wz: f32, surface_m: f32, seed: u64) -> bool {
    if wy < 2.2 || wy > surface_m - 1.5 {
        return false;
    }
    let n1 = smooth_noise_3d(wx * 0.11, wy * 0.16, wz * 0.11, seed ^ 0x5151);
    let n2 = smooth_noise_3d(wx * 0.22, wy * 0.26, wz * 0.22, seed ^ 0x6262);
    n1 * 0.72 + n2 * 0.28 > 0.64
}

/// Surface material of a natural column whose top sits at `h_m` meters.
pub fn surface_material(h_m: f32) -> u8 {
    if h_m >= 14.5 {
        mat_id::CRYO
    } else if h_m < SEA_LEVEL_M + 0.35 {
        mat_id::SOIL
    } else if h_m < 10.5 {
        mat_id::GRASS
    } else {
        mat_id::LIMESTONE
    }
}

/// Deterministically generates one `16m x 64m x 16m` chunk at chunk coordinates `(cx, cz)`.
pub fn generate_chunk(materials: &MaterialTable, seed: u64, cx: i32, cz: i32) -> GeneratedChunk {
    let mut builder = ChunkBuilder::new(materials, cx, cz);
    let l1_x_dim = CHUNK_DIMS_L1[0] as i32;
    let l1_y_dim = CHUNK_DIMS_L1[1] as i32;
    let l1_z_dim = CHUNK_DIMS_L1[2] as i32;
    let sea_l2y = (SEA_LEVEL_M * 4.0).round() as i32;

    // 1. Copy-On-Write L1 bedrock & deep limestone basement (`y in 0..2m`).
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
                // Flat 1 m surface block (`top_l2y == block_top_l2y` across all 16 columns): emit pure L1 COW.
                if min_top_l2y == max_top_l2y
                    && min_top_l2y == block_top_l2y
                    && min_h >= SEA_LEVEL_M
                {
                    let c_cave = smooth_noise_3d(
                        l1x as f32 + 0.5,
                        l1y as f32 + 0.5,
                        l1z as f32 + 0.5,
                        seed ^ 0x5151,
                    );
                    if c_cave < 0.45 {
                        builder.set_l1_cow(
                            [local_l1x as u32, l1y as u32, local_l1z as u32],
                            surface_material(min_h),
                        );
                        continue;
                    }
                }
                if block_top_l2y <= min_top_l2y - 6 && l1y >= 2 {
                    let c_cave = smooth_noise_3d(
                        l1x as f32 + 0.5,
                        l1y as f32 + 0.5,
                        l1z as f32 + 0.5,
                        seed ^ 0x5151,
                    );
                    if c_cave < 0.45 {
                        let mat = if min_h >= 10.5 || l1y <= 3 {
                            mat_id::LIMESTONE
                        } else {
                            mat_id::SOIL
                        };
                        builder.set_l1_cow([local_l1x as u32, l1y as u32, local_l1z as u32], mat);
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
                                    surface_material(h_m)
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

    // 3. Spawn courtyard: flatten, carve the pond, then hand-place one of every blueprint.
    stamp_spawn_courtyard(&mut builder);
    // 4. Scatter natural vegetation (trees, pines, shrubs) over untouched ground.
    stamp_natural_vegetation(&mut builder, seed, cx, cz);

    builder.finish()
}

fn chunk_writer<'a>(builder: &'a mut ChunkBuilder<'_>) -> WorldVoxelWriter<'a> {
    WorldVoxelWriter {
        chunk: &mut builder.chunk,
        cells: &mut builder.cells,
        active: &mut builder.active,
        materials: builder.materials,
    }
}

fn rect_intersects_chunk(min_m: [f32; 2], max_m: [f32; 2], cx: i32, cz: i32) -> bool {
    let chunk_min = [cx as f32 * 16.0, cz as f32 * 16.0];
    let chunk_max = [chunk_min[0] + 16.0, chunk_min[1] + 16.0];
    min_m[0] < chunk_max[0]
        && max_m[0] > chunk_min[0]
        && min_m[1] < chunk_max[1]
        && max_m[1] > chunk_min[1]
}

fn in_courtyard(wx: f32, wz: f32) -> bool {
    (COURTYARD_MIN_M[0]..COURTYARD_MAX_M[0]).contains(&wx)
        && (COURTYARD_MIN_M[1]..COURTYARD_MAX_M[1]).contains(&wz)
}

/// Flatten the spawn courtyard, carve its pond, and hand-place every default blueprint once.
fn stamp_spawn_courtyard(builder: &mut ChunkBuilder<'_>) {
    if !rect_intersects_chunk(COURTYARD_MIN_M, COURTYARD_MAX_M, builder.cx, builder.cz) {
        return;
    }
    {
        let mut writer = chunk_writer(builder);
        // Clear the courtyard volume above the 4.0 m plateau so buildings and the pond read cleanly.
        let x0 = COURTYARD_MIN_M[0] as i32;
        let z0 = COURTYARD_MIN_M[1] as i32;
        let x1 = COURTYARD_MAX_M[0] as i32;
        let z1 = COURTYARD_MAX_M[1] as i32;
        for l1z in z0..z1 {
            for l1x in x0..x1 {
                for l1y in 4..20 {
                    writer.write_l1([l1x, l1y, l1z], mat_id::AIR);
                }
            }
        }
        // Carve the pond basin and fill it with water up to sea level.
        for l1z in POND_MIN_M[1]..POND_MAX_M[1] {
            for l1x in POND_MIN_M[0]..POND_MAX_M[0] {
                for l1y in POND_FLOOR_L1..4 {
                    writer.write_l1([l1x, l1y, l1z], mat_id::AIR);
                }
                for l2y in POND_FLOOR_L1 * 4..(SEA_LEVEL_M * 4.0).round() as i32 {
                    for dz in 0..4 {
                        for dx in 0..4 {
                            writer.write_l2(
                                [l1x * 4 + dx, l2y, l1z * 4 + dz],
                                mat_id::WATER,
                                u64::MAX,
                                false,
                            );
                        }
                    }
                }
            }
        }
    }

    let pool = build_default_template_pool();
    for (id, origin, rot) in SPAWN_BLUEPRINTS {
        let Some(template) = pool.by_id(id) else {
            continue;
        };
        let mut writer = chunk_writer(builder);
        stamp_structure(&mut writer, template, origin, rot);
    }
}

/// Deterministic vegetation pass: trees, pines and shrubs scattered over natural grass.
///
/// Candidate positions are derived per chunk and re-evaluated by the three neighbouring chunks so
/// a canopy that crosses a chunk border is stamped completely on both sides.
fn stamp_natural_vegetation(builder: &mut ChunkBuilder<'_>, seed: u64, cx: i32, cz: i32) {
    let pool = build_natural_template_pool();
    let mut writer = chunk_writer(builder);
    for dcz in -1..=1 {
        for dcx in -1..=1 {
            let ncx = cx + dcx;
            let ncz = cz + dcz;
            for slot in 0..3i32 {
                let jitter_x = hash2d(ncx * 4 + slot, ncz * 8 - slot, seed ^ 0x9191);
                let jitter_z = hash2d(ncx * 8 + slot, ncz * 4 + slot, seed ^ 0x9292);
                let density = hash2d(ncx * 16 + slot, ncz * 16 - slot, seed ^ 0x9393);
                if density >= 0.42 {
                    continue;
                }
                let wx = ncx * 16 + 2 + (jitter_x * 12.0) as i32;
                let wz = ncz * 16 + 2 + (jitter_z * 12.0) as i32;
                let surface_m = terrain_height_m(wx as f32 + 0.5, wz as f32 + 0.5, seed);
                if surface_m < SEA_LEVEL_M + 0.5 || surface_m >= 10.5 {
                    continue;
                }
                if in_courtyard(wx as f32, wz as f32) {
                    continue;
                }
                let roll =
                    (hash2d(ncx * 32 - slot, ncz * 32 + slot, seed ^ 0x9494) * 10_000.0) as u32;
                let Some(template) = pool.pick(surface_m, false, roll) else {
                    continue;
                };
                let origin = [
                    wx * 4 - template.size[0] / 2,
                    (surface_m * 4.0).round() as i32,
                    wz * 4 - template.size[2] / 2,
                ];
                stamp_structure(&mut writer, template, origin, StructureRot::Rot0);
            }
        }
    }
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
    fn world_gen_keeps_terrain_l1_and_places_each_blueprint_once() {
        let mats = MaterialTable::compiled_default();
        let gen = generate_world(&mats, 2026);
        assert_eq!(gen.chunks.len(), 16);
        assert!(!gen.cells.is_empty());

        let cow_count: usize = gen
            .chunks
            .iter()
            .flat_map(|c| c.l1_brick_ref.iter())
            .filter(|&&r| r == L1_UNIFORM_REF)
            .count();
        assert!(
            cow_count > 2000,
            "natural terrain must stay L1 Copy-On-Write (got {cow_count})"
        );

        // Negative chunk coordinates stream seamlessly for infinite worlds.
        let neg = generate_chunk(&mats, 2026, -3, -4);
        assert_eq!(neg.chunk.origin_l1, [-48, 0, -64]);

        // Mountains and water basins exist across a 256 m x 256 m sample.
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
            "expected mountain ridges above 20.0 m, max_h={max_h}"
        );
    }

    #[test]
    fn procedural_terrain_outside_the_courtyard_has_no_man_made_materials() {
        let mats = MaterialTable::compiled_default();
        let gen = generate_world(&mats, 2026);
        let mut man_made = 0;
        for (l2, (mat, _)) in &gen.cells {
            let wx = l2[0] as f32 * 0.25;
            let wz = l2[2] as f32 * 0.25;
            if in_courtyard(wx, wz) {
                continue;
            }
            if matches!(
                *mat,
                mat_id::ALLOY
                    | mat_id::GLASS
                    | mat_id::ISOTOPE
                    | mat_id::BIOMASS
                    | mat_id::TANK
                    | mat_id::WIRE
                    | mat_id::VALVE
                    | mat_id::CONDUIT
            ) {
                man_made += 1;
            }
        }
        assert_eq!(
            man_made, 0,
            "procedural terrain must stay natural; found {man_made} artificial voxels"
        );
    }

    #[test]
    fn spawn_courtyard_stamps_every_blueprint() {
        let mats = MaterialTable::compiled_default();
        let gen = generate_world(&mats, 2026);
        let pool = build_default_template_pool();
        for (id, origin, rot) in SPAWN_BLUEPRINTS {
            let template = pool.by_id(id).expect("blueprint exists");
            let footprint = rot.rotated_footprint(template.size);
            let mut found = false;
            for dz in 0..footprint[1] {
                for dx in 0..footprint[0] {
                    for dy in 0..template.size[1] {
                        let l2 = [origin[0] + dx, origin[1] + dy, origin[2] + dz];
                        if gen.cells.contains_key(&l2) {
                            found = true;
                        }
                    }
                }
            }
            assert!(found, "spawn blueprint {id} left no L2 voxels behind");
        }
    }
}
