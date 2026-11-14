//! Runtime PRT64 evaluation, Q8.12 dynamic light propagation, anti-leakage
//! resolve gating, and 64-tree streaming manager.

use std::collections::VecDeque;

use crate::prt_volume::{PrtCluster, PrtVolume, BRANCHING, NO_CHILD, NO_LEAF, SH_COEFF_COUNT};

/// Fixed-point scale for Q8.12 `InterlockedAdd` accumulation (2^12 = 4096).
pub const Q8_12_SCALE: f32 = 4096.0;

/// Maximum allowed `leak_bias` clamp to prevent 20 cm thin-wall light bleeding.
pub const MAX_LEAK_BIAS: f32 = 0.25;

/// Dynamic point/spot light for cluster-major PRT propagation.
#[derive(Clone, Copy, Debug)]
pub struct PrtDynamicLight {
    pub position: [f32; 3],
    pub color: [f32; 3],
    pub radius: f32,
}

/// Traverse the 64-tree for `world_pos`, returning `Some(leaf_index)` if inside
/// an occupied leaf.
pub fn traverse_64tree(volume: &PrtVolume, world_pos: [f32; 3]) -> Option<u32> {
    if volume.nodes.is_empty() || volume.world_extent <= 0.0 {
        return None;
    }
    let rel = [
        (world_pos[0] - volume.world_origin[0]) / volume.world_extent,
        (world_pos[1] - volume.world_origin[1]) / volume.world_extent,
        (world_pos[2] - volume.world_origin[2]) / volume.world_extent,
    ];
    if rel.iter().any(|&v| !(0.0..1.0).contains(&v)) {
        return None;
    }

    let mut node_idx = 0usize;
    let mut local = rel;
    loop {
        let node = volume.nodes.get(node_idx)?;
        if node.first_child == NO_CHILD {
            return (node.leaf_index != NO_LEAF).then_some(node.leaf_index);
        }
        let cx = ((local[0] * 4.0).floor() as u32).min(3);
        let cy = ((local[1] * 4.0).floor() as u32).min(3);
        let cz = ((local[2] * 4.0).floor() as u32).min(3);
        let slot = cx + cy * 4 + cz * 16;
        debug_assert!(slot < BRANCHING);
        if (node.child_mask >> slot) & 1 == 0 {
            return None;
        }
        // Count set bits below `slot` to find the dense child offset.
        let lower_mask = if slot == 0 { 0 } else { (1u64 << slot) - 1 };
        let child_offset = (node.child_mask & lower_mask).count_ones() as usize;
        node_idx = node.first_child as usize + child_offset;
        local = [
            local[0] * 4.0 - cx as f32,
            local[1] * 4.0 - cy as f32,
            local[2] * 4.0 - cz as f32,
        ];
    }
}

/// Evaluate a 64-coefficient PRT cluster against a 64-coefficient sky/sun SH vector.
pub fn eval_prt64_cluster(cluster: &PrtCluster, sky_sh64: &[f32; SH_COEFF_COUNT]) -> [f32; 3] {
    let mut rgb = [0.0f32; 3];
    for c in 0..SH_COEFF_COUNT {
        let s = sky_sh64[c];
        rgb[0] += cluster[c][0] * s;
        rgb[1] += cluster[c][1] * s;
        rgb[2] += cluster[c][2] * s;
    }
    [rgb[0].max(0.0), rgb[1].max(0.0), rgb[2].max(0.0)]
}

/// Propagate dynamic lights into per-leaf Q8.12 fixed-point accumulators.
pub fn propagate_dynamic_lights_q8_12(
    volume: &PrtVolume,
    lights: &[PrtDynamicLight],
) -> Vec<[i32; 3]> {
    let mut accum = vec![[0i32; 3]; volume.clusters.len()];
    if lights.is_empty() || volume.clusters.is_empty() {
        return accum;
    }

    let grid_res = 4u32.pow(volume.leaf_grid.max(1)) as f32;
    let leaf_size = volume.world_extent / grid_res;
    let leaf_radius = leaf_size * 0.8660254; // sqrt(3)/2

    for (leaf_idx, slot) in accum.iter_mut().enumerate() {
        let dc = volume.clusters[leaf_idx][0];
        let transfer_gain = ((dc[0] + dc[1] + dc[2]) / 3.0).clamp(0.0, 2.0);
        for light in lights {
            // Proxy sphere-vs-cluster-volume culling.
            let max_dist = light.radius + volume.world_extent + leaf_radius;
            let dx = light.position[0] - (volume.world_origin[0] + volume.world_extent * 0.5);
            let dy = light.position[1] - (volume.world_origin[1] + volume.world_extent * 0.5);
            let dz = light.position[2] - (volume.world_origin[2] + volume.world_extent * 0.5);
            let dist_sq = dx * dx + dy * dy + dz * dz;
            if dist_sq > max_dist * max_dist {
                continue;
            }
            let atten = 1.0 / (1.0 + dist_sq / (light.radius * light.radius).max(1e-4));
            for ch in 0..3 {
                let contrib = light.color[ch] * atten * transfer_gain;
                let q = (contrib * Q8_12_SCALE).round() as i32;
                slot[ch] = slot[ch].saturating_add(q);
            }
        }
    }
    accum
}

/// Resolve cluster irradiance with anti-leakage validity gating and `leak_bias` clamp.
pub fn resolve_prt_irradiance(
    cluster_rgb: [f32; 3],
    q8_12_dyn: [i32; 3],
    valid: bool,
    leak_bias: f32,
) -> [f32; 3] {
    if !valid {
        return [0.0, 0.0, 0.0];
    }
    let bias = leak_bias.clamp(0.0, MAX_LEAK_BIAS);
    [
        ((cluster_rgb[0] + q8_12_dyn[0] as f32 / Q8_12_SCALE) - bias).max(0.0),
        ((cluster_rgb[1] + q8_12_dyn[1] as f32 / Q8_12_SCALE) - bias).max(0.0),
        ((cluster_rgb[2] + q8_12_dyn[2] as f32 / Q8_12_SCALE) - bias).max(0.0),
    ]
}

/// Streaming statistics for 64-tree cluster residency.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrtStreamingStats {
    pub resident_leaves: usize,
    pub resident_bytes: usize,
    pub uploads: usize,
    pub evictions: usize,
}

/// Bounded-memory 64-tree cluster streaming manager.
pub struct PrtStreamingManager {
    memory_budget_bytes: usize,
    max_uploads_per_frame: usize,
    resident: Vec<bool>,
    lru_queue: VecDeque<usize>,
    pending_uploads: VecDeque<usize>,
    stats: PrtStreamingStats,
}

impl PrtStreamingManager {
    pub fn new(
        total_leaves: usize,
        memory_budget_bytes: usize,
        max_uploads_per_frame: usize,
    ) -> Self {
        Self {
            memory_budget_bytes,
            max_uploads_per_frame: max_uploads_per_frame.max(1),
            resident: vec![false; total_leaves],
            lru_queue: VecDeque::new(),
            pending_uploads: VecDeque::new(),
            stats: PrtStreamingStats::default(),
        }
    }

    pub fn request_leaf(&mut self, leaf_idx: usize) {
        if leaf_idx < self.resident.len()
            && !self.resident[leaf_idx]
            && !self.pending_uploads.contains(&leaf_idx)
        {
            self.pending_uploads.push_back(leaf_idx);
        }
    }

    pub fn step_frame(&mut self) -> PrtStreamingStats {
        let cluster_bytes = crate::prt_volume::CLUSTER_SIZE;
        let mut uploaded = 0usize;
        while uploaded < self.max_uploads_per_frame {
            let Some(leaf_idx) = self.pending_uploads.pop_front() else {
                break;
            };
            if self.resident[leaf_idx] {
                continue;
            }
            while self.stats.resident_bytes + cluster_bytes > self.memory_budget_bytes {
                let Some(evict_idx) = self.lru_queue.pop_front() else {
                    break;
                };
                if self.resident[evict_idx] {
                    self.resident[evict_idx] = false;
                    self.stats.resident_leaves -= 1;
                    self.stats.resident_bytes -= cluster_bytes;
                    self.stats.evictions += 1;
                }
            }
            if self.stats.resident_bytes + cluster_bytes <= self.memory_budget_bytes {
                self.resident[leaf_idx] = true;
                self.lru_queue.push_back(leaf_idx);
                self.stats.resident_leaves += 1;
                self.stats.resident_bytes += cluster_bytes;
                self.stats.uploads += 1;
            }
            uploaded += 1;
        }
        self.stats
    }

    pub fn is_resident(&self, leaf_idx: usize) -> bool {
        self.resident.get(leaf_idx).copied().unwrap_or(false)
    }

    pub fn stats(&self) -> PrtStreamingStats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prt_volume::PrtNode;

    fn sample_volume() -> PrtVolume {
        let mut c0 = [[0.0f32; 3]; SH_COEFF_COUNT];
        c0[0] = [0.5, 0.5, 0.5];
        PrtVolume {
            cluster_cells: 1,
            world_origin: [0.0, 0.0, 0.0],
            world_extent: 16.0,
            leaf_grid: 1,
            built_from_sha: [0u8; 8],
            nodes: vec![
                PrtNode {
                    first_child: 1,
                    leaf_index: NO_LEAF,
                    child_mask: 1,
                },
                PrtNode {
                    first_child: NO_CHILD,
                    leaf_index: 0,
                    child_mask: 0,
                },
            ],
            clusters: vec![c0],
            cluster_valid: Some(vec![true]),
        }
    }

    #[test]
    fn traverse_and_eval_prt64_cluster() {
        let vol = sample_volume();
        assert_eq!(traverse_64tree(&vol, [1.0, 1.0, 1.0]), Some(0));
        assert_eq!(traverse_64tree(&vol, [10.0, 10.0, 10.0]), None);
        let mut sky = [0.0f32; SH_COEFF_COUNT];
        sky[0] = 2.0;
        let rgb = eval_prt64_cluster(&vol.clusters[0], &sky);
        assert!((rgb[0] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn dynamic_lights_q8_12_and_leak_gating() {
        let vol = sample_volume();
        let lights = [PrtDynamicLight {
            position: [8.0, 8.0, 8.0],
            color: [1.0, 0.5, 0.25],
            radius: 20.0,
        }];
        let q = propagate_dynamic_lights_q8_12(&vol, &lights);
        assert!(q[0][0] > 0);
        assert_eq!(
            resolve_prt_irradiance([1.0, 1.0, 1.0], q[0], false, 0.1),
            [0.0; 3]
        );
        let res = resolve_prt_irradiance([1.0, 1.0, 1.0], q[0], true, 1.0);
        // leak_bias clamped to 0.25, so result > 0.75
        assert!(res[0] > 0.75);
    }

    #[test]
    fn streaming_manager_enforces_memory_budget() {
        let mut mgr = PrtStreamingManager::new(10, 384 * 2, 2);
        mgr.request_leaf(0);
        mgr.request_leaf(1);
        mgr.step_frame();
        assert_eq!(mgr.stats().resident_leaves, 2);
        mgr.request_leaf(2);
        mgr.step_frame();
        assert_eq!(mgr.stats().resident_leaves, 2);
        assert_eq!(mgr.stats().evictions, 1);
        assert!(!mgr.is_resident(0));
        assert!(mgr.is_resident(2));
    }
}
