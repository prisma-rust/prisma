//! Compute DDA pass rendering sparse three-tier voxel chunks (`L1 -> L2 -> L3`)
//! with 2-bounce GI, explicit emissive voxel NEE, temporal reprojection, and an
//! edge-aware 4-pass À-Trous bilateral denoiser.
//!
//! Reads `FORWARD_COLOR_H` and `FORWARD_DEPTH_H`, raymarches active [`VoxelChunkUpload`]s
//! against scene geometry, denoises demodulated indirect GI + AO, composites translucent
//! glass and the optional 3D cursor wireframe, and publishes `VOXEL_COLOR_H` and `VOXEL_DEPTH_H`.
//! When no voxel chunks are active, the pass is a zero-cost no-op that leaves
//! `VOXEL_COLOR_H` unpublished so downstream passes consume `FORWARD_COLOR_H` unchanged.

use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use ash::vk;

use crate::buffer::{create_buffer, find_memory_type};
use crate::compute::ComputePipeline;
use crate::context::VulkanContext;
use crate::render_graph::{
    GraphResources, PassInfo, PassKind, RenderContext, RenderGraphBuilder, RenderMode,
    RenderPassNode, RenderSettings, ResourceUsage, FORWARD_COLOR_H, FORWARD_DEPTH_H, VOXEL_COLOR_H,
    VOXEL_DEPTH_H,
};
use crate::shader::load_shader_module;
use crate::shader_bindings::voxel_dda::{
    self as bindings, VoxelChunkGpu, VoxelDenoisePush, VoxelEmitterGpu, VoxelFrameUbo,
};

const VOXEL_DDA_SPV: &[u8] = prism_render_shaders::VOXEL_DDA_COMP_SPV;
const VOXEL_GI_SPV: &[u8] = prism_render_shaders::VOXEL_GI_COMP_SPV;
const VOXEL_GI_DENOISE_SPV: &[u8] = prism_render_shaders::VOXEL_GI_DENOISE_COMP_SPV;
const VOXEL_COMPOSITE_SPV: &[u8] = prism_render_shaders::VOXEL_COMPOSITE_COMP_SPV;

const ENTRY_VOXEL_DDA: &CStr = c"voxelDdaMain";
const ENTRY_VOXEL_GI: &CStr = c"voxelGiMain";
const ENTRY_VOXEL_GI_DENOISE: &CStr = c"voxelGiDenoiseMain";
const ENTRY_VOXEL_COMPOSITE: &CStr = c"voxelCompositeMain";

const L1_UNIFORM_REF: u32 = u32::MAX;
const MAX_ACTIVE_EMITTERS: usize = 16;

const VIS_BURNING: u32 = 1 << 1;
const VIS_CHARGED: u32 = 1 << 3;
const VIS_CORRODED: u32 = 1 << 4;
const VIS_INFECTED: u32 = 1 << 5;
const VIS_WARNING: u32 = 1 << 6;
const VIS_STASIS: u32 = 1 << 7;
const VIS_LIQUID: u32 = 1 << 9;
const VIS_TRANSLUCENT: u32 = 1 << 10;
const VIS_EMISSIVE: u32 = 1 << 11;
const VIS_METALLIC: u32 = 1 << 12;
const VIS_HEAT_GLOW: u32 = 1 << 13;
const VIS_FROZEN: u32 = 1 << 2;

/// CPU-extracted chunk buffers ready to upload into GPU SSBOs.
#[derive(Clone, Debug)]
pub struct VoxelChunkUpload {
    pub entity_id: u64,
    pub version: u64,
    pub origin_l1: [i32; 3],
    pub dims_l1: [u32; 3],
    /// `[brick_ref, uniform_rgba, uniform_visual, 0]` per L1 cell.
    pub l1_cells: Vec<[u32; 4]>,
    /// `[occ_lo, occ_hi]` per allocated L2 brick.
    pub brick_occ: Vec<[u32; 2]>,
    /// `[rgba, visual, l3_lo, l3_hi]` (`64` entries per brick).
    pub brick_cells: Vec<[u32; 4]>,
}

/// Optional 3D wireframe box highlighted by the compute pass.
#[derive(Clone, Copy, Debug)]
pub struct VoxelCursorUpload {
    pub active: bool,
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub color: [f32; 4],
}

/// Aggregated voxel scene payload passed from `FramePacket` into `GraphRenderer`.
#[derive(Clone, Debug, Default)]
pub struct VoxelSceneUpload {
    pub signature: u64,
    pub elapsed_seconds: f32,
    pub chunks: Vec<Arc<VoxelChunkUpload>>,
    pub cursor: Option<VoxelCursorUpload>,
    /// Optional 256x48 SPBR LabPBR atlas (16 tiles x 3 rows of 16x16 RGBA8 packed `u32`).
    pub atlas: Option<Arc<Vec<u32>>>,
}

/// Loads the 16-tile (256x48) SPBR LabPBR atlas from `<assets_root>/textures/blocks/` if present.
///
/// Row 0 (`y = 0..16`): albedo `.png`
/// Row 1 (`y = 16..32`): normal `_n.png`
/// Row 2 (`y = 32..48`): LabPBR specular `_s.png`
pub fn load_spbr_atlas_from_assets(assets_root: &std::path::Path) -> Option<Arc<Vec<u32>>> {
    let blocks_dir = assets_root.join("textures").join("blocks");
    if !blocks_dir.is_dir() {
        return None;
    }
    const TILES: [&str; 16] = [
        "bedrock",
        "dirt",
        "grass_block_top",
        "stone",
        "iron_block",
        "glass_pane_top",
        "oak_planks",
        "water_still",
        "emerald_block",
        "ice",
        "amethyst_block",
        "copper_block",
        "iron_block",
        "redstone_ore",
        "emerald_block",
        "grass_block_side",
    ];
    let mut pixels = vec![0u32; 256 * 48];
    let mut loaded_any = false;

    for (tile_idx, name) in TILES.iter().enumerate() {
        let suffixes = [("", 0usize), ("_n", 16usize), ("_s", 32usize)];
        for (suffix, row_y) in suffixes {
            let path = blocks_dir.join(format!("{name}{suffix}.png"));
            let Ok(img) = image::open(&path) else {
                continue;
            };
            loaded_any = true;
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            for py in 0..16u32 {
                let sy = (py * h) / 16;
                for px in 0..16u32 {
                    let sx = (px * w) / 16;
                    let p = rgba.get_pixel(sx, sy).0;
                    let packed = (p[0] as u32)
                        | ((p[1] as u32) << 8)
                        | ((p[2] as u32) << 16)
                        | ((p[3] as u32) << 24);
                    pixels[(row_y + py as usize) * 256 + tile_idx * 16 + px as usize] = packed;
                }
            }
        }
    }

    if loaded_any {
        Some(Arc::new(pixels))
    } else {
        None
    }
}

#[derive(Clone, Debug)]
struct CachedEmitter {
    bmin: [f32; 3],
    bmax: [f32; 3],
    radiance: [f32; 3],
    range: f32,
    radius: f32,
    power_weight: f32,
}

fn unpack_rgba_linear(packed: u32) -> [f32; 4] {
    let r = ((packed & 0xFF) as f32 / 255.0).powf(2.2);
    let g = (((packed >> 8) & 0xFF) as f32 / 255.0).powf(2.2);
    let b = (((packed >> 16) & 0xFF) as f32 / 255.0).powf(2.2);
    let a = ((packed >> 24) & 0xFF) as f32 / 255.0;
    [r, g, b, a]
}

fn compute_atlas_tile_albedo(atlas: &[u32]) -> [[f32; 3]; 16] {
    let mut out = [[0.5, 0.5, 0.5]; 16];
    for (tile, dst) in out.iter_mut().enumerate() {
        let mut sum = [0.0f32; 3];
        let mut count = 0.0f32;
        for py in 0..16usize {
            for px in 0..16usize {
                let packed = atlas[py * 256 + tile * 16 + px];
                let c = unpack_rgba_linear(packed);
                if c[3] > 0.05 {
                    sum[0] += c[0];
                    sum[1] += c[1];
                    sum[2] += c[2];
                    count += 1.0;
                }
            }
        }
        if count > 0.0 {
            *dst = [sum[0] / count, sum[1] / count, sum[2] / count];
        }
    }
    out
}

fn resolve_cpu_tile(rgba: u32, visual: u32) -> Option<usize> {
    let r = rgba & 0xFF;
    let g = (rgba >> 8) & 0xFF;
    let b = (rgba >> 16) & 0xFF;
    if (visual & VIS_LIQUID) != 0 {
        if (visual & VIS_FROZEN) != 0 {
            return Some(9);
        }
        if (visual & VIS_CORRODED) != 0 {
            return Some(8);
        }
        return Some(7);
    }
    if (visual & VIS_TRANSLUCENT) != 0 {
        return Some(5);
    }
    if (visual & VIS_INFECTED) != 0 {
        return Some(10);
    }
    if (visual & VIS_EMISSIVE) != 0 && g > r && g > 140 {
        return Some(14);
    }
    if (visual & VIS_METALLIC) != 0 {
        if r > 180 && g > 100 && b < 80 {
            return Some(11);
        }
        if r > 180 && g < 110 {
            return Some(13);
        }
        return Some(4);
    }
    if g > r + 25 && g > b + 25 {
        return Some(2);
    }
    if r < 90 && g < 90 && b < 100 {
        return Some(0);
    }
    if r > 145 && g > 140 && b > 130 {
        return Some(3);
    }
    if r > 135 && g > 85 && b < 80 {
        return Some(6);
    }
    Some(1)
}

fn eval_cpu_emissive(
    rgba: u32,
    visual: u32,
    tile_albedos: Option<&[[f32; 3]; 16]>,
) -> Option<[f32; 3]> {
    if (rgba >> 24) == 0 {
        return None;
    }
    const EMIT_MASK: u32 = VIS_EMISSIVE
        | VIS_HEAT_GLOW
        | VIS_BURNING
        | VIS_CHARGED
        | VIS_INFECTED
        | VIS_WARNING
        | VIS_STASIS;
    let emissive_byte = (visual >> 24) & 0xFF;
    if (visual & EMIT_MASK) == 0 && emissive_byte == 0 {
        return None;
    }

    let base = unpack_rgba_linear(rgba);
    let mut albedo = [base[0], base[1], base[2]];
    if let Some(tiles) = tile_albedos {
        if let Some(tile) = resolve_cpu_tile(rgba, visual) {
            if tile != 2 && tile != 13 && tile != 4 {
                albedo = tiles[tile];
            }
        }
    }

    let amt = emissive_byte as f32 / 255.0;
    let mut rad = [0.0f32; 3];

    if (visual & VIS_EMISSIVE) != 0 || emissive_byte > 0 {
        let s = amt.max(0.35) * 3.8;
        rad[0] += albedo[0] * s;
        rad[1] += albedo[1] * s;
        rad[2] += albedo[2] * s;
    }
    if (visual & VIS_HEAT_GLOW) != 0 {
        let s = (amt * 3.8).max(0.55);
        rad[0] += 2.2 * s;
        rad[1] += 0.55 * s;
        rad[2] += 0.08 * s;
    }
    if (visual & VIS_INFECTED) != 0 {
        rad[0] += 0.75 * 0.75;
        rad[1] += 0.18 * 0.75;
        rad[2] += 1.15 * 0.75;
    }
    if (visual & VIS_BURNING) != 0 {
        rad[0] += 3.6 * 0.85;
        rad[1] += 1.05 * 0.85;
        rad[2] += 0.16 * 0.85;
    }
    if (visual & VIS_CHARGED) != 0 {
        rad[0] += 0.45 * 0.75;
        rad[1] += 1.45 * 0.75;
        rad[2] += 3.60 * 0.75;
    }
    if (visual & VIS_WARNING) != 0 {
        rad[0] += 2.2 * 0.65;
        rad[1] += 0.45 * 0.65;
        rad[2] += 0.08 * 0.65;
    }
    if (visual & VIS_STASIS) != 0 {
        rad[0] += 0.35 * 0.75;
        rad[1] += 0.95 * 0.75;
        rad[2] += 2.40 * 0.75;
    }

    let peak = rad[0].max(rad[1]).max(rad[2]);
    if peak > 0.04 {
        Some(rad)
    } else {
        None
    }
}

fn extract_chunk_emitters(
    chunk: &VoxelChunkUpload,
    tile_albedos: Option<&[[f32; 3]; 16]>,
) -> Vec<CachedEmitter> {
    let mut out = Vec::new();
    let dx_l1 = chunk.dims_l1[0].max(1);
    let dy_l1 = chunk.dims_l1[1].max(1);
    for (idx, l1) in chunk.l1_cells.iter().enumerate() {
        let l1_ref = l1[0];
        if l1_ref == 0 {
            continue;
        }
        let lx = (idx as u32) % dx_l1;
        let ly = ((idx as u32) / dx_l1) % dy_l1;
        let lz = (idx as u32) / (dx_l1 * dy_l1);
        let wx = (chunk.origin_l1[0] + lx as i32) as f32;
        let wy = (chunk.origin_l1[1] + ly as i32) as f32;
        let wz = (chunk.origin_l1[2] + lz as i32) as f32;

        if l1_ref == L1_UNIFORM_REF {
            if let Some(rad) = eval_cpu_emissive(l1[1], l1[2], tile_albedos) {
                let peak = rad[0].max(rad[1]).max(rad[2]);
                out.push(CachedEmitter {
                    bmin: [wx, wy, wz],
                    bmax: [wx + 1.0, wy + 1.0, wz + 1.0],
                    radiance: rad,
                    range: (7.0 + peak.sqrt() * 5.0).clamp(7.0, 20.0),
                    radius: 0.55,
                    power_weight: 2.2,
                });
            }
            continue;
        }

        let brick_idx = (l1_ref - 1) as usize;
        let Some(&[occ_lo, occ_hi]) = chunk.brick_occ.get(brick_idx) else {
            continue;
        };
        if (occ_lo | occ_hi) == 0 {
            continue;
        }

        let mut any = false;
        let mut bmin = [f32::MAX; 3];
        let mut bmax = [f32::MIN; 3];
        let mut rad_sum = [0.0f32; 3];
        let mut total_fill = 0.0f32;

        for sub in 0..64usize {
            let word = if sub < 32 { occ_lo } else { occ_hi };
            if ((word >> (sub & 31)) & 1) == 0 {
                continue;
            }
            let Some(&cell) = chunk.brick_cells.get(brick_idx * 64 + sub) else {
                continue;
            };
            if (cell[0] >> 24) == 0 || (cell[2] | cell[3]) == 0 {
                continue;
            }
            let Some(rad) = eval_cpu_emissive(cell[0], cell[1], tile_albedos) else {
                continue;
            };
            let fill = ((cell[2].count_ones() + cell[3].count_ones()) as f32) / 64.0;
            let sx = (sub % 4) as f32 * 0.25;
            let sy = ((sub / 4) % 4) as f32 * 0.25;
            let sz = (sub / 16) as f32 * 0.25;

            any = true;
            bmin[0] = bmin[0].min(wx + sx);
            bmin[1] = bmin[1].min(wy + sy);
            bmin[2] = bmin[2].min(wz + sz);
            bmax[0] = bmax[0].max(wx + sx + 0.25);
            bmax[1] = bmax[1].max(wy + sy + 0.25);
            bmax[2] = bmax[2].max(wz + sz + 0.25);
            rad_sum[0] += rad[0] * fill;
            rad_sum[1] += rad[1] * fill;
            rad_sum[2] += rad[2] * fill;
            total_fill += fill;
        }

        if any && total_fill > 0.01 {
            let avg_rad = [
                rad_sum[0] / total_fill,
                rad_sum[1] / total_fill,
                rad_sum[2] / total_fill,
            ];
            let peak = avg_rad[0].max(avg_rad[1]).max(avg_rad[2]);
            let extent_max = (bmax[0] - bmin[0])
                .max(bmax[1] - bmin[1])
                .max(bmax[2] - bmin[2]);
            out.push(CachedEmitter {
                bmin,
                bmax,
                radiance: avg_rad,
                range: (6.5 + peak.sqrt() * 5.2).clamp(6.5, 20.0),
                radius: (extent_max * 0.5).clamp(0.28, 0.75),
                power_weight: (total_fill.sqrt() * 1.1).clamp(0.70, 2.4),
            });
        }
    }
    merge_nearby_emitters(out)
}

fn emitter_chroma_similar(a: [f32; 3], b: [f32; 3]) -> bool {
    let la = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    let lb = (b[0] * b[0] + b[1] * b[1] + b[2] * b[2]).sqrt();
    if la < 1e-4 || lb < 1e-4 {
        return false;
    }
    let dot = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]) / (la * lb);
    dot >= 0.88
}

/// Merge nearby same-hue emitters (even separated by 0.0625m..0.25m gaps) into area emitters.
fn merge_nearby_emitters(mut emitters: Vec<CachedEmitter>) -> Vec<CachedEmitter> {
    const MERGE_GAP: f32 = 0.28;
    const MAX_EXTENT: f32 = 6.0;

    if emitters.len() <= 1 {
        return emitters;
    }

    let mut merged = true;
    while merged {
        merged = false;
        let mut i = 0;
        while i < emitters.len() {
            let mut j = i + 1;
            while j < emitters.len() {
                let a = &emitters[i];
                let b = &emitters[j];
                let gap_ok = a.bmax[0] + MERGE_GAP >= b.bmin[0]
                    && b.bmax[0] + MERGE_GAP >= a.bmin[0]
                    && a.bmax[1] + MERGE_GAP >= b.bmin[1]
                    && b.bmax[1] + MERGE_GAP >= a.bmin[1]
                    && a.bmax[2] + MERGE_GAP >= b.bmin[2]
                    && b.bmax[2] + MERGE_GAP >= a.bmin[2];

                if gap_ok && emitter_chroma_similar(a.radiance, b.radiance) {
                    let nbmin = [
                        a.bmin[0].min(b.bmin[0]),
                        a.bmin[1].min(b.bmin[1]),
                        a.bmin[2].min(b.bmin[2]),
                    ];
                    let nbmax = [
                        a.bmax[0].max(b.bmax[0]),
                        a.bmax[1].max(b.bmax[1]),
                        a.bmax[2].max(b.bmax[2]),
                    ];
                    let ext_x = nbmax[0] - nbmin[0];
                    let ext_y = nbmax[1] - nbmin[1];
                    let ext_z = nbmax[2] - nbmin[2];
                    let ext_max = ext_x.max(ext_y).max(ext_z);
                    if ext_max <= MAX_EXTENT {
                        let wa = a.power_weight.max(0.1);
                        let wb = b.power_weight.max(0.1);
                        let wsum = wa + wb;
                        let rad = [
                            (a.radiance[0] * wa + b.radiance[0] * wb) / wsum,
                            (a.radiance[1] * wa + b.radiance[1] * wb) / wsum,
                            (a.radiance[2] * wa + b.radiance[2] * wb) / wsum,
                        ];
                        let peak = rad[0].max(rad[1]).max(rad[2]);
                        emitters[i] = CachedEmitter {
                            bmin: nbmin,
                            bmax: nbmax,
                            radiance: rad,
                            range: (a.range.max(b.range).max(6.5 + peak.sqrt() * 5.2))
                                .clamp(6.5, 22.0),
                            radius: (ext_max * 0.5).clamp(0.28, 1.5),
                            power_weight: (wa * wa + wb * wb).sqrt().clamp(0.70, 3.2),
                        };
                        emitters.swap_remove(j);
                        merged = true;
                        continue;
                    }
                }
                j += 1;
            }
            i += 1;
        }
    }
    emitters
}

#[derive(Default)]
struct SceneUploadScratch {
    chunk_order: Vec<(f32, usize)>,
    gpu_chunks: Vec<VoxelChunkGpu>,
    flat_l1: Vec<[u32; 4]>,
    flat_occ: Vec<[u32; 2]>,
    flat_cells: Vec<[u32; 4]>,
    emitter_ranked: Vec<(f32, usize)>,
    gpu_emitters: Vec<VoxelEmitterGpu>,
}

struct HostBuffer {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    capacity_bytes: vk::DeviceSize,
}

impl HostBuffer {
    fn new(
        context: &VulkanContext,
        size: vk::DeviceSize,
        usage: vk::BufferUsageFlags,
    ) -> Result<Self> {
        let alloc_size = size.max(64);
        let (buffer, memory) = create_buffer(
            context,
            alloc_size,
            usage,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )?;
        Ok(Self {
            buffer,
            memory,
            capacity_bytes: alloc_size,
        })
    }

    fn write_bytes(&self, device: &ash::Device, data: &[u8]) -> Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        unsafe {
            let ptr = device.map_memory(
                self.memory,
                0,
                data.len() as vk::DeviceSize,
                vk::MemoryMapFlags::empty(),
            )?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len());
            device.unmap_memory(self.memory);
        }
        Ok(())
    }

    fn destroy(&mut self, device: &ash::Device) {
        unsafe {
            if self.buffer != vk::Buffer::null() {
                device.destroy_buffer(self.buffer, None);
                self.buffer = vk::Buffer::null();
            }
            if self.memory != vk::DeviceMemory::null() {
                device.free_memory(self.memory, None);
                self.memory = vk::DeviceMemory::null();
            }
        }
        self.capacity_bytes = 0;
    }
}

struct ImageResource {
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
}

impl ImageResource {
    fn create(
        context: &VulkanContext,
        width: u32,
        height: u32,
        format: vk::Format,
    ) -> Result<Self> {
        let device = &context.device;
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image =
            unsafe { device.create_image(&info, None) }.context("VoxelDdaPass: create image")?;
        let reqs = unsafe { device.get_image_memory_requirements(image) };
        let mem_type = find_memory_type(
            context,
            reqs.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .context("VoxelDdaPass: find image memory type")?;
        let alloc = vk::MemoryAllocateInfo::default()
            .allocation_size(reqs.size)
            .memory_type_index(mem_type);
        let memory = unsafe { device.allocate_memory(&alloc, None) }
            .context("VoxelDdaPass: allocate image")?;
        unsafe { device.bind_image_memory(image, memory, 0) }
            .context("VoxelDdaPass: bind image memory")?;

        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(format)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        let view = unsafe { device.create_image_view(&view_info, None) }
            .context("VoxelDdaPass: create image view")?;
        Ok(Self {
            image,
            memory,
            view,
        })
    }

    fn destroy(&mut self, device: &ash::Device) {
        unsafe {
            if self.view != vk::ImageView::null() {
                device.destroy_image_view(self.view, None);
                self.view = vk::ImageView::null();
            }
            if self.image != vk::Image::null() {
                device.destroy_image(self.image, None);
                self.image = vk::Image::null();
            }
            if self.memory != vk::DeviceMemory::null() {
                device.free_memory(self.memory, None);
                self.memory = vk::DeviceMemory::null();
            }
        }
    }
}

struct DummyImages {
    read_f16: ImageResource,
    read_u32: ImageResource,
    rw_f16: ImageResource,
    rw_f32: ImageResource,
    rw_u32: ImageResource,
    rw_r32f: ImageResource,
    initialized: bool,
}

impl DummyImages {
    fn create(context: &VulkanContext) -> Result<Self> {
        Ok(Self {
            read_f16: ImageResource::create(context, 1, 1, vk::Format::R16G16B16A16_SFLOAT)?,
            read_u32: ImageResource::create(context, 1, 1, vk::Format::R32G32B32A32_UINT)?,
            rw_f16: ImageResource::create(context, 1, 1, vk::Format::R16G16B16A16_SFLOAT)?,
            rw_f32: ImageResource::create(context, 1, 1, vk::Format::R32G32B32A32_SFLOAT)?,
            rw_u32: ImageResource::create(context, 1, 1, vk::Format::R32G32B32A32_UINT)?,
            rw_r32f: ImageResource::create(context, 1, 1, vk::Format::R32_SFLOAT)?,
            initialized: false,
        })
    }

    fn destroy(&mut self, device: &ash::Device) {
        self.read_f16.destroy(device);
        self.read_u32.destroy(device);
        self.rw_f16.destroy(device);
        self.rw_f32.destroy(device);
        self.rw_u32.destroy(device);
        self.rw_r32f.destroy(device);
    }
}

struct TargetImages {
    extent: vk::Extent2D,
    color: ImageResource,
    depth: ImageResource,
    direct: ImageResource,
    albedo: ImageResource,
    spec_env: ImageResource,
    pos_norm: ImageResource,
    accum_gi: ImageResource,
    denoise_a: ImageResource,
    denoise_b: ImageResource,
    glass: ImageResource,
    initialized: bool,
    history_ready: bool,
}

impl TargetImages {
    fn create(context: &VulkanContext, extent: vk::Extent2D) -> Result<Self> {
        let w = extent.width.max(1);
        let h = extent.height.max(1);
        Ok(Self {
            extent: vk::Extent2D {
                width: w,
                height: h,
            },
            color: ImageResource::create(context, w, h, vk::Format::R16G16B16A16_SFLOAT)?,
            depth: ImageResource::create(context, w, h, vk::Format::R32_SFLOAT)?,
            direct: ImageResource::create(context, w, h, vk::Format::R16G16B16A16_SFLOAT)?,
            albedo: ImageResource::create(context, w, h, vk::Format::R16G16B16A16_SFLOAT)?,
            spec_env: ImageResource::create(context, w, h, vk::Format::R16G16B16A16_SFLOAT)?,
            pos_norm: ImageResource::create(context, w, h, vk::Format::R32G32B32A32_SFLOAT)?,
            accum_gi: ImageResource::create(context, w, h, vk::Format::R16G16B16A16_SFLOAT)?,
            denoise_a: ImageResource::create(context, w, h, vk::Format::R16G16B16A16_SFLOAT)?,
            denoise_b: ImageResource::create(context, w, h, vk::Format::R16G16B16A16_SFLOAT)?,
            glass: ImageResource::create(context, w, h, vk::Format::R32G32B32A32_UINT)?,
            initialized: false,
            history_ready: false,
        })
    }

    fn all_images(&self) -> [vk::Image; 10] {
        [
            self.color.image,
            self.depth.image,
            self.direct.image,
            self.albedo.image,
            self.spec_env.image,
            self.pos_norm.image,
            self.accum_gi.image,
            self.denoise_a.image,
            self.denoise_b.image,
            self.glass.image,
        ]
    }

    fn destroy(&mut self, device: &ash::Device) {
        self.color.destroy(device);
        self.depth.destroy(device);
        self.direct.destroy(device);
        self.albedo.destroy(device);
        self.spec_env.destroy(device);
        self.pos_norm.destroy(device);
        self.accum_gi.destroy(device);
        self.denoise_a.destroy(device);
        self.denoise_b.destroy(device);
        self.glass.destroy(device);
    }
}

struct FrameSlot {
    ds_primary: vk::DescriptorSet,
    ds_gi: vk::DescriptorSet,
    ds_denoise_0: vk::DescriptorSet,
    ds_denoise_1: vk::DescriptorSet,
    ds_denoise_2: vk::DescriptorSet,
    ds_composite: vk::DescriptorSet,
    ubo_buf: HostBuffer,
    chunks_buf: HostBuffer,
    l1_buf: HostBuffer,
    occ_buf: HostBuffer,
    cells_buf: HostBuffer,
    atlas_buf: HostBuffer,
    emitters_buf: HostBuffer,
    uploaded_signature: Option<u64>,
    atlas_uploaded: bool,
    target: Option<TargetImages>,
}

impl FrameSlot {
    fn new(context: &VulkanContext, sets: &[vk::DescriptorSet]) -> Result<Self> {
        let ubo_buf = HostBuffer::new(
            context,
            std::mem::size_of::<VoxelFrameUbo>() as vk::DeviceSize,
            vk::BufferUsageFlags::UNIFORM_BUFFER,
        )?;
        let chunks_buf = HostBuffer::new(context, 256, vk::BufferUsageFlags::STORAGE_BUFFER)?;
        let l1_buf = HostBuffer::new(context, 256, vk::BufferUsageFlags::STORAGE_BUFFER)?;
        let occ_buf = HostBuffer::new(context, 256, vk::BufferUsageFlags::STORAGE_BUFFER)?;
        let cells_buf = HostBuffer::new(context, 256, vk::BufferUsageFlags::STORAGE_BUFFER)?;
        let atlas_buf = HostBuffer::new(context, 64, vk::BufferUsageFlags::STORAGE_BUFFER)?;
        let emitters_buf = HostBuffer::new(context, 256, vk::BufferUsageFlags::STORAGE_BUFFER)?;
        Ok(Self {
            ds_primary: sets[0],
            ds_denoise_0: sets[1],
            ds_denoise_1: sets[2],
            ds_denoise_2: sets[3],
            ds_composite: sets[4],
            ds_gi: sets[5],
            ubo_buf,
            chunks_buf,
            l1_buf,
            occ_buf,
            cells_buf,
            atlas_buf,
            emitters_buf,
            uploaded_signature: None,
            atlas_uploaded: false,
            target: None,
        })
    }

    fn ensure_ssbo(buf: &mut HostBuffer, context: &VulkanContext, bytes: &[u8]) -> Result<()> {
        let needed = (bytes.len() as vk::DeviceSize).max(64);
        if buf.capacity_bytes < needed {
            buf.destroy(&context.device);
            let grown = (needed * 3 / 2).max(needed);
            *buf = HostBuffer::new(context, grown, vk::BufferUsageFlags::STORAGE_BUFFER)?;
        }
        buf.write_bytes(&context.device, bytes)
    }

    fn upload_scene(
        &mut self,
        context: &VulkanContext,
        scene: &VoxelSceneUpload,
        camera_pos: [f32; 3],
        scratch: &mut SceneUploadScratch,
    ) -> Result<bool> {
        if self.uploaded_signature == Some(scene.signature) {
            return Ok(false);
        }
        scratch.chunk_order.clear();
        scratch.gpu_chunks.clear();
        scratch.flat_l1.clear();
        scratch.flat_occ.clear();
        scratch.flat_cells.clear();

        for (idx, chunk) in scene.chunks.iter().enumerate() {
            let cx = chunk.origin_l1[0] as f32 + chunk.dims_l1[0] as f32 * 0.5;
            let cy = chunk.origin_l1[1] as f32 + chunk.dims_l1[1] as f32 * 0.5;
            let cz = chunk.origin_l1[2] as f32 + chunk.dims_l1[2] as f32 * 0.5;
            let dx = cx - camera_pos[0];
            let dy = cy - camera_pos[1];
            let dz = cz - camera_pos[2];
            scratch.chunk_order.push((dx * dx + dy * dy + dz * dz, idx));
        }
        scratch.chunk_order.sort_by(|a, b| a.0.total_cmp(&b.0));

        for &(_, idx) in &scratch.chunk_order {
            let chunk = &scene.chunks[idx];
            let l1_offset = scratch.flat_l1.len() as u32;
            let brick_offset = scratch.flat_occ.len() as u32;
            scratch.flat_l1.extend_from_slice(&chunk.l1_cells);
            scratch.flat_occ.extend_from_slice(&chunk.brick_occ);
            scratch.flat_cells.extend_from_slice(&chunk.brick_cells);
            scratch.gpu_chunks.push(VoxelChunkGpu {
                origin_and_l1_offset: [
                    chunk.origin_l1[0],
                    chunk.origin_l1[1],
                    chunk.origin_l1[2],
                    l1_offset as i32,
                ],
                dims_and_brick_offset: [
                    chunk.dims_l1[0],
                    chunk.dims_l1[1],
                    chunk.dims_l1[2],
                    brick_offset,
                ],
            });
        }

        if scratch.gpu_chunks.is_empty() {
            scratch.gpu_chunks.push(VoxelChunkGpu::default());
        }
        if scratch.flat_l1.is_empty() {
            scratch.flat_l1.push([0; 4]);
        }
        if scratch.flat_occ.is_empty() {
            scratch.flat_occ.push([0; 2]);
        }
        if scratch.flat_cells.is_empty() {
            scratch.flat_cells.push([0; 4]);
        }

        Self::ensure_ssbo(
            &mut self.chunks_buf,
            context,
            bytemuck::cast_slice(&scratch.gpu_chunks),
        )?;
        Self::ensure_ssbo(
            &mut self.l1_buf,
            context,
            bytemuck::cast_slice(&scratch.flat_l1),
        )?;
        Self::ensure_ssbo(
            &mut self.occ_buf,
            context,
            bytemuck::cast_slice(&scratch.flat_occ),
        )?;
        Self::ensure_ssbo(
            &mut self.cells_buf,
            context,
            bytemuck::cast_slice(&scratch.flat_cells),
        )?;
        if !self.atlas_uploaded {
            if let Some(atlas) = &scene.atlas {
                Self::ensure_ssbo(&mut self.atlas_buf, context, bytemuck::cast_slice(atlas))?;
                self.atlas_uploaded = true;
            }
        }
        self.uploaded_signature = Some(scene.signature);
        Ok(true)
    }

    fn upload_active_emitters(
        &mut self,
        context: &VulkanContext,
        cached_emitters: &[CachedEmitter],
        camera_pos: [f32; 3],
        scratch: &mut SceneUploadScratch,
    ) -> Result<u32> {
        scratch.emitter_ranked.clear();
        for (idx, e) in cached_emitters.iter().enumerate() {
            let cx = 0.5 * (e.bmin[0] + e.bmax[0]);
            let cy = 0.5 * (e.bmin[1] + e.bmax[1]);
            let cz = 0.5 * (e.bmin[2] + e.bmax[2]);
            let dx = cx - camera_pos[0];
            let dy = cy - camera_pos[1];
            let dz = cz - camera_pos[2];
            let d2 = dx * dx + dy * dy + dz * dz;
            let peak = e.radiance[0].max(e.radiance[1]).max(e.radiance[2]);
            let score = (peak * e.power_weight) / (1.0 + d2 * 0.05);
            scratch.emitter_ranked.push((score, idx));
        }
        scratch.emitter_ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
        scratch.emitter_ranked.truncate(MAX_ACTIVE_EMITTERS);

        let count = scratch.emitter_ranked.len() as u32;
        scratch.gpu_emitters.clear();
        for &(_, idx) in &scratch.emitter_ranked {
            let e = &cached_emitters[idx];
            scratch.gpu_emitters.push(VoxelEmitterGpu {
                bmin_range: [e.bmin[0], e.bmin[1], e.bmin[2], e.range],
                bmax_radius: [e.bmax[0], e.bmax[1], e.bmax[2], e.radius],
                color_intensity: [e.radiance[0], e.radiance[1], e.radiance[2], e.power_weight],
            });
        }
        if scratch.gpu_emitters.is_empty() {
            scratch.gpu_emitters.push(VoxelEmitterGpu::default());
        }
        Self::ensure_ssbo(
            &mut self.emitters_buf,
            context,
            bytemuck::cast_slice(&scratch.gpu_emitters),
        )?;
        Ok(count)
    }

    fn destroy(&mut self, device: &ash::Device) {
        self.ubo_buf.destroy(device);
        self.chunks_buf.destroy(device);
        self.l1_buf.destroy(device);
        self.occ_buf.destroy(device);
        self.cells_buf.destroy(device);
        self.atlas_buf.destroy(device);
        self.emitters_buf.destroy(device);
        if let Some(mut t) = self.target.take() {
            t.destroy(device);
        }
    }
}

/// Three-tier compute DDA render pass node registered in `GraphRenderer`.
pub struct VoxelDdaPass {
    context: Arc<VulkanContext>,
    descriptor_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    pipeline_primary: ComputePipeline,
    pipeline_gi: ComputePipeline,
    pipeline_denoise: ComputePipeline,
    pipeline_composite: ComputePipeline,
    slots: [FrameSlot; 2],
    dummies: DummyImages,
    scene: Option<Arc<VoxelSceneUpload>>,
    cached_emitter_sig: Option<u64>,
    cached_emitters: Vec<CachedEmitter>,
    chunk_emitter_cache: HashMap<u64, (u64, Vec<CachedEmitter>)>,
    atlas_tile_albedos: Option<[[f32; 3]; 16]>,
    upload_scratch: SceneUploadScratch,
    prev_view_proj: Option<[[f32; 4]; 4]>,
}

impl VoxelDdaPass {
    /// Allocate Vulkan pipelines, descriptor pool/layout, and double-buffered SSBOs.
    pub fn new(context: Arc<VulkanContext>) -> Result<Self> {
        let device = &context.device;
        let layout_bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::UBO_BINDING)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::CHUNKS_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::L1_CELLS_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::BRICK_OCC_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::BRICK_CELLS_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::SPBR_ATLAS_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::EMITTERS_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::FORWARD_COLOR_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::FORWARD_DEPTH_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::PREV_GI_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::PREV_POS_NORM_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::GI_READ_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::POS_NORM_READ_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::DIRECT_READ_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::ALBEDO_READ_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::SPEC_ENV_READ_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::GLASS_READ_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::OUT_COLOR_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::OUT_DEPTH_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::DIRECT_OUT_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::ALBEDO_OUT_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::SPEC_ENV_OUT_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::POS_NORM_OUT_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::GI_OUT_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::GLASS_OUT_TEX_BINDING)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
        ];

        let descriptor_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&layout_bindings),
                None,
            )
        }
        .context("VoxelDdaPass: create descriptor layout")?;

        const TOTAL_SETS: u32 = 12;
        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UNIFORM_BUFFER,
                descriptor_count: TOTAL_SETS,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_BUFFER,
                descriptor_count: TOTAL_SETS * 6,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLED_IMAGE,
                descriptor_count: TOTAL_SETS * 10,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_IMAGE,
                descriptor_count: TOTAL_SETS * 8,
            },
        ];
        let descriptor_pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(TOTAL_SETS)
                    .pool_sizes(&pool_sizes),
                None,
            )
        }
        .context("VoxelDdaPass: create descriptor pool")?;

        let layouts = [descriptor_layout; TOTAL_SETS as usize];
        let allocated = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(descriptor_pool)
                    .set_layouts(&layouts),
            )
        }
        .context("VoxelDdaPass: allocate descriptor sets")?;

        let push_range = [vk::PushConstantRange {
            stage_flags: vk::ShaderStageFlags::COMPUTE,
            offset: 0,
            size: bindings::PUSH_CONSTANT_SIZE,
        }];

        let primary_mod = load_shader_module(device, VOXEL_DDA_SPV)?;
        let pipeline_primary = ComputePipeline::new(
            device,
            primary_mod,
            ENTRY_VOXEL_DDA,
            std::slice::from_ref(&descriptor_layout),
            &push_range,
        );
        unsafe { device.destroy_shader_module(primary_mod, None) };
        let pipeline_primary = pipeline_primary?;

        let gi_mod = load_shader_module(device, VOXEL_GI_SPV)?;
        let pipeline_gi = ComputePipeline::new(
            device,
            gi_mod,
            ENTRY_VOXEL_GI,
            std::slice::from_ref(&descriptor_layout),
            &push_range,
        );
        unsafe { device.destroy_shader_module(gi_mod, None) };
        let pipeline_gi = pipeline_gi?;

        let denoise_mod = load_shader_module(device, VOXEL_GI_DENOISE_SPV)?;
        let pipeline_denoise = ComputePipeline::new(
            device,
            denoise_mod,
            ENTRY_VOXEL_GI_DENOISE,
            std::slice::from_ref(&descriptor_layout),
            &push_range,
        );
        unsafe { device.destroy_shader_module(denoise_mod, None) };
        let pipeline_denoise = pipeline_denoise?;

        let composite_mod = load_shader_module(device, VOXEL_COMPOSITE_SPV)?;
        let pipeline_composite = ComputePipeline::new(
            device,
            composite_mod,
            ENTRY_VOXEL_COMPOSITE,
            std::slice::from_ref(&descriptor_layout),
            &push_range,
        );
        unsafe { device.destroy_shader_module(composite_mod, None) };
        let pipeline_composite = pipeline_composite?;

        let slot0 = FrameSlot::new(&context, &allocated[0..6])?;
        let slot1 = FrameSlot::new(&context, &allocated[6..12])?;
        let dummies = DummyImages::create(&context)?;

        Ok(Self {
            context,
            descriptor_layout,
            descriptor_pool,
            pipeline_primary,
            pipeline_gi,
            pipeline_denoise,
            pipeline_composite,
            slots: [slot0, slot1],
            dummies,
            scene: None,
            cached_emitter_sig: None,
            cached_emitters: Vec::new(),
            chunk_emitter_cache: HashMap::new(),
            atlas_tile_albedos: None,
            upload_scratch: SceneUploadScratch::default(),
            prev_view_proj: None,
        })
    }

    /// Replace the active voxel scene snapshot rendered by subsequent frames.
    pub fn set_scene(&mut self, scene: Option<Arc<VoxelSceneUpload>>) {
        self.scene = scene;
    }

    /// Release GPU target images before swapchain/offscreen resize.
    pub fn drop_targets(&mut self) {
        for slot in &mut self.slots {
            if let Some(mut t) = slot.target.take() {
                t.destroy(&self.context.device);
            }
        }
        self.prev_view_proj = None;
    }
}

impl Drop for VoxelDdaPass {
    fn drop(&mut self) {
        let device = &self.context.device;
        for slot in &mut self.slots {
            slot.destroy(device);
        }
        self.dummies.destroy(device);
        unsafe {
            if self.descriptor_pool != vk::DescriptorPool::null() {
                device.destroy_descriptor_pool(self.descriptor_pool, None);
            }
            if self.descriptor_layout != vk::DescriptorSetLayout::null() {
                device.destroy_descriptor_set_layout(self.descriptor_layout, None);
            }
        }
    }
}

fn image_barrier<'a>(
    image: vk::Image,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
    src_access: vk::AccessFlags,
    dst_access: vk::AccessFlags,
) -> vk::ImageMemoryBarrier<'a> {
    vk::ImageMemoryBarrier::default()
        .old_layout(old_layout)
        .new_layout(new_layout)
        .src_access_mask(src_access)
        .dst_access_mask(dst_access)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        })
}

#[derive(Clone, Copy)]
struct SlotBuffers {
    ubo: vk::Buffer,
    chunks: vk::Buffer,
    l1: vk::Buffer,
    occ: vk::Buffer,
    cells: vk::Buffer,
    atlas: vk::Buffer,
    emitters: vk::Buffer,
}

#[expect(
    clippy::too_many_arguments,
    reason = "one write per binding in the fixed voxel-DDA descriptor layout"
)]
fn write_full_descriptor_set(
    device: &ash::Device,
    ds: vk::DescriptorSet,
    bufs: SlotBuffers,
    forward_color_view: vk::ImageView,
    forward_depth_view: vk::ImageView,
    prev_gi_view: vk::ImageView,
    prev_pos_norm_view: vk::ImageView,
    gi_read_view: vk::ImageView,
    pos_norm_read_view: vk::ImageView,
    direct_read_view: vk::ImageView,
    albedo_read_view: vk::ImageView,
    spec_env_read_view: vk::ImageView,
    glass_read_view: vk::ImageView,
    out_color_view: vk::ImageView,
    out_depth_view: vk::ImageView,
    direct_out_view: vk::ImageView,
    albedo_out_view: vk::ImageView,
    spec_env_out_view: vk::ImageView,
    pos_norm_out_view: vk::ImageView,
    gi_out_view: vk::ImageView,
    glass_out_view: vk::ImageView,
) {
    let ubo_info = vk::DescriptorBufferInfo::default()
        .buffer(bufs.ubo)
        .range(std::mem::size_of::<VoxelFrameUbo>() as vk::DeviceSize);
    let chunks_info = vk::DescriptorBufferInfo::default()
        .buffer(bufs.chunks)
        .range(vk::WHOLE_SIZE);
    let l1_info = vk::DescriptorBufferInfo::default()
        .buffer(bufs.l1)
        .range(vk::WHOLE_SIZE);
    let occ_info = vk::DescriptorBufferInfo::default()
        .buffer(bufs.occ)
        .range(vk::WHOLE_SIZE);
    let cells_info = vk::DescriptorBufferInfo::default()
        .buffer(bufs.cells)
        .range(vk::WHOLE_SIZE);
    let atlas_info = vk::DescriptorBufferInfo::default()
        .buffer(bufs.atlas)
        .range(vk::WHOLE_SIZE);
    let emitters_info = vk::DescriptorBufferInfo::default()
        .buffer(bufs.emitters)
        .range(vk::WHOLE_SIZE);

    let img_ro = |view: vk::ImageView| {
        vk::DescriptorImageInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
    };
    let img_depth = |view: vk::ImageView| {
        vk::DescriptorImageInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL)
    };
    let img_rw = |view: vk::ImageView| {
        vk::DescriptorImageInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::GENERAL)
    };

    let fwd_col_i = img_ro(forward_color_view);
    let fwd_dep_i = img_depth(forward_depth_view);
    let prev_gi_i = img_ro(prev_gi_view);
    let prev_pn_i = img_ro(prev_pos_norm_view);
    let gi_r_i = img_ro(gi_read_view);
    let pn_r_i = img_ro(pos_norm_read_view);
    let dir_r_i = img_ro(direct_read_view);
    let alb_r_i = img_ro(albedo_read_view);
    let spc_r_i = img_ro(spec_env_read_view);
    let gls_r_i = img_ro(glass_read_view);

    let out_col_i = img_rw(out_color_view);
    let out_dep_i = img_rw(out_depth_view);
    let dir_w_i = img_rw(direct_out_view);
    let alb_w_i = img_rw(albedo_out_view);
    let spc_w_i = img_rw(spec_env_out_view);
    let pn_w_i = img_rw(pos_norm_out_view);
    let gi_w_i = img_rw(gi_out_view);
    let gls_w_i = img_rw(glass_out_view);

    let writes = [
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::UBO_BINDING)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .buffer_info(std::slice::from_ref(&ubo_info)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::CHUNKS_BINDING)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&chunks_info)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::L1_CELLS_BINDING)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&l1_info)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::BRICK_OCC_BINDING)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&occ_info)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::BRICK_CELLS_BINDING)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&cells_info)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::SPBR_ATLAS_BINDING)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&atlas_info)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::EMITTERS_BINDING)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&emitters_info)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::FORWARD_COLOR_TEX_BINDING)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(std::slice::from_ref(&fwd_col_i)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::FORWARD_DEPTH_TEX_BINDING)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(std::slice::from_ref(&fwd_dep_i)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::PREV_GI_TEX_BINDING)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(std::slice::from_ref(&prev_gi_i)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::PREV_POS_NORM_TEX_BINDING)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(std::slice::from_ref(&prev_pn_i)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::GI_READ_TEX_BINDING)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(std::slice::from_ref(&gi_r_i)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::POS_NORM_READ_TEX_BINDING)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(std::slice::from_ref(&pn_r_i)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::DIRECT_READ_TEX_BINDING)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(std::slice::from_ref(&dir_r_i)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::ALBEDO_READ_TEX_BINDING)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(std::slice::from_ref(&alb_r_i)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::SPEC_ENV_READ_TEX_BINDING)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(std::slice::from_ref(&spc_r_i)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::GLASS_READ_TEX_BINDING)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(std::slice::from_ref(&gls_r_i)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::OUT_COLOR_TEX_BINDING)
            .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
            .image_info(std::slice::from_ref(&out_col_i)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::OUT_DEPTH_TEX_BINDING)
            .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
            .image_info(std::slice::from_ref(&out_dep_i)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::DIRECT_OUT_TEX_BINDING)
            .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
            .image_info(std::slice::from_ref(&dir_w_i)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::ALBEDO_OUT_TEX_BINDING)
            .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
            .image_info(std::slice::from_ref(&alb_w_i)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::SPEC_ENV_OUT_TEX_BINDING)
            .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
            .image_info(std::slice::from_ref(&spc_w_i)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::POS_NORM_OUT_TEX_BINDING)
            .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
            .image_info(std::slice::from_ref(&pn_w_i)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::GI_OUT_TEX_BINDING)
            .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
            .image_info(std::slice::from_ref(&gi_w_i)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(bindings::GLASS_OUT_TEX_BINDING)
            .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
            .image_info(std::slice::from_ref(&gls_w_i)),
    ];
    unsafe { device.update_descriptor_sets(&writes, &[]) };
}

impl RenderPassNode for VoxelDdaPass {
    fn name(&self) -> &str {
        "VoxelDdaPass"
    }

    fn setup(&mut self, graph: &mut RenderGraphBuilder, _settings: &RenderSettings) {
        graph.read_usage(ResourceUsage {
            handle: FORWARD_COLOR_H,
            access: vk::AccessFlags::SHADER_READ,
            stage: vk::PipelineStageFlags::COMPUTE_SHADER,
            layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        });
        graph.read_usage(ResourceUsage {
            handle: FORWARD_DEPTH_H,
            access: vk::AccessFlags::SHADER_READ,
            stage: vk::PipelineStageFlags::COMPUTE_SHADER,
            layout: vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL,
        });
        graph.write_usage(ResourceUsage {
            handle: VOXEL_COLOR_H,
            access: vk::AccessFlags::SHADER_WRITE,
            stage: vk::PipelineStageFlags::COMPUTE_SHADER,
            layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        });
        graph.write_usage(ResourceUsage {
            handle: VOXEL_DEPTH_H,
            access: vk::AccessFlags::SHADER_WRITE,
            stage: vk::PipelineStageFlags::COMPUTE_SHADER,
            layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        });
    }

    fn execute(&mut self, ctx: &RenderContext, resources: &mut GraphResources) -> Result<()> {
        let Some(scene) = self
            .scene
            .as_ref()
            .filter(|s| !s.chunks.is_empty())
            .cloned()
        else {
            return Ok(());
        };
        if !ctx.frame.has_camera
            || ctx.frame.render_mode != RenderMode::Raster
            || ctx.frame.debug_rt != 0
        {
            return Ok(());
        }
        let Some(forward_color_view) = resources.published_view(FORWARD_COLOR_H) else {
            return Ok(());
        };
        let Some(forward_depth_view) = resources.published_view(FORWARD_DEPTH_H) else {
            return Ok(());
        };

        let extent = vk::Extent2D {
            width: ctx.render_extent.width.max(1),
            height: ctx.render_extent.height.max(1),
        };
        let slot_idx = (ctx.frame_index as usize) % 2;
        let prev_idx = 1 - slot_idx;

        if self.cached_emitter_sig != Some(scene.signature) {
            if self.atlas_tile_albedos.is_none() {
                self.atlas_tile_albedos =
                    scene.atlas.as_deref().map(|a| compute_atlas_tile_albedo(a));
            }
            let tile_albedos = self.atlas_tile_albedos.as_ref();
            self.chunk_emitter_cache
                .retain(|&id, _| scene.chunks.iter().any(|c| c.entity_id == id));
            self.cached_emitters.clear();
            for chunk in &scene.chunks {
                let entry = self
                    .chunk_emitter_cache
                    .entry(chunk.entity_id)
                    .or_insert_with(|| {
                        (chunk.version, extract_chunk_emitters(chunk, tile_albedos))
                    });
                if entry.0 != chunk.version {
                    entry.0 = chunk.version;
                    entry.1 = extract_chunk_emitters(chunk, tile_albedos);
                }
                self.cached_emitters.extend(entry.1.iter().cloned());
            }
            self.cached_emitter_sig = Some(scene.signature);
        }

        let (prev_valid, prev_gi_view, prev_pn_view) = {
            let prev_t = self.slots[prev_idx].target.as_ref();
            if let Some(pt) = prev_t.filter(|t| t.extent == extent && t.history_ready) {
                (true, pt.accum_gi.view, pt.pos_norm.view)
            } else {
                (
                    false,
                    self.dummies.read_f16.view,
                    self.dummies.read_f16.view,
                )
            }
        };

        let camera_pos = [
            ctx.frame.camera_pos[0],
            ctx.frame.camera_pos[1],
            ctx.frame.camera_pos[2],
        ];
        let slot = &mut self.slots[slot_idx];
        let recreate = slot.target.as_ref().is_none_or(|t| t.extent != extent);
        if recreate {
            if let Some(mut old) = slot.target.take() {
                old.destroy(&self.context.device);
            }
            slot.target = Some(TargetImages::create(&self.context, extent)?);
        }

        let scene_changed =
            slot.upload_scene(&self.context, &scene, camera_pos, &mut self.upload_scratch)?;
        let emitter_count = slot.upload_active_emitters(
            &self.context,
            &self.cached_emitters,
            camera_pos,
            &mut self.upload_scratch,
        )?;

        let mut sun_dir = [
            ctx.frame.light_dir[0],
            ctx.frame.light_dir[1],
            ctx.frame.light_dir[2],
        ];
        let sun_len =
            (sun_dir[0] * sun_dir[0] + sun_dir[1] * sun_dir[1] + sun_dir[2] * sun_dir[2]).sqrt();
        if sun_len > 1e-5 {
            sun_dir = [
                sun_dir[0] / sun_len,
                sun_dir[1] / sun_len,
                sun_dir[2] / sun_len,
            ];
        } else {
            sun_dir = [0.408, 0.816, 0.408];
        }
        let (sun_rad, sun_col) = (ctx.frame.light_dir[3].max(0.0), ctx.frame.light_color);
        let cursor = scene.cursor.unwrap_or(VoxelCursorUpload {
            active: false,
            min: [0.0; 3],
            max: [0.0; 3],
            color: [1.0, 0.9, 0.3, 1.0],
        });

        let inv_vp = crate::math::mat_inverse(&ctx.frame.view_proj);
        let prev_vp = self.prev_view_proj.unwrap_or(ctx.frame.view_proj);
        let has_prev = u32::from(prev_valid && self.prev_view_proj.is_some());

        let ubo_data = VoxelFrameUbo {
            view_proj: ctx.frame.view_proj,
            inv_view_proj: inv_vp,
            prev_view_proj: prev_vp,
            camera_pos: [
                ctx.frame.camera_pos[0],
                ctx.frame.camera_pos[1],
                ctx.frame.camera_pos[2],
                scene.elapsed_seconds,
            ],
            sun_dir_radiance: [sun_dir[0], sun_dir[1], sun_dir[2], sun_rad],
            sun_color_ambient: sun_col,
            counts: [
                scene.chunks.len() as u32,
                extent.width,
                extent.height,
                u32::from(cursor.active),
            ],
            counts2: [
                emitter_count,
                ctx.frame_index,
                has_prev,
                u32::from(scene_changed),
            ],
            cursor_min: [
                cursor.min[0],
                cursor.min[1],
                cursor.min[2],
                if scene.atlas.is_some() { 16.0 } else { 0.0 },
            ],
            cursor_max: [
                cursor.max[0],
                cursor.max[1],
                cursor.max[2],
                ctx.frame.exposure.max(0.01),
            ],
            cursor_color: cursor.color,
        };
        slot.ubo_buf
            .write_bytes(ctx.device, bytemuck::bytes_of(&ubo_data))?;

        let slot_bufs = SlotBuffers {
            ubo: slot.ubo_buf.buffer,
            chunks: slot.chunks_buf.buffer,
            l1: slot.l1_buf.buffer,
            occ: slot.occ_buf.buffer,
            cells: slot.cells_buf.buffer,
            atlas: slot.atlas_buf.buffer,
            emitters: slot.emitters_buf.buffer,
        };
        let ds_primary = slot.ds_primary;
        let ds_gi = slot.ds_gi;
        let ds_denoise_0 = slot.ds_denoise_0;
        let ds_denoise_1 = slot.ds_denoise_1;
        let ds_denoise_2 = slot.ds_denoise_2;
        let ds_composite = slot.ds_composite;

        let target = slot
            .target
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("voxel dda slot target missing"))?;

        if !self.dummies.initialized {
            let dummy_barriers = [
                image_barrier(
                    self.dummies.read_f16.image,
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    vk::AccessFlags::empty(),
                    vk::AccessFlags::SHADER_READ,
                ),
                image_barrier(
                    self.dummies.read_u32.image,
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    vk::AccessFlags::empty(),
                    vk::AccessFlags::SHADER_READ,
                ),
                image_barrier(
                    self.dummies.rw_f16.image,
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::GENERAL,
                    vk::AccessFlags::empty(),
                    vk::AccessFlags::SHADER_WRITE,
                ),
                image_barrier(
                    self.dummies.rw_f32.image,
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::GENERAL,
                    vk::AccessFlags::empty(),
                    vk::AccessFlags::SHADER_WRITE,
                ),
                image_barrier(
                    self.dummies.rw_u32.image,
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::GENERAL,
                    vk::AccessFlags::empty(),
                    vk::AccessFlags::SHADER_WRITE,
                ),
                image_barrier(
                    self.dummies.rw_r32f.image,
                    vk::ImageLayout::UNDEFINED,
                    vk::ImageLayout::GENERAL,
                    vk::AccessFlags::empty(),
                    vk::AccessFlags::SHADER_WRITE,
                ),
            ];
            unsafe {
                ctx.device.cmd_pipeline_barrier(
                    ctx.cmd,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &dummy_barriers,
                );
            }
            self.dummies.initialized = true;
        }

        write_full_descriptor_set(
            ctx.device,
            ds_primary,
            slot_bufs,
            forward_color_view,
            forward_depth_view,
            prev_gi_view,
            prev_pn_view,
            self.dummies.read_f16.view,
            self.dummies.read_f16.view,
            self.dummies.read_f16.view,
            self.dummies.read_f16.view,
            self.dummies.read_f16.view,
            self.dummies.read_u32.view,
            self.dummies.rw_f16.view,
            self.dummies.rw_r32f.view,
            target.direct.view,
            target.albedo.view,
            target.spec_env.view,
            target.pos_norm.view,
            self.dummies.rw_f16.view,
            target.glass.view,
        );

        write_full_descriptor_set(
            ctx.device,
            ds_gi,
            slot_bufs,
            forward_color_view,
            forward_depth_view,
            prev_gi_view,
            prev_pn_view,
            self.dummies.read_f16.view,
            target.pos_norm.view,
            self.dummies.read_f16.view,
            self.dummies.read_f16.view,
            target.spec_env.view,
            self.dummies.read_u32.view,
            self.dummies.rw_f16.view,
            self.dummies.rw_r32f.view,
            self.dummies.rw_f16.view,
            self.dummies.rw_f16.view,
            self.dummies.rw_f16.view,
            self.dummies.rw_f32.view,
            target.accum_gi.view,
            self.dummies.rw_u32.view,
        );

        let write_denoise = |ds: vk::DescriptorSet, gi_in: vk::ImageView, gi_out: vk::ImageView| {
            write_full_descriptor_set(
                ctx.device,
                ds,
                slot_bufs,
                forward_color_view,
                forward_depth_view,
                self.dummies.read_f16.view,
                self.dummies.read_f16.view,
                gi_in,
                target.pos_norm.view,
                self.dummies.read_f16.view,
                self.dummies.read_f16.view,
                self.dummies.read_f16.view,
                self.dummies.read_u32.view,
                self.dummies.rw_f16.view,
                self.dummies.rw_r32f.view,
                self.dummies.rw_f16.view,
                self.dummies.rw_f16.view,
                self.dummies.rw_f16.view,
                self.dummies.rw_f32.view,
                gi_out,
                self.dummies.rw_u32.view,
            );
        };
        write_denoise(ds_denoise_0, target.accum_gi.view, target.denoise_a.view);
        write_denoise(ds_denoise_1, target.denoise_a.view, target.denoise_b.view);
        write_denoise(ds_denoise_2, target.denoise_b.view, target.denoise_a.view);

        write_full_descriptor_set(
            ctx.device,
            ds_composite,
            slot_bufs,
            forward_color_view,
            forward_depth_view,
            self.dummies.read_f16.view,
            self.dummies.read_f16.view,
            target.denoise_b.view,
            target.pos_norm.view,
            target.direct.view,
            target.albedo.view,
            target.spec_env.view,
            target.glass.view,
            target.color.view,
            target.depth.view,
            self.dummies.rw_f16.view,
            self.dummies.rw_f16.view,
            self.dummies.rw_f16.view,
            self.dummies.rw_f32.view,
            self.dummies.rw_f16.view,
            self.dummies.rw_u32.view,
        );

        let gx = extent.width.div_ceil(8);
        let gy = extent.height.div_ceil(8);

        let init_old = if target.initialized {
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
        } else {
            vk::ImageLayout::UNDEFINED
        };
        let init_src_stage = if target.initialized {
            vk::PipelineStageFlags::COMPUTE_SHADER | vk::PipelineStageFlags::FRAGMENT_SHADER
        } else {
            vk::PipelineStageFlags::TOP_OF_PIPE
        };
        let init_src_access = if target.initialized {
            vk::AccessFlags::SHADER_READ
        } else {
            vk::AccessFlags::empty()
        };
        let start_barriers: [vk::ImageMemoryBarrier<'_>; 10] = target.all_images().map(|img| {
            image_barrier(
                img,
                init_old,
                vk::ImageLayout::GENERAL,
                init_src_access,
                vk::AccessFlags::SHADER_WRITE,
            )
        });
        unsafe {
            ctx.device.cmd_pipeline_barrier(
                ctx.cmd,
                init_src_stage,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &start_barriers,
            );
        }
        target.initialized = true;

        unsafe {
            ctx.device.cmd_bind_pipeline(
                ctx.cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline_primary.pipeline,
            );
            ctx.device.cmd_bind_descriptor_sets(
                ctx.cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline_primary.layout,
                0,
                &[ds_primary],
                &[],
            );
            ctx.device.cmd_dispatch(ctx.cmd, gx, gy, 1);
        }

        let after_primary = [
            image_barrier(
                target.direct.image,
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::AccessFlags::SHADER_WRITE,
                vk::AccessFlags::SHADER_READ,
            ),
            image_barrier(
                target.albedo.image,
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::AccessFlags::SHADER_WRITE,
                vk::AccessFlags::SHADER_READ,
            ),
            image_barrier(
                target.spec_env.image,
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::AccessFlags::SHADER_WRITE,
                vk::AccessFlags::SHADER_READ,
            ),
            image_barrier(
                target.pos_norm.image,
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::AccessFlags::SHADER_WRITE,
                vk::AccessFlags::SHADER_READ,
            ),
            image_barrier(
                target.glass.image,
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::AccessFlags::SHADER_WRITE,
                vk::AccessFlags::SHADER_READ,
            ),
        ];
        unsafe {
            ctx.device.cmd_pipeline_barrier(
                ctx.cmd,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &after_primary,
            );
        }

        unsafe {
            ctx.device.cmd_bind_pipeline(
                ctx.cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline_gi.pipeline,
            );
            ctx.device.cmd_bind_descriptor_sets(
                ctx.cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline_gi.layout,
                0,
                &[ds_gi],
                &[],
            );
            ctx.device.cmd_dispatch(ctx.cmd, gx, gy, 1);
        }

        let after_gi = [image_barrier(
            target.accum_gi.image,
            vk::ImageLayout::GENERAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            vk::AccessFlags::SHADER_WRITE,
            vk::AccessFlags::SHADER_READ,
        )];
        unsafe {
            ctx.device.cmd_pipeline_barrier(
                ctx.cmd,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &after_gi,
            );
        }

        unsafe {
            ctx.device.cmd_bind_pipeline(
                ctx.cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline_denoise.pipeline,
            );
        }
        let denoise_passes = [
            (1i32, 0u32, ds_denoise_0, target.denoise_a.image, None),
            (
                2i32,
                1u32,
                ds_denoise_1,
                target.denoise_b.image,
                Some(target.denoise_a.image),
            ),
            (
                4i32,
                2u32,
                ds_denoise_2,
                target.denoise_a.image,
                Some(target.denoise_b.image),
            ),
            (
                8i32,
                3u32,
                ds_denoise_1,
                target.denoise_b.image,
                Some(target.denoise_a.image),
            ),
        ];
        for (idx, &(step_size, pass_index, ds, out_img, next_rw)) in
            denoise_passes.iter().enumerate()
        {
            let push = VoxelDenoisePush {
                step_size,
                width: extent.width,
                height: extent.height,
                pass_index,
            };
            unsafe {
                ctx.device.cmd_bind_descriptor_sets(
                    ctx.cmd,
                    vk::PipelineBindPoint::COMPUTE,
                    self.pipeline_denoise.layout,
                    0,
                    &[ds],
                    &[],
                );
                ctx.device.cmd_push_constants(
                    ctx.cmd,
                    self.pipeline_denoise.layout,
                    vk::ShaderStageFlags::COMPUTE,
                    0,
                    bytemuck::bytes_of(&push),
                );
                ctx.device.cmd_dispatch(ctx.cmd, gx, gy, 1);
            }
            if idx + 1 < denoise_passes.len() {
                let b0 = image_barrier(
                    out_img,
                    vk::ImageLayout::GENERAL,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    vk::AccessFlags::SHADER_WRITE,
                    vk::AccessFlags::SHADER_READ,
                );
                if let Some(rw_img) = next_rw {
                    let b1 = image_barrier(
                        rw_img,
                        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        vk::ImageLayout::GENERAL,
                        vk::AccessFlags::SHADER_READ,
                        vk::AccessFlags::SHADER_WRITE,
                    );
                    let pair = [b0, b1];
                    unsafe {
                        ctx.device.cmd_pipeline_barrier(
                            ctx.cmd,
                            vk::PipelineStageFlags::COMPUTE_SHADER,
                            vk::PipelineStageFlags::COMPUTE_SHADER,
                            vk::DependencyFlags::empty(),
                            &[],
                            &[],
                            &pair,
                        );
                    }
                } else {
                    unsafe {
                        ctx.device.cmd_pipeline_barrier(
                            ctx.cmd,
                            vk::PipelineStageFlags::COMPUTE_SHADER,
                            vk::PipelineStageFlags::COMPUTE_SHADER,
                            vk::DependencyFlags::empty(),
                            &[],
                            &[],
                            &[b0],
                        );
                    }
                }
            } else {
                let b_final = image_barrier(
                    out_img,
                    vk::ImageLayout::GENERAL,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    vk::AccessFlags::SHADER_WRITE,
                    vk::AccessFlags::SHADER_READ,
                );
                unsafe {
                    ctx.device.cmd_pipeline_barrier(
                        ctx.cmd,
                        vk::PipelineStageFlags::COMPUTE_SHADER,
                        vk::PipelineStageFlags::COMPUTE_SHADER,
                        vk::DependencyFlags::empty(),
                        &[],
                        &[],
                        &[b_final],
                    );
                }
            }
        }

        unsafe {
            ctx.device.cmd_bind_pipeline(
                ctx.cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline_composite.pipeline,
            );
            ctx.device.cmd_bind_descriptor_sets(
                ctx.cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline_composite.layout,
                0,
                &[ds_composite],
                &[],
            );
            ctx.device.cmd_dispatch(ctx.cmd, gx, gy, 1);
        }

        let end_barriers = [
            image_barrier(
                target.color.image,
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::AccessFlags::SHADER_WRITE,
                vk::AccessFlags::SHADER_READ,
            ),
            image_barrier(
                target.depth.image,
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::AccessFlags::SHADER_WRITE,
                vk::AccessFlags::SHADER_READ,
            ),
        ];
        unsafe {
            ctx.device.cmd_pipeline_barrier(
                ctx.cmd,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::FRAGMENT_SHADER | vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &end_barriers,
            );
        }

        target.history_ready = true;
        self.prev_view_proj = Some(ctx.frame.view_proj);

        resources.set_image_view(VOXEL_COLOR_H, target.color.view);
        resources.set_image(VOXEL_COLOR_H, target.color.image);
        resources.set_image_view(VOXEL_DEPTH_H, target.depth.view);
        resources.set_image(VOXEL_DEPTH_H, target.depth.image);
        Ok(())
    }

    fn graph_info(&self) -> PassInfo {
        PassInfo {
            index: 0,
            name: "VoxelDdaPass".to_string(),
            kind: PassKind::Voxel,
            inputs: vec![FORWARD_COLOR_H, FORWARD_DEPTH_H],
            outputs: vec![VOXEL_COLOR_H, VOXEL_DEPTH_H],
        }
    }
}
