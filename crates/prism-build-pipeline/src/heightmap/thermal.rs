//! # Thermal Erosion
//!
//! Material sliding based on repose angle, priority used to flatten extreme slopes.
//! Can be fully parallel, using double buffering to avoid write conflicts.

use super::{clone_data, ErosionParams, Heightmap};
use rayon::prelude::*;

/// Execute one round of thermal erosion.
///
/// Correct each pixel; if difference from neighbor height exceeds repose angle correct max difference value,
/// then transfer excess portion from high pixel to low pixel by `thermal_strength` ratio.
/// Use double buffering (`src` read, `dst` write) to ensure no race.
pub fn thermal_erosion(hm: &mut Heightmap, params: &ErosionParams) {
    let w = hm.width;
    let h = hm.height;
    let max_diff = (params.talus_angle.to_radians().tan()) * params.cell_size;
    let strength = params.thermal_strength;

    let mut dst = clone_data(&hm.data);

    // Double buffering: src = hm.data (read-only), dst = write.
    // Each pixel independent net change calculation (stream out + stream in), rayon parallel no race.
    dst.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, cell) in row.iter_mut().enumerate() {
            let src_h = hm.data[y * w + x];
            let mut net_change: f64 = 0.0;

            // Its own stream out amount: current pixel higher than neighbor and exceeds repose angle threshold
            for &(dx, dy) in &NEIGHBORS_4 {
                let nx = x as isize + dx;
                let ny = y as isize + dy;
                if nx < 0 || nx >= w as isize || ny < 0 || ny >= h as isize {
                    continue;
                }
                let nx = nx as usize;
                let ny = ny as usize;
                let neighbor_h = hm.get(nx, ny);
                let diff = src_h - neighbor_h;

                if diff > max_diff {
                    let amount = (diff - max_diff) * strength * 0.5;
                    net_change -= amount;
                }
            }

            // Neighbor stream into current pixel amount: neighbor higher than current pixel and exceeds repose angle threshold
            for &(dx, dy) in &NEIGHBORS_4 {
                let nx = x as isize + dx;
                let ny = y as isize + dy;
                if nx < 0 || nx >= w as isize || ny < 0 || ny >= h as isize {
                    continue;
                }
                let nx = nx as usize;
                let ny = ny as usize;
                let neighbor_h = hm.get(nx, ny);
                let diff = neighbor_h - src_h;

                if diff > max_diff {
                    let amount = (diff - max_diff) * strength * 0.5;
                    net_change += amount;
                }
            }

            *cell = hm.data[y * w + x] + net_change;
        }
    });

    hm.data = dst;

    // Update min/max
    hm.min_height = hm.data.iter().cloned().fold(f64::INFINITY, f64::min);
    hm.max_height = hm.data.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
}

/// 4-neighbor offset
const NEIGHBORS_4: [(isize, isize); 4] = [(0, -1), (0, 1), (-1, 0), (1, 0)];

#[cfg(test)]
#[path = "thermal_tests.rs"]
mod tests;
