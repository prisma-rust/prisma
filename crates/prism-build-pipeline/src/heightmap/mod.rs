//! # High-degree terrain generator
//!
//! Parameterized ultra-height-difference simulated water+thermal erosion high-degree terrain generator.
//! Supports −11,000 m to +8,850 m (total drop ≈ 20 km).
//!
//! ## Usage
//! ```rust,ignore
//! let hm = Heightmap::new(width, height, data);
//! let params = ErosionParams::default();
//! let result = generate_eroded_heightmap(hm, &params);
//! ```

mod hydraulic;
mod terrain;
mod thermal;

pub use terrain::generate_terrain;
pub use thermal::thermal_erosion;

/// Heightgraph, using `f64` to guarantee 20 km height difference precision.
#[derive(Clone, Debug)]
pub struct Heightmap {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f64>,
    /// Sea level height (relative height mode below is 0.0).
    pub sea_level: f64,
    /// Global lowest point (relative / absolute).
    pub min_height: f64,
    /// Global highest point (relative / absolute).
    pub max_height: f64,
}

impl Heightmap {
    /// Create from `Vec<f64>`, automatically calculate min/max.
    pub fn new(width: usize, height: usize, data: Vec<f64>) -> Self {
        assert_eq!(
            data.len(),
            width * height,
            "data length must match width × height"
        );
        let mut min_h = f64::INFINITY;
        let mut max_h = f64::NEG_INFINITY;
        for &v in &data {
            if v < min_h {
                min_h = v;
            }
            if v > max_h {
                max_h = v;
            }
        }
        Self {
            width,
            height,
            data,
            sea_level: 0.0,
            min_height: min_h,
            max_height: max_h,
        }
    }

    /// Get height at (x, y) (x ∈ [0, width), y ∈ [0, height)).
    #[inline]
    pub fn get(&self, x: usize, y: usize) -> f64 {
        self.data[y * self.width + x]
    }

    /// Set height at (x, y), automatically update min/max.
    #[inline]
    pub fn set(&mut self, x: usize, y: usize, value: f64) {
        let idx = y * self.width + x;
        self.data[idx] = value;
        if value < self.min_height {
            self.min_height = value;
        }
        if value > self.max_height {
            self.max_height = value;
        }
    }

    /// Bilinear interpolated sample (supports floating-point coordinates).
    pub fn sample(&self, x: f64, y: f64) -> f64 {
        let w = self.width as f64;
        let h = self.height as f64;
        let x = x.clamp(0.0, w - 1.001);
        let y = y.clamp(0.0, h - 1.001);

        let x0 = x as usize;
        let y0 = y as usize;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let fx = x - x0 as f64;
        let fy = y - y0 as f64;

        let h00 = self.get(x0, y0);
        let h10 = self.get(x1, y0);
        let h01 = self.get(x0, y1);
        let h11 = self.get(x1, y1);

        let top = h00 + (h10 - h00) * fx;
        let bot = h01 + (h11 - h01) * fx;
        top + (bot - top) * fy
    }

    /// Convert to relative height: subtract `min_height` from all values, making min value 0.
    pub fn to_relative(&mut self) {
        let min = self.min_height;
        if min == 0.0 {
            return;
        }
        for v in self.data.iter_mut() {
            *v -= min;
        }
        self.max_height -= min;
        self.min_height = 0.0;
    }

    /// Restore absolute height: add saved min value to all values.
    pub fn from_relative(&mut self, saved_min: f64) {
        if saved_min == 0.0 {
            return;
        }
        for v in self.data.iter_mut() {
            *v += saved_min;
        }
        self.min_height += saved_min;
        self.max_height += saved_min;
    }

    /// Dimensionless: scale to [0, 1] range.
    pub fn normalize(&mut self) {
        let range = self.max_height - self.min_height;
        if range <= 0.0 {
            return;
        }
        for v in self.data.iter_mut() {
            *v = (*v - self.min_height) / range;
        }
        self.max_height = 1.0;
        self.min_height = 0.0;
    }

    /// Restore from [0, 1] to specified range.
    pub fn denormalize(&mut self, new_min: f64, new_max: f64) {
        let range = new_max - new_min;
        for v in self.data.iter_mut() {
            *v = *v * range + new_min;
        }
        self.min_height = new_min;
        self.max_height = new_max;
    }
}

/// Erosion parameters, key physical parameters adjustable in real time.
#[derive(Clone, Debug)]
pub struct ErosionParams {
    pub inertia: f64,
    pub capacity_factor: f64,
    pub min_slope: f64,
    pub deposition_rate: f64,
    pub erosion_rate: f64,
    pub evaporation_rate: f64,
    pub gravity: f64,
    /// Speed hard limit, prevent divergence at excessive speed (key parameter).
    pub max_velocity: f64,
    /// Deposition capacity hard limit (key parameter).
    pub max_capacity: f64,

    /// Repose angle (degrees).
    pub talus_angle: f64,
    pub thermal_strength: f64,

    pub iterations: usize,
    pub particle_count: usize,
    pub max_steps: usize,
    pub cell_size: f64,
    /// Random seed (for hydraulic erosion particle generation and random walk, ensures same parameter output determinism).
    pub seed: u64,
    /// Thermal erosion interval (thermal once every N hydraulic rounds).
    pub thermal_interval: usize,
    /// Preprocess thermal rounds.
    pub thermal_pre_iterations: usize,
    /// Sea level erodes at below rate (0.0 ~ 1.0).
    pub sea_erosion_factor: f64,

    pub use_relative_height: bool,
}

impl Default for ErosionParams {
    fn default() -> Self {
        Self {
            inertia: 0.05,
            capacity_factor: 4.0,
            min_slope: 0.01,
            deposition_rate: 0.3,
            erosion_rate: 0.3,
            evaporation_rate: 0.01,
            gravity: 4.0,
            max_velocity: 20.0,
            max_capacity: 10.0,
            talus_angle: 35.0,
            thermal_strength: 0.5,
            iterations: 100,
            particle_count: 200_000,
            max_steps: 150,
            cell_size: 1.0,
            seed: 0x5EED,
            thermal_interval: 5,
            thermal_pre_iterations: 100,
            sea_erosion_factor: 0.05,
            use_relative_height: true,
        }
    }
}

/// Hydraulic erosion particle.
#[derive(Clone, Debug)]
pub(crate) struct Particle {
    pub x: f64,
    pub y: f64,
    pub velocity_x: f64,
    pub velocity_y: f64,
    pub water: f64,
    pub sediment: f64,
    pub speed: f64,
    pub seed: u64,
}

impl Particle {
    pub fn new(x: f64, y: f64) -> Self {
        Self {
            x,
            y,
            velocity_x: 0.0,
            velocity_y: 0.0,
            water: 1.0,
            sediment: 0.0,
            speed: 0.0,
            seed: 0,
        }
    }
}

/// Execute complete erosion pipeline: thermal preprocessing → hydraulic + thermal alternating iteration.
///
/// Return height graph after erosion. If `params.use_relative_height` is true,
/// Internal calculation converts to relative height, output retains relative height. Otherwise restores original absolute height.
pub fn generate_eroded_heightmap(mut hm: Heightmap, params: &ErosionParams) -> Heightmap {
    let saved_min = hm.min_height;

    if params.use_relative_height {
        hm.to_relative();
    }

    for _ in 0..params.thermal_pre_iterations {
        thermal_erosion(&mut hm, params);
    }

    let mut scratch = hydraulic::ErosionScratch::new();
    for iter in 0..params.iterations {
        let round_seed = params
            .seed
            .wrapping_add((iter as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        hydraulic::hydraulic_erosion_with_scratch(&mut hm, params, &mut scratch, round_seed);
        if params.thermal_interval > 0 && (iter + 1) % params.thermal_interval == 0 {
            thermal_erosion(&mut hm, params);
        }
    }

    if !params.use_relative_height {
        hm.from_relative(saved_min);
    }

    hm
}

/// Double buffer copy
pub(crate) fn clone_data(data: &[f64]) -> Vec<f64> {
    data.to_vec()
}

#[cfg(test)]
mod tests;
