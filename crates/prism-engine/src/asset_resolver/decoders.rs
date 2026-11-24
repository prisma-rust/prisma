//! CPU-side asset decoders and pack loading helpers for [`GpuAssetResolver`].

use prism_asset::core::AssetId;
use prism_asset::formats::{
    RTEX_FORMAT_ASTC_12X12, RTEX_FORMAT_ASTC_4X4, RTEX_FORMAT_ASTC_6X6, RTEX_FORMAT_ASTC_8X8,
    RTEX_FORMAT_BC1, RTEX_FORMAT_BC3, RTEX_FORMAT_BC5, RTEX_FORMAT_BC6H, RTEX_FORMAT_BC7,
    RTEX_FORMAT_ETC2_RGBA8, RTEX_FORMAT_RGBA8,
};
use prism_asset::formats_virtual_geo::{
    decode_octahedral_normal, decode_rcld_page_slice, RcldAsset, RcldCluster, RCLD_MAGIC,
};
use prism_asset::runtime::{
    MaterialAsset, MeshAsset, SkinnedMeshAsset, TextureAsset, VirtualGeoAsset,
};
use prism_render::asset_bridge::MaterialResolveData;
use prism_render::managers::{MeshUploadInput, SkinAttributes, TextureFormat, TextureUploadInput};
use prism_render::virtual_geo::{decode_rcld_runtime, VgMeshDesc, VgPageDesc};

use crate::scene::runtime_components::MeshSubmesh;

use super::{
    report_missing_reference, AtlasSpriteMeta, GpuAssetResolver, LoadedMesh, LoadedSkinnedMesh,
};

/// Map RTEX header `format` byte and slot-bit color space to render-layer [`TextureFormat`].
///
/// RTEX does not encode color space: same RGBA8 payload can be sRGB (albedo / emissive)
/// or linear (normal / MR / AO), caller passes `srgb` per slot. Compressed formats have
/// fixed channel semantics (BC5 is linear dual-channel normal, BC6H is HDR linear). Unsupported returns
/// `None`, caller falls back to magenta.
pub(super) fn texture_format_for(rtex_format: u8, srgb: bool) -> Option<TextureFormat> {
    match rtex_format {
        RTEX_FORMAT_RGBA8 if srgb => Some(TextureFormat::Rgba8Srgb),
        RTEX_FORMAT_RGBA8 => Some(TextureFormat::Rgba8),
        RTEX_FORMAT_BC1 if srgb => Some(TextureFormat::Bc1Srgb),
        RTEX_FORMAT_BC1 => Some(TextureFormat::Bc1Unorm),
        RTEX_FORMAT_BC3 if srgb => Some(TextureFormat::Bc3Srgb),
        RTEX_FORMAT_BC3 => Some(TextureFormat::Bc3Unorm),
        RTEX_FORMAT_BC7 if srgb => Some(TextureFormat::Bc7Srgb),
        RTEX_FORMAT_BC7 => Some(TextureFormat::Bc7Unorm),
        RTEX_FORMAT_BC5 => Some(TextureFormat::Bc5Unorm),
        RTEX_FORMAT_BC6H => Some(TextureFormat::Bc6HUfloat),
        RTEX_FORMAT_ASTC_4X4 if srgb => Some(TextureFormat::Astc4x4Srgb),
        RTEX_FORMAT_ASTC_4X4 => Some(TextureFormat::Astc4x4Unorm),
        RTEX_FORMAT_ASTC_6X6 if srgb => Some(TextureFormat::Astc6x6Srgb),
        RTEX_FORMAT_ASTC_6X6 => Some(TextureFormat::Astc6x6Unorm),
        RTEX_FORMAT_ASTC_8X8 if srgb => Some(TextureFormat::Astc8x8Srgb),
        RTEX_FORMAT_ASTC_8X8 => Some(TextureFormat::Astc8x8Unorm),
        RTEX_FORMAT_ASTC_12X12 if srgb => Some(TextureFormat::Astc12x12Srgb),
        RTEX_FORMAT_ASTC_12X12 => Some(TextureFormat::Astc12x12Unorm),
        RTEX_FORMAT_ETC2_RGBA8 if srgb => Some(TextureFormat::Etc2R8G8B8A8Srgb),
        RTEX_FORMAT_ETC2_RGBA8 => Some(TextureFormat::Etc2R8G8B8A8Unorm),
        _ => None,
    }
}

/// `.rskin` submesh table → render-side material ranges.
pub(super) fn skinned_submeshes(
    info: &prism_asset::formats_skinned::SkinnedMeshInfo,
) -> Vec<MeshSubmesh> {
    info.submeshes
        .iter()
        .map(|submesh| MeshSubmesh {
            index_start: submesh.index_offset,
            index_count: submesh.index_count,
            material_index: if submesh.material_index == u32::MAX {
                None
            } else {
                Some(submesh.material_index)
            },
        })
        .collect()
}

impl GpuAssetResolver {
    /// CPU stage: load mesh asset + deinterleave vertex data → `MeshUploadInput` + submesh table.
    /// CPU stage: decode `.rskin` into vertex data + skinning attributes (#560 P0-3b).
    ///
    /// Vertex layout consistent with static RMES (positions/normals/uv0/tangents/indices), additionally
    /// carries 4 bone indices and weights per vertex — renderer puts them in second vertex buffer.
    pub(super) fn load_skinned_mesh_inputs(&mut self, path: &str) -> Option<LoadedSkinnedMesh> {
        let id = self.resource_manager.id_by_path(path).or_else(|| {
            report_missing_reference(path, "skinned mesh");
            None
        })?;
        let handle = self
            .resource_manager
            .load_with_deps::<SkinnedMeshAsset>(id)
            .map_err(|e| log::warn!("load_skinned_mesh_inputs: load '{path}' failed: {e}"))
            .ok()?;
        let mesh = self
            .resource_manager
            .get(handle)
            .map_err(|e| log::warn!("load_skinned_mesh_inputs: get '{path}' failed: {e}"))
            .ok()?;
        let info = &mesh.info;

        let skin = SkinAttributes {
            joints: info.joints.clone(),
            weights: info.weights.clone(),
        };
        if !skin.matches_vertex_count(info.positions.len()) {
            log::warn!(
                "load_skinned_mesh_inputs: '{path}' has {} skin entries for {} vertices; \
                 drawing it with the bind pose",
                skin.joints.len(),
                info.positions.len()
            );
            return Some(LoadedSkinnedMesh {
                input: MeshUploadInput {
                    positions: info.positions.clone(),
                    normals: info.normals.clone(),
                    colors: Vec::new(),
                    uvs: info.uv0.clone(),
                    tangents: info.tangents.clone(),
                    indices: info.indices.clone(),
                },
                submeshes: skinned_submeshes(info),
                skin: SkinAttributes::default(),
            });
        }

        let submeshes = skinned_submeshes(info);
        Some(LoadedSkinnedMesh {
            input: MeshUploadInput {
                positions: info.positions.clone(),
                normals: info.normals.clone(),
                colors: Vec::new(),
                uvs: info.uv0.clone(),
                tangents: info.tangents.clone(),
                indices: info.indices.clone(),
            },
            submeshes,
            skin,
        })
    }

    pub(super) fn load_mesh_inputs(&mut self, path: &str) -> Option<LoadedMesh> {
        if prism_render::primitives::is_builtin(path) {
            return prism_render::primitives::builtin(path).map(|input| LoadedMesh {
                input,
                submeshes: Vec::new(),
            });
        }

        let id = self.resource_manager.id_by_path(path).or_else(|| {
            report_missing_reference(path, "mesh");
            None
        })?;

        match self.resource_manager.load_with_deps::<MeshAsset>(id) {
            Ok(handle) => {
                let mesh = self
                    .resource_manager
                    .get(handle)
                    .map_err(|e| log::warn!("load_mesh_inputs: get '{path}' failed: {e}"))
                    .ok()?;
                deinterleave_rmes(path, &mesh.info)
                    .map(|(input, submeshes)| LoadedMesh { input, submeshes })
            }
            Err(rmes_err) => {
                let is_rcld = self
                    .resource_manager
                    .read_asset_range(id, 0, RCLD_MAGIC.len())
                    .ok()
                    .as_deref()
                    == Some(RCLD_MAGIC);
                if !is_rcld {
                    log::warn!("load_mesh_inputs: load '{path}' failed: {rmes_err}");
                    return None;
                }
                let handle = self
                    .resource_manager
                    .load_with_deps::<VirtualGeoAsset>(id)
                    .map_err(|e| log::warn!("load_mesh_inputs: load RCLD '{path}' failed: {e}"))
                    .ok()?;
                let vg = self
                    .resource_manager
                    .get(handle)
                    .map_err(|e| log::warn!("load_mesh_inputs: get RCLD '{path}' failed: {e}"))
                    .ok()?;
                dequantize_rcld_root_mesh(&vg.info)
                    .map(|(input, submeshes)| LoadedMesh { input, submeshes })
            }
        }
    }

    /// Loads the runtime Cluster-DAG descriptor (`VgMeshDesc`) for an `RCLD` mesh asset.
    pub fn load_virtual_geo_descriptor(&mut self, path: &str) -> Option<VgMeshDesc> {
        let id = self.resource_manager.id_by_path(path)?;
        let raw = self.resource_manager.load_with_deps_raw(id).ok()?;
        decode_rcld_runtime(id.into_raw(), &raw)
    }

    /// Streams and decodes a single 64 KiB `RCLD` page from disk/package without loading full LOD data.
    pub fn stream_virtual_geo_page(
        &self,
        asset_id: AssetId,
        page: &VgPageDesc,
    ) -> Option<Vec<RcldCluster>> {
        let bytes = self
            .resource_manager
            .read_asset_range(
                asset_id,
                u64::from(page.byte_offset),
                page.byte_size as usize,
            )
            .ok()?;
        decode_rcld_page_slice(&bytes, page.cluster_count)
    }

    /// CPU stage: load material asset → 18 PBR scalars + 5 texture pixels
    /// (order: albedo, normal, metallic_roughness, emissive, occlusion).
    pub(super) fn load_material_data(&mut self, path: &str) -> Option<MaterialResolveData> {
        let id = self.resource_manager.id_by_path(path).or_else(|| {
            report_missing_reference(path, "material");
            None
        })?;

        let handle = self
            .resource_manager
            .load_with_deps::<MaterialAsset>(id)
            .map_err(|e| log::warn!("load_material_data: load '{path}' failed: {e}"))
            .ok()?;
        let mat = self
            .resource_manager
            .get(handle)
            .map_err(|e| log::warn!("load_material_data: get '{path}' failed: {e}"))
            .ok()?;

        let s = mat.scalars();
        let scalars: [f32; 18] = match s.len() {
            n if n >= 18 => {
                let mut a = [0f32; 18];
                a.copy_from_slice(&s[..18]);
                a
            }
            n => {
                log::warn!("load_material_data: expected >=18 scalars, got {n} for '{path}'");
                let mut a = [0f32; 18];
                a[..n.min(18)].copy_from_slice(&s[..n.min(18)]);
                a
            }
        };

        let tex_ids = mat.texture_ids();
        let albedo = self.load_texture_data(tex_ids[0], true);
        let normal = self.load_texture_data(tex_ids[1], false);
        let mr = self.load_texture_data(tex_ids[2], false);
        let emissive = self.load_texture_data(tex_ids[3], true);
        let occlusion = self.load_texture_data(tex_ids[4], false);
        let textures = [albedo, normal, mr, emissive, occlusion];

        let alpha_mode = match mat.alpha_mode() {
            prism_asset::formats::MaterialAlphaMode::Opaque => {
                prism_render::managers::MaterialAlphaMode::Opaque
            }
            prism_asset::formats::MaterialAlphaMode::Mask => {
                prism_render::managers::MaterialAlphaMode::Mask
            }
            prism_asset::formats::MaterialAlphaMode::Blend => {
                prism_render::managers::MaterialAlphaMode::Blend
            }
        };

        Some(MaterialResolveData {
            scalars,
            alpha_mode,
            alpha_cutoff: mat.alpha_cutoff(),
            textures,
        })
    }

    /// Resolve an authoring texture path (`"sprites/foo.png"` or `"atlases/bar.ratlas#baz"`)
    /// into its backing texture path and optional atlas sub-sprite metadata.
    pub(super) fn resolve_texture_target(
        &mut self,
        path: &str,
    ) -> Option<(String, Option<AtlasSpriteMeta>)> {
        if let Some((atlas_path, sprite_name)) = path.split_once('#') {
            if let Some(cached) = self.atlas_cache.get(path) {
                return Some((cached.texture_path.clone(), Some(cached.meta)));
            }
            let atlas_id = self.resource_manager.id_by_path(atlas_path).or_else(|| {
                report_missing_reference(atlas_path, "atlas");
                None
            })?;
            let raw = self
                .resource_manager
                .load_with_deps_raw(atlas_id)
                .map_err(|e| log::warn!("load atlas {atlas_path} failed: {e}"))
                .ok()?;
            let info = prism_asset::formats::decode_ratl(&raw)?;
            let mut found = None;
            for entry in &info.sprites {
                let meta = AtlasSpriteMeta {
                    uv_rect: entry.uv_rect,
                    trimmed_offset: entry.trimmed_offset,
                    original_size: entry.original_size,
                    size: [entry.pixel_rect[2], entry.pixel_rect[3]],
                };
                self.atlas_cache.insert(
                    format!("{atlas_path}#{}", entry.name),
                    super::CachedAtlasEntry {
                        texture_path: info.texture_path.clone(),
                        meta,
                    },
                );
                if entry.name == sprite_name {
                    found = Some(meta);
                }
            }
            let meta = found.or_else(|| {
                let entry = info.find_sprite(sprite_name)?;
                let meta = AtlasSpriteMeta {
                    uv_rect: entry.uv_rect,
                    trimmed_offset: entry.trimmed_offset,
                    original_size: entry.original_size,
                    size: [entry.pixel_rect[2], entry.pixel_rect[3]],
                };
                self.atlas_cache.insert(
                    path.to_string(),
                    super::CachedAtlasEntry {
                        texture_path: info.texture_path.clone(),
                        meta,
                    },
                );
                Some(meta)
            })?;
            return Some((info.texture_path, Some(meta)));
        }
        Some((path.to_string(), None))
    }

    /// Load texture bytes by resolved backing texture path (not an atlas `#` reference).
    pub(super) fn load_texture_by_resolved_path(
        &mut self,
        texture_path: &str,
    ) -> Option<TextureUploadInput> {
        if texture_path == prism_scene::BUILTIN_WHITE_SPRITE {
            return Some(TextureUploadInput {
                width: 1,
                height: 1,
                format: TextureFormat::Rgba8,
                mip_levels: 1,
                pixels: vec![255, 255, 255, 255],
            });
        }
        let id = self.resource_manager.id_by_path(texture_path).or_else(|| {
            report_missing_reference(texture_path, "texture");
            None
        })?;
        self.load_texture_data(Some(id), true)
    }

    #[cfg(test)]
    pub(super) fn load_texture_by_path(
        &mut self,
        entity: prism_ecs::Entity,
        path: &str,
    ) -> Option<TextureUploadInput> {
        let (texture_path, meta) = self.resolve_texture_target(path)?;
        if let Some(meta) = meta {
            self.pending_atlas_sprites.insert(entity, meta);
        }
        self.load_texture_by_resolved_path(&texture_path)
    }

    /// CPU stage: load single texture dependency → `TextureUploadInput` pixels (or magenta fallback).
    ///
    /// `srgb` is **slot bit** color space (RTEX header does not distinguish sRGB/linear): albedo, emissive,
    /// sprite texture `true`, normal, metallic_roughness, occlusion `false`.
    pub(super) fn load_texture_data(
        &mut self,
        tex_id_opt: Option<AssetId>,
        srgb: bool,
    ) -> Option<TextureUploadInput> {
        let tex_id = tex_id_opt?;
        let tex_handle = self
            .resource_manager
            .load_with_deps::<TextureAsset>(tex_id)
            .map_err(|e| log::warn!("load_texture_data: load {tex_id} failed: {e}"))
            .ok()?;
        let tex = self
            .resource_manager
            .get(tex_handle)
            .map_err(|e| log::warn!("load_texture_data: get {tex_id} failed: {e}"))
            .ok()?;

        let mip0 = tex.info.mip_data.first().cloned().unwrap_or_default();
        let placeholder = || TextureUploadInput {
            width: 1,
            height: 1,
            format: TextureFormat::Rgba8,
            mip_levels: 1,
            pixels: vec![128, 128, 128, 255],
        };

        let Some(format) = texture_format_for(tex.info.format, srgb) else {
            log::warn!(
                "load_texture_data: texture {tex_id} uses unsupported RTEX format {}; \
                 using the neutral placeholder",
                tex.info.format
            );
            return Some(placeholder());
        };

        let input = if mip0.is_empty() {
            log::warn!(
                "load_texture_data: texture {tex_id} has no mip 0; using the neutral placeholder"
            );
            placeholder()
        } else {
            let expected = format.compressed_byte_len(tex.info.width, tex.info.height);
            if mip0.len() != expected {
                log::warn!(
                    "load_texture_data: texture {tex_id} mip0 size {} != {}x{} {format:?} ({}); \
                     using the neutral placeholder",
                    mip0.len(),
                    tex.info.width,
                    tex.info.height,
                    expected
                );
                placeholder()
            } else if tex.info.mip_data.len() > 1 {
                let mut chain_valid = true;
                let mut total_len = mip0.len();
                for (i, mip) in tex.info.mip_data.iter().enumerate().skip(1) {
                    let w = (tex.info.width >> i).max(1);
                    let h = (tex.info.height >> i).max(1);
                    let expected_i = format.compressed_byte_len(w, h);
                    if mip.len() != expected_i {
                        log::warn!(
                            "load_texture_data: texture {tex_id} mip {i} size {} != {w}x{h} {format:?} ({expected_i}); \
                             falling back to mip 0 only",
                            mip.len()
                        );
                        chain_valid = false;
                        break;
                    }
                    total_len += mip.len();
                }

                if chain_valid {
                    let mut pixels = Vec::with_capacity(total_len);
                    for mip in &tex.info.mip_data {
                        pixels.extend_from_slice(mip);
                    }
                    TextureUploadInput {
                        width: tex.info.width,
                        height: tex.info.height,
                        format,
                        mip_levels: tex.info.mip_data.len() as u32,
                        pixels,
                    }
                } else {
                    TextureUploadInput {
                        width: tex.info.width,
                        height: tex.info.height,
                        format,
                        mip_levels: 0,
                        pixels: mip0,
                    }
                }
            } else {
                TextureUploadInput {
                    width: tex.info.width,
                    height: tex.info.height,
                    format,
                    mip_levels: 0,
                    pixels: mip0,
                }
            }
        };

        Some(input)
    }
}

/// Decode RMES segmented vertex region into split arrays.
///
/// RMES vertex region is **not** interleaved per vertex, but three contiguous segments:
/// `[positions × vert_count][normals × vert_count][uv0 × vert_count]`.
/// Layout from writer side — `prism-cooker` `GltfImporter::write_intermediate`
/// produces RMXI in this order, `MeshCooker::write_rmes` copies as-is, and writes
/// `pos_off / nrm_off / uv0_off` in same fashion.
///
/// `stride_bytes` only means "total bytes per vertex"; treating it as interleaved row width would read
/// positions after second vertex into normals / uv memory.
pub(super) fn deinterleave_rmes(
    path: &str,
    info: &prism_asset::formats::RmesInfo,
) -> Option<(MeshUploadInput, Vec<MeshSubmesh>)> {
    let stride = info.stride_bytes as usize;
    if stride == 0 || !stride.is_multiple_of(4) {
        log::warn!("load_mesh_inputs: bad stride {stride} for '{path}'");
        return None;
    }
    let float_stride = stride / 4;
    let vert_count = info.vert_count as usize;
    let uv_floats = if info.uv_channels >= 1 { 2 } else { 0 };

    let has_normals = if float_stride == 3 + uv_floats {
        false
    } else if float_stride == 6 + uv_floats {
        true
    } else {
        log::warn!(
            "load_mesh_inputs: stride mismatch for '{path}' \
             (got {float_stride} floats, uv_channels {})",
            info.uv_channels
        );
        return None;
    };

    if info.vertex_data.len() < vert_count * stride {
        log::warn!("load_mesh_inputs: vertex buffer truncated for '{path}'");
        return None;
    }
    if info.index_data.len() < info.idx_count as usize * 4 {
        log::warn!("load_mesh_inputs: index buffer truncated for '{path}'");
        return None;
    }

    let nrm_base = vert_count * 3;
    let uv_base = nrm_base + if has_normals { vert_count * 3 } else { 0 };

    let read = |float_index: usize| -> f32 {
        let off = float_index * 4;
        f32::from_le_bytes(info.vertex_data[off..off + 4].try_into().unwrap())
    };

    let mut positions = Vec::with_capacity(vert_count);
    let mut normals = Vec::with_capacity(vert_count);
    let mut uvs = Vec::with_capacity(vert_count);
    let mut tangents = Vec::with_capacity(vert_count);

    for v in 0..vert_count {
        let p = v * 3;
        positions.push([read(p), read(p + 1), read(p + 2)]);
        if has_normals {
            let n = nrm_base + v * 3;
            normals.push([read(n), read(n + 1), read(n + 2)]);
        } else {
            normals.push([0.0, 0.0, 1.0]);
        }
        if uv_floats == 2 {
            let t = uv_base + v * 2;
            uvs.push([read(t), read(t + 1)]);
        } else {
            uvs.push([0.0, 0.0]);
        }
        tangents.push([1.0, 0.0, 0.0, 1.0]);
    }

    let mut indices = Vec::with_capacity(info.idx_count as usize);
    for i in 0..info.idx_count as usize {
        let off = i * 4;
        indices.push(u32::from_le_bytes(
            info.index_data[off..off + 4].try_into().unwrap(),
        ));
    }

    let submeshes = info
        .submeshes
        .iter()
        .map(|submesh| MeshSubmesh {
            index_start: submesh.index_start,
            index_count: submesh.index_count,
            material_index: submesh.material_index,
        })
        .collect();

    Some((
        MeshUploadInput {
            positions,
            normals,
            colors: vec![],
            uvs,
            tangents,
            indices,
        },
        submeshes,
    ))
}

fn reconstruct_rcld_tangent(normal: [f32; 3], tangent_angle: i8) -> [f32; 4] {
    let up = if normal[2].abs() < 0.999 {
        [0.0, 0.0, 1.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let bx = up[1] * normal[2] - up[2] * normal[1];
    let by = up[2] * normal[0] - up[0] * normal[2];
    let bz = up[0] * normal[1] - up[1] * normal[0];
    let blen = (bx * bx + by * by + bz * bz).sqrt();
    let b1 = if blen > 1e-8 {
        [bx / blen, by / blen, bz / blen]
    } else {
        [1.0, 0.0, 0.0]
    };
    let b2 = [
        normal[1] * b1[2] - normal[2] * b1[1],
        normal[2] * b1[0] - normal[0] * b1[2],
        normal[0] * b1[1] - normal[1] * b1[0],
    ];
    let angle = (f32::from(tangent_angle) / 127.0) * std::f32::consts::PI;
    let (s, c) = angle.sin_cos();
    [
        b1[0] * c + b2[0] * s,
        b1[1] * c + b2[1] * s,
        b1[2] * c + b2[2] * s,
        1.0,
    ]
}

/// Dequantizes the coarsest root-cut clusters of an `RCLD` virtual geometry asset
/// into a standard [`MeshUploadInput`] fallback so non-VG passes render seamlessly.
pub(super) fn dequantize_rcld_root_mesh(
    info: &RcldAsset,
) -> Option<(MeshUploadInput, Vec<MeshSubmesh>)> {
    let root_group = info
        .groups
        .get(info.header.root_group_index as usize)
        .or_else(|| info.groups.first())?;
    let root_lod = root_group.lod_level;

    let mut root_clusters: Vec<&RcldCluster> = Vec::new();
    for group in info.groups.iter().filter(|g| g.lod_level == root_lod) {
        let start = group.first_cluster as usize;
        let end = (start + usize::from(group.cluster_count)).min(info.clusters.len());
        for cluster in &info.clusters[start..end] {
            if !cluster.vertices.is_empty() && !cluster.triangles.is_empty() {
                root_clusters.push(cluster);
            }
        }
    }
    if root_clusters.is_empty() {
        return None;
    }

    root_clusters.sort_by_key(|c| (c.material_index, c.cluster_id));

    let total_verts: usize = root_clusters.iter().map(|c| c.vertices.len()).sum();
    let total_indices: usize = root_clusters.iter().map(|c| c.triangles.len() * 3).sum();
    if total_verts == 0 || total_indices == 0 {
        return None;
    }

    let mut positions = Vec::with_capacity(total_verts);
    let mut normals = Vec::with_capacity(total_verts);
    let mut uvs = Vec::with_capacity(total_verts);
    let mut tangents = Vec::with_capacity(total_verts);
    let mut indices = Vec::with_capacity(total_indices);
    let mut submeshes = Vec::new();

    for cluster in root_clusters {
        let base_vertex = positions.len() as u32;
        for v in &cluster.vertices {
            let pos = v.dequantize_position(
                cluster.aabb_min,
                cluster.aabb_max,
                cluster.geomorph_scale,
                0.0,
            );
            let normal = decode_octahedral_normal(v.normal_oct);
            let uv = v.dequantize_uv();
            let tangent = reconstruct_rcld_tangent(normal, v.tangent_angle);
            positions.push(pos);
            normals.push(normal);
            uvs.push(uv);
            tangents.push(tangent);
        }
        let tri_start = indices.len() as u32;
        for tri in &cluster.triangles {
            indices.push(base_vertex + u32::from(tri[0]));
            indices.push(base_vertex + u32::from(tri[1]));
            indices.push(base_vertex + u32::from(tri[2]));
        }
        let tri_indices = (cluster.triangles.len() * 3) as u32;
        let mat_idx = Some(u32::from(cluster.material_index));
        if let Some(last) = submeshes
            .last_mut()
            .filter(|s: &&mut MeshSubmesh| s.material_index == mat_idx)
        {
            last.index_count += tri_indices;
        } else {
            submeshes.push(MeshSubmesh {
                index_start: tri_start,
                index_count: tri_indices,
                material_index: mat_idx,
            });
        }
    }

    Some((
        MeshUploadInput {
            positions,
            normals,
            colors: vec![],
            uvs,
            tangents,
            indices,
        },
        submeshes,
    ))
}
