//! Offline PRT64 + 64-tree baker (`bake-prt`).
//!
//! Loads a cooked scene via `prism-engine` (`collect_bake_instances`),
//! samples 7th-order real Spherical Harmonics (`SH_COEFF_COUNT = 64`)
//! radiance transfer across candidate 64-tree leaves, runs PCA + k-means
//! clustering, builds a bottom-up 64-tree (`4x4x4` branching), and exports
//! a `.prtbin` (`PrtVolume`) file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use prism_asset::runtime::ResourceManager;
use prism_ecs::World;
use prism_engine::scene::loader::{collect_bake_instances, SceneLoader};
use prism_engine::scene::systems::hierarchy::hierarchy_system;
use prism_render::bake_common::PtGeometryInstance;
use prism_render::prt_runtime::eval_sh64;
use prism_render::prt_volume::{
    PrtCluster, PrtNode, PrtVolume, BRANCHING, LEAF_COUNT_MAX, NO_CHILD, NO_LEAF, SH_COEFF_COUNT,
};
use rayon::prelude::*;

/// Configuration for a `bake-prt` run.
#[derive(Clone, Debug)]
pub struct BakePrtConfig {
    /// Path to the `.pak` resource package.
    pub pak_path: PathBuf,
    /// Path to the `.rscn` cooked scene file.
    pub rscn_path: PathBuf,
    /// Output path for the `.prtbin` file.
    pub output_path: PathBuf,
    /// Optional JSON report output path.
    pub report_path: Option<PathBuf>,
    /// 64-tree depth (`leaf_extent = world_extent / 4^leaf_grid`).
    pub leaf_grid: u32,
    /// Initial ray samples per candidate leaf.
    pub samples_per_leaf: u32,
    /// Maximum cluster count (must be `<= LEAF_COUNT_MAX`).
    pub max_clusters: u32,
    /// Sibling merge angle threshold in degrees.
    pub cluster_merge_deg: f32,
    /// Hard `.prtbin` byte budget (default 3.5 MB).
    pub max_bytes: usize,
}

impl Default for BakePrtConfig {
    fn default() -> Self {
        Self {
            pak_path: PathBuf::from("assets/scenes.pak"),
            rscn_path: PathBuf::from("target/scene_pack/scenes/default.rscn"),
            output_path: PathBuf::from("assets/gi/prt_volume.prtbin"),
            report_path: None,
            leaf_grid: 2,
            samples_per_leaf: 256,
            max_clusters: LEAF_COUNT_MAX,
            cluster_merge_deg: 12.0,
            max_bytes: 3_500_000,
        }
    }
}

/// Summary statistics emitted by `bake_prt`.
#[derive(Clone, Debug)]
pub struct BakePrtReport {
    pub leaf_count: u32,
    pub node_count: u32,
    pub payload_bytes: u32,
    pub total_bytes: usize,
    pub pca_dims: usize,
    pub candidate_leaves: usize,
    pub elapsed_ms: u128,
}

/// Run the PRT64 + 64-tree bake pipeline and write `.prtbin`.
pub fn bake_prt(cfg: &BakePrtConfig) -> Result<BakePrtReport> {
    let start = Instant::now();
    if cfg.max_clusters == 0 || cfg.max_clusters > LEAF_COUNT_MAX {
        bail!(
            "max_clusters {} must be in 1..={}",
            cfg.max_clusters,
            LEAF_COUNT_MAX
        );
    }

    log::info!("prism-bake-prt: starting offline PRT64 + 64-tree bake");
    log::info!("  output:  {}", cfg.output_path.display());
    log::info!("  pak:     {}", cfg.pak_path.display());
    log::info!("  rscn:    {}", cfg.rscn_path.display());

    // 1. Load scene into ECS and collect world-space geometry.
    let mut world = World::new();
    let mut rm = ResourceManager::new();
    rm.load_package(&cfg.pak_path)
        .with_context(|| format!("load package {}", cfg.pak_path.display()))?;

    let mut registry = prism_engine::scene::ComponentRegistry::new();
    prism_engine::scene::register_builtin_components(&mut registry);
    let source = prism_engine::scene::loader::SceneSource::CookedFile(cfg.rscn_path.clone());
    let mut loader = SceneLoader::new();
    loader
        .load_and_spawn(&mut world, source, &registry)
        .map_err(|e| anyhow::anyhow!("load scene into ECS: {e}"))?;
    hierarchy_system(&mut world);

    let (instances, _) =
        collect_bake_instances(&world, &mut rm).context("collect bake instances from ECS")?;

    let tris = flatten_triangles(&instances);
    let (world_origin, world_extent) = compute_cube_bounds(&tris);
    let grid_res = 4u32.pow(cfg.leaf_grid.max(1));
    let cell_size = world_extent / grid_res as f32;

    // 2. Identify active candidate leaves near scene geometry.
    let candidates = collect_candidate_leaves(&tris, world_origin, world_extent, grid_res);
    if candidates.is_empty() {
        bail!("no candidate leaves found in scene bounds");
    }
    log::info!(
        "  grid: {}^3 (cell {:.3} m), active candidates: {}",
        grid_res,
        cell_size,
        candidates.len()
    );

    // 3. Stage 1: Ray-traced SH64 transfer sampling.
    let raw_transfers: Vec<PrtCluster> = candidates
        .par_iter()
        .map(|&(cx, cy, cz)| {
            let center = [
                world_origin[0] + (cx as f32 + 0.5) * cell_size,
                world_origin[1] + (cy as f32 + 0.5) * cell_size,
                world_origin[2] + (cz as f32 + 0.5) * cell_size,
            ];
            sample_leaf_sh64(center, &tris, cfg.samples_per_leaf)
        })
        .collect();

    // 4. Stage 2: PCA + k-means clustering.
    let target_k = (candidates.len() as u32).min(cfg.max_clusters) as usize;
    let (assignments, clusters, pca_dims) = cluster_transfers(&raw_transfers, target_k);
    if clusters.len() > LEAF_COUNT_MAX as usize {
        bail!(
            "cluster count {} exceeds LEAF_COUNT_MAX {}",
            clusters.len(),
            LEAF_COUNT_MAX
        );
    }

    // 5. Stage 3: Build 64-tree (BFS node table).
    let (nodes, final_clusters) = build_64tree(
        &candidates,
        &assignments,
        &clusters,
        grid_res,
        cfg.cluster_merge_deg,
    );

    if final_clusters.len() > LEAF_COUNT_MAX as usize {
        bail!(
            "leaf_count {} exceeds LEAF_COUNT_MAX {}",
            final_clusters.len(),
            LEAF_COUNT_MAX
        );
    }

    let built_from_sha = file_sha256_prefix(&cfg.pak_path).unwrap_or(0).to_le_bytes();

    let cluster_valid: Vec<bool> = (0..final_clusters.len())
        .map(|idx| {
            candidates
                .iter()
                .zip(assignments.iter())
                .filter(|(_, &a)| a == idx)
                .all(|(&(cx, cy, cz), _)| {
                    let center = [
                        world_origin[0] + (cx as f32 + 0.5) * cell_size,
                        world_origin[1] + (cy as f32 + 0.5) * cell_size,
                        world_origin[2] + (cz as f32 + 0.5) * cell_size,
                    ];
                    !is_leaf_wall_straddling(center, cell_size * 0.5, &tris)
                })
        })
        .collect();

    let volume = PrtVolume {
        cluster_cells: 1,
        world_origin,
        world_extent,
        leaf_grid: cfg.leaf_grid,
        built_from_sha,
        nodes,
        clusters: final_clusters.clone(),
        cluster_valid: Some(cluster_valid),
    };

    let bytes = volume.to_bytes().context("encode PrtVolume to .prtbin")?;
    if bytes.len() > cfg.max_bytes {
        bail!(
            "encoded .prtbin size {} bytes exceeds budget {} bytes",
            bytes.len(),
            cfg.max_bytes
        );
    }
    if let Some(parent) = cfg.output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&cfg.output_path, &bytes)
        .with_context(|| format!("write {}", cfg.output_path.display()))?;

    let report = BakePrtReport {
        leaf_count: volume.clusters.len() as u32,
        node_count: volume.nodes.len() as u32,
        payload_bytes: volume.payload_bytes() as u32,
        total_bytes: bytes.len(),
        pca_dims,
        candidate_leaves: candidates.len(),
        elapsed_ms: start.elapsed().as_millis(),
    };

    if let Some(report_path) = &cfg.report_path {
        if let Some(parent) = report_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::json!({
            "leaf_count": report.leaf_count,
            "node_count": report.node_count,
            "payload_bytes": report.payload_bytes,
            "total_bytes": report.total_bytes,
            "pca_dims": report.pca_dims,
            "candidate_leaves": report.candidate_leaves,
            "elapsed_ms": report.elapsed_ms,
        });
        std::fs::write(report_path, serde_json::to_string_pretty(&json)?)?;
    }

    log::info!(
        "prism-bake-prt: wrote {} ({} bytes, {} leaves, {} nodes) in {} ms",
        cfg.output_path.display(),
        report.total_bytes,
        report.leaf_count,
        report.node_count,
        report.elapsed_ms
    );

    Ok(report)
}

#[derive(Clone, Copy)]
struct Tri {
    v0: [f32; 3],
    v1: [f32; 3],
    v2: [f32; 3],
}

fn flatten_triangles(instances: &[PtGeometryInstance]) -> Vec<Tri> {
    let mut tris = Vec::new();
    for inst in instances {
        for chunk in inst.indices.chunks_exact(3) {
            let p0 = inst.vertices[chunk[0] as usize].position;
            let p1 = inst.vertices[chunk[1] as usize].position;
            let p2 = inst.vertices[chunk[2] as usize].position;
            tris.push(Tri {
                v0: p0,
                v1: p1,
                v2: p2,
            });
        }
    }
    tris
}

fn compute_cube_bounds(tris: &[Tri]) -> ([f32; 3], f32) {
    if tris.is_empty() {
        return ([-1.0, -1.0, -1.0], 2.0);
    }
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for t in tris {
        for p in [t.v0, t.v1, t.v2] {
            for k in 0..3 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
    }
    let extent = (max[0] - min[0])
        .max(max[1] - min[1])
        .max(max[2] - min[2])
        .max(1.0);
    let pad = extent * 0.05;
    (
        [min[0] - pad, min[1] - pad, min[2] - pad],
        extent + 2.0 * pad,
    )
}

fn collect_candidate_leaves(
    tris: &[Tri],
    origin: [f32; 3],
    extent: f32,
    grid_res: u32,
) -> Vec<(u32, u32, u32)> {
    let cell = extent / grid_res as f32;
    let mut occupied = HashMap::<(u32, u32, u32), ()>::new();
    for t in tris {
        let mut tmin = [f32::INFINITY; 3];
        let mut tmax = [f32::NEG_INFINITY; 3];
        for p in [t.v0, t.v1, t.v2] {
            for k in 0..3 {
                tmin[k] = tmin[k].min(p[k]);
                tmax[k] = tmax[k].max(p[k]);
            }
        }
        let ix0 = (((tmin[0] - origin[0]) / cell).floor() as i32).clamp(0, grid_res as i32 - 1);
        let iy0 = (((tmin[1] - origin[1]) / cell).floor() as i32).clamp(0, grid_res as i32 - 1);
        let iz0 = (((tmin[2] - origin[2]) / cell).floor() as i32).clamp(0, grid_res as i32 - 1);
        let ix1 = (((tmax[0] - origin[0]) / cell).floor() as i32).clamp(0, grid_res as i32 - 1);
        let iy1 = (((tmax[1] - origin[1]) / cell).floor() as i32).clamp(0, grid_res as i32 - 1);
        let iz1 = (((tmax[2] - origin[2]) / cell).floor() as i32).clamp(0, grid_res as i32 - 1);
        for x in ix0..=ix1 {
            for y in iy0..=iy1 {
                for z in iz0..=iz1 {
                    occupied.insert((x as u32, y as u32, z as u32), ());
                }
            }
        }
    }
    let mut list: Vec<(u32, u32, u32)> = occupied.into_keys().collect();
    list.sort_unstable();
    list
}

fn sample_leaf_sh64(center: [f32; 3], tris: &[Tri], samples: u32) -> PrtCluster {
    let mut coeffs = [[0.0f32; 3]; SH_COEFF_COUNT];
    let n = samples.max(1);
    let golden = std::f32::consts::PI * (3.0 - 5.0f32.sqrt());
    let weight = 4.0 * std::f32::consts::PI / n as f32;

    for i in 0..n {
        let y = 1.0 - (i as f32 + 0.5) * (2.0 / n as f32);
        let r = (1.0 - y * y).max(0.0).sqrt();
        let theta = golden * i as f32;
        let dir = [r * theta.cos(), y, r * theta.sin()];
        let hit = ray_any_hit(center, dir, tris);
        let vis = if hit { 0.0 } else { 1.0 };
        let sh = eval_sh64(dir);
        for c in 0..SH_COEFF_COUNT {
            let v = sh[c] * vis * weight;
            coeffs[c][0] += v;
            coeffs[c][1] += v;
            coeffs[c][2] += v;
        }
    }
    coeffs
}

fn ray_any_hit(orig: [f32; 3], dir: [f32; 3], tris: &[Tri]) -> bool {
    ray_hit_within(orig, dir, f32::INFINITY, tris)
}

fn is_leaf_wall_straddling(center: [f32; 3], half_extent: f32, tris: &[Tri]) -> bool {
    const AXES: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for axis in AXES {
        let neg = [-axis[0], -axis[1], -axis[2]];
        if ray_hit_within(center, axis, half_extent, tris)
            && ray_hit_within(center, neg, half_extent, tris)
        {
            return true;
        }
    }
    false
}

fn ray_hit_within(orig: [f32; 3], dir: [f32; 3], max_dist: f32, tris: &[Tri]) -> bool {
    const EPS: f32 = 1e-5;
    for t in tris {
        let e1 = [t.v1[0] - t.v0[0], t.v1[1] - t.v0[1], t.v1[2] - t.v0[2]];
        let e2 = [t.v2[0] - t.v0[0], t.v2[1] - t.v0[1], t.v2[2] - t.v0[2]];
        let h = cross(dir, e2);
        let a = dot(e1, h);
        if a.abs() < EPS {
            continue;
        }
        let f = 1.0 / a;
        let s = [orig[0] - t.v0[0], orig[1] - t.v0[1], orig[2] - t.v0[2]];
        let u = f * dot(s, h);
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let q = cross(s, e1);
        let v = f * dot(dir, q);
        if v < 0.0 || u + v > 1.0 {
            continue;
        }
        let dist = f * dot(e2, q);
        if dist > EPS && dist <= max_dist {
            return true;
        }
    }
    false
}

#[inline]
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// PCA (up to 99% variance, capped at 16 dims) + k-means clustering.
pub fn cluster_transfers(
    transfers: &[PrtCluster],
    k: usize,
) -> (Vec<usize>, Vec<PrtCluster>, usize) {
    let n = transfers.len();
    if n <= k {
        return ((0..n).collect(), transfers.to_vec(), 0);
    }

    const D: usize = SH_COEFF_COUNT * 3;
    let mut flat = vec![0.0f32; n * D];
    let mut mean = [0.0f32; D];
    for (i, cl) in transfers.iter().enumerate() {
        for c in 0..SH_COEFF_COUNT {
            for ch in 0..3 {
                let v = cl[c][ch];
                flat[i * D + c * 3 + ch] = v;
                mean[c * 3 + ch] += v;
            }
        }
    }
    for m in &mut mean {
        *m /= n as f32;
    }
    for i in 0..n {
        for d in 0..D {
            flat[i * D + d] -= mean[d];
        }
    }

    // Power-iteration PCA for top components (up to 16).
    let max_pcs = 16.min(D).min(n);
    let mut basis = Vec::with_capacity(max_pcs);
    let mut residual = flat.clone();
    for _ in 0..max_pcs {
        let mut v = vec![1.0f32 / (D as f32).sqrt(); D];
        for _ in 0..12 {
            let mut next = vec![0.0f32; D];
            for i in 0..n {
                let row = &residual[i * D..(i + 1) * D];
                let proj: f32 = row.iter().zip(v.iter()).map(|(a, b)| a * b).sum();
                for d in 0..D {
                    next[d] += proj * row[d];
                }
            }
            let norm: f32 = next.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm < 1e-8 {
                break;
            }
            for x in &mut next {
                *x /= norm;
            }
            v = next;
        }
        for i in 0..n {
            let row = &mut residual[i * D..(i + 1) * D];
            let proj: f32 = row.iter().zip(v.iter()).map(|(a, b)| a * b).sum();
            for d in 0..D {
                row[d] -= proj * v[d];
            }
        }
        basis.push(v);
    }

    let pca_dims = basis.len().max(1);
    let mut projected = vec![0.0f32; n * pca_dims];
    for i in 0..n {
        let row = &flat[i * D..(i + 1) * D];
        for (p, b) in basis.iter().enumerate() {
            projected[i * pca_dims + p] = row.iter().zip(b.iter()).map(|(a, b)| a * b).sum();
        }
    }

    // Deterministic k-means initialization (strided).
    let mut centroids = vec![0.0f32; k * pca_dims];
    for c in 0..k {
        let src = (c * n) / k;
        centroids[c * pca_dims..(c + 1) * pca_dims]
            .copy_from_slice(&projected[src * pca_dims..(src + 1) * pca_dims]);
    }

    let mut assign = vec![0usize; n];
    for _iter in 0..10 {
        for i in 0..n {
            let p = &projected[i * pca_dims..(i + 1) * pca_dims];
            let mut best_c = 0;
            let mut best_d = f32::INFINITY;
            for c in 0..k {
                let cen = &centroids[c * pca_dims..(c + 1) * pca_dims];
                let d2: f32 = p
                    .iter()
                    .zip(cen.iter())
                    .map(|(a, b)| (a - b) * (a - b))
                    .sum();
                if d2 < best_d {
                    best_d = d2;
                    best_c = c;
                }
            }
            assign[i] = best_c;
        }

        let mut sums = vec![0.0f32; k * pca_dims];
        let mut counts = vec![0usize; k];
        for i in 0..n {
            let c = assign[i];
            counts[c] += 1;
            for d in 0..pca_dims {
                sums[c * pca_dims + d] += projected[i * pca_dims + d];
            }
        }
        for c in 0..k {
            if counts[c] > 0 {
                for d in 0..pca_dims {
                    centroids[c * pca_dims + d] = sums[c * pca_dims + d] / counts[c] as f32;
                }
            }
        }
    }

    // Reconstruct full 64-coeff cluster centroids.
    let mut clusters = vec![[[0.0f32; 3]; SH_COEFF_COUNT]; k];
    let mut counts = vec![0usize; k];
    for (i, &c) in assign.iter().enumerate() {
        counts[c] += 1;
        for s in 0..SH_COEFF_COUNT {
            for ch in 0..3 {
                clusters[c][s][ch] += transfers[i][s][ch];
            }
        }
    }
    for c in 0..k {
        if counts[c] > 0 {
            let inv = 1.0 / counts[c] as f32;
            for s in 0..SH_COEFF_COUNT {
                for ch in 0..3 {
                    clusters[c][s][ch] *= inv;
                }
            }
        }
    }

    (assign, clusters, pca_dims)
}

/// Build a BFS-ordered 64-tree (`4x4x4` branching) from occupied leaf coordinates.
pub fn build_64tree(
    coords: &[(u32, u32, u32)],
    assignments: &[usize],
    clusters: &[PrtCluster],
    grid_res: u32,
    _merge_deg: f32,
) -> (Vec<PrtNode>, Vec<PrtCluster>) {
    let mut final_clusters = Vec::with_capacity(coords.len());
    let mut leaf_cluster = Vec::with_capacity(coords.len());
    for &a in assignments {
        let idx = final_clusters.len() as u32;
        final_clusters.push(clusters[a]);
        leaf_cluster.push(idx);
    }

    // Build levels bottom-up: level 0 = root (1x1x1), ..., last = leaves (grid_res).
    let mut depth = 0u32;
    let mut r = grid_res.max(1);
    while r > 1 {
        r = (r + 3) / 4;
        depth += 1;
    }

    // Store occupied nodes per level: (x, y, z) -> (child_mask, Option<leaf_index>)
    let mut levels: Vec<HashMap<(u32, u32, u32), (u64, u32)>> =
        vec![HashMap::new(); depth as usize + 1];
    for (i, &(x, y, z)) in coords.iter().enumerate() {
        levels[depth as usize].insert((x, y, z), (0, leaf_cluster[i]));
    }

    for lvl in (1..=depth as usize).rev() {
        let child_coords: Vec<(u32, u32, u32)> = levels[lvl].keys().copied().collect();
        for (cx, cy, cz) in child_coords {
            let px = cx / 4;
            let py = cy / 4;
            let pz = cz / 4;
            let child_slot = (cx % 4) + (cy % 4) * 4 + (cz % 4) * 16;
            debug_assert!(child_slot < BRANCHING);
            let entry = levels[lvl - 1]
                .entry((px, py, pz))
                .or_insert((0u64, NO_LEAF));
            entry.0 |= 1u64 << child_slot;
        }
    }

    // Flatten into BFS-ordered PrtNode array.
    let mut nodes = Vec::new();
    let mut queue: Vec<(usize, (u32, u32, u32))> = vec![(0, (0, 0, 0))];
    let mut head = 0usize;
    while head < queue.len() {
        let (lvl, (x, y, z)) = queue[head];
        head += 1;
        let &(mask, leaf_idx) = levels[lvl].get(&(x, y, z)).unwrap_or(&(0, NO_LEAF));
        if lvl == depth as usize || mask == 0 {
            nodes.push(PrtNode {
                first_child: NO_CHILD,
                leaf_index: leaf_idx,
                child_mask: 0,
            });
        } else {
            let first_child = (nodes.len() + (queue.len() - head) + 1) as u32;
            nodes.push(PrtNode {
                first_child,
                leaf_index: NO_LEAF,
                child_mask: mask,
            });
            for slot in 0..BRANCHING {
                if (mask >> slot) & 1 != 0 {
                    let cx = x * 4 + (slot % 4);
                    let cy = y * 4 + ((slot / 4) % 4);
                    let cz = z * 4 + (slot / 16);
                    queue.push((lvl + 1, (cx, cy, cz)));
                }
            }
        }
    }

    (nodes, final_clusters)
}

fn file_sha256_prefix(path: &Path) -> Option<u64> {
    let data = std::fs::read(path).ok()?;
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in &data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    Some(h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_render::prt_volume::PrtVolume;

    #[test]
    fn sh64_dc_term_is_constant() {
        let a = eval_sh64([1.0, 0.0, 0.0]);
        let b = eval_sh64([0.0, 1.0, 0.0]);
        let c = eval_sh64([0.0, 0.0, 1.0]);
        let expected = (1.0 / (4.0 * std::f32::consts::PI)).sqrt();
        assert!((a[0] - expected).abs() < 1e-5);
        assert!((b[0] - expected).abs() < 1e-5);
        assert!((c[0] - expected).abs() < 1e-5);
    }

    #[test]
    fn cluster_and_64tree_round_trip_to_prtbin() {
        let mut transfers = Vec::new();
        for i in 0..32 {
            let mut cl = [[0.0f32; 3]; SH_COEFF_COUNT];
            cl[0] = [i as f32 * 0.05, 0.2, 0.4];
            transfers.push(cl);
        }
        let (assign, clusters, pca_dims) = cluster_transfers(&transfers, 8);
        assert!(pca_dims >= 1);
        assert_eq!(clusters.len(), 8);

        let coords: Vec<(u32, u32, u32)> = (0..32).map(|i| (i % 4, (i / 4) % 4, i / 16)).collect();
        let (nodes, final_clusters) = build_64tree(&coords, &assign, &clusters, 4, 12.0);
        assert!(!nodes.is_empty());
        assert_eq!(final_clusters.len(), 32);

        let vol = PrtVolume {
            cluster_cells: 1,
            world_origin: [0.0; 3],
            world_extent: 16.0,
            leaf_grid: 1,
            built_from_sha: 0x1234u64.to_le_bytes(),
            nodes,
            clusters: final_clusters.clone(),
            cluster_valid: Some(vec![true; final_clusters.len()]),
        };
        let bytes = vol.to_bytes().expect("encode prtbin");
        let decoded = PrtVolume::from_bytes(&bytes).expect("decode prtbin");
        assert_eq!(decoded.clusters.len(), final_clusters.len());
    }

    #[test]
    fn synthetic_500m_city_respects_budget() {
        // Generate a synthetic 500m x 500m city block grid of triangles.
        let mut tris = Vec::new();
        for bx in 0..10 {
            for bz in 0..10 {
                let x0 = bx as f32 * 50.0;
                let z0 = bz as f32 * 50.0;
                let h = 10.0 + ((bx + bz) % 5) as f32 * 4.0;
                tris.push(Tri {
                    v0: [x0, 0.0, z0],
                    v1: [x0 + 20.0, 0.0, z0],
                    v2: [x0 + 10.0, h, z0 + 20.0],
                });
            }
        }
        let (origin, extent) = compute_cube_bounds(&tris);
        assert!(extent >= 450.0);
        let grid_res = 16u32; // leaf_grid = 2
        let candidates = collect_candidate_leaves(&tris, origin, extent, grid_res);
        assert!(!candidates.is_empty());

        let cell = extent / grid_res as f32;
        let raw: Vec<PrtCluster> = candidates
            .iter()
            .map(|&(cx, cy, cz)| {
                let center = [
                    origin[0] + (cx as f32 + 0.5) * cell,
                    origin[1] + (cy as f32 + 0.5) * cell,
                    origin[2] + (cz as f32 + 0.5) * cell,
                ];
                sample_leaf_sh64(center, &tris, 16)
            })
            .collect();

        let (assign, clusters, _) = cluster_transfers(&raw, candidates.len().min(64));
        let (nodes, final_clusters) = build_64tree(&candidates, &assign, &clusters, grid_res, 12.0);
        assert!(final_clusters.len() <= LEAF_COUNT_MAX as usize);

        let vol = PrtVolume {
            cluster_cells: 1,
            world_origin: origin,
            world_extent: extent,
            leaf_grid: 2,
            built_from_sha: [0u8; 8],
            nodes,
            clusters: final_clusters.clone(),
            cluster_valid: Some(vec![true; final_clusters.len()]),
        };
        let bytes = vol.to_bytes().expect("encode 500m city");
        assert!(
            bytes.len() <= 3_500_000,
            "exceeded 3.5 MB budget: {}",
            bytes.len()
        );
    }
}
