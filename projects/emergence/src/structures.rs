//! Minecraft-style procedural structure templates and weighted template pools.
//!
//! Replaces hardcoded demo coordinates with reusable [`StructureTemplate`]
//! blueprints (`research_outpost`, `watchtower`, `monolith_shrine`,
//! `timber_cabin`, `oak_tree`, `water_pier`) that support 90-degree rotations,
//! automatic slope foundations, and weighted pool selection.

use prism_scene::VoxelChunk;

use crate::materials::mat_id;

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

/// One voxel operation inside a [`StructureTemplate`].
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

/// Reusable 3D L2/L3 voxel blueprint with optional slope foundation.
#[derive(Debug, Clone)]
pub struct StructureTemplate {
    pub id: &'static str,
    /// Bounding box `[width_l2, height_l2, depth_l2]` in unrotated local space.
    pub size: [i32; 3],
    /// Material used to extend columns downward into sloped terrain.
    pub foundation_mat: Option<u8>,
    pub ops: Vec<StructureOp>,
}

/// Weighted entry in a Minecraft-style [`TemplatePool`].
#[derive(Debug, Clone)]
pub struct PoolEntry {
    pub template: StructureTemplate,
    pub weight: u32,
    pub min_surface_m: f32,
    pub max_surface_m: f32,
    pub requires_water: bool,
}

/// Collection of weighted structure templates for procedural placement.
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
    // Base pad & hollow interior (`36 x 13 x 32` L2 = `9m x 3.25m x 8m`).
    push_box(&mut ops, [0, 0, 0], [36, 1, 32], mat_id::ALLOY, false);
    push_box(&mut ops, [1, 1, 1], [35, 12, 31], mat_id::AIR, false);
    // Limestone perimeter walls.
    push_box(&mut ops, [0, 1, 0], [36, 12, 1], mat_id::LIMESTONE, false);
    push_box(&mut ops, [0, 1, 31], [36, 12, 32], mat_id::LIMESTONE, false);
    push_box(&mut ops, [0, 1, 0], [1, 12, 32], mat_id::LIMESTONE, false);
    push_box(&mut ops, [35, 1, 0], [36, 12, 32], mat_id::LIMESTONE, false);
    // Roof + glass skylight.
    push_box(&mut ops, [0, 12, 0], [36, 13, 32], mat_id::ALLOY, false);
    push_box(&mut ops, [8, 12, 8], [28, 13, 24], mat_id::GLASS, false);
    // Front doorway + L3 steps.
    push_box(&mut ops, [16, 1, 0], [20, 9, 1], mat_id::AIR, false);
    for x in 16..20 {
        ops.push(StructureOp::Micro {
            pos: [x, 1, 0],
            mat: mat_id::ALLOY,
            l3_mask: micro_steps_l3_mask(),
        });
    }
    // Front glass window + L3 grate mullion (`x = 52 + 7 = 59`, `y = 16 + 4 = 20`).
    push_box(&mut ops, [3, 3, 0], [14, 9, 1], mat_id::GLASS, false);
    for y in 3..9 {
        ops.push(StructureOp::Micro {
            pos: [7, y, 0],
            mat: mat_id::ALLOY,
            l3_mask: grate_l3_mask(),
        });
    }
    // Interior Isotope reactor & Cryo-Biomass bay along back corners.
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
    // `12 x 22 x 12` L2 (`3m x 5.5m x 3m`).
    push_box(&mut ops, [0, 0, 0], [12, 1, 12], mat_id::LIMESTONE, false);
    push_box(&mut ops, [1, 1, 1], [11, 16, 11], mat_id::AIR, false);
    push_box(&mut ops, [1, 1, 1], [3, 16, 3], mat_id::ALLOY, false);
    push_box(&mut ops, [9, 1, 1], [11, 16, 3], mat_id::ALLOY, false);
    push_box(&mut ops, [1, 1, 9], [3, 16, 11], mat_id::ALLOY, false);
    push_box(&mut ops, [9, 1, 9], [11, 16, 11], mat_id::ALLOY, false);
    // Observation deck + glass parapet + glowing Isotope beacon.
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
    // Stepped ancient altar `12 x 14 x 12` L2.
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
    // `14 x 12 x 14` L2 (`3.5m x 3m x 3.5m`).
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
    // `12 x 18 x 12` L2 (`3m x 4.5m x 3m`).
    push_box(&mut ops, [5, 0, 5], [7, 10, 7], mat_id::TIMBER, false);
    push_box(&mut ops, [2, 9, 2], [10, 14, 10], mat_id::GRASS, false);
    push_box(&mut ops, [3, 14, 3], [9, 17, 9], mat_id::GRASS, false);
    StructureTemplate {
        id: "oak_tree",
        size: [12, 18, 12],
        foundation_mat: None,
        ops,
    }
}

fn build_water_pier() -> StructureTemplate {
    let mut ops = Vec::new();
    // `16 x 6 x 8` L2 floating timber dock with alloy pilings.
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

/// Builds the default Minecraft-style weighted template pool for Emergence.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_pool_contains_all_blueprints_and_rotates_cleanly() {
        let pool = build_default_template_pool();
        assert!(pool.by_id("research_outpost").is_some());
        assert!(pool.by_id("watchtower").is_some());
        assert!(pool.by_id("monolith_shrine").is_some());
        assert!(pool.by_id("timber_cabin").is_some());
        assert!(pool.by_id("oak_tree").is_some());
        assert!(pool.by_id("water_pier").is_some());

        let tower = pool.by_id("watchtower").unwrap();
        let rot = StructureRot::Rot90;
        let fp = rot.rotated_footprint(tower.size);
        assert_eq!(fp, [tower.size[2], tower.size[0]]);
    }
}
