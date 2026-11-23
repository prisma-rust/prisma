//! Read-only material ROM table mapping compact material IDs to `PhysicalCell`s and visuals.

use prism_game_api::GameWorld;
use prism_scene::{pack_rgba, pack_visual, VoxelVisualFlags};
use serde::Deserialize;

use crate::tags::{
    BioActivity, EnvPacked, FlowDir, MaterialFlags, MatterState, PhysicalCell, Pressure,
    SignalClockState, StimulusFlags, Temperature, TopologyFlags, TransientFlags,
};

/// Compact ROM material identifiers stored alongside L2 simulation cells.
pub mod mat_id {
    pub const AIR: u8 = 0;
    pub const BEDROCK: u8 = 1;
    pub const SOIL: u8 = 2;
    pub const GRASS: u8 = 3;
    pub const LIMESTONE: u8 = 4;
    pub const ALLOY: u8 = 5;
    pub const GLASS: u8 = 6;
    pub const TIMBER: u8 = 7;
    pub const WATER: u8 = 8;
    pub const ACID: u8 = 9;
    pub const CRYO: u8 = 10;
    pub const BIOMASS: u8 = 11;
    pub const WIRE: u8 = 12;
    pub const CONDUIT: u8 = 13;
    pub const VALVE: u8 = 14;
    pub const ISOTOPE: u8 = 15;
    pub const TANK: u8 = 16;
}

#[derive(Debug, Clone, Deserialize)]
struct MaterialFileJson {
    materials: Vec<MaterialJson>,
}

#[derive(Debug, Clone, Deserialize)]
struct MaterialJson {
    id: u8,
    name: String,
    rgba: [u8; 4],
    roughness: u8,
    emissive: u8,
    state: MatterState,
    temperature: Temperature,
    pressure: Pressure,
    bio_activity: BioActivity,
    #[serde(default)]
    material_flags: Vec<String>,
    #[serde(default)]
    transient_flags: Vec<String>,
    #[serde(default)]
    topology_flags: Vec<String>,
    #[serde(default = "default_durability")]
    durability: u8,
}

fn default_durability() -> u8 {
    100
}

/// Runtime immutable ROM entry describing one base material preset.
#[derive(Debug, Clone)]
pub struct MaterialEntry {
    pub id: u8,
    pub name: String,
    pub rgba: [u8; 4],
    pub roughness: u8,
    pub emissive: u8,
    pub template: PhysicalCell,
}

/// Dense lookup table (`id -> MaterialEntry`) parsed from `data/materials.json` or compiled fallback.
#[derive(Debug, Clone)]
pub struct MaterialTable {
    entries: Vec<MaterialEntry>,
}

impl Default for MaterialTable {
    fn default() -> Self {
        Self::compiled_default()
    }
}

impl MaterialTable {
    /// Compiled fallback ROM parsed from the embedded `materials.json`.
    pub fn compiled_default() -> Self {
        let json = include_str!("../assets/data/materials.json");
        Self::from_json(json).expect("embedded materials.json must be valid")
    }

    /// Load from mounted asset pack (`data/materials.json`), falling back to embedded ROM.
    pub fn load(world: Option<&GameWorld<'_>>) -> Self {
        if let Some(world) = world {
            if let Ok(text) = world.read_asset_text("data/materials.json") {
                if let Ok(table) = Self::from_json(&text) {
                    return table;
                }
            }
        }
        Self::compiled_default()
    }

    /// Parse a JSON materials table document.
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        let doc: MaterialFileJson = serde_json::from_str(text)?;
        let max_id = doc
            .materials
            .iter()
            .map(|m| m.id as usize)
            .max()
            .unwrap_or(0);
        let mut entries = vec![
            MaterialEntry {
                id: 0,
                name: "Air".to_string(),
                rgba: [0, 0, 0, 0],
                roughness: 255,
                emissive: 0,
                template: PhysicalCell::default(),
            };
            max_id + 1
        ];
        for m in doc.materials {
            let mat_flags = parse_material_flags(&m.material_flags);
            let trans_flags = parse_transient_flags(&m.transient_flags);
            let topo_flags = parse_topology_flags(&m.topology_flags);
            let clocks = if m.id == mat_id::ISOTOPE {
                SignalClockState::new(300, 0, 12, 5)
            } else {
                SignalClockState::default()
            };
            let cell = PhysicalCell {
                env: EnvPacked::new(
                    m.state,
                    m.temperature,
                    m.pressure,
                    5,
                    m.bio_activity,
                    FlowDir::None,
                ),
                material: mat_flags,
                transient: trans_flags,
                topology: topo_flags,
                pending_stimuli: StimulusFlags::NONE,
                durability: m.durability,
                clocks,
            };
            entries[m.id as usize] = MaterialEntry {
                id: m.id,
                name: m.name,
                rgba: m.rgba,
                roughness: m.roughness,
                emissive: m.emissive,
                template: cell,
            };
        }
        Ok(Self { entries })
    }

    /// Return metadata for `id` (clamps unknown IDs to `Air`).
    #[inline]
    pub fn get(&self, id: u8) -> &MaterialEntry {
        self.entries.get(id as usize).unwrap_or(&self.entries[0])
    }

    /// Spawn a fresh 16-byte `PhysicalCell` initialized from material `id`.
    #[inline]
    pub fn spawn_cell(&self, id: u8) -> PhysicalCell {
        self.get(id).template
    }

    /// Resolve a base material ID from packed GPU RGBA (used for untouched COW L1 cells).
    pub fn find_by_packed_rgba(&self, packed: u32) -> Option<u8> {
        self.entries
            .iter()
            .skip(1)
            .find(|e| pack_rgba(e.rgba[0], e.rgba[1], e.rgba[2], e.rgba[3]) == packed)
            .map(|e| e.id)
    }

    /// Convert a material ID and live `PhysicalCell` into packed GPU `(rgba, visual)` words.
    pub fn cell_visuals(&self, id: u8, cell: &PhysicalCell) -> (u32, u32) {
        if id == mat_id::AIR
            || cell.durability == 0
                && cell.transient.contains(TransientFlags::BROKEN)
                && cell.material.contains(MaterialFlags::GLASS)
        {
            return (0, 0);
        }
        let entry = self.get(id);
        let [mut r, mut g, mut b, a] = entry.rgba;
        let mut flags = VoxelVisualFlags::NONE;
        if cell.env.matter() == MatterState::Liquid {
            flags |= VoxelVisualFlags::LIQUID | VoxelVisualFlags::TRANSLUCENT;
        }
        if a < 250 || cell.material.contains(MaterialFlags::GLASS) {
            flags |= VoxelVisualFlags::TRANSLUCENT;
        }
        if cell.material.contains(MaterialFlags::METALLIC) {
            flags |= VoxelVisualFlags::METALLIC;
        }
        if entry.emissive > 0 {
            flags |= VoxelVisualFlags::EMISSIVE;
        }

        if cell.transient.contains(TransientFlags::WET) {
            flags |= VoxelVisualFlags::WET;
        }
        if cell.transient.contains(TransientFlags::BURNING) {
            flags |= VoxelVisualFlags::BURNING | VoxelVisualFlags::HEAT_GLOW;
            r = (r as u16 / 2 + 120).min(255) as u8;
            g = (g as u16 / 2 + 45).min(255) as u8;
            b /= 3;
        }
        if cell.transient.contains(TransientFlags::FROZEN)
            || cell.env.temperature() <= Temperature::Cryo
        {
            flags |= VoxelVisualFlags::FROZEN;
        }
        if cell.transient.contains(TransientFlags::CHARGED) {
            flags |= VoxelVisualFlags::CHARGED;
        }
        if cell.transient.contains(TransientFlags::CORRODED)
            || cell.transient.contains(TransientFlags::CORROSIVE)
        {
            flags |= VoxelVisualFlags::CORRODED;
        }
        if cell.transient.contains(TransientFlags::INFECTED)
            || cell.env.bio_activity() >= BioActivity::Vital
        {
            flags |= VoxelVisualFlags::INFECTED;
        }
        if cell.transient.contains(TransientFlags::WARNING) {
            flags |= VoxelVisualFlags::WARNING;
        }
        if cell.transient.contains(TransientFlags::STASIS) {
            flags |= VoxelVisualFlags::STASIS;
        }
        if cell.transient.contains(TransientFlags::BROKEN) {
            flags |= VoxelVisualFlags::BROKEN;
        }
        if cell.env.temperature() >= Temperature::Hot {
            flags |= VoxelVisualFlags::HEAT_GLOW;
        }

        let rgba = pack_rgba(r, g, b, a);
        let visual = pack_visual(flags, entry.roughness, entry.emissive);
        (rgba, visual)
    }
}

fn parse_material_flags(names: &[String]) -> MaterialFlags {
    let mut f = MaterialFlags::NONE;
    for name in names {
        f = f.union(match name.as_str() {
            "METALLIC" => MaterialFlags::METALLIC,
            "ORGANIC" => MaterialFlags::ORGANIC,
            "GLASS" => MaterialFlags::GLASS,
            "CONDUCTIVE" => MaterialFlags::CONDUCTIVE,
            "INSULATED" => MaterialFlags::INSULATED,
            "FLAMMABLE" => MaterialFlags::FLAMMABLE,
            "AIRTIGHT" => MaterialFlags::AIRTIGHT,
            "PRESSURE_RATED" => MaterialFlags::PRESSURE_RATED,
            "HOT_RESISTANT" => MaterialFlags::HOT_RESISTANT,
            "RAD_SHIELDED" => MaterialFlags::RAD_SHIELDED,
            "PIEZOELECTRIC" => MaterialFlags::PIEZOELECTRIC,
            "SUPERCONDUCTIVE" => MaterialFlags::SUPERCONDUCTIVE,
            "ELASTIC" => MaterialFlags::ELASTIC,
            "ABSORBENT" => MaterialFlags::ABSORBENT,
            "OXIDIZER" => MaterialFlags::OXIDIZER,
            _ => MaterialFlags::NONE,
        });
    }
    f
}

fn parse_transient_flags(names: &[String]) -> TransientFlags {
    let mut f = TransientFlags::NONE;
    for name in names {
        f = f.union(match name.as_str() {
            "WET" => TransientFlags::WET,
            "BURNING" => TransientFlags::BURNING,
            "FROZEN" => TransientFlags::FROZEN,
            "CHARGED" => TransientFlags::CHARGED,
            "MAGNETIZED" => TransientFlags::MAGNETIZED,
            "BRITTLE" => TransientFlags::BRITTLE,
            "CORRODED" => TransientFlags::CORRODED,
            "BROKEN" => TransientFlags::BROKEN,
            "VOLATILE" => TransientFlags::VOLATILE,
            "TOXIC" => TransientFlags::TOXIC,
            "CORROSIVE" => TransientFlags::CORROSIVE,
            "RADIOACTIVE" => TransientFlags::RADIOACTIVE,
            "INFECTED" => TransientFlags::INFECTED,
            "STASIS" => TransientFlags::STASIS,
            "WARNING" => TransientFlags::WARNING,
            "DORMANT" => TransientFlags::DORMANT,
            "SHARP" => TransientFlags::SHARP,
            "VISCOUS" => TransientFlags::VISCOUS,
            _ => TransientFlags::NONE,
        });
    }
    f
}

fn parse_topology_flags(names: &[String]) -> TopologyFlags {
    let mut f = TopologyFlags::NONE;
    for name in names {
        f = f.union(match name.as_str() {
            "RIGID_WELD" => TopologyFlags::RIGID_WELD,
            "BIO_NEST" => TopologyFlags::BIO_NEST,
            "FROZEN_LOCKED" => TopologyFlags::FROZEN_LOCKED,
            "PUNCTURED" => TopologyFlags::PUNCTURED,
            "EDIBLE" => TopologyFlags::EDIBLE,
            "EQUIPPABLE" => TopologyFlags::EQUIPPABLE,
            "SEALED" => TopologyFlags::SEALED,
            "CONVEYOR" => TopologyFlags::CONVEYOR,
            "CONDUIT" => TopologyFlags::CONDUIT,
            "VALVE" => TopologyFlags::VALVE,
            _ => TopologyFlags::NONE,
        });
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_scene::visual_flags;

    #[test]
    fn compiled_materials_parse_and_project_visual_flags() {
        let table = MaterialTable::compiled_default();
        assert_eq!(table.get(mat_id::ALLOY).name, "AlloyPlate");
        assert_eq!(table.get(mat_id::GLASS).name, "GlassPane");

        let mut cable = table.spawn_cell(mat_id::WIRE);
        cable.transient.insert(TransientFlags::CHARGED);
        cable.transient.insert(TransientFlags::WET);
        let (_, vis) = table.cell_visuals(mat_id::WIRE, &cable);
        let vf = visual_flags(vis);
        assert_ne!(vf & VoxelVisualFlags::METALLIC, 0);
        assert_ne!(vf & VoxelVisualFlags::CHARGED, 0);
        assert_ne!(vf & VoxelVisualFlags::WET, 0);
    }

    #[test]
    fn spbr_pack_manifest_references_existing_textures() {
        let json = include_str!("../assets/data/spbr_pack.json");
        let doc: serde_json::Value = serde_json::from_str(json).expect("valid spbr_pack.json");
        let textures = doc["textures"].as_object().expect("textures object");
        assert!(!textures.is_empty());
        let assets_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        for (name, entry) in textures {
            for slot in ["albedo", "normal", "specular"] {
                if let Some(rel) = entry[slot].as_str() {
                    let full = assets_root.join(rel);
                    assert!(
                        full.exists(),
                        "missing SPBR {slot} texture for {name}: {full:?}"
                    );
                }
            }
        }
    }
}
