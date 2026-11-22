//! CPU skinning reference implementation.
//!
//! The GPU path (P0-3) does the same thing, except matrices and vertices live in device memory;
//! this implementation is the **golden baseline**: feeding the same palette / joints / weights
//! into both must yield identical results.

use glam::{Mat4, Vec3};

/// A vertex's blend matrix: `sum(w[i] * palette[joints[i]])` (weights normalized to sum to 1).
///
/// Vertices with weight sum 0 degenerate to the identity matrix (stay put instead of collapsing
/// to the origin).
pub fn skin_matrix(palette: &[Mat4], joints: &[u16; 4], weights: &[f32; 4]) -> Mat4 {
    let mut matrix = Mat4::ZERO;
    let mut total = 0.0f32;
    for slot in 0..4 {
        let weight = weights[slot];
        if weight <= 0.0 {
            continue;
        }
        let Some(palette_matrix) = palette.get(usize::from(joints[slot])) else {
            continue;
        };
        matrix += *palette_matrix * weight;
        total += weight;
    }
    if total <= f32::EPSILON {
        return Mat4::IDENTITY;
    }
    if (total - 1.0).abs() > 1e-4 {
        matrix *= 1.0 / total;
    }
    matrix
}

/// Batch-compute each vertex's blend matrix (P0-3 can compare this directly against GPU palette sampling).
pub fn skin_matrices(palette: &[Mat4], joints: &[[u16; 4]], weights: &[[f32; 4]]) -> Vec<Mat4> {
    let count = joints.len().min(weights.len());
    (0..count)
        .map(|index| skin_matrix(palette, &joints[index], &weights[index]))
        .collect()
}

/// Skinned position: `w * matrix * vec4(position, 1)`.
///
/// When `joints` / `weights` are shorter than `positions`, the extra vertices are kept as-is.
pub fn skin_positions(
    palette: &[Mat4],
    joints: &[[u16; 4]],
    weights: &[[f32; 4]],
    positions: &[[f32; 3]],
) -> Vec<[f32; 3]> {
    let count = joints.len().min(weights.len()).min(positions.len());
    let mut out = Vec::with_capacity(positions.len());
    for (index, position) in positions.iter().enumerate() {
        if index >= count {
            out.push(*position);
            continue;
        }
        let matrix = skin_matrix(palette, &joints[index], &weights[index]);
        let skinned = matrix.transform_point3(Vec3::from_array(*position));
        out.push(skinned.to_array());
    }
    out
}

/// Skinned normal: only the 3×3 of the blend matrix (translation removed), then normalized.
pub fn skin_normals(
    palette: &[Mat4],
    joints: &[[u16; 4]],
    weights: &[[f32; 4]],
    normals: &[[f32; 3]],
) -> Vec<[f32; 3]> {
    let count = joints.len().min(weights.len()).min(normals.len());
    let mut out = Vec::with_capacity(normals.len());
    for (index, normal) in normals.iter().enumerate() {
        if index >= count {
            out.push(*normal);
            continue;
        }
        let matrix = skin_matrix(palette, &joints[index], &weights[index]);
        let rotated = matrix.transform_vector3(Vec3::from_array(*normal));
        let length = rotated.length();
        out.push(if length > 1e-6 {
            (rotated / length).to_array()
        } else {
            *normal
        });
    }
    out
}

/// Raw array form of the palette (for uploading to GPU).
pub fn palette_to_arrays(palette: &[Mat4]) -> Vec<[[f32; 4]; 4]> {
    palette
        .iter()
        .map(|matrix| matrix.to_cols_array_2d())
        .collect()
}

#[cfg(test)]
#[path = "skinning_tests.rs"]
mod tests;
