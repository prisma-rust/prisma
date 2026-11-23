//! Chunked 2D simulation grid with u64 active bitmaps and multi-rate stepping.
//!
//! Each `8x8` chunk tracks a `u64` active mask so fully `DORMANT` regions are
//! skipped in O(1) without touching individual `PhysicalCell` payloads.

use crate::tags::{
    BioActivity, EnvPacked, FlowDir, MaterialFlags, MatterState, PhysicalCell, Pressure,
    StimulusFlags, Temperature, TopologyFlags, TransientFlags,
};

/// Width and height of one simulation chunk in cells (`8 * 8 = 64` bits).
pub const CHUNK_SIZE: usize = 8;

#[derive(Debug, Clone, Copy, Default)]
struct ChunkMeta {
    active_mask: u64,
}

/// Deterministic 2D chunked grid of 16-byte `PhysicalCell` payloads.
pub struct CellGrid {
    width: usize,
    height: usize,
    chunks_x: usize,
    chunks_y: usize,
    cells: Vec<PhysicalCell>,
    chunks: Vec<ChunkMeta>,
}

impl CellGrid {
    /// Allocates a grid of `width x height` cells initialized to `DORMANT` vacuum.
    pub fn new(width: usize, height: usize) -> Self {
        let chunks_x = width.div_ceil(CHUNK_SIZE);
        let chunks_y = height.div_ceil(CHUNK_SIZE);
        let default_cell = PhysicalCell {
            env: EnvPacked::new(
                MatterState::Gas,
                Temperature::Ambient,
                Pressure::HardVacuum,
                0,
                BioActivity::Inert,
                FlowDir::None,
            ),
            transient: TransientFlags::DORMANT,
            ..PhysicalCell::default()
        };
        Self {
            width,
            height,
            chunks_x,
            chunks_y,
            cells: vec![default_cell; width * height],
            chunks: vec![ChunkMeta::default(); chunks_x * chunks_y],
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn cells(&self) -> &[PhysicalCell] {
        &self.cells
    }

    /// Returns how many `8x8` chunks currently have at least one active cell.
    pub fn active_chunk_count(&self) -> usize {
        self.chunks.iter().filter(|c| c.active_mask != 0).count()
    }

    /// Returns how many cells are currently marked active in chunk bitmaps.
    pub fn active_cell_count(&self) -> usize {
        self.chunks
            .iter()
            .map(|c| c.active_mask.count_ones() as usize)
            .sum()
    }

    pub fn get(&self, x: usize, y: usize) -> Option<&PhysicalCell> {
        if x < self.width && y < self.height {
            Some(&self.cells[y * self.width + x])
        } else {
            None
        }
    }

    /// Replaces a cell at `(x, y)` and updates its chunk active bitmap.
    pub fn set_cell(&mut self, x: usize, y: usize, cell: PhysicalCell) {
        if x >= self.width || y >= self.height {
            return;
        }
        let idx = y * self.width + x;
        self.cells[idx] = cell;
        let is_active =
            !cell.transient.contains(TransientFlags::DORMANT) || !cell.pending_stimuli.is_empty();
        self.set_chunk_bit(x, y, is_active);
    }

    /// Injects an external stimulus at `(x, y)` and wakes its chunk immediately.
    pub fn apply_stimulus(&mut self, x: usize, y: usize, stimulus: StimulusFlags) {
        if x >= self.width || y >= self.height || stimulus.is_empty() {
            return;
        }
        let idx = y * self.width + x;
        self.cells[idx].apply_stimulus(stimulus);
        self.set_chunk_bit(x, y, true);
    }

    /// Advances all active chunks by one deterministic reaction + propagation step.
    pub fn step_reaction(&mut self, advance_clock_tick: bool) {
        for cy in 0..self.chunks_y {
            for cx in 0..self.chunks_x {
                let chunk_idx = cy * self.chunks_x + cx;
                let mut mask = self.chunks[chunk_idx].active_mask;
                if mask == 0 {
                    continue;
                }
                self.chunks[chunk_idx].active_mask = 0;

                while mask != 0 {
                    let bit = mask.trailing_zeros() as usize;
                    mask &= mask - 1;
                    let lx = bit % CHUNK_SIZE;
                    let ly = bit / CHUNK_SIZE;
                    let x = cx * CHUNK_SIZE + lx;
                    let y = cy * CHUNK_SIZE + ly;
                    if x >= self.width || y >= self.height {
                        continue;
                    }
                    let idx = y * self.width + x;
                    if self.cells[idx].step_reaction(advance_clock_tick) {
                        self.chunks[chunk_idx].active_mask |= 1u64 << bit;
                    }
                    self.propagate_from(x, y);
                }
            }
        }
    }

    /// Runs one `4~5 Hz` buoyancy stratification pass (lighter fluids/gases rise).
    pub fn step_buoyancy(&mut self) {
        if self.height < 2 {
            return;
        }
        for parity in 0..2 {
            let mut y = parity;
            while y + 1 < self.height {
                for x in 0..self.width {
                    let upper_idx = y * self.width + x;
                    let lower_idx = (y + 1) * self.width + x;
                    let upper = self.cells[upper_idx];
                    let lower = self.cells[lower_idx];

                    if !can_swap_buoyancy(&upper, &lower) {
                        continue;
                    }
                    self.cells.swap(upper_idx, lower_idx);
                    self.set_chunk_bit(x, y, is_cell_active(&self.cells[upper_idx]));
                    self.set_chunk_bit(x, y + 1, is_cell_active(&self.cells[lower_idx]));
                }
                y += 2;
            }
        }
    }

    fn propagate_from(&mut self, x: usize, y: usize) {
        let src = self.cells[y * self.width + x];
        let mut emit = StimulusFlags::NONE;

        if src.transient.contains(TransientFlags::BURNING)
            || src.env.temperature() >= Temperature::Hot
        {
            emit.insert(StimulusFlags::HEAT);
        }
        if src.env.temperature() <= Temperature::Cryo {
            emit.insert(StimulusFlags::COLD);
        }
        if src.transient.contains(TransientFlags::CORROSIVE) {
            emit.insert(StimulusFlags::ACID);
        }
        if src.transient.contains(TransientFlags::INFECTED)
            || src.env.bio_activity() == BioActivity::HyperNeo
        {
            emit.insert(StimulusFlags::BIO);
        }
        if src.transient.contains(TransientFlags::RADIOACTIVE) {
            emit.insert(StimulusFlags::RADIATION);
        }
        if src.transient.contains(TransientFlags::WET)
            && src.env.temperature() <= Temperature::Ambient
        {
            emit.insert(StimulusFlags::COLD);
        }

        let shock_intensity = if src.transient.contains(TransientFlags::CHARGED)
            && !src.transient.contains(TransientFlags::BROKEN)
        {
            src.clocks.intensity()
        } else {
            0
        };

        if emit.is_empty() && shock_intensity <= 1 {
            return;
        }

        const DIRS: [(isize, isize); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
        for (dx, dy) in DIRS {
            let nx = x as isize + dx;
            let ny = y as isize + dy;
            if nx < 0 || ny < 0 {
                continue;
            }
            let (nx, ny) = (nx as usize, ny as usize);
            if nx >= self.width || ny >= self.height {
                continue;
            }
            let n_idx = ny * self.width + nx;
            let neighbor = &mut self.cells[n_idx];
            if neighbor.transient.contains(TransientFlags::STASIS) {
                continue;
            }

            let mut woke = false;
            if !emit.is_empty() {
                neighbor.apply_stimulus(emit);
                woke = true;
            }

            if shock_intensity > 1
                && !neighbor.transient.contains(TransientFlags::BROKEN)
                && (neighbor.material.contains(MaterialFlags::CONDUCTIVE)
                    || neighbor.transient.contains(TransientFlags::WET))
            {
                let attenuated = shock_intensity - 1;
                if neighbor.clocks.intensity() < attenuated {
                    neighbor.clocks.set_intensity(attenuated);
                }
                neighbor.apply_stimulus(StimulusFlags::SHOCK);
                woke = true;
            }

            if woke {
                self.set_chunk_bit(nx, ny, true);
            }
        }
    }

    fn set_chunk_bit(&mut self, x: usize, y: usize, active: bool) {
        let cx = x / CHUNK_SIZE;
        let cy = y / CHUNK_SIZE;
        let lx = x % CHUNK_SIZE;
        let ly = y % CHUNK_SIZE;
        let bit = 1u64 << (ly * CHUNK_SIZE + lx);
        let chunk = &mut self.chunks[cy * self.chunks_x + cx];
        if active {
            chunk.active_mask |= bit;
        } else {
            chunk.active_mask &= !bit;
        }
    }
}

fn is_cell_active(cell: &PhysicalCell) -> bool {
    !cell.transient.contains(TransientFlags::DORMANT) || !cell.pending_stimuli.is_empty()
}

fn can_swap_buoyancy(upper: &PhysicalCell, lower: &PhysicalCell) -> bool {
    if lower.env.matter() == MatterState::Solid {
        return false;
    }
    if upper.env.matter() == MatterState::Solid && !upper.material.contains(MaterialFlags::GRANULAR)
    {
        return false;
    }
    if upper.topology.contains(TopologyFlags::RIGID_WELD)
        || lower.topology.contains(TopologyFlags::RIGID_WELD)
        || upper.transient.contains(TransientFlags::STASIS)
        || lower.transient.contains(TransientFlags::STASIS)
    {
        return false;
    }
    lower.env.density() < upper.env.density()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dormant_chunks_are_skipped_until_stimulus_wakes_them() {
        let mut grid = CellGrid::new(16, 16);
        assert_eq!(grid.active_chunk_count(), 0);
        assert_eq!(grid.active_cell_count(), 0);

        grid.step_reaction(true);
        assert_eq!(grid.active_chunk_count(), 0);

        grid.apply_stimulus(9, 9, StimulusFlags::HEAT);
        assert_eq!(grid.active_chunk_count(), 1);
        assert_eq!(grid.active_cell_count(), 1);
    }

    #[test]
    fn wet_floor_conducts_shock_and_overloads_wire() {
        let mut grid = CellGrid::new(8, 8);
        let mut source = PhysicalCell {
            material: MaterialFlags::METALLIC.union(MaterialFlags::CONDUCTIVE),
            transient: TransientFlags::CHARGED,
            ..PhysicalCell::default()
        };
        source.clocks.set_intensity(8);

        let wet_tile = PhysicalCell {
            material: MaterialFlags::INSULATED,
            transient: TransientFlags::WET,
            ..PhysicalCell::default()
        };

        let wire = PhysicalCell {
            material: MaterialFlags::METALLIC.union(MaterialFlags::CONDUCTIVE),
            ..PhysicalCell::default()
        };

        grid.set_cell(1, 1, source);
        grid.set_cell(2, 1, wet_tile);
        grid.set_cell(3, 1, wire);

        grid.step_reaction(false);
        grid.step_reaction(false);

        let wet = grid.get(2, 1).expect("wet cell exists");
        let downstream = grid.get(3, 1).expect("wire cell exists");
        assert!(wet.transient.contains(TransientFlags::CHARGED));
        assert!(downstream.transient.contains(TransientFlags::CHARGED));
        assert_eq!(downstream.clocks.intensity(), 6);
    }

    #[test]
    fn buoyancy_stratifies_gas_above_liquid() {
        let mut grid = CellGrid::new(4, 4);
        let liquid = PhysicalCell {
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
        let gas = PhysicalCell {
            env: EnvPacked::new(
                MatterState::Gas,
                Temperature::Ambient,
                Pressure::Normal,
                0,
                BioActivity::Inert,
                FlowDir::None,
            ),
            ..PhysicalCell::default()
        };

        grid.set_cell(1, 0, liquid);
        grid.set_cell(1, 1, gas);
        grid.set_cell(1, 2, PhysicalCell::default());

        grid.step_buoyancy();

        assert_eq!(
            grid.get(1, 0).expect("upper").env.matter(),
            MatterState::Gas
        );
        assert_eq!(
            grid.get(1, 1).expect("lower").env.matter(),
            MatterState::Liquid
        );
    }
}
