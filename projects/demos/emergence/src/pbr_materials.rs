//! Per-block PBR material definitions for `emergence`, modeled after
//! Minecraft LabPBR 1.3 / shaderpack (BSL / Complementary / SEUS) conventions.
//!
//! Each of the 16 `CATALOG` entries maps 1-to-1 to a [`BlockPbrMaterial`]
//! whose fields align with Prisma's `MaterialDef` / `GpuMaterial` layout
//! (`base_color`, `metallic`, `roughness`, `emissive`, `transmission`, `ior`,
//! `translucency`, `anisotropy`, `clearcoat`, `clearcoat_roughness`).

use crate::catalog::CATALOG;
use crate::tags::{PhysicalCell, TransientFlags};

/// Blend / transparency mode matching `GpuMaterial::alpha_mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlphaMode {
    Opaque = 0,
    Mask = 1,
    Blend = 2,
}

/// Complete PBR + LabPBR-inspired material specification for one block type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockPbrMaterial {
    pub id: &'static str,
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: [f32; 3],
    pub emissive_strength: f32,
    pub transmission: f32,
    pub ior: f32,
    pub translucency: f32,
    pub anisotropy: f32,
    pub clearcoat: f32,
    pub clearcoat_roughness: f32,
    /// LabPBR porosity `[0, 1]`: controls albedo darkening and puddle gloss when `WET`.
    pub porosity: f32,
    pub alpha_mode: AlphaMode,
}

impl BlockPbrMaterial {
    const fn opaque(id: &'static str, base_color: [f32; 4], metallic: f32, roughness: f32) -> Self {
        Self {
            id,
            base_color,
            metallic,
            roughness,
            emissive: [0.0, 0.0, 0.0],
            emissive_strength: 0.0,
            transmission: 0.0,
            ior: 1.5,
            translucency: 0.0,
            anisotropy: 0.0,
            clearcoat: 0.0,
            clearcoat_roughness: 0.0,
            porosity: 0.0,
            alpha_mode: AlphaMode::Opaque,
        }
    }
}

/// Per-block PBR materials for all 16 `CATALOG` entries (order matches `CATALOG`).
pub const BLOCK_PBR_MATERIALS: &[BlockPbrMaterial] = &[
    // 1. Conductive Wire — LabPBR Copper Block (drawn metallic wire).
    BlockPbrMaterial {
        anisotropy: 0.45,
        ..BlockPbrMaterial::opaque("conductive_wire", [0.82, 0.48, 0.25, 1.0], 1.0, 0.24)
    },
    // 2. Superconductor Coil — Cryo-cooled metallic coil with subtle cyan glow.
    BlockPbrMaterial {
        emissive: [0.10, 0.65, 0.95],
        emissive_strength: 0.45,
        clearcoat: 0.35,
        clearcoat_roughness: 0.08,
        ..BlockPbrMaterial::opaque("superconductor", [0.25, 0.80, 0.94, 1.0], 0.92, 0.14)
    },
    // 3. Water — BSL/Complementary-style refractive liquid (IOR 1.333).
    BlockPbrMaterial {
        transmission: 0.88,
        ior: 1.333,
        clearcoat: 0.50,
        clearcoat_roughness: 0.02,
        alpha_mode: AlphaMode::Blend,
        ..BlockPbrMaterial::opaque("water_cell", [0.12, 0.42, 0.82, 0.72], 0.0, 0.03)
    },
    // 4. Reinforced Glass — LabPBR crown glass (IOR 1.52, crisp specular).
    BlockPbrMaterial {
        transmission: 0.92,
        ior: 1.52,
        clearcoat: 0.60,
        clearcoat_roughness: 0.03,
        alpha_mode: AlphaMode::Blend,
        ..BlockPbrMaterial::opaque("glass_wall", [0.72, 0.90, 0.96, 0.42], 0.0, 0.04)
    },
    // 5. Insulated Bulkhead — Composite ceramic-steel bulkhead.
    BlockPbrMaterial {
        porosity: 0.15,
        ..BlockPbrMaterial::opaque("insulated_wall", [0.42, 0.46, 0.52, 1.0], 0.35, 0.68)
    },
    // 6. Volatile Fuel Drum — Industrial enamel-coated steel drum.
    BlockPbrMaterial {
        clearcoat: 0.45,
        clearcoat_roughness: 0.18,
        ..BlockPbrMaterial::opaque("volatile_fuel", [0.78, 0.22, 0.14, 1.0], 0.65, 0.38)
    },
    // 7. Absorbent Foam — High-porosity LabPBR sponge (darkens & glosses when wet).
    BlockPbrMaterial {
        translucency: 0.20,
        porosity: 0.95,
        ..BlockPbrMaterial::opaque("absorbent_sponge", [0.86, 0.78, 0.38, 1.0], 0.0, 0.92)
    },
    // 8. Corrosive Acid — Bioluminescent/caustic refractive pool.
    BlockPbrMaterial {
        emissive: [0.32, 0.95, 0.12],
        emissive_strength: 1.35,
        transmission: 0.65,
        ior: 1.36,
        translucency: 0.55,
        alpha_mode: AlphaMode::Blend,
        ..BlockPbrMaterial::opaque("acid_pool", [0.38, 0.92, 0.18, 0.82], 0.0, 0.08)
    },
    // 9. Cryo-Spore Specimen — LabPBR packed-ice + subsurface glow lichen.
    BlockPbrMaterial {
        emissive: [0.20, 0.65, 0.95],
        emissive_strength: 0.65,
        ior: 1.31,
        translucency: 0.75,
        clearcoat: 0.70,
        clearcoat_roughness: 0.10,
        ..BlockPbrMaterial::opaque("cryo_spore", [0.55, 0.86, 0.98, 1.0], 0.0, 0.22)
    },
    // 10. Hyper-Neo Mutant — Wet biofilm organic SSS + bioluminescence.
    BlockPbrMaterial {
        emissive: [0.28, 0.95, 0.22],
        emissive_strength: 1.60,
        translucency: 0.65,
        clearcoat: 0.55,
        clearcoat_roughness: 0.14,
        ..BlockPbrMaterial::opaque("hyper_neo_core", [0.36, 0.85, 0.30, 1.0], 0.0, 0.28)
    },
    // 11. Piezo Crystal Node — LabPBR Amethyst / Quartz (IOR 1.544).
    BlockPbrMaterial {
        emissive: [0.58, 0.28, 0.95],
        emissive_strength: 1.10,
        transmission: 0.55,
        ior: 1.544,
        clearcoat: 0.40,
        clearcoat_roughness: 0.05,
        alpha_mode: AlphaMode::Blend,
        ..BlockPbrMaterial::opaque("piezo_crystal", [0.72, 0.45, 0.95, 0.92], 0.0, 0.10)
    },
    // 12. Pressurized Valve Conduit — Brushed brass/steel industrial pipework.
    BlockPbrMaterial {
        anisotropy: 0.30,
        ..BlockPbrMaterial::opaque("valve_conduit", [0.65, 0.58, 0.35, 1.0], 0.90, 0.32)
    },
    // 13. Overclocked Conveyor — Anisotropic rubber-steel track with amber chevrons.
    BlockPbrMaterial {
        emissive: [0.95, 0.45, 0.08],
        emissive_strength: 0.40,
        anisotropy: 0.65,
        ..BlockPbrMaterial::opaque("conveyor_east", [0.22, 0.24, 0.28, 1.0], 0.45, 0.52)
    },
    // 14. Radioactive Isotope — Heavy actinide metal with strong Cherenkov glow.
    BlockPbrMaterial {
        emissive: [0.45, 1.00, 0.15],
        emissive_strength: 3.20,
        ..BlockPbrMaterial::opaque("rad_isotope", [0.55, 0.95, 0.20, 1.0], 0.75, 0.35)
    },
    // 15. Stasis Field Core — LabPBR Beacon/Conduit glass-metal resonator.
    BlockPbrMaterial {
        emissive: [0.22, 0.62, 1.00],
        emissive_strength: 2.20,
        clearcoat: 0.80,
        clearcoat_roughness: 0.04,
        ..BlockPbrMaterial::opaque("stasis_emitter", [0.30, 0.65, 0.98, 1.0], 0.50, 0.16)
    },
    // 16. Plasma Arc Cell — High-HDR-bloom plasma containment core.
    BlockPbrMaterial {
        emissive: [0.95, 0.35, 1.00],
        emissive_strength: 4.50,
        transmission: 0.40,
        alpha_mode: AlphaMode::Blend,
        ..BlockPbrMaterial::opaque("plasma_arc", [0.95, 0.48, 0.98, 0.95], 0.0, 0.12)
    },
    // 17. Bedrock — Indestructible basaltic floor (LabPBR rough dark stone).
    BlockPbrMaterial {
        ..BlockPbrMaterial::opaque("bedrock", [0.16, 0.16, 0.19, 1.0], 0.05, 0.90)
    },
    // 18. Stone — Classic Minecraft matte silicate rock.
    BlockPbrMaterial {
        porosity: 0.20,
        ..BlockPbrMaterial::opaque("stone", [0.50, 0.52, 0.54, 1.0], 0.0, 0.85)
    },
    // 19. Deepslate — Stratified dark metamorphic slate with subtle anisotropy.
    BlockPbrMaterial {
        anisotropy: 0.25,
        porosity: 0.15,
        ..BlockPbrMaterial::opaque("deepslate", [0.26, 0.27, 0.30, 1.0], 0.05, 0.72)
    },
    // 20. Dirt — High-porosity earthy soil.
    BlockPbrMaterial {
        porosity: 0.65,
        ..BlockPbrMaterial::opaque("dirt", [0.48, 0.33, 0.22, 1.0], 0.0, 0.94)
    },
    // 21. Grass Block — LabPBR foliage/grass SSS + damp porosity.
    BlockPbrMaterial {
        translucency: 0.20,
        porosity: 0.50,
        ..BlockPbrMaterial::opaque("grass_block", [0.32, 0.65, 0.24, 1.0], 0.0, 0.82)
    },
    // 22. Sand — Granular quartz sand (darkens cleanly when wet).
    BlockPbrMaterial {
        porosity: 0.45,
        ..BlockPbrMaterial::opaque("sand", [0.86, 0.79, 0.56, 1.0], 0.0, 0.88)
    },
    // 23. Sandstone — Compressed desert sedimentary rock.
    BlockPbrMaterial {
        porosity: 0.35,
        ..BlockPbrMaterial::opaque("sandstone", [0.80, 0.72, 0.48, 1.0], 0.0, 0.86)
    },
    // 24. Glacial Ice — LabPBR refractive ice (IOR 1.31, crisp clearcoat).
    BlockPbrMaterial {
        transmission: 0.72,
        ior: 1.31,
        translucency: 0.30,
        clearcoat: 0.75,
        clearcoat_roughness: 0.06,
        alpha_mode: AlphaMode::Blend,
        ..BlockPbrMaterial::opaque("ice_block", [0.62, 0.84, 0.98, 0.82], 0.0, 0.08)
    },
    // 25. Snow — Powdery snow with LabPBR subsurface scattering.
    BlockPbrMaterial {
        translucency: 0.45,
        porosity: 0.60,
        ..BlockPbrMaterial::opaque("snow_block", [0.94, 0.96, 1.00, 1.0], 0.0, 0.78)
    },
    // 26. Magma Lava — Incandescent molten rock (BSL/SEUS-style HDR lava).
    BlockPbrMaterial {
        emissive: [1.00, 0.28, 0.02],
        emissive_strength: 4.00,
        translucency: 0.35,
        ..BlockPbrMaterial::opaque("magma_lava", [1.00, 0.32, 0.05, 1.0], 0.0, 0.35)
    },
    // 27. Coal Ore — Matte carbon seams embedded in stone.
    BlockPbrMaterial {
        porosity: 0.30,
        ..BlockPbrMaterial::opaque("coal_ore", [0.18, 0.19, 0.21, 1.0], 0.0, 0.90)
    },
    // 28. Iron Ore — Raw metallic iron veins in silicate host rock.
    BlockPbrMaterial {
        porosity: 0.15,
        ..BlockPbrMaterial::opaque("iron_ore", [0.66, 0.56, 0.49, 1.0], 0.55, 0.58)
    },
    // 29. Copper Ore — LabPBR copper-malachite mineral veins.
    BlockPbrMaterial {
        porosity: 0.15,
        ..BlockPbrMaterial::opaque("copper_ore", [0.76, 0.44, 0.28, 1.0], 0.60, 0.48)
    },
    // 30. Gold Ore — High-reflectance LabPBR gold flecks in deep rock.
    BlockPbrMaterial {
        ..BlockPbrMaterial::opaque("gold_ore", [0.95, 0.78, 0.26, 1.0], 0.85, 0.25)
    },
    // 31. Redstone Ore — Piezoelectric glowing crimson mineral.
    BlockPbrMaterial {
        emissive: [0.95, 0.10, 0.08],
        emissive_strength: 0.85,
        ..BlockPbrMaterial::opaque("redstone_ore", [0.85, 0.16, 0.14, 1.0], 0.10, 0.42)
    },
    // 32. Diamond Ore — LabPBR high-IOR (2.417) crystalline carbon in deepslate.
    BlockPbrMaterial {
        emissive: [0.15, 0.65, 0.80],
        emissive_strength: 0.35,
        ior: 2.417,
        clearcoat: 0.70,
        clearcoat_roughness: 0.05,
        ..BlockPbrMaterial::opaque("diamond_ore", [0.35, 0.88, 0.95, 1.0], 0.15, 0.12)
    },
];

/// Look up the static PBR material for a `CATALOG` block id.
pub fn pbr_material_by_id(id: &str) -> Option<&'static BlockPbrMaterial> {
    BLOCK_PBR_MATERIALS.iter().find(|mat| mat.id == id)
}

/// Apply dynamic LabPBR-style surface modulation from runtime [`TransientFlags`].
///
/// Mirrors Minecraft shaderpack dynamic passes:
/// - `WET`: porosity-weighted albedo darkening + puddle roughness/clearcoat.
/// - `FROZEN`: frost clearcoat + slight cyan tint.
/// - `BURNING`: blackbody fire emission.
/// - `CHARGED`: electric arc cyan emission boost.
/// - `BROKEN`: fractured matte surface.
pub fn resolve_cell_pbr(cell: &PhysicalCell, mut mat: BlockPbrMaterial) -> BlockPbrMaterial {
    if cell.transient.contains(TransientFlags::WET) {
        let darken = 1.0 - 0.45 * mat.porosity.clamp(0.0, 1.0);
        mat.base_color[0] *= darken;
        mat.base_color[1] *= darken;
        mat.base_color[2] *= darken;
        mat.roughness = mat.roughness.min(0.08);
        mat.clearcoat = mat.clearcoat.max(0.65);
        mat.clearcoat_roughness = mat.clearcoat_roughness.min(0.05);
    }
    if cell.transient.contains(TransientFlags::FROZEN) {
        mat.base_color[0] = (mat.base_color[0] * 0.8 + 0.15).min(1.0);
        mat.base_color[1] = (mat.base_color[1] * 0.85 + 0.15).min(1.0);
        mat.base_color[2] = (mat.base_color[2] * 0.9 + 0.18).min(1.0);
        mat.roughness = mat.roughness.min(0.18);
        mat.clearcoat = mat.clearcoat.max(0.75);
        mat.clearcoat_roughness = mat.clearcoat_roughness.min(0.08);
    }
    if cell.transient.contains(TransientFlags::BURNING) {
        mat.emissive = [1.0, 0.32, 0.05];
        mat.emissive_strength = mat.emissive_strength.max(3.5);
    }
    if cell.transient.contains(TransientFlags::CHARGED) {
        mat.emissive = [
            (mat.emissive[0] + 0.25).min(1.0),
            (mat.emissive[1] + 0.82).min(1.0),
            (mat.emissive[2] + 1.0).min(1.0),
        ];
        mat.emissive_strength += 1.5;
    }
    if cell.transient.contains(TransientFlags::BROKEN) {
        mat.base_color[0] *= 0.65;
        mat.base_color[1] *= 0.65;
        mat.base_color[2] *= 0.65;
        mat.roughness = mat.roughness.max(0.85);
        mat.clearcoat = 0.0;
    }
    mat
}

/// Verify catalog alignment at compile time / tests.
pub fn catalog_matches_pbr_table() -> bool {
    CATALOG.len() == BLOCK_PBR_MATERIALS.len()
        && CATALOG
            .iter()
            .zip(BLOCK_PBR_MATERIALS.iter())
            .all(|(c, m)| c.id == m.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tags::{
        BioActivity, EnvPacked, FlowDir, MaterialFlags, MatterState, Pressure, SignalClockState,
        StimulusFlags, Temperature, TopologyFlags,
    };

    #[test]
    fn every_catalog_block_has_a_valid_pbr_material() {
        assert!(catalog_matches_pbr_table());
        for mat in BLOCK_PBR_MATERIALS {
            assert!((0.0..=1.0).contains(&mat.metallic));
            assert!((0.02..=1.0).contains(&mat.roughness));
            assert!(mat.ior >= 1.0);
            assert!((0.0..=1.0).contains(&mat.transmission));
            assert!((0.0..=1.0).contains(&mat.clearcoat));
        }
    }

    #[test]
    fn wet_porosity_darkens_sponge_and_adds_clearcoat() {
        let sponge = *pbr_material_by_id("absorbent_sponge").expect("sponge material");
        let wet_cell = PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Ambient,
                Pressure::Normal,
                1,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::NONE,
            transient: TransientFlags::WET,
            topology: TopologyFlags::NONE,
            pending_stimuli: StimulusFlags::NONE,
            durability: u8::MAX,
            clocks: SignalClockState::new(0, 0, 0, 0),
        };
        let resolved = resolve_cell_pbr(&wet_cell, sponge);
        assert!(resolved.base_color[0] < sponge.base_color[0]);
        assert!(resolved.roughness <= 0.08);
        assert!(resolved.clearcoat >= 0.65);
    }
}
