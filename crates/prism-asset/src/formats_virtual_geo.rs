//! Virtual geometry paged container format (`RCLD` v1).
//!
//! Pure data definitions and lossless encode/decode helpers shared between
//! `prism-cooker` (offline Cluster-DAG builder) and runtime consumers.
//! No Vulkan handles or external dependencies are used here.
//!
//! Binary layout (all scalars little-endian):
//! - `RcldHeader` (64 bytes)
//! - `PageDescriptorTable` (`page_count * 20` bytes)
//! - `ClusterGroupTable` (`group_count * 48` bytes)
//! - `PagePayloads` (Page 0 is the pinned resident root, Pages 1..N are streamable)

use crate::formats_skinned::Reader;

/// Magic bytes identifying a virtual geometry `RCLD` blob.
pub const RCLD_MAGIC: &[u8; 4] = b"RCLD";
/// Current `RCLD` binary wire version.
pub const RCLD_VERSION: u8 = 1;
/// Target streaming page budget in bytes (64 KiB).
pub const RCLD_PAGE_SIZE: usize = 65_536;
/// Hard per-cluster vertex limit.
pub const RCLD_MAX_CLUSTER_VERTICES: usize = 128;
/// Hard per-cluster triangle limit (local indices fit in 7 bits of VisBuffer).
pub const RCLD_MAX_CLUSTER_TRIANGLES: usize = 128;
/// Maximum number of bones in a cluster's local skinning palette.
pub const RCLD_MAX_CLUSTER_BONES: usize = 8;

/// Asset carries per-cluster skeletal skinning palettes and bone bindings.
pub const RCLD_FLAG_SKINNED: u8 = 1 << 0;
/// Asset contains displaced micro-mesh scalar displacement blocks.
pub const RCLD_FLAG_HAS_DMM: u8 = 1 << 1;
/// Asset contains far-field SGGX 3D Gaussian surfel surrogates.
pub const RCLD_FLAG_HAS_SPLATS: u8 = 1 << 2;
/// Vertices carry continuous geomorphing delta residuals toward parent LODs.
pub const RCLD_FLAG_GEOMORPHING: u8 = 1 << 3;

/// Marks Page 0 as permanently pinned in memory as the coarsest fallback.
pub const RCLD_PAGE_FLAG_RESIDENT_ROOT: u8 = 1 << 0;

const RCLD_HEADER_SIZE: usize = 64;
const RCLD_PAGE_DESC_SIZE: usize = 20;
const RCLD_GROUP_RECORD_SIZE: usize = 48;
const RCLD_VERTEX_SIZE: usize = 20;
const RCLD_SPLAT_SIZE: usize = 68;

/// Geometric representation stored inside a cluster or cluster group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum VirtualGeoPrimitiveKind {
    Triangles = 0,
    DisplacedMicroMesh = 1,
    SggxGaussianSplats = 2,
}

impl VirtualGeoPrimitiveKind {
    /// Decodes a wire byte into a primitive kind.
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Triangles),
            1 => Some(Self::DisplacedMicroMesh),
            2 => Some(Self::SggxGaussianSplats),
            _ => None,
        }
    }
}

/// Converts a 32-bit float to IEEE-754 binary16 bits.
pub fn f32_to_f16(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xFF) as i32;
    let mant = bits & 0x007F_FFFF;

    if exp == 0xFF {
        if mant == 0 {
            return sign | 0x7C00;
        }
        return sign | 0x7E00 | ((mant >> 13) as u16).max(1);
    }

    let unbiased = exp - 127 + 15;
    if unbiased >= 31 {
        return sign | 0x7C00;
    }
    if unbiased <= 0 {
        if unbiased < -10 {
            return sign;
        }
        let m = (mant | 0x0080_0000) >> (1 - unbiased + 13);
        return sign | (m as u16);
    }
    let rounded = mant + 0x1000;
    if rounded & 0x0080_0000 != 0 {
        let new_exp = unbiased + 1;
        if new_exp >= 31 {
            return sign | 0x7C00;
        }
        return sign | ((new_exp as u16) << 10);
    }
    sign | ((unbiased as u16) << 10) | ((rounded >> 13) as u16)
}

/// Converts IEEE-754 binary16 bits back to a 32-bit float.
pub fn f16_to_f32(bits: u16) -> f32 {
    let sign = u32::from(bits & 0x8000) << 16;
    let exp = u32::from((bits >> 10) & 0x1F);
    let mant = u32::from(bits & 0x03FF);

    let out_bits = if exp == 0 {
        if mant == 0 {
            sign
        } else {
            let mut m = mant;
            let mut e: i32 = 127 - 15 + 1;
            while (m & 0x0400) == 0 {
                m <<= 1;
                e -= 1;
            }
            m &= 0x03FF;
            sign | ((e as u32) << 23) | (m << 13)
        }
    } else if exp == 31 {
        if mant == 0 {
            sign | 0x7F80_0000
        } else {
            sign | 0x7FC0_0000 | (mant << 13)
        }
    } else {
        let e = exp + (127 - 15);
        sign | (e << 23) | (mant << 13)
    };
    f32::from_bits(out_bits)
}

/// Encodes a unit normal vector into 2-byte signed octahedral coordinates.
pub fn encode_octahedral_normal(n: [f32; 3]) -> [i8; 2] {
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    let (nx, ny, nz) = if len.is_finite() && len > 1e-8 {
        (n[0] / len, n[1] / len, n[2] / len)
    } else {
        (0.0, 1.0, 0.0)
    };
    let l1 = nx.abs() + ny.abs() + nz.abs();
    let mut ox = nx / l1;
    let mut oy = ny / l1;
    if nz < 0.0 {
        let sx = if ox >= 0.0 { 1.0 } else { -1.0 };
        let sy = if oy >= 0.0 { 1.0 } else { -1.0 };
        let old_x = ox;
        ox = (1.0 - oy.abs()) * sx;
        oy = (1.0 - old_x.abs()) * sy;
    }
    [
        (ox * 127.0).round().clamp(-127.0, 127.0) as i8,
        (oy * 127.0).round().clamp(-127.0, 127.0) as i8,
    ]
}

/// Decodes 2-byte signed octahedral coordinates back into a unit normal vector.
pub fn decode_octahedral_normal(oct: [i8; 2]) -> [f32; 3] {
    let ox = (f32::from(oct[0]) / 127.0).clamp(-1.0, 1.0);
    let oy = (f32::from(oct[1]) / 127.0).clamp(-1.0, 1.0);
    let nz = 1.0 - ox.abs() - oy.abs();
    let (nx, ny) = if nz < 0.0 {
        let sx = if ox >= 0.0 { 1.0 } else { -1.0 };
        let sy = if oy >= 0.0 { 1.0 } else { -1.0 };
        ((1.0 - oy.abs()) * sx, (1.0 - ox.abs()) * sy)
    } else {
        (ox, oy)
    };
    let len = (nx * nx + ny * ny + nz * nz).sqrt();
    if len > 1e-8 {
        [nx / len, ny / len, nz / len]
    } else {
        [0.0, 1.0, 0.0]
    }
}

/// Packed 20-byte quantized cluster vertex with continuous geomorphing residual.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuantizedClusterVertex {
    pub pos_q: [u16; 3],
    pub normal_oct: [i8; 2],
    pub tangent_angle: i8,
    pub bone_local_pair: u8,
    pub uv_f16: [u16; 2],
    pub geomorph_delta_q: [i16; 3],
}

impl QuantizedClusterVertex {
    /// Quantizes a local position into 16-bit normalized coordinates within `[aabb_min, aabb_max]`.
    pub fn quantize_position(pos: [f32; 3], aabb_min: [f32; 3], aabb_max: [f32; 3]) -> [u16; 3] {
        let mut out = [0u16; 3];
        for axis in 0..3 {
            let span = aabb_max[axis] - aabb_min[axis];
            let norm = if span > 1e-8 {
                ((pos[axis] - aabb_min[axis]) / span).clamp(0.0, 1.0)
            } else {
                0.0
            };
            out[axis] = (norm * 65535.0).round().clamp(0.0, 65535.0) as u16;
        }
        out
    }

    /// Quantizes a geomorphing displacement delta relative to `geomorph_scale`.
    pub fn quantize_geomorph_delta(delta: [f32; 3], geomorph_scale: f32) -> [i16; 3] {
        if geomorph_scale <= 1e-8 {
            return [0, 0, 0];
        }
        let mut out = [0i16; 3];
        for axis in 0..3 {
            let scaled = (delta[axis] / geomorph_scale) * 32767.0;
            out[axis] = scaled.round().clamp(-32767.0, 32767.0) as i16;
        }
        out
    }

    /// Reconstructs the local vertex position blended along the geomorphing vector by `morph_alpha` in `[0, 1]`.
    pub fn dequantize_position(
        &self,
        aabb_min: [f32; 3],
        aabb_max: [f32; 3],
        geomorph_scale: f32,
        morph_alpha: f32,
    ) -> [f32; 3] {
        let alpha = morph_alpha.clamp(0.0, 1.0);
        let mut out = [0.0f32; 3];
        for axis in 0..3 {
            let span = aabb_max[axis] - aabb_min[axis];
            let base = aabb_min[axis] + (f32::from(self.pos_q[axis]) / 65535.0) * span;
            let delta = (f32::from(self.geomorph_delta_q[axis]) / 32767.0) * geomorph_scale;
            out[axis] = base + alpha * delta;
        }
        out
    }

    /// Reconstructs half-float UV0 coordinates.
    pub fn dequantize_uv(&self) -> [f32; 2] {
        [f16_to_f32(self.uv_f16[0]), f16_to_f32(self.uv_f16[1])]
    }
}

/// Anisotropic 3D Gaussian surfel with SGGX normal distribution covariance for far-field foliage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SggxSplatPrimitive {
    pub center: [f32; 3],
    pub scale: [f32; 3],
    pub rotation: [f32; 4],
    pub sggx_cov: [f32; 6],
    pub opacity: f32,
}

/// Atomic LOD cut decision unit grouping 4-8 boundary-locked sibling clusters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RcldClusterGroup {
    pub group_id: u32,
    pub lod_level: u8,
    pub primitive_kind: VirtualGeoPrimitiveKind,
    pub child_group_count: u16,
    pub page_index: u32,
    pub child_group_first: u32,
    pub first_cluster: u32,
    pub cluster_count: u16,
    pub parent_error: f32,
    pub self_error: f32,
    pub bounding_sphere: [f32; 4],
}

/// Self-contained meshlet cluster with up to 128 vertices and 128 triangles.
#[derive(Debug, Clone, PartialEq)]
pub struct RcldCluster {
    pub cluster_id: u32,
    pub group_id: u32,
    pub lod_level: u8,
    pub primitive_kind: VirtualGeoPrimitiveKind,
    pub material_index: u16,
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
    /// Bounding normal cone: `xyz = normalized axis`, `w = sin(half_angle)`.
    pub normal_cone: [f32; 4],
    pub parent_error: f32,
    pub self_error: f32,
    pub geomorph_scale: f32,
    pub bone_palette: [u16; RCLD_MAX_CLUSTER_BONES],
    pub bone_count: u8,
    pub max_skin_inflation: f32,
    pub vertices: Vec<QuantizedClusterVertex>,
    pub triangles: Vec<[u8; 3]>,
    pub dmm_displacements: Vec<i16>,
    pub splat_surrogate: Option<SggxSplatPrimitive>,
}

impl RcldCluster {
    /// Serialized byte size of this cluster inside a page payload.
    pub fn encoded_byte_size(&self) -> usize {
        let base = 4
            + 4
            + 1
            + 1
            + 2
            + 12
            + 12
            + 16
            + 4
            + 4
            + 4
            + (RCLD_MAX_CLUSTER_BONES * 2)
            + 1
            + 1
            + 2
            + 4
            + 4
            + 4;
        let verts = self.vertices.len() * RCLD_VERTEX_SIZE;
        let tris = self.triangles.len() * 3;
        let dmm = self.dmm_displacements.len() * 2;
        let splat = if self.splat_surrogate.is_some() {
            RCLD_SPLAT_SIZE
        } else {
            0
        };
        base + verts + tris + dmm + splat
    }
}

/// Streaming page directory entry inside an `RCLD` container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RcldPageDescriptor {
    pub page_index: u32,
    pub byte_offset: u32,
    pub byte_size: u32,
    pub first_cluster: u32,
    pub cluster_count: u16,
    pub lod_level: u8,
    pub flags: u8,
}

/// Fixed 64-byte header of an `RCLD` virtual geometry asset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RcldHeader {
    pub version: u8,
    pub flags: u8,
    pub max_lod_level: u16,
    pub page_count: u32,
    pub group_count: u32,
    pub cluster_count: u32,
    pub root_group_index: u32,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    pub page_table_offset: u32,
    pub group_table_offset: u32,
    pub payload_offset: u32,
}

/// Complete decoded `RCLD` virtual geometry asset.
#[derive(Debug, Clone, PartialEq)]
pub struct RcldAsset {
    pub header: RcldHeader,
    pub pages: Vec<RcldPageDescriptor>,
    pub groups: Vec<RcldClusterGroup>,
    pub clusters: Vec<RcldCluster>,
}

impl RcldAsset {
    /// Verifies structural, paging, vertex cap, and monotonic DAG error invariants.
    pub fn validate_invariants(&self) -> Result<(), String> {
        if self.header.version != RCLD_VERSION {
            return Err(format!("unsupported version {}", self.header.version));
        }
        if self.pages.is_empty() || self.groups.is_empty() || self.clusters.is_empty() {
            return Err("RCLD asset must have at least one page, group, and cluster".to_owned());
        }
        if (self.pages[0].flags & RCLD_PAGE_FLAG_RESIDENT_ROOT) == 0 {
            return Err("page 0 must be marked RCLD_PAGE_FLAG_RESIDENT_ROOT".to_owned());
        }
        if self.header.root_group_index as usize >= self.groups.len() {
            return Err("root_group_index out of bounds".to_owned());
        }
        if self.pages.len() != self.header.page_count as usize
            || self.groups.len() != self.header.group_count as usize
            || self.clusters.len() != self.header.cluster_count as usize
        {
            return Err("header counts disagree with vector lengths".to_owned());
        }

        let mut covered_clusters = 0usize;
        for (idx, page) in self.pages.iter().enumerate() {
            if page.page_index as usize != idx {
                return Err(format!("page_index mismatch at slot {idx}"));
            }
            let end = page
                .first_cluster
                .checked_add(u32::from(page.cluster_count))
                .ok_or_else(|| "page cluster range overflow".to_owned())?
                as usize;
            if page.first_cluster as usize != covered_clusters || end > self.clusters.len() {
                return Err(format!(
                    "page {idx} has non-contiguous or out-of-range clusters"
                ));
            }
            covered_clusters = end;
        }
        if covered_clusters != self.clusters.len() {
            return Err("pages do not cover all clusters".to_owned());
        }

        for group in &self.groups {
            if !group.parent_error.is_finite()
                || !group.self_error.is_finite()
                || group.parent_error <= group.self_error
            {
                return Err(format!(
                    "group {} violates monotonic error: parent={} <= self={}",
                    group.group_id, group.parent_error, group.self_error
                ));
            }
            if group.page_index as usize >= self.pages.len() {
                return Err(format!(
                    "group {} references invalid page {}",
                    group.group_id, group.page_index
                ));
            }
            let cluster_end = group
                .first_cluster
                .checked_add(u32::from(group.cluster_count))
                .ok_or_else(|| "group cluster range overflow".to_owned())?
                as usize;
            if group.cluster_count == 0 || cluster_end > self.clusters.len() {
                return Err(format!(
                    "group {} has invalid cluster range",
                    group.group_id
                ));
            }
            let child_end = group
                .child_group_first
                .checked_add(u32::from(group.child_group_count))
                .ok_or_else(|| "child group range overflow".to_owned())?
                as usize;
            if child_end > self.groups.len() {
                return Err(format!(
                    "group {} has out-of-range child groups",
                    group.group_id
                ));
            }
            for child in &self.groups[group.child_group_first as usize..child_end] {
                if child.self_error >= group.self_error {
                    return Err(format!(
                        "child group {} self_error {} >= parent {} self_error {}",
                        child.group_id, child.self_error, group.group_id, group.self_error
                    ));
                }
            }
        }

        for (idx, cluster) in self.clusters.iter().enumerate() {
            if cluster.cluster_id as usize != idx {
                return Err(format!("cluster_id mismatch at {idx}"));
            }
            if cluster.vertices.len() > RCLD_MAX_CLUSTER_VERTICES {
                return Err(format!(
                    "cluster {idx} vertex count {} exceeds {RCLD_MAX_CLUSTER_VERTICES}",
                    cluster.vertices.len()
                ));
            }
            if cluster.triangles.len() > RCLD_MAX_CLUSTER_TRIANGLES {
                return Err(format!(
                    "cluster {idx} triangle count {} exceeds {RCLD_MAX_CLUSTER_TRIANGLES}",
                    cluster.triangles.len()
                ));
            }
            if usize::from(cluster.bone_count) > RCLD_MAX_CLUSTER_BONES {
                return Err(format!(
                    "cluster {idx} bone_count exceeds {RCLD_MAX_CLUSTER_BONES}"
                ));
            }
            if !cluster.parent_error.is_finite()
                || !cluster.self_error.is_finite()
                || cluster.parent_error <= cluster.self_error
            {
                return Err(format!(
                    "cluster {idx} violates monotonic error: parent={} <= self={}",
                    cluster.parent_error, cluster.self_error
                ));
            }
            let vlen = cluster.vertices.len();
            for tri in &cluster.triangles {
                if usize::from(tri[0]) >= vlen
                    || usize::from(tri[1]) >= vlen
                    || usize::from(tri[2]) >= vlen
                {
                    return Err(format!("cluster {idx} triangle index out of bounds"));
                }
            }
        }
        Ok(())
    }
}

fn push_f32_slice(out: &mut Vec<u8>, values: &[f32]) {
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

fn encode_cluster(out: &mut Vec<u8>, cluster: &RcldCluster) -> Option<()> {
    if cluster.vertices.len() > RCLD_MAX_CLUSTER_VERTICES
        || cluster.triangles.len() > RCLD_MAX_CLUSTER_TRIANGLES
        || usize::from(cluster.bone_count) > RCLD_MAX_CLUSTER_BONES
    {
        return None;
    }
    out.extend_from_slice(&cluster.cluster_id.to_le_bytes());
    out.extend_from_slice(&cluster.group_id.to_le_bytes());
    out.push(cluster.lod_level);
    out.push(cluster.primitive_kind as u8);
    out.extend_from_slice(&cluster.material_index.to_le_bytes());
    push_f32_slice(out, &cluster.aabb_min);
    push_f32_slice(out, &cluster.aabb_max);
    push_f32_slice(out, &cluster.normal_cone);
    out.extend_from_slice(&cluster.parent_error.to_le_bytes());
    out.extend_from_slice(&cluster.self_error.to_le_bytes());
    out.extend_from_slice(&cluster.geomorph_scale.to_le_bytes());
    for bone in cluster.bone_palette {
        out.extend_from_slice(&bone.to_le_bytes());
    }
    out.push(cluster.bone_count);
    out.push(cluster.vertices.len() as u8);
    out.extend_from_slice(&(cluster.triangles.len() as u16).to_le_bytes());
    out.extend_from_slice(&cluster.max_skin_inflation.to_le_bytes());
    out.extend_from_slice(&(cluster.dmm_displacements.len() as u32).to_le_bytes());
    let has_splat = u32::from(cluster.splat_surrogate.is_some());
    out.extend_from_slice(&has_splat.to_le_bytes());

    for v in &cluster.vertices {
        for q in v.pos_q {
            out.extend_from_slice(&q.to_le_bytes());
        }
        out.push(v.normal_oct[0] as u8);
        out.push(v.normal_oct[1] as u8);
        out.push(v.tangent_angle as u8);
        out.push(v.bone_local_pair);
        for uv in v.uv_f16 {
            out.extend_from_slice(&uv.to_le_bytes());
        }
        for d in v.geomorph_delta_q {
            out.extend_from_slice(&d.to_le_bytes());
        }
    }

    for tri in &cluster.triangles {
        out.extend_from_slice(tri);
    }

    for disp in &cluster.dmm_displacements {
        out.extend_from_slice(&disp.to_le_bytes());
    }

    if let Some(splat) = &cluster.splat_surrogate {
        push_f32_slice(out, &splat.center);
        push_f32_slice(out, &splat.scale);
        push_f32_slice(out, &splat.rotation);
        push_f32_slice(out, &splat.sggx_cov);
        out.extend_from_slice(&splat.opacity.to_le_bytes());
    }
    Some(())
}

/// Serializes an [`RcldAsset`] into `RCLD` v1 wire bytes and updates page offsets.
pub fn encode_rcld_bytes(asset: &RcldAsset) -> Option<Vec<u8>> {
    asset.validate_invariants().ok()?;

    let page_table_offset = RCLD_HEADER_SIZE as u32;
    let group_table_offset =
        page_table_offset.checked_add((asset.pages.len() * RCLD_PAGE_DESC_SIZE) as u32)?;
    let payload_offset =
        group_table_offset.checked_add((asset.groups.len() * RCLD_GROUP_RECORD_SIZE) as u32)?;

    let mut page_bytes_all = Vec::new();
    let mut updated_pages = asset.pages.clone();
    for page in &mut updated_pages {
        let page_start = payload_offset as usize + page_bytes_all.len();
        let mut page_buf = Vec::new();
        let start = page.first_cluster as usize;
        let end = start + usize::from(page.cluster_count);
        for cluster in &asset.clusters[start..end] {
            encode_cluster(&mut page_buf, cluster)?;
        }
        page.byte_offset = u32::try_from(page_start).ok()?;
        page.byte_size = u32::try_from(page_buf.len()).ok()?;
        page_bytes_all.extend_from_slice(&page_buf);
    }

    let mut out = Vec::with_capacity(payload_offset as usize + page_bytes_all.len());
    out.extend_from_slice(RCLD_MAGIC);
    out.push(RCLD_VERSION);
    out.push(asset.header.flags);
    out.extend_from_slice(&asset.header.max_lod_level.to_le_bytes());
    out.extend_from_slice(&(updated_pages.len() as u32).to_le_bytes());
    out.extend_from_slice(&(asset.groups.len() as u32).to_le_bytes());
    out.extend_from_slice(&(asset.clusters.len() as u32).to_le_bytes());
    out.extend_from_slice(&asset.header.root_group_index.to_le_bytes());
    push_f32_slice(&mut out, &asset.header.bounds_min);
    push_f32_slice(&mut out, &asset.header.bounds_max);
    out.extend_from_slice(&page_table_offset.to_le_bytes());
    out.extend_from_slice(&group_table_offset.to_le_bytes());
    out.extend_from_slice(&payload_offset.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    debug_assert_eq!(out.len(), RCLD_HEADER_SIZE);

    for page in &updated_pages {
        out.extend_from_slice(&page.page_index.to_le_bytes());
        out.extend_from_slice(&page.byte_offset.to_le_bytes());
        out.extend_from_slice(&page.byte_size.to_le_bytes());
        out.extend_from_slice(&page.first_cluster.to_le_bytes());
        out.extend_from_slice(&page.cluster_count.to_le_bytes());
        out.push(page.lod_level);
        out.push(page.flags);
    }

    for group in &asset.groups {
        out.extend_from_slice(&group.group_id.to_le_bytes());
        out.push(group.lod_level);
        out.push(group.primitive_kind as u8);
        out.extend_from_slice(&group.child_group_count.to_le_bytes());
        out.extend_from_slice(&group.page_index.to_le_bytes());
        out.extend_from_slice(&group.child_group_first.to_le_bytes());
        out.extend_from_slice(&group.first_cluster.to_le_bytes());
        out.extend_from_slice(&group.cluster_count.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&group.parent_error.to_le_bytes());
        out.extend_from_slice(&group.self_error.to_le_bytes());
        push_f32_slice(&mut out, &group.bounding_sphere);
    }

    out.extend_from_slice(&page_bytes_all);
    Some(out)
}

fn read_f32_array<const N: usize>(reader: &mut Reader<'_>) -> Option<[f32; N]> {
    let mut out = [0.0f32; N];
    for item in &mut out {
        *item = reader.f32()?;
    }
    Some(out)
}

/// Decodes only the 64-byte [`RcldHeader`] for streaming table discovery.
pub fn decode_rcld_header(data: &[u8]) -> Option<RcldHeader> {
    if data.len() < RCLD_HEADER_SIZE || &data[..4] != RCLD_MAGIC {
        return None;
    }
    let mut r = Reader::new(&data[4..RCLD_HEADER_SIZE], 0);
    let version = r.u8()?;
    if version != RCLD_VERSION {
        return None;
    }
    let flags = r.u8()?;
    let max_lod_level = r.u16()?;
    let page_count = r.u32()?;
    let group_count = r.u32()?;
    let cluster_count = r.u32()?;
    let root_group_index = r.u32()?;
    let bounds_min = read_f32_array::<3>(&mut r)?;
    let bounds_max = read_f32_array::<3>(&mut r)?;
    let page_table_offset = r.u32()?;
    let group_table_offset = r.u32()?;
    let payload_offset = r.u32()?;
    let _reserved = r.u32()?;

    Some(RcldHeader {
        version,
        flags,
        max_lod_level,
        page_count,
        group_count,
        cluster_count,
        root_group_index,
        bounds_min,
        bounds_max,
        page_table_offset,
        group_table_offset,
        payload_offset,
    })
}

fn decode_cluster(r: &mut Reader<'_>) -> Option<RcldCluster> {
    let cluster_id = r.u32()?;
    let group_id = r.u32()?;
    let lod_level = r.u8()?;
    let primitive_kind = VirtualGeoPrimitiveKind::from_u8(r.u8()?)?;
    let material_index = r.u16()?;
    let aabb_min = read_f32_array::<3>(r)?;
    let aabb_max = read_f32_array::<3>(r)?;
    let normal_cone = read_f32_array::<4>(r)?;
    let parent_error = r.f32()?;
    let self_error = r.f32()?;
    let geomorph_scale = r.f32()?;

    let mut bone_palette = [0u16; RCLD_MAX_CLUSTER_BONES];
    for bone in &mut bone_palette {
        *bone = r.u16()?;
    }
    let bone_count = r.u8()?;
    let vertex_count = usize::from(r.u8()?);
    let triangle_count = usize::from(r.u16()?);
    if vertex_count > RCLD_MAX_CLUSTER_VERTICES
        || triangle_count > RCLD_MAX_CLUSTER_TRIANGLES
        || usize::from(bone_count) > RCLD_MAX_CLUSTER_BONES
    {
        return None;
    }

    let max_skin_inflation = r.f32()?;
    let dmm_count = r.u32()? as usize;
    let has_splat = r.u32()? != 0;

    let mut vertices = Vec::with_capacity(vertex_count);
    for _ in 0..vertex_count {
        let pos_q = [r.u16()?, r.u16()?, r.u16()?];
        let normal_oct = [r.u8()? as i8, r.u8()? as i8];
        let tangent_angle = r.u8()? as i8;
        let bone_local_pair = r.u8()?;
        let uv_f16 = [r.u16()?, r.u16()?];
        let geomorph_delta_q = [r.u16()? as i16, r.u16()? as i16, r.u16()? as i16];
        vertices.push(QuantizedClusterVertex {
            pos_q,
            normal_oct,
            tangent_angle,
            bone_local_pair,
            uv_f16,
            geomorph_delta_q,
        });
    }

    let mut triangles = Vec::with_capacity(triangle_count);
    for _ in 0..triangle_count {
        let tri = [r.u8()?, r.u8()?, r.u8()?];
        if usize::from(tri[0]) >= vertex_count
            || usize::from(tri[1]) >= vertex_count
            || usize::from(tri[2]) >= vertex_count
        {
            return None;
        }
        triangles.push(tri);
    }

    if dmm_count > 65_536 {
        return None;
    }
    let mut dmm_displacements = Vec::with_capacity(dmm_count);
    for _ in 0..dmm_count {
        dmm_displacements.push(r.u16()? as i16);
    }

    let splat_surrogate = if has_splat {
        Some(SggxSplatPrimitive {
            center: read_f32_array::<3>(r)?,
            scale: read_f32_array::<3>(r)?,
            rotation: read_f32_array::<4>(r)?,
            sggx_cov: read_f32_array::<6>(r)?,
            opacity: r.f32()?,
        })
    } else {
        None
    };

    Some(RcldCluster {
        cluster_id,
        group_id,
        lod_level,
        primitive_kind,
        material_index,
        aabb_min,
        aabb_max,
        normal_cone,
        parent_error,
        self_error,
        geomorph_scale,
        bone_palette,
        bone_count,
        max_skin_inflation,
        vertices,
        triangles,
        dmm_displacements,
        splat_surrogate,
    })
}

/// Decodes all clusters stored within a raw page byte slice obtained via `pread`.
pub fn decode_rcld_page_slice(page_bytes: &[u8], cluster_count: u16) -> Option<Vec<RcldCluster>> {
    let mut r = Reader::new(page_bytes, 0);
    let mut clusters = Vec::with_capacity(usize::from(cluster_count));
    for _ in 0..cluster_count {
        clusters.push(decode_cluster(&mut r)?);
    }
    if r.remaining() != 0 {
        return None;
    }
    Some(clusters)
}

/// Decodes a single page's clusters from a full `RCLD` buffer using `page_desc`.
pub fn decode_rcld_page_clusters(
    data: &[u8],
    page_desc: &RcldPageDescriptor,
) -> Option<Vec<RcldCluster>> {
    let start = page_desc.byte_offset as usize;
    let size = page_desc.byte_size as usize;
    let end = start.checked_add(size)?;
    let slice = data.get(start..end)?;
    decode_rcld_page_slice(slice, page_desc.cluster_count)
}

/// Decodes the page directory and group table from an `RCLD` header prefix without loading all pages.
pub fn decode_rcld_directory(
    data: &[u8],
) -> Option<(RcldHeader, Vec<RcldPageDescriptor>, Vec<RcldClusterGroup>)> {
    let header = decode_rcld_header(data)?;

    let page_start = header.page_table_offset as usize;
    let page_bytes = (header.page_count as usize).checked_mul(RCLD_PAGE_DESC_SIZE)?;
    let page_end = page_start.checked_add(page_bytes)?;
    let mut pr = Reader::new(data.get(page_start..page_end)?, 0);
    let mut pages = Vec::with_capacity(header.page_count as usize);
    for _ in 0..header.page_count {
        pages.push(RcldPageDescriptor {
            page_index: pr.u32()?,
            byte_offset: pr.u32()?,
            byte_size: pr.u32()?,
            first_cluster: pr.u32()?,
            cluster_count: pr.u16()?,
            lod_level: pr.u8()?,
            flags: pr.u8()?,
        });
    }

    let group_start = header.group_table_offset as usize;
    let group_bytes = (header.group_count as usize).checked_mul(RCLD_GROUP_RECORD_SIZE)?;
    let group_end = group_start.checked_add(group_bytes)?;
    let mut gr = Reader::new(data.get(group_start..group_end)?, 0);
    let mut groups = Vec::with_capacity(header.group_count as usize);
    for _ in 0..header.group_count {
        let group_id = gr.u32()?;
        let lod_level = gr.u8()?;
        let primitive_kind = VirtualGeoPrimitiveKind::from_u8(gr.u8()?)?;
        let child_group_count = gr.u16()?;
        let page_index = gr.u32()?;
        let child_group_first = gr.u32()?;
        let first_cluster = gr.u32()?;
        let cluster_count = gr.u16()?;
        let _reserved = gr.u16()?;
        let parent_error = gr.f32()?;
        let self_error = gr.f32()?;
        let bounding_sphere = read_f32_array::<4>(&mut gr)?;
        groups.push(RcldClusterGroup {
            group_id,
            lod_level,
            primitive_kind,
            child_group_count,
            page_index,
            child_group_first,
            first_cluster,
            cluster_count,
            parent_error,
            self_error,
            bounding_sphere,
        });
    }

    Some((header, pages, groups))
}

/// Decodes a complete `RCLD` virtual geometry asset from bytes.
pub fn decode_rcld(data: &[u8]) -> Option<RcldAsset> {
    let (header, pages, groups) = decode_rcld_directory(data)?;
    let mut clusters = Vec::with_capacity(header.cluster_count as usize);
    for page in &pages {
        let page_clusters = decode_rcld_page_clusters(data, page)?;
        clusters.extend(page_clusters);
    }
    let asset = RcldAsset {
        header,
        pages,
        groups,
        clusters,
    };
    asset.validate_invariants().ok()?;
    Some(asset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_asset() -> RcldAsset {
        let aabb_min = [0.0, 0.0, 0.0];
        let aabb_max = [2.0, 2.0, 2.0];
        let geomorph_scale = 0.25;
        let v0 = QuantizedClusterVertex {
            pos_q: QuantizedClusterVertex::quantize_position([0.0, 0.0, 0.0], aabb_min, aabb_max),
            normal_oct: encode_octahedral_normal([0.0, 1.0, 0.0]),
            tangent_angle: 12,
            bone_local_pair: 0x01,
            uv_f16: [f32_to_f16(0.0), f32_to_f16(0.0)],
            geomorph_delta_q: QuantizedClusterVertex::quantize_geomorph_delta(
                [0.1, 0.0, -0.05],
                geomorph_scale,
            ),
        };
        let v1 = QuantizedClusterVertex {
            pos_q: QuantizedClusterVertex::quantize_position([2.0, 0.0, 0.0], aabb_min, aabb_max),
            normal_oct: encode_octahedral_normal([0.0, 1.0, 0.0]),
            tangent_angle: 12,
            bone_local_pair: 0x01,
            uv_f16: [f32_to_f16(1.0), f32_to_f16(0.0)],
            geomorph_delta_q: [0, 0, 0],
        };
        let v2 = QuantizedClusterVertex {
            pos_q: QuantizedClusterVertex::quantize_position([1.0, 2.0, 0.0], aabb_min, aabb_max),
            normal_oct: encode_octahedral_normal([0.0, 0.0, 1.0]),
            tangent_angle: -5,
            bone_local_pair: 0x00,
            uv_f16: [f32_to_f16(0.5), f32_to_f16(1.0)],
            geomorph_delta_q: QuantizedClusterVertex::quantize_geomorph_delta(
                [0.0, -0.2, 0.0],
                geomorph_scale,
            ),
        };

        let leaf_cluster = RcldCluster {
            cluster_id: 0,
            group_id: 0,
            lod_level: 0,
            primitive_kind: VirtualGeoPrimitiveKind::Triangles,
            material_index: 0,
            aabb_min,
            aabb_max,
            normal_cone: [0.0, 0.0, 1.0, 0.5],
            parent_error: 0.25,
            self_error: 0.0,
            geomorph_scale,
            bone_palette: [0, 1, 0, 0, 0, 0, 0, 0],
            bone_count: 2,
            max_skin_inflation: 0.1,
            vertices: vec![v0, v1, v2],
            triangles: vec![[0, 1, 2]],
            dmm_displacements: vec![120, -45, 0],
            splat_surrogate: None,
        };
        let root_cluster = RcldCluster {
            cluster_id: 1,
            group_id: 1,
            lod_level: 1,
            primitive_kind: VirtualGeoPrimitiveKind::SggxGaussianSplats,
            material_index: 0,
            aabb_min,
            aabb_max,
            normal_cone: [0.0, 0.0, 1.0, 0.8],
            parent_error: 100.0,
            self_error: 0.25,
            geomorph_scale: 0.0,
            bone_palette: [0; 8],
            bone_count: 0,
            max_skin_inflation: 0.0,
            vertices: vec![v0, v1, v2],
            triangles: vec![[0, 1, 2]],
            dmm_displacements: Vec::new(),
            splat_surrogate: Some(SggxSplatPrimitive {
                center: [1.0, 1.0, 0.0],
                scale: [1.0, 1.0, 0.2],
                rotation: [0.0, 0.0, 0.0, 1.0],
                sggx_cov: [0.1, 0.8, 0.1, 0.0, 0.0, 0.0],
                opacity: 0.85,
            }),
        };

        RcldAsset {
            header: RcldHeader {
                version: RCLD_VERSION,
                flags: RCLD_FLAG_SKINNED
                    | RCLD_FLAG_HAS_DMM
                    | RCLD_FLAG_HAS_SPLATS
                    | RCLD_FLAG_GEOMORPHING,
                max_lod_level: 1,
                page_count: 2,
                group_count: 2,
                cluster_count: 2,
                root_group_index: 1,
                bounds_min: aabb_min,
                bounds_max: aabb_max,
                page_table_offset: 64,
                group_table_offset: 64 + 40,
                payload_offset: 64 + 40 + 96,
            },
            pages: vec![
                RcldPageDescriptor {
                    page_index: 0,
                    byte_offset: 0,
                    byte_size: 0,
                    first_cluster: 0,
                    cluster_count: 1,
                    lod_level: 1,
                    flags: RCLD_PAGE_FLAG_RESIDENT_ROOT,
                },
                RcldPageDescriptor {
                    page_index: 1,
                    byte_offset: 0,
                    byte_size: 0,
                    first_cluster: 1,
                    cluster_count: 1,
                    lod_level: 0,
                    flags: 0,
                },
            ],
            groups: vec![
                RcldClusterGroup {
                    group_id: 0,
                    lod_level: 0,
                    primitive_kind: VirtualGeoPrimitiveKind::Triangles,
                    child_group_count: 0,
                    page_index: 1,
                    child_group_first: 0,
                    first_cluster: 1,
                    cluster_count: 1,
                    parent_error: 0.25,
                    self_error: 0.0,
                    bounding_sphere: [1.0, 1.0, 0.0, 1.8],
                },
                RcldClusterGroup {
                    group_id: 1,
                    lod_level: 1,
                    primitive_kind: VirtualGeoPrimitiveKind::SggxGaussianSplats,
                    child_group_count: 1,
                    page_index: 0,
                    child_group_first: 0,
                    first_cluster: 0,
                    cluster_count: 1,
                    parent_error: 100.0,
                    self_error: 0.25,
                    bounding_sphere: [1.0, 1.0, 0.0, 1.8],
                },
            ],
            clusters: vec![
                RcldCluster {
                    cluster_id: 0,
                    ..root_cluster
                },
                RcldCluster {
                    cluster_id: 1,
                    ..leaf_cluster
                },
            ],
        }
    }

    #[test]
    fn rcld_encode_decode_roundtrip_and_page_streaming() {
        let asset = sample_asset();
        let bytes = encode_rcld_bytes(&asset).expect("sample RCLD must encode");
        let header = decode_rcld_header(&bytes).expect("header must decode");
        assert_eq!(header.page_count, 2);
        assert_eq!(header.group_count, 2);
        assert_eq!(header.cluster_count, 2);

        let decoded = decode_rcld(&bytes).expect("full RCLD must decode");
        assert_eq!(decoded.clusters.len(), 2);
        assert_eq!(decoded.groups[0].self_error, 0.0);
        assert_eq!(decoded.groups[1].child_group_count, 1);

        let page1_desc = decoded.pages[1];
        let streamed_page = decode_rcld_page_clusters(&bytes, &page1_desc)
            .expect("single page slice decode must succeed");
        assert_eq!(streamed_page.len(), 1);
        assert_eq!(streamed_page[0].dmm_displacements, vec![120, -45, 0]);
        assert_eq!(
            decoded.clusters[0].splat_surrogate,
            asset.clusters[0].splat_surrogate
        );
    }

    #[test]
    fn quantized_vertex_and_octahedral_normal_roundtrip() {
        let samples = [
            [0.0, 1.0, 0.0],
            [0.0, 0.0, -1.0],
            [1.0, 0.0, 0.0],
            [-0.577_350_26, -0.577_350_26, 0.577_350_26],
        ];
        for n in samples {
            let oct = encode_octahedral_normal(n);
            let dec = decode_octahedral_normal(oct);
            let dot = n[0] * dec[0] + n[1] * dec[1] + n[2] * dec[2];
            assert!(dot > 0.995, "normal roundtrip dot too low: {dot}");
        }

        let min = [-1.0, 2.0, -3.0];
        let max = [3.0, 6.0, 1.0];
        let scale = 0.5;
        let v = QuantizedClusterVertex {
            pos_q: QuantizedClusterVertex::quantize_position([1.0, 4.0, -1.0], min, max),
            normal_oct: [0, 0],
            tangent_angle: 0,
            bone_local_pair: 0,
            uv_f16: [f32_to_f16(0.25), f32_to_f16(0.75)],
            geomorph_delta_q: QuantizedClusterVertex::quantize_geomorph_delta(
                [0.25, -0.5, 0.125],
                scale,
            ),
        };
        let p0 = v.dequantize_position(min, max, scale, 0.0);
        let p1 = v.dequantize_position(min, max, scale, 1.0);
        assert!((p0[0] - 1.0).abs() < 1e-3);
        assert!((p1[0] - 1.25).abs() < 1e-3);
        assert!((p1[1] - 3.5).abs() < 1e-3);
        let uv = v.dequantize_uv();
        assert!((uv[0] - 0.25).abs() < 1e-3 && (uv[1] - 0.75).abs() < 1e-3);
    }

    #[test]
    fn rcld_rejects_non_monotonic_errors() {
        let mut asset = sample_asset();
        asset.groups[0].parent_error = 0.0;
        assert!(asset.validate_invariants().is_err());
        assert!(encode_rcld_bytes(&asset).is_none());
    }
}
