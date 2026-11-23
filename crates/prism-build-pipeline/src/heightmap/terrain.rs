//! Initial terrain generation: seeded value noise + FBM.
//!
//! Generate [0, 1] relative height field, caller available [`Heightmap::denormalize`]
//! map to true real height range (like −11,000 m ~ +8,850 m).

use super::Heightmap;

// ---------------------------------------------------------------------------
// Hash / noise
// ---------------------------------------------------------------------------

/// SplitMix64 finalizer: integer-only, so every platform produces the same bits.
#[inline]
fn mix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// 2D value noise hash, seed mixed into integer blend. Returns [0, 1).
///
/// Cannot again use `f64::sin`: libm implementation varies by platform, same seed in
/// Windows / Linux / Android will get different terrain; large coordinates will lose precision.
#[inline]
fn hash2(p: [f64; 2], seed: u64) -> f64 {
    let ix = p[0].floor() as i64 as u64;
    let iy = p[1].floor() as i64 as u64;
    let h = mix64(ix ^ mix64(iy ^ mix64(seed)));
    // Keep the 53 bits an f64 mantissa can represent exactly.
    (h >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
}

/// 2D value noise, returns [−1, 1].
#[inline]
fn noise_2d(p: [f64; 2], seed: u64) -> f64 {
    let i = [p[0].floor(), p[1].floor()];
    let w = [p[0] - i[0], p[1] - i[1]];
    // Smooth interpolation (3rd-order Hermite).
    let u = [
        w[0] * w[0] * (3.0 - 2.0 * w[0]),
        w[1] * w[1] * (3.0 - 2.0 * w[1]),
    ];

    let a = hash2(i, seed);
    let b = hash2([i[0] + 1.0, i[1]], seed);
    let c = hash2([i[0], i[1] + 1.0], seed);
    let d = hash2([i[0] + 1.0, i[1] + 1.0], seed);

    -1.0 + 2.0 * (a + (b - a) * u[0] + (c - a) * u[1] + (a - b - c + d) * u[0] * u[1])
}

/// Fractional Brownian motion: many octaves superimposed, returns approximately [−1, 1].
///
/// `gain` — amplitude decay per octave (typical 0.5); `lacunarity` — frequency multiplier (typical 2.0).
fn fbm(p: [f64; 2], octaves: u32, gain: f64, lacunarity: f64, seed: u64) -> f64 {
    let mut p = p;
    let mut amplitude = 1.0;
    let mut total = 0.0;
    let mut norm = 0.0;
    for _ in 0..octaves {
        total += amplitude * noise_2d(p, seed);
        norm += amplitude;
        amplitude *= gain;
        p = [p[0] * lacunarity, p[1] * lacunarity];
    }
    total / norm
}

// ---------------------------------------------------------------------------
// Terrain synthesis
// ---------------------------------------------------------------------------

/// Generate [0, 1] relative height initial terrain.
///
/// Frequency calibrated by **physical tile size**: `scale_m` is tile width (meters).
/// 512 m tile baseline (`freq_scale = 1.0`):
/// - Low frequency skeleton `base` (~1.5 period): large-scale undulation about 340 m
/// - Ridge layer `ridge` = 1 − |fbm| (~2.5 period): ridge interval about 200 m
/// - Detail layer `detail` (~10 period): valley texture about 50 m
/// - Strong domain distortion makes ridges naturally meander
///
/// Larger tile means lower frequency, maintaining same physical feature period.
pub fn generate_terrain(width: usize, height: usize, seed: u64, scale_m: f64) -> Heightmap {
    // Frequency scale: 512 m tile → 1.0; 1024 m → 0.5 (same physical period).
    let freq = 512.0 / scale_m.max(1.0);

    // Each layer uses different seed bitwise, avoid all octaves aligning to produce mesh feel.
    let seed_a = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let seed_b = seed.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let seed_c = seed.wrapping_mul(0x94D0_49BB_1331_11EB);

    let mut data = Vec::with_capacity(width * height);
    for y in 0..height {
        let ny = y as f64 / height as f64;
        for x in 0..width {
            let nx = x as f64 / width as f64;
            // Domain distortion: low frequency noise offsets sample coordinates, ridge direction meanders with it.
            let warp = 0.5 * fbm([nx * 2.0, ny * 2.0], 4, 0.5, 2.0, seed_a);
            let q = [nx + warp, ny + warp];

            let base = fbm([q[0] * 1.5 * freq, q[1] * 1.5 * freq], 5, 0.5, 2.0, seed_a);
            // Ridge noise: |fbm| absolute → f=0 is ridge line (1.0), both sides fall off (0.0).
            let ridge =
                1.0 - fbm([q[0] * 2.5 * freq, q[1] * 2.5 * freq], 5, 0.5, 2.0, seed_b).abs();
            let detail = fbm(
                [q[0] * 10.0 * freq, q[1] * 10.0 * freq],
                3,
                0.5,
                2.0,
                seed_c,
            );

            // base ∈ [−1,1]; ridge ∈ [0,1] → translate/scale back to [−1,1]; detail ∈ [−1,1]
            let e = 0.40 * base + 0.45 * (2.0 * ridge - 1.0) + 0.15 * detail;
            data.push(e);
        }
    }

    let mut hm = Heightmap::new(width, height, data);
    // Normalize to [0, 1], ensure output range consistent for different seeds.
    hm.normalize();
    hm
}

#[cfg(test)]
#[path = "terrain_tests.rs"]
mod tests;
