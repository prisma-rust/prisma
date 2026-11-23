//! 2D blend space (velocity × direction, `#560 P1`).
//!
//! The author provides a set of samples `(x, y) → clip`; at runtime a point on the parameter
//! plane is used to compute the pose. Like the 1D blend space, this layer only does math and
//! does not depend on engine / render / ECS.
//!
//! Evaluation is two steps:
//!
//! 1. Triangulate the sample points (Delaunay). Triangulation is computed once in
//!    [`BlendSpace2D::new`]; each frame only does "find triangle + barycentric interpolation";
//! 2. When barycentric coordinates fall outside the convex hull, negative weights are clamped
//!    to 0 and renormalized — the effect is projection onto the nearest convex hull boundary,
//!    rather than snapping the character back to some sample pose.
//!
//! Fewer than 3 samples, all collinear, or non-finite positions mean no usable triangle;
//! `evaluate_into` returns `false` and the caller falls back to 1D / single clip.
//!
//! Coordinate convention: `x` usually corresponds to speed (walk / run), `y` to direction
//! (front / back / left / right); units are decided by the author, only relative positions
//! are compared here.

use std::sync::Arc;

use crate::blend::{blend_into, sample_at_phase};
use crate::clip::Clip;
use crate::pose::Pose;
use crate::sync::SyncGroup;

/// One sample point of the 2D blend space: `position` is the coordinate on the parameter plane, `clip` is the corresponding clip.
#[derive(Debug, Clone)]
pub struct BlendSample2D {
    pub position: [f32; 2],
    pub clip: Arc<Clip>,
}

/// 2D blend space.
///
/// Like [`BlendSpace1D`](crate::blend::BlendSpace1D), each clip's pose is reset to `reference`
/// before sampling: when a clip only covers some tracks, uncovered joints keep reference and
/// do not collapse to identity.
#[derive(Debug, Clone, Default)]
pub struct BlendSpace2D {
    samples: Vec<BlendSample2D>,
    /// Triangulation result; elements are indices into `samples`, triangles arranged counter-clockwise.
    triangles: Vec<[usize; 3]>,
    reference: Pose,
    /// When all samples have sync markers, use the first sample as the leading clip for phase mapping.
    sync: Option<SyncGroup>,
    /// Three sample poses + one accumulation buffer; reused each frame to avoid allocation during evaluation.
    scratch: [Pose; 4],
}

impl BlendSpace2D {
    /// Build a 2D blend space.
    ///
    /// - Drop samples with non-finite positions (NaN / ∞);
    /// - Keep only the first sample at the same coordinate (duplicate points would degenerate
    ///   triangulation), preserving the author's order;
    /// - No triangles are produced when fewer than 3 non-collinear points.
    pub fn new(samples: Vec<BlendSample2D>, reference: Pose) -> Self {
        let mut unique: Vec<BlendSample2D> = Vec::with_capacity(samples.len());
        for sample in samples {
            if !sample.position[0].is_finite() || !sample.position[1].is_finite() {
                continue;
            }
            let duplicate = unique.iter().any(|kept| kept.position == sample.position);
            if !duplicate {
                unique.push(sample);
            }
        }
        let points: Vec<[f64; 2]> = unique
            .iter()
            .map(|sample| [sample.position[0] as f64, sample.position[1] as f64])
            .collect();
        let triangles = triangulate(&points);
        let sync = unique
            .iter()
            .map(|sample| sample.clip.sync_marker_set())
            .collect::<Option<Vec<_>>>()
            .and_then(SyncGroup::new)
            .filter(|group| group.len() == unique.len());
        Self {
            samples: unique,
            triangles,
            reference,
            sync,
            scratch: std::array::from_fn(|_| Pose::default()),
        }
    }

    /// The normalized phase actually used for sample `index` (same as [`BlendSpace1D`](crate::blend::BlendSpace1D)).
    /// `leader` is the sample with the highest current weight.
    fn phase_for(&self, leader: usize, index: usize, phase: f32) -> f32 {
        self.sync
            .as_ref()
            .filter(|group| leader < group.len() && index < group.len())
            .map_or(phase, |group| group.map_phase(leader, index, phase))
    }

    pub fn reference(&self) -> &Pose {
        &self.reference
    }

    pub fn samples(&self) -> &[BlendSample2D] {
        &self.samples
    }

    /// Triangulation result (indices into `samples`, counter-clockwise). Editors can use it to draw the sample grid.
    pub fn triangles(&self) -> &[[usize; 3]] {
        &self.triangles
    }

    /// Index of the highest-weight sample at the current parameter (used by Notify's Dominant strategy).
    ///
    /// When weights are equal, take the earlier vertex in the triangle; returns `None` when no triangle is available.
    pub fn dominant_sample(&self, at: [f32; 2]) -> Option<usize> {
        let (vertices, weights) = self.weights_at(at)?;
        let (slot, _) = weights
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.total_cmp(b))?;
        Some(vertices[slot])
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty() || self.triangles.is_empty()
    }

    /// Evaluate at `at` on the parameter plane into `out`.
    ///
    /// Returns `false` while **not touching** `out` (no available triangle / sampling failure),
    /// so the caller can keep the previous frame's pose.
    pub fn evaluate_into(&mut self, at: [f32; 2], phase: f32, out: &mut Pose) -> bool {
        let Some((vertices, weights)) = self.weights_at(at) else {
            return false;
        };

        let leader_slot = weights
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.total_cmp(b))
            .map(|(slot, _)| slot)
            .unwrap_or(0);
        let leader = vertices[leader_slot];
        for (slot, vertex) in vertices.iter().enumerate() {
            self.scratch[slot].copy_from(&self.reference);
            let sample = &self.samples[*vertex];
            let phase = self.phase_for(leader, *vertex, phase);
            if !sample_at_phase(&sample.clip, phase, &mut self.scratch[slot]) {
                return false;
            }
        }
        blend_weighted_into(out, &mut self.scratch, &weights);
        true
    }

    /// Evaluate each sample with **its own phase** (independent playhead).
    ///
    /// `phases[i]` is the normalized phase of `samples[i]`; missing samples are sampled at 0. No
    /// SyncGroup phase mapping is done — the caller has already computed its own time for each sample.
    pub fn evaluate_into_with_phases(
        &mut self,
        at: [f32; 2],
        phases: &[f32],
        out: &mut Pose,
    ) -> bool {
        let Some((vertices, weights)) = self.weights_at(at) else {
            return false;
        };
        for (slot, vertex) in vertices.iter().enumerate() {
            self.scratch[slot].copy_from(&self.reference);
            let phase = phases.get(*vertex).copied().unwrap_or(0.0);
            let sample = &self.samples[*vertex];
            if !sample_at_phase(&sample.clip, phase, &mut self.scratch[slot]) {
                return false;
            }
        }
        blend_weighted_into(out, &mut self.scratch, &weights);
        true
    }
    /// The triangle and three barycentric weights corresponding to the query point.
    ///
    /// When the point is inside a triangle, its barycentric coordinates are used directly; when no
    /// triangle contains it, the point on the **nearest triangle boundary** to the query point (edge
    /// or vertex) is taken as the result, with weights guaranteed non-negative and summing to 1.
    /// Returns `None` when no triangle is available.
    fn weights_at(&self, at: [f32; 2]) -> Option<([usize; 3], [f32; 3])> {
        if !at[0].is_finite() || !at[1].is_finite() || self.triangles.is_empty() {
            return None;
        }
        let point = [at[0] as f64, at[1] as f64];
        let mut nearest: Option<([usize; 3], [f32; 3], f64)> = None;
        for triangle in &self.triangles {
            let a = to_f64(self.samples[triangle[0]].position);
            let b = to_f64(self.samples[triangle[1]].position);
            let c = to_f64(self.samples[triangle[2]].position);
            let bary = barycentric(point, a, b, c);
            if bary.iter().all(|weight| *weight >= -1e-9) {
                let sum: f64 = bary.iter().map(|weight| weight.max(0.0)).sum();
                if sum <= 1e-12 {
                    continue;
                }
                return Some((
                    *triangle,
                    [
                        (bary[0].max(0.0) / sum) as f32,
                        (bary[1].max(0.0) / sum) as f32,
                        (bary[2].max(0.0) / sum) as f32,
                    ],
                ));
            }
            let (edge_weights, distance_sq) = closest_on_triangle(point, a, b, c);
            if nearest
                .map(|(_, _, best)| distance_sq < best)
                .unwrap_or(true)
            {
                nearest = Some((
                    *triangle,
                    [
                        edge_weights[0] as f32,
                        edge_weights[1] as f32,
                        edge_weights[2] as f32,
                    ],
                    distance_sq,
                ));
            }
        }
        nearest.map(|(triangle, weights, _)| (triangle, weights))
    }
}

/// The nearest point on the triangle boundary to the query point, returning that point's weights
/// with respect to the three vertices.
///
/// Each edge is projected once and the smallest distance taken; vertices are covered by the segment
/// `t = 0 / 1` endpoints.
fn closest_on_triangle(p: [f64; 2], a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> ([f64; 3], f64) {
    let mut best = ([0.0; 3], f64::INFINITY);
    for (weights, distance_sq) in [
        closest_on_edge(p, a, b, 0, 1),
        closest_on_edge(p, b, c, 1, 2),
        closest_on_edge(p, c, a, 2, 0),
    ] {
        if distance_sq < best.1 {
            best = (weights, distance_sq);
        }
    }
    best
}

/// The nearest point on the segment `from → to` to the query point, returning the weights in `(a, b, c)`
/// order and the squared distance.
///
/// `from_slot` / `to_slot` are the indices of the segment's two endpoints within the triangle.
fn closest_on_edge(
    p: [f64; 2],
    from: [f64; 2],
    to: [f64; 2],
    from_slot: usize,
    to_slot: usize,
) -> ([f64; 3], f64) {
    let edge = sub(to, from);
    let length_sq = edge[0] * edge[0] + edge[1] * edge[1];
    let t = if length_sq <= 1e-12 {
        0.0
    } else {
        let offset = sub(p, from);
        ((offset[0] * edge[0] + offset[1] * edge[1]) / length_sq).clamp(0.0, 1.0)
    };
    let closest = [from[0] + edge[0] * t, from[1] + edge[1] * t];
    let delta = sub(p, closest);
    let mut weights = [0.0; 3];
    weights[from_slot] = 1.0 - t;
    weights[to_slot] = t;
    (weights, delta[0] * delta[0] + delta[1] * delta[1])
}

fn to_f64(point: [f32; 2]) -> [f64; 2] {
    [point[0] as f64, point[1] as f64]
}

/// Blend three sample poses by weight into `out`.
///
/// Starting from the highest-weight sample, accumulate one by one with `blend_into` (rotation uses
/// slerp, consistent with the 1D blend space). `scratch[3]` is used to hold the accumulation result
/// because `blend_into` cannot use the same pose as both input and output.
fn blend_weighted_into(out: &mut Pose, scratch: &mut [Pose; 4], weights: &[f32; 3]) {
    let mut order = [0usize, 1, 2];
    order.sort_by(|a, b| weights[*b].total_cmp(&weights[*a]));
    let first = order[0];
    out.copy_from(&scratch[first]);
    let mut total = weights[first];
    for &slot in &order[1..] {
        let weight = weights[slot];
        if weight <= 0.0 {
            continue;
        }
        scratch[3].copy_from(out);
        let t = weight / (total + weight);
        blend_into(out, &scratch[3], &scratch[slot], t);
        total += weight;
    }
}

/// The three barycentric weights of a point inside the triangle (corresponding to vertex order);
/// vertices must be counter-clockwise.
///
/// Degenerate triangles return all zeros, so the caller treats them as "does not contain the query point".
fn barycentric(p: [f64; 2], a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> [f64; 3] {
    let area = cross(sub(b, a), sub(c, a));
    if area.abs() <= 1e-12 {
        return [0.0; 3];
    }
    let wa = cross(sub(b, p), sub(c, p)) / area;
    let wb = cross(sub(c, p), sub(a, p)) / area;
    let wc = 1.0 - wa - wb;
    [wa, wb, wc]
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

/// Bowyer–Watson incremental triangulation. Returns index triples of `points` (counter-clockwise).
///
/// Few points (blend space samples hand-filled by the author), so a direct O(n²) implementation
/// is used here; no spatial index.
fn triangulate(points: &[[f64; 2]]) -> Vec<[usize; 3]> {
    if points.len() < 3 {
        return Vec::new();
    }
    let (mut min, mut max) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for point in points {
        min[0] = min[0].min(point[0]);
        min[1] = min[1].min(point[1]);
        max[0] = max[0].max(point[0]);
        max[1] = max[1].max(point[1]);
    }
    let span = (max[0] - min[0]).max(max[1] - min[1]).max(1.0);
    let mid = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
    let super_start = points.len();
    let mut all = points.to_vec();
    all.push([mid[0] - 32.0 * span, mid[1] - span]);
    all.push([mid[0] + 32.0 * span, mid[1] - span]);
    all.push([mid[0], mid[1] + 32.0 * span]);

    let mut triangles: Vec<[usize; 3]> = vec![[super_start, super_start + 1, super_start + 2]];
    for index in 0..points.len() {
        let mut edges: Vec<(usize, usize)> = Vec::new();
        let mut kept: Vec<[usize; 3]> = Vec::new();
        for triangle in &triangles {
            if in_circumcircle(
                all[index],
                all[triangle[0]],
                all[triangle[1]],
                all[triangle[2]],
            ) {
                for edge in [
                    (triangle[0], triangle[1]),
                    (triangle[1], triangle[2]),
                    (triangle[2], triangle[0]),
                ] {
                    if let Some(position) = edges
                        .iter()
                        .position(|candidate| *candidate == (edge.1, edge.0))
                    {
                        edges.swap_remove(position);
                    } else {
                        edges.push(edge);
                    }
                }
            } else {
                kept.push(*triangle);
            }
        }
        for (a, b) in edges {
            let mut triangle = [a, b, index];
            if signed_area(all[triangle[0]], all[triangle[1]], all[triangle[2]]) < 0.0 {
                triangle.swap(1, 2);
            }
            kept.push(triangle);
        }
        triangles = kept;
    }
    let mut result: Vec<[usize; 3]> = triangles
        .into_iter()
        .filter(|triangle| triangle.iter().all(|index| *index < super_start))
        .map(|mut triangle| {
            if signed_area(all[triangle[0]], all[triangle[1]], all[triangle[2]]) < 0.0 {
                triangle.swap(1, 2);
            }
            triangle
        })
        .collect();
    result.sort_by_key(|triangle| {
        let mut sorted = *triangle;
        sorted.sort_unstable();
        sorted
    });
    result
}

/// Whether the point is strictly inside the triangle's circumcircle (triangle must be counter-clockwise).
fn in_circumcircle(p: [f64; 2], a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> bool {
    let (ax, ay) = (a[0] - p[0], a[1] - p[1]);
    let (bx, by) = (b[0] - p[0], b[1] - p[1]);
    let (cx, cy) = (c[0] - p[0], c[1] - p[1]);
    let det = (ax * ax + ay * ay) * (bx * cy - by * cx) - (bx * bx + by * by) * (ax * cy - ay * cx)
        + (cx * cx + cy * cy) * (ax * by - ay * bx);
    det > 1e-12
}

fn signed_area(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    cross(sub(b, a), sub(c, a)) * 0.5
}

#[cfg(test)]
#[path = "blend_space2d_tests.rs"]
mod tests;
