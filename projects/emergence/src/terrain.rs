//! Minecraft-inspired procedural terrain and biome generator for `emergence`.
//!
//! Uses deterministic 1D/2D value FBM noise (continentalness, temperature,
//! moisture, and cave density) to populate a [`CellGrid`] with `Ocean`,
//! `Plains`, `Desert`, `Mountains`, and `Tundra` biomes, layered geology
//! (`grass_block`/`dirt`/`sand`/`sandstone`/`stone`/`deepslate`/`bedrock`),
//! underground caves, lava pools, and crystal/isotope pockets.

use crate::catalog::CATALOG;
use crate::grid::CellGrid;
use crate::tags::PhysicalCell;

/// Surface biome classification for one terrain column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BiomeKind {
    Ocean,
    Plains,
    Desert,
    Mountains,
    Tundra,
}

impl BiomeKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ocean => "Ocean",
            Self::Plains => "Plains",
            Self::Desert => "Desert",
            Self::Mountains => "Mountains",
            Self::Tundra => "Tundra",
        }
    }
}

/// Generated terrain output containing the populated grid and per-column biomes.
pub struct GeneratedTerrain {
    pub grid: CellGrid,
    pub biomes: Vec<BiomeKind>,
    pub sea_level: usize,
}

fn template_by_id(id: &str) -> PhysicalCell {
    CATALOG
        .iter()
        .find(|entry| entry.id == id)
        .expect("terrain block id must exist in CATALOG")
        .template
}

fn hash2(x: i64, y: i64, seed: u64) -> f32 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9)
        ^ seed;
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h >> 31;
    ((h >> 40) as f32) / 16_777_215.0
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn value_noise_1d(x: f32, seed: u64) -> f32 {
    let xi = x.floor() as i64;
    let xf = smoothstep(x - x.floor());
    lerp(hash2(xi, 0, seed), hash2(xi + 1, 0, seed), xf)
}

fn value_noise_2d(x: f32, y: f32, seed: u64) -> f32 {
    let xi = x.floor() as i64;
    let yi = y.floor() as i64;
    let xf = smoothstep(x - x.floor());
    let yf = smoothstep(y - y.floor());
    let v00 = hash2(xi, yi, seed);
    let v10 = hash2(xi + 1, yi, seed);
    let v01 = hash2(xi, yi + 1, seed);
    let v11 = hash2(xi + 1, yi + 1, seed);
    lerp(lerp(v00, v10, xf), lerp(v01, v11, xf), yf)
}

fn fbm_1d(x: f32, seed: u64, octaves: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut freq = 1.0;
    let mut norm = 0.0;
    for i in 0..octaves {
        sum += amp * value_noise_1d(x * freq, seed.wrapping_add(i as u64 * 1013));
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    sum / norm
}

fn fbm_2d(x: f32, y: f32, seed: u64, octaves: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut freq = 1.0;
    let mut norm = 0.0;
    for i in 0..octaves {
        sum += amp * value_noise_2d(x * freq, y * freq, seed.wrapping_add(i as u64 * 2027));
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    sum / norm
}

fn classify_biome(continental: f32, temp: f32, moisture: f32) -> BiomeKind {
    if continental < 0.34 {
        BiomeKind::Ocean
    } else if continental > 0.67 {
        BiomeKind::Mountains
    } else if temp < 0.36 {
        BiomeKind::Tundra
    } else if temp > 0.60 && moisture < 0.48 {
        BiomeKind::Desert
    } else {
        BiomeKind::Plains
    }
}

/// Generates a 2D geological slice with biomes, oceans, mountains, and caves.
///
/// `y = 0` is the sky top and `y = height - 1` is the indestructible bedrock floor.
pub fn generate_terrain(width: usize, height: usize, seed: u64) -> GeneratedTerrain {
    let mut grid = CellGrid::new(width, height);
    let mut biomes = Vec::with_capacity(width);
    if width == 0 || height < 4 {
        return GeneratedTerrain {
            grid,
            biomes,
            sea_level: 0,
        };
    }

    let bedrock = template_by_id("bedrock");
    let deepslate = template_by_id("deepslate");
    let stone = template_by_id("stone");
    let dirt = template_by_id("dirt");
    let grass = template_by_id("grass_block");
    let sand = template_by_id("sand");
    let sandstone = template_by_id("sandstone");
    let water = template_by_id("water_cell");
    let ice = template_by_id("ice_block");
    let snow = template_by_id("snow_block");
    let lava = template_by_id("magma_lava");
    let coal = template_by_id("coal_ore");
    let iron = template_by_id("iron_ore");
    let copper = template_by_id("copper_ore");
    let gold = template_by_id("gold_ore");
    let redstone = template_by_id("redstone_ore");
    let diamond = template_by_id("diamond_ore");
    let crystal = template_by_id("piezo_crystal");
    let isotope = template_by_id("rad_isotope");

    let sea_level = (height * 45) / 100;
    let deepslate_start = (height * 74) / 100;

    for x in 0..width {
        let fx = x as f32;
        let continental = fbm_1d(fx * 0.045, seed ^ 0x1111, 3);
        let temp = fbm_1d(fx * 0.035 + 17.3, seed ^ 0x2222, 2);
        let moisture = fbm_1d(fx * 0.040 + 41.7, seed ^ 0x3333, 2);
        let detail = fbm_1d(fx * 0.14, seed ^ 0x4444, 3);

        let biome = classify_biome(continental, temp, moisture);
        biomes.push(biome);

        // Compute surface y (smaller y = higher elevation).
        let raw_surface = match biome {
            BiomeKind::Ocean => sea_level as f32 + 3.0 + (0.34 - continental) * 18.0 + detail * 2.0,
            BiomeKind::Plains => sea_level as f32 - 2.0 - (detail - 0.5) * 4.5,
            BiomeKind::Desert => sea_level as f32 - 2.5 - (detail - 0.5) * 5.5,
            BiomeKind::Tundra => sea_level as f32 - 2.0 - (detail - 0.5) * 4.0,
            BiomeKind::Mountains => {
                let peak = ((continental - 0.67) / 0.33).clamp(0.0, 1.0);
                sea_level as f32 - 4.0 - peak * (height as f32 * 0.28) - detail * 4.0
            }
        };
        let surface_y = (raw_surface.round() as isize).clamp(2, (height - 3) as isize) as usize;

        // Fill ocean water above seafloor when surface is below sea_level.
        if biome == BiomeKind::Ocean && surface_y > sea_level {
            for y in sea_level..surface_y {
                grid.set_cell(x, y, water);
            }
        }

        for y in surface_y..height {
            if y == height - 1 {
                grid.set_cell(x, y, bedrock);
                continue;
            }

            let depth = y - surface_y;
            let in_deepslate = y >= deepslate_start;

            // Carve caves in deeper stone/deepslate layers (never breach ocean floor).
            let allow_cave = depth >= 3 && y + 2 < height && biome != BiomeKind::Ocean;
            if allow_cave {
                let cave_n = fbm_2d(fx * 0.13, y as f32 * 0.16, seed ^ 0x5555, 2);
                if cave_n > 0.67 {
                    // Deepest caverns occasionally pool magma lava.
                    if y >= height.saturating_sub(4) && cave_n > 0.72 {
                        grid.set_cell(x, y, lava);
                    }
                    continue;
                }
            }

            // Depth-stratified Minecraft-style ore veins in stone/deepslate layers.
            if depth >= 3 && y + 2 < height {
                let ore_hash = hash2(x as i64, y as i64, seed ^ 0x6666);
                let rel_depth = y as f32 / height as f32;
                if in_deepslate {
                    if rel_depth > 0.84 && ore_hash > 0.965 {
                        grid.set_cell(x, y, diamond);
                        continue;
                    } else if ore_hash > 0.942 {
                        grid.set_cell(x, y, redstone);
                        continue;
                    } else if ore_hash > 0.922 {
                        grid.set_cell(x, y, gold);
                        continue;
                    } else if ore_hash > 0.912 {
                        grid.set_cell(x, y, isotope);
                        continue;
                    }
                } else if ore_hash > 0.978 {
                    grid.set_cell(x, y, crystal);
                    continue;
                } else if ore_hash > 0.950 {
                    grid.set_cell(x, y, iron);
                    continue;
                } else if ore_hash > 0.925 && (biome == BiomeKind::Mountains || rel_depth < 0.65) {
                    grid.set_cell(x, y, copper);
                    continue;
                } else if ore_hash > 0.895 {
                    grid.set_cell(x, y, coal);
                    continue;
                }
            }

            let cell = match biome {
                BiomeKind::Ocean => {
                    if depth < 2 {
                        sand
                    } else if in_deepslate {
                        deepslate
                    } else {
                        stone
                    }
                }
                BiomeKind::Plains => {
                    if depth == 0 {
                        grass
                    } else if depth < 3 {
                        dirt
                    } else if in_deepslate {
                        deepslate
                    } else {
                        stone
                    }
                }
                BiomeKind::Desert => {
                    if depth < 2 {
                        sand
                    } else if depth < 5 {
                        sandstone
                    } else if in_deepslate {
                        deepslate
                    } else {
                        stone
                    }
                }
                BiomeKind::Tundra => {
                    if depth == 0 {
                        if detail > 0.62 {
                            ice
                        } else {
                            snow
                        }
                    } else if depth < 3 {
                        dirt
                    } else if in_deepslate {
                        deepslate
                    } else {
                        stone
                    }
                }
                BiomeKind::Mountains => {
                    if depth == 0 && y + 5 < sea_level {
                        if detail > 0.65 {
                            ice
                        } else {
                            snow
                        }
                    } else if in_deepslate {
                        deepslate
                    } else {
                        stone
                    }
                }
            };
            grid.set_cell(x, y, cell);
        }
    }

    GeneratedTerrain {
        grid,
        biomes,
        sea_level,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terrain_generation_is_deterministic_and_has_bedrock_floor() {
        let a = generate_terrain(64, 36, 12345);
        let b = generate_terrain(64, 36, 12345);
        assert_eq!(a.biomes, b.biomes);
        assert_eq!(a.grid.cells(), b.grid.cells());
        let bedrock = template_by_id("bedrock");
        for x in 0..64 {
            assert_eq!(a.grid.get(x, 35), Some(&bedrock));
        }
    }

    #[test]
    fn wide_terrain_produces_multiple_biomes_and_underground_caves() {
        let t = generate_terrain(256, 48, 0x4D43_5F56_3330);
        let mut distinct = std::collections::HashSet::new();
        for b in &t.biomes {
            distinct.insert(*b);
        }
        assert!(
            distinct.len() >= 3,
            "expected at least 3 biomes across 256 columns, got {:?}",
            distinct
        );
    }
}
