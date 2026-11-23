//! Minecraft-style structure blueprints, weighted template pools, and world-space stamping.
//!
//! Natural terrain is authored at L1 (1 m) granularity, while every blueprint here is stamped at
//! L2 (0.25 m) granularity with optional L3 (0.0625 m) micro detail. Blueprints therefore always
//! read as finer than the ground they stand on, matching the rule: terrain is L1, structures are
//! L1 + L2 + L3, and wires are pure L3.
//!
//! The default pool only *describes* blueprints: procedural terrain never places them.
//! [build_natural_template_pool] holds the vegetation scattered over natural ground, and the spawn
//! courtyard lays out exactly one of each default blueprint by hand (see
//! [crate::world_gen::SPAWN_BLUEPRINTS]).

use prism_scene::{VoxelChunk, L1_UNIFORM_REF};

use crate::materials::{mat_id, MaterialTable};
use crate::tags::TransientFlags;
use crate::world_gen::SparseCellMap;

/// Discrete 90-degree yaw rotation around the Y axis in L2 local space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructureRot {
    Rot0,
    Rot90,
    Rot180,
    Rot270,
}

impl StructureRot {
    pub const ALL: [Self; 4] = [Self::Rot0, Self::Rot90, Self::Rot180, Self::Rot270];

    pub fn rotate_xz(self, pos: [i32; 3], size: [i32; 3]) -> [i32; 3] {
        match self {
            Self::Rot0 => pos,
            Self::Rot90 => [size[2] - 1 - pos[2], pos[1], pos[0]],
            Self::Rot180 => [size[0] - 1 - pos[0], pos[1], size[2] - 1 - pos[2]],
            Self::Rot270 => [pos[2], pos[1], size[0] - 1 - pos[0]],
        }
    }

    pub fn rotated_footprint(self, size: [i32; 3]) -> [i32; 2] {
        match self {
            Self::Rot0 | Self::Rot180 => [size[0], size[2]],
            Self::Rot90 | Self::Rot270 => [size[2], size[0]],
        }
    }
}

/// One voxel operation inside a [StructureTemplate].
#[derive(Debug, Clone, Copy)]
pub enum StructureOp {
    Solid {
        pos: [i32; 3],
        mat: u8,
        active: bool,
    },
    Carve {
        pos: [i32; 3],
    },
    Micro {
        pos: [i32; 3],
        mat: u8,
        l3_mask: u64,
    },
}

impl StructureOp {
    /// Local L2 position of this operation inside the template bounding box.
    pub fn local_pos(self) -> [i32; 3] {
        match self {
            Self::Solid { pos, .. } | Self::Carve { pos } | Self::Micro { pos, .. } => pos,
        }
    }
}

/// Reusable 3D L2/L3 voxel blueprint with an optional levelling foundation.
#[derive(Debug, Clone)]
pub struct StructureTemplate {
    pub id: &'static str,
    /// Bounding box [width_l2, height_l2, depth_l2] in unrotated local space.
    pub size: [i32; 3],
    /// Material used to level the plot and back-fill columns down into sloped terrain.
    pub foundation_mat: Option<u8>,
    pub ops: Vec<StructureOp>,
}

/// Weighted entry in a Minecraft-style [TemplatePool].
#[derive(Debug, Clone)]
pub struct PoolEntry {
    pub template: StructureTemplate,
    pub weight: u32,
    pub min_surface_m: f32,
    pub max_surface_m: f32,
    pub requires_water: bool,
}

/// Collection of weighted structure templates.
pub struct TemplatePool {
    pub entries: Vec<PoolEntry>,
}

impl TemplatePool {
    /// Picks a deterministic structure matching the terrain height / water context.
    pub fn pick(&self, surface_m: f32, is_water: bool, roll: u32) -> Option<&StructureTemplate> {
        let mut total_w = 0u32;
        for e in &self.entries {
            if e.requires_water == is_water
                && surface_m >= e.min_surface_m
                && surface_m <= e.max_surface_m
            {
                total_w += e.weight;
            }
        }
        if total_w == 0 {
            return None;
        }
        let mut target = roll % total_w;
        for e in &self.entries {
            if e.requires_water == is_water
                && surface_m >= e.min_surface_m
                && surface_m <= e.max_surface_m
            {
                if target < e.weight {
                    return Some(&e.template);
                }
                target -= e.weight;
            }
        }
        None
    }

    pub fn by_id(&self, id: &str) -> Option<&StructureTemplate> {
        self.entries
            .iter()
            .find(|e| e.template.id == id)
            .map(|e| &e.template)
    }
}

/// World-space voxel writer shared by natural decoration and blueprint stamping.
///
/// Every write is expressed in world L1/L2 coordinates and silently skipped when it falls outside
/// the target chunk, so one blueprint can be stamped into each chunk it overlaps.
pub struct WorldVoxelWriter<'a> {
    pub chunk: &'a mut VoxelChunk,
    pub cells: &'a mut SparseCellMap,
    pub active: &'a mut Vec<[i32; 3]>,
    pub materials: &'a MaterialTable,
}

impl<'a> WorldVoxelWriter<'a> {
    fn local_l1(&self, l1: [i32; 3]) -> Option<usize> {
        let lx = l1[0] - self.chunk.origin_l1[0];
        let ly = l1[1];
        let lz = l1[2] - self.chunk.origin_l1[2];
        if lx < 0 || ly < 0 || lz < 0 {
            return None;
        }
        self.chunk.l1_index(lx as u32, ly as u32, lz as u32)
    }

    fn local_l2(&self, l2: [i32; 3]) -> Option<[u32; 3]> {
        let lx = l2[0] - self.chunk.origin_l1[0] * 4;
        let ly = l2[1];
        let lz = l2[2] - self.chunk.origin_l1[2] * 4;
        if lx < 0 || ly < 0 || lz < 0 {
            return None;
        }
        let (lx, ly, lz) = (lx as u32, ly as u32, lz as u32);
        if lx >= self.chunk.dims_l1[0] * 4
            || ly >= self.chunk.dims_l1[1] * 4
            || lz >= self.chunk.dims_l1[2] * 4
        {
            return None;
        }
        Some([lx, ly, lz])
    }

    /// Fill one whole L1 (1 m) cell uniformly via Copy-On-Write.
    pub fn write_l1(&mut self, l1: [i32; 3], mat: u8) -> bool {
        if self.local_l1(l1).is_none() {
            return false;
        }
        for dz in 0..4 {
            for dy in 0..4 {
                for dx in 0..4 {
                    self.cells
                        .remove(&[l1[0] * 4 + dx, l1[1] * 4 + dy, l1[2] * 4 + dz]);
                }
            }
        }
        let cell = self.materials.spawn_cell(mat);
        let (rgba, vis) = self.materials.cell_visuals(mat, &cell);
        let lx = (l1[0] - self.chunk.origin_l1[0]) as u32;
        let lz = (l1[2] - self.chunk.origin_l1[2]) as u32;
        self.chunk.set_l1_uniform(lx, l1[1] as u32, lz, rgba, vis)
    }

    /// Stamp one L2 cell (uniform when l3_mask is u64::MAX, micro-shaped otherwise).
    pub fn write_l2(&mut self, l2: [i32; 3], mat: u8, l3_mask: u64, active: bool) -> bool {
        let Some(local) = self.local_l2(l2) else {
            return false;
        };
        if mat == mat_id::AIR || l3_mask == 0 {
            self.cells.remove(&l2);
            self.chunk
                .set_l2_cell(local[0], local[1], local[2], 0, 0, 0);
            return true;
        }
        let mut cell = self.materials.spawn_cell(mat);
        if active {
            cell.transient.remove(TransientFlags::DORMANT);
            self.active.push(l2);
        }
        let (rgba, vis) = self.materials.cell_visuals(mat, &cell);
        self.cells.insert(l2, (mat, cell));
        self.chunk
            .set_l2_cell(local[0], local[1], local[2], rgba, vis, l3_mask)
    }

    /// Base material of a uniform L1 cell (None when empty or split into L2 bricks).
    pub fn l1_uniform_material(&self, l1: [i32; 3]) -> Option<u8> {
        let idx = self.local_l1(l1)?;
        if self.chunk.l1_brick_ref[idx] != L1_UNIFORM_REF {
            return None;
        }
        self.materials
            .find_by_packed_rgba(self.chunk.l1_uniform_rgba[idx])
    }

    /// True when world L2 cell l2 holds any visible voxel.
    pub fn l2_is_solid(&self, l2: [i32; 3]) -> bool {
        let Some(local) = self.local_l2(l2) else {
            return false;
        };
        match self.chunk.get_l2_cell(local[0], local[1], local[2]) {
            Some((rgba, _, _)) => (rgba >> 24) != 0,
            None => false,
        }
    }
}

pub fn grate_l3_mask() -> u64 {
    let mut mask = 0u64;
    for mz in 0..4u32 {
        for my in 0..4u32 {
            for mx in 0..4u32 {
                if (mx == 0 || my == 0) && (mz == 1 || mz == 2) {
                    mask |= 1u64 << VoxelChunk::l3_sub_index(mx, my, mz);
                }
            }
        }
    }
    mask
}

pub fn thin_pane_l3_mask_z() -> u64 {
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

pub fn micro_steps_l3_mask() -> u64 {
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

/// Sparse L3 blade mask for grass tufts and shrubs (1..=max_layers micro-voxels tall).
pub fn tuft_l3_mask(variant: u32, max_layers: u32) -> u64 {
    let mut mask = 0u64;
    for mz in 0..4u32 {
        for mx in 0..4u32 {
            if !(mx + mz + variant).is_multiple_of(3) {
                continue;
            }
            let layers = 1 + ((mx * 7 + mz * 5 + variant) % max_layers.max(1));
            for my in 0..layers.min(4) {
                mask |= 1u64 << VoxelChunk::l3_sub_index(mx, my, mz);
            }
        }
    }
    mask
}

fn push_box(ops: &mut Vec<StructureOp>, min: [i32; 3], max: [i32; 3], mat: u8, active: bool) {
    for z in min[2]..max[2] {
        for y in min[1]..max[1] {
            for x in min[0]..max[0] {
                if mat == mat_id::AIR {
                    ops.push(StructureOp::Carve { pos: [x, y, z] });
                } else {
                    ops.push(StructureOp::Solid {
                        pos: [x, y, z],
                        mat,
                        active,
                    });
                }
            }
        }
    }
}

fn build_research_outpost() -> StructureTemplate {
    let mut ops = Vec::new();
    push_box(&mut ops, [0, 0, 0], [36, 1, 32], mat_id::ALLOY, false);
    push_box(&mut ops, [1, 1, 1], [35, 12, 31], mat_id::AIR, false);
    push_box(&mut ops, [0, 1, 0], [36, 12, 1], mat_id::LIMESTONE, false);
    push_box(&mut ops, [0, 1, 31], [36, 12, 32], mat_id::LIMESTONE, false);
    push_box(&mut ops, [0, 1, 0], [1, 12, 32], mat_id::LIMESTONE, false);
    push_box(&mut ops, [35, 1, 0], [36, 12, 32], mat_id::LIMESTONE, false);
    push_box(&mut ops, [0, 12, 0], [36, 13, 32], mat_id::ALLOY, false);
    push_box(&mut ops, [8, 12, 8], [28, 13, 24], mat_id::GLASS, false);
    push_box(&mut ops, [16, 1, 0], [20, 9, 1], mat_id::AIR, false);
    for x in 16..20 {
        ops.push(StructureOp::Micro {
            pos: [x, 1, 0],
            mat: mat_id::ALLOY,
            l3_mask: micro_steps_l3_mask(),
        });
    }
    push_box(&mut ops, [3, 3, 0], [14, 9, 1], mat_id::GLASS, false);
    for y in 3..9 {
        ops.push(StructureOp::Micro {
            pos: [7, y, 0],
            mat: mat_id::ALLOY,
            l3_mask: grate_l3_mask(),
        });
    }
    push_box(&mut ops, [6, 1, 20], [10, 3, 24], mat_id::ALLOY, false);
    push_box(&mut ops, [7, 3, 21], [9, 5, 23], mat_id::ISOTOPE, true);
    push_box(&mut ops, [24, 1, 18], [30, 3, 25], mat_id::CRYO, false);
    push_box(&mut ops, [26, 3, 20], [28, 5, 22], mat_id::BIOMASS, true);
    StructureTemplate {
        id: "research_outpost",
        size: [36, 13, 32],
        foundation_mat: Some(mat_id::LIMESTONE),
        ops,
    }
}

fn build_watchtower() -> StructureTemplate {
    let mut ops = Vec::new();
    push_box(&mut ops, [0, 0, 0], [12, 1, 12], mat_id::LIMESTONE, false);
    push_box(&mut ops, [1, 1, 1], [11, 16, 11], mat_id::AIR, false);
    push_box(&mut ops, [1, 1, 1], [3, 16, 3], mat_id::ALLOY, false);
    push_box(&mut ops, [9, 1, 1], [11, 16, 3], mat_id::ALLOY, false);
    push_box(&mut ops, [1, 1, 9], [3, 16, 11], mat_id::ALLOY, false);
    push_box(&mut ops, [9, 1, 9], [11, 16, 11], mat_id::ALLOY, false);
    push_box(&mut ops, [0, 16, 0], [12, 17, 12], mat_id::ALLOY, false);
    push_box(&mut ops, [0, 17, 0], [12, 20, 1], mat_id::GLASS, false);
    push_box(&mut ops, [0, 17, 11], [12, 20, 12], mat_id::GLASS, false);
    push_box(&mut ops, [5, 17, 5], [7, 20, 7], mat_id::ISOTOPE, true);
    StructureTemplate {
        id: "watchtower",
        size: [12, 21, 12],
        foundation_mat: Some(mat_id::LIMESTONE),
        ops,
    }
}

fn build_monolith_shrine() -> StructureTemplate {
    let mut ops = Vec::new();
    push_box(&mut ops, [0, 0, 0], [12, 2, 12], mat_id::LIMESTONE, false);
    push_box(&mut ops, [2, 2, 2], [10, 4, 10], mat_id::ALLOY, false);
    push_box(&mut ops, [4, 4, 4], [8, 10, 8], mat_id::CRYO, false);
    push_box(&mut ops, [5, 10, 5], [7, 13, 7], mat_id::ISOTOPE, true);
    for x in 1..11 {
        ops.push(StructureOp::Solid {
            pos: [x, 2, 1],
            mat: mat_id::WIRE,
            active: false,
        });
    }
    StructureTemplate {
        id: "monolith_shrine",
        size: [12, 14, 12],
        foundation_mat: Some(mat_id::LIMESTONE),
        ops,
    }
}

fn build_timber_cabin() -> StructureTemplate {
    let mut ops = Vec::new();
    push_box(&mut ops, [0, 0, 0], [14, 1, 14], mat_id::LIMESTONE, false);
    push_box(&mut ops, [1, 1, 1], [13, 9, 13], mat_id::AIR, false);
    push_box(&mut ops, [0, 1, 0], [14, 9, 1], mat_id::TIMBER, false);
    push_box(&mut ops, [0, 1, 13], [14, 9, 14], mat_id::TIMBER, false);
    push_box(&mut ops, [0, 1, 0], [1, 9, 14], mat_id::TIMBER, false);
    push_box(&mut ops, [13, 1, 0], [14, 9, 14], mat_id::TIMBER, false);
    push_box(&mut ops, [0, 9, 0], [14, 11, 14], mat_id::TIMBER, false);
    push_box(&mut ops, [5, 1, 0], [9, 7, 1], mat_id::AIR, false);
    push_box(&mut ops, [2, 4, 0], [4, 7, 1], mat_id::GLASS, false);
    push_box(&mut ops, [10, 4, 0], [12, 7, 1], mat_id::GLASS, false);
    ops.push(StructureOp::Solid {
        pos: [3, 1, 10],
        mat: mat_id::TANK,
        active: false,
    });
    ops.push(StructureOp::Solid {
        pos: [10, 1, 10],
        mat: mat_id::VALVE,
        active: false,
    });
    StructureTemplate {
        id: "timber_cabin",
        size: [14, 11, 14],
        foundation_mat: Some(mat_id::LIMESTONE),
        ops,
    }
}

fn build_oak_tree() -> StructureTemplate {
    let mut ops = Vec::new();
    push_box(&mut ops, [5, 0, 5], [7, 10, 7], mat_id::TIMBER, false);
    push_box(&mut ops, [2, 9, 2], [10, 14, 10], mat_id::GRASS, false);
    push_box(&mut ops, [3, 14, 3], [9, 17, 9], mat_id::GRASS, false);
    for (x, z) in [(1, 1), (1, 10), (10, 1), (10, 10)] {
        ops.push(StructureOp::Micro {
            pos: [x, 11, z],
            mat: mat_id::GRASS,
            l3_mask: tuft_l3_mask((x + z) as u32, 4),
        });
    }
    StructureTemplate {
        id: "oak_tree",
        size: [12, 18, 12],
        foundation_mat: None,
        ops,
    }
}

fn build_spruce_pine() -> StructureTemplate {
    let mut ops = Vec::new();
    push_box(&mut ops, [4, 0, 4], [6, 16, 6], mat_id::TIMBER, false);
    for tier in 0..5i32 {
        let base_y = 5 + tier * 3;
        let inset = 4 - tier;
        push_box(
            &mut ops,
            [inset, base_y, inset],
            [10 - inset, base_y + 1, 10 - inset],
            mat_id::GRASS,
            false,
        );
        ops.push(StructureOp::Micro {
            pos: [5, base_y, inset],
            mat: mat_id::GRASS,
            l3_mask: tuft_l3_mask(tier as u32, 3),
        });
    }
    ops.push(StructureOp::Solid {
        pos: [5, 20, 5],
        mat: mat_id::GRASS,
        active: false,
    });
    StructureTemplate {
        id: "spruce_pine",
        size: [10, 22, 10],
        foundation_mat: None,
        ops,
    }
}

fn build_shrub() -> StructureTemplate {
    let mut ops = Vec::new();
    for z in 0..4i32 {
        for x in 0..4i32 {
            ops.push(StructureOp::Micro {
                pos: [x, 0, z],
                mat: mat_id::GRASS,
                l3_mask: tuft_l3_mask((x * 3 + z) as u32, 4),
            });
        }
    }
    ops.push(StructureOp::Micro {
        pos: [1, 1, 1],
        mat: mat_id::GRASS,
        l3_mask: tuft_l3_mask(7, 4),
    });
    StructureTemplate {
        id: "shrub",
        size: [4, 3, 4],
        foundation_mat: None,
        ops,
    }
}

fn build_water_pier() -> StructureTemplate {
    let mut ops = Vec::new();
    push_box(&mut ops, [0, 3, 0], [16, 4, 8], mat_id::TIMBER, false);
    push_box(&mut ops, [1, 0, 1], [3, 4, 3], mat_id::ALLOY, false);
    push_box(&mut ops, [13, 0, 1], [15, 4, 3], mat_id::ALLOY, false);
    push_box(&mut ops, [1, 0, 5], [3, 4, 7], mat_id::ALLOY, false);
    push_box(&mut ops, [13, 0, 5], [15, 4, 7], mat_id::ALLOY, false);
    ops.push(StructureOp::Solid {
        pos: [7, 4, 3],
        mat: mat_id::TANK,
        active: false,
    });
    StructureTemplate {
        id: "water_pier",
        size: [16, 6, 8],
        foundation_mat: None,
        ops,
    }
}

/// Every blueprint a player can inspect next to the spawn point (one of each).
pub fn build_default_template_pool() -> TemplatePool {
    TemplatePool {
        entries: vec![
            PoolEntry {
                template: build_research_outpost(),
                weight: 2,
                min_surface_m: 3.5,
                max_surface_m: 11.0,
                requires_water: false,
            },
            PoolEntry {
                template: build_watchtower(),
                weight: 3,
                min_surface_m: 4.5,
                max_surface_m: 14.5,
                requires_water: false,
            },
            PoolEntry {
                template: build_monolith_shrine(),
                weight: 3,
                min_surface_m: 3.8,
                max_surface_m: 16.0,
                requires_water: false,
            },
            PoolEntry {
                template: build_timber_cabin(),
                weight: 4,
                min_surface_m: 4.2,
                max_surface_m: 10.5,
                requires_water: false,
            },
            PoolEntry {
                template: build_oak_tree(),
                weight: 10,
                min_surface_m: 5.1,
                max_surface_m: 11.5,
                requires_water: false,
            },
            PoolEntry {
                template: build_water_pier(),
                weight: 4,
                min_surface_m: 2.0,
                max_surface_m: 5.0,
                requires_water: true,
            },
        ],
    }
}

/// Vegetation blueprints scattered over natural ground by the terrain generator.
pub fn build_natural_template_pool() -> TemplatePool {
    TemplatePool {
        entries: vec![
            PoolEntry {
                template: build_oak_tree(),
                weight: 6,
                min_surface_m: 5.1,
                max_surface_m: 11.5,
                requires_water: false,
            },
            PoolEntry {
                template: build_spruce_pine(),
                weight: 4,
                min_surface_m: 4.2,
                max_surface_m: 12.5,
                requires_water: false,
            },
            PoolEntry {
                template: build_shrub(),
                weight: 8,
                min_surface_m: 4.5,
                max_surface_m: 11.5,
                requires_water: false,
            },
        ],
    }
}

/// Stamp a template into a writer at world L2 origin with yaw rot.
///
/// Templates with a foundation first level their plot: the blueprint volume is carved, then every
/// footprint column is filled with the foundation material from the natural surface up to the
/// plot level, so a building never floats over a slope. Natural L1 ground underneath is never
/// split into L2 bricks. Vegetation skips this and simply sits on the surface.
pub fn stamp_structure(
    writer: &mut WorldVoxelWriter<'_>,
    template: &StructureTemplate,
    origin: [i32; 3],
    rot: StructureRot,
) {
    let footprint = rot.rotated_footprint(template.size);
    let lowest = template
        .ops
        .iter()
        .map(|op| op.local_pos()[1])
        .min()
        .unwrap_or(0);
    if let Some(foundation_mat) = template.foundation_mat {
        for dz in 0..footprint[1] {
            for dx in 0..footprint[0] {
                for dy in lowest..template.size[1] {
                    writer.write_l2(
                        [origin[0] + dx, origin[1] + dy, origin[2] + dz],
                        mat_id::AIR,
                        0,
                        false,
                    );
                }
                let mut fy = origin[1] + lowest - 1;
                while fy >= 4 && !writer.l2_is_solid([origin[0] + dx, fy, origin[2] + dz]) {
                    if !writer.write_l2(
                        [origin[0] + dx, fy, origin[2] + dz],
                        foundation_mat,
                        u64::MAX,
                        false,
                    ) {
                        break;
                    }
                    fy -= 1;
                }
            }
        }
    }

    for op in &template.ops {
        let r = rot.rotate_xz(op.local_pos(), template.size);
        let world = [origin[0] + r[0], origin[1] + r[1], origin[2] + r[2]];
        match *op {
            StructureOp::Solid { mat, active, .. } => {
                writer.write_l2(world, mat, u64::MAX, active);
            }
            StructureOp::Carve { .. } => {
                writer.write_l2(world, mat_id::AIR, 0, false);
            }
            StructureOp::Micro { mat, l3_mask, .. } => {
                writer.write_l2(world, mat, l3_mask, false);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_pools_contain_all_blueprints_and_rotate_cleanly() {
        let pool = build_default_template_pool();
        assert!(pool.by_id("research_outpost").is_some());
        assert!(pool.by_id("watchtower").is_some());
        assert!(pool.by_id("monolith_shrine").is_some());
        assert!(pool.by_id("timber_cabin").is_some());
        assert!(pool.by_id("oak_tree").is_some());
        assert!(pool.by_id("water_pier").is_some());

        let natural = build_natural_template_pool();
        assert!(natural.by_id("oak_tree").is_some());
        assert!(natural.by_id("spruce_pine").is_some());
        assert!(natural.by_id("shrub").is_some());

        let tower = pool.by_id("watchtower").unwrap();
        let rot = StructureRot::Rot90;
        let fp = rot.rotated_footprint(tower.size);
        assert_eq!(fp, [tower.size[2], tower.size[0]]);
    }

    #[test]
    fn stamping_builds_l2_bricks_with_l3_detail() {
        let mats = MaterialTable::compiled_default();
        let mut chunk = VoxelChunk::new([0, 0, 0], [16, 64, 16]);
        let mut cells = SparseCellMap::new();
        let mut active = Vec::new();
        let pool = build_default_template_pool();
        let outpost = pool.by_id("research_outpost").unwrap();
        let mut writer = WorldVoxelWriter {
            chunk: &mut chunk,
            cells: &mut cells,
            active: &mut active,
            materials: &mats,
        };
        stamp_structure(&mut writer, outpost, [8, 16, 8], StructureRot::Rot0);

        let l1_idx = chunk.l1_index(2, 4, 2).expect("in bounds");
        assert_ne!(chunk.l1_brick_ref[l1_idx], L1_UNIFORM_REF);
        assert_ne!(chunk.l1_brick_ref[l1_idx], 0);

        let mullion = chunk.get_l2_cell(8 + 7, 16 + 4, 8).expect("mullion cell");
        assert_ne!(mullion.2, u64::MAX, "L3 grate must stay a partial mask");
        assert_ne!(mullion.2, 0);
    }
}
