//! Asset upload/release, IBL environment, LUT, probe volume, and UI font helpers.

use std::path::Path;

use anyhow::Context as _;
use ash::vk;

use crate::forward_pass::ForwardPass;
use crate::ibl::IblResources;
use crate::managers::{
    AssetTextureHandle, MaterialHandle, MaterialUploadInput, MeshHandle, MeshUploadInput,
    RenderMaterialManager, RenderMeshManager, RenderTextureManager, TextureUploadInput,
};
use crate::mesh::Vertex;
#[cfg(feature = "pathtrace")]
use crate::pt_pass::PathTracePass;
use crate::render_graph::RenderMode;

use super::GraphRenderer;

/// Construct IBL resources. When `ibl` feature disabled always returns 1x1 neutral gray (descriptor layout unchanged,
/// forward pass still binds set 2), HDR decode / convolution / prefilter / BRDF LUT entirely not compiled.
#[cfg(feature = "ibl")]
pub(super) fn make_ibl(
    context: std::sync::Arc<crate::context::VulkanContext>,
    command_pool: ash::vk::CommandPool,
    queue: ash::vk::Queue,
    env: Option<&[u8]>,
    scope: crate::ibl::IblScope,
    layout: ash::vk::DescriptorSetLayout,
) -> anyhow::Result<IblResources> {
    match env {
        Some(bytes) => IblResources::new(
            context,
            command_pool,
            queue,
            Some(bytes.to_vec()),
            scope,
            layout,
        ),
        None => IblResources::new_empty(context, command_pool, queue, layout),
    }
}

#[cfg(not(feature = "ibl"))]
pub(super) fn make_ibl(
    context: std::sync::Arc<crate::context::VulkanContext>,
    command_pool: ash::vk::CommandPool,
    queue: ash::vk::Queue,
    env: Option<&[u8]>,
    scope: crate::ibl::IblScope,
    layout: ash::vk::DescriptorSetLayout,
) -> anyhow::Result<IblResources> {
    let _ = (env, scope);
    IblResources::new_empty(context, command_pool, queue, layout)
}

pub(super) fn env_bytes_fingerprint(bytes: &[u8]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

impl GraphRenderer {
    /// Register a project-bundled font (raw bytes of `.ttf` / `.otf` / `.ttc`, #583).
    ///
    /// Host reads bytes from `.pak` then calls once; name is asset path written in `Text.font`.
    /// UI overlay is lazily created, so here first stores bytes, waits until first frame actually drawing UI
    /// then feeds into atlas; already-created one takes effect immediately.
    pub fn register_ui_font(&mut self, name: impl Into<String>, bytes: Vec<u8>) {
        self.ui_host.register_ui_font(name.into(), bytes);
    }

    /// IBL descriptor set + layout for the environment cubemap (set 2).
    /// Used by PathTracePass and ForwardPass for high dynamic range sky / indirect lighting.
    pub fn ibl_descriptor_set(&self) -> vk::DescriptorSet {
        self.ibl.descriptor_set
    }
    pub fn ibl_descriptor_set_layout(&self) -> vk::DescriptorSetLayout {
        self.ibl.descriptor_set_layout
    }

    /// Rebuild IBL per scene. `env_bytes = Some(data)` accepts offline-baked `PIBL` or raw `.hdr`;
    /// `None` rebuilds as cheap neutral empty IBL (equivalent to `new_empty`). Used when building / removing lighting
    /// on demand at "scene switch", not unconditionally loading 3D IBL/GI at engine startup.
    ///
    /// In `PathTrace` mode only loads `envCube` (skips `irradiance` / `prefiltered` / `BRDF LUT`),
    /// switches back to `Raster` mode upgrades on demand again.
    pub fn set_environment(&mut self, env_bytes: Option<Vec<u8>>) -> anyhow::Result<()> {
        let fp = env_bytes.as_deref().map(env_bytes_fingerprint);
        let want_pbr = self.settings.render_mode == RenderMode::Raster;
        if self.env_fingerprint == Some(fp)
            && (env_bytes.is_none() || self.ibl_has_pbr || !want_pbr)
        {
            return Ok(());
        }

        let scope = if want_pbr {
            crate::ibl::IblScope::Full
        } else {
            crate::ibl::IblScope::EnvOnly
        };
        self.rebuild_ibl_with_scope(env_bytes.as_deref(), scope)?;
        self.env_fingerprint = Some(fp);
        self.env_source = env_bytes;
        Ok(())
    }

    pub(super) fn rebuild_ibl_with_scope(
        &mut self,
        env_bytes: Option<&[u8]>,
        scope: crate::ibl::IblScope,
    ) -> anyhow::Result<()> {
        unsafe { self.runtime.context.device.device_wait_idle() }
            .context("wait idle before replacing IBL")?;

        let new_ibl = make_ibl(
            self.runtime.context.clone(),
            self.runtime.command_pool,
            self.runtime.context.graphics_queue,
            env_bytes,
            scope,
            self.ibl.descriptor_set_layout,
        )
        .context("rebuild IBL resources for scene")?;
        self.texture_manager
            .bindless_mut()
            .register_with_handle(self.brdf_bindless_slot, new_ibl.brdf_image_view())
            .context("overwrite BRDF LUT bindless slot")?;

        if let Some(fp) = self.graph.pass_mut::<ForwardPass>() {
            fp.set_ibl(new_ibl.descriptor_set, self.brdf_bindless_slot);
        }
        #[cfg(feature = "pathtrace")]
        if let Some(pt) = self.graph.pass_mut::<PathTracePass>() {
            pt.set_ibl_resources(new_ibl.descriptor_set, self.ibl.descriptor_set_layout);
        }

        self.ibl.destroy();
        self.ibl = new_ibl;
        self.ibl_has_pbr = env_bytes.is_some() && scope == crate::ibl::IblScope::Full;
        #[cfg(feature = "sharc")]
        {
            self.sharc_invalidation.sky = true;
        }
        Ok(())
    }

    pub fn register_mesh(&mut self, input: &MeshUploadInput) -> anyhow::Result<MeshHandle> {
        self.mesh_manager.register(
            &self.runtime.context,
            self.runtime.command_pool,
            self.runtime.context.graphics_queue,
            input,
        )
    }

    /// Skinning version of `register_mesh` (#560 P0-3): additionally uploads joints/weights
    /// to second vertex buffer, draw item must use `bone_offset` to point to bone palette.
    pub fn register_mesh_skinned(
        &mut self,
        input: &MeshUploadInput,
        skin: &crate::managers::SkinAttributes,
    ) -> anyhow::Result<MeshHandle> {
        self.mesh_manager.register_skinned(
            &self.runtime.context,
            self.runtime.command_pool,
            self.runtime.context.graphics_queue,
            input,
            skin,
        )
    }

    pub fn create_mesh(
        &self,
        vertices: &[Vertex],
        indices: Option<&[u32]>,
    ) -> anyhow::Result<crate::mesh::Mesh> {
        crate::mesh::Mesh::new(
            &self.runtime.context,
            self.runtime.command_pool,
            self.runtime.context.graphics_queue,
            vertices,
            indices,
        )
    }

    pub fn register_mesh_into(
        &mut self,
        uploader: &mut crate::batch::BatchUploader<'_>,
        input: &MeshUploadInput,
    ) -> anyhow::Result<MeshHandle> {
        self.mesh_manager
            .register_into(&self.runtime.context, uploader, input)
    }

    pub fn register_texture(
        &mut self,
        input: &TextureUploadInput,
    ) -> anyhow::Result<AssetTextureHandle> {
        self.texture_manager.reserve(
            &self.runtime.context,
            self.runtime.command_pool,
            self.runtime.context.graphics_queue,
            input,
        )
    }

    pub fn register_texture_into(
        &mut self,
        uploader: &mut crate::batch::BatchUploader<'_>,
        input: &TextureUploadInput,
    ) -> anyhow::Result<AssetTextureHandle> {
        self.texture_manager
            .reserve_into(&self.runtime.context, uploader, input)
    }

    pub fn register_material(
        &mut self,
        input: MaterialUploadInput,
    ) -> anyhow::Result<MaterialHandle> {
        self.material_manager.register(input)
    }

    pub fn texture_srv(&self, handle: AssetTextureHandle) -> crate::bindless::TextureHandle {
        self.texture_manager.get_srv(handle)
    }

    /// Bindless SRV slot of the built-in 1x1 pure-white texture.
    pub fn white_texture_slot(&self) -> u32 {
        self.texture_manager.white_srv().0
    }

    pub fn material_slot(&self, handle: MaterialHandle) -> Option<u32> {
        self.material_manager.slot_of(handle)
    }

    pub fn flush_materials(&mut self) -> anyhow::Result<()> {
        self.material_manager.upload(
            &self.runtime.context,
            self.runtime.command_pool,
            self.runtime.context.graphics_queue,
        )
    }

    pub fn mesh_manager(&self) -> &RenderMeshManager {
        &self.mesh_manager
    }

    /// GPU-side asset parse: execute upload requests prepared by main thread, return handles.
    ///
    /// Called on render thread (owns `&mut self`). Each request independently uploads mesh / texture /
    /// material: texture uploaded first to get bindless slot, then assembles `MaterialUploadInput` to upload material.
    /// Return order consistent with input, caller matches by entity to write back `World`.
    ///
    /// Empty input returns directly, avoiding needlessly creating `BatchUploader` command buffer.
    /// Release a group of GPU resources main thread judged no longer needed (#495).
    ///
    /// On hot reload / entity destroy / component path rewritten, `GpuAssetResolver` returns old slot bits and
    /// handles; here truly destroys images, meshes and material slots, avoiding a VRAM leak per reload.
    /// Already released values (duplicate request, renderer restart) silently ignored.
    pub fn release_assets(&mut self, releases: &[crate::asset_bridge::AssetRelease]) {
        if releases.is_empty() {
            return;
        }
        let device = self.runtime.context.device.clone();
        let mut freed = 0usize;
        for release in releases {
            if let Some(slot) = release.texture_slot {
                if self.texture_manager.unregister_srv(slot, &device) {
                    freed += 1;
                }
            }
            if let Some(handle) = release.mesh_handle {
                self.mesh_manager.unregister(&device, handle);
                freed += 1;
            }
            let mut material_slots = release.material_slots.clone();
            if let Some(slot) = release.material_slot {
                if !material_slots.contains(&slot) {
                    material_slots.push(slot);
                }
            }
            for slot in material_slots {
                if self.material_manager.unregister_slot(slot) {
                    freed += 1;
                }
            }
        }
        log::debug!("release_assets: freed {freed} GPU resource(s)");
        if freed > 0 {
            #[cfg(feature = "sharc")]
            {
                self.sharc_invalidation.geometry = true;
            }
        }
    }

    /// Stage asset upload requests, batch executed by next `prepare` (DESIGN §9 / #1157).
    pub fn stage_asset_requests(
        &mut self,
        requests: Vec<crate::asset_bridge::AssetResolveRequest>,
    ) {
        self.staged_asset_requests.extend(requests);
    }

    /// Stage asset release requests, batch executed by next `prepare` (DESIGN §9 / #1157).
    pub fn stage_asset_releases(&mut self, releases: Vec<crate::asset_bridge::AssetRelease>) {
        self.staged_asset_releases.extend(releases);
    }

    /// In `prepare` phase converge and flush already-staged batch asset release & upload (DESIGN §9 / #1157).
    pub fn flush_staged_assets(&mut self) -> anyhow::Result<()> {
        if !self.staged_asset_releases.is_empty() {
            let releases = std::mem::take(&mut self.staged_asset_releases);
            self.release_assets(&releases);
        }
        if !self.staged_asset_requests.is_empty() {
            let requests = std::mem::take(&mut self.staged_asset_requests);
            let _ = self.apply_asset_requests(requests);
        }
        Ok(())
    }

    /// Register a custom fragment shader SPIR-V module with [`ForwardPass`],
    /// returning a deduplicated [`ShaderHandle`](crate::ShaderHandle).
    pub fn register_custom_shader(&mut self, spv: Vec<u8>) -> anyhow::Result<crate::ShaderHandle> {
        anyhow::ensure!(
            spv.len() >= 20 && spv.len().is_multiple_of(4),
            "invalid SPIR-V bytecode length ({})",
            spv.len()
        );
        let magic = u32::from_le_bytes([spv[0], spv[1], spv[2], spv[3]]);
        anyhow::ensure!(magic == 0x0723_0203, "invalid SPIR-V magic: 0x{magic:08x}");
        let fp = self
            .graph
            .pass_mut::<ForwardPass>()
            .context("GraphRenderer: ForwardPass not registered")?;
        Ok(fp.register_custom_shader(spv))
    }

    pub fn apply_asset_requests(
        &mut self,
        requests: Vec<crate::asset_bridge::AssetResolveRequest>,
    ) -> Vec<crate::asset_bridge::AssetResolveResult> {
        if requests.is_empty() {
            return Vec::new();
        }

        let ctx = self.context_arc();
        let cmd_pool = self.command_pool();
        let mut uploader = match crate::batch::BatchUploader::new(&ctx, cmd_pool) {
            Ok(u) => u,
            Err(e) => {
                log::error!("apply_asset_requests: BatchUploader::new failed: {e}");
                return Vec::new();
            }
        };

        let mut results = Vec::with_capacity(requests.len());
        for req in requests {
            let mesh_handle = match &req.mesh {
                Some(input) => self
                    .mesh_manager
                    .register_skinned_into(
                        &self.runtime.context,
                        &mut uploader,
                        input,
                        req.skin.as_ref(),
                    )
                    .ok(),
                None => None,
            };

            let material_slots: Vec<Option<u32>> = req
                .materials
                .iter()
                .map(|material| {
                    let mat = material.as_ref()?;
                    let mut slots: [Option<u32>; 5] = [None; 5];
                    for (i, tex) in mat.textures.iter().enumerate() {
                        if let Some(t) = tex {
                            if let Ok(h) = self.register_texture_into(&mut uploader, t) {
                                slots[i] = Some(self.texture_srv(h).0);
                            }
                        }
                    }
                    let s = mat.scalars;
                    let input = crate::managers::MaterialUploadInput {
                        base_color: [s[0], s[1], s[2], s[3]],
                        metallic: s[4],
                        roughness: s[5],
                        emissive: [s[6], s[7], s[8]],
                        albedo_tex: slots[0],
                        normal_tex: slots[1],
                        metallic_roughness_tex: slots[2],
                        emissive_tex: slots[3],
                        occlusion_tex: slots[4],
                        normal_scale: s[10],
                        occlusion_strength: s[11],
                        transmission: s[12],
                        ior: s[13],
                        translucency: s[14],
                        anisotropy: s[15],
                        clearcoat: s[16],
                        clearcoat_roughness: s[17],
                        emissive_strength: s[9],
                        alpha_mode: mat.alpha_mode,
                        alpha_cutoff: mat.alpha_cutoff,
                    };
                    self.register_material(input)
                        .ok()
                        .and_then(|h| self.material_slot(h))
                })
                .collect();
            let material_slot = material_slots.iter().find_map(|slot| *slot);

            let (texture_slot, texture_size) = match &req.texture {
                Some(input) => {
                    let size = [input.width, input.height];
                    match self.register_texture_into(&mut uploader, input) {
                        Ok(handle) => (Some(self.texture_srv(handle).0), Some(size)),
                        Err(e) => {
                            log::warn!("apply_asset_requests: sprite texture upload failed: {e}");
                            (None, None)
                        }
                    }
                }
                None => (None, None),
            };

            let shader_handle = match &req.shader {
                Some(spv) => match self.register_custom_shader(spv.clone()) {
                    Ok(handle) => Some(handle),
                    Err(e) => {
                        log::warn!("apply_asset_requests: custom shader registration failed: {e}");
                        None
                    }
                },
                None => None,
            };

            results.push(crate::asset_bridge::AssetResolveResult {
                mesh_handle,
                material_slot,
                material_slots,
                texture_slot,
                texture_size,
                shader_handle,
            });
        }

        if let Err(e) = uploader.finish(self.graphics_queue()) {
            log::error!("apply_asset_requests: BatchUploader::finish failed: {e}");
        }
        if let Err(e) = self.flush_materials() {
            log::warn!("apply_asset_requests: flush_materials failed: {e}");
        }

        results
    }

    /// Read-only access to the texture manager (owns the bindless texture
    /// table). Used by the path-trace pass wiring to fetch the shared bindless
    /// descriptor set + layout.
    pub fn texture_manager(&self) -> &RenderTextureManager {
        &self.texture_manager
    }

    /// Read-only access to the material manager (owns the `GpuMaterial[]`
    /// SSBO). Used by the path-trace pass to bind the materials SSBO.
    pub fn material_manager(&self) -> &RenderMaterialManager {
        &self.material_manager
    }

    /// Load a pre-parsed probe volume data directly (RM or bytes path).
    ///
    /// Returns `true` if the volume was accepted (scene check + validity check
    /// passed, GPU upload succeeded).
    pub fn load_probe_volume_data(
        &mut self,
        data: crate::probe_loader::ProbeVolumeData,
        scene_name: Option<&str>,
    ) -> bool {
        if let Some(name) = scene_name {
            if !name.is_empty() && !data.scene_name.is_empty() && data.scene_name != name {
                log::warn!(
                    "GraphRenderer: baked GI data is for scene '{}', but loaded scene is \
                     '{}'; keeping synthetic sky field (rebake to apply)",
                    data.scene_name,
                    name
                );
                return false;
            }
        }

        if data.global_hit_ratio >= 0.0 && data.global_hit_ratio < 0.05 {
            log::warn!(
                "GraphRenderer: baked GI data looks invalid (hit_ratio={:.3} < 0.05, \
                 all rays missed the TLAS); keeping synthetic sky field",
                data.global_hit_ratio
            );
            return false;
        }

        match self.scene_scope.from_probe_data(&data) {
            Ok(()) => {
                log::info!(
                    "GraphRenderer: loaded baked GI probe volume (dims {:?}, scene='{}', \
                     hit_ratio={:.3})",
                    data.dims,
                    data.scene_name,
                    data.global_hit_ratio
                );
                true
            }
            Err(e) => {
                log::warn!("GraphRenderer: failed to upload baked probe volume: {e:#}");
                false
            }
        }
    }

    /// Load a baked GI probe volume from a `.bin` file on disk.
    ///
    /// Convenience wrapper around [`load_probe_volume_data`] for dev / loose
    /// files.  The RM path should call [`load_probe_volume_data`] directly
    /// with bytes parsed via [`probe_loader::load_probe_volume_from_bytes`].
    pub fn load_probe_volume_file(
        &mut self,
        path: &std::path::Path,
        scene_name: Option<&str>,
    ) -> bool {
        let data = match crate::probe_loader::load_probe_volume(path) {
            Ok(d) => d,
            Err(e) => {
                log::info!(
                    "GraphRenderer: no baked GI at {} ({e}); keeping synthetic sky field",
                    path.display()
                );
                return false;
            }
        };
        self.load_probe_volume_data(data, scene_name)
    }

    /// Load a decoded PRT64 `.prtbin` volume into `PrtPass`.
    #[cfg(feature = "prt")]
    pub fn load_prt_volume(&mut self, volume: crate::prt_volume::PrtVolume) -> bool {
        if let Some(pass) = self.graph.pass_mut::<crate::prt_pass::PrtPass>() {
            pass.load_volume(volume);
            true
        } else {
            false
        }
    }

    /// No-op when compiled without the `prt` feature.
    #[cfg(not(feature = "prt"))]
    pub fn load_prt_volume(&mut self, _volume: crate::prt_volume::PrtVolume) -> bool {
        false
    }

    /// Load a PRT64 `.prtbin` file from disk into `PrtPass`.
    #[cfg(feature = "prt")]
    pub fn load_prt_volume_file(&mut self, path: &std::path::Path) -> bool {
        match crate::prt_volume::PrtVolume::load(path) {
            Ok(vol) => self.load_prt_volume(vol),
            Err(e) => {
                log::info!("GraphRenderer: no PRT volume at {} ({e})", path.display());
                false
            }
        }
    }

    /// No-op when compiled without the `prt` feature.
    #[cfg(not(feature = "prt"))]
    pub fn load_prt_volume_file(&mut self, _path: &std::path::Path) -> bool {
        false
    }

    /// Set internal render resolution (#613): `[0, 0]` = render at window resolution.
    ///
    /// Calling every frame is safe: returns directly when unchanged. After change `ForwardPass` will on next
    /// `execute` detect differing extent and rebuild target (`ensure_target` already does
    /// invalidate check by extent).
    /// Load a `.cube` LUT and bind it to `PostPass` (#502).
    ///
    /// This is the explicit entry point used by tests and tools; the per-frame
    /// path calls it lazily from [`Self::prepare`] when `FrameInput::lut_path`
    /// changes. Parsing/validation errors are returned so callers can report
    /// them; the per-frame path logs and keeps the previous LUT.
    pub fn set_lut_cube(&mut self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read LUT {}", path.display()))?;
        let lut = crate::lut::parse_cube(&text)
            .map_err(|error| anyhow::anyhow!("parse LUT {}: {error}", path.display()))?;
        let context = &self.runtime.context;
        if let Some(post) = self.graph.pass_mut::<crate::post::PostPass>() {
            post.set_lut(context, &lut)?;
        }
        self.applied_lut = Some(path.to_path_buf());
        Ok(())
    }

    /// Lazily apply `FrameInput::lut_path`. A broken/missing file logs once and
    /// keeps rendering with the previous LUT instead of failing the frame.
    pub(super) fn ensure_lut(&mut self, path: Option<&Path>) -> anyhow::Result<()> {
        let Some(path) = path else {
            return Ok(());
        };
        if self.applied_lut.as_deref() == Some(path) {
            return Ok(());
        }
        if let Err(error) = self.set_lut_cube(path) {
            log::warn!(
                "post LUT {} could not be applied: {error:#}",
                path.display()
            );
            self.applied_lut = Some(path.to_path_buf());
        }
        Ok(())
    }
}
