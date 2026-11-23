//! Core v3.0 tag, environment enum, stimulus, and compact cell data model.
//!
//! All five categories from the Emergence v3.0 specification pack cleanly into a
//! 16-byte `PhysicalCell` so four simulation cells fit inside a single 64-byte
//! CPU cache line.

use serde::{Deserialize, Serialize};

/// Macroscopic matter phase (2 bits).
///
/// Matter state strictly constrains the macroscopic density ladder so gases and
/// plasmas are always lighter than liquids, which are always lighter than
/// solids.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum MatterState {
    Solid = 0,
    Liquid = 1,
    Gas = 2,
    Plasma = 3,
}

impl MatterState {
    pub const fn from_bits(bits: u16) -> Self {
        match bits & 0b11 {
            0 => Self::Solid,
            1 => Self::Liquid,
            2 => Self::Gas,
            _ => Self::Plasma,
        }
    }

    /// Inclusive `(min, max)` discrete density tier permitted for this phase.
    ///
    /// Enforces `Gas/Plasma (0..=1) < Liquid (2..=4) < Solid (5..=7)`.
    pub const fn density_band(self) -> (u8, u8) {
        match self {
            Self::Gas | Self::Plasma => (0, 1),
            Self::Liquid => (2, 4),
            Self::Solid => (5, 7),
        }
    }

    /// Clamps a material-level density hint into this matter phase's valid band.
    pub const fn clamp_density(self, raw: u8) -> u8 {
        let (min, max) = self.density_band();
        if raw < min {
            min
        } else if raw > max {
            max
        } else {
            raw
        }
    }
}

/// Discrete temperature tiers driving phase transitions and metabolism (3 bits).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Temperature {
    UltraCryo = 0,
    Cryo = 1,
    Ambient = 2,
    Hot = 3,
    PlasmaHeat = 4,
}

impl Temperature {
    pub const fn from_bits(bits: u16) -> Self {
        match bits & 0b111 {
            0 => Self::UltraCryo,
            1 => Self::Cryo,
            2 => Self::Ambient,
            3 => Self::Hot,
            _ => Self::PlasmaHeat,
        }
    }

    pub const fn step_warmer(self) -> Self {
        match self {
            Self::UltraCryo => Self::Cryo,
            Self::Cryo => Self::Ambient,
            Self::Ambient => Self::Hot,
            Self::Hot | Self::PlasmaHeat => Self::PlasmaHeat,
        }
    }

    pub const fn step_cooler(self) -> Self {
        match self {
            Self::UltraCryo | Self::Cryo => Self::UltraCryo,
            Self::Ambient => Self::Cryo,
            Self::Hot => Self::Ambient,
            Self::PlasmaHeat => Self::Hot,
        }
    }
}

/// Discrete pressure tiers driving vacuum boiling and overpressure liquefaction (3 bits).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Pressure {
    HardVacuum = 0,
    Vacuum = 1,
    Normal = 2,
    Pressurized = 3,
    GigaPressure = 4,
}

impl Pressure {
    pub const fn from_bits(bits: u16) -> Self {
        match bits & 0b111 {
            0 => Self::HardVacuum,
            1 => Self::Vacuum,
            2 => Self::Normal,
            3 => Self::Pressurized,
            _ => Self::GigaPressure,
        }
    }
}

/// Biological activity 4-stage state machine for organic matter (2 bits).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum BioActivity {
    Inert = 0,
    Cryptosis = 1,
    Vital = 2,
    HyperNeo = 3,
}

impl BioActivity {
    pub const fn from_bits(bits: u16) -> Self {
        match bits & 0b11 {
            0 => Self::Inert,
            1 => Self::Cryptosis,
            2 => Self::Vital,
            _ => Self::HyperNeo,
        }
    }
}

/// Directed transport vector for conduits and surface conveyors (3 bits).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FlowDir {
    None = 0,
    N = 1,
    S = 2,
    E = 3,
    W = 4,
    In = 5,
    Out = 6,
    BiDir = 7,
}

impl FlowDir {
    pub const fn from_bits(bits: u16) -> Self {
        match bits & 0b111 {
            0 => Self::None,
            1 => Self::N,
            2 => Self::S,
            3 => Self::E,
            4 => Self::W,
            5 => Self::In,
            6 => Self::Out,
            _ => Self::BiDir,
        }
    }
}

/// Bit-packed core state and environment descriptor (exact 16 bits).
///
/// Layout (LSB to MSB):
/// - `0..2`: `MatterState` (2 bits)
/// - `2..5`: `Temperature` (3 bits)
/// - `5..8`: `Pressure` (3 bits)
/// - `8..11`: `Density` (3 bits, constrained by `MatterState`)
/// - `11..13`: `BioActivity` (2 bits)
/// - `13..16`: `FlowDir` (3 bits)
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EnvPacked(pub u16);

impl EnvPacked {
    const MATTER_SHIFT: u16 = 0;
    const TEMP_SHIFT: u16 = 2;
    const PRESSURE_SHIFT: u16 = 5;
    const DENSITY_SHIFT: u16 = 8;
    const BIO_SHIFT: u16 = 11;
    const FLOW_SHIFT: u16 = 13;

    pub const fn new(
        matter: MatterState,
        temp: Temperature,
        pressure: Pressure,
        density_hint: u8,
        bio: BioActivity,
        flow: FlowDir,
    ) -> Self {
        let clamped_density = matter.clamp_density(density_hint);
        let bits = ((matter as u16) << Self::MATTER_SHIFT)
            | ((temp as u16) << Self::TEMP_SHIFT)
            | ((pressure as u16) << Self::PRESSURE_SHIFT)
            | ((clamped_density as u16) << Self::DENSITY_SHIFT)
            | ((bio as u16) << Self::BIO_SHIFT)
            | ((flow as u16) << Self::FLOW_SHIFT);
        Self(bits)
    }

    pub const fn matter(self) -> MatterState {
        MatterState::from_bits(self.0 >> Self::MATTER_SHIFT)
    }

    pub const fn temperature(self) -> Temperature {
        Temperature::from_bits(self.0 >> Self::TEMP_SHIFT)
    }

    pub const fn pressure(self) -> Pressure {
        Pressure::from_bits(self.0 >> Self::PRESSURE_SHIFT)
    }

    pub const fn density(self) -> u8 {
        ((self.0 >> Self::DENSITY_SHIFT) & 0b111) as u8
    }

    pub const fn bio_activity(self) -> BioActivity {
        BioActivity::from_bits(self.0 >> Self::BIO_SHIFT)
    }

    pub const fn flow_dir(self) -> FlowDir {
        FlowDir::from_bits(self.0 >> Self::FLOW_SHIFT)
    }

    /// Updates `MatterState` and clamps density into the new phase's valid band.
    pub fn set_matter(&mut self, matter: MatterState) {
        let density = matter.clamp_density(self.density());
        self.0 = (self.0 & !(0b11 << Self::MATTER_SHIFT) & !(0b111 << Self::DENSITY_SHIFT))
            | ((matter as u16) << Self::MATTER_SHIFT)
            | ((density as u16) << Self::DENSITY_SHIFT);
    }

    pub fn set_temperature(&mut self, temp: Temperature) {
        self.0 = (self.0 & !(0b111 << Self::TEMP_SHIFT)) | ((temp as u16) << Self::TEMP_SHIFT);
    }

    pub fn set_pressure(&mut self, pressure: Pressure) {
        self.0 = (self.0 & !(0b111 << Self::PRESSURE_SHIFT))
            | ((pressure as u16) << Self::PRESSURE_SHIFT);
    }

    pub fn set_density_hint(&mut self, density_hint: u8) {
        let density = self.matter().clamp_density(density_hint);
        self.0 =
            (self.0 & !(0b111 << Self::DENSITY_SHIFT)) | ((density as u16) << Self::DENSITY_SHIFT);
    }

    pub fn set_bio_activity(&mut self, bio: BioActivity) {
        self.0 = (self.0 & !(0b11 << Self::BIO_SHIFT)) | ((bio as u16) << Self::BIO_SHIFT);
    }

    pub fn set_flow_dir(&mut self, flow: FlowDir) {
        self.0 = (self.0 & !(0b111 << Self::FLOW_SHIFT)) | ((flow as u16) << Self::FLOW_SHIFT);
    }
}

impl Default for EnvPacked {
    fn default() -> Self {
        Self::new(
            MatterState::Solid,
            Temperature::Ambient,
            Pressure::Normal,
            5,
            BioActivity::Inert,
            FlowDir::None,
        )
    }
}

/// Intrinsic material and physical resistance bitflags (15 bits in `u16`).
#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MaterialFlags(pub u16);

impl MaterialFlags {
    pub const NONE: Self = Self(0);
    pub const METALLIC: Self = Self(1 << 0);
    pub const ORGANIC: Self = Self(1 << 1);
    pub const GLASS: Self = Self(1 << 2);
    pub const CONDUCTIVE: Self = Self(1 << 3);
    pub const INSULATED: Self = Self(1 << 4);
    pub const FLAMMABLE: Self = Self(1 << 5);
    pub const AIRTIGHT: Self = Self(1 << 6);
    pub const PRESSURE_RATED: Self = Self(1 << 7);
    pub const HOT_RESISTANT: Self = Self(1 << 8);
    pub const RAD_SHIELDED: Self = Self(1 << 9);
    pub const PIEZOELECTRIC: Self = Self(1 << 10);
    pub const SUPERCONDUCTIVE: Self = Self(1 << 11);
    pub const ELASTIC: Self = Self(1 << 12);
    pub const ABSORBENT: Self = Self(1 << 13);
    pub const OXIDIZER: Self = Self(1 << 14);
    pub const GRANULAR: Self = Self(1 << 15);

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub const fn intersects(self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// Dynamic transient physical and hazard state bitflags (18 bits in `u32`).
#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TransientFlags(pub u32);

impl TransientFlags {
    pub const NONE: Self = Self(0);
    pub const WET: Self = Self(1 << 0);
    pub const BURNING: Self = Self(1 << 1);
    pub const FROZEN: Self = Self(1 << 2);
    pub const CHARGED: Self = Self(1 << 3);
    pub const MAGNETIZED: Self = Self(1 << 4);
    pub const BRITTLE: Self = Self(1 << 5);
    pub const CORRODED: Self = Self(1 << 6);
    pub const BROKEN: Self = Self(1 << 7);
    pub const VOLATILE: Self = Self(1 << 8);
    pub const TOXIC: Self = Self(1 << 9);
    pub const CORROSIVE: Self = Self(1 << 10);
    pub const RADIOACTIVE: Self = Self(1 << 11);
    pub const INFECTED: Self = Self(1 << 12);
    pub const STASIS: Self = Self(1 << 13);
    pub const WARNING: Self = Self(1 << 14);
    pub const DORMANT: Self = Self(1 << 15);
    pub const SHARP: Self = Self(1 << 16);
    pub const VISCOUS: Self = Self(1 << 17);

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub const fn intersects(self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }
}

/// Spatial topology and transport affordance bitflags (10 bits in `u16`).
#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TopologyFlags(pub u16);

impl TopologyFlags {
    pub const NONE: Self = Self(0);
    pub const RIGID_WELD: Self = Self(1 << 0);
    pub const BIO_NEST: Self = Self(1 << 1);
    pub const FROZEN_LOCKED: Self = Self(1 << 2);
    pub const PUNCTURED: Self = Self(1 << 3);
    pub const EDIBLE: Self = Self(1 << 4);
    pub const EQUIPPABLE: Self = Self(1 << 5);
    pub const SEALED: Self = Self(1 << 6);
    pub const CONVEYOR: Self = Self(1 << 7);
    pub const CONDUIT: Self = Self(1 << 8);
    pub const VALVE: Self = Self(1 << 9);

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub const fn intersects(self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }
}

/// External stimulus channels driving state transitions (exact 8 bits in `u8`).
#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StimulusFlags(pub u8);

impl StimulusFlags {
    pub const NONE: Self = Self(0);
    pub const HEAT: Self = Self(1 << 0);
    pub const COLD: Self = Self(1 << 1);
    pub const SHOCK: Self = Self(1 << 2);
    pub const ACID: Self = Self(1 << 3);
    pub const BIO: Self = Self(1 << 4);
    pub const KINETIC: Self = Self(1 << 5);
    pub const RADIATION: Self = Self(1 << 6);
    pub const LIGHT: Self = Self(1 << 7);

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub fn clear(&mut self) {
        self.0 = 0;
    }
}

/// Packed dynamic clocks and discrete signal registers (4 bytes).
///
/// Layout (LSB to MSB):
/// - `0..15`: `half_life` (0..=32767 ticks)
/// - `15..23`: `switch_heat` (0..=255 switching budget counter)
/// - `23..27`: `intensity` (0..=15 signal strength)
/// - `27..32`: `tick_hz` (0..=20 discrete pulse rate)
#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SignalClockState(pub u32);

impl SignalClockState {
    pub const MAX_HALF_LIFE: u16 = 0x7FFF;
    pub const MAX_INTENSITY: u8 = 15;
    pub const MAX_TICK_HZ: u8 = 20;

    const HALF_LIFE_MASK: u32 = 0x7FFF;
    const SWITCH_HEAT_SHIFT: u32 = 15;
    const SWITCH_HEAT_MASK: u32 = 0xFF << Self::SWITCH_HEAT_SHIFT;
    const INTENSITY_SHIFT: u32 = 23;
    const INTENSITY_MASK: u32 = 0xF << Self::INTENSITY_SHIFT;
    const TICK_HZ_SHIFT: u32 = 27;
    const TICK_HZ_MASK: u32 = 0x1F << Self::TICK_HZ_SHIFT;

    pub const fn new(half_life: u16, switch_heat: u8, intensity: u8, tick_hz: u8) -> Self {
        let clamped_half_life = if half_life > Self::MAX_HALF_LIFE {
            Self::MAX_HALF_LIFE
        } else {
            half_life
        };
        let clamped_intensity = if intensity > Self::MAX_INTENSITY {
            Self::MAX_INTENSITY
        } else {
            intensity
        };
        let clamped_hz = if tick_hz > Self::MAX_TICK_HZ {
            Self::MAX_TICK_HZ
        } else {
            tick_hz
        };
        let bits = (clamped_half_life as u32)
            | ((switch_heat as u32) << Self::SWITCH_HEAT_SHIFT)
            | ((clamped_intensity as u32) << Self::INTENSITY_SHIFT)
            | ((clamped_hz as u32) << Self::TICK_HZ_SHIFT);
        Self(bits)
    }

    pub const fn half_life(self) -> u16 {
        (self.0 & Self::HALF_LIFE_MASK) as u16
    }

    pub const fn switch_heat(self) -> u8 {
        ((self.0 & Self::SWITCH_HEAT_MASK) >> Self::SWITCH_HEAT_SHIFT) as u8
    }

    pub const fn intensity(self) -> u8 {
        ((self.0 & Self::INTENSITY_MASK) >> Self::INTENSITY_SHIFT) as u8
    }

    pub const fn tick_hz(self) -> u8 {
        ((self.0 & Self::TICK_HZ_MASK) >> Self::TICK_HZ_SHIFT) as u8
    }

    pub fn set_half_life(&mut self, value: u16) {
        let clamped = value.min(Self::MAX_HALF_LIFE);
        self.0 = (self.0 & !Self::HALF_LIFE_MASK) | (clamped as u32);
    }

    pub fn set_switch_heat(&mut self, value: u8) {
        self.0 = (self.0 & !Self::SWITCH_HEAT_MASK) | ((value as u32) << Self::SWITCH_HEAT_SHIFT);
    }

    pub fn set_intensity(&mut self, value: u8) {
        let clamped = value.min(Self::MAX_INTENSITY);
        self.0 = (self.0 & !Self::INTENSITY_MASK) | ((clamped as u32) << Self::INTENSITY_SHIFT);
    }

    pub fn set_tick_hz(&mut self, value: u8) {
        let clamped = value.min(Self::MAX_TICK_HZ);
        self.0 = (self.0 & !Self::TICK_HZ_MASK) | ((clamped as u32) << Self::TICK_HZ_SHIFT);
    }
}

/// Complete 16-byte simulation payload for one cell or physical entity.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PhysicalCell {
    pub env: EnvPacked,
    pub material: MaterialFlags,
    pub transient: TransientFlags,
    pub topology: TopologyFlags,
    pub pending_stimuli: StimulusFlags,
    pub durability: u8,
    pub clocks: SignalClockState,
}

const _: () = assert!(std::mem::size_of::<PhysicalCell>() == 16);

impl Default for PhysicalCell {
    fn default() -> Self {
        Self {
            env: EnvPacked::default(),
            material: MaterialFlags::NONE,
            transient: TransientFlags::NONE,
            topology: TopologyFlags::NONE,
            pending_stimuli: StimulusFlags::NONE,
            durability: u8::MAX,
            clocks: SignalClockState::default(),
        }
    }
}

/// Threshold of `switch_heat` toggles before a non-superconductive conductor fails.
pub const SWITCH_HEAT_MELTDOWN_THRESHOLD: u8 = 12;

impl PhysicalCell {
    /// Queues an external stimulus and wakes a `DORMANT` cell immediately.
    pub fn apply_stimulus(&mut self, stimulus: StimulusFlags) {
        if stimulus.is_empty() {
            return;
        }
        self.pending_stimuli.insert(stimulus);
        self.transient.remove(TransientFlags::DORMANT);
    }

    /// Evaluates one deterministic reaction tick on this cell.
    ///
    /// Returns `true` if the cell remains active and needs further ticks, or
    /// `false` if it settled into equilibrium and entered `DORMANT`.
    pub fn step_reaction(&mut self, advance_clock_tick: bool) -> bool {
        if self.transient.contains(TransientFlags::STASIS) {
            self.pending_stimuli.clear();
            self.transient.insert(TransientFlags::DORMANT);
            return false;
        }

        if self.transient.contains(TransientFlags::DORMANT) && self.pending_stimuli.is_empty() {
            return false;
        }

        let stimuli = self.pending_stimuli;
        self.pending_stimuli.clear();

        if stimuli.contains(StimulusFlags::HEAT)
            && !self.material.contains(MaterialFlags::INSULATED)
        {
            let next_temp = self.env.temperature().step_warmer();
            self.env.set_temperature(next_temp);
            if next_temp >= Temperature::Ambient {
                if self.transient.contains(TransientFlags::FROZEN)
                    && !self.material.intersects(
                        MaterialFlags::METALLIC
                            .union(MaterialFlags::GLASS)
                            .union(MaterialFlags::ORGANIC),
                    )
                {
                    self.env.set_matter(MatterState::Liquid);
                }
                self.transient.remove(TransientFlags::FROZEN);
                self.transient.remove(TransientFlags::BRITTLE);
                self.topology.remove(TopologyFlags::FROZEN_LOCKED);
            }
            if self.material.contains(MaterialFlags::FLAMMABLE)
                && !self.material.contains(MaterialFlags::HOT_RESISTANT)
                && !self.transient.contains(TransientFlags::WET)
                && !self.transient.contains(TransientFlags::BROKEN)
                && (self.env.pressure() >= Pressure::Normal
                    || self.material.contains(MaterialFlags::OXIDIZER))
            {
                self.transient.insert(TransientFlags::BURNING);
            }
            if next_temp >= Temperature::PlasmaHeat
                && self.material.contains(MaterialFlags::GRANULAR)
                && self.material.contains(MaterialFlags::GLASS)
            {
                self.material.0 &= !MaterialFlags::GRANULAR.0;
                self.topology.insert(TopologyFlags::RIGID_WELD);
            }
            if next_temp >= Temperature::Hot
                && self.env.matter() == MatterState::Solid
                && !self.topology.contains(TopologyFlags::RIGID_WELD)
                && (self.material.contains(MaterialFlags::GLASS)
                    || (self.material.contains(MaterialFlags::GRANULAR)
                        && self.material.contains(MaterialFlags::ABSORBENT)))
            {
                self.env.set_matter(MatterState::Liquid);
                self.material.0 &= !MaterialFlags::GRANULAR.0;
                self.transient.insert(TransientFlags::WET);
            }
        }

        if stimuli.contains(StimulusFlags::COLD)
            && !self.material.contains(MaterialFlags::INSULATED)
        {
            let next_temp = self.env.temperature().step_cooler();
            self.env.set_temperature(next_temp);
            self.transient.remove(TransientFlags::BURNING);
            if self.env.matter() == MatterState::Liquid
                && self.material.contains(MaterialFlags::HOT_RESISTANT)
                && next_temp <= Temperature::Hot
            {
                self.env.set_matter(MatterState::Solid);
            }
            if next_temp <= Temperature::Cryo {
                if self.env.matter() == MatterState::Liquid {
                    self.env.set_matter(MatterState::Solid);
                    self.transient.insert(TransientFlags::FROZEN);
                }
                if !self.material.contains(MaterialFlags::ELASTIC) {
                    self.transient.insert(TransientFlags::BRITTLE);
                }
            }
        }

        match (
            self.env.matter(),
            self.env.pressure(),
            self.env.temperature(),
        ) {
            (MatterState::Liquid, Pressure::HardVacuum | Pressure::Vacuum, t)
                if t >= Temperature::Ambient =>
            {
                self.env.set_matter(MatterState::Gas);
            }
            (MatterState::Gas, Pressure::GigaPressure, t) if t <= Temperature::Ambient => {
                self.env.set_matter(MatterState::Liquid);
            }
            _ => {}
        }

        if stimuli.contains(StimulusFlags::SHOCK) {
            let can_conduct = self.material.contains(MaterialFlags::CONDUCTIVE)
                || self.transient.contains(TransientFlags::WET);
            if can_conduct && !self.transient.contains(TransientFlags::BROKEN) {
                let was_charged = self.transient.contains(TransientFlags::CHARGED);
                self.transient.insert(TransientFlags::CHARGED);
                if !was_charged && !self.material.contains(MaterialFlags::SUPERCONDUCTIVE) {
                    let heat = self.clocks.switch_heat().saturating_add(1);
                    self.clocks.set_switch_heat(heat);
                    if heat + 2 >= SWITCH_HEAT_MELTDOWN_THRESHOLD {
                        self.transient.insert(TransientFlags::WARNING);
                    }
                    if heat >= SWITCH_HEAT_MELTDOWN_THRESHOLD {
                        self.break_cell(false);
                    }
                }
            }
        }

        if stimuli.contains(StimulusFlags::KINETIC) {
            if self.material.contains(MaterialFlags::PIEZOELECTRIC)
                && !self.transient.contains(TransientFlags::BROKEN)
            {
                self.transient.insert(TransientFlags::CHARGED);
                if self.clocks.intensity() < 4 {
                    self.clocks.set_intensity(4);
                }
            }
            if self.transient.contains(TransientFlags::BRITTLE)
                || self.material.contains(MaterialFlags::GLASS)
            {
                self.break_cell(true);
            }
        }

        if stimuli.contains(StimulusFlags::ACID)
            && !self.material.contains(MaterialFlags::GLASS)
            && !self.topology.contains(TopologyFlags::SEALED)
        {
            self.transient.insert(TransientFlags::CORRODED);
            self.durability = self.durability.saturating_sub(32);
            if self.durability == 0 {
                self.break_cell(true);
            }
        }

        if self.transient.contains(TransientFlags::BURNING) {
            let has_oxidizer = self.env.pressure() >= Pressure::Normal
                || self.material.contains(MaterialFlags::OXIDIZER);
            if self.transient.contains(TransientFlags::WET)
                || self.material.contains(MaterialFlags::HOT_RESISTANT)
                || !has_oxidizer
            {
                self.transient.remove(TransientFlags::BURNING);
            } else if advance_clock_tick {
                self.durability = self.durability.saturating_sub(8);
                if self.durability == 0 {
                    self.break_cell(false);
                }
            }
        }

        if self.material.contains(MaterialFlags::ORGANIC) {
            if self.env.temperature() <= Temperature::Cryo
                || self.transient.contains(TransientFlags::FROZEN)
            {
                if self.env.bio_activity() == BioActivity::Vital {
                    self.env.set_bio_activity(BioActivity::Cryptosis);
                }
            } else if self.env.bio_activity() == BioActivity::Cryptosis
                && self.env.temperature() == Temperature::Ambient
                && self.transient.contains(TransientFlags::WET)
            {
                self.env.set_bio_activity(BioActivity::Vital);
            }

            if stimuli.contains(StimulusFlags::RADIATION)
                && !self.material.contains(MaterialFlags::RAD_SHIELDED)
                && self.env.bio_activity() == BioActivity::Vital
            {
                self.env.set_bio_activity(BioActivity::HyperNeo);
            }

            if stimuli.contains(StimulusFlags::BIO)
                && self.env.temperature() == Temperature::Ambient
                && self.transient.contains(TransientFlags::WET)
                && !self.topology.contains(TopologyFlags::SEALED)
            {
                self.transient.insert(TransientFlags::INFECTED);
            }
        }

        if advance_clock_tick {
            let is_frozen = self.transient.contains(TransientFlags::FROZEN)
                || self.env.temperature() <= Temperature::Cryo;
            let pause_bio_decay = self.material.contains(MaterialFlags::ORGANIC) && is_frozen;

            if !pause_bio_decay && self.clocks.half_life() > 0 {
                self.clocks.set_half_life(self.clocks.half_life() - 1);
            }

            if self.clocks.switch_heat() > 0 {
                let next_heat = self.clocks.switch_heat() - 1;
                self.clocks.set_switch_heat(next_heat);
                if next_heat + 2 < SWITCH_HEAT_MELTDOWN_THRESHOLD {
                    self.transient.remove(TransientFlags::WARNING);
                }
            }
        }

        let has_active_clock = self.clocks.half_life() > 0 || self.clocks.switch_heat() > 0;
        let has_active_hazard = self.transient.intersects(
            TransientFlags::BURNING
                .union(TransientFlags::WARNING)
                .union(TransientFlags::INFECTED),
        ) || self.env.bio_activity() == BioActivity::HyperNeo;
        if has_active_clock || has_active_hazard {
            self.transient.remove(TransientFlags::DORMANT);
            true
        } else {
            self.transient.insert(TransientFlags::DORMANT);
            false
        }
    }

    /// Marks the cell as structurally broken and clears active energy/combustion.
    fn break_cell(&mut self, puncture: bool) {
        self.durability = 0;
        self.transient.insert(TransientFlags::BROKEN);
        self.transient.remove(TransientFlags::CHARGED);
        self.transient.remove(TransientFlags::BURNING);
        self.transient.remove(TransientFlags::WARNING);
        self.clocks.set_intensity(0);
        if puncture {
            self.topology.insert(TopologyFlags::PUNCTURED);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_cell_fits_quarter_cache_line() {
        assert_eq!(std::mem::size_of::<PhysicalCell>(), 16);
        assert_eq!(std::mem::size_of::<EnvPacked>(), 2);
        assert_eq!(std::mem::size_of::<SignalClockState>(), 4);
    }

    #[test]
    fn matter_state_macroscopically_bounds_density_ladder() {
        let gas = EnvPacked::new(
            MatterState::Gas,
            Temperature::Ambient,
            Pressure::Normal,
            7,
            BioActivity::Inert,
            FlowDir::None,
        );
        let plasma = EnvPacked::new(
            MatterState::Plasma,
            Temperature::PlasmaHeat,
            Pressure::Normal,
            7,
            BioActivity::Inert,
            FlowDir::None,
        );
        let liquid = EnvPacked::new(
            MatterState::Liquid,
            Temperature::Ambient,
            Pressure::Normal,
            0,
            BioActivity::Inert,
            FlowDir::None,
        );
        let solid = EnvPacked::new(
            MatterState::Solid,
            Temperature::Ambient,
            Pressure::Normal,
            0,
            BioActivity::Inert,
            FlowDir::None,
        );

        assert!(gas.density() < liquid.density());
        assert!(plasma.density() < liquid.density());
        assert!(liquid.density() < solid.density());
    }

    #[test]
    fn stasis_freezes_phase_changes_and_half_life() {
        let mut cell = PhysicalCell {
            env: EnvPacked::new(
                MatterState::Liquid,
                Temperature::Ambient,
                Pressure::Normal,
                3,
                BioActivity::Vital,
                FlowDir::None,
            ),
            material: MaterialFlags::ORGANIC.union(MaterialFlags::FLAMMABLE),
            transient: TransientFlags::STASIS,
            clocks: SignalClockState::new(42, 0, 0, 0),
            ..PhysicalCell::default()
        };

        cell.apply_stimulus(StimulusFlags::HEAT);
        let active = cell.step_reaction(true);

        assert!(!active);
        assert_eq!(cell.env.temperature(), Temperature::Ambient);
        assert_eq!(cell.clocks.half_life(), 42);
        assert!(!cell.transient.contains(TransientFlags::BURNING));
    }

    #[test]
    fn bio_activity_cryptosis_and_radiation_mutation() {
        let mut cell = PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Cryo,
                Pressure::Normal,
                5,
                BioActivity::Vital,
                FlowDir::None,
            ),
            material: MaterialFlags::ORGANIC,
            transient: TransientFlags::FROZEN.union(TransientFlags::WET),
            clocks: SignalClockState::new(10, 0, 0, 0),
            ..PhysicalCell::default()
        };

        cell.step_reaction(true);
        assert_eq!(cell.env.bio_activity(), BioActivity::Cryptosis);
        assert_eq!(cell.clocks.half_life(), 10);

        cell.apply_stimulus(StimulusFlags::HEAT);
        cell.step_reaction(true);
        assert_eq!(cell.env.temperature(), Temperature::Ambient);
        assert_eq!(cell.env.bio_activity(), BioActivity::Vital);
        assert_eq!(cell.clocks.half_life(), 9);

        cell.apply_stimulus(StimulusFlags::RADIATION);
        cell.step_reaction(false);
        assert_eq!(cell.env.bio_activity(), BioActivity::HyperNeo);
    }

    #[test]
    fn switch_heat_melts_overloaded_non_superconductive_wire() {
        let mut wire = PhysicalCell {
            material: MaterialFlags::METALLIC.union(MaterialFlags::CONDUCTIVE),
            clocks: SignalClockState::new(0, SWITCH_HEAT_MELTDOWN_THRESHOLD - 1, 12, 20),
            ..PhysicalCell::default()
        };

        wire.apply_stimulus(StimulusFlags::SHOCK);
        wire.step_reaction(false);

        assert!(wire.transient.contains(TransientFlags::BROKEN));
        assert!(!wire.transient.contains(TransientFlags::CHARGED));
        assert_eq!(wire.durability, 0);
    }

    #[test]
    fn freeze_and_melt_are_reversible_and_clear_brittle() {
        let mut water = PhysicalCell {
            env: EnvPacked::new(
                MatterState::Liquid,
                Temperature::Ambient,
                Pressure::Normal,
                3,
                BioActivity::Inert,
                FlowDir::None,
            ),
            ..PhysicalCell::default()
        };

        water.apply_stimulus(StimulusFlags::COLD);
        water.step_reaction(false);
        assert_eq!(water.env.matter(), MatterState::Solid);
        assert_eq!(water.env.temperature(), Temperature::Cryo);
        assert!(water.transient.contains(TransientFlags::FROZEN));
        assert!(water.transient.contains(TransientFlags::BRITTLE));

        water.apply_stimulus(StimulusFlags::HEAT);
        water.step_reaction(false);
        assert_eq!(water.env.matter(), MatterState::Liquid);
        assert_eq!(water.env.temperature(), Temperature::Ambient);
        assert!(!water.transient.contains(TransientFlags::FROZEN));
        assert!(!water.transient.contains(TransientFlags::BRITTLE));
    }

    #[test]
    fn kinetic_shattering_piezo_glass_does_not_stay_charged() {
        let mut piezo_glass = PhysicalCell {
            material: MaterialFlags::GLASS.union(MaterialFlags::PIEZOELECTRIC),
            ..PhysicalCell::default()
        };

        piezo_glass.apply_stimulus(StimulusFlags::KINETIC);
        piezo_glass.step_reaction(false);

        assert!(piezo_glass.transient.contains(TransientFlags::BROKEN));
        assert!(piezo_glass.topology.contains(TopologyFlags::PUNCTURED));
        assert!(!piezo_glass.transient.contains(TransientFlags::CHARGED));
        assert_eq!(piezo_glass.clocks.intensity(), 0);
    }

    #[test]
    fn burning_consumes_durability_and_extinguishes_when_wet() {
        let mut fuel = PhysicalCell {
            material: MaterialFlags::FLAMMABLE,
            durability: 16,
            ..PhysicalCell::default()
        };

        fuel.apply_stimulus(StimulusFlags::HEAT);
        assert!(fuel.step_reaction(true));
        assert!(fuel.transient.contains(TransientFlags::BURNING));
        assert_eq!(fuel.durability, 8);

        fuel.transient.insert(TransientFlags::WET);
        fuel.apply_stimulus(StimulusFlags::LIGHT);
        assert!(!fuel.step_reaction(true));
        assert!(!fuel.transient.contains(TransientFlags::BURNING));
    }

    #[test]
    fn hyper_neo_remains_active_and_stasis_sets_dormant() {
        let mut mutant = PhysicalCell {
            env: EnvPacked::new(
                MatterState::Solid,
                Temperature::Ambient,
                Pressure::Normal,
                5,
                BioActivity::Vital,
                FlowDir::None,
            ),
            material: MaterialFlags::ORGANIC,
            ..PhysicalCell::default()
        };

        mutant.apply_stimulus(StimulusFlags::RADIATION);
        assert!(mutant.step_reaction(false));
        assert_eq!(mutant.env.bio_activity(), BioActivity::HyperNeo);
        assert!(!mutant.transient.contains(TransientFlags::DORMANT));

        mutant.transient.insert(TransientFlags::STASIS);
        mutant.apply_stimulus(StimulusFlags::HEAT);
        assert!(!mutant.step_reaction(true));
        assert!(mutant.transient.contains(TransientFlags::DORMANT));
    }

    #[test]
    fn vacuum_boiling_and_overpressure_liquefaction() {
        let mut liquid = PhysicalCell {
            env: EnvPacked::new(
                MatterState::Liquid,
                Temperature::Ambient,
                Pressure::Vacuum,
                3,
                BioActivity::Inert,
                FlowDir::None,
            ),
            ..PhysicalCell::default()
        };
        liquid.apply_stimulus(StimulusFlags::LIGHT);
        liquid.step_reaction(false);
        assert_eq!(liquid.env.matter(), MatterState::Gas);
        assert!(liquid.env.density() <= 1);

        liquid.env.set_pressure(Pressure::GigaPressure);
        liquid.apply_stimulus(StimulusFlags::LIGHT);
        liquid.step_reaction(false);
        assert_eq!(liquid.env.matter(), MatterState::Liquid);
        assert!((2..=4).contains(&liquid.env.density()));
    }
}
