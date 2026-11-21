//! Offline multi-modal Cluster-DAG virtual geometry cooker (`RCLD` v1).
//!
//! Splits input triangle meshes into spatial 128-vertex / 128-triangle meshlets,
//! groups adjacent clusters into atomic LOD units, runs boundary-locked QEM
//! edge-collapse simplification with continuous geomorphing residual tracking,
//! generates optional SGGX 3D Gaussian surfel surrogates for far-field foliage
//! and scalar displacement blocks for micro-meshes, and packs clusters into
//! 64 KiB streaming pages.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

use prism_asset::formats_virtual_geo::{
    encode_octahedral_normal, f32_to_f16, QuantizedClusterVertex, RcldAsset, RcldCluster,
    RcldClusterGroup, RcldHeader, RcldPageDescriptor, SggxSplatPrimitive, VirtualGeoPrimitiveKind,
    RCLD_FLAG_GEOMORPHING, RCLD_FLAG_HAS_DMM, RCLD_FLAG_HAS_SPLATS, RCLD_FLAG_SKINNED,
    RCLD_MAX_CLUSTER_BONES, RCLD_MAX_CLUSTER_TRIANGLES, RCLD_MAX_CLUSTER_VERTICES,
    RCLD_PAGE_FLAG_RESIDENT_ROOT, RCLD_PAGE_SIZE, RCLD_VERSION,
};

use super::CookError;

const MAX_DAG_LOD_LEVELS: u8 = 8;

/// Input geometry fed into the offline Cluster-DAG builder.
pub struct VirtualGeoBuildInput<'a> {
    pub positions: &'a [[f32; 3]],
    pub normals: &'a [[f32; 3]],
    pub uvs: &'a [[f32; 2]],
    pub indices: &'a [u32],
    pub triangle_materials: Option<&'a [u16]>,
    pub joints: Option<&'a [[u16; 4]]>,
    pub weights: Option<&'a [[f32; 4]]>,
    pub displacements: Option<&'a [f32]>,
    pub alpha_masked_foliage: bool,
}

#[derive(Clone, Copy)]
struct WorkingVertex {
    pos: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
    joints: [u16; 4],
    weights: [f32; 4],
    disp: f32,
}

#[derive(Clone)]
struct WorkingCluster {
    cluster_id: u32,
    group_id: u32,
    lod_level: u8,
    material_index: u16,
    primitive_kind: VirtualGeoPrimitiveKind,
    vertex_ids: Vec<u32>,
    geomorph_targets: Vec<[f32; 3]>,
    triangles: Vec<[u8; 3]>,
    self_error: f32,
    parent_error: f32,
    aabb_min: [f32; 3],
    aabb_max: [f32; 3],
    bounding_sphere: [f32; 4],
    normal_cone: [f32; 4],
    splat_surrogate: Option<SggxSplatPrimitive>,
    has_dmm: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct PosKey([i64; 3]);

fn pos_key(p: [f32; 3]) -> PosKey {
    PosKey([
        (p[0] * 100_000.0).round() as i64,
        (p[1] * 100_000.0).round() as i64,
        (p[2] * 100_000.0).round() as i64,
    ])
}

fn edge_pos_key(a: PosKey, b: PosKey) -> (PosKey, PosKey) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

fn expand_bits_10(v: u32) -> u32 {
    let mut x = v & 0x03FF;
    x = (x | (x << 16)) & 0x0300_00FF;
    x = (x | (x << 8)) & 0x0300_F00F;
    x = (x | (x << 4)) & 0x030C_30C3;
    x = (x | (x << 2)) & 0x0924_9249;
    x
}

fn morton_3d(p: [f32; 3], bmin: [f32; 3], bmax: [f32; 3]) -> u32 {
    let norm = |axis: usize| -> u32 {
        let span = bmax[axis] - bmin[axis];
        let u = if span > 1e-8 {
            ((p[axis] - bmin[axis]) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (u * 1023.0).round().clamp(0.0, 1023.0) as u32
    };
    (expand_bits_10(norm(0)) << 2) | (expand_bits_10(norm(1)) << 1) | expand_bits_10(norm(2))
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale3(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn len3(a: [f32; 3]) -> f32 {
    dot3(a, a).sqrt()
}

fn normalize3(a: [f32; 3]) -> [f32; 3] {
    let l = len3(a);
    if l > 1e-10 {
        [a[0] / l, a[1] / l, a[2] / l]
    } else {
        [0.0, 1.0, 0.0]
    }
}

fn compute_aabb(positions: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for p in positions {
        for axis in 0..3 {
            min[axis] = min[axis].min(p[axis]);
            max[axis] = max[axis].max(p[axis]);
        }
    }
    if !min[0].is_finite() {
        return ([0.0; 3], [0.0; 3]);
    }
    (min, max)
}

fn compute_cluster_bounds(
    verts: &[WorkingVertex],
    v_ids: &[u32],
    tris: &[[u8; 3]],
) -> ([f32; 3], [f32; 3], [f32; 4], [f32; 4]) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for &vid in v_ids {
        let p = verts[vid as usize].pos;
        for axis in 0..3 {
            min[axis] = min[axis].min(p[axis]);
            max[axis] = max[axis].max(p[axis]);
        }
    }
    let center = [
        0.5 * (min[0] + max[0]),
        0.5 * (min[1] + max[1]),
        0.5 * (min[2] + max[2]),
    ];
    let mut max_r2 = 0.0f32;
    for &vid in v_ids {
        let d = sub3(verts[vid as usize].pos, center);
        max_r2 = max_r2.max(dot3(d, d));
    }
    let sphere = [center[0], center[1], center[2], max_r2.sqrt().max(1e-5)];

    let mut tri_normals = Vec::with_capacity(tris.len());
    let mut avg_n = [0.0f32; 3];
    for tri in tris {
        let p0 = verts[v_ids[usize::from(tri[0])] as usize].pos;
        let p1 = verts[v_ids[usize::from(tri[1])] as usize].pos;
        let p2 = verts[v_ids[usize::from(tri[2])] as usize].pos;
        let n = cross3(sub3(p1, p0), sub3(p2, p0));
        if len3(n) > 1e-10 {
            let unit = normalize3(n);
            avg_n = add3(avg_n, unit);
            tri_normals.push(unit);
        }
    }
    let cone = if tri_normals.is_empty() || len3(avg_n) <= 1e-8 {
        [0.0, 1.0, 0.0, 1.0]
    } else {
        let axis = normalize3(avg_n);
        let mut min_dot = 1.0f32;
        for n in tri_normals {
            min_dot = min_dot.min(dot3(axis, n));
        }
        let half_angle = min_dot.clamp(-1.0, 1.0).acos();
        let cutoff = if half_angle >= std::f32::consts::FRAC_PI_2 {
            1.0
        } else {
            half_angle.sin().min(1.0)
        };
        [axis[0], axis[1], axis[2], cutoff]
    };

    (min, max, sphere, cone)
}

fn fit_sggx_splat(
    verts: &[WorkingVertex],
    v_ids: &[u32],
    aabb_min: [f32; 3],
    aabb_max: [f32; 3],
) -> SggxSplatPrimitive {
    let count = v_ids.len().max(1) as f32;
    let mut center = [0.0f32; 3];
    let mut cov = [0.0f32; 6];
    for &vid in v_ids {
        let v = &verts[vid as usize];
        center = add3(center, v.pos);
        let n = normalize3(v.normal);
        cov[0] += n[0] * n[0];
        cov[1] += n[1] * n[1];
        cov[2] += n[2] * n[2];
        cov[3] += n[0] * n[1];
        cov[4] += n[0] * n[2];
        cov[5] += n[1] * n[2];
    }
    center = scale3(center, 1.0 / count);
    for entry in &mut cov {
        *entry /= count;
    }
    let half_ext = [
        (0.5 * (aabb_max[0] - aabb_min[0])).max(1e-4),
        (0.5 * (aabb_max[1] - aabb_min[1])).max(1e-4),
        (0.5 * (aabb_max[2] - aabb_min[2])).max(1e-4),
    ];
    SggxSplatPrimitive {
        center,
        scale: half_ext,
        rotation: [0.0, 0.0, 0.0, 1.0],
        sggx_cov: cov,
        opacity: 0.9,
    }
}

fn compute_smooth_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut normals = vec![[0.0f32; 3]; positions.len()];
    for tri in indices.as_chunks::<3>().0 {
        let i0 = tri[0] as usize;
        let i1 = tri[1] as usize;
        let i2 = tri[2] as usize;
        let p0 = positions[i0];
        let p1 = positions[i1];
        let p2 = positions[i2];
        let n = cross3(sub3(p1, p0), sub3(p2, p0));
        normals[i0] = add3(normals[i0], n);
        normals[i1] = add3(normals[i1], n);
        normals[i2] = add3(normals[i2], n);
    }
    for n in &mut normals {
        *n = normalize3(*n);
    }
    normals
}

struct PartitionConfig {
    lod_level: u8,
    self_error: f32,
    global_min: [f32; 3],
    global_max: [f32; 3],
    allow_dmm: bool,
    foliage_splat_mode: bool,
}

fn partition_triangles_into_clusters(
    verts: &[WorkingVertex],
    tris: &[([u32; 3], u16)],
    cfg: &PartitionConfig,
) -> Vec<WorkingCluster> {
    let lod_level = cfg.lod_level;
    let self_error = cfg.self_error;
    let global_min = cfg.global_min;
    let global_max = cfg.global_max;
    let allow_dmm = cfg.allow_dmm;
    let foliage_splat_mode = cfg.foliage_splat_mode;
    let mut sorted: Vec<(u16, u32, [u32; 3])> = tris
        .iter()
        .map(|&(tri, mat)| {
            let p0 = verts[tri[0] as usize].pos;
            let p1 = verts[tri[1] as usize].pos;
            let p2 = verts[tri[2] as usize].pos;
            let centroid = scale3(add3(add3(p0, p1), p2), 1.0 / 3.0);
            (mat, morton_3d(centroid, global_min, global_max), tri)
        })
        .collect();
    sorted.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

    let mut clusters = Vec::new();
    let mut local_map: HashMap<u32, u8> = HashMap::new();
    let mut local_vids: Vec<u32> = Vec::new();
    let mut local_tris: Vec<[u8; 3]> = Vec::new();
    let mut cur_mat = sorted.first().map_or(0, |t| t.0);

    let flush_cluster = |clusters: &mut Vec<WorkingCluster>,
                         local_vids: &mut Vec<u32>,
                         local_tris: &mut Vec<[u8; 3]>,
                         local_map: &mut HashMap<u32, u8>,
                         mat: u16| {
        if local_tris.is_empty() {
            return;
        }
        let (aabb_min, aabb_max, bounding_sphere, normal_cone) =
            compute_cluster_bounds(verts, local_vids, local_tris);
        let geomorph_targets = local_vids
            .iter()
            .map(|&vid| verts[vid as usize].pos)
            .collect();
        let (primitive_kind, splat_surrogate) = if foliage_splat_mode && lod_level >= 1 {
            (
                VirtualGeoPrimitiveKind::SggxGaussianSplats,
                Some(fit_sggx_splat(verts, local_vids, aabb_min, aabb_max)),
            )
        } else if allow_dmm && lod_level == 0 && local_tris.len() <= 64 {
            (VirtualGeoPrimitiveKind::DisplacedMicroMesh, None)
        } else {
            (VirtualGeoPrimitiveKind::Triangles, None)
        };
        clusters.push(WorkingCluster {
            cluster_id: 0,
            group_id: 0,
            lod_level,
            material_index: mat,
            primitive_kind,
            vertex_ids: std::mem::take(local_vids),
            geomorph_targets,
            triangles: std::mem::take(local_tris),
            self_error,
            parent_error: self_error + 1.0,
            aabb_min,
            aabb_max,
            bounding_sphere,
            normal_cone,
            splat_surrogate,
            has_dmm: allow_dmm
                && lod_level == 0
                && matches!(primitive_kind, VirtualGeoPrimitiveKind::DisplacedMicroMesh),
        });
        local_map.clear();
    };

    for (mat, _code, tri) in sorted {
        let mut needed = 0usize;
        for &vid in &tri {
            if !local_map.contains_key(&vid) {
                needed += 1;
            }
        }
        let max_tris = if allow_dmm && lod_level == 0 {
            64
        } else {
            RCLD_MAX_CLUSTER_TRIANGLES
        };
        if mat != cur_mat
            || local_tris.len() + 1 > max_tris
            || local_vids.len() + needed > RCLD_MAX_CLUSTER_VERTICES
        {
            flush_cluster(
                &mut clusters,
                &mut local_vids,
                &mut local_tris,
                &mut local_map,
                cur_mat,
            );
            cur_mat = mat;
        }
        let mut ltri = [0u8; 3];
        for (slot, &vid) in tri.iter().enumerate() {
            let idx = *local_map.entry(vid).or_insert_with(|| {
                let id = local_vids.len() as u8;
                local_vids.push(vid);
                id
            });
            ltri[slot] = idx;
        }
        local_tris.push(ltri);
    }
    flush_cluster(
        &mut clusters,
        &mut local_vids,
        &mut local_tris,
        &mut local_map,
        cur_mat,
    );
    clusters
}

#[derive(Clone, Copy)]
struct Quadric([f64; 10]);

impl Quadric {
    fn zero() -> Self {
        Self([0.0; 10])
    }

    fn from_plane(a: f64, b: f64, c: f64, d: f64) -> Self {
        Self([
            a * a,
            a * b,
            a * c,
            a * d,
            b * b,
            b * c,
            b * d,
            c * c,
            c * d,
            d * d,
        ])
    }

    fn add(self, rhs: Self) -> Self {
        let mut out = [0.0; 10];
        for (slot, (lhs, rhs)) in out.iter_mut().zip(self.0.iter().zip(rhs.0.iter())) {
            *slot = lhs + rhs;
        }
        Self(out)
    }

    fn eval(&self, p: [f32; 3]) -> f64 {
        let x = f64::from(p[0]);
        let y = f64::from(p[1]);
        let z = f64::from(p[2]);
        let q = &self.0;
        let val = q[0] * x * x
            + 2.0 * q[1] * x * y
            + 2.0 * q[2] * x * z
            + 2.0 * q[3] * x
            + q[4] * y * y
            + 2.0 * q[5] * y * z
            + 2.0 * q[6] * y
            + q[7] * z * z
            + 2.0 * q[8] * z
            + q[9];
        val.max(0.0)
    }

    fn solve_optimal(&self) -> Option<[f32; 3]> {
        let q = &self.0;
        let a00 = q[0];
        let a01 = q[1];
        let a02 = q[2];
        let a11 = q[4];
        let a12 = q[5];
        let a22 = q[7];
        let det = a00 * (a11 * a22 - a12 * a12) - a01 * (a01 * a22 - a12 * a02)
            + a02 * (a01 * a12 - a11 * a02);
        if det.abs() < 1e-8 {
            return None;
        }
        let b0 = -q[3];
        let b1 = -q[6];
        let b2 = -q[8];
        let c00 = a11 * a22 - a12 * a12;
        let c01 = -(a01 * a22 - a12 * a02);
        let c02 = a01 * a12 - a11 * a02;
        let c11 = a00 * a22 - a02 * a02;
        let c12 = -(a00 * a12 - a01 * a02);
        let c22 = a00 * a11 - a01 * a01;
        let x = (c00 * b0 + c01 * b1 + c02 * b2) / det;
        let y = (c01 * b0 + c11 * b1 + c12 * b2) / det;
        let z = (c02 * b0 + c12 * b1 + c22 * b2) / det;
        if x.is_finite() && y.is_finite() && z.is_finite() {
            Some([x as f32, y as f32, z as f32])
        } else {
            None
        }
    }
}

#[derive(Clone, Copy)]
struct CollapseCandidate {
    cost: f64,
    u: usize,
    v: usize,
    target_pos: [f32; 3],
    stamp_u: u32,
    stamp_v: u32,
}

impl PartialEq for CollapseCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.cost.to_bits() == other.cost.to_bits() && self.u == other.u && self.v == other.v
    }
}

impl Eq for CollapseCandidate {}

impl Ord for CollapseCandidate {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .cost
            .total_cmp(&self.cost)
            .then_with(|| self.u.cmp(&other.u))
            .then_with(|| self.v.cmp(&other.v))
    }
}

impl PartialOrd for CollapseCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn find_rep(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

fn group_frontier_batches(
    clusters: &[WorkingCluster],
    batches: &[Vec<usize>],
    verts: &[WorkingVertex],
    global_min: [f32; 3],
    global_max: [f32; 3],
) -> Vec<Vec<usize>> {
    let m = batches.len();
    if m == 0 {
        return Vec::new();
    }
    if m == 1 {
        return vec![vec![0]];
    }
    let total_clusters: usize = batches.iter().map(|b| b.len()).sum();
    if total_clusters <= 8 && m <= 8 {
        return vec![(0..m).collect()];
    }

    let mut edge_to_batches: HashMap<(PosKey, PosKey), Vec<usize>> = HashMap::new();
    let mut batch_centers = Vec::with_capacity(m);
    for (bi, batch) in batches.iter().enumerate() {
        let mut center = [0.0f32; 3];
        for &c_idx in batch {
            let c = &clusters[c_idx];
            center = add3(
                center,
                [
                    c.bounding_sphere[0],
                    c.bounding_sphere[1],
                    c.bounding_sphere[2],
                ],
            );
            for tri in &c.triangles {
                let k = [
                    pos_key(verts[c.vertex_ids[usize::from(tri[0])] as usize].pos),
                    pos_key(verts[c.vertex_ids[usize::from(tri[1])] as usize].pos),
                    pos_key(verts[c.vertex_ids[usize::from(tri[2])] as usize].pos),
                ];
                for e in 0..3 {
                    let ek = edge_pos_key(k[e], k[(e + 1) % 3]);
                    edge_to_batches.entry(ek).or_default().push(bi);
                }
            }
        }
        batch_centers.push(scale3(center, 1.0 / (batch.len().max(1) as f32)));
    }

    let mut adj: Vec<HashMap<usize, usize>> = vec![HashMap::new(); m];
    for shared in edge_to_batches.values() {
        if shared.len() < 2 {
            continue;
        }
        for i in 0..shared.len() {
            for j in (i + 1)..shared.len() {
                let a = shared[i];
                let b = shared[j];
                if a != b {
                    *adj[a].entry(b).or_insert(0) += 1;
                    *adj[b].entry(a).or_insert(0) += 1;
                }
            }
        }
    }

    let mut order: Vec<usize> = (0..m).collect();
    order.sort_by_key(|&i| morton_3d(batch_centers[i], global_min, global_max));

    let mut assigned = vec![false; m];
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for &seed in &order {
        if assigned[seed] {
            continue;
        }
        assigned[seed] = true;
        let mut grp = vec![seed];
        let mut cluster_sum = batches[seed].len();
        while cluster_sum < 6 && grp.len() < 8 {
            let mut best_nbr: Option<(usize, usize)> = None;
            for &member in &grp {
                for (&nbr, &weight) in &adj[member] {
                    if !assigned[nbr]
                        && cluster_sum + batches[nbr].len() <= 8
                        && best_nbr.is_none_or(|(_, bw)| weight > bw)
                    {
                        best_nbr = Some((nbr, weight));
                    }
                }
            }
            let Some((next, _)) = best_nbr else {
                break;
            };
            assigned[next] = true;
            cluster_sum += batches[next].len();
            grp.push(next);
        }
        groups.push(grp);
    }

    if groups.len() > 1 {
        let mut merged: Vec<Vec<usize>> = Vec::with_capacity(groups.len());
        for grp in groups {
            let cur_clusters: usize = grp.iter().map(|&bi| batches[bi].len()).sum();
            let prev_clusters: usize = merged.last().map_or(usize::MAX, |prev| {
                prev.iter().map(|&bi| batches[bi].len()).sum()
            });
            if cur_clusters < 2 && !merged.is_empty() && prev_clusters + cur_clusters <= 8 {
                merged.last_mut().unwrap().extend(grp);
            } else {
                merged.push(grp);
            }
        }
        merged
    } else {
        groups
    }
}

struct SimplifiedGroupOutput {
    parent_tris: Vec<([u32; 3], u16)>,
    max_qem_error: f32,
    collapsed_targets_per_cluster: Vec<Vec<[f32; 3]>>,
}

fn simplify_group_boundary_locked(
    group_clusters: &[usize],
    clusters: &[WorkingCluster],
    global_edge_counts: &HashMap<(PosKey, PosKey), usize>,
    verts: &mut Vec<WorkingVertex>,
) -> SimplifiedGroupOutput {
    let mut local_verts: Vec<WorkingVertex> = Vec::new();
    let mut global_to_local: HashMap<u32, usize> = HashMap::new();
    let mut local_tris: Vec<([usize; 3], u16)> = Vec::new();
    let mut cluster_vert_map: Vec<Vec<usize>> = Vec::with_capacity(group_clusters.len());
    let mut group_edge_counts: HashMap<(PosKey, PosKey), usize> = HashMap::new();
    let mut pos_key_vids: HashMap<PosKey, HashSet<u32>> = HashMap::new();

    for &c_idx in group_clusters {
        let cluster = &clusters[c_idx];
        let mut cmap = Vec::with_capacity(cluster.vertex_ids.len());
        for &vid in &cluster.vertex_ids {
            let lv = *global_to_local.entry(vid).or_insert_with(|| {
                let idx = local_verts.len();
                let wv = verts[vid as usize];
                local_verts.push(wv);
                pos_key_vids.entry(pos_key(wv.pos)).or_default().insert(vid);
                idx
            });
            cmap.push(lv);
        }
        for tri in &cluster.triangles {
            let i0 = cmap[usize::from(tri[0])];
            let i1 = cmap[usize::from(tri[1])];
            let i2 = cmap[usize::from(tri[2])];
            let k = [
                pos_key(local_verts[i0].pos),
                pos_key(local_verts[i1].pos),
                pos_key(local_verts[i2].pos),
            ];
            for e in 0..3 {
                *group_edge_counts
                    .entry(edge_pos_key(k[e], k[(e + 1) % 3]))
                    .or_insert(0) += 1;
            }
            local_tris.push(([i0, i1, i2], cluster.material_index));
        }
        cluster_vert_map.push(cmap);
    }

    let n_verts = local_verts.len();
    let mut locked = vec![false; n_verts];
    for (ek, &g_count) in &group_edge_counts {
        let global_c = global_edge_counts.get(ek).copied().unwrap_or(g_count);
        if g_count != 2 || global_c != g_count {
            for (lv_idx, v) in local_verts.iter().enumerate() {
                let pk = pos_key(v.pos);
                if pk == ek.0 || pk == ek.1 {
                    locked[lv_idx] = true;
                }
            }
        }
    }
    for (lv_idx, v) in local_verts.iter().enumerate() {
        let pk = pos_key(v.pos);
        if pos_key_vids.get(&pk).is_some_and(|s| s.len() > 1) {
            locked[lv_idx] = true;
        }
    }

    let mut quadrics = vec![Quadric::zero(); n_verts];
    for &(tri, _) in &local_tris {
        let p0 = local_verts[tri[0]].pos;
        let p1 = local_verts[tri[1]].pos;
        let p2 = local_verts[tri[2]].pos;
        let n = cross3(sub3(p1, p0), sub3(p2, p0));
        if len3(n) <= 1e-10 {
            continue;
        }
        let unit = normalize3(n);
        let d = -f64::from(dot3(unit, p0));
        let q = Quadric::from_plane(
            f64::from(unit[0]),
            f64::from(unit[1]),
            f64::from(unit[2]),
            d,
        );
        quadrics[tri[0]] = quadrics[tri[0]].add(q);
        quadrics[tri[1]] = quadrics[tri[1]].add(q);
        quadrics[tri[2]] = quadrics[tri[2]].add(q);
    }

    let mut tri_alive = vec![true; local_tris.len()];
    let mut vert_alive = vec![true; n_verts];
    let mut stamps = vec![0u32; n_verts];
    let mut rep: Vec<usize> = (0..n_verts).collect();
    let mut vert_tris: Vec<Vec<usize>> = vec![Vec::new(); n_verts];
    for (ti, &(tri, _)) in local_tris.iter().enumerate() {
        vert_tris[tri[0]].push(ti);
        vert_tris[tri[1]].push(ti);
        vert_tris[tri[2]].push(ti);
    }

    let evaluate_edge = |u: usize,
                         v: usize,
                         local_verts: &[WorkingVertex],
                         quadrics: &[Quadric],
                         locked: &[bool],
                         stamps: &[u32]|
     -> Option<CollapseCandidate> {
        if u == v || (locked[u] && locked[v]) {
            return None;
        }
        let q_sum = quadrics[u].add(quadrics[v]);
        let target_pos = if locked[u] {
            local_verts[u].pos
        } else if locked[v] {
            local_verts[v].pos
        } else if let Some(opt) = q_sum.solve_optimal() {
            opt
        } else {
            let pu = local_verts[u].pos;
            let pv = local_verts[v].pos;
            let pm = scale3(add3(pu, pv), 0.5);
            let candidates = [pu, pv, pm];
            *candidates
                .iter()
                .min_by(|&&a, &&b| q_sum.eval(a).total_cmp(&q_sum.eval(b)))
                .unwrap()
        };
        let cost = q_sum.eval(target_pos);
        Some(CollapseCandidate {
            cost,
            u,
            v,
            target_pos,
            stamp_u: stamps[u],
            stamp_v: stamps[v],
        })
    };

    let mut heap = BinaryHeap::new();
    let mut seen_edges = HashSet::new();
    for &(tri, _) in &local_tris {
        for e in 0..3 {
            let a = tri[e];
            let b = tri[(e + 1) % 3];
            let key = if a < b { (a, b) } else { (b, a) };
            if seen_edges.insert(key) {
                if let Some(cand) =
                    evaluate_edge(key.0, key.1, &local_verts, &quadrics, &locked, &stamps)
                {
                    heap.push(cand);
                }
            }
        }
    }

    let initial_tris = local_tris.len();
    let target_tris = (initial_tris / 2).max(1);
    let mut live_tris = initial_tris;
    let mut max_cost = 0.0f64;

    while live_tris > target_tris {
        let Some(cand) = heap.pop() else {
            break;
        };
        if !vert_alive[cand.u]
            || !vert_alive[cand.v]
            || stamps[cand.u] != cand.stamp_u
            || stamps[cand.v] != cand.stamp_v
        {
            continue;
        }
        let (keep, drop) = if locked[cand.v] {
            (cand.v, cand.u)
        } else {
            (cand.u, cand.v)
        };
        if locked[drop] {
            continue;
        }

        let mut nbr_keep: HashSet<usize> = HashSet::new();
        for &ti in &vert_tris[keep] {
            if !tri_alive[ti] {
                continue;
            }
            for &v in &local_tris[ti].0 {
                if v != keep {
                    nbr_keep.insert(v);
                }
            }
        }
        let mut nbr_drop: HashSet<usize> = HashSet::new();
        for &ti in &vert_tris[drop] {
            if !tri_alive[ti] {
                continue;
            }
            for &v in &local_tris[ti].0 {
                if v != drop {
                    nbr_drop.insert(v);
                }
            }
        }
        let mut shared_tris = Vec::new();
        let mut opposite_verts: HashSet<usize> = HashSet::new();
        for &ti in &vert_tris[drop] {
            if !tri_alive[ti] {
                continue;
            }
            let tri = local_tris[ti].0;
            if tri.contains(&keep) {
                shared_tris.push(ti);
                for &v in &tri {
                    if v != keep && v != drop {
                        opposite_verts.insert(v);
                    }
                }
            }
        }
        let common_nbrs: HashSet<usize> = nbr_keep.intersection(&nbr_drop).copied().collect();
        if common_nbrs != opposite_verts {
            continue;
        }

        let mut normal_ok = true;
        for &ti in vert_tris[keep].iter().chain(vert_tris[drop].iter()) {
            if !tri_alive[ti] {
                continue;
            }
            let tri = local_tris[ti].0;
            if tri.contains(&keep) && tri.contains(&drop) {
                continue;
            }
            let old_p = [
                local_verts[tri[0]].pos,
                local_verts[tri[1]].pos,
                local_verts[tri[2]].pos,
            ];
            let new_p = [
                if tri[0] == keep || tri[0] == drop {
                    cand.target_pos
                } else {
                    old_p[0]
                },
                if tri[1] == keep || tri[1] == drop {
                    cand.target_pos
                } else {
                    old_p[1]
                },
                if tri[2] == keep || tri[2] == drop {
                    cand.target_pos
                } else {
                    old_p[2]
                },
            ];
            let old_n = cross3(sub3(old_p[1], old_p[0]), sub3(old_p[2], old_p[0]));
            let new_n = cross3(sub3(new_p[1], new_p[0]), sub3(new_p[2], new_p[0]));
            if len3(new_n) <= 1e-10 {
                normal_ok = false;
                break;
            }
            if len3(old_n) > 1e-10 && dot3(normalize3(old_n), normalize3(new_n)) <= 0.2 {
                normal_ok = false;
                break;
            }
        }
        if !normal_ok {
            continue;
        }

        max_cost = max_cost.max(cand.cost);
        if !locked[keep] {
            local_verts[keep].pos = cand.target_pos;
            local_verts[keep].normal =
                normalize3(add3(local_verts[keep].normal, local_verts[drop].normal));
            local_verts[keep].uv = [
                0.5 * (local_verts[keep].uv[0] + local_verts[drop].uv[0]),
                0.5 * (local_verts[keep].uv[1] + local_verts[drop].uv[1]),
            ];
        }
        quadrics[keep] = quadrics[keep].add(quadrics[drop]);
        vert_alive[drop] = false;
        rep[drop] = keep;
        stamps[keep] = stamps[keep].wrapping_add(1);

        for &ti in &shared_tris {
            if tri_alive[ti] {
                tri_alive[ti] = false;
                live_tris -= 1;
            }
        }
        let drop_tris = std::mem::take(&mut vert_tris[drop]);
        for ti in drop_tris {
            if !tri_alive[ti] {
                continue;
            }
            for v_slot in &mut local_tris[ti].0 {
                if *v_slot == drop {
                    *v_slot = keep;
                }
            }
            let t = local_tris[ti].0;
            if t[0] == t[1] || t[1] == t[2] || t[2] == t[0] {
                tri_alive[ti] = false;
                live_tris -= 1;
            } else {
                vert_tris[keep].push(ti);
            }
        }

        let mut touched = HashSet::new();
        for &ti in &vert_tris[keep] {
            if !tri_alive[ti] {
                continue;
            }
            for &v in &local_tris[ti].0 {
                if v != keep && vert_alive[v] && touched.insert(v) {
                    if let Some(next_cand) =
                        evaluate_edge(keep, v, &local_verts, &quadrics, &locked, &stamps)
                    {
                        heap.push(next_cand);
                    }
                }
            }
        }
    }

    let mut collapsed_targets_per_cluster = Vec::with_capacity(cluster_vert_map.len());
    for cmap in &cluster_vert_map {
        let mut targets = Vec::with_capacity(cmap.len());
        for &lv in cmap {
            let root = find_rep(&mut rep, lv);
            targets.push(local_verts[root].pos);
        }
        collapsed_targets_per_cluster.push(targets);
    }

    let mut local_to_new_global: HashMap<usize, u32> = HashMap::new();
    let mut parent_tris = Vec::with_capacity(live_tris);
    for (ti, &(tri, mat)) in local_tris.iter().enumerate() {
        if !tri_alive[ti] {
            continue;
        }
        let mut gtri = [0u32; 3];
        for slot in 0..3 {
            let lv = tri[slot];
            let gvid = *local_to_new_global.entry(lv).or_insert_with(|| {
                let id = verts.len() as u32;
                verts.push(local_verts[lv]);
                id
            });
            gtri[slot] = gvid;
        }
        parent_tris.push((gtri, mat));
    }

    SimplifiedGroupOutput {
        parent_tris,
        max_qem_error: (max_cost.max(0.0) as f32).sqrt(),
        collapsed_targets_per_cluster,
    }
}

fn pack_cluster_to_rcld(
    wc: &WorkingCluster,
    verts: &[WorkingVertex],
    is_skinned: bool,
) -> RcldCluster {
    let mut bone_palette = [0u16; RCLD_MAX_CLUSTER_BONES];
    let mut bone_map: HashMap<u16, u8> = HashMap::new();
    let mut bone_count = 0u8;

    if is_skinned {
        for &vid in &wc.vertex_ids {
            let wv = &verts[vid as usize];
            for slot in 0..4 {
                if wv.weights[slot] > 1e-4 {
                    let j = wv.joints[slot];
                    if !bone_map.contains_key(&j)
                        && usize::from(bone_count) < RCLD_MAX_CLUSTER_BONES
                    {
                        bone_map.insert(j, bone_count);
                        bone_palette[usize::from(bone_count)] = j;
                        bone_count += 1;
                    }
                }
            }
        }
    }

    let mut max_delta = 0.0f32;
    let mut deltas = Vec::with_capacity(wc.vertex_ids.len());
    for (idx, &vid) in wc.vertex_ids.iter().enumerate() {
        let d = sub3(wc.geomorph_targets[idx], verts[vid as usize].pos);
        max_delta = max_delta.max(d[0].abs()).max(d[1].abs()).max(d[2].abs());
        deltas.push(d);
    }

    let mut packed_verts = Vec::with_capacity(wc.vertex_ids.len());
    let mut dmm_displacements = Vec::new();
    for (idx, &vid) in wc.vertex_ids.iter().enumerate() {
        let wv = &verts[vid as usize];
        let b0 = bone_map.get(&wv.joints[0]).copied().unwrap_or(0) & 0x0F;
        let b1 = bone_map.get(&wv.joints[1]).copied().unwrap_or(0) & 0x0F;
        packed_verts.push(QuantizedClusterVertex {
            pos_q: QuantizedClusterVertex::quantize_position(wv.pos, wc.aabb_min, wc.aabb_max),
            normal_oct: encode_octahedral_normal(wv.normal),
            tangent_angle: 0,
            bone_local_pair: (b1 << 4) | b0,
            uv_f16: [f32_to_f16(wv.uv[0]), f32_to_f16(wv.uv[1])],
            geomorph_delta_q: QuantizedClusterVertex::quantize_geomorph_delta(
                deltas[idx],
                max_delta,
            ),
        });
        if wc.has_dmm {
            let q = (wv.disp * 1023.0).round().clamp(-1024.0, 1023.0) as i16;
            dmm_displacements.push(q);
        }
    }

    RcldCluster {
        cluster_id: wc.cluster_id,
        group_id: wc.group_id,
        lod_level: wc.lod_level,
        primitive_kind: wc.primitive_kind,
        material_index: wc.material_index,
        aabb_min: wc.aabb_min,
        aabb_max: wc.aabb_max,
        normal_cone: wc.normal_cone,
        parent_error: wc.parent_error,
        self_error: wc.self_error,
        geomorph_scale: max_delta,
        bone_palette,
        bone_count,
        max_skin_inflation: if is_skinned {
            0.15 * wc.bounding_sphere[3]
        } else {
            0.0
        },
        vertices: packed_verts,
        triangles: wc.triangles.clone(),
        dmm_displacements,
        splat_surrogate: wc.splat_surrogate,
    }
}

/// Builds a paged, boundary-locked Cluster-DAG [`RcldAsset`] from raw geometry.
pub fn build_virtual_geometry_asset(
    input: &VirtualGeoBuildInput<'_>,
) -> Result<RcldAsset, CookError> {
    if input.positions.is_empty()
        || input.indices.len() < 3
        || !input.indices.len().is_multiple_of(3)
    {
        return Err(CookError::CookFailed(
            "Virtual geometry requires non-empty positions and triangle indices".into(),
        ));
    }

    let has_normals = input.normals.len() == input.positions.len();
    let computed_normals;
    let normals_ref: &[[f32; 3]] = if has_normals {
        input.normals
    } else {
        computed_normals = compute_smooth_normals(input.positions, input.indices);
        &computed_normals
    };
    let has_uvs = input.uvs.len() == input.positions.len();
    let is_skinned = input
        .joints
        .is_some_and(|j| j.len() == input.positions.len())
        && input
            .weights
            .is_some_and(|w| w.len() == input.positions.len());
    let has_dmm = input
        .displacements
        .is_some_and(|d| d.len() == input.positions.len());

    let mut verts: Vec<WorkingVertex> = (0..input.positions.len())
        .map(|i| WorkingVertex {
            pos: input.positions[i],
            normal: normals_ref[i],
            uv: if has_uvs { input.uvs[i] } else { [0.0, 0.0] },
            joints: input.joints.map_or([0; 4], |j| j[i]),
            weights: input.weights.map_or([1.0, 0.0, 0.0, 0.0], |w| w[i]),
            disp: input.displacements.map_or(0.0, |d| d[i]),
        })
        .collect();

    let (global_min, global_max) = compute_aabb(input.positions);
    let mut base_tris = Vec::with_capacity(input.indices.len() / 3);
    for (ti, tri) in input.indices.as_chunks::<3>().0.iter().enumerate() {
        let i0 = tri[0];
        let i1 = tri[1];
        let i2 = tri[2];
        if i0 as usize >= verts.len() || i1 as usize >= verts.len() || i2 as usize >= verts.len() {
            return Err(CookError::CookFailed(
                "Virtual geometry triangle index out of bounds".into(),
            ));
        }
        let mat = input
            .triangle_materials
            .and_then(|m| m.get(ti).copied())
            .unwrap_or(0);
        base_tris.push(([i0, i1, i2], mat));
    }

    let mut all_clusters = partition_triangles_into_clusters(
        &verts,
        &base_tris,
        &PartitionConfig {
            lod_level: 0,
            self_error: 0.0,
            global_min,
            global_max,
            allow_dmm: has_dmm,
            foliage_splat_mode: false,
        },
    );
    struct FrontierBatch {
        clusters: Vec<usize>,
        child_group: Option<usize>,
    }

    let mut frontier_batches: Vec<FrontierBatch> = (0..all_clusters.len())
        .map(|ci| FrontierBatch {
            clusters: vec![ci],
            child_group: None,
        })
        .collect();

    struct PendingGroup {
        lod_level: u8,
        cluster_indices: Vec<usize>,
        child_group_indices: Vec<usize>,
        self_error: f32,
        parent_error: f32,
        bounding_sphere: [f32; 4],
        primitive_kind: VirtualGeoPrimitiveKind,
    }

    let mut pending_groups: Vec<PendingGroup> = Vec::new();
    let mut max_lod_level = 0u8;

    for lod in 0..MAX_DAG_LOD_LEVELS {
        max_lod_level = lod;
        let batch_slices: Vec<Vec<usize>> = frontier_batches
            .iter()
            .map(|b| b.clusters.clone())
            .collect();
        let batch_groups =
            group_frontier_batches(&all_clusters, &batch_slices, &verts, global_min, global_max);

        let mut global_edge_counts: HashMap<(PosKey, PosKey), usize> = HashMap::new();
        for batch in &frontier_batches {
            for &c_idx in &batch.clusters {
                let c = &all_clusters[c_idx];
                for tri in &c.triangles {
                    let k = [
                        pos_key(verts[c.vertex_ids[usize::from(tri[0])] as usize].pos),
                        pos_key(verts[c.vertex_ids[usize::from(tri[1])] as usize].pos),
                        pos_key(verts[c.vertex_ids[usize::from(tri[2])] as usize].pos),
                    ];
                    for e in 0..3 {
                        *global_edge_counts
                            .entry(edge_pos_key(k[e], k[(e + 1) % 3]))
                            .or_insert(0) += 1;
                    }
                }
            }
        }

        let mut next_batches = Vec::new();
        let can_ascend = batch_groups.len() > 1 && lod + 1 < MAX_DAG_LOD_LEVELS;

        for batch_idx_list in batch_groups {
            let mut grp_clusters = Vec::new();
            let mut child_groups = Vec::new();
            for bi in batch_idx_list {
                grp_clusters.extend_from_slice(&frontier_batches[bi].clusters);
                if let Some(cg) = frontier_batches[bi].child_group {
                    child_groups.push(cg);
                }
            }

            let mut max_child_err = 0.0f32;
            let mut grp_min = [f32::INFINITY; 3];
            let mut grp_max = [f32::NEG_INFINITY; 3];
            for &c_idx in &grp_clusters {
                let c = &all_clusters[c_idx];
                max_child_err = max_child_err.max(c.self_error);
                for axis in 0..3 {
                    grp_min[axis] = grp_min[axis].min(c.aabb_min[axis]);
                    grp_max[axis] = grp_max[axis].max(c.aabb_max[axis]);
                }
            }
            for &cg in &child_groups {
                pending_groups[cg].parent_error = max_child_err;
                for &cc in &pending_groups[cg].cluster_indices {
                    all_clusters[cc].parent_error = max_child_err;
                }
            }
            for &c_idx in &grp_clusters {
                all_clusters[c_idx].self_error = max_child_err;
            }

            let center = [
                0.5 * (grp_min[0] + grp_max[0]),
                0.5 * (grp_min[1] + grp_max[1]),
                0.5 * (grp_min[2] + grp_max[2]),
            ];
            let half = sub3(grp_max, center);
            let sphere = [center[0], center[1], center[2], len3(half).max(1e-4)];

            if !can_ascend {
                let root_parent_error = (max_child_err + 1.0) * 10_000.0;
                for &c_idx in &grp_clusters {
                    all_clusters[c_idx].parent_error = root_parent_error;
                }
                let pkind = all_clusters[grp_clusters[0]].primitive_kind;
                pending_groups.push(PendingGroup {
                    lod_level: lod,
                    cluster_indices: grp_clusters,
                    child_group_indices: child_groups,
                    self_error: max_child_err,
                    parent_error: root_parent_error,
                    bounding_sphere: sphere,
                    primitive_kind: pkind,
                });
                continue;
            }

            let simp = simplify_group_boundary_locked(
                &grp_clusters,
                &all_clusters,
                &global_edge_counts,
                &mut verts,
            );
            let group_parent_error = simp
                .max_qem_error
                .max(max_child_err + (sphere[3] * 1e-3).max(1e-4));

            for (slot, &c_idx) in grp_clusters.iter().enumerate() {
                all_clusters[c_idx].parent_error = group_parent_error;
                all_clusters[c_idx].geomorph_targets =
                    simp.collapsed_targets_per_cluster[slot].clone();
            }

            let child_g_idx = pending_groups.len();
            let pkind = all_clusters[grp_clusters[0]].primitive_kind;
            pending_groups.push(PendingGroup {
                lod_level: lod,
                cluster_indices: grp_clusters.clone(),
                child_group_indices: child_groups,
                self_error: max_child_err,
                parent_error: group_parent_error,
                bounding_sphere: sphere,
                primitive_kind: pkind,
            });

            if !simp.parent_tris.is_empty() {
                let parent_clusters = partition_triangles_into_clusters(
                    &verts,
                    &simp.parent_tris,
                    &PartitionConfig {
                        lod_level: lod + 1,
                        self_error: group_parent_error,
                        global_min,
                        global_max,
                        allow_dmm: false,
                        foliage_splat_mode: input.alpha_masked_foliage,
                    },
                );
                let mut parent_indices = Vec::with_capacity(parent_clusters.len());
                for pc in parent_clusters {
                    let new_idx = all_clusters.len();
                    all_clusters.push(pc);
                    parent_indices.push(new_idx);
                }
                next_batches.push(FrontierBatch {
                    clusters: parent_indices,
                    child_group: Some(child_g_idx),
                });
            }
        }

        if !can_ascend || next_batches.is_empty() {
            break;
        }
        frontier_batches = next_batches;
    }

    let root_old_idx = pending_groups
        .iter()
        .enumerate()
        .max_by_key(|&(idx, pg)| (pg.lod_level, idx))
        .map(|(idx, _)| idx)
        .unwrap_or(0);

    let mut ordered_group_indices: Vec<usize> = Vec::with_capacity(pending_groups.len());
    let mut visited = vec![false; pending_groups.len()];
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(root_old_idx);
    visited[root_old_idx] = true;
    while let Some(g_idx) = queue.pop_front() {
        ordered_group_indices.push(g_idx);
        for &child_idx in &pending_groups[g_idx].child_group_indices {
            if !visited[child_idx] {
                visited[child_idx] = true;
                queue.push_back(child_idx);
            }
        }
    }
    for (idx, &was_visited) in visited.iter().enumerate() {
        if !was_visited {
            ordered_group_indices.push(idx);
        }
    }

    let mut old_to_new_group = vec![0usize; pending_groups.len()];
    for (new_idx, &old_idx) in ordered_group_indices.iter().enumerate() {
        old_to_new_group[old_idx] = new_idx;
    }

    let mut packed_clusters = Vec::with_capacity(all_clusters.len());
    let mut final_groups = Vec::with_capacity(pending_groups.len());
    let mut pages: Vec<RcldPageDescriptor> = Vec::new();
    let mut cur_page_bytes = 0usize;
    let mut cur_page_first_cluster = 0u32;
    let mut cur_page_cluster_count = 0u16;
    let mut cur_page_lod = pending_groups[ordered_group_indices[0]].lod_level;

    for (new_group_id, &old_group_idx) in ordered_group_indices.iter().enumerate() {
        let pg = &pending_groups[old_group_idx];
        let mut group_rcld_clusters = Vec::with_capacity(pg.cluster_indices.len());
        let mut group_bytes = 0usize;
        for &c_idx in &pg.cluster_indices {
            let rc = pack_cluster_to_rcld(&all_clusters[c_idx], &verts, is_skinned);
            group_bytes += rc.encoded_byte_size();
            group_rcld_clusters.push(rc);
        }

        let is_root_group = new_group_id == 0;
        let exceeds_page = cur_page_cluster_count > 0
            && (cur_page_bytes + group_bytes > RCLD_PAGE_SIZE
                || (pages.is_empty() && !is_root_group && pg.lod_level != cur_page_lod));

        if exceeds_page {
            let p_idx = pages.len() as u32;
            let p_flags = if p_idx == 0 {
                RCLD_PAGE_FLAG_RESIDENT_ROOT
            } else {
                0
            };
            pages.push(RcldPageDescriptor {
                page_index: p_idx,
                byte_offset: 0,
                byte_size: cur_page_bytes as u32,
                first_cluster: cur_page_first_cluster,
                cluster_count: cur_page_cluster_count,
                lod_level: cur_page_lod,
                flags: p_flags,
            });
            cur_page_bytes = 0;
            cur_page_first_cluster = packed_clusters.len() as u32;
            cur_page_cluster_count = 0;
            cur_page_lod = pg.lod_level;
        }

        let assigned_page = pages.len() as u32;
        let first_cluster = packed_clusters.len() as u32;
        let cluster_count = group_rcld_clusters.len() as u16;
        for mut rc in group_rcld_clusters {
            rc.cluster_id = packed_clusters.len() as u32;
            rc.group_id = new_group_id as u32;
            packed_clusters.push(rc);
        }
        cur_page_bytes += group_bytes;
        cur_page_cluster_count += cluster_count;

        final_groups.push(RcldClusterGroup {
            group_id: new_group_id as u32,
            lod_level: pg.lod_level,
            primitive_kind: pg.primitive_kind,
            child_group_count: 0,
            page_index: assigned_page,
            child_group_first: 0,
            first_cluster,
            cluster_count,
            parent_error: pg.parent_error,
            self_error: pg.self_error,
            bounding_sphere: pg.bounding_sphere,
        });
    }

    if cur_page_cluster_count > 0 {
        let p_idx = pages.len() as u32;
        pages.push(RcldPageDescriptor {
            page_index: p_idx,
            byte_offset: 0,
            byte_size: cur_page_bytes as u32,
            first_cluster: cur_page_first_cluster,
            cluster_count: cur_page_cluster_count,
            lod_level: cur_page_lod,
            flags: if p_idx == 0 {
                RCLD_PAGE_FLAG_RESIDENT_ROOT
            } else {
                0
            },
        });
    }

    for (new_group_id, &old_group_idx) in ordered_group_indices.iter().enumerate() {
        let pg = &pending_groups[old_group_idx];
        if !pg.child_group_indices.is_empty() {
            let mapped: Vec<u32> = pg
                .child_group_indices
                .iter()
                .map(|&old| old_to_new_group[old] as u32)
                .collect();
            let min_child = *mapped.iter().min().unwrap();
            let max_child = *mapped.iter().max().unwrap();
            if (max_child - min_child + 1) as usize == mapped.len() {
                final_groups[new_group_id].child_group_first = min_child;
                final_groups[new_group_id].child_group_count = mapped.len() as u16;
            }
        }
    }

    let mut flags = RCLD_FLAG_GEOMORPHING;
    if is_skinned {
        flags |= RCLD_FLAG_SKINNED;
    }
    if has_dmm {
        flags |= RCLD_FLAG_HAS_DMM;
    }
    if packed_clusters.iter().any(|c| c.splat_surrogate.is_some()) {
        flags |= RCLD_FLAG_HAS_SPLATS;
    }

    let asset = RcldAsset {
        header: RcldHeader {
            version: RCLD_VERSION,
            flags,
            max_lod_level: u16::from(max_lod_level),
            page_count: pages.len() as u32,
            group_count: final_groups.len() as u32,
            cluster_count: packed_clusters.len() as u32,
            root_group_index: 0,
            bounds_min: global_min,
            bounds_max: global_max,
            page_table_offset: 64,
            group_table_offset: 64 + (pages.len() as u32) * 20,
            payload_offset: 64 + (pages.len() as u32) * 20 + (final_groups.len() as u32) * 48,
        },
        pages,
        groups: final_groups,
        clusters: packed_clusters,
    };
    asset
        .validate_invariants()
        .map_err(|e| CookError::CookFailed(format!("RCLD invariant check failed: {e}")))?;
    Ok(asset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_asset::formats_virtual_geo::{decode_rcld, encode_rcld_bytes};

    struct TestGridMesh {
        positions: Vec<[f32; 3]>,
        normals: Vec<[f32; 3]>,
        uvs: Vec<[f32; 2]>,
        indices: Vec<u32>,
    }

    fn make_grid_mesh(nx: u32, nz: u32) -> TestGridMesh {
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut uvs = Vec::new();
        let mut indices = Vec::new();

        for z in 0..=nz {
            for x in 0..=nx {
                let fx = x as f32 * 0.25;
                let fz = z as f32 * 0.25;
                let fy = (fx * 0.8).sin() * (fz * 0.6).cos() * 0.4;
                positions.push([fx, fy, fz]);
                normals.push([0.0, 1.0, 0.0]);
                uvs.push([x as f32 / nx as f32, z as f32 / nz as f32]);
            }
        }
        let stride = nx + 1;
        for z in 0..nz {
            for x in 0..nx {
                let v0 = z * stride + x;
                let v1 = v0 + 1;
                let v2 = v0 + stride;
                let v3 = v2 + 1;
                indices.extend_from_slice(&[v0, v2, v1, v1, v2, v3]);
            }
        }
        TestGridMesh {
            positions,
            normals,
            uvs,
            indices,
        }
    }

    fn group_boundary_edges(asset: &RcldAsset, group_idx: usize) -> HashSet<(PosKey, PosKey)> {
        let grp = &asset.groups[group_idx];
        let mut edge_counts: HashMap<(PosKey, PosKey), usize> = HashMap::new();
        let start = grp.first_cluster as usize;
        let end = start + usize::from(grp.cluster_count);
        for cluster in &asset.clusters[start..end] {
            for tri in &cluster.triangles {
                let p = [
                    cluster.vertices[usize::from(tri[0])].dequantize_position(
                        cluster.aabb_min,
                        cluster.aabb_max,
                        cluster.geomorph_scale,
                        0.0,
                    ),
                    cluster.vertices[usize::from(tri[1])].dequantize_position(
                        cluster.aabb_min,
                        cluster.aabb_max,
                        cluster.geomorph_scale,
                        0.0,
                    ),
                    cluster.vertices[usize::from(tri[2])].dequantize_position(
                        cluster.aabb_min,
                        cluster.aabb_max,
                        cluster.geomorph_scale,
                        0.0,
                    ),
                ];
                let k = [pos_key(p[0]), pos_key(p[1]), pos_key(p[2])];
                for e in 0..3 {
                    *edge_counts
                        .entry(edge_pos_key(k[e], k[(e + 1) % 3]))
                        .or_insert(0) += 1;
                }
            }
        }
        edge_counts
            .into_iter()
            .filter_map(|(ek, count)| (count == 1).then_some(ek))
            .collect()
    }

    #[test]
    fn build_cluster_dag_is_monotonic_and_crack_free() {
        let grid = make_grid_mesh(28, 28);
        let input = VirtualGeoBuildInput {
            positions: &grid.positions,
            normals: &grid.normals,
            uvs: &grid.uvs,
            indices: &grid.indices,
            triangle_materials: None,
            joints: None,
            weights: None,
            displacements: None,
            alpha_masked_foliage: false,
        };
        let asset = build_virtual_geometry_asset(&input).expect("DAG build must succeed");
        assert!(asset.header.max_lod_level >= 1, "expected multi-level DAG");
        assert!(asset.validate_invariants().is_ok());

        let bytes = encode_rcld_bytes(&asset).expect("RCLD encoding must succeed");
        let decoded = decode_rcld(&bytes).expect("RCLD decoding must succeed");
        assert_eq!(decoded.clusters.len(), asset.clusters.len());

        let mut reachable_groups = 0usize;
        let mut stack = vec![decoded.header.root_group_index as usize];
        let mut seen = vec![false; decoded.groups.len()];
        seen[decoded.header.root_group_index as usize] = true;
        while let Some(gi) = stack.pop() {
            reachable_groups += 1;
            let grp = &decoded.groups[gi];
            let start = grp.child_group_first as usize;
            let end = start + usize::from(grp.child_group_count);
            for (offset, child) in decoded.groups[start..end].iter().enumerate() {
                let ci = start + offset;
                assert!(
                    (child.parent_error - grp.self_error).abs() < 1e-6,
                    "child parent_error {} must equal parent self_error {}",
                    child.parent_error,
                    grp.self_error
                );
                if !seen[ci] {
                    seen[ci] = true;
                    stack.push(ci);
                }
            }
        }
        assert_eq!(
            reachable_groups,
            decoded.groups.len(),
            "BFS/DFS from root group must reach every group in the DAG"
        );

        let lod0_groups: Vec<usize> = decoded
            .groups
            .iter()
            .enumerate()
            .filter_map(|(i, g)| (g.lod_level == 0).then_some(i))
            .collect();
        let lod1_groups: Vec<usize> = decoded
            .groups
            .iter()
            .enumerate()
            .filter_map(|(i, g)| (g.lod_level == 1).then_some(i))
            .collect();
        assert!(!lod0_groups.is_empty() && !lod1_groups.is_empty());

        let mut lod0_boundary_verts = HashSet::new();
        for &gi in &lod0_groups {
            for (a, b) in group_boundary_edges(&decoded, gi) {
                lod0_boundary_verts.insert(a);
                lod0_boundary_verts.insert(b);
            }
        }

        let mut shared_boundary_edges_checked = 0usize;
        for &gi1 in &lod1_groups {
            let b1 = group_boundary_edges(&decoded, gi1);
            for (ea, eb) in b1 {
                if lod0_boundary_verts.contains(&ea) && lod0_boundary_verts.contains(&eb) {
                    shared_boundary_edges_checked += 1;
                }
            }
        }
        assert!(
            shared_boundary_edges_checked > 0,
            "mixed LOD cuts must preserve exact shared boundary vertices without cracks"
        );
    }

    #[test]
    fn geomorphing_and_multimodal_foliage_dmm_build() {
        let grid = make_grid_mesh(24, 24);
        let displacements: Vec<f32> = (0..grid.positions.len())
            .map(|i| ((i % 17) as f32 - 8.0) / 8.0)
            .collect();
        let joints = vec![[0u16, 1, 0, 0]; grid.positions.len()];
        let weights = vec![[0.75f32, 0.25, 0.0, 0.0]; grid.positions.len()];
        let input = VirtualGeoBuildInput {
            positions: &grid.positions,
            normals: &grid.normals,
            uvs: &grid.uvs,
            indices: &grid.indices,
            triangle_materials: None,
            joints: Some(&joints),
            weights: Some(&weights),
            displacements: Some(&displacements),
            alpha_masked_foliage: true,
        };

        let asset =
            build_virtual_geometry_asset(&input).expect("multimodal DAG build must succeed");
        assert_ne!(asset.header.flags & RCLD_FLAG_SKINNED, 0);
        assert_ne!(asset.header.flags & RCLD_FLAG_HAS_DMM, 0);
        assert_ne!(asset.header.flags & RCLD_FLAG_HAS_SPLATS, 0);
        assert_ne!(asset.header.flags & RCLD_FLAG_GEOMORPHING, 0);

        let any_nonzero_morph = asset.clusters.iter().any(|c| c.geomorph_scale > 1e-6);
        assert!(
            any_nonzero_morph,
            "expected continuous geomorphing deltas on simplified child clusters"
        );
        let any_splat = asset.clusters.iter().any(|c| {
            c.primitive_kind == VirtualGeoPrimitiveKind::SggxGaussianSplats
                && c.splat_surrogate.is_some()
        });
        assert!(
            any_splat,
            "expected far-field SGGX surfel surrogate clusters"
        );
    }
}
