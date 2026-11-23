//! Hydraulic Erosion — particle method
//!
//! Simulates water erosion and deposition on terrain using many particles.
//! Particles are nearly independent; processed with `rayon` parallelism.
//!
//! Key stability guards:
//! - Max velocity cap `max_velocity` (prevent divergence from extreme height drops)
//! - Max sediment capacity cap `max_capacity`
//! - Sea-level erosion multiplier `sea_erosion_factor`

use super::{ErosionParams, Heightmap, Particle};
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use rayon::prelude::*;
use std::f64::consts::PI;

/// Erosion accumulation buffer: one full-image `f64` array per thread.
///
/// [`generate_eroded_heightmap`](super::generate_eroded_heightmap) defaults to running
/// `iterations` (100) erosion iterations; buffer reused here so each round only does
/// `fill(0.0)` instead of realloc + zero. For 2048² grid with 16 threads this is about
/// 536 MB allocate/zero per round.
#[derive(Default)]
pub(crate) struct ErosionScratch {
    bufs: Vec<Vec<f64>>,
    width: usize,
    height: usize,
}

impl ErosionScratch {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Ensure `count` buffers of `w × h` zeros; reuse existing allocation when size unchanged.
    fn prepare(&mut self, count: usize, w: usize, h: usize) {
        if self.width != w || self.height != h {
            self.bufs.clear();
            self.width = w;
            self.height = h;
        }
        let len = w * h;
        self.bufs.resize_with(count, || vec![0.0; len]);
        for buf in &mut self.bufs {
            buf.fill(0.0);
        }
    }
}

/// Plan particle chunking: returns `(chunk_size, buffer_count)`.
///
/// `chunk_size` rounded up, buffer count computed from actual block count, ensuring
/// `chunk_size * buffer_count >= n_particles`—no particle is dropped.
fn chunk_plan(n_particles: usize, n_threads: usize) -> (usize, usize) {
    let n_chunks = n_threads.max(1).min(n_particles);
    if n_chunks == 0 {
        return (1, 0);
    }
    let chunk_size = n_particles.div_ceil(n_chunks);
    (chunk_size, n_particles.div_ceil(chunk_size))
}

/// Run one hydraulic erosion pass using scratch buffer provided by caller.
pub(crate) fn hydraulic_erosion_with_scratch(
    hm: &mut Heightmap,
    params: &ErosionParams,
    scratch: &mut ErosionScratch,
    round_seed: u64,
) {
    let w = hm.width;
    let h = hm.height;
    let sea_level = hm.sea_level;

    if w < 2 || h < 2 || params.particle_count == 0 {
        return;
    }

    let particles: Vec<Particle> = spawn_particles(params.particle_count, w, h, round_seed);

    let (chunk_size, n_buffers) = chunk_plan(particles.len(), rayon::current_num_threads());
    scratch.prepare(n_buffers, w, h);

    {
        let hm_ref: &Heightmap = &*hm;
        let bufs = &mut scratch.bufs;
        particles
            .par_chunks(chunk_size)
            .zip(bufs.par_iter_mut())
            .for_each(|(chunk, buf)| {
                for p in chunk {
                    let mut p = p.clone();
                    process_particle(&mut p, hm_ref, buf, params, w, h, sea_level);
                }
            });
    }

    {
        let bufs = &scratch.bufs;
        hm.data.par_iter_mut().enumerate().for_each(|(i, value)| {
            let mut total = *value;
            for buf in bufs {
                total += buf[i];
            }
            *value = total;
        });
    }

    let (min, max) = hm
        .data
        .par_iter()
        .fold(
            || (f64::INFINITY, f64::NEG_INFINITY),
            |(lo, hi), &v| (lo.min(v), hi.max(v)),
        )
        .reduce(
            || (f64::INFINITY, f64::NEG_INFINITY),
            |(lo_a, hi_a), (lo_b, hi_b)| (lo_a.min(lo_b), hi_a.max(hi_b)),
        );
    hm.min_height = min;
    hm.max_height = max;
}

/// Deterministically generate particles.
fn spawn_particles(count: usize, w: usize, h: usize, seed: u64) -> Vec<Particle> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut particles = Vec::with_capacity(count);
    for _ in 0..count {
        let x = rng.random::<f64>() * (w - 1) as f64;
        let y = rng.random::<f64>() * (h - 1) as f64;
        let water = 0.8 + rng.random::<f64>() * 0.4;
        let mut p = Particle::new(x, y);
        p.water = water;
        p.seed = rng.random::<u64>();
        particles.push(p);
    }
    particles
}

/// Process one particle full lifecycle.
fn process_particle(
    p: &mut Particle,
    hm: &Heightmap,
    buf: &mut [f64],
    params: &ErosionParams,
    w: usize,
    h: usize,
    sea_level: f64,
) {
    let max_vel = params.max_velocity;
    let max_cap = params.max_capacity;
    let gravity = params.gravity;
    let inertia = params.inertia;
    let capacity_factor = params.capacity_factor;
    let min_slope = params.min_slope;
    let deposition_rate = params.deposition_rate;
    let erosion_rate = params.erosion_rate;
    let evaporation_rate = params.evaporation_rate;
    let sea_erosion = params.sea_erosion_factor;

    for _ in 0..params.max_steps {
        let height_here = safe_sample(hm, p.x, p.y, w, h);

        let (grad_x, grad_y) = gradient(hm, p.x, p.y, w, h);

        let dir_x = inertia * p.velocity_x + (1.0 - inertia) * (-grad_x);
        let dir_y = inertia * p.velocity_y + (1.0 - inertia) * (-grad_y);
        let len = (dir_x * dir_x + dir_y * dir_y).sqrt();

        let (dx, dy) = if len > 1e-12 {
            (dir_x / len, dir_y / len)
        } else {
            p.seed = p
                .seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let angle = ((p.seed >> 11) as f64 / (1u64 << 53) as f64) * 2.0 * PI;
            (angle.cos(), angle.sin())
        };

        p.speed = (p.speed + gravity * (height_here - safe_sample(hm, p.x + dx, p.y + dy, w, h)))
            .max(0.0);
        p.speed = p.speed.min(max_vel);

        p.velocity_x = dx * p.speed;
        p.velocity_y = dy * p.speed;

        let new_x = p.x + dx;
        let new_y = p.y + dy;

        if new_x < 0.0 || new_x >= (w - 1) as f64 || new_y < 0.0 || new_y >= (h - 1) as f64 {
            break;
        }

        let height_new = safe_sample(hm, new_x, new_y, w, h);
        let height_diff = height_here - height_new;

        let capacity =
            (capacity_factor * p.speed * (height_diff.abs().max(min_slope)) * p.water).min(max_cap);

        if p.sediment > capacity || height_diff < 0.0 {
            let deposit = (p.sediment - capacity) * deposition_rate;
            p.sediment -= deposit;
            scatter(buf, p.x, p.y, deposit, w, h);
        } else {
            let mut erode_amount = (capacity - p.sediment) * erosion_rate;
            erode_amount = erode_amount.min(height_diff);

            let here_height = safe_sample(hm, p.x, p.y, w, h);
            if here_height < sea_level {
                erode_amount *= sea_erosion;
            }

            p.sediment += erode_amount;
            scatter(buf, p.x, p.y, -erode_amount, w, h);
        }

        p.water *= 1.0 - evaporation_rate;
        p.x = new_x;
        p.y = new_y;

        if p.water < 0.001 {
            break;
        }
    }
}

/// Diffuse `amount` by distance-weighted spread into 3×3 neighborhood around `(x, y)`.
///
/// Center weight max, four neighbors next, four corners min; direct per-pixel read/write produces salt-and-pepper noise,
/// Diffusion writes let erosion/deposition act on contiguous regions, preserving large-scale morphology.
fn scatter(buf: &mut [f64], x: f64, y: f64, amount: f64, w: usize, h: usize) {
    let cx = x as usize;
    let cy = y as usize;
    let weights = [
        (cx.wrapping_sub(1), cy.wrapping_sub(1), 1.0 / 18.0),
        (cx, cy.wrapping_sub(1), 1.0 / 9.0),
        (cx + 1, cy.wrapping_sub(1), 1.0 / 18.0),
        (cx.wrapping_sub(1), cy, 1.0 / 9.0),
        (cx, cy, 4.0 / 9.0),
        (cx + 1, cy, 1.0 / 9.0),
        (cx.wrapping_sub(1), cy + 1, 1.0 / 18.0),
        (cx, cy + 1, 1.0 / 9.0),
        (cx + 1, cy + 1, 1.0 / 18.0),
    ];
    for (px, py, weight) in weights {
        if px < w && py < h {
            buf[py * w + px] += amount * weight;
        }
    }
}

/// Safe bilinear sampling.
fn safe_sample(hm: &Heightmap, x: f64, y: f64, _w: usize, _h: usize) -> f64 {
    hm.sample(x, y)
}

/// Compute gradient at (x, y) using finite differences.
fn gradient(hm: &Heightmap, x: f64, y: f64, w: usize, h: usize) -> (f64, f64) {
    let eps = 0.5_f64.min(1.0);
    let x0 = (x - eps).max(0.0);
    let x1 = (x + eps).min((w - 1) as f64);
    let y0 = (y - eps).max(0.0);
    let y1 = (y + eps).min((h - 1) as f64);

    let h_l = safe_sample(hm, x0, y, w, h);
    let h_r = safe_sample(hm, x1, y, w, h);
    let h_d = safe_sample(hm, x, y0, w, h);
    let h_u = safe_sample(hm, x, y1, w, h);

    let gx = if (x1 - x0).abs() > 1e-12 {
        (h_r - h_l) / (x1 - x0)
    } else {
        0.0
    };
    let gy = if (y1 - y0).abs() > 1e-12 {
        (h_u - h_d) / (y1 - y0)
    } else {
        0.0
    };

    (gx, gy)
}

#[cfg(test)]
#[path = "hydraulic_tests.rs"]
mod tests;
