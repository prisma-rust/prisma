//! Runtime virtual geometry core: 64 KiB page pool streaming, upscaler-aware
//! Cluster-DAG cut evaluation, Two-Phase HZB occlusion culling, and VisBuffer
//! analytic barycentric derivative reconstruction.
//!
//! Coordinates and matrices follow `docs/DESIGN.md`: right-handed world space,
//! camera looking down `-Z`, column-major `mat[col][row]`, `clip = proj * view * model`,
//! and Vulkan depth in `[0, 1]` (Reverse-Z `1.0 -> 0.0` for VisBuffer 64-bit atomics).

use std::collections::{HashMap, HashSet};

use crate::capabilities::VirtualGeometryTier;
use crate::culling::{Aabb, Frustum};
use crate::render_graph::{DlssMode, MobileUpscaler};

/// Target size of a single streaming virtual geometry page in bytes (64 KiB).
pub const VG_PAGE_SIZE: usize = 65_536;
/// Hard per-cluster vertex cap.
pub const VG_MAX_CLUSTER_VERTICES: usize = 128;
/// Hard per-cluster triangle cap (fits inside 7 bits of VisBuffer ID).
pub const VG_MAX_CLUSTER_TRIANGLES: usize = 128;
/// Screen-space triangle area threshold (`px^2`) routing clusters between Mesh Shader and 64-bit SW raster.
pub const VG_MICRO_TRI_AREA_THRESHOLD_PX2: f32 = 4.0;

const RCLD_MAGIC: &[u8; 4] = b"RCLD";
const RCLD_VERSION: u8 = 1;
const RCLD_HEADER_SIZE: usize = 64;
const RCLD_PAGE_DESC_SIZE: usize = 20;
const RCLD_GROUP_RECORD_SIZE: usize = 48;
const RCLD_PAGE_FLAG_RESIDENT_ROOT: u8 = 1 << 0;

/// Geometric primitive representation carried by a virtual geometry cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum VgPrimitiveKind {
    Triangles = 0,
    DisplacedMicroMesh = 1,
    SggxGaussianSplats = 2,
}

impl VgPrimitiveKind {
    fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Triangles),
            1 => Some(Self::DisplacedMicroMesh),
            2 => Some(Self::SggxGaussianSplats),
            _ => None,
        }
    }
}

/// Runtime descriptor for a 64 KiB virtual geometry streaming page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VgPageDesc {
    pub page_index: u32,
    pub byte_offset: u32,
    pub byte_size: u32,
    pub first_cluster: u32,
    pub cluster_count: u16,
    pub lod_level: u8,
    pub is_resident_root: bool,
}

/// Runtime descriptor for an atomic Cluster-DAG LOD decision group.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VgClusterGroupDesc {
    pub group_id: u32,
    pub lod_level: u8,
    pub primitive_kind: VgPrimitiveKind,
    pub page_index: u32,
    pub child_group_first: u32,
    pub child_group_count: u16,
    pub first_cluster: u32,
    pub cluster_count: u16,
    pub parent_error: f32,
    pub self_error: f32,
    pub bounding_sphere: [f32; 4],
}

/// Runtime descriptor for a single renderable cluster.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VgClusterDesc {
    pub cluster_id: u32,
    pub group_id: u32,
    pub lod_level: u8,
    pub primitive_kind: VgPrimitiveKind,
    pub material_index: u16,
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
    /// Bounding normal cone: `xyz = normalized local axis`, `w = sin(half_angle)`.
    pub normal_cone: [f32; 4],
    pub parent_error: f32,
    pub self_error: f32,
    pub geomorph_scale: f32,
    pub vertex_count: u8,
    pub triangle_count: u16,
}

/// Runtime metadata for a cooked virtual geometry mesh.
#[derive(Debug, Clone, PartialEq)]
pub struct VgMeshDesc {
    pub asset_id: u64,
    pub root_group_index: u32,
    pub max_lod_level: u16,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    pub pages: Vec<VgPageDesc>,
    pub groups: Vec<VgClusterGroupDesc>,
    pub clusters: Vec<VgClusterDesc>,
}

struct SliceReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> SliceReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(len)?;
        let slice = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn f32_array<const N: usize>(&mut self) -> Option<[f32; N]> {
        let mut out = [0.0f32; N];
        for elem in &mut out {
            *elem = self.f32()?;
        }
        Some(out)
    }
}

/// Decodes runtime virtual geometry descriptors directly from an `RCLD` v1 byte stream.
pub fn decode_rcld_runtime(asset_id: u64, data: &[u8]) -> Option<VgMeshDesc> {
    if data.len() < RCLD_HEADER_SIZE || &data[..4] != RCLD_MAGIC {
        return None;
    }
    let mut hr = SliceReader::new(&data[4..RCLD_HEADER_SIZE]);
    if hr.u8()? != RCLD_VERSION {
        return None;
    }
    let _flags = hr.u8()?;
    let max_lod_level = hr.u16()?;
    let page_count = hr.u32()? as usize;
    let group_count = hr.u32()? as usize;
    let cluster_count = hr.u32()? as usize;
    let root_group_index = hr.u32()?;
    let bounds_min = hr.f32_array::<3>()?;
    let bounds_max = hr.f32_array::<3>()?;
    let page_table_offset = hr.u32()? as usize;
    let group_table_offset = hr.u32()? as usize;
    let _payload_offset = hr.u32()?;
    let _reserved = hr.u32()?;

    let page_end = page_table_offset.checked_add(page_count.checked_mul(RCLD_PAGE_DESC_SIZE)?)?;
    let mut pr = SliceReader::new(data.get(page_table_offset..page_end)?);
    let mut pages = Vec::with_capacity(page_count);
    for _ in 0..page_count {
        pages.push(VgPageDesc {
            page_index: pr.u32()?,
            byte_offset: pr.u32()?,
            byte_size: pr.u32()?,
            first_cluster: pr.u32()?,
            cluster_count: pr.u16()?,
            lod_level: pr.u8()?,
            is_resident_root: (pr.u8()? & RCLD_PAGE_FLAG_RESIDENT_ROOT) != 0,
        });
    }

    let group_end =
        group_table_offset.checked_add(group_count.checked_mul(RCLD_GROUP_RECORD_SIZE)?)?;
    let mut gr = SliceReader::new(data.get(group_table_offset..group_end)?);
    let mut groups = Vec::with_capacity(group_count);
    for _ in 0..group_count {
        let group_id = gr.u32()?;
        let lod_level = gr.u8()?;
        let primitive_kind = VgPrimitiveKind::from_u8(gr.u8()?)?;
        let child_group_count = gr.u16()?;
        let page_index = gr.u32()?;
        let child_group_first = gr.u32()?;
        let first_cluster = gr.u32()?;
        let cluster_count = gr.u16()?;
        let _reserved = gr.u16()?;
        let parent_error = gr.f32()?;
        let self_error = gr.f32()?;
        let bounding_sphere = gr.f32_array::<4>()?;
        groups.push(VgClusterGroupDesc {
            group_id,
            lod_level,
            primitive_kind,
            page_index,
            child_group_first,
            child_group_count,
            first_cluster,
            cluster_count,
            parent_error,
            self_error,
            bounding_sphere,
        });
    }

    let mut clusters = Vec::with_capacity(cluster_count);
    for page in &pages {
        let start = page.byte_offset as usize;
        let end = start.checked_add(page.byte_size as usize)?;
        let mut cr = SliceReader::new(data.get(start..end)?);
        for _ in 0..page.cluster_count {
            let cluster_id = cr.u32()?;
            let group_id = cr.u32()?;
            let lod_level = cr.u8()?;
            let primitive_kind = VgPrimitiveKind::from_u8(cr.u8()?)?;
            let material_index = cr.u16()?;
            let aabb_min = cr.f32_array::<3>()?;
            let aabb_max = cr.f32_array::<3>()?;
            let normal_cone = cr.f32_array::<4>()?;
            let parent_error = cr.f32()?;
            let self_error = cr.f32()?;
            let geomorph_scale = cr.f32()?;
            cr.take(16)?;
            let _bone_count = cr.u8()?;
            let vertex_count = cr.u8()?;
            let triangle_count = cr.u16()?;
            let _max_skin_inflation = cr.f32()?;
            let dmm_count = cr.u32()? as usize;
            let has_splat = cr.u32()? != 0;
            cr.take(usize::from(vertex_count) * 20)?;
            cr.take(usize::from(triangle_count) * 3)?;
            cr.take(dmm_count.checked_mul(2)?)?;
            if has_splat {
                cr.take(68)?;
            }
            clusters.push(VgClusterDesc {
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
                vertex_count,
                triangle_count,
            });
        }
    }

    Some(VgMeshDesc {
        asset_id,
        root_group_index,
        max_lod_level,
        bounds_min,
        bounds_max,
        pages,
        groups,
        clusters,
    })
}

/// Unique key identifying a 64 KiB page inside a virtual geometry asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PageKey {
    pub asset_id: u64,
    pub page_index: u32,
}

/// Origin of a page streaming fault request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PageRequestKind {
    RasterCut,
    RayTracingHit { cone_width: f32 },
}

/// Streaming load request queued by GPU/CPU DAG traversal on a page miss.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageFaultRequest {
    pub key: PageKey,
    pub lod_level: u8,
    pub priority: f32,
    pub kind: PageRequestKind,
}

/// Residency and eviction counters for [`VirtualGeoPagePool`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VirtualGeoPoolStats {
    pub capacity_pages: usize,
    pub resident_pages: usize,
    pub pinned_root_pages: usize,
    pub evictions: u64,
    pub cache_hits: u64,
    pub page_faults: u64,
}

#[derive(Debug, Clone, Copy)]
struct PoolSlot {
    key: Option<PageKey>,
    pinned_root: bool,
    last_used_frame: u64,
}

/// Fixed-budget GPU geometry page pool with root-page pinning and LRU eviction.
pub struct VirtualGeoPagePool {
    slots: Vec<PoolSlot>,
    lookup: HashMap<PageKey, usize>,
    pending_faults: HashMap<PageKey, PageFaultRequest>,
    evictions: u64,
    cache_hits: u64,
    page_faults: u64,
}

impl VirtualGeoPagePool {
    /// Creates a page pool holding up to `max_pages` 64 KiB pages.
    pub fn new(max_pages: usize) -> Self {
        let cap = max_pages.max(1);
        Self {
            slots: vec![
                PoolSlot {
                    key: None,
                    pinned_root: false,
                    last_used_frame: 0,
                };
                cap
            ],
            lookup: HashMap::with_capacity(cap),
            pending_faults: HashMap::new(),
            evictions: 0,
            cache_hits: 0,
            page_faults: 0,
        }
    }

    /// Creates a tier-appropriate page pool budget (256 MiB desktop, 64 MiB flagship mobile).
    pub fn for_tier(tier: VirtualGeometryTier) -> Self {
        let pages = match tier {
            VirtualGeometryTier::NextGenClasMeshSw => 4096,
            VirtualGeometryTier::FlagshipMeshSw | VirtualGeometryTier::TileLocalSubpass => 1024,
            VirtualGeometryTier::IndirectIndexedFallback => 512,
        };
        Self::new(pages)
    }

    /// Returns whether `key` is currently resident in the GPU page pool.
    pub fn is_resident(&self, key: PageKey) -> bool {
        self.lookup.contains_key(&key)
    }

    /// Permanently pins a root page so fallback geometry can never be evicted.
    pub fn pin_root_page(&mut self, key: PageKey) -> Option<usize> {
        if let Some(&slot_idx) = self.lookup.get(&key) {
            self.slots[slot_idx].pinned_root = true;
            return Some(slot_idx);
        }
        self.install_page(key, true, u64::MAX)
    }

    /// Marks a resident page as used on `frame_index`, returning its physical slot.
    pub fn touch(&mut self, key: PageKey, frame_index: u64) -> Option<usize> {
        let slot_idx = *self.lookup.get(&key)?;
        self.slots[slot_idx].last_used_frame = frame_index;
        self.cache_hits += 1;
        Some(slot_idx)
    }

    /// Installs a streamed page, evicting the oldest non-root page when full.
    pub fn install_page(&mut self, key: PageKey, is_root: bool, frame_index: u64) -> Option<usize> {
        if let Some(&existing) = self.lookup.get(&key) {
            self.slots[existing].pinned_root |= is_root;
            self.slots[existing].last_used_frame = frame_index;
            return Some(existing);
        }

        let target_slot = if let Some(empty_idx) = self.slots.iter().position(|s| s.key.is_none()) {
            empty_idx
        } else {
            let mut lru_idx = None;
            let mut oldest_frame = u64::MAX;
            for (idx, slot) in self.slots.iter().enumerate() {
                if !slot.pinned_root && slot.last_used_frame <= oldest_frame {
                    oldest_frame = slot.last_used_frame;
                    lru_idx = Some(idx);
                }
            }
            let evict_idx = lru_idx?;
            if let Some(old_key) = self.slots[evict_idx].key.take() {
                self.lookup.remove(&old_key);
                self.evictions += 1;
            }
            evict_idx
        };

        self.slots[target_slot] = PoolSlot {
            key: Some(key),
            pinned_root: is_root,
            last_used_frame: frame_index,
        };
        self.lookup.insert(key, target_slot);
        self.pending_faults.remove(&key);
        Some(target_slot)
    }

    /// Records a page fault from raster cut traversal or ray-guided reflection/GI hits.
    pub fn record_fault(&mut self, fault: PageFaultRequest) {
        if self.is_resident(fault.key) {
            return;
        }
        self.page_faults += 1;
        let effective_priority = match fault.kind {
            PageRequestKind::RasterCut => fault.priority + 1_000.0,
            PageRequestKind::RayTracingHit { cone_width } => fault.priority / cone_width.max(0.01),
        };
        let entry = PageFaultRequest {
            priority: effective_priority,
            ..fault
        };
        self.pending_faults
            .entry(fault.key)
            .and_modify(|existing| {
                if entry.priority > existing.priority {
                    *existing = entry;
                }
            })
            .or_insert(entry);
    }

    /// Drains deduplicated page requests ordered from highest priority to lowest.
    pub fn drain_fault_requests(&mut self) -> Vec<PageFaultRequest> {
        let mut out: Vec<PageFaultRequest> = self.pending_faults.drain().map(|(_, v)| v).collect();
        out.sort_by(|a, b| b.priority.total_cmp(&a.priority));
        out
    }

    /// Returns current residency and eviction telemetry.
    pub fn stats(&self) -> VirtualGeoPoolStats {
        let resident_pages = self.lookup.len();
        let pinned_root_pages = self
            .slots
            .iter()
            .filter(|s| s.key.is_some() && s.pinned_root)
            .count();
        VirtualGeoPoolStats {
            capacity_pages: self.slots.len(),
            resident_pages,
            pinned_root_pages,
            evictions: self.evictions,
            cache_hits: self.cache_hits,
            page_faults: self.page_faults,
        }
    }
}

/// Hierarchical Reverse-Z depth pyramid (`1.0 = near`, `0.0 = far background`).
///
/// Each mip texel stores the minimum Reverse-Z value over its `2x2` children so
/// an AABB is conservatively occluded only if its nearest corner is strictly
/// behind the farthest occluder across every covered mip texel.
#[derive(Debug, Clone)]
pub struct HzbPyramid {
    mips: Vec<(u32, u32, Vec<f32>)>,
}

impl HzbPyramid {
    /// Builds a Reverse-Z min-depth pyramid from full-resolution Reverse-Z samples.
    pub fn from_reverse_z_samples(width: u32, height: u32, samples: &[f32]) -> Option<Self> {
        if width == 0 || height == 0 || samples.len() < (width as usize) * (height as usize) {
            return None;
        }
        let mut mips = Vec::new();
        let base: Vec<f32> = samples[..(width as usize) * (height as usize)]
            .iter()
            .map(|&d| {
                if d.is_finite() {
                    d.clamp(0.0, 1.0)
                } else {
                    0.0
                }
            })
            .collect();
        mips.push((width, height, base));

        loop {
            let (pw, ph, prev) = mips.last().unwrap();
            if *pw == 1 && *ph == 1 {
                break;
            }
            let nw = (*pw / 2).max(1);
            let nh = (*ph / 2).max(1);
            let mut next = vec![0.0f32; (nw as usize) * (nh as usize)];
            for y in 0..nh {
                for x in 0..nw {
                    let sx = (x * 2).min(pw - 1);
                    let sy = (y * 2).min(ph - 1);
                    let sx1 = (sx + 1).min(pw - 1);
                    let sy1 = (sy + 1).min(ph - 1);
                    let d00 = prev[(sy * pw + sx) as usize];
                    let d10 = prev[(sy * pw + sx1) as usize];
                    let d01 = prev[(sy1 * pw + sx) as usize];
                    let d11 = prev[(sy1 * pw + sx1) as usize];
                    next[(y * nw + x) as usize] = d00.min(d10).min(d01).min(d11);
                }
            }
            mips.push((nw, nh, next));
        }
        Some(Self { mips })
    }

    /// Builds a Reverse-Z HZB from standard Vulkan `[0, 1]` depth (`0 = near`, `1 = far`).
    pub fn from_standard_depth_samples(width: u32, height: u32, samples: &[f32]) -> Option<Self> {
        if width == 0 || height == 0 || samples.len() < (width as usize) * (height as usize) {
            return None;
        }
        let rev: Vec<f32> = samples[..(width as usize) * (height as usize)]
            .iter()
            .map(|&d| {
                if d.is_finite() {
                    (1.0 - d).clamp(0.0, 1.0)
                } else {
                    0.0
                }
            })
            .collect();
        Self::from_reverse_z_samples(width, height, &rev)
    }

    /// Tests whether `aabb` is conservatively occluded by this HZB pyramid.
    pub fn is_aabb_occluded(
        &self,
        view_proj: &[[f32; 4]; 4],
        aabb: &Aabb,
        reverse_z_projection: bool,
    ) -> bool {
        if !aabb.is_valid() || self.mips.is_empty() {
            return false;
        }
        let (base_w, base_h, _) = self.mips[0];
        let mut min_uv = [f32::INFINITY; 2];
        let mut max_uv = [f32::NEG_INFINITY; 2];
        let mut box_nearest_rev_z = 0.0f32;

        for [x, y, z] in aabb.corners() {
            let clip = mul_mat4_vec4(view_proj, [x, y, z, 1.0]);
            if !clip[3].is_finite() || clip[3] <= 1e-6 {
                return false;
            }
            let inv_w = 1.0 / clip[3];
            let ndc = [clip[0] * inv_w, clip[1] * inv_w, clip[2] * inv_w];
            if !ndc.iter().all(|v| v.is_finite()) || !(0.0..=1.0).contains(&ndc[2]) {
                return false;
            }
            let uv = [ndc[0] * 0.5 + 0.5, ndc[1] * 0.5 + 0.5];
            if uv.iter().any(|&v| !(0.0..=1.0).contains(&v)) {
                return false;
            }
            min_uv = [min_uv[0].min(uv[0]), min_uv[1].min(uv[1])];
            max_uv = [max_uv[0].max(uv[0]), max_uv[1].max(uv[1])];
            let rev_z = if reverse_z_projection {
                ndc[2]
            } else {
                1.0 - ndc[2]
            };
            box_nearest_rev_z = box_nearest_rev_z.max(rev_z);
        }

        let span_px = ((max_uv[0] - min_uv[0]) * base_w as f32)
            .max((max_uv[1] - min_uv[1]) * base_h as f32)
            .max(1.0);
        let mip_idx = (span_px.log2().ceil() as usize).min(self.mips.len() - 1);
        let (mw, mh, data) = &self.mips[mip_idx];

        let tx0 = ((min_uv[0] * *mw as f32).floor() as u32).min(mw - 1);
        let ty0 = ((min_uv[1] * *mh as f32).floor() as u32).min(mh - 1);
        let tx1 = ((max_uv[0] * *mw as f32).floor() as u32).min(mw - 1);
        let ty1 = ((max_uv[1] * *mh as f32).floor() as u32).min(mh - 1);

        let mut hzb_min_rev_z = 1.0f32;
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                hzb_min_rev_z = hzb_min_rev_z.min(data[(ty * mw + tx) as usize]);
            }
        }
        box_nearest_rev_z < hzb_min_rev_z - 1e-4
    }
}

/// Frame parameters driving upscaler-aware Cluster-DAG traversal and culling.
#[derive(Debug, Clone)]
pub struct VirtualGeoCutConfig {
    pub view_proj: [[f32; 4]; 4],
    pub camera_pos: [f32; 3],
    pub cot_half_fovy: f32,
    pub near_plane: f32,
    pub output_extent: [u32; 2],
    pub internal_extent: [u32; 2],
    pub dlss_mode: DlssMode,
    pub mobile_upscaler: MobileUpscaler,
    pub anf_active: bool,
    pub base_error_threshold_px: f32,
    pub geomorph_window_ratio: f32,
    pub frame_index: u64,
    pub hzb_reverse_z_projection: bool,
}

impl VirtualGeoCutConfig {
    /// Computes the effective screen-space error threshold tightened for active neural upscalers.
    pub fn effective_error_threshold_px(&self) -> f32 {
        let out_h = self.output_extent[1].max(1) as f32;
        let int_h = self.internal_extent[1].max(1) as f32;
        let res_scale = (int_h / out_h).clamp(0.25, 1.0);
        let upscaler_bias = if self.anf_active
            || self.mobile_upscaler.is_temporal_compute()
            || !matches!(self.dlss_mode, DlssMode::Off)
        {
            0.85
        } else {
            1.0
        };
        (self.base_error_threshold_px * res_scale * upscaler_bias).clamp(0.25, 4.0)
    }
}

/// Instance binding an immutable virtual geometry asset to a world transform.
pub struct VirtualGeoInstance<'a> {
    pub mesh: &'a VgMeshDesc,
    pub model: [[f32; 4]; 4],
}

/// Surviving cluster routed to rasterization and active-cut ray tracing queues.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DispatchedCluster {
    pub asset_id: u64,
    pub instance_index: u32,
    pub cluster_id: u32,
    pub group_id: u32,
    pub lod_level: u8,
    pub primitive_kind: VgPrimitiveKind,
    pub morph_alpha: f32,
    pub projected_tri_area_px2: f32,
}

/// Output queues and culling telemetry from [`evaluate_dag_cut`].
#[derive(Debug, Default, Clone)]
pub struct VirtualGeoCutOutput {
    pub mesh_shader_clusters: Vec<DispatchedCluster>,
    pub sw_raster_clusters: Vec<DispatchedCluster>,
    pub sggx_splat_clusters: Vec<DispatchedCluster>,
    pub active_cut_blas_clusters: Vec<DispatchedCluster>,
    pub phase2_candidate_clusters: Vec<DispatchedCluster>,
    pub frustum_culled_groups: usize,
    pub normal_cone_culled_clusters: usize,
    pub hzb_phase1_occluded_clusters: usize,
    pub parent_fallbacks_on_miss: usize,
}

fn mul_mat4_vec4(m: &[[f32; 4]; 4], v: [f32; 4]) -> [f32; 4] {
    [
        m[0][0] * v[0] + m[1][0] * v[1] + m[2][0] * v[2] + m[3][0] * v[3],
        m[0][1] * v[0] + m[1][1] * v[1] + m[2][1] * v[2] + m[3][1] * v[3],
        m[0][2] * v[0] + m[1][2] * v[1] + m[2][2] * v[2] + m[3][2] * v[3],
        m[0][3] * v[0] + m[1][3] * v[1] + m[2][3] * v[2] + m[3][3] * v[3],
    ]
}

fn transform_point3(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
    let v = mul_mat4_vec4(m, [p[0], p[1], p[2], 1.0]);
    [v[0], v[1], v[2]]
}

fn transform_dir3(m: &[[f32; 4]; 4], d: [f32; 3]) -> [f32; 3] {
    let x = m[0][0] * d[0] + m[1][0] * d[1] + m[2][0] * d[2];
    let y = m[0][1] * d[0] + m[1][1] * d[1] + m[2][1] * d[2];
    let z = m[0][2] * d[0] + m[1][2] * d[1] + m[2][2] * d[2];
    let len = (x * x + y * y + z * z).sqrt();
    if len > 1e-8 {
        [x / len, y / len, z / len]
    } else {
        [0.0, 1.0, 0.0]
    }
}

fn max_axis_scale(m: &[[f32; 4]; 4]) -> f32 {
    let s0 = (m[0][0] * m[0][0] + m[0][1] * m[0][1] + m[0][2] * m[0][2]).sqrt();
    let s1 = (m[1][0] * m[1][0] + m[1][1] * m[1][1] + m[1][2] * m[1][2]).sqrt();
    let s2 = (m[2][0] * m[2][0] + m[2][1] * m[2][1] + m[2][2] * m[2][2]).sqrt();
    s0.max(s1).max(s2).max(1e-6)
}

fn project_world_error_px(
    world_error: f32,
    world_sphere: [f32; 4],
    cfg: &VirtualGeoCutConfig,
) -> f32 {
    let dx = world_sphere[0] - cfg.camera_pos[0];
    let dy = world_sphere[1] - cfg.camera_pos[1];
    let dz = world_sphere[2] - cfg.camera_pos[2];
    let dist = (dx * dx + dy * dy + dz * dz).sqrt();
    let denom = (dist - world_sphere[3]).max(cfg.near_plane.max(1e-3));
    let half_h = 0.5 * (cfg.output_extent[1].max(1) as f32);
    (world_error * cfg.cot_half_fovy * half_h) / denom
}

/// Evaluates the active Cluster-DAG cut, page residency fallbacks, culling, and queue routing.
pub fn evaluate_dag_cut(
    instances: &[VirtualGeoInstance<'_>],
    pool: &mut VirtualGeoPagePool,
    prev_hzb: Option<&HzbPyramid>,
    curr_hzb: Option<&HzbPyramid>,
    cfg: &VirtualGeoCutConfig,
    tier: VirtualGeometryTier,
) -> VirtualGeoCutOutput {
    let mut out = VirtualGeoCutOutput::default();
    let frustum = Frustum::from_view_proj(&cfg.view_proj);
    let tau = cfg.effective_error_threshold_px();
    let supports_sw_raster = matches!(
        tier,
        VirtualGeometryTier::NextGenClasMeshSw | VirtualGeometryTier::FlagshipMeshSw
    );

    for (inst_idx, inst) in instances.iter().enumerate() {
        let mesh = inst.mesh;
        if mesh.groups.is_empty() {
            continue;
        }
        let scale = max_axis_scale(&inst.model);
        let root_idx = mesh.root_group_index as usize;
        if root_idx >= mesh.groups.len() {
            continue;
        }
        let root_page = PageKey {
            asset_id: mesh.asset_id,
            page_index: mesh.groups[root_idx].page_index,
        };
        if !pool.is_resident(root_page) {
            pool.pin_root_page(root_page);
        }

        let mut selected_groups = Vec::new();
        let mut stack = vec![root_idx];
        let mut visited_groups = HashSet::new();
        visited_groups.insert(root_idx);

        while let Some(gi) = stack.pop() {
            let group = &mesh.groups[gi];
            let world_center = transform_point3(
                &inst.model,
                [
                    group.bounding_sphere[0],
                    group.bounding_sphere[1],
                    group.bounding_sphere[2],
                ],
            );
            let world_radius = group.bounding_sphere[3] * scale;
            let sphere_aabb = Aabb::new(
                [
                    world_center[0] - world_radius,
                    world_center[1] - world_radius,
                    world_center[2] - world_radius,
                ],
                [
                    world_center[0] + world_radius,
                    world_center[1] + world_radius,
                    world_center[2] + world_radius,
                ],
            );
            if let Some(ref f) = frustum {
                if !f.intersects_aabb(&sphere_aabb) {
                    out.frustum_culled_groups += 1;
                    continue;
                }
            }

            let world_sphere = [
                world_center[0],
                world_center[1],
                world_center[2],
                world_radius,
            ];
            let self_err_px = project_world_error_px(group.self_error * scale, world_sphere, cfg);
            let want_descend = self_err_px > tau && group.child_group_count > 0;

            if want_descend {
                let child_start = group.child_group_first as usize;
                let child_end =
                    (child_start + usize::from(group.child_group_count)).min(mesh.groups.len());
                let mut missing_pages = Vec::new();
                for child in &mesh.groups[child_start..child_end] {
                    let key = PageKey {
                        asset_id: mesh.asset_id,
                        page_index: child.page_index,
                    };
                    if !pool.is_resident(key) {
                        missing_pages.push((key, child.lod_level));
                    }
                }
                if missing_pages.is_empty() {
                    for (offset, child) in mesh.groups[child_start..child_end].iter().enumerate() {
                        let key = PageKey {
                            asset_id: mesh.asset_id,
                            page_index: child.page_index,
                        };
                        pool.touch(key, cfg.frame_index);
                        let ci = child_start + offset;
                        if visited_groups.insert(ci) {
                            stack.push(ci);
                        }
                    }
                    continue;
                }
                for (key, lod_level) in missing_pages {
                    pool.record_fault(PageFaultRequest {
                        key,
                        lod_level,
                        priority: self_err_px,
                        kind: PageRequestKind::RasterCut,
                    });
                }
                out.parent_fallbacks_on_miss += 1;
            }

            let page_key = PageKey {
                asset_id: mesh.asset_id,
                page_index: group.page_index,
            };
            pool.touch(page_key, cfg.frame_index);
            selected_groups.push((gi, world_sphere));
        }

        for (gi, group_sphere) in selected_groups {
            let group = &mesh.groups[gi];
            let c_start = group.first_cluster as usize;
            let c_end = (c_start + usize::from(group.cluster_count)).min(mesh.clusters.len());
            for cluster in &mesh.clusters[c_start..c_end] {
                let local_aabb = Aabb::new(cluster.aabb_min, cluster.aabb_max);
                let world_aabb = local_aabb
                    .transformed(&inst.model)
                    .unwrap_or(Aabb::new(cluster.aabb_min, cluster.aabb_max));

                if let Some(ref f) = frustum {
                    if !f.intersects_aabb(&world_aabb) {
                        continue;
                    }
                }

                let world_center = world_aabb.center();
                let to_cluster = [
                    world_center[0] - cfg.camera_pos[0],
                    world_center[1] - cfg.camera_pos[1],
                    world_center[2] - cfg.camera_pos[2],
                ];
                let dist = (to_cluster[0] * to_cluster[0]
                    + to_cluster[1] * to_cluster[1]
                    + to_cluster[2] * to_cluster[2])
                    .sqrt();
                if cluster.normal_cone[3] < 0.999 && dist > world_aabb.radius() {
                    let view_dir = [
                        to_cluster[0] / dist,
                        to_cluster[1] / dist,
                        to_cluster[2] / dist,
                    ];
                    let cone_axis = transform_dir3(
                        &inst.model,
                        [
                            cluster.normal_cone[0],
                            cluster.normal_cone[1],
                            cluster.normal_cone[2],
                        ],
                    );
                    let dot = cone_axis[0] * view_dir[0]
                        + cone_axis[1] * view_dir[1]
                        + cone_axis[2] * view_dir[2];
                    if dot > cluster.normal_cone[3] {
                        out.normal_cone_culled_clusters += 1;
                        continue;
                    }
                }

                let parent_err_px =
                    project_world_error_px(cluster.parent_error * scale, group_sphere, cfg);
                let window = cfg.geomorph_window_ratio.max(1e-3);
                let morph_alpha =
                    ((tau * (1.0 + window) - parent_err_px) / (tau * window)).clamp(0.0, 1.0);

                let cluster_r = world_aabb.radius().max(1e-4);
                let proj_r_px = (cluster_r * cfg.cot_half_fovy * 0.5 * cfg.output_extent[1] as f32)
                    / (dist - cluster_r).max(cfg.near_plane.max(1e-3));
                let tri_count = f32::from(cluster.triangle_count.max(1));
                let projected_tri_area_px2 =
                    (std::f32::consts::PI * proj_r_px * proj_r_px) / tri_count;

                let dispatched = DispatchedCluster {
                    asset_id: mesh.asset_id,
                    instance_index: inst_idx as u32,
                    cluster_id: cluster.cluster_id,
                    group_id: cluster.group_id,
                    lod_level: cluster.lod_level,
                    primitive_kind: cluster.primitive_kind,
                    morph_alpha,
                    projected_tri_area_px2,
                };

                let occluded_phase1 = prev_hzb.is_some_and(|hzb| {
                    hzb.is_aabb_occluded(&cfg.view_proj, &world_aabb, cfg.hzb_reverse_z_projection)
                });
                if occluded_phase1 {
                    out.phase2_candidate_clusters.push(dispatched);
                    let occluded_phase2 = curr_hzb.is_none_or(|hzb| {
                        hzb.is_aabb_occluded(
                            &cfg.view_proj,
                            &world_aabb,
                            cfg.hzb_reverse_z_projection,
                        )
                    });
                    if occluded_phase2 {
                        out.hzb_phase1_occluded_clusters += 1;
                        continue;
                    }
                }

                out.active_cut_blas_clusters.push(dispatched);
                match cluster.primitive_kind {
                    VgPrimitiveKind::SggxGaussianSplats => {
                        out.sggx_splat_clusters.push(dispatched);
                    }
                    VgPrimitiveKind::Triangles | VgPrimitiveKind::DisplacedMicroMesh => {
                        if supports_sw_raster
                            && projected_tri_area_px2 < VG_MICRO_TRI_AREA_THRESHOLD_PX2
                        {
                            out.sw_raster_clusters.push(dispatched);
                        } else {
                            out.mesh_shader_clusters.push(dispatched);
                        }
                    }
                }
            }
        }
    }

    out
}

/// Packs `(cluster_idx: 25 bits, tri_idx: 7 bits)` into a 32-bit VisBuffer payload.
pub fn pack_vis32(cluster_idx: u32, tri_idx: u8) -> u32 {
    ((cluster_idx & 0x01FF_FFFF) << 7) | u32::from(tri_idx & 0x7F)
}

/// Unpacks `(cluster_idx, tri_idx)` from a 32-bit VisBuffer payload.
pub fn unpack_vis32(vis: u32) -> (u32, u8) {
    (vis >> 7, (vis & 0x7F) as u8)
}

/// Packs Reverse-Z depth (`[0, 1]`, nearest = `1.0`) and `vis32` into a 64-bit atomic VisBuffer word.
pub fn pack_vis64(reverse_z_depth: f32, cluster_idx: u32, tri_idx: u8) -> u64 {
    let depth_bits = u64::from(reverse_z_depth.clamp(0.0, 1.0).to_bits());
    let vis32 = u64::from(pack_vis32(cluster_idx, tri_idx));
    (depth_bits << 32) | vis32
}

/// Unpacks `(reverse_z_depth, cluster_idx, tri_idx)` from a 64-bit VisBuffer word.
pub fn unpack_vis64(vis64: u64) -> (f32, u32, u8) {
    let depth = f32::from_bits((vis64 >> 32) as u32);
    let (cluster_idx, tri_idx) = unpack_vis32(vis64 as u32);
    (depth, cluster_idx, tri_idx)
}

/// Perspective-correct barycentric coordinates and exact screen-space partial derivatives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnalyticBarycentrics {
    pub lambda: [f32; 3],
    pub dlambda_dx: [f32; 3],
    pub dlambda_dy: [f32; 3],
    pub linear_inv_w: f32,
}

impl AnalyticBarycentrics {
    /// Evaluates a 2D attribute (such as UV0) and its analytic `ddx` / `ddy` screen gradients.
    pub fn interpolate_with_grad(
        &self,
        a0: [f32; 2],
        a1: [f32; 2],
        a2: [f32; 2],
    ) -> ([f32; 2], [f32; 2], [f32; 2]) {
        let mut val = [0.0f32; 2];
        let mut ddx = [0.0f32; 2];
        let mut ddy = [0.0f32; 2];
        let attrs = [a0, a1, a2];
        for c in 0..2 {
            val[c] = self.lambda[0] * attrs[0][c]
                + self.lambda[1] * attrs[1][c]
                + self.lambda[2] * attrs[2][c];
            ddx[c] = self.dlambda_dx[0] * attrs[0][c]
                + self.dlambda_dx[1] * attrs[1][c]
                + self.dlambda_dx[2] * attrs[2][c];
            ddy[c] = self.dlambda_dy[0] * attrs[0][c]
                + self.dlambda_dy[1] * attrs[1][c]
                + self.dlambda_dy[2] * attrs[2][c];
        }
        (val, ddx, ddy)
    }
}

/// Computes perspective-correct barycentrics and exact pixel-space gradients without 2x2 quad lanes.
pub fn compute_analytic_barycentrics(
    p0_clip: [f32; 4],
    p1_clip: [f32; 4],
    p2_clip: [f32; 4],
    pixel_ndc: [f32; 2],
    inv_resolution: [f32; 2],
) -> Option<AnalyticBarycentrics> {
    if p0_clip[3] <= 1e-6 || p1_clip[3] <= 1e-6 || p2_clip[3] <= 1e-6 {
        return None;
    }
    let inv_w = [1.0 / p0_clip[3], 1.0 / p1_clip[3], 1.0 / p2_clip[3]];
    let ndc = [
        [p0_clip[0] * inv_w[0], p0_clip[1] * inv_w[0]],
        [p1_clip[0] * inv_w[1], p1_clip[1] * inv_w[1]],
        [p2_clip[0] * inv_w[2], p2_clip[1] * inv_w[2]],
    ];

    let det = (ndc[1][0] - ndc[0][0]) * (ndc[2][1] - ndc[0][1])
        - (ndc[2][0] - ndc[0][0]) * (ndc[1][1] - ndc[0][1]);
    if det.abs() <= 1e-12 || !det.is_finite() {
        return None;
    }
    let inv_det = 1.0 / det;

    let db_dndcx = [
        (ndc[1][1] - ndc[2][1]) * inv_det,
        (ndc[2][1] - ndc[0][1]) * inv_det,
        (ndc[0][1] - ndc[1][1]) * inv_det,
    ];
    let db_dndcy = [
        (ndc[2][0] - ndc[1][0]) * inv_det,
        (ndc[0][0] - ndc[2][0]) * inv_det,
        (ndc[1][0] - ndc[0][0]) * inv_det,
    ];

    let px = pixel_ndc[0];
    let py = pixel_ndc[1];
    let b0 = ((ndc[1][0] - px) * (ndc[2][1] - py) - (ndc[2][0] - px) * (ndc[1][1] - py)) * inv_det;
    let b1 = ((ndc[2][0] - px) * (ndc[0][1] - py) - (ndc[0][0] - px) * (ndc[2][1] - py)) * inv_det;
    let b2 = 1.0 - b0 - b1;
    let b_ndc = [b0, b1, b2];

    let u = [
        b_ndc[0] * inv_w[0],
        b_ndc[1] * inv_w[1],
        b_ndc[2] * inv_w[2],
    ];
    let linear_inv_w = u[0] + u[1] + u[2];
    if linear_inv_w.abs() <= 1e-12 || !linear_inv_w.is_finite() {
        return None;
    }
    let w = 1.0 / linear_inv_w;
    let lambda = [u[0] * w, u[1] * w, u[2] * w];

    let ndc_step_x = 2.0 * inv_resolution[0];
    let ndc_step_y = 2.0 * inv_resolution[1];

    let du_dx = [
        db_dndcx[0] * ndc_step_x * inv_w[0],
        db_dndcx[1] * ndc_step_x * inv_w[1],
        db_dndcx[2] * ndc_step_x * inv_w[2],
    ];
    let du_dy = [
        db_dndcy[0] * ndc_step_y * inv_w[0],
        db_dndcy[1] * ndc_step_y * inv_w[1],
        db_dndcy[2] * ndc_step_y * inv_w[2],
    ];
    let d_inv_w_dx = du_dx[0] + du_dx[1] + du_dx[2];
    let d_inv_w_dy = du_dy[0] + du_dy[1] + du_dy[2];

    let dlambda_dx = [
        (du_dx[0] - lambda[0] * d_inv_w_dx) * w,
        (du_dx[1] - lambda[1] * d_inv_w_dx) * w,
        (du_dx[2] - lambda[2] * d_inv_w_dx) * w,
    ];
    let dlambda_dy = [
        (du_dy[0] - lambda[0] * d_inv_w_dy) * w,
        (du_dy[1] - lambda[1] * d_inv_w_dy) * w,
        (du_dy[2] - lambda[2] * d_inv_w_dy) * w,
    ];

    Some(AnalyticBarycentrics {
        lambda,
        dlambda_dx,
        dlambda_dy,
        linear_inv_w,
    })
}

/// Inputs for computing geomorphing-compensated screen-space motion vectors.
pub struct GeomorphVelocityInput<'a> {
    pub prev_view_proj: &'a [[f32; 4]; 4],
    pub prev_model: &'a [[f32; 4]; 4],
    pub curr_view_proj: &'a [[f32; 4]; 4],
    pub curr_model: &'a [[f32; 4]; 4],
    pub base_pos: [f32; 3],
    pub geomorph_delta: [f32; 3],
    pub prev_morph_alpha: f32,
    pub curr_morph_alpha: f32,
}

/// Computes UV-space motion vector tracking the continuously morphed surface point across frames.
pub fn compute_geomorph_compensated_velocity(
    input: &GeomorphVelocityInput<'_>,
) -> Option<[f32; 2]> {
    let a_prev = input.prev_morph_alpha.clamp(0.0, 1.0);
    let a_curr = input.curr_morph_alpha.clamp(0.0, 1.0);
    let prev_local = [
        input.base_pos[0] + a_prev * input.geomorph_delta[0],
        input.base_pos[1] + a_prev * input.geomorph_delta[1],
        input.base_pos[2] + a_prev * input.geomorph_delta[2],
    ];
    let curr_local = [
        input.base_pos[0] + a_curr * input.geomorph_delta[0],
        input.base_pos[1] + a_curr * input.geomorph_delta[1],
        input.base_pos[2] + a_curr * input.geomorph_delta[2],
    ];

    let prev_world = transform_point3(input.prev_model, prev_local);
    let curr_world = transform_point3(input.curr_model, curr_local);
    let prev_clip = mul_mat4_vec4(
        input.prev_view_proj,
        [prev_world[0], prev_world[1], prev_world[2], 1.0],
    );
    let curr_clip = mul_mat4_vec4(
        input.curr_view_proj,
        [curr_world[0], curr_world[1], curr_world[2], 1.0],
    );
    if prev_clip[3] <= 1e-6 || curr_clip[3] <= 1e-6 {
        return None;
    }
    let prev_uv = [
        (prev_clip[0] / prev_clip[3]) * 0.5 + 0.5,
        (prev_clip[1] / prev_clip[3]) * 0.5 + 0.5,
    ];
    let curr_uv = [
        (curr_clip[0] / curr_clip[3]) * 0.5 + 0.5,
        (curr_clip[1] / curr_clip[3]) * 0.5 + 0.5,
    ];
    Some([curr_uv[0] - prev_uv[0], curr_uv[1] - prev_uv[1]])
}

/// Unique key identifying a rendered cluster instance in active-cut ray tracing pools.
pub type ActiveClusterKey = (u64, u32, u32);

/// Incremental delta produced by [`ActiveCutRtClusterPool::sync_with_cut`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ActiveCutRtSyncDiff {
    pub to_build: Vec<DispatchedCluster>,
    pub to_reuse: Vec<ActiveClusterKey>,
    pub to_evict: Vec<ActiveClusterKey>,
    pub requires_tlas_rebuild: bool,
}

/// Tracks active-cut cluster BLAS / CLAS instances so raster and ray tracing share identical geometry.
#[derive(Debug, Default)]
pub struct ActiveCutRtClusterPool {
    resident_clas: HashSet<ActiveClusterKey>,
}

impl ActiveCutRtClusterPool {
    /// Creates an empty active-cut RT cluster pool.
    pub fn new() -> Self {
        Self::default()
    }

    /// Computes incremental build/reuse/evict sets against the current frame's active DAG cut.
    pub fn sync_with_cut(&mut self, active_cut: &[DispatchedCluster]) -> ActiveCutRtSyncDiff {
        let mut next_set = HashSet::with_capacity(active_cut.len());
        let mut to_build = Vec::new();
        let mut to_reuse = Vec::new();

        for &cluster in active_cut {
            let key = (cluster.asset_id, cluster.instance_index, cluster.cluster_id);
            if !next_set.insert(key) {
                continue;
            }
            if self.resident_clas.contains(&key) {
                to_reuse.push(key);
            } else {
                to_build.push(cluster);
            }
        }

        let mut to_evict = Vec::new();
        for &old_key in &self.resident_clas {
            if !next_set.contains(&old_key) {
                to_evict.push(old_key);
            }
        }

        let requires_tlas_rebuild = !to_build.is_empty() || !to_evict.is_empty();
        self.resident_clas = next_set;

        ActiveCutRtSyncDiff {
            to_build,
            to_reuse,
            to_evict,
            requires_tlas_rebuild,
        }
    }

    /// Returns the number of currently resident active-cut RT clusters.
    pub fn resident_count(&self) -> usize {
        self.resident_clas.len()
    }
}

/// Evaluates whether a secondary ray hit requires finer child geometry and queues ray-guided page faults.
pub fn evaluate_ray_guided_page_fault(
    mesh: &VgMeshDesc,
    hit_group_id: u32,
    ray_hit_distance: f32,
    ray_cone_spread_angle_rad: f32,
    pool: &mut VirtualGeoPagePool,
) -> usize {
    let Some(group) = mesh.groups.get(hit_group_id as usize) else {
        return 0;
    };
    if group.child_group_count == 0 {
        return 0;
    }
    let cone_width = (ray_hit_distance.max(0.0) * ray_cone_spread_angle_rad.max(1e-4)).max(1e-4);
    if group.self_error <= cone_width {
        return 0;
    }

    let child_start = group.child_group_first as usize;
    let child_end = (child_start + usize::from(group.child_group_count)).min(mesh.groups.len());
    let mut queued = 0usize;
    for child in &mesh.groups[child_start..child_end] {
        let key = PageKey {
            asset_id: mesh.asset_id,
            page_index: child.page_index,
        };
        if !pool.is_resident(key) {
            pool.record_fault(PageFaultRequest {
                key,
                lod_level: child.lod_level,
                priority: group.self_error,
                kind: PageRequestKind::RayTracingHit { cone_width },
            });
            queued += 1;
        }
    }
    queued
}

/// Computes a conservative world-space AABB for a skinned cluster from its local 8-bone palette.
pub fn evaluate_cluster_bone_envelope(
    local_aabb: &Aabb,
    bone_palette: &[u16],
    bone_matrices: &[[[f32; 4]; 4]],
    max_skin_inflation: f32,
) -> Option<Aabb> {
    if !local_aabb.is_valid() {
        return None;
    }
    let mut combined: Option<Aabb> = None;
    for &bone_idx in bone_palette {
        let Some(mat) = bone_matrices.get(usize::from(bone_idx)) else {
            continue;
        };
        if let Some(transformed) = local_aabb.transformed(mat) {
            combined = match combined {
                Some(acc) => acc.union(&transformed),
                None => Some(transformed),
            };
        }
    }
    let mut envelope = combined?;
    let pad = max_skin_inflation.max(0.0);
    for axis in 0..3 {
        envelope.min[axis] -= pad;
        envelope.max[axis] += pad;
    }
    Some(envelope)
}

/// Evaluates on-demand 2-bone vertex skinning using the cluster's local palette.
pub fn skin_cluster_vertex(
    local_pos: [f32; 3],
    bone_local_pair: u8,
    primary_weight: f32,
    bone_palette: &[u16],
    bone_matrices: &[[[f32; 4]; 4]],
) -> [f32; 3] {
    let w0 = primary_weight.clamp(0.0, 1.0);
    let w1 = 1.0 - w0;
    let b0_local = usize::from(bone_local_pair & 0x0F);
    let b1_local = usize::from((bone_local_pair >> 4) & 0x0F);

    let transform_by_local = |local_slot: usize| -> [f32; 3] {
        bone_palette
            .get(local_slot)
            .and_then(|&global_idx| bone_matrices.get(usize::from(global_idx)))
            .map_or(local_pos, |mat| transform_point3(mat, local_pos))
    };

    let p0 = transform_by_local(b0_local);
    let p1 = transform_by_local(b1_local);
    [
        p0[0] * w0 + p1[0] * w1,
        p0[1] * w0 + p1[1] * w1,
        p0[2] * w0 + p1[2] * w1,
    ]
}

/// Barycentrically subdivided micro-mesh emitted by [`subdivide_dmm_triangle`].
#[derive(Debug, Clone, PartialEq)]
pub struct MicroSubdividedMesh {
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u16; 3]>,
}

/// Selects the displaced micro-mesh subdivision level (`0..=3`, yielding `4^k` micro-triangles).
pub fn select_dmm_subdivision_level(projected_tri_area_px2: f32) -> u8 {
    if projected_tri_area_px2 < 4.0 {
        0
    } else if projected_tri_area_px2 < 16.0 {
        1
    } else if projected_tri_area_px2 < 64.0 {
        2
    } else {
        3
    }
}

/// Uniformly subdivides a base triangle along a `2^level` barycentric grid and applies scalar displacement.
pub fn subdivide_dmm_triangle(
    p0: [f32; 3],
    p1: [f32; 3],
    p2: [f32; 3],
    n0: [f32; 3],
    n1: [f32; 3],
    n2: [f32; 3],
    displacements_q11: &[i16],
    displacement_scale: f32,
    level: u8,
) -> MicroSubdividedMesh {
    let segs = 1usize << level.min(3);
    let total_verts = (segs + 1) * (segs + 2) / 2;
    let mut positions = Vec::with_capacity(total_verts);
    let mut row_starts = Vec::with_capacity(segs + 1);

    for r in 0..=segs {
        row_starts.push(positions.len() as u16);
        for c in 0..=r {
            let w1 = (r - c) as f32 / segs as f32;
            let w2 = c as f32 / segs as f32;
            let w0 = 1.0 - w1 - w2;

            let base_p = [
                w0 * p0[0] + w1 * p1[0] + w2 * p2[0],
                w0 * p0[1] + w1 * p1[1] + w2 * p2[1],
                w0 * p0[2] + w1 * p1[2] + w2 * p2[2],
            ];
            let interp_n = [
                w0 * n0[0] + w1 * n1[0] + w2 * n2[0],
                w0 * n0[1] + w1 * n1[1] + w2 * n2[1],
                w0 * n0[2] + w1 * n1[2] + w2 * n2[2],
            ];
            let n_len =
                (interp_n[0] * interp_n[0] + interp_n[1] * interp_n[1] + interp_n[2] * interp_n[2])
                    .sqrt();
            let unit_n = if n_len > 1e-8 {
                [
                    interp_n[0] / n_len,
                    interp_n[1] / n_len,
                    interp_n[2] / n_len,
                ]
            } else {
                [0.0, 1.0, 0.0]
            };

            let vid = positions.len();
            let disp_norm = if displacements_q11.len() >= total_verts {
                f32::from(displacements_q11[vid]) / 1023.0
            } else if displacements_q11.len() >= 3 {
                (w0 * f32::from(displacements_q11[0])
                    + w1 * f32::from(displacements_q11[1])
                    + w2 * f32::from(displacements_q11[2]))
                    / 1023.0
            } else {
                0.0
            };
            let h = disp_norm * displacement_scale;
            positions.push([
                base_p[0] + unit_n[0] * h,
                base_p[1] + unit_n[1] * h,
                base_p[2] + unit_n[2] * h,
            ]);
        }
    }

    let mut triangles = Vec::with_capacity(segs * segs);
    for r in 0..segs {
        let r0 = row_starts[r];
        let r1 = row_starts[r + 1];
        for c in 0..=r {
            let c16 = c as u16;
            triangles.push([r0 + c16, r1 + c16, r1 + c16 + 1]);
            if c < r {
                triangles.push([r0 + c16, r1 + c16 + 1, r0 + c16 + 1]);
            }
        }
    }

    MicroSubdividedMesh {
        positions,
        triangles,
    }
}

fn normalize_unit_vec(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-8 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [0.0, 1.0, 0.0]
    }
}

/// Evaluates the SGGX micro-flake projected area `sigma(dir) = sqrt(dir^T * S * dir)`.
pub fn evaluate_sggx_projected_area(sggx_cov: &[f32; 6], dir: [f32; 3]) -> f32 {
    let d = normalize_unit_vec(dir);
    let [sxx, syy, szz, sxy, sxz, syz] = *sggx_cov;
    let quad = d[0] * d[0] * sxx
        + d[1] * d[1] * syy
        + d[2] * d[2] * szz
        + 2.0 * (d[0] * d[1] * sxy + d[0] * d[2] * sxz + d[1] * d[2] * syz);
    quad.max(0.0).sqrt()
}

/// Evaluates the anisotropic SGGX micro-flake normal distribution function `D(half_vec)`.
pub fn evaluate_sggx_ndf(sggx_cov: &[f32; 6], half_vec: [f32; 3]) -> f32 {
    let h = normalize_unit_vec(half_vec);
    let [mut sxx, mut syy, mut szz, sxy, sxz, syz] = *sggx_cov;
    let cofactors = |xx: f32, yy: f32, zz: f32| -> (f32, f32, f32, f32, f32, f32, f32) {
        let cxx = yy * zz - syz * syz;
        let cyy = xx * zz - sxz * sxz;
        let czz = xx * yy - sxy * sxy;
        let cxy = sxz * syz - sxy * zz;
        let cxz = sxy * syz - sxz * yy;
        let cyz = sxy * sxz - xx * syz;
        let det = xx * cxx + sxy * cxy + sxz * cxz;
        (cxx, cyy, czz, cxy, cxz, cyz, det)
    };

    let (mut cxx, mut cyy, mut czz, mut cxy, mut cxz, mut cyz, mut det) = cofactors(sxx, syy, szz);
    if !det.is_finite() || det <= 1e-12 {
        sxx = sxx.max(0.0) + 1e-4;
        syy = syy.max(0.0) + 1e-4;
        szz = szz.max(0.0) + 1e-4;
        (cxx, cyy, czz, cxy, cxz, cyz, det) = cofactors(sxx, syy, szz);
    }
    if det <= 1e-18 {
        return 0.0;
    }
    let quad_adj = (h[0] * h[0] * cxx
        + h[1] * h[1] * cyy
        + h[2] * h[2] * czz
        + 2.0 * (h[0] * h[1] * cxy + h[0] * h[2] * cxz + h[1] * h[2] * cyz))
        .max(1e-12);
    let det_pow = det.powf(1.5);
    det_pow / (std::f32::consts::PI * quad_adj * quad_adj)
}

/// Evaluates Beer-Lambert foliage transmittance along `light_dir` through an SGGX surfel volume.
pub fn evaluate_sggx_foliage_transmittance(
    sggx_cov: &[f32; 6],
    light_dir: [f32; 3],
    optical_density: f32,
) -> f32 {
    let sigma = evaluate_sggx_projected_area(sggx_cov, light_dir);
    (-sigma * optical_density.max(0.0)).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vulkan_perspective(fovy_rad: f32, aspect: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
        let f = 1.0 / (0.5 * fovy_rad).tan();
        let nf = 1.0 / (near - far);
        let mut m = [[0.0f32; 4]; 4];
        m[0][0] = f / aspect;
        m[1][1] = -f;
        m[2][2] = far * nf;
        m[2][3] = -1.0;
        m[3][2] = (far * near) * nf;
        m
    }

    fn identity_mat4() -> [[f32; 4]; 4] {
        [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ]
    }

    fn sample_two_level_mesh() -> VgMeshDesc {
        VgMeshDesc {
            asset_id: 42,
            root_group_index: 0,
            max_lod_level: 1,
            bounds_min: [-2.0, -2.0, -6.0],
            bounds_max: [2.0, 2.0, -4.0],
            pages: vec![
                VgPageDesc {
                    page_index: 0,
                    byte_offset: 64,
                    byte_size: 128,
                    first_cluster: 0,
                    cluster_count: 1,
                    lod_level: 1,
                    is_resident_root: true,
                },
                VgPageDesc {
                    page_index: 1,
                    byte_offset: 192,
                    byte_size: 128,
                    first_cluster: 1,
                    cluster_count: 2,
                    lod_level: 0,
                    is_resident_root: false,
                },
            ],
            groups: vec![
                VgClusterGroupDesc {
                    group_id: 0,
                    lod_level: 1,
                    primitive_kind: VgPrimitiveKind::Triangles,
                    page_index: 0,
                    child_group_first: 1,
                    child_group_count: 1,
                    first_cluster: 0,
                    cluster_count: 1,
                    parent_error: 10_000.0,
                    self_error: 0.2,
                    bounding_sphere: [0.0, 0.0, -5.0, 1.5],
                },
                VgClusterGroupDesc {
                    group_id: 1,
                    lod_level: 0,
                    primitive_kind: VgPrimitiveKind::Triangles,
                    page_index: 1,
                    child_group_first: 0,
                    child_group_count: 0,
                    first_cluster: 1,
                    cluster_count: 2,
                    parent_error: 0.2,
                    self_error: 0.0,
                    bounding_sphere: [0.0, 0.0, -5.0, 1.5],
                },
            ],
            clusters: vec![
                VgClusterDesc {
                    cluster_id: 0,
                    group_id: 0,
                    lod_level: 1,
                    primitive_kind: VgPrimitiveKind::Triangles,
                    material_index: 0,
                    aabb_min: [-1.0, -1.0, -5.5],
                    aabb_max: [1.0, 1.0, -4.5],
                    normal_cone: [0.0, 0.0, 1.0, 0.2],
                    parent_error: 10_000.0,
                    self_error: 0.2,
                    geomorph_scale: 0.0,
                    vertex_count: 32,
                    triangle_count: 24,
                },
                VgClusterDesc {
                    cluster_id: 1,
                    group_id: 1,
                    lod_level: 0,
                    primitive_kind: VgPrimitiveKind::Triangles,
                    material_index: 0,
                    aabb_min: [-1.0, -1.0, -5.5],
                    aabb_max: [1.0, 1.0, -4.5],
                    normal_cone: [0.0, 0.0, 1.0, 0.2],
                    parent_error: 0.2,
                    self_error: 0.0,
                    geomorph_scale: 0.05,
                    vertex_count: 120,
                    triangle_count: 128,
                },
                VgClusterDesc {
                    cluster_id: 2,
                    group_id: 1,
                    lod_level: 0,
                    primitive_kind: VgPrimitiveKind::SggxGaussianSplats,
                    material_index: 1,
                    aabb_min: [-0.8, -0.8, -5.4],
                    aabb_max: [0.8, 0.8, -4.6],
                    normal_cone: [0.0, 0.0, 1.0, 1.0],
                    parent_error: 0.2,
                    self_error: 0.0,
                    geomorph_scale: 0.0,
                    vertex_count: 64,
                    triangle_count: 64,
                },
            ],
        }
    }

    #[test]
    fn page_pool_pins_roots_and_evicts_lru_pages() {
        let mut pool = VirtualGeoPagePool::new(3);
        let root = PageKey {
            asset_id: 1,
            page_index: 0,
        };
        let p1 = PageKey {
            asset_id: 1,
            page_index: 1,
        };
        let p2 = PageKey {
            asset_id: 1,
            page_index: 2,
        };
        let p3 = PageKey {
            asset_id: 1,
            page_index: 3,
        };

        assert!(pool.pin_root_page(root).is_some());
        assert!(pool.install_page(p1, false, 1).is_some());
        assert!(pool.install_page(p2, false, 2).is_some());
        assert!(pool.touch(p1, 5).is_some());

        pool.record_fault(PageFaultRequest {
            key: p3,
            lod_level: 0,
            priority: 4.0,
            kind: PageRequestKind::RayTracingHit { cone_width: 0.25 },
        });
        pool.record_fault(PageFaultRequest {
            key: p3,
            lod_level: 0,
            priority: 1.0,
            kind: PageRequestKind::RasterCut,
        });
        let faults = pool.drain_fault_requests();
        assert_eq!(faults.len(), 1);
        assert!(matches!(faults[0].kind, PageRequestKind::RasterCut));

        assert!(pool.install_page(p3, false, 6).is_some());
        assert!(pool.is_resident(root), "pinned root must never be evicted");
        assert!(pool.is_resident(p1), "recently touched page must stay");
        assert!(!pool.is_resident(p2), "oldest non-root page must evict");
        assert_eq!(pool.stats().evictions, 1);
    }

    #[test]
    fn dag_cut_falls_back_to_resident_parent_then_streams_and_routes() {
        let mesh = sample_two_level_mesh();
        let proj = vulkan_perspective(60.0_f32.to_radians(), 16.0 / 9.0, 0.1, 100.0);
        let cfg = VirtualGeoCutConfig {
            view_proj: proj,
            camera_pos: [0.0, 0.0, 0.0],
            cot_half_fovy: proj[1][1].abs(),
            near_plane: 0.1,
            output_extent: [1920, 1080],
            internal_extent: [960, 540],
            dlss_mode: DlssMode::Performance,
            mobile_upscaler: MobileUpscaler::Off,
            anf_active: false,
            base_error_threshold_px: 1.0,
            geomorph_window_ratio: 0.35,
            frame_index: 1,
            hzb_reverse_z_projection: false,
        };
        assert!(cfg.effective_error_threshold_px() < 0.5);

        let mut pool = VirtualGeoPagePool::for_tier(VirtualGeometryTier::FlagshipMeshSw);
        let instances = [VirtualGeoInstance {
            mesh: &mesh,
            model: identity_mat4(),
        }];

        let out_miss = evaluate_dag_cut(
            &instances,
            &mut pool,
            None,
            None,
            &cfg,
            VirtualGeometryTier::FlagshipMeshSw,
        );
        assert_eq!(out_miss.parent_fallbacks_on_miss, 1);
        assert_eq!(
            out_miss.active_cut_blas_clusters.len(),
            1,
            "missing child page must fall back to root cluster with zero holes"
        );
        assert_eq!(out_miss.active_cut_blas_clusters[0].cluster_id, 0);

        let faults = pool.drain_fault_requests();
        assert_eq!(faults.len(), 1);
        assert_eq!(faults[0].key.page_index, 1);

        pool.install_page(faults[0].key, false, 2);
        let out_hit = evaluate_dag_cut(
            &instances,
            &mut pool,
            None,
            None,
            &cfg,
            VirtualGeometryTier::FlagshipMeshSw,
        );
        assert_eq!(out_hit.parent_fallbacks_on_miss, 0);
        assert_eq!(out_hit.active_cut_blas_clusters.len(), 2);
        assert_eq!(out_hit.sggx_splat_clusters.len(), 1);
        assert_eq!(
            out_hit.mesh_shader_clusters.len() + out_hit.sw_raster_clusters.len(),
            1
        );

        let mut micro_mesh = sample_two_level_mesh();
        micro_mesh.groups.truncate(1);
        micro_mesh.groups[0].child_group_count = 0;
        micro_mesh.groups[0].bounding_sphere = [0.0, 0.0, -50.0, 0.4];
        micro_mesh.clusters.truncate(1);
        micro_mesh.clusters[0].triangle_count = 128;
        micro_mesh.clusters[0].aabb_min = [-0.2, -0.2, -50.2];
        micro_mesh.clusters[0].aabb_max = [0.2, 0.2, -49.8];
        let micro_inst = [VirtualGeoInstance {
            mesh: &micro_mesh,
            model: identity_mat4(),
        }];
        let out_micro = evaluate_dag_cut(
            &micro_inst,
            &mut pool,
            None,
            None,
            &cfg,
            VirtualGeometryTier::NextGenClasMeshSw,
        );
        assert!(
            !out_micro.sw_raster_clusters.is_empty(),
            "micro-triangles < 4px^2 must route to 64-bit SW raster on NextGenClasMeshSw"
        );
    }

    #[test]
    fn two_phase_hzb_occludes_hidden_clusters_and_recovers_disocclusions() {
        let mut mesh = sample_two_level_mesh();
        mesh.groups.truncate(1);
        mesh.groups[0].child_group_count = 0;
        mesh.clusters.truncate(1);

        let proj = vulkan_perspective(60.0_f32.to_radians(), 1.0, 0.1, 100.0);
        let cfg = VirtualGeoCutConfig {
            view_proj: proj,
            camera_pos: [0.0, 0.0, 0.0],
            cot_half_fovy: proj[1][1].abs(),
            near_plane: 0.1,
            output_extent: [64, 64],
            internal_extent: [64, 64],
            dlss_mode: DlssMode::Off,
            mobile_upscaler: MobileUpscaler::Off,
            anf_active: false,
            base_error_threshold_px: 1.0,
            geomorph_window_ratio: 0.35,
            frame_index: 1,
            hzb_reverse_z_projection: false,
        };
        let mut pool = VirtualGeoPagePool::new(8);
        let instances = [VirtualGeoInstance {
            mesh: &mesh,
            model: identity_mat4(),
        }];

        let near_wall = HzbPyramid::from_reverse_z_samples(64, 64, &vec![0.99; 64 * 64]).unwrap();
        let cleared_wall = HzbPyramid::from_reverse_z_samples(64, 64, &vec![0.0; 64 * 64]).unwrap();

        let occluded = evaluate_dag_cut(
            &instances,
            &mut pool,
            Some(&near_wall),
            Some(&near_wall),
            &cfg,
            VirtualGeometryTier::FlagshipMeshSw,
        );
        assert_eq!(occluded.hzb_phase1_occluded_clusters, 1);
        assert!(occluded.active_cut_blas_clusters.is_empty());

        let disoccluded = evaluate_dag_cut(
            &instances,
            &mut pool,
            Some(&near_wall),
            Some(&cleared_wall),
            &cfg,
            VirtualGeometryTier::FlagshipMeshSw,
        );
        assert_eq!(disoccluded.phase2_candidate_clusters.len(), 1);
        assert_eq!(disoccluded.hzb_phase1_occluded_clusters, 0);
        assert_eq!(disoccluded.active_cut_blas_clusters.len(), 1);
    }

    #[test]
    fn visbuffer_packing_and_analytic_barycentric_derivatives() {
        let near = pack_vis64(0.92, 123_456, 77);
        let far = pack_vis64(0.41, 999, 12);
        assert!(
            near > far,
            "64-bit Reverse-Z packing must order nearest > farthest"
        );
        let (depth, cid, tid) = unpack_vis64(near);
        assert!((depth - 0.92).abs() < 1e-6);
        assert_eq!(cid, 123_456);
        assert_eq!(tid, 77);

        let proj = vulkan_perspective(60.0_f32.to_radians(), 1.0, 0.1, 100.0);
        let p0 = mul_mat4_vec4(&proj, [-1.0, -0.5, -2.0, 1.0]);
        let p1 = mul_mat4_vec4(&proj, [1.5, -0.5, -4.0, 1.0]);
        let p2 = mul_mat4_vec4(&proj, [0.0, 1.5, -3.0, 1.0]);
        let uv0 = [0.0, 0.0];
        let uv1 = [1.0, 0.0];
        let uv2 = [0.5, 1.0];

        let inv_res = [1.0 / 1000.0, 1.0 / 1000.0];
        let ndc_center = [0.02, -0.03];
        let bary = compute_analytic_barycentrics(p0, p1, p2, ndc_center, inv_res)
            .expect("valid projected triangle");
        let (uv, ddx, ddy) = bary.interpolate_with_grad(uv0, uv1, uv2);

        let eps_px = 1e-3;
        let ndc_px = [ndc_center[0] + 2.0 * inv_res[0] * eps_px, ndc_center[1]];
        let ndc_py = [ndc_center[0], ndc_center[1] + 2.0 * inv_res[1] * eps_px];
        let bx = compute_analytic_barycentrics(p0, p1, p2, ndc_px, inv_res).unwrap();
        let by = compute_analytic_barycentrics(p0, p1, p2, ndc_py, inv_res).unwrap();
        let (uv_px, _, _) = bx.interpolate_with_grad(uv0, uv1, uv2);
        let (uv_py, _, _) = by.interpolate_with_grad(uv0, uv1, uv2);

        let fd_dx = [(uv_px[0] - uv[0]) / eps_px, (uv_px[1] - uv[1]) / eps_px];
        let fd_dy = [(uv_py[0] - uv[0]) / eps_px, (uv_py[1] - uv[1]) / eps_px];
        assert!((ddx[0] - fd_dx[0]).abs() < 1e-4 && (ddx[1] - fd_dx[1]).abs() < 1e-4);
        assert!((ddy[0] - fd_dy[0]).abs() < 1e-4 && (ddy[1] - fd_dy[1]).abs() < 1e-4);

        let vel = compute_geomorph_compensated_velocity(&GeomorphVelocityInput {
            prev_view_proj: &proj,
            prev_model: &identity_mat4(),
            curr_view_proj: &proj,
            curr_model: &identity_mat4(),
            base_pos: [0.2, 0.1, -3.0],
            geomorph_delta: [0.4, -0.2, 0.0],
            prev_morph_alpha: 0.25,
            curr_morph_alpha: 0.75,
        })
        .unwrap();
        assert!(vel[0].abs() > 1e-4 && vel[1].abs() > 1e-4);

        let zero_vel = compute_geomorph_compensated_velocity(&GeomorphVelocityInput {
            prev_view_proj: &proj,
            prev_model: &identity_mat4(),
            curr_view_proj: &proj,
            curr_model: &identity_mat4(),
            base_pos: [0.2, 0.1, -3.0],
            geomorph_delta: [0.4, -0.2, 0.0],
            prev_morph_alpha: 0.5,
            curr_morph_alpha: 0.5,
        })
        .unwrap();
        assert!(zero_vel[0].abs() < 1e-6 && zero_vel[1].abs() < 1e-6);
    }

    #[test]
    fn normal_cone_culls_backfacing_cluster_and_rcld_runtime_decodes() {
        let mut backfacing = sample_two_level_mesh();
        backfacing.groups.truncate(1);
        backfacing.groups[0].child_group_count = 0;
        backfacing.clusters.truncate(1);
        backfacing.clusters[0].normal_cone = [0.0, 0.0, -1.0, 0.2];

        let proj = vulkan_perspective(60.0_f32.to_radians(), 1.0, 0.1, 100.0);
        let cfg = VirtualGeoCutConfig {
            view_proj: proj,
            camera_pos: [0.0, 0.0, 0.0],
            cot_half_fovy: proj[1][1].abs(),
            near_plane: 0.1,
            output_extent: [128, 128],
            internal_extent: [128, 128],
            dlss_mode: DlssMode::Off,
            mobile_upscaler: MobileUpscaler::Off,
            anf_active: false,
            base_error_threshold_px: 1.0,
            geomorph_window_ratio: 0.35,
            frame_index: 1,
            hzb_reverse_z_projection: false,
        };
        let mut pool = VirtualGeoPagePool::new(4);
        let out = evaluate_dag_cut(
            &[VirtualGeoInstance {
                mesh: &backfacing,
                model: identity_mat4(),
            }],
            &mut pool,
            None,
            None,
            &cfg,
            VirtualGeometryTier::FlagshipMeshSw,
        );
        assert_eq!(out.normal_cone_culled_clusters, 1);
        assert!(out.active_cut_blas_clusters.is_empty());

        let page_table_offset = 64u32;
        let group_table_offset = 64u32 + 20u32;
        let payload_offset = group_table_offset + 48u32;
        let cluster_payload_size = 96u32 + 3 * 20 + 3;
        let mut blob = Vec::new();
        blob.extend_from_slice(RCLD_MAGIC);
        blob.push(RCLD_VERSION);
        blob.push(0);
        blob.extend_from_slice(&0u16.to_le_bytes());
        blob.extend_from_slice(&1u32.to_le_bytes());
        blob.extend_from_slice(&1u32.to_le_bytes());
        blob.extend_from_slice(&1u32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        for _ in 0..6 {
            blob.extend_from_slice(&0.0f32.to_le_bytes());
        }
        blob.extend_from_slice(&page_table_offset.to_le_bytes());
        blob.extend_from_slice(&group_table_offset.to_le_bytes());
        blob.extend_from_slice(&payload_offset.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());

        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.extend_from_slice(&payload_offset.to_le_bytes());
        blob.extend_from_slice(&cluster_payload_size.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.extend_from_slice(&1u16.to_le_bytes());
        blob.push(0);
        blob.push(RCLD_PAGE_FLAG_RESIDENT_ROOT);

        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.push(0);
        blob.push(0);
        blob.extend_from_slice(&0u16.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.extend_from_slice(&1u16.to_le_bytes());
        blob.extend_from_slice(&0u16.to_le_bytes());
        blob.extend_from_slice(&100.0f32.to_le_bytes());
        blob.extend_from_slice(&0.0f32.to_le_bytes());
        for f in [0.0f32, 0.0, -5.0, 1.0] {
            blob.extend_from_slice(&f.to_le_bytes());
        }

        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.push(0);
        blob.push(0);
        blob.extend_from_slice(&0u16.to_le_bytes());
        for f in [-1.0f32, -1.0, -6.0, 1.0, 1.0, -4.0, 0.0, 0.0, 1.0, 0.2] {
            blob.extend_from_slice(&f.to_le_bytes());
        }
        blob.extend_from_slice(&100.0f32.to_le_bytes());
        blob.extend_from_slice(&0.0f32.to_le_bytes());
        blob.extend_from_slice(&0.0f32.to_le_bytes());
        blob.extend_from_slice(&[0u8; 16]);
        blob.push(0);
        blob.push(3);
        blob.extend_from_slice(&1u16.to_le_bytes());
        blob.extend_from_slice(&0.0f32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.extend_from_slice(&[0u8; 3 * 20]);
        blob.extend_from_slice(&[0, 1, 2]);

        let mesh = decode_rcld_runtime(77, &blob).expect("RCLD runtime header decode");
        assert_eq!(mesh.asset_id, 77);
        assert_eq!(mesh.clusters.len(), 1);
        assert_eq!(mesh.clusters[0].vertex_count, 3);
        assert_eq!(mesh.clusters[0].triangle_count, 1);
    }

    #[test]
    fn active_cut_rt_sync_and_ray_guided_faults() {
        let mut rt_pool = ActiveCutRtClusterPool::new();
        let c0 = DispatchedCluster {
            asset_id: 42,
            instance_index: 0,
            cluster_id: 1,
            group_id: 1,
            lod_level: 0,
            primitive_kind: VgPrimitiveKind::Triangles,
            morph_alpha: 0.0,
            projected_tri_area_px2: 12.0,
        };
        let c1 = DispatchedCluster {
            cluster_id: 2,
            ..c0
        };
        let c2 = DispatchedCluster {
            cluster_id: 3,
            ..c0
        };

        let diff1 = rt_pool.sync_with_cut(&[c0, c1]);
        assert_eq!(diff1.to_build.len(), 2);
        assert!(diff1.to_reuse.is_empty() && diff1.to_evict.is_empty());
        assert!(diff1.requires_tlas_rebuild);
        assert_eq!(rt_pool.resident_count(), 2);

        let diff2 = rt_pool.sync_with_cut(&[c1, c2]);
        assert_eq!(diff2.to_reuse, vec![(42, 0, 2)]);
        assert_eq!(diff2.to_build.len(), 1);
        assert_eq!(diff2.to_build[0].cluster_id, 3);
        assert_eq!(diff2.to_evict, vec![(42, 0, 1)]);
        assert!(diff2.requires_tlas_rebuild);

        let diff3 = rt_pool.sync_with_cut(&[c1, c2]);
        assert!(!diff3.requires_tlas_rebuild);
        assert_eq!(diff3.to_reuse.len(), 2);

        let mesh = sample_two_level_mesh();
        let mut page_pool = VirtualGeoPagePool::new(8);
        let faults = evaluate_ray_guided_page_fault(&mesh, 0, 4.0, 0.01, &mut page_pool);
        assert_eq!(faults, 1);
        let queued = page_pool.drain_fault_requests();
        assert_eq!(queued.len(), 1);
        assert!(matches!(
            queued[0].kind,
            PageRequestKind::RayTracingHit { .. }
        ));
    }

    #[test]
    fn cluster_bone_envelope_dmm_subdivision_and_sggx_brdf() {
        let local = Aabb::new([-0.5, -0.5, -0.5], [0.5, 0.5, 0.5]);
        let mut b0 = identity_mat4();
        b0[3] = [2.0, 0.0, 0.0, 1.0];
        let mut b1 = identity_mat4();
        b1[3] = [-2.0, 0.0, 0.0, 1.0];
        let bones = [b0, b1];
        let palette = [0u16, 1u16];

        let env = evaluate_cluster_bone_envelope(&local, &palette, &bones, 0.25)
            .expect("valid bone envelope");
        assert!((env.min[0] - (-2.75)).abs() < 1e-5);
        assert!((env.max[0] - 2.75).abs() < 1e-5);

        let skinned = skin_cluster_vertex([0.0, 0.0, 0.0], 0x10, 0.75, &palette, &bones);
        assert!((skinned[0] - 1.0).abs() < 1e-5);

        assert_eq!(select_dmm_subdivision_level(2.0), 0);
        assert_eq!(select_dmm_subdivision_level(8.0), 1);
        assert_eq!(select_dmm_subdivision_level(32.0), 2);
        assert_eq!(select_dmm_subdivision_level(128.0), 3);

        for level in 0..=3u8 {
            let sub = subdivide_dmm_triangle(
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                [0.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
                &[1023, 0, 1023],
                0.5,
                level,
            );
            assert_eq!(sub.triangles.len(), 1usize << (2 * usize::from(level)));
            assert!((sub.positions[0][1] - 0.5).abs() < 1e-3);
        }

        let iso = [1.0f32, 1.0, 1.0, 0.0, 0.0, 0.0];
        assert!((evaluate_sggx_projected_area(&iso, [0.0, 1.0, 0.0]) - 1.0).abs() < 1e-5);
        assert!(
            (evaluate_sggx_ndf(&iso, [0.0, 1.0, 0.0]) - std::f32::consts::FRAC_1_PI).abs() < 1e-5
        );
        let tr = evaluate_sggx_foliage_transmittance(&iso, [0.0, 1.0, 0.0], 1.0);
        assert!((tr - (-1.0f32).exp()).abs() < 1e-5);
    }
}
