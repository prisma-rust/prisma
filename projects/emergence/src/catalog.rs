//! Item and entity catalog with full v3.0 tag inspection formatting.
//!
//! Provides a unified Not-Enough-Items (NEI) registry of placeable materials
//! and active emergent entities, along with human-readable tag and state
//! breakdown for aimed-at cell inspection.

use std::fmt::Write as _;

use crate::tags::{
    BioActivity, EnvPacked, FlowDir, MaterialFlags, MatterState, PhysicalCell, Pressure,
    SignalClockState, StimulusFlags, Temperature, TopologyFlags, TransientFlags,
    SWITCH_HEAT_MELTDOWN_THRESHOLD,
};

/// Distinguishes passive/structural items from active/autonomous entities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogKind {
    Item,
    Entity,
}

impl CatalogKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Item => "Item",
            Self::Entity => "Entity",
        }
    }
}

/// One entry in the NEI item and entity palette.
#[derive(Debug, Clone, Copy)]
pub struct CatalogEntry {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: CatalogKind,
    pub color: [f32; 4],
    pub template: PhysicalCell,
}

/// Complete NEI catalog covering all v3.0 materials, hazards, and entities.
pub const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        id: "conductive_wire",
        name: "Conductive Wire",
        kind: CatalogKind::Item,
        color: [0.82, 0.55, 0.25, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Ambient,
                Pressure::Normal,
                6,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::METALLIC.union(MaterialFlags::CONDUCTIVE),
            transient: TransientFlags::DORMANT,
            topology: TopologyFlags::NONE,
            pending_stimuli: StimulusFlags::NONE,
            durability: u8::MAX,
            clocks: SignalClockState::new(0, 0, 10, 20),
        },
    },
    CatalogEntry {
        id: "superconductor",
        name: "Superconductor Coil",
        kind: CatalogKind::Item,
        color: [0.35, 0.85, 0.95, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Cryo,
                Pressure::Normal,
                6,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::METALLIC
                .union(MaterialFlags::CONDUCTIVE)
                .union(MaterialFlags::SUPERCONDUCTIVE),
            transient: TransientFlags::DORMANT,
            topology: TopologyFlags::NONE,
            pending_stimuli: StimulusFlags::NONE,
            durability: u8::MAX,
            clocks: SignalClockState::new(0, 0, 15, 20),
        },
    },
    CatalogEntry {
        id: "water_cell",
        name: "Water",
        kind: CatalogKind::Item,
        color: [0.2, 0.5, 0.9, 0.95],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Liquid,
                Temperature::Ambient,
                Pressure::Normal,
                3,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::NONE,
            transient: TransientFlags::WET.union(TransientFlags::DORMANT),
            topology: TopologyFlags::NONE,
            pending_stimuli: StimulusFlags::NONE,
            durability: u8::MAX,
            clocks: SignalClockState::new(0, 0, 0, 0),
        },
    },
    CatalogEntry {
        id: "glass_wall",
        name: "Reinforced Glass",
        kind: CatalogKind::Item,
        color: [0.7, 0.88, 0.95, 0.75],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Ambient,
                Pressure::Normal,
                5,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::GLASS.union(MaterialFlags::AIRTIGHT),
            transient: TransientFlags::DORMANT,
            topology: TopologyFlags::RIGID_WELD,
            pending_stimuli: StimulusFlags::NONE,
            durability: 160,
            clocks: SignalClockState::new(0, 0, 0, 0),
        },
    },
    CatalogEntry {
        id: "insulated_wall",
        name: "Insulated Bulkhead",
        kind: CatalogKind::Item,
        color: [0.48, 0.52, 0.58, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Ambient,
                Pressure::Normal,
                7,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::INSULATED
                .union(MaterialFlags::AIRTIGHT)
                .union(MaterialFlags::PRESSURE_RATED)
                .union(MaterialFlags::HOT_RESISTANT)
                .union(MaterialFlags::RAD_SHIELDED),
            transient: TransientFlags::DORMANT,
            topology: TopologyFlags::RIGID_WELD.union(TopologyFlags::SEALED),
            pending_stimuli: StimulusFlags::NONE,
            durability: u8::MAX,
            clocks: SignalClockState::new(0, 0, 0, 0),
        },
    },
    CatalogEntry {
        id: "volatile_fuel",
        name: "Volatile Fuel Drum",
        kind: CatalogKind::Item,
        color: [0.9, 0.35, 0.2, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Liquid,
                Temperature::Ambient,
                Pressure::Pressurized,
                2,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::FLAMMABLE.union(MaterialFlags::OXIDIZER),
            transient: TransientFlags::VOLATILE.union(TransientFlags::DORMANT),
            topology: TopologyFlags::SEALED,
            pending_stimuli: StimulusFlags::NONE,
            durability: 120,
            clocks: SignalClockState::new(0, 0, 0, 0),
        },
    },
    CatalogEntry {
        id: "absorbent_sponge",
        name: "Absorbent Foam",
        kind: CatalogKind::Item,
        color: [0.85, 0.8, 0.45, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Ambient,
                Pressure::Normal,
                5,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::ABSORBENT
                .union(MaterialFlags::ELASTIC)
                .union(MaterialFlags::ORGANIC),
            transient: TransientFlags::DORMANT,
            topology: TopologyFlags::NONE,
            pending_stimuli: StimulusFlags::NONE,
            durability: 180,
            clocks: SignalClockState::new(0, 0, 0, 0),
        },
    },
    CatalogEntry {
        id: "acid_pool",
        name: "Corrosive Acid",
        kind: CatalogKind::Item,
        color: [0.6, 0.95, 0.2, 0.95],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Liquid,
                Temperature::Ambient,
                Pressure::Normal,
                3,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::NONE,
            transient: TransientFlags::CORROSIVE
                .union(TransientFlags::TOXIC)
                .union(TransientFlags::VISCOUS),
            topology: TopologyFlags::NONE,
            pending_stimuli: StimulusFlags::NONE,
            durability: u8::MAX,
            clocks: SignalClockState::new(600, 0, 0, 1),
        },
    },
    CatalogEntry {
        id: "cryo_spore",
        name: "Cryo-Spore Specimen",
        kind: CatalogKind::Entity,
        color: [0.45, 0.82, 0.65, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Cryo,
                Pressure::Normal,
                5,
                BioActivity::Cryptosis,
                FlowDir::None,
            ),
            material: MaterialFlags::ORGANIC.union(MaterialFlags::FLAMMABLE),
            transient: TransientFlags::NONE,
            topology: TopologyFlags::EDIBLE,
            pending_stimuli: StimulusFlags::NONE,
            durability: 200,
            clocks: SignalClockState::new(120, 0, 0, 1),
        },
    },
    CatalogEntry {
        id: "hyper_neo_core",
        name: "Hyper-Neo Mutant",
        kind: CatalogKind::Entity,
        color: [0.35, 1.0, 0.25, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Ambient,
                Pressure::Normal,
                5,
                BioActivity::HyperNeo,
                FlowDir::None,
            ),
            material: MaterialFlags::ORGANIC,
            transient: TransientFlags::INFECTED.union(TransientFlags::TOXIC),
            topology: TopologyFlags::BIO_NEST,
            pending_stimuli: StimulusFlags::NONE,
            durability: 240,
            clocks: SignalClockState::new(300, 0, 0, 4),
        },
    },
    CatalogEntry {
        id: "piezo_crystal",
        name: "Piezo Crystal Node",
        kind: CatalogKind::Entity,
        color: [0.75, 0.45, 0.98, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Ambient,
                Pressure::Normal,
                6,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::PIEZOELECTRIC.union(MaterialFlags::CONDUCTIVE),
            transient: TransientFlags::DORMANT,
            topology: TopologyFlags::EQUIPPABLE,
            pending_stimuli: StimulusFlags::NONE,
            durability: 220,
            clocks: SignalClockState::new(0, 0, 4, 10),
        },
    },
    CatalogEntry {
        id: "valve_conduit",
        name: "Pressurized Valve Conduit",
        kind: CatalogKind::Entity,
        color: [0.35, 0.65, 0.78, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Liquid,
                Temperature::Ambient,
                Pressure::Pressurized,
                3,
                BioActivity::Inert,
                FlowDir::E,
            ),
            material: MaterialFlags::METALLIC
                .union(MaterialFlags::AIRTIGHT)
                .union(MaterialFlags::PRESSURE_RATED),
            transient: TransientFlags::DORMANT,
            topology: TopologyFlags::CONDUIT.union(TopologyFlags::VALVE),
            pending_stimuli: StimulusFlags::NONE,
            durability: u8::MAX,
            clocks: SignalClockState::new(0, 0, 8, 5),
        },
    },
    CatalogEntry {
        id: "conveyor_east",
        name: "Overclocked Conveyor",
        kind: CatalogKind::Entity,
        color: [0.92, 0.72, 0.25, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Ambient,
                Pressure::Normal,
                6,
                BioActivity::Inert,
                FlowDir::E,
            ),
            material: MaterialFlags::METALLIC.union(MaterialFlags::CONDUCTIVE),
            transient: TransientFlags::DORMANT,
            topology: TopologyFlags::CONVEYOR,
            pending_stimuli: StimulusFlags::NONE,
            durability: u8::MAX,
            clocks: SignalClockState::new(0, 0, 0, 20),
        },
    },
    CatalogEntry {
        id: "rad_isotope",
        name: "Radioactive Isotope",
        kind: CatalogKind::Entity,
        color: [0.85, 0.95, 0.2, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Hot,
                Pressure::Normal,
                7,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::METALLIC.union(MaterialFlags::HOT_RESISTANT),
            transient: TransientFlags::RADIOACTIVE.union(TransientFlags::WARNING),
            topology: TopologyFlags::NONE,
            pending_stimuli: StimulusFlags::NONE,
            durability: u8::MAX,
            clocks: SignalClockState::new(1000, 0, 0, 1),
        },
    },
    CatalogEntry {
        id: "stasis_emitter",
        name: "Stasis Field Core",
        kind: CatalogKind::Entity,
        color: [0.55, 0.85, 1.0, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Ambient,
                Pressure::Normal,
                6,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::METALLIC.union(MaterialFlags::INSULATED),
            transient: TransientFlags::STASIS.union(TransientFlags::DORMANT),
            topology: TopologyFlags::SEALED,
            pending_stimuli: StimulusFlags::NONE,
            durability: u8::MAX,
            clocks: SignalClockState::new(0, 0, 0, 0),
        },
    },
    CatalogEntry {
        id: "plasma_arc",
        name: "Plasma Arc Cell",
        kind: CatalogKind::Entity,
        color: [0.95, 0.4, 0.95, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Plasma,
                Temperature::PlasmaHeat,
                Pressure::Pressurized,
                1,
                BioActivity::Inert,
                FlowDir::Out,
            ),
            material: MaterialFlags::CONDUCTIVE,
            transient: TransientFlags::CHARGED
                .union(TransientFlags::MAGNETIZED)
                .union(TransientFlags::BURNING),
            topology: TopologyFlags::NONE,
            pending_stimuli: StimulusFlags::NONE,
            durability: 128,
            clocks: SignalClockState::new(60, 0, 15, 20),
        },
    },
    CatalogEntry {
        id: "bedrock",
        name: "Bedrock",
        kind: CatalogKind::Item,
        color: [0.16, 0.16, 0.19, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Ambient,
                Pressure::GigaPressure,
                7,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::HOT_RESISTANT
                .union(MaterialFlags::PRESSURE_RATED)
                .union(MaterialFlags::RAD_SHIELDED)
                .union(MaterialFlags::AIRTIGHT),
            transient: TransientFlags::DORMANT,
            topology: TopologyFlags::RIGID_WELD.union(TopologyFlags::SEALED),
            pending_stimuli: StimulusFlags::NONE,
            durability: u8::MAX,
            clocks: SignalClockState::new(0, 0, 0, 0),
        },
    },
    CatalogEntry {
        id: "stone",
        name: "Stone",
        kind: CatalogKind::Item,
        color: [0.50, 0.52, 0.54, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Ambient,
                Pressure::Normal,
                6,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::HOT_RESISTANT,
            transient: TransientFlags::DORMANT,
            topology: TopologyFlags::RIGID_WELD,
            pending_stimuli: StimulusFlags::NONE,
            durability: 200,
            clocks: SignalClockState::new(0, 0, 0, 0),
        },
    },
    CatalogEntry {
        id: "deepslate",
        name: "Deepslate",
        kind: CatalogKind::Item,
        color: [0.26, 0.27, 0.30, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Ambient,
                Pressure::Pressurized,
                7,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::HOT_RESISTANT.union(MaterialFlags::PRESSURE_RATED),
            transient: TransientFlags::DORMANT,
            topology: TopologyFlags::RIGID_WELD,
            pending_stimuli: StimulusFlags::NONE,
            durability: 240,
            clocks: SignalClockState::new(0, 0, 0, 0),
        },
    },
    CatalogEntry {
        id: "dirt",
        name: "Dirt",
        kind: CatalogKind::Item,
        color: [0.48, 0.33, 0.22, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Ambient,
                Pressure::Normal,
                5,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::ABSORBENT,
            transient: TransientFlags::DORMANT,
            topology: TopologyFlags::NONE,
            pending_stimuli: StimulusFlags::NONE,
            durability: 60,
            clocks: SignalClockState::new(0, 0, 0, 0),
        },
    },
    CatalogEntry {
        id: "grass_block",
        name: "Grass Block",
        kind: CatalogKind::Item,
        color: [0.32, 0.65, 0.24, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Ambient,
                Pressure::Normal,
                5,
                BioActivity::Vital,
                FlowDir::None,
            ),
            material: MaterialFlags::ORGANIC.union(MaterialFlags::ABSORBENT),
            transient: TransientFlags::DORMANT,
            topology: TopologyFlags::NONE,
            pending_stimuli: StimulusFlags::NONE,
            durability: 65,
            clocks: SignalClockState::new(0, 0, 0, 0),
        },
    },
    CatalogEntry {
        id: "sand",
        name: "Sand",
        kind: CatalogKind::Item,
        color: [0.86, 0.79, 0.56, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Ambient,
                Pressure::Normal,
                5,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::GRANULAR.union(MaterialFlags::GLASS),
            transient: TransientFlags::DORMANT,
            topology: TopologyFlags::NONE,
            pending_stimuli: StimulusFlags::NONE,
            durability: 45,
            clocks: SignalClockState::new(0, 0, 0, 0),
        },
    },
    CatalogEntry {
        id: "sandstone",
        name: "Sandstone",
        kind: CatalogKind::Item,
        color: [0.80, 0.72, 0.48, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Ambient,
                Pressure::Normal,
                6,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::HOT_RESISTANT,
            transient: TransientFlags::DORMANT,
            topology: TopologyFlags::RIGID_WELD,
            pending_stimuli: StimulusFlags::NONE,
            durability: 140,
            clocks: SignalClockState::new(0, 0, 0, 0),
        },
    },
    CatalogEntry {
        id: "ice_block",
        name: "Glacial Ice",
        kind: CatalogKind::Item,
        color: [0.62, 0.84, 0.98, 0.85],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Cryo,
                Pressure::Normal,
                5,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::GLASS,
            transient: TransientFlags::FROZEN.union(TransientFlags::DORMANT),
            topology: TopologyFlags::NONE,
            pending_stimuli: StimulusFlags::NONE,
            durability: 70,
            clocks: SignalClockState::new(0, 0, 0, 0),
        },
    },
    CatalogEntry {
        id: "snow_block",
        name: "Snow",
        kind: CatalogKind::Item,
        color: [0.94, 0.96, 1.0, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Cryo,
                Pressure::Normal,
                5,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::GRANULAR.union(MaterialFlags::ABSORBENT),
            transient: TransientFlags::FROZEN.union(TransientFlags::DORMANT),
            topology: TopologyFlags::NONE,
            pending_stimuli: StimulusFlags::NONE,
            durability: 30,
            clocks: SignalClockState::new(0, 0, 0, 0),
        },
    },
    CatalogEntry {
        id: "magma_lava",
        name: "Magma Lava",
        kind: CatalogKind::Item,
        color: [1.0, 0.32, 0.05, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Liquid,
                Temperature::PlasmaHeat,
                Pressure::Normal,
                4,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::HOT_RESISTANT,
            transient: TransientFlags::BURNING,
            topology: TopologyFlags::NONE,
            pending_stimuli: StimulusFlags::NONE,
            durability: u8::MAX,
            clocks: SignalClockState::new(0, 0, 0, 0),
        },
    },
    CatalogEntry {
        id: "coal_ore",
        name: "Coal Ore",
        kind: CatalogKind::Item,
        color: [0.18, 0.19, 0.21, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Ambient,
                Pressure::Normal,
                6,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::FLAMMABLE,
            transient: TransientFlags::DORMANT,
            topology: TopologyFlags::RIGID_WELD,
            pending_stimuli: StimulusFlags::NONE,
            durability: 130,
            clocks: SignalClockState::new(0, 0, 0, 0),
        },
    },
    CatalogEntry {
        id: "iron_ore",
        name: "Iron Ore",
        kind: CatalogKind::Item,
        color: [0.66, 0.56, 0.49, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Ambient,
                Pressure::Normal,
                6,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::METALLIC.union(MaterialFlags::CONDUCTIVE),
            transient: TransientFlags::DORMANT,
            topology: TopologyFlags::RIGID_WELD,
            pending_stimuli: StimulusFlags::NONE,
            durability: 185,
            clocks: SignalClockState::new(0, 0, 0, 0),
        },
    },
    CatalogEntry {
        id: "copper_ore",
        name: "Copper Ore",
        kind: CatalogKind::Item,
        color: [0.76, 0.44, 0.28, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Ambient,
                Pressure::Normal,
                6,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::METALLIC.union(MaterialFlags::CONDUCTIVE),
            transient: TransientFlags::DORMANT,
            topology: TopologyFlags::RIGID_WELD,
            pending_stimuli: StimulusFlags::NONE,
            durability: 165,
            clocks: SignalClockState::new(0, 0, 0, 0),
        },
    },
    CatalogEntry {
        id: "gold_ore",
        name: "Gold Ore",
        kind: CatalogKind::Item,
        color: [0.95, 0.78, 0.26, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Ambient,
                Pressure::Pressurized,
                7,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::METALLIC
                .union(MaterialFlags::CONDUCTIVE)
                .union(MaterialFlags::HOT_RESISTANT),
            transient: TransientFlags::DORMANT,
            topology: TopologyFlags::RIGID_WELD,
            pending_stimuli: StimulusFlags::NONE,
            durability: 195,
            clocks: SignalClockState::new(0, 0, 0, 0),
        },
    },
    CatalogEntry {
        id: "redstone_ore",
        name: "Redstone Ore",
        kind: CatalogKind::Item,
        color: [0.85, 0.16, 0.14, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Ambient,
                Pressure::Pressurized,
                7,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::CONDUCTIVE.union(MaterialFlags::PIEZOELECTRIC),
            transient: TransientFlags::DORMANT,
            topology: TopologyFlags::RIGID_WELD,
            pending_stimuli: StimulusFlags::NONE,
            durability: 190,
            clocks: SignalClockState::new(0, 0, 12, 16),
        },
    },
    CatalogEntry {
        id: "diamond_ore",
        name: "Diamond Ore",
        kind: CatalogKind::Item,
        color: [0.35, 0.88, 0.95, 1.0],
        template: PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Ambient,
                Pressure::GigaPressure,
                7,
                BioActivity::Inert,
                FlowDir::None,
            ),
            material: MaterialFlags::GLASS
                .union(MaterialFlags::PRESSURE_RATED)
                .union(MaterialFlags::HOT_RESISTANT),
            transient: TransientFlags::DORMANT,
            topology: TopologyFlags::RIGID_WELD,
            pending_stimuli: StimulusFlags::NONE,
            durability: 245,
            clocks: SignalClockState::new(0, 0, 0, 0),
        },
    },
];

/// Identifies the display name and kind of a live grid cell.
pub fn identify_cell(cell: &PhysicalCell) -> (&'static str, CatalogKind) {
    if cell.transient.contains(TransientFlags::BROKEN) {
        return ("Broken Debris", CatalogKind::Item);
    }
    if cell.env.pressure() == Pressure::HardVacuum
        && cell.material.is_empty()
        && cell.transient.contains(TransientFlags::DORMANT)
    {
        return ("Vacuum", CatalogKind::Item);
    }
    if cell.env.bio_activity() == BioActivity::HyperNeo {
        return ("Hyper-Neo Mutant", CatalogKind::Entity);
    }
    if cell.topology.contains(TopologyFlags::BIO_NEST) {
        return ("Bio-Nest Hive", CatalogKind::Entity);
    }
    if cell.material.contains(MaterialFlags::ORGANIC)
        && cell.env.bio_activity() != BioActivity::Inert
    {
        return ("Bio-Specimen", CatalogKind::Entity);
    }
    if cell.topology.contains(TopologyFlags::CONDUIT)
        || cell.topology.contains(TopologyFlags::VALVE)
    {
        return ("Valve Conduit", CatalogKind::Entity);
    }
    if cell.topology.contains(TopologyFlags::CONVEYOR) {
        return ("Conveyor Track", CatalogKind::Entity);
    }
    if cell.material.contains(MaterialFlags::PIEZOELECTRIC) {
        return ("Piezo Crystal", CatalogKind::Entity);
    }
    if cell.transient.contains(TransientFlags::STASIS) {
        return ("Stasis Core", CatalogKind::Entity);
    }
    if cell.transient.contains(TransientFlags::RADIOACTIVE) {
        return ("Radioactive Source", CatalogKind::Entity);
    }
    if cell.transient.contains(TransientFlags::CORROSIVE) {
        return ("Corrosive Acid", CatalogKind::Item);
    }
    if cell.material.contains(MaterialFlags::SUPERCONDUCTIVE) {
        return ("Superconductor", CatalogKind::Item);
    }
    if cell.material.contains(MaterialFlags::CONDUCTIVE) {
        return ("Conductive Wire", CatalogKind::Item);
    }
    if cell.material.contains(MaterialFlags::GLASS) {
        return ("Glass Block", CatalogKind::Item);
    }
    if cell.material.contains(MaterialFlags::FLAMMABLE) {
        return ("Flammable Matter", CatalogKind::Item);
    }
    if cell.env.matter() == MatterState::Liquid {
        return ("Fluid Cell", CatalogKind::Item);
    }
    if cell.env.matter() == MatterState::Plasma {
        return ("Plasma Cell", CatalogKind::Entity);
    }
    if cell.env.matter() == MatterState::Gas {
        return ("Gas Cell", CatalogKind::Item);
    }
    ("Solid Matter", CatalogKind::Item)
}

/// Formats a complete, multi-line inspector readout for an aimed-at cell.
pub fn format_cell_inspector(
    title_override: Option<(&str, CatalogKind)>,
    coords: Option<(usize, usize)>,
    cell: &PhysicalCell,
) -> String {
    let (name, kind) = title_override.unwrap_or_else(|| identify_cell(cell));
    let mut out = String::with_capacity(256);

    if let Some((x, y)) = coords {
        let _ = writeln!(out, "[{x},{y}] {name} ({})", kind.label());
    } else {
        let _ = writeln!(out, "{name} ({})", kind.label());
    }

    let _ = writeln!(
        out,
        "Env: {:?} | {:?} | {:?} | Den:{} | {:?} | Flow:{:?}",
        cell.env.matter(),
        cell.env.temperature(),
        cell.env.pressure(),
        cell.env.density(),
        cell.env.bio_activity(),
        cell.env.flow_dir()
    );

    out.push_str("Mat: ");
    append_material_flags(&mut out, cell.material);
    out.push('\n');

    out.push_str("Trn: ");
    append_transient_flags(&mut out, cell.transient);
    out.push('\n');

    out.push_str("Top: ");
    append_topology_flags(&mut out, cell.topology);
    out.push('\n');

    out.push_str("Stm: ");
    append_stimulus_flags(&mut out, cell.pending_stimuli);
    out.push('\n');

    let _ = write!(
        out,
        "State: dur={}/255 hl={} sw_heat={}/{} int={}/15 hz={}/20",
        cell.durability,
        cell.clocks.half_life(),
        cell.clocks.switch_heat(),
        SWITCH_HEAT_MELTDOWN_THRESHOLD,
        cell.clocks.intensity(),
        cell.clocks.tick_hz(),
    );

    out
}

const MATERIAL_FLAG_TABLE: &[(u32, &str)] = &[
    (MaterialFlags::METALLIC.0 as u32, "METALLIC"),
    (MaterialFlags::ORGANIC.0 as u32, "ORGANIC"),
    (MaterialFlags::GLASS.0 as u32, "GLASS"),
    (MaterialFlags::CONDUCTIVE.0 as u32, "CONDUCTIVE"),
    (MaterialFlags::INSULATED.0 as u32, "INSULATED"),
    (MaterialFlags::FLAMMABLE.0 as u32, "FLAMMABLE"),
    (MaterialFlags::AIRTIGHT.0 as u32, "AIRTIGHT"),
    (MaterialFlags::PRESSURE_RATED.0 as u32, "PRESSURE_RATED"),
    (MaterialFlags::HOT_RESISTANT.0 as u32, "HOT_RESISTANT"),
    (MaterialFlags::RAD_SHIELDED.0 as u32, "RAD_SHIELDED"),
    (MaterialFlags::PIEZOELECTRIC.0 as u32, "PIEZOELECTRIC"),
    (MaterialFlags::SUPERCONDUCTIVE.0 as u32, "SUPERCONDUCTIVE"),
    (MaterialFlags::ELASTIC.0 as u32, "ELASTIC"),
    (MaterialFlags::ABSORBENT.0 as u32, "ABSORBENT"),
    (MaterialFlags::OXIDIZER.0 as u32, "OXIDIZER"),
    (MaterialFlags::GRANULAR.0 as u32, "GRANULAR"),
];

const TRANSIENT_FLAG_TABLE: &[(u32, &str)] = &[
    (TransientFlags::WET.0, "WET"),
    (TransientFlags::BURNING.0, "BURNING"),
    (TransientFlags::FROZEN.0, "FROZEN"),
    (TransientFlags::CHARGED.0, "CHARGED"),
    (TransientFlags::MAGNETIZED.0, "MAGNETIZED"),
    (TransientFlags::BRITTLE.0, "BRITTLE"),
    (TransientFlags::CORRODED.0, "CORRODED"),
    (TransientFlags::BROKEN.0, "BROKEN"),
    (TransientFlags::VOLATILE.0, "VOLATILE"),
    (TransientFlags::TOXIC.0, "TOXIC"),
    (TransientFlags::CORROSIVE.0, "CORROSIVE"),
    (TransientFlags::RADIOACTIVE.0, "RADIOACTIVE"),
    (TransientFlags::INFECTED.0, "INFECTED"),
    (TransientFlags::STASIS.0, "STASIS"),
    (TransientFlags::WARNING.0, "WARNING"),
    (TransientFlags::DORMANT.0, "DORMANT"),
    (TransientFlags::SHARP.0, "SHARP"),
    (TransientFlags::VISCOUS.0, "VISCOUS"),
];

const TOPOLOGY_FLAG_TABLE: &[(u32, &str)] = &[
    (TopologyFlags::RIGID_WELD.0 as u32, "RIGID_WELD"),
    (TopologyFlags::BIO_NEST.0 as u32, "BIO_NEST"),
    (TopologyFlags::FROZEN_LOCKED.0 as u32, "FROZEN_LOCKED"),
    (TopologyFlags::PUNCTURED.0 as u32, "PUNCTURED"),
    (TopologyFlags::EDIBLE.0 as u32, "EDIBLE"),
    (TopologyFlags::EQUIPPABLE.0 as u32, "EQUIPPABLE"),
    (TopologyFlags::SEALED.0 as u32, "SEALED"),
    (TopologyFlags::CONVEYOR.0 as u32, "CONVEYOR"),
    (TopologyFlags::CONDUIT.0 as u32, "CONDUIT"),
    (TopologyFlags::VALVE.0 as u32, "VALVE"),
];

/// Collects active `#emergence:<flag>` tags for Horizon-style F3 inspection.
pub(crate) fn collect_cell_debug_tags(cell: &PhysicalCell) -> Vec<String> {
    let mut tags = Vec::new();
    for (bits, table) in [
        (cell.material.0 as u32, MATERIAL_FLAG_TABLE),
        (cell.transient.0, TRANSIENT_FLAG_TABLE),
        (cell.topology.0 as u32, TOPOLOGY_FLAG_TABLE),
    ] {
        for &(flag, label) in table {
            if (bits & flag) != 0 {
                tags.push(format!("#emergence:{}", label.to_ascii_lowercase()));
            }
        }
    }
    tags
}

fn append_flag_list(out: &mut String, bits: u32, table: &[(u32, &str)]) {
    let mut first = true;
    for &(flag, label) in table {
        if (bits & flag) != 0 {
            if !first {
                out.push_str(" | ");
            }
            out.push_str(label);
            first = false;
        }
    }
    if first {
        out.push_str("NONE");
    }
}

fn append_material_flags(out: &mut String, flags: MaterialFlags) {
    append_flag_list(out, flags.0 as u32, MATERIAL_FLAG_TABLE);
}

fn append_transient_flags(out: &mut String, flags: TransientFlags) {
    append_flag_list(out, flags.0, TRANSIENT_FLAG_TABLE);
}

fn append_topology_flags(out: &mut String, flags: TopologyFlags) {
    append_flag_list(out, flags.0 as u32, TOPOLOGY_FLAG_TABLE);
}

fn append_stimulus_flags(out: &mut String, flags: StimulusFlags) {
    const TABLE: &[(u32, &str)] = &[
        (StimulusFlags::HEAT.0 as u32, "HEAT"),
        (StimulusFlags::COLD.0 as u32, "COLD"),
        (StimulusFlags::SHOCK.0 as u32, "SHOCK"),
        (StimulusFlags::ACID.0 as u32, "ACID"),
        (StimulusFlags::BIO.0 as u32, "BIO"),
        (StimulusFlags::KINETIC.0 as u32, "KINETIC"),
        (StimulusFlags::RADIATION.0 as u32, "RADIATION"),
        (StimulusFlags::LIGHT.0 as u32, "LIGHT"),
    ];
    append_flag_list(out, flags.0 as u32, TABLE);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_contains_both_items_and_entities_with_unique_ids() {
        let mut ids = std::collections::HashSet::new();
        let mut items = 0usize;
        let mut entities = 0usize;
        for entry in CATALOG {
            assert!(ids.insert(entry.id), "duplicate catalog id {}", entry.id);
            match entry.kind {
                CatalogKind::Item => items += 1,
                CatalogKind::Entity => entities += 1,
            }
        }
        assert!(items >= 6);
        assert!(entities >= 6);
    }

    #[test]
    fn inspector_formats_all_tag_dimensions_and_clocks() {
        let entry = CATALOG
            .iter()
            .find(|e| e.id == "valve_conduit")
            .expect("valve_conduit in catalog");
        let text = format_cell_inspector(None, Some((10, 9)), &entry.template);
        assert!(text.contains("[10,9] Valve Conduit (Entity)"));
        assert!(text.contains("METALLIC | AIRTIGHT | PRESSURE_RATED"));
        assert!(text.contains("CONDUIT | VALVE"));
        assert!(text.contains("dur=255/255"));
        assert!(text.contains("int=8/15"));
        assert!(text.contains("hz=5/20"));
    }
}
